//! Hunk patches and the apply runner (spec 9.2, 9.4). The only mutating git calls live here.

use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;

use crate::engine::sections::sections;
use crate::engine::worktree;
use crate::engine::{Action, ActionKind, Comparison, WorktreeKind};

pub const NOTICE_SCOPE: &str = "switch to worktree scope (b) to stage or discard";
pub const NOTICE_NO_HUNK: &str = "no hunk under the cursor";
pub const NOTICE_CUT: &str = "diff cut by the size cap; use a shell";
pub const NOTICE_BINARY: &str = "binary file: not applied here";
pub const NOTICE_SUBMODULE: &str = "submodule: not applied here";
pub const NOTICE_NOT_A_FILE: &str = "not a regular file: not applied here";
pub const NOTICE_CHANGED: &str = "the diff changed; look again";
pub const NOTICE_RUNNING: &str = "an action is still running";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Forward,
    Reverse,
}

/// One `git apply` invocation of spec 9.2's table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    StageHunk,
    UnstageHunk,
    DiscardUnstaged,
    DiscardStaged,
    StageUntracked,
    DeleteUntracked,
}

impl Form {
    pub fn flags(self) -> &'static [&'static str] {
        match self {
            Self::StageHunk | Self::StageUntracked => &["--cached"],
            Self::UnstageHunk => &["--cached", "-R"],
            Self::DiscardUnstaged | Self::DeleteUntracked => &["-R"],
            Self::DiscardStaged => &["--index", "-R"],
        }
    }

    pub fn direction(self) -> Direction {
        match self {
            Self::StageHunk | Self::StageUntracked => Direction::Forward,
            _ => Direction::Reverse,
        }
    }

    pub fn reads_index(self) -> bool {
        matches!(
            self,
            Self::StageHunk | Self::UnstageHunk | Self::DiscardStaged | Self::StageUntracked
        )
    }

    pub fn reads_worktree(self) -> bool {
        matches!(
            self,
            Self::DiscardUnstaged | Self::DiscardStaged | Self::DeleteUntracked
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Binary,
    Submodule,
}

/// The lines of `section`, each with its newline; a last line without one is still returned.
fn lines(section: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, &b) in section.iter().enumerate() {
        if b == b'\n' {
            out.push(&section[start..=i]);
            start = i + 1;
        }
    }
    if start < section.len() {
        out.push(&section[start..]);
    }
    out
}

/// `Binary files` and `GIT binary patch` carry no payload git could apply; a gitlink (mode 160000,
/// named in the section's `index` or mode lines, never in its content) is not a file.
pub fn classify(patch: &[u8]) -> Kind {
    for section in sections(patch) {
        let (header, _) = split_section(section);
        for line in &header {
            let text = String::from_utf8_lossy(line);
            let text = text.trim_end();
            if text.starts_with("Binary files ") || text.starts_with("GIT binary patch") {
                return Kind::Binary;
            }
            let is_gitlink = (text.starts_with("index ") && text.ends_with(" 160000"))
                || text == "new file mode 160000"
                || text == "deleted file mode 160000"
                || text == "old mode 160000"
                || text == "new mode 160000";
            if is_gitlink {
                return Kind::Submodule;
            }
        }
    }
    Kind::Text
}

/// A section's header lines and its hunks, each hunk as the lines from its `@@` to the next.
fn split_section(section: &[u8]) -> (Vec<&[u8]>, Vec<Vec<&[u8]>>) {
    let mut header = Vec::new();
    let mut hunks: Vec<Vec<&[u8]>> = Vec::new();
    for line in lines(section) {
        if line.starts_with(b"@@") {
            hunks.push(vec![line]);
        } else if let Some(current) = hunks.last_mut() {
            current.push(line);
        } else {
            header.push(line);
        }
    }
    (header, hunks)
}

/// How many hunks the kept sections hold, the same count the view shows.
pub fn hunk_count(patch: &[u8]) -> usize {
    sections(patch)
        .into_iter()
        .map(|s| split_section(s).1.len())
        .sum()
}

/// `b/<quoted>` -> `a/<quoted>`, keeping git's own quoting.
fn a_side_of(plus: &[u8]) -> Vec<u8> {
    if let Some(rest) = plus.strip_prefix(b"\"b/") {
        let mut out = b"\"a/".to_vec();
        out.extend_from_slice(rest);
        out
    } else if let Some(rest) = plus.strip_prefix(b"b/") {
        let mut out = b"a/".to_vec();
        out.extend_from_slice(rest);
        out
    } else {
        plus.to_vec()
    }
}

