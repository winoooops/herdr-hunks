//! Worktree scope's diffs, built by the engine (spec 9.2, D7): bytes an action can apply back.

use std::collections::BTreeMap;
use std::hash::Hasher;

use crate::engine::base;
use crate::engine::branch::parse_name_status;
use crate::engine::sections::{keep_sections, sections};
use crate::engine::{FileKey, PreImage, WorktreeKind};
use crate::git::{parse_git_diff, validate_file_path, FileDiff, GetGitDiffResponse};

const DIFF_FLAGS: [&str; 6] = [
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "-U3",
    "--src-prefix=a/",
    "--dst-prefix=b/",
];

/// SipHash with fixed keys: stable within a process, which is all the pre-image needs.
pub fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hasher.write(bytes);
    hasher.finish()
}

const HASH_BUFFER: usize = 64 * 1024;

/// The same hash as `hash_bytes` of the file's contents, read through a bounded buffer: a large
/// file never costs its size in memory for a pre-image.
pub fn hash_file(path: &std::path::Path) -> std::io::Result<u64> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut buffer = vec![0u8; HASH_BUFFER];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(hasher.finish());
        }
        hasher.write(&buffer[..read]);
    }
}

fn parse_sections(patch: &[u8], path: &str) -> FileDiff {
    let mut file_diff = FileDiff {
        file_path: path.to_string(),
        old_path: None,
        new_path: None,
        hunks: Vec::new(),
    };
    for section in sections(patch) {
        let parsed = parse_git_diff(&String::from_utf8_lossy(section), path);
        file_diff.old_path = file_diff.old_path.or(parsed.old_path);
        file_diff.new_path = file_diff.new_path.or(parsed.new_path);
        file_diff.hunks.extend(parsed.hunks);
    }
    file_diff
}

fn response(toplevel: &str, path: &str, patch: &[u8]) -> GetGitDiffResponse {
    GetGitDiffResponse {
        file_diff: parse_sections(patch, path),
        old_text: String::new(),
        new_text: String::new(),
        raw_diff: String::from_utf8_lossy(patch).into_owned(),
        repo_root: toplevel.to_string(),
    }
}

/// `git diff [--cached] <flags> [-M] -- [<old>] <path>`, or `--no-index -- /dev/null <path>` for an untracked row.
pub(crate) async fn diff(
    toplevel: &str,
    key: &FileKey,
    old: Option<&str>,
) -> Result<(GetGitDiffResponse, Vec<u8>), String> {
    validate_file_path(&key.path)?;
    if let Some(old) = old {
        validate_file_path(old)?;
    }
    let mut args: Vec<&str> = vec!["diff"];
    if key.untracked {
        args.push("--no-index");
        args.extend(DIFF_FLAGS);
        args.extend(["--", "/dev/null", &key.path]);
    } else {
        if key.staged {
            args.push("--cached");
        }
        args.extend(DIFF_FLAGS);
        if old.is_some() {
            args.push("-M");
        }
        args.push("--");
        if let Some(old) = old {
            args.push(old);
        }
        args.push(&key.path);
    }
    let output = base::git(toplevel, &args).await?;
    // `--no-index` exits 1 when the sides differ, which here they always do.
    let ok = output.status.success() || (key.untracked && output.status.code() == Some(1));
    if !ok {
        return Err(format!(
            "git diff failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let patch = keep_sections(&output.stdout, &key.path, old);
    Ok((response(toplevel, &key.path, &patch), patch))
}

/// `<mode> <object id> <stage>` of the index entry at exactly `path`, or `None`.
async fn index_entry(toplevel: &str, path: &str) -> Result<Option<String>, String> {
    validate_file_path(path)?;
    let listed = base::git(toplevel, &["ls-files", "-s", "-z", "--", path]).await?;
    if !listed.status.success() {
        return Err(format!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&listed.stderr).trim()
        ));
    }
    // A pathspec matches its descendants too: only the record for this very path counts.
    Ok(String::from_utf8_lossy(&listed.stdout)
        .split('\0')
        .filter_map(|record| record.split_once('\t'))
        .find(|(_, name)| *name == path)
        .map(|(entry, _)| entry.to_string()))
}

/// The index entry and the working-tree kind at `path`, and the entry of its rename source
/// `old` when the row has one, all read now.
pub(crate) async fn pre_image(
    toplevel: &str,
    path: &str,
    old: Option<&str>,
) -> Result<PreImage, String> {
    let index = index_entry(toplevel, path).await?;
    let source = match old {
        Some(old) => index_entry(toplevel, old).await?,
        None => None,
    };
    let full = std::path::Path::new(toplevel).join(path);
    let worktree = match std::fs::symlink_metadata(&full) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => WorktreeKind::Absent,
        Err(e) => return Err(format!("cannot read {path}: {e}")),
        Ok(meta) if meta.file_type().is_symlink() => {
            let target =
                std::fs::read_link(&full).map_err(|e| format!("cannot read {path}: {e}"))?;
            WorktreeKind::Symlink(hash_bytes(target.as_os_str().as_encoded_bytes()))
        }
        Ok(meta) if meta.is_dir() => WorktreeKind::Directory,
        Ok(meta) if meta.is_file() => {
            let hash = hash_file(&full).map_err(|e| format!("cannot read {path}: {e}"))?;
            WorktreeKind::File(hash)
        }
        // Fifos, sockets and devices: a kind no form may run on.
        Ok(_) => WorktreeKind::Other,
    };
    Ok(PreImage {
        index,
        source,
        worktree,
    })
}

