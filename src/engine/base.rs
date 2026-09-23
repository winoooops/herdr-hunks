//! Base resolution, validation, remembered picks and the picker's ref list (spec 7.3, 7.4).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::actions::reuse;
use crate::engine::{Base, BaseSource};
use crate::git::run_git_with_timeout;

pub const REFS_CAP: usize = 200;
const PICKS_FILE: &str = "bases.json";

/// One git invocation in `toplevel` through the frozen runner (30 s timeout; the D3 variables are process-wide).
pub(crate) async fn git(toplevel: &str, args: &[&str]) -> Result<std::process::Output, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(toplevel)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0");
    run_git_with_timeout(cmd).await
}

fn stderr_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_string()
}

/// A base that starts with `-` could become an option; refused before git sees it.
pub fn check_text(text: &str) -> Result<(), String> {
    if text.is_empty() || text.starts_with('-') {
        return Err(format!("not a commit: {text}"));
    }
    Ok(())
}

/// `git rev-parse --verify --quiet <text>^{commit}`: the object id, or `not a commit: <text>`.
pub(crate) async fn verify(toplevel: &str, text: &str) -> Result<String, String> {
    check_text(text)?;
    let spec = format!("{text}^{{commit}}");
    let output = git(toplevel, &["rev-parse", "--verify", "--quiet", &spec]).await?;
    if !output.status.success() {
        return Err(format!("not a commit: {text}"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// `git merge-base HEAD <commit>`; fails for unrelated histories.
pub(crate) async fn merge_base(toplevel: &str, commit: &str) -> Result<String, String> {
    let output = git(toplevel, &["merge-base", "HEAD", commit]).await?;
    if !output.status.success() {
        let short = &commit[..commit.len().min(7)];
        let err = stderr_of(&output);
        return Err(if err.is_empty() {
            format!("no merge-base with {short}")
        } else {
            format!("no merge-base with {short}: {err}")
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// What resolution reads besides the repository.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolveInputs {
    /// `Some(Some(ref))` a pick, `Some(None)` a reset, kept in memory when `bases.json` could not be written.
    pub session_pick: Option<Option<String>>,
    /// `[base] ref`.
    pub config: Option<String>,
    pub state_dir: Option<PathBuf>,
}

pub struct Resolution {
    pub base: Option<Base>,
    /// The first of steps 2-5 that names a commit: what the reset row shows.
    pub default: Option<String>,
    /// Steps that were skipped, each with its reason.
    pub skipped: Vec<String>,
}

/// Steps 1-5 of spec 7.3; `merge_base` is left `None` for the caller to fill in branch scope.
pub(crate) async fn resolve(toplevel: &str, inputs: &ResolveInputs) -> Resolution {
    let mut skipped = Vec::new();
    let pick = match &inputs.session_pick {
        Some(pick) => pick.clone(),
        None => inputs.state_dir.as_deref().and_then(|dir| {
            let (picks, problem) = load_picks(dir);
            if let Some(problem) = problem {
                note_problem(dir, &problem);
                skipped.push(problem);
            }
            picks.get(toplevel).cloned()
        }),
    };
    let mut picked = None;
    if let Some(text) = pick {
        match verify(toplevel, &text).await {
            Ok(commit) => {
                picked = Some(Base {
                    requested: text,
                    commit,
                    merge_base: None,
                    source: BaseSource::Picked,
                })
            }
            Err(e) => skipped.push(format!("remembered pick: {e}")),
        }
    }
    let mut default = None;
    if let Some(text) = &inputs.config {
        match verify(toplevel, text).await {
            Ok(commit) => {
                default = Some(Base {
                    requested: text.clone(),
                    commit,
                    merge_base: None,
                    source: BaseSource::Config,
                })
            }
            Err(e) => skipped.push(format!("[base] ref: {e}")),
        }
    }
    if default.is_none() {
        let mut candidates = vec!["refs/heads/main".to_string()];
        if let Ok(output) = git(
            toplevel,
            &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
        )
        .await
        {
            if output.status.success() {
                let target = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !target.is_empty() {
                    candidates.push(target);
                }
            }
        }
        candidates.push("refs/heads/master".to_string());
        for text in candidates {
            if let Ok(commit) = verify(toplevel, &text).await {
                default = Some(Base {
                    requested: text,
                    commit,
                    merge_base: None,
                    source: BaseSource::Default,
                });
                break;
            }
        }
    }
    Resolution {
        default: default.as_ref().map(|b| b.requested.clone()),
        base: picked.or(default),
        skipped,
    }
}

/// The remembered picks, keyed by canonical toplevel; empty with the reason when unreadable or malformed.
pub fn load_picks(state_dir: &Path) -> (BTreeMap<String, String>, Option<String>) {
    match std::fs::read_to_string(state_dir.join(PICKS_FILE)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (BTreeMap::new(), None),
        Err(e) => (BTreeMap::new(), Some(format!("{PICKS_FILE}: {e}"))),
        Ok(text) => match serde_json::from_str(&text) {
            Ok(picks) => (picks, None),
            Err(e) => (BTreeMap::new(), Some(format!("{PICKS_FILE}: {e}"))),
        },
    }
}

/// Read-modify-write under the split-panes lock, then an atomic replace; `None` removes the entry.
pub fn save_pick(state_dir: &Path, toplevel: &str, pick: Option<&str>) -> std::io::Result<()> {
    if !state_dir.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "state directory must be absolute",
        ));
    }
    reuse::with_lock(state_dir, || {
        let (mut picks, _) = load_picks(state_dir);
        match pick {
            Some(text) => {
                picks.insert(toplevel.to_string(), text.to_string());
            }
            None => {
                picks.remove(toplevel);
            }
        }
        let tmp = state_dir.join(format!("{PICKS_FILE}.{}.tmp", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec_pretty(&picks).unwrap_or_default())?;
        std::fs::rename(tmp, state_dir.join(PICKS_FILE))
    })?
}

/// One line appended to `config-problems.log`, the file the shell uses for config problems.
pub fn note_problem(state_dir: &Path, line: &str) {
    use std::io::Write;
    if !state_dir.is_absolute() {
        return;
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state_dir.join("config-problems.log"))
    {
        let _ = writeln!(file, "{line}");
    }
}

/// Qualified refs, most recently created first, symbolic entries removed, capped at `REFS_CAP`.
pub(crate) async fn list_refs(toplevel: &str) -> Result<(Vec<String>, bool), String> {
    let output = git(
        toplevel,
        &[
            "for-each-ref",
            "--sort=-creatordate",
            "--format=%(refname)%00%(symref)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ],
    )
    .await?;
    if !output.status.success() {
        return Err(format!("for-each-ref failed: {}", stderr_of(&output)));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut refs: Vec<String> = text
        .lines()
        .filter_map(|line| {
            let (name, symref) = line.split_once('\0')?;
            (symref.is_empty() && !name.is_empty()).then(|| name.to_string())
        })
        .collect();
    let overflow = refs.len() > REFS_CAP;
    refs.truncate(REFS_CAP);
    Ok((refs, overflow))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::BaseSource;
    use std::process::Command as Proc;

    fn run(dir: &std::path::Path, args: &[&str]) {
        let status = Proc::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
            .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    /// `main` with one commit; returns the canonical toplevel.
    fn repo(branch: &str) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        run(p, &["init", "-q", "-b", branch]);
        run(p, &["config", "user.email", "t@example.com"]);
        run(p, &["config", "user.name", "t"]);
        std::fs::write(p.join("a.txt"), "one\n").unwrap();
        run(p, &["add", "-A"]);
        run(p, &["commit", "-q", "-m", "init"]);
        let toplevel = p.canonicalize().unwrap().to_string_lossy().into_owned();
        (dir, toplevel)
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn option_like_text_is_refused_before_git_runs() {
        assert_eq!(check_text("-x"), Err("not a commit: -x".into()));
        assert_eq!(
            check_text("--output=x"),
            Err("not a commit: --output=x".into())
        );
        assert_eq!(check_text(""), Err("not a commit: ".into()));
        assert_eq!(check_text("HEAD~1"), Ok(()));
        // No repository is needed for the refusal, so no git can have been spawned.
        let error = rt().block_on(verify("/nonexistent", "-x")).unwrap_err();
        assert_eq!(error, "not a commit: -x");
    }

    #[test]
    fn verify_prints_the_object_id_or_names_the_text() {
        let (_dir, top) = repo("main");
        let id = rt().block_on(verify(&top, "refs/heads/main")).unwrap();
        assert_eq!(id.len(), 40);
        assert_eq!(rt().block_on(verify(&top, "HEAD")).unwrap(), id);
        assert_eq!(
            rt().block_on(verify(&top, "nope")).unwrap_err(),
            "not a commit: nope"
        );
        assert_eq!(rt().block_on(merge_base(&top, &id)).unwrap(), id);
    }

    #[test]
    fn resolution_takes_main_then_origin_head_then_master() {
        let (_dir, top) = repo("main");
        let none = ResolveInputs::default();
        let r = rt().block_on(resolve(&top, &none));
        let base = r.base.unwrap();
        assert_eq!(base.requested, "refs/heads/main");
        assert_eq!(base.source, BaseSource::Default);
        assert_eq!(base.merge_base, None);
        assert_eq!(r.default.as_deref(), Some("refs/heads/main"));
        assert!(r.skipped.is_empty());

        let (dir, top) = repo("trunk");
        run(
            dir.path(),
            &["update-ref", "refs/remotes/origin/trunk", "HEAD"],
        );
        run(
            dir.path(),
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/trunk",
            ],
        );
        let r = rt().block_on(resolve(&top, &none));
        assert_eq!(r.base.unwrap().requested, "refs/remotes/origin/trunk");

        let (_dir, top) = repo("master");
        let r = rt().block_on(resolve(&top, &none));
        assert_eq!(r.base.unwrap().requested, "refs/heads/master");

        let (_dir, top) = repo("dev");
        let r = rt().block_on(resolve(&top, &none));
        assert!(r.base.is_none() && r.default.is_none() && r.skipped.is_empty());
    }

    #[test]
    fn config_and_picks_come_first_and_bad_values_are_skipped_with_a_reason() {
        let (dir, top) = repo("main");
        run(dir.path(), &["branch", "feat"]);
        run(dir.path(), &["tag", "v1"]);
        let state = tempfile::tempdir().unwrap();
        let config = ResolveInputs {
            config: Some("feat".into()),
            state_dir: Some(state.path().to_path_buf()),
            ..Default::default()
        };
        let r = rt().block_on(resolve(&top, &config));
        let base = r.base.unwrap();
        assert_eq!(
            (base.requested.as_str(), base.source),
            ("feat", BaseSource::Config)
        );
        assert_eq!(r.default.as_deref(), Some("feat"));

        save_pick(state.path(), &top, Some("refs/tags/v1")).unwrap();
        let r = rt().block_on(resolve(&top, &config));
        let base = r.base.unwrap();
        assert_eq!(
            (base.requested.as_str(), base.source),
            ("refs/tags/v1", BaseSource::Picked)
        );
        assert_eq!(
            r.default.as_deref(),
            Some("feat"),
            "the reset row names steps 2-5"
        );

        // A session override beats the file; a reset override ignores it.
        let over = ResolveInputs {
            session_pick: Some(Some("refs/heads/main".into())),
            ..config.clone()
        };
        assert_eq!(
            rt().block_on(resolve(&top, &over)).base.unwrap().requested,
            "refs/heads/main"
        );
        let reset = ResolveInputs {
            session_pick: Some(None),
            ..config.clone()
        };
        assert_eq!(
            rt().block_on(resolve(&top, &reset)).base.unwrap().requested,
            "feat"
        );

        // Bad values are skipped, in order, and the reason is kept.
        save_pick(state.path(), &top, Some("gone")).unwrap();
        let bad = ResolveInputs {
            config: Some("-x".into()),
            ..config.clone()
        };
        let r = rt().block_on(resolve(&top, &bad));
        assert_eq!(r.base.unwrap().requested, "refs/heads/main");
        assert_eq!(
            r.skipped,
            vec![
                "remembered pick: not a commit: gone".to_string(),
                "[base] ref: not a commit: -x".to_string()
            ]
        );
    }

    #[test]
    fn picks_round_trip_are_removed_and_survive_a_malformed_file() {
        let state = tempfile::tempdir().unwrap();
        assert_eq!(load_picks(state.path()), (Default::default(), None));
        save_pick(state.path(), "/r/one", Some("refs/heads/x")).unwrap();
        save_pick(state.path(), "/r/two", Some("HEAD~2")).unwrap();
        let (picks, problem) = load_picks(state.path());
        assert_eq!(
            picks.get("/r/one").map(String::as_str),
            Some("refs/heads/x")
        );
        assert_eq!(picks.get("/r/two").map(String::as_str), Some("HEAD~2"));
        assert!(problem.is_none());
        save_pick(state.path(), "/r/one", None).unwrap();
        assert!(!load_picks(state.path()).0.contains_key("/r/one"));
        std::fs::write(state.path().join("bases.json"), "{ not json").unwrap();
        let (picks, problem) = load_picks(state.path());
        assert!(picks.is_empty());
        assert!(problem.unwrap().starts_with("bases.json: "));
        save_pick(state.path(), "/r/three", Some("v1")).unwrap();
        assert_eq!(
            load_picks(state.path()).0.len(),
            1,
            "the next pick rewrites it"
        );
        let mut names: Vec<_> = std::fs::read_dir(state.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            ["bases.json", "split-panes.lock"],
            "no temp file is left behind"
        );
        note_problem(state.path(), "bases.json: bad");
        assert_eq!(
            std::fs::read_to_string(state.path().join("config-problems.log")).unwrap(),
            "bases.json: bad\n"
        );
        let relative = tempfile::tempdir_in(".").unwrap();
        let path = relative
            .path()
            .strip_prefix(std::env::current_dir().unwrap())
            .unwrap();
        let error = save_pick(path, "/r/one", Some("main")).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        note_problem(path, "no relative log");
        assert!(std::fs::read_dir(relative.path()).unwrap().next().is_none());
    }

    #[test]
    fn refs_are_listed_newest_first_without_symrefs_and_capped() {
        let (dir, top) = repo("main");
        let p = dir.path();
        run(p, &["tag", "old-tag"]);
        std::fs::write(p.join("a.txt"), "two\n").unwrap();
        run(p, &["add", "-A"]);
        let later = Proc::new("git")
            .arg("-C")
            .arg(p)
            .args(["commit", "-q", "-m", "later"])
            .env("GIT_COMMITTER_DATE", "2026-06-01T00:00:00Z")
            .env("GIT_AUTHOR_DATE", "2026-06-01T00:00:00Z")
            .status()
            .unwrap();
        assert!(later.success());
        run(p, &["branch", "feat"]);
        run(p, &["update-ref", "refs/remotes/origin/main", "HEAD~1"]);
        run(
            p,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
        let (refs, overflow) = rt().block_on(list_refs(&top)).unwrap();
        assert!(!overflow);
        assert!(
            !refs.iter().any(|r| r == "refs/remotes/origin/HEAD"),
            "{refs:?}"
        );
        let pos = |name: &str| {
            refs.iter()
                .position(|r| r == name)
                .unwrap_or_else(|| panic!("{name} in {refs:?}"))
        };
        assert!(pos("refs/heads/feat") < pos("refs/tags/old-tag"));
        assert!(pos("refs/heads/main") < pos("refs/remotes/origin/main"));
        for i in 0..(REFS_CAP + 5) {
            run(p, &["update-ref", &format!("refs/tags/t{i:03}"), "HEAD"]);
        }
        let (refs, overflow) = rt().block_on(list_refs(&top)).unwrap();
        assert_eq!(refs.len(), REFS_CAP);
        assert!(overflow);
        // A repository with no commits has no refs at all.
        let empty = tempfile::tempdir().unwrap();
        run(empty.path(), &["init", "-q", "-b", "main"]);
        let top = empty
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let (refs, overflow) = rt().block_on(list_refs(&top)).unwrap();
        assert!(refs.is_empty() && !overflow);
    }
}