/// Spec 9.2 "Headers": mode lines go; a rename's header is rebuilt from its `+++` line.
fn rewrite_header(header: &[&[u8]]) -> Vec<u8> {
    let is_rename = header
        .iter()
        .any(|l| l.starts_with(b"rename from ") || l.starts_with(b"copy from "));
    let plus = header
        .iter()
        .find_map(|l| l.strip_prefix(b"+++ "))
        .map(|rest| rest.strip_suffix(b"\n").unwrap_or(rest));
    let mut out = Vec::new();
    for line in header {
        if line.starts_with(b"old mode ") || line.starts_with(b"new mode ") {
            continue;
        }
        if is_rename {
            if line.starts_with(b"similarity index ")
                || line.starts_with(b"rename from ")
                || line.starts_with(b"rename to ")
                || line.starts_with(b"copy from ")
                || line.starts_with(b"copy to ")
            {
                continue;
            }
            if let (Some(plus), true) = (plus, line.starts_with(b"diff --git ")) {
                out.extend_from_slice(b"diff --git ");
                out.extend_from_slice(&a_side_of(plus));
                out.push(b' ');
                out.extend_from_slice(plus);
                out.push(b'\n');
                continue;
            }
            if let (Some(plus), true) = (plus, line.starts_with(b"--- ")) {
                out.extend_from_slice(b"--- ");
                out.extend_from_slice(&a_side_of(plus));
                out.push(b'\n');
                continue;
            }
        }
        out.extend_from_slice(line);
    }
    out
}

/// `@@ -o[,n] +p[,m] @@ rest` with both starts set to the pre-image side.
fn recount(at: &[u8], direction: Direction) -> Vec<u8> {
    let text = String::from_utf8_lossy(at);
    let Some(rest) = text.strip_prefix("@@ -") else {
        return at.to_vec();
    };
    let Some((ranges, tail)) = rest.split_once(" @@") else {
        return at.to_vec();
    };
    let Some((old, new)) = ranges.split_once(" +") else {
        return at.to_vec();
    };
    let (old_start, old_count) = old
        .split_once(',')
        .map(|(s, c)| (s, Some(c)))
        .unwrap_or((old, None));
    let (new_start, new_count) = new
        .split_once(',')
        .map(|(s, c)| (s, Some(c)))
        .unwrap_or((new, None));
    let start = match direction {
        Direction::Forward => old_start,
        Direction::Reverse => new_start,
    };
    let range = |count: Option<&str>| match count {
        Some(c) => format!("{start},{c}"),
        None => start.to_string(),
    };
    format!("@@ -{} +{} @@{tail}", range(old_count), range(new_count)).into_bytes()
}

/// Hunk `index` of the kept sections, counted across sections, with its section's rewritten header.
pub fn hunk_patch(patch: &[u8], index: usize, direction: Direction) -> Option<Vec<u8>> {
    let mut seen = 0;
    for section in sections(patch) {
        let (header, hunks) = split_section(section);
        if index < seen + hunks.len() {
            let hunk = &hunks[index - seen];
            let mut out = rewrite_header(&header);
            out.extend(recount(hunk[0], direction));
            for line in &hunk[1..] {
                out.extend_from_slice(line);
            }
            return Some(out);
        }
        seen += hunks.len();
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// stderr named `index.lock`: worth a retry.
    Locked(String),
    /// git's first non-empty stderr line, or the spawn/timeout text.
    Failed(String),
}

const APPLY_TIMEOUT: Duration = Duration::from_secs(30);

pub async fn apply(toplevel: &str, form: Form, patch: &[u8]) -> Result<(), Refusal> {
    apply_with("git", APPLY_TIMEOUT, toplevel, form, patch).await
}

/// `git -c apply.ignoreWhitespace=no -C <toplevel> apply --whitespace=nowarn <flags>`, the patch on stdin.
pub async fn apply_with(
    program: &str,
    timeout: Duration,
    toplevel: &str,
    form: Form,
    patch: &[u8],
) -> Result<(), Refusal> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.arg("-c")
        .arg("apply.ignoreWhitespace=no")
        .arg("-C")
        .arg(toplevel)
        .arg("apply")
        .arg("--whitespace=nowarn")
        .args(form.flags())
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| Refusal::Failed(format!("spawn failed: {e}")))?;
    let mut stdin = child.stdin.take().expect("stdin was piped");
    let patch = patch.to_vec();
    // Write and wait together: a patch past the pipe buffer would otherwise deadlock against git's own output.
    let writer = tokio::spawn(async move {
        let written = stdin.write_all(&patch).await;
        drop(stdin);
        written
    });
    let abort = writer.abort_handle();
    let waited = tokio::time::timeout(timeout, async {
        let output = child.wait_with_output().await;
        let _ = writer.await;
        output
    })
    .await;
    let output = match waited {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => return Err(Refusal::Failed(format!("git apply failed: {e}"))),
        Err(_) => {
            // The child is killed by kill_on_drop; the writer may still be blocked on a pipe a
            // grandchild kept open, so it is aborted rather than awaited.
            abort.abort();
            return Err(Refusal::Failed(format!(
                "git apply timed out after {}s",
                timeout.as_secs()
            )));
        }
    };
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let line = stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string);
    match line {
        Some(line) if line.contains("index.lock") => Err(Refusal::Locked(line)),
        Some(line) => Err(Refusal::Failed(line)),
        None => Err(Refusal::Failed(format!(
            "git apply failed with status {}",
            output.status
        ))),
    }
}

