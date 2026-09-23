//! The unread set and the ancestry classification of a review mark (spec 8.3).

use std::collections::BTreeSet;

use crate::engine::{base, Mark, MarkState};

/// Paths whose committed content differs between `mark` and `head`. Rename detection is off:
/// git reports a detected rename by its destination alone, which would leave the deletion of
/// the source -- a row of its own -- unflagged.
pub(crate) async fn unread(
    toplevel: &str,
    mark: &str,
    head: &str,
) -> Result<BTreeSet<String>, String> {
    let output = base::git(
        toplevel,
        &[
            "diff",
            mark,
            head,
            "--name-only",
            "--no-renames",
            "-z",
            "--",
        ],
    )
    .await?;
    if !output.status.success() {
        return Err(format!(
            "git diff --name-only failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output
        .stdout
        .split(|&b| b == 0)
        .filter(|f| !f.is_empty())
        .map(|f| String::from_utf8_lossy(f).into_owned())
        .collect())
}

/// `Ok(true)` on exit 0, `Ok(false)` on exit 1, `Err` on anything else: `128` for a missing
/// object, a signal, or a failure to spawn. The frozen runner returns `Ok` for every status.
pub(crate) async fn is_ancestor(toplevel: &str, mark: &str, head: &str) -> Result<bool, String> {
    let output = base::git(toplevel, &["merge-base", "--is-ancestor", mark, head]).await?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        other => Err(match other {
            Some(code) => format!("merge-base --is-ancestor exited {code}"),
            None => "merge-base --is-ancestor was killed".to_string(),
        }),
    }
}

/// One refresh's answer for a mark: its state and the paths to flag. An empty set with a state
/// other than `Current` means "flag every row"; the caller reads `state`, never the set's size.
pub(crate) async fn classify(
    toplevel: &str,
    record: &base::MarkRecord,
    head: &str,
    previous: Option<&Mark>,
    previous_unread: Option<(&str, &BTreeSet<String>)>,
) -> (Mark, BTreeSet<String>) {
    // Reuse only when both the classification and the set describe this head.
    if let (Some(p), Some((read_at, set))) = (previous, previous_unread) {
        if p.commit == record.commit && read_at == head && p.classified_at.as_deref() == Some(head)
        {
            // Another viewer may have marked the same commit at a later time.
            let mut mark = p.clone();
            mark.at = record.at;
            return (mark, set.clone());
        }
    }
    let mut mark = Mark {
        commit: record.commit.clone(),
        at: record.at,
        state: previous
            .filter(|p| p.commit == record.commit)
            .map(|p| p.state.clone())
            .unwrap_or(MarkState::Current),
        classified_at: previous
            .filter(|p| p.commit == record.commit)
            .and_then(|p| p.classified_at.clone()),
    };
    if record.commit == head {
        mark.state = MarkState::Current;
        mark.classified_at = Some(head.to_string());
        return (mark, BTreeSet::new());
    }
    match is_ancestor(toplevel, &record.commit, head).await {
        Ok(true) => match unread(toplevel, &record.commit, head).await {
            Ok(set) => {
                mark.state = MarkState::Current;
                mark.classified_at = Some(head.to_string());
                (mark, set)
            }
            Err(reason) => {
                mark.state = MarkState::Unreadable(reason);
                mark.classified_at = Some(head.to_string());
                (mark, BTreeSet::new())
            }
        },
        Ok(false) => {
            mark.state = MarkState::Rewritten;
            mark.classified_at = Some(head.to_string());
            (mark, BTreeSet::new())
        }
        // Retry unanswered ancestry unless the diff conclusively fails too.
        Err(_) => match unread(toplevel, &record.commit, head).await {
            Ok(set) => (mark, set),
            Err(reason) => {
                mark.state = MarkState::Unreadable(reason);
                mark.classified_at = Some(head.to_string());
                (mark, BTreeSet::new())
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as Proc;

    fn run(dir: &std::path::Path, args: &[&str]) {
        assert!(
            Proc::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .status()
                .unwrap()
                .success(),
            "git {args:?}"
        );
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn head_of(dir: &std::path::Path, rev: &str) -> String {
        let out = Proc::new("git")
            .arg("-C")
            .arg(dir)
            .args(["rev-parse", rev])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// main, then four commits: a.txt, b.txt, c.txt (which also rewrites a.txt), d.txt.
    fn fixture() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        run(p, &["init", "-q", "-b", "main"]);
        run(p, &["config", "user.email", "t@example.com"]);
        run(p, &["config", "user.name", "t"]);
        for (n, files) in [
            (1, vec![("a.txt", "one\n")]),
            (2, vec![("b.txt", "two\n")]),
            (3, vec![("c.txt", "three\n"), ("a.txt", "one changed\n")]),
            (4, vec![("d.txt", "four\n")]),
        ] {
            for (name, body) in files {
                std::fs::write(p.join(name), body).unwrap();
            }
            run(p, &["add", "-A"]);
            run(p, &["commit", "-q", "-m", &format!("c{n}")]);
        }
        let toplevel = p.canonicalize().unwrap().to_string_lossy().into_owned();
        (dir, toplevel)
    }

    #[test]
    fn unread_is_the_net_difference_between_two_commits() {
        let (dir, top) = fixture();
        let second = head_of(dir.path(), "HEAD~2");
        let head = head_of(dir.path(), "HEAD");
        let set = rt().block_on(unread(&top, &second, &head)).unwrap();
        let mut names: Vec<&str> = set.iter().map(String::as_str).collect();
        names.sort();
        assert_eq!(names, ["a.txt", "c.txt", "d.txt"], "b.txt was already read");

        // A path changed and then restored carries no dot: this is a net difference.
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        run(dir.path(), &["add", "-A"]);
        run(dir.path(), &["commit", "-q", "-m", "restore a"]);
        let head = head_of(dir.path(), "HEAD");
        let set = rt().block_on(unread(&top, &second, &head)).unwrap();
        assert!(!set.contains("a.txt"), "{set:?}");
        assert!(set.contains("c.txt") && set.contains("d.txt"));

        // The set is empty against itself, which is what M leaves behind.
        assert!(rt()
            .block_on(unread(&top, &head, &head))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn rename_detection_is_off_so_both_endpoints_appear() {
        let (dir, top) = fixture();
        let before = head_of(dir.path(), "HEAD");
        run(dir.path(), &["mv", "a.txt", "renamed.txt"]);
        run(dir.path(), &["commit", "-q", "-m", "rename"]);
        let head = head_of(dir.path(), "HEAD");
        let set = rt().block_on(unread(&top, &before, &head)).unwrap();
        let mut names: Vec<&str> = set.iter().map(String::as_str).collect();
        names.sort();
        assert_eq!(
            names,
            ["a.txt", "renamed.txt"],
            "both endpoints, not just the destination"
        );
    }

    #[test]
    fn ancestry_answers_only_on_exit_zero_and_one() {
        let (dir, top) = fixture();
        let head = head_of(dir.path(), "HEAD");
        let second = head_of(dir.path(), "HEAD~2");
        assert_eq!(rt().block_on(is_ancestor(&top, &second, &head)), Ok(true));
        run(dir.path(), &["switch", "-q", "-c", "side", "HEAD~1"]);
        std::fs::write(dir.path().join("side.txt"), "side\n").unwrap();
        run(dir.path(), &["add", "-A"]);
        run(dir.path(), &["commit", "-q", "-m", "side"]);
        let side = head_of(dir.path(), "HEAD");
        assert_eq!(rt().block_on(is_ancestor(&top, &head, &side)), Ok(false));
        // A commit that does not exist is an error, not a false answer.
        assert!(rt()
            .block_on(is_ancestor(&top, &"b".repeat(40), &side))
            .is_err());
    }

    #[test]
    fn an_answered_pair_is_not_asked_again() {
        let (dir, top) = fixture();
        let head = head_of(dir.path(), "HEAD");
        let second = head_of(dir.path(), "HEAD~2");
        let record = base::MarkRecord {
            commit: second,
            at: 1,
        };
        let (mark, set) = rt().block_on(classify(&top, &record, &head, None, None));
        assert!(set.contains("c.txt"));
        // A cached pair needs no repository access.
        let cached = rt().block_on(classify(
            "/nonexistent-toplevel",
            &record,
            &head,
            Some(&mark),
            Some((head.as_str(), &set)),
        ));
        assert_eq!(cached.0, mark);
        assert_eq!(cached.1, set);

        // Returning from worktree scope requires a fresh set.
        let (again, set_again) = rt().block_on(classify(&top, &record, &head, Some(&mark), None));
        assert_eq!(again.state, MarkState::Current);
        assert_eq!(set_again, set);

        // A newer record for the same commit keeps the classification and takes its time.
        let newer = base::MarkRecord {
            commit: record.commit.clone(),
            at: record.at + 60,
        };
        let (fresh, _) = rt().block_on(classify(
            "/nonexistent-toplevel",
            &newer,
            &head,
            Some(&mark),
            Some((head.as_str(), &set)),
        ));
        assert_eq!(fresh.at, newer.at);

        // A set from another head cannot supply the current dots.
        let earlier = head_of(dir.path(), "HEAD~1");
        let (other, other_set) = rt().block_on(classify(&top, &record, &earlier, None, None));
        assert_ne!(other_set, set, "the two heads have different unread sets");
        let (back, back_set) = rt().block_on(classify(
            &top,
            &record,
            &head,
            Some(&other),
            Some((earlier.as_str(), &other_set)),
        ));
        assert_eq!(
            back_set, set,
            "the set for this head is recomputed, not carried over"
        );
        assert_eq!(back.classified_at.as_deref(), Some(head.as_str()));
    }

    #[test]
    fn an_unanswered_ancestry_keeps_the_previous_classification_and_retries() {
        let (dir, top) = fixture();
        let head = head_of(dir.path(), "HEAD");
        let second = head_of(dir.path(), "HEAD~2");
        // A tree ID makes ancestry fail while its diff still succeeds.
        let tree = head_of(dir.path(), "HEAD~2^{tree}");
        assert!(rt().block_on(is_ancestor(&top, &tree, &head)).is_err());
        assert!(rt()
            .block_on(unread(&top, &tree, &head))
            .unwrap()
            .contains("c.txt"));

        let record = base::MarkRecord {
            commit: tree.clone(),
            at: 1,
        };
        let previous = Mark {
            commit: tree.clone(),
            at: 1,
            state: MarkState::Current,
            classified_at: Some(second.clone()),
        };
        let (mark, set) = rt().block_on(classify(&top, &record, &head, Some(&previous), None));
        assert_eq!(
            mark.state,
            MarkState::Current,
            "the previous classification survives"
        );
        assert_eq!(
            mark.classified_at.as_deref(),
            Some(second.as_str()),
            "the pair is left undated, so nothing warns about an answer nobody gave"
        );
        assert!(
            set.contains("c.txt"),
            "the rows come from the read that did answer"
        );

        // An unclassified pair must retry even with a set read at this head.
        let (again, set_again) = rt().block_on(classify(
            &top,
            &record,
            &head,
            Some(&mark),
            Some((head.as_str(), &set)),
        ));
        assert_eq!(again.classified_at.as_deref(), Some(second.as_str()));
        assert_eq!(set_again, set);

        // Returning to the classified head still requires its own set.
        let (at_old, set_at_old) = rt().block_on(classify(
            &top,
            &record,
            &second,
            Some(&mark),
            Some((head.as_str(), &set)),
        ));
        assert_eq!(
            set_at_old,
            rt().block_on(unread(&top, &tree, &second)).unwrap(),
            "the older head's own rows, not the newer head's"
        );
        assert!(matches!(at_old.state, MarkState::Current));
    }

    #[test]
    fn classify_reports_current_rewritten_and_unreadable() {
        let (dir, top) = fixture();
        let head = head_of(dir.path(), "HEAD");
        let second = head_of(dir.path(), "HEAD~2");
        let record = base::MarkRecord {
            commit: second.clone(),
            at: 1,
        };

        let (mark, set) = rt().block_on(classify(&top, &record, &head, None, None));
        assert_eq!(mark.state, MarkState::Current);
        assert_eq!(mark.classified_at.as_deref(), Some(head.as_str()));
        assert!(set.contains("c.txt") && !set.contains("b.txt"));

        // Rewritten returns no set; the caller flags every row from the state.
        run(dir.path(), &["switch", "-q", "-c", "side", "HEAD~1"]);
        std::fs::write(dir.path().join("side.txt"), "side\n").unwrap();
        run(dir.path(), &["add", "-A"]);
        run(dir.path(), &["commit", "-q", "-m", "side"]);
        let side = head_of(dir.path(), "HEAD");
        let record = base::MarkRecord {
            commit: head.clone(),
            at: 1,
        };
        let (mark, set) = rt().block_on(classify(&top, &record, &side, None, None));
        assert_eq!(mark.state, MarkState::Rewritten);
        assert!(set.is_empty());

        // A missing object yields a dated Unreadable answer.
        let gone = base::MarkRecord {
            commit: "b".repeat(40),
            at: 1,
        };
        let previous = Mark {
            commit: gone.commit.clone(),
            at: 1,
            state: MarkState::Current,
            classified_at: Some("old".into()),
        };
        let (mark, set) = rt().block_on(classify(&top, &gone, &side, Some(&previous), None));
        assert!(matches!(mark.state, MarkState::Unreadable(_)));
        assert_eq!(
            mark.classified_at.as_deref(),
            Some(side.as_str()),
            "a dated answer, so it warns"
        );
        assert!(set.is_empty());

        // A mark equal to the head needs no command and is Current.
        let same = base::MarkRecord {
            commit: side.clone(),
            at: 1,
        };
        let (mark, set) = rt().block_on(classify(&top, &same, &side, None, None));
        assert_eq!(mark.state, MarkState::Current);
        assert!(set.is_empty());
    }
}