/// Renames on both sides, (destination, staged) -> source. A failed probe contributes nothing: the row then shows a creation.
pub(crate) async fn rename_sources(toplevel: &str) -> BTreeMap<(String, bool), String> {
    let mut sources = BTreeMap::new();
    for cached in [true, false] {
        let mut args = vec!["diff"];
        if cached {
            args.push("--cached");
        }
        args.extend(["--name-status", "-M", "-z", "--"]);
        if let Ok(output) = base::git(toplevel, &args).await {
            if output.status.success() {
                for record in parse_name_status(&output.stdout) {
                    if let (true, Some(old)) = (record.status == 'R', record.old) {
                        sources.insert((record.path, cached), old);
                    }
                }
            }
        }
    }
    sources
}

/// An untracked row in either scope: the Phase 1 shape, built here.
pub(crate) async fn untracked_diff(
    toplevel: &str,
    path: &str,
) -> Result<(GetGitDiffResponse, Vec<u8>), String> {
    let key = FileKey {
        path: path.to_string(),
        staged: false,
        untracked: true,
    };
    diff(toplevel, &key, None).await
}

/// The parser other modules' tests build a `LoadedDiff` with.
#[cfg(test)]
pub(crate) fn parse_for_tests(patch: &[u8], path: &str) -> FileDiff {
    parse_sections(patch, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as Proc;

    fn git(dir: &std::path::Path, args: &[&str]) {
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

    fn git_out(dir: &std::path::Path, args: &[&str]) -> Vec<u8> {
        let out = Proc::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        out.stdout
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        git(p, &["config", "user.email", "t@example.com"]);
        git(p, &["config", "user.name", "t"]);
        dir
    }

    fn top(dir: &tempfile::TempDir) -> String {
        dir.path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn key(path: &str, staged: bool, untracked: bool) -> FileKey {
        FileKey {
            path: path.into(),
            staged,
            untracked,
        }
    }

    /// The frozen function is the oracle: hunks must agree for every row kind of 5.2 item 4.
    #[test]
    fn hunks_agree_with_the_frozen_oracle_for_every_row_kind() {
        let dir = repo();
        let p = dir.path();
        for (name, body) in [
            ("mm.txt", "1\n2\n3\n4\n5\n6\n7\n8\n"),
            ("del.txt", "d\n"),
            ("ren.txt", "r1\nr2\nr3\n"),
        ] {
            std::fs::write(p.join(name), body).unwrap();
        }
        git(p, &["add", "-A"]);
        git(p, &["commit", "-q", "-m", "init"]);
        std::fs::write(p.join("mm.txt"), "ONE\n2\n3\n4\n5\n6\n7\n8\n").unwrap();
        git(p, &["add", "mm.txt"]);
        std::fs::write(p.join("mm.txt"), "ONE\n2\n3\n4\n5\n6\n7\nEIGHT\n").unwrap();
        std::fs::write(p.join("am.txt"), "a\n").unwrap();
        git(p, &["add", "am.txt"]);
        std::fs::write(p.join("am.txt"), "a\nb\n").unwrap();
        git(p, &["rm", "-q", "del.txt"]);
        git(p, &["mv", "ren.txt", "renamed.txt"]);
        std::fs::write(p.join("renamed.txt"), "r1\nR2\nr3\n").unwrap();
        std::fs::create_dir_all(p.join("newdir/deep")).unwrap();
        std::fs::write(p.join("newdir/deep/u.txt"), "u\n").unwrap();
        let cwd = p.to_string_lossy().into_owned();
        let toplevel = top(&dir);
        let renames = rt().block_on(rename_sources(&toplevel));
        assert_eq!(
            renames
                .get(&("renamed.txt".to_string(), true))
                .map(String::as_str),
            Some("ren.txt")
        );
        assert!(
            !renames.contains_key(&("renamed.txt".to_string(), false)),
            "the unstaged row has no rename"
        );
        for (path, staged, untracked) in [
            ("mm.txt", true, false),
            ("mm.txt", false, false),
            ("am.txt", true, false),
            ("am.txt", false, false),
            ("del.txt", true, false),
            ("renamed.txt", true, false),
            ("renamed.txt", false, false),
            ("newdir/deep/u.txt", false, true),
        ] {
            let k = key(path, staged, untracked);
            let old = renames.get(&(path.to_string(), staged)).map(String::as_str);
            let (ours, patch) = rt().block_on(diff(&toplevel, &k, old)).unwrap();
            let theirs = rt()
                .block_on(crate::git::get_git_diff_inner(
                    cwd.clone(),
                    path.into(),
                    staged,
                    Some(untracked),
                ))
                .unwrap();
            let shape = |d: &FileDiff| {
                d.hunks
                    .iter()
                    .map(|h| {
                        (
                            h.old_start,
                            h.old_lines,
                            h.new_start,
                            h.new_lines,
                            h.lines.len(),
                        )
                    })
                    .collect::<Vec<_>>()
            };
            assert_eq!(shape(&ours.file_diff), shape(&theirs.file_diff), "{k:?}");
            let content = |d: &FileDiff| {
                d.hunks
                    .iter()
                    .flat_map(|h| {
                        h.lines.iter().map(|l| {
                            let sign = match l.line_type {
                                crate::git::DiffLineType::Added => '+',
                                crate::git::DiffLineType::Removed => '-',
                                crate::git::DiffLineType::Context => ' ',
                            };
                            (
                                sign,
                                l.content.clone(),
                                l.old_line_number,
                                l.new_line_number,
                            )
                        })
                    })
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                content(&ours.file_diff),
                content(&theirs.file_diff),
                "{k:?}"
            );
            assert_eq!(ours.file_diff.old_path, theirs.file_diff.old_path, "{k:?}");
            assert_eq!(
                String::from_utf8_lossy(&patch),
                ours.raw_diff,
                "{k:?}: raw_diff is the lossy patch"
            );
            // The bytes are what git prints with the same flags, run by the test itself.
            let mut args = vec!["diff"];
            if untracked {
                args.push("--no-index");
            } else if staged {
                args.push("--cached");
            }
            args.extend([
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
                "-U3",
                "--src-prefix=a/",
                "--dst-prefix=b/",
            ]);
            if old.is_some() {
                args.push("-M");
            }
            args.push("--");
            if untracked {
                args.push("/dev/null");
            }
            if let Some(o) = old {
                args.push(o);
            }
            args.push(path);
            assert_eq!(
                patch,
                git_out(p, &args),
                "{k:?}: the patch bytes are git's own"
            );
        }
    }

    #[test]
    fn the_patch_keeps_bytes_the_display_cannot() {
        let dir = repo();
        let p = dir.path();
        std::fs::write(p.join("latin.txt"), b"caf\xe9\n").unwrap();
        let toplevel = top(&dir);
        let (response, patch) = rt()
            .block_on(diff(&toplevel, &key("latin.txt", false, true), None))
            .unwrap();
        assert!(
            patch.windows(5).any(|w| w == b"caf\xe9\n"),
            "the bytes survive"
        );
        assert!(
            response.raw_diff.contains('\u{fffd}'),
            "the display is lossy"
        );
        assert_eq!(response.file_diff.hunks.len(), 1);
    }

    #[test]
    fn a_descendants_section_is_cut_before_parsing() {
        let dir = repo();
        let p = dir.path();
        std::fs::write(p.join("tools"), "x\n").unwrap();
        git(p, &["add", "tools"]);
        git(p, &["commit", "-q", "-m", "init"]);
        git(p, &["rm", "-q", "tools"]);
        std::fs::create_dir(p.join("tools")).unwrap();
        std::fs::write(p.join("tools/run"), "r\n").unwrap();
        git(p, &["add", "tools/run"]);
        let toplevel = top(&dir);
        let (response, patch) = rt()
            .block_on(diff(&toplevel, &key("tools", true, false), None))
            .unwrap();
        assert_eq!(
            sections(&patch).len(),
            1,
            "{}",
            String::from_utf8_lossy(&patch)
        );
        assert_eq!(response.file_diff.hunks.len(), 1);
        assert_eq!(response.file_diff.hunks[0].lines.len(), 1, "only `-x`");
        assert!(!response.raw_diff.contains("tools/run"));
        // The index holds tools/run, not tools: the pre-image must not borrow the descendant's entry.
        let pre = rt().block_on(pre_image(&toplevel, "tools", None)).unwrap();
        assert_eq!(pre.index, None);
        assert_eq!(pre.worktree, WorktreeKind::Directory);
    }

    #[test]
    fn a_conflicts_combined_section_is_kept_for_the_view() {
        let dir = repo();
        let p = dir.path();
        std::fs::write(p.join("c.txt"), "base\n").unwrap();
        git(p, &["add", "c.txt"]);
        git(p, &["commit", "-q", "-m", "base"]);
        git(p, &["switch", "-q", "-c", "other"]);
        std::fs::write(p.join("c.txt"), "theirs\n").unwrap();
        git(p, &["commit", "-q", "-am", "theirs"]);
        git(p, &["switch", "-q", "main"]);
        std::fs::write(p.join("c.txt"), "ours\n").unwrap();
        git(p, &["commit", "-q", "-am", "ours"]);
        // conflicts
        let _ = Proc::new("git")
            .arg("-C")
            .arg(p)
            .args(["merge", "-q", "other"])
            .output();
        let toplevel = top(&dir);
        let (response, patch) = rt()
            .block_on(diff(&toplevel, &key("c.txt", false, false), None))
            .unwrap();
        assert!(
            String::from_utf8_lossy(&patch).starts_with("diff --cc c.txt\n"),
            "{}",
            String::from_utf8_lossy(&patch)
        );
        assert!(response.raw_diff.contains("@@@"));
        assert!(
            response.file_diff.hunks.is_empty(),
            "combined diffs parse to zero hunks, as before"
        );
    }

    #[test]
    fn config_cannot_change_the_patch_and_textconv_is_off() {
        let dir = repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "a\nb\nc\nd\n").unwrap();
        std::fs::write(p.join("bin.dat"), [0u8, 1, 2]).unwrap();
        std::fs::write(p.join(".gitattributes"), "bin.dat diff=hex\n").unwrap();
        git(p, &["add", "-A"]);
        git(p, &["commit", "-q", "-m", "init"]);
        git(p, &["config", "diff.noprefix", "true"]);
        git(p, &["config", "diff.context", "0"]);
        git(p, &["config", "diff.hex.textconv", "xxd"]);
        std::fs::write(p.join("a.txt"), "a\nB\nc\nd\n").unwrap();
        std::fs::write(p.join("bin.dat"), [0u8, 1, 3]).unwrap();
        let toplevel = top(&dir);
        let (_, patch) = rt()
            .block_on(diff(&toplevel, &key("a.txt", false, false), None))
            .unwrap();
        let text = String::from_utf8_lossy(&patch);
        assert!(text.contains("--- a/a.txt\n+++ b/a.txt\n"), "{text}");
        assert!(
            text.contains("@@ -1,4 +1,4 @@"),
            "three lines of context: {text}"
        );
        let (response, patch) = rt()
            .block_on(diff(&toplevel, &key("bin.dat", false, false), None))
            .unwrap();
        assert!(
            String::from_utf8_lossy(&patch).contains("Binary files"),
            "{}",
            String::from_utf8_lossy(&patch)
        );
        assert!(response.file_diff.hunks.is_empty());
    }

    #[test]
    fn the_pre_image_names_the_index_entry_and_the_worktree_kind() {
        let dir = repo();
        let p = dir.path();
        std::fs::write(p.join("f.txt"), "f\n").unwrap();
        git(p, &["add", "f.txt"]);
        git(p, &["commit", "-q", "-m", "init"]);
        let toplevel = top(&dir);
        let before = rt().block_on(pre_image(&toplevel, "f.txt", None)).unwrap();
        let listed = String::from_utf8_lossy(&git_out(p, &["ls-files", "-s", "--", "f.txt"]))
            .trim()
            .to_string();
        assert_eq!(
            before.index.as_deref(),
            Some(listed.split('\t').next().unwrap())
        );
        assert_eq!(before.worktree, WorktreeKind::File(hash_bytes(b"f\n")));
        std::fs::write(p.join("f.txt"), "F\n").unwrap();
        let after = rt().block_on(pre_image(&toplevel, "f.txt", None)).unwrap();
        assert_eq!(after.index, before.index);
        assert_ne!(after.worktree, before.worktree);
        std::fs::remove_file(p.join("f.txt")).unwrap();
        assert_eq!(
            rt().block_on(pre_image(&toplevel, "f.txt", None))
                .unwrap()
                .worktree,
            WorktreeKind::Absent
        );
        std::os::unix::fs::symlink("elsewhere", p.join("f.txt")).unwrap();
        assert_eq!(
            rt().block_on(pre_image(&toplevel, "f.txt", None))
                .unwrap()
                .worktree,
            WorktreeKind::Symlink(hash_bytes(b"elsewhere"))
        );
        std::fs::remove_file(p.join("f.txt")).unwrap();
        std::fs::create_dir(p.join("f.txt")).unwrap();
        assert_eq!(
            rt().block_on(pre_image(&toplevel, "f.txt", None))
                .unwrap()
                .worktree,
            WorktreeKind::Directory
        );
        let untracked = rt()
            .block_on(pre_image(&toplevel, "nothere.txt", None))
            .unwrap();
        assert_eq!(
            untracked,
            PreImage {
                index: None,
                source: None,
                worktree: WorktreeKind::Absent
            }
        );
    }

    #[test]
    fn the_pre_image_records_the_rename_sources_entry() {
        let dir = repo();
        let p = dir.path();
        std::fs::write(p.join("ren.txt"), "r1\nr2\nr3\n").unwrap();
        std::fs::write(p.join("old.txt"), "same\ncontent\nhere\n").unwrap();
        git(p, &["add", "-A"]);
        git(p, &["commit", "-q", "-m", "init"]);
        let toplevel = top(&dir);
        // A staged rename: the source left the index, so the destination's pre-image records none.
        git(p, &["mv", "ren.txt", "renamed.txt"]);
        let staged = rt()
            .block_on(pre_image(&toplevel, "renamed.txt", Some("ren.txt")))
            .unwrap();
        assert!(staged.index.is_some());
        assert_eq!(staged.source, None);
        // An intent-to-add rename: the source's entry is what a forward form applies to.
        std::fs::rename(p.join("old.txt"), p.join("new.txt")).unwrap();
        git(p, &["add", "-N", "new.txt"]);
        let listed = String::from_utf8_lossy(&git_out(p, &["ls-files", "-s", "--", "old.txt"]))
            .trim()
            .to_string();
        let entry = listed.split('\t').next().unwrap();
        assert!(entry.starts_with("100644 "), "{listed}");
        let unstaged = rt()
            .block_on(pre_image(&toplevel, "new.txt", Some("old.txt")))
            .unwrap();
        assert_eq!(unstaged.source.as_deref(), Some(entry));
        assert!(unstaged.index.is_some(), "the intent-to-add entry");
        assert_ne!(unstaged.index, unstaged.source);
        // Without a source nothing is read for one.
        let plain = rt()
            .block_on(pre_image(&toplevel, "new.txt", None))
            .unwrap();
        assert_eq!(plain.source, None);
    }

    #[test]
    fn a_file_is_hashed_through_a_bounded_buffer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.bin");
        let contents: Vec<u8> = (0..300 * 1024u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &contents).unwrap();
        assert_eq!(hash_file(&path).unwrap(), hash_bytes(&contents));
        let prefix = &contents[..HASH_BUFFER];
        assert_ne!(hash_file(&path).unwrap(), hash_bytes(prefix));
        std::fs::write(&path, prefix).unwrap();
        assert_eq!(hash_file(&path).unwrap(), hash_bytes(prefix));
        std::fs::write(&path, b"").unwrap();
        assert_eq!(hash_file(&path).unwrap(), hash_bytes(b""));
    }

    #[test]
    fn an_intent_to_add_rename_is_found_on_the_unstaged_side() {
        let dir = repo();
        let p = dir.path();
        std::fs::write(p.join("old.txt"), "same\ncontent\nhere\n").unwrap();
        git(p, &["add", "old.txt"]);
        git(p, &["commit", "-q", "-m", "init"]);
        std::fs::rename(p.join("old.txt"), p.join("new.txt")).unwrap();
        git(p, &["add", "-N", "new.txt"]);
        let toplevel = top(&dir);
        let renames = rt().block_on(rename_sources(&toplevel));
        assert_eq!(
            renames
                .get(&("new.txt".to_string(), false))
                .map(String::as_str),
            Some("old.txt")
        );
        let (response, _) = rt()
            .block_on(diff(
                &toplevel,
                &key("new.txt", false, false),
                Some("old.txt"),
            ))
            .unwrap();
        assert_eq!(response.file_diff.old_path.as_deref(), Some("old.txt"));
    }

    #[test]
    fn a_quoted_path_is_cut_and_pre_imaged() {
        let dir = repo();
        let p = dir.path();
        let name = "sp ace\ttab.txt";
        std::fs::write(p.join(name), "a\n").unwrap();
        git(p, &["add", "-A"]);
        git(p, &["commit", "-q", "-m", "init"]);
        std::fs::write(p.join(name), "A\n").unwrap();
        let toplevel = top(&dir);
        let (response, patch) = rt()
            .block_on(diff(&toplevel, &key(name, false, false), None))
            .unwrap();
        assert_eq!(sections(&patch).len(), 1);
        assert_eq!(response.file_diff.hunks.len(), 1);
        assert_eq!(
            rt().block_on(pre_image(&toplevel, name, None))
                .unwrap()
                .worktree,
            WorktreeKind::File(hash_bytes(b"A\n"))
        );
    }
}