/// Spec 9.2's eligibility, from the Arc alone, in the order the TUI decides it too.
pub fn plan(action: &Action) -> Result<(Form, Vec<u8>), String> {
    let diff = &action.diff;
    if diff.comparison != Comparison::Worktree {
        return Err(NOTICE_SCOPE.into());
    }
    if diff.truncated_lines > 0 {
        return Err(NOTICE_CUT.into());
    }
    let kind = classify(&diff.patch);
    if kind == Kind::Submodule {
        return Err(NOTICE_SUBMODULE.into());
    }
    let whole = action.kind == ActionKind::DiscardFile || diff.key.untracked;
    if whole {
        if kind == Kind::Binary {
            return Err(NOTICE_BINARY.into());
        }
        let form = match (diff.key.untracked, action.kind, diff.key.staged) {
            (true, ActionKind::Stage, _) => Form::StageUntracked,
            (true, _, _) => Form::DeleteUntracked,
            (false, _, true) => Form::DiscardStaged,
            (false, _, false) => Form::DiscardUnstaged,
        };
        return Ok((form, diff.patch.clone()));
    }
    let Some(hunk) = action.hunk.filter(|&h| h < hunk_count(&diff.patch)) else {
        return Err(NOTICE_NO_HUNK.into());
    };
    let form = match (action.kind, diff.key.staged) {
        (ActionKind::Stage, false) => Form::StageHunk,
        (ActionKind::Stage, true) => Form::UnstageHunk,
        (_, false) => Form::DiscardUnstaged,
        (_, true) => Form::DiscardStaged,
    };
    let patch = hunk_patch(&diff.patch, hunk, form.direction()).ok_or(NOTICE_NO_HUNK)?;
    Ok((form, patch))
}

/// The side a form applies to must hold what the diff was read against; an `--index` form also
/// needs the file present while the index still holds an entry.
pub(crate) async fn check_pre_image(
    toplevel: &str,
    action: &Action,
    form: Form,
) -> Result<(), String> {
    let Some(recorded) = &action.diff.pre_image else {
        return Err(NOTICE_CHANGED.into());
    };
    let now = worktree::pre_image(toplevel, &action.diff.key.path).await?;
    if form.reads_index() && now.index != recorded.index {
        return Err(NOTICE_CHANGED.into());
    }
    if form.reads_worktree() {
        if now.worktree != recorded.worktree {
            return Err(NOTICE_CHANGED.into());
        }
        if matches!(now.worktree, WorktreeKind::Directory | WorktreeKind::Other) {
            return Err(NOTICE_NOT_A_FILE.into());
        }
        // With an entry still in the index (MD), git would recreate the missing file and erase the
        // unstaged deletion; a clean staged deletion has no entry, and --index -R restores it.
        if form == Form::DiscardStaged
            && now.worktree == WorktreeKind::Absent
            && now.index.is_some()
        {
            return Err(format!("{}: does not match index", action.diff.key.path));
        }
    }
    Ok(())
}

const LOCK_RETRY: Duration = Duration::from_secs(2);

/// Check, apply, and retry an `index.lock` collision for up to 2 s, re-checking before each retry.
/// `.1` is whether a form ran.
pub(crate) async fn run(
    toplevel: &str,
    action: &Action,
    form: Form,
    patch: &[u8],
) -> (Result<(), String>, bool) {
    let deadline = Instant::now() + LOCK_RETRY;
    let mut applied = false;
    loop {
        if let Err(e) = check_pre_image(toplevel, action, form).await {
            return (Err(e), applied);
        }
        applied = true;
        match apply(toplevel, form, patch).await {
            Ok(()) => return (Ok(()), true),
            Err(Refusal::Locked(_)) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(Refusal::Locked(m)) | Err(Refusal::Failed(m)) => return (Err(m), true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A valid two-hunk diff of `old_text()` to `new_text()`: an insertion after line 2 shifts the
    /// second hunk's new-side start by one.
    const TWO_HUNKS: &[u8] = b"diff --git a/f.txt b/f.txt\nindex 0ff3bbb..7647ea4 100644\n--- a/f.txt\n+++ b/f.txt\n@@ -1,5 +1,6 @@\n 1\n 2\n+ins\n 3\n 4\n 5\n@@ -24,7 +25,7 @@ f23\n f24\n blk\n one\n-two\n+TWO\n three\n blk-end\n tail\n";

    fn old_text() -> String {
        let mut s: String = (1..=5).map(|i| format!("{i}\n")).collect();
        s.extend((6..=24).map(|i| format!("f{i}\n")));
        s.push_str("blk\none\ntwo\nthree\nblk-end\ntail\n");
        s
    }

    fn new_text() -> String {
        old_text()
            .replacen("2\n3\n", "2\nins\n3\n", 1)
            .replacen("one\ntwo\n", "one\nTWO\n", 1)
    }

    #[test]
    fn a_hunk_carries_its_header_and_only_itself() {
        let one = hunk_patch(TWO_HUNKS, 1, Direction::Forward).unwrap();
        let text = String::from_utf8_lossy(&one);
        assert!(text.starts_with(
            "diff --git a/f.txt b/f.txt\nindex 0ff3bbb..7647ea4 100644\n--- a/f.txt\n+++ b/f.txt\n@@ "
        ));
        assert!(text.contains("+TWO\n") && !text.contains("+ins\n"));
        assert_eq!(hunk_count(TWO_HUNKS), 2);
        assert!(hunk_patch(TWO_HUNKS, 2, Direction::Forward).is_none());
    }

    #[test]
    fn coordinates_are_recounted_for_each_direction() {
        let forward = hunk_patch(TWO_HUNKS, 1, Direction::Forward).unwrap();
        assert!(String::from_utf8_lossy(&forward).contains("\n@@ -24,7 +24,7 @@ f23\n"));
        let reverse = hunk_patch(TWO_HUNKS, 1, Direction::Reverse).unwrap();
        assert!(String::from_utf8_lossy(&reverse).contains("\n@@ -25,7 +25,7 @@ f23\n"));
        // A count of one is written as git writes it: no `,1`.
        let single: &[u8] = b"diff --git a/g b/g\n--- a/g\n+++ b/g\n@@ -3 +4 @@\n-x\n+y\n";
        let r = hunk_patch(single, 0, Direction::Reverse).unwrap();
        assert!(
            String::from_utf8_lossy(&r).contains("\n@@ -4 +4 @@\n"),
            "{}",
            String::from_utf8_lossy(&r)
        );
    }

    /// The recount is proven against git: each slice applies alone, forward onto the old text and
    /// reverse off the new text, and touches only its own lines.
    #[test]
    fn the_recounted_slices_apply_to_the_real_files() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        let run = |patch: &[u8], reverse: bool| {
            let mut cmd = std::process::Command::new("git");
            cmd.arg("-C").arg(dir.path()).arg("apply");
            if reverse {
                cmd.arg("-R");
            }
            let mut child = cmd.stdin(std::process::Stdio::piped()).spawn().unwrap();
            child.stdin.as_mut().unwrap().write_all(patch).unwrap();
            drop(child.stdin.take());
            assert!(child.wait().unwrap().success());
        };
        std::fs::write(&path, old_text()).unwrap();
        run(
            &hunk_patch(TWO_HUNKS, 1, Direction::Forward).unwrap(),
            false,
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("one\nTWO\nthree\n") && !text.contains("ins\n"),
            "{text}"
        );
        std::fs::write(&path, old_text()).unwrap();
        run(
            &hunk_patch(TWO_HUNKS, 0, Direction::Forward).unwrap(),
            false,
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("2\nins\n3\n") && text.contains("one\ntwo\n"),
            "{text}"
        );
        std::fs::write(&path, new_text()).unwrap();
        run(&hunk_patch(TWO_HUNKS, 1, Direction::Reverse).unwrap(), true);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("one\ntwo\nthree\n") && text.contains("2\nins\n3\n"),
            "{text}"
        );
    }

    #[test]
    fn mode_lines_leave_a_hunk_patch_and_stay_in_the_whole_file() {
        let patch: &[u8] = b"diff --git a/r b/r\nold mode 100644\nnew mode 100755\nindex 088db5c..77a7a0a\n--- a/r\n+++ b/r\n@@ -1 +1 @@\n-a\n+b\n";
        let hunk = hunk_patch(patch, 0, Direction::Forward).unwrap();
        let text = String::from_utf8_lossy(&hunk);
        assert!(
            !text.contains("old mode") && !text.contains("new mode"),
            "{text}"
        );
        assert!(text.contains("index 088db5c..77a7a0a\n"));
        assert_eq!(classify(patch), Kind::Text);
    }

    #[test]
    fn a_rename_section_keeps_gits_quoting() {
        let patch: &[u8] = b"diff --git \"a/old\\tname.txt\" \"b/new\\tname.txt\"\nsimilarity index 74%\nrename from \"old\\tname.txt\"\nrename to \"new\\tname.txt\"\nindex ac33350..5dbdd84 100644\n--- \"a/old\\tname.txt\"\n+++ \"b/new\\tname.txt\"\n@@ -1,4 +1,4 @@\n-alpha\n+ALPHA\n ctx1\n ctx2\n ctx3\n@@ -6,4 +6,4 @@ ctx4\n ctx5\n ctx6\n ctx7\n-gamma\n+GAMMA\n";
        let hunk = hunk_patch(patch, 0, Direction::Reverse).unwrap();
        let text = String::from_utf8_lossy(&hunk);
        assert!(text.starts_with("diff --git \"a/new\\tname.txt\" \"b/new\\tname.txt\"\nindex ac33350..5dbdd84 100644\n--- \"a/new\\tname.txt\"\n+++ \"b/new\\tname.txt\"\n@@ -1,4 +1,4 @@\n"), "{text}");
        assert!(!text.contains("rename") && !text.contains("similarity"));
        let plain: &[u8] = b"diff --git a/old.txt b/new.txt\nsimilarity index 74%\nrename from old.txt\nrename to new.txt\nindex ac33350..5dbdd84 100644\n--- a/old.txt\n+++ b/new.txt\n@@ -1,4 +1,4 @@\n-alpha\n+ALPHA\n c\n c\n c\n";
        let hunk = hunk_patch(plain, 0, Direction::Reverse).unwrap();
        assert!(String::from_utf8_lossy(&hunk).starts_with(
            "diff --git a/new.txt b/new.txt\nindex ac33350..5dbdd84 100644\n--- a/new.txt\n+++ b/new.txt\n"
        ));
    }

    #[test]
    fn a_type_change_counts_hunks_across_its_two_sections() {
        let patch: &[u8] = b"diff --git a/t b/t\ndeleted file mode 100644\nindex 1..2\n--- a/t\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\ndiff --git a/t b/t\nnew file mode 120000\nindex 0..3\n--- /dev/null\n+++ b/t\n@@ -0,0 +1 @@\n+target\n\\ No newline at end of file\n";
        assert_eq!(hunk_count(patch), 2);
        let second = hunk_patch(patch, 1, Direction::Forward).unwrap();
        let text = String::from_utf8_lossy(&second);
        assert!(
            text.starts_with("diff --git a/t b/t\nnew file mode 120000\n"),
            "{text}"
        );
        assert!(text.ends_with("+target\n\\ No newline at end of file\n"));
        assert!(!text.contains("deleted file mode"));
    }

    #[test]
    fn an_empty_file_has_no_hunk_and_binary_and_submodules_are_told_apart() {
        let empty: &[u8] = b"diff --git a/e b/e\nnew file mode 100644\nindex 0000000..e69de29\n";
        assert_eq!(hunk_count(empty), 0);
        assert!(hunk_patch(empty, 0, Direction::Forward).is_none());
        assert_eq!(classify(empty), Kind::Text);
        assert_eq!(
            classify(b"diff --git a/b b/b\nindex 1..2\nBinary files a/b and b/b differ\n"),
            Kind::Binary
        );
        assert_eq!(
            classify(b"diff --git a/b b/b\nindex 1..2\nGIT binary patch\nliteral 3\n"),
            Kind::Binary
        );
        assert_eq!(classify(b"diff --git a/sub b/sub\nindex 1..2 160000\n--- a/sub\n+++ b/sub\n@@ -1 +1 @@\n-Subproject commit aaaa\n+Subproject commit bbbb\n"), Kind::Submodule);
        assert_eq!(classify(b"diff --git a/sub b/sub\nnew file mode 160000\nindex 0..2\n--- /dev/null\n+++ b/sub\n@@ -0,0 +1 @@\n+Subproject commit bbbb\n"), Kind::Submodule);
        // A text file that merely mentions the words is a text file.
        assert_eq!(classify(b"diff --git a/notes b/notes\nindex 1..2 100644\n--- a/notes\n+++ b/notes\n@@ -1 +1 @@\n-x\n+Subproject commit aaaa\n"), Kind::Text);
    }

    #[test]
    fn the_forms_name_their_flags_and_sides() {
        assert_eq!(Form::StageHunk.flags(), ["--cached"]);
        assert_eq!(Form::UnstageHunk.flags(), ["--cached", "-R"]);
        assert_eq!(Form::DiscardUnstaged.flags(), ["-R"]);
        assert_eq!(Form::DiscardStaged.flags(), ["--index", "-R"]);
        assert_eq!(Form::StageUntracked.flags(), ["--cached"]);
        assert_eq!(Form::DeleteUntracked.flags(), ["-R"]);
        assert_eq!(Form::StageHunk.direction(), Direction::Forward);
        assert_eq!(Form::DiscardStaged.direction(), Direction::Reverse);
        assert!(Form::DiscardStaged.reads_index() && Form::DiscardStaged.reads_worktree());
        assert!(Form::UnstageHunk.reads_index() && !Form::UnstageHunk.reads_worktree());
        assert!(!Form::DeleteUntracked.reads_index() && Form::DeleteUntracked.reads_worktree());
    }

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

    fn git_out(dir: &std::path::Path, args: &[&str]) -> String {
        String::from_utf8_lossy(
            &Proc::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .unwrap()
                .stdout,
        )
        .into_owned()
    }

    fn repo() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        git(p, &["config", "user.email", "t@example.com"]);
        git(p, &["config", "user.name", "t"]);
        std::fs::write(p.join("f.txt"), "a\nb\nc\nd\ne\n").unwrap();
        git(p, &["add", "f.txt"]);
        git(p, &["commit", "-q", "-m", "init"]);
        std::fs::write(p.join("f.txt"), "A\nb\nc\nd\ne\n").unwrap();
        let top = p.canonicalize().unwrap().to_string_lossy().into_owned();
        (dir, top)
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn fake_git(dir: &std::path::Path, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("fakegit");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn a_patch_that_applies_stages_and_one_that_does_not_is_refused_with_gits_line() {
        let (dir, top) = repo();
        let patch = Proc::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["diff", "-U3", "--", "f.txt"])
            .output()
            .unwrap()
            .stdout;
        rt().block_on(apply(&top, Form::StageHunk, &patch)).unwrap();
        assert!(git_out(dir.path(), &["diff", "--cached"]).contains("+A\n"));
        // Staging it again finds the context gone from the index side.
        let again = rt()
            .block_on(apply(&top, Form::StageHunk, &patch))
            .unwrap_err();
        match again {
            Refusal::Failed(line) => assert!(
                line.contains("patch failed") || line.contains("does not apply"),
                "{line}"
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_missing_program_a_lock_and_a_timeout_each_answer_their_own_way() {
        let (dir, top) = repo();
        let missing = rt().block_on(apply_with(
            "/definitely/not/git",
            Duration::from_secs(5),
            &top,
            Form::StageHunk,
            b"",
        ));
        assert!(
            matches!(missing, Err(Refusal::Failed(ref m)) if m.contains("spawn")),
            "{missing:?}"
        );
        let locked = fake_git(
            dir.path(),
            "echo 'fatal: Unable to create .git/index.lock: File exists.' >&2; exit 128",
        );
        let locked = rt().block_on(apply_with(
            &locked,
            Duration::from_secs(5),
            &top,
            Form::StageHunk,
            b"",
        ));
        assert!(
            matches!(locked, Err(Refusal::Locked(ref m)) if m.contains("index.lock")),
            "{locked:?}"
        );
        let slow = fake_git(dir.path(), "sleep 5");
        let started = std::time::Instant::now();
        let timed = rt().block_on(apply_with(
            &slow,
            Duration::from_millis(200),
            &top,
            Form::StageHunk,
            b"",
        ));
        assert_eq!(
            timed,
            Err(Refusal::Failed("git apply timed out after 0s".into()))
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the child was not killed"
        );
        // A grandchild that keeps stdin open while the child dies: the writer must not outlive the timeout.
        let holder = fake_git(dir.path(), "sleep 5 & wait");
        let big: Vec<u8> = (0..120_000)
            .map(|i| format!("line {i}\n"))
            .collect::<String>()
            .into_bytes();
        let started = std::time::Instant::now();
        let held = rt().block_on(apply_with(
            &holder,
            Duration::from_millis(200),
            &top,
            Form::StageHunk,
            &big,
        ));
        assert!(
            matches!(held, Err(Refusal::Failed(ref m)) if m.contains("timed out")),
            "{held:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the writer was left blocked on the grandchild's pipe"
        );
        let silent = fake_git(dir.path(), "exit 3");
        let silent = rt().block_on(apply_with(
            &silent,
            Duration::from_secs(5),
            &top,
            Form::StageHunk,
            b"",
        ));
        assert_eq!(
            silent,
            Err(Refusal::Failed(
                "git apply failed with status exit status: 3".into()
            ))
        );
    }

    #[test]
    fn a_large_patch_on_stdin_applies() {
        let (dir, top) = repo();
        let body: String = (0..120_000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(dir.path().join("big.txt"), &body).unwrap();
        let patch = Proc::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["diff", "--no-index", "-U3", "--", "/dev/null", "big.txt"])
            .output()
            .unwrap()
            .stdout;
        assert!(
            patch.len() > 1 << 20,
            "the fixture must exceed the pipe buffer"
        );
        rt().block_on(apply(&top, Form::StageUntracked, &patch))
            .unwrap();
        assert!(git_out(dir.path(), &["ls-files", "--", "big.txt"]).contains("big.txt"));
    }

    use crate::engine::{
        Action, ActionKind, Comparison, FileKey, LoadedDiff, PreImage, WorktreeKind,
    };
    use crate::git::GetGitDiffResponse;
    use std::sync::Arc;

    fn loaded(
        path: &str,
        staged: bool,
        untracked: bool,
        comparison: Comparison,
        patch: &[u8],
        cap: usize,
    ) -> Arc<LoadedDiff> {
        let file_diff = crate::engine::worktree::parse_for_tests(patch, path);
        let response = GetGitDiffResponse {
            file_diff,
            old_text: String::new(),
            new_text: String::new(),
            raw_diff: String::from_utf8_lossy(patch).into_owned(),
            repo_root: "/r".into(),
        };
        let key = FileKey {
            path: path.into(),
            staged,
            untracked,
        };
        let pre = Some(PreImage {
            index: None,
            worktree: WorktreeKind::Absent,
        });
        Arc::new(LoadedDiff::build_with_cap(
            key,
            comparison,
            None,
            response,
            patch.to_vec(),
            pre,
            cap,
        ))
    }

    fn act(kind: ActionKind, diff: &Arc<LoadedDiff>, hunk: Option<usize>) -> Action {
        Action {
            kind,
            diff: diff.clone(),
            hunk,
        }
    }

    #[test]
    fn the_plan_follows_the_row_and_refuses_what_the_spec_refuses() {
        let unstaged = loaded(
            "f.txt",
            false,
            false,
            Comparison::Worktree,
            TWO_HUNKS,
            1_000,
        );
        let staged = loaded("f.txt", true, false, Comparison::Worktree, TWO_HUNKS, 1_000);
        assert_eq!(
            plan(&act(ActionKind::Stage, &unstaged, Some(1))).unwrap().0,
            Form::StageHunk
        );
        assert_eq!(
            plan(&act(ActionKind::Stage, &staged, Some(1))).unwrap().0,
            Form::UnstageHunk
        );
        assert_eq!(
            plan(&act(ActionKind::Discard, &unstaged, Some(0)))
                .unwrap()
                .0,
            Form::DiscardUnstaged
        );
        assert_eq!(
            plan(&act(ActionKind::Discard, &staged, Some(0))).unwrap().0,
            Form::DiscardStaged
        );
        let (form, whole) = plan(&act(ActionKind::DiscardFile, &staged, None)).unwrap();
        assert_eq!((form, whole.as_slice()), (Form::DiscardStaged, TWO_HUNKS));
        let (_, one) = plan(&act(ActionKind::Stage, &unstaged, Some(1))).unwrap();
        assert_eq!(one, hunk_patch(TWO_HUNKS, 1, Direction::Forward).unwrap());
        assert_eq!(
            plan(&act(ActionKind::Stage, &unstaged, None)).unwrap_err(),
            NOTICE_NO_HUNK
        );
        assert_eq!(
            plan(&act(ActionKind::Discard, &unstaged, Some(2))).unwrap_err(),
            NOTICE_NO_HUNK
        );
        let branch = loaded(
            "f.txt",
            false,
            false,
            Comparison::Branch {
                merge_base: "m".repeat(40),
            },
            TWO_HUNKS,
            1_000,
        );
        assert_eq!(
            plan(&act(ActionKind::DiscardFile, &branch, None)).unwrap_err(),
            NOTICE_SCOPE
        );
        let cut = loaded("f.txt", false, false, Comparison::Worktree, TWO_HUNKS, 3);
        assert!(cut.truncated_lines > 0);
        for (kind, hunk) in [
            (ActionKind::Stage, Some(0)),
            (ActionKind::DiscardFile, None),
        ] {
            assert_eq!(plan(&act(kind, &cut, hunk)).unwrap_err(), NOTICE_CUT);
        }
        let binary = loaded(
            "b",
            false,
            false,
            Comparison::Worktree,
            b"diff --git a/b b/b\nindex 1..2\nBinary files a/b and b/b differ\n",
            1_000,
        );
        assert_eq!(
            plan(&act(ActionKind::Stage, &binary, Some(0))).unwrap_err(),
            NOTICE_NO_HUNK
        );
        assert_eq!(
            plan(&act(ActionKind::DiscardFile, &binary, None)).unwrap_err(),
            NOTICE_BINARY
        );
        let untracked_binary = loaded(
            "b",
            false,
            true,
            Comparison::Worktree,
            b"diff --git a/b b/b\nnew file mode 100644\nindex 0..2\nBinary files /dev/null and b/b differ\n",
            1_000,
        );
        for kind in [
            ActionKind::Stage,
            ActionKind::Discard,
            ActionKind::DiscardFile,
        ] {
            assert_eq!(
                plan(&act(kind, &untracked_binary, None)).unwrap_err(),
                NOTICE_BINARY
            );
        }
        let sub = loaded("sub", true, false, Comparison::Worktree, b"diff --git a/sub b/sub\nindex 1..2 160000\n--- a/sub\n+++ b/sub\n@@ -1 +1 @@\n-Subproject commit a\n+Subproject commit b\n", 1_000);
        assert_eq!(
            plan(&act(ActionKind::Stage, &sub, Some(0))).unwrap_err(),
            NOTICE_SUBMODULE
        );
        let untracked = loaded("u.txt", false, true, Comparison::Worktree, b"diff --git a/u.txt b/u.txt\nnew file mode 100644\nindex 0..1\n--- /dev/null\n+++ b/u.txt\n@@ -0,0 +1 @@\n+u\n", 1_000);
        assert_eq!(
            plan(&act(ActionKind::Stage, &untracked, None)).unwrap().0,
            Form::StageUntracked
        );
        assert_eq!(
            plan(&act(ActionKind::Discard, &untracked, Some(0)))
                .unwrap()
                .0,
            Form::DeleteUntracked
        );
        assert_eq!(
            plan(&act(ActionKind::DiscardFile, &untracked, None))
                .unwrap()
                .0,
            Form::DeleteUntracked
        );
        let empty = loaded(
            "e",
            false,
            true,
            Comparison::Worktree,
            b"diff --git a/e b/e\nnew file mode 100644\nindex 0000000..e69de29\n",
            1_000,
        );
        assert_eq!(
            plan(&act(ActionKind::Stage, &empty, None)).unwrap().0,
            Form::StageUntracked
        );
        let mode_only = loaded(
            "m",
            false,
            false,
            Comparison::Worktree,
            b"diff --git a/m b/m\nold mode 100644\nnew mode 100755\n",
            1_000,
        );
        assert_eq!(
            plan(&act(ActionKind::Stage, &mode_only, None)).unwrap_err(),
            NOTICE_NO_HUNK
        );
        assert_eq!(
            plan(&act(ActionKind::DiscardFile, &mode_only, None))
                .unwrap()
                .0,
            Form::DiscardUnstaged
        );
    }
}
