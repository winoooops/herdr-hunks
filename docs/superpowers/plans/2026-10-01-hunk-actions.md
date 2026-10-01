# herdr-hunks hunk actions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `s`, `d` and `D` stage, unstage and discard the hunk under the cursor or the whole row behind a y/n box, in worktree scope only, with every mutation one `git apply` form on a patch sliced from the bytes of the diff on screen; and the frozen-tree defects K1-K7 are fixed with it, as release 0.0.4.

**Architecture:** The engine builds every diff it shows (D7) and keeps the bytes beside the parsed hunks, together with a pre-image (the index blob and the working-tree file's kind and hash) read before and after the diff. An action is a `Command::Act` carrying the `Arc<LoadedDiff>` the key was pressed against; it is queued as a `Change` and carried by one refresh, which slices the patch, re-checks eligibility and the pre-image, runs the `apply` forms of spec 9.2, then loads the rows. The TUI adds three bindings, one reusable y/n `dialog::Panel`, one toolbar group and the footer hints; the answer rides on the snapshot as `action_seq`, `action_error` and `action_applied`. The frozen tree gains one patch (K5); the mutators stay uncalled.

**Tech Stack:** Rust 1.88, edition 2021; the 0.0.3 dependencies unchanged (`tokio` 1 with `process`, `serde`/`serde_json` 1, `toml` 0.8, `libc` 0.2, `ratatui` 0.30 behind the `tui` feature, `crossterm` 0.29; dev `tempfile` 3). Runtime: `git` 2.31 or newer.

**Spec:** `docs/superpowers/specs/2026-09-30-hunk-actions-design.md` (section 9), on top of sections 1-6 (`2026-09-18-hunks-roadmap-p1-viewer-design.md`), 7 (`2026-09-22-branch-scope-design.md`) and 8 (`2026-09-23-review-marks-design.md`). Each task names the subsections it implements; read them before starting it.

## Global Constraints

- The vimeflow pin stays `91e45b1c` and `src/git/` is frozen. This section adds exactly one patch, `port/patches/0004-status-two-halves.patch` (Task 2), registered in `PORT-SURFACE.md`; `scripts/port-check.sh "$VIMEFLOW"` and `sh scripts/port-check-selftest.sh "$VIMEFLOW"` must pass after every task. `$VIMEFLOW` is a read-only checkout of vimeflow at that pin (on the author's machine `~/projects/vimeflow`).
- The guarantee of spec 9.6: the engine spawns only `git --version rev-parse status diff ls-files show cat-file symbolic-ref merge-base for-each-ref` and, from a confirmed `Act` only, `apply`. `tests/readonly_guarantee.rs` keeps its allow-list at ten. Every revision or object id is one argv element, never interpolated into a shell; a patch is bytes on stdin.
- Every `apply` is `git -c apply.ignoreWhitespace=no -C <toplevel> apply --whitespace=nowarn [--cached | --index] [-R]`, run from `RepoState::Repo.toplevel`.
- Every diff the engine builds carries `--no-color --no-ext-diff --no-textconv -U3 --src-prefix=a/ --dst-prefix=b/`; the frozen `get_git_diff_inner` is no longer called.
- Reserved keys stay unbound: `i I u U x X v y Y @ c /`. `s`, `d`, `D` are the only keys added; `y` and `n` are the box's keys only and are not in `KEYS`.
- The actions bind in `worktree` scope only; in `branch` scope the keys show `switch to worktree scope (b) to stage or discard` and send nothing.
- Colours are the terminal's named ANSI colours only. Every string that came from git passes through `tui::sanitize` before it reaches a cell.
- Behaviour without a confirmed action is 0.0.3's: every existing test keeps passing without being weakened, except where this plan names the test and the reason (the four-flag diff commands, the key count, the reserved set).
- Commits are conventional with a lowercase subject; inline comments are one short line and never reference a task or PR. The orchestrator makes every commit; the implementer leaves the tree uncommitted.
- `cargo test` needs a writable `HOME` outside any git repository: `CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" HOME="$(mktemp -d)" cargo test --locked -- --test-threads=1`.
- The version becomes `0.0.4` in Task 6 only; no other task touches `Cargo.toml`, `Cargo.lock` or `herdr-plugin.toml`.
- Every fix is falsified: the task's test is seen to fail against the old code before the code changes (each task's steps say which test and what failure to expect).
- Run before every commit: `cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked -- --test-threads=1 && cargo check --locked --no-default-features && scripts/port-check.sh "$VIMEFLOW" && sh scripts/port-check-selftest.sh "$VIMEFLOW"` (with the `HOME` of the line above on the test).

## Review Focus

Inputs the spec implies but no task's tests would otherwise exercise; each has its test added to the owning task.

1. A path with a space and a tab in its name, which git quotes in the `diff --git` header: the section cutter, the rename header rewrite and the pre-image must all name the same file (Task 1 test `a_quoted_path_is_cut_and_pre_imaged`, Task 3 test `a_rename_section_keeps_gits_quoting`).
2. A `y` pressed twice fast, or `s` held with key repeat: the second press after `y` must say `an action is still running` and never queue a second `Act`; the engine must still answer the first exactly once (Task 5 test `a_second_key_while_an_action_is_running_opens_nothing`, Task 4 test `a_second_act_while_one_is_queued_is_answered_without_git`).
3. A row whose diff's first hunk the size cap cut: the hunk stepper reads `1/1` but `s` must refuse, and so must `D` (Task 5 test `a_cut_diff_refuses_every_key`).
4. The watcher's own trigger for the files the action changed, arriving while the carrying refresh is in flight: exactly one follow-up refresh runs and the answer is published once (Task 4 test `the_watchers_trigger_for_the_actions_writes_coalesces`).
5. `D` on an untracked file inside a new directory (`newdir/deep/u.txt`): the patch's path has a directory, `apply -R` removes the file and git leaves the empty directory; the row disappears and nothing else changes (Task 6, in `tests/hunk_actions.rs`, case `delete_nested_untracked`).

## File Structure

```
src/engine/types.rs        PreImage, WorktreeKind, LoadedDiff.{patch, pre_image}, Snapshot.{action_seq,
                           action_error, action_applied}, ActionKind, Action, Command::Act (Tasks 1, 4)
src/engine/sections.rs     NEW: keep_sections/sections over bytes, moved out of branch.rs (Task 1)
src/engine/worktree.rs     NEW: the two worktree diff commands, rename sources for both sides, the
                           pre-image reads and hash (Task 1)
src/engine/branch.rs       diff() keeps bytes and uses sections.rs; untracked_diff() moves to worktree.rs (Task 1)
src/engine/session.rs      the diff lane and supersession check (K6), load_rows' rename probes,
                           Change::Act, the carrying refresh, answers, the acted Arc, the fresh-Arc reload,
                           the selection rule (Tasks 1, 4)
src/engine/actions.rs      NEW: Form, Direction, classify, hunk_patch, whole_patch, the apply runner (Task 3)
src/engine/mod.rs          module list (Tasks 1, 3)
src/tui/keys.rs            s d D; RESERVED (Task 5)
src/tui/confirm.rs         NEW: Confirm { kind, diff, hunk, title, body, warning, drawn }, panel() (Task 5)
src/tui/state.rs           confirm, pending_action, observe for action answers (Task 5)
src/tui/input.rs           the three keys, eligibility, the box's y/n, the guard (Task 5)
src/tui/view.rs            the toolbar group, footer hints, the box overlay, body_is_drawn (Task 5)
src/tui/shell.rs           confirm.drawn after each frame (Task 5)
src/git/mod.rs             via port/patches/0004-status-two-halves.patch only (Task 2)
PORT-SURFACE.md            patch 0004, D7, D4 amended, K1-K7 closed, the called-function list (Task 2)
src/git_diff_response_tests.rs  MD and AD in the status fixture (Task 2)
tests/readonly_guarantee.rs  the four-flag and name-status assertions (Task 1)
tests/hunk_actions.rs      NEW: the recording session of spec 9.6 (Task 6)
tests/e2e_real_herdr.rs    presses s then y (Task 6)
README.md, .zh-CN, .ja     "Hunk actions" section, keys, first line, limitations (Task 6)
AGENTS.md                  the reserved-key line and the allow-list sentence (Task 6)
docs/acceptance-p1.md      row 9 (Task 6)
Cargo.toml, Cargo.lock, herdr-plugin.toml  0.0.4 (Task 6)
```

---

### Task 1: The engine builds the worktree diffs (D7), with bytes, a pre-image and one git at a time (K6, K7)

Implements spec 9.2 "The diff behind the row", 9.4 "The diffs" and "K6", 9.5's D7 row, and 9.6's first test paragraph. Read them, and 7.2's command block, before starting.

**Files:**
- Create: `src/engine/sections.rs`, `src/engine/worktree.rs`
- Modify: `src/engine/branch.rs` (`diff`, `untracked_diff`, remove `keep_sections`/`sections`), `src/engine/types.rs` (`PreImage`, `WorktreeKind`, `LoadedDiff`), `src/engine/session.rs` (`request_diff`, `load_rows`, `run`, `SessionConfig`), `src/engine/mod.rs`, `src/tui/rows.rs`, `src/tui/state.rs` and `src/engine/types.rs` test constructors (the new `build` arguments)
- Modify: `tests/readonly_guarantee.rs` (the D7 assertions)

**Interfaces:**
- Consumes: 0.0.3's `base::git`, `base::read_head`, `branch::parse_name_status`, `branch::rows`, the frozen `parse_git_diff`, `decode_git_patch_path`, `validate_file_path`.
- Produces:

```rust
// src/engine/types.rs
/// What the working tree holds at the row's path, from lstat (spec 9.2 "Stale content").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeKind {
    Absent,
    /// A regular file: a hash of its bytes.
    File(u64),
    /// A symlink: a hash of its target's bytes.
    Symlink(u64),
    Directory,
    Other,
}

/// The pre-image a diff was read against; `None` on `LoadedDiff` when its two readings disagreed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreImage {
    /// `<mode> <object id> <stage>` of `ls-files -s`; `None` when the index has no entry.
    pub index: Option<String>,
    pub worktree: WorktreeKind,
}

// LoadedDiff gains:
pub patch: Vec<u8>,                  // the kept sections, byte for byte
pub pre_image: Option<PreImage>,
// build(key, comparison, read_at, response, patch, pre_image)
// build_with_cap(key, comparison, read_at, response, patch, pre_image, cap)

// src/engine/sections.rs
/// The `diff --git` sections of `output` whose header names `b/<path>` (and `a/<old>` for a rename).
pub fn keep_sections(output: &[u8], path: &str, old: Option<&str>) -> Vec<u8>;
/// Each section of `raw` on its own; a type change is two sections for one path.
pub fn sections(raw: &[u8]) -> Vec<&[u8]>;

// src/engine/worktree.rs
/// The row's diff (spec 9.2's two commands), cut to its sections: the response for the view, the bytes for actions.
pub(crate) async fn diff(toplevel: &str, key: &FileKey, old: Option<&str>) -> Result<(GetGitDiffResponse, Vec<u8>), String>;
/// `ls-files -s` plus lstat of the working-tree path.
pub(crate) async fn pre_image(toplevel: &str, path: &str) -> Result<PreImage, String>;
/// `R` records of `diff --name-status -M -z --`, with and without `--cached`: destination -> source.
pub(crate) async fn rename_sources(toplevel: &str) -> BTreeMap<String, String>;
pub fn hash_bytes(bytes: &[u8]) -> u64;

// src/engine/session.rs: EngineHandle is unchanged; SessionConfig is unchanged (the lane is internal).
```

One deviation from the spec's letter, recorded here and in the spec by this task: `rename_sources` stays keyed by path alone, not `(path, staged)`. A destination cannot be a rename on both sides at once (a staged rename makes it a real index entry, an intent-to-add rename needs it to be an intent-to-add entry), so one map serves both rows, and the view's existing lookup (`view.rs:361`) and the branch-scope code stay as they are.

- [ ] **Step 1: Move the section cutter to its own module, over bytes**

Create `src/engine/sections.rs` by moving `quoted_end`, `split_header`, `names_row`, `keep_sections` and `sections` out of `branch.rs` **verbatim, with their tests** (the mixed-quoting and `a b/c` cases among them; they must keep passing unchanged), then change only the two entry points to take and return bytes. `split_header` and `names_row` keep their `&str` signatures: a `diff --git` header line is matched after a lossy decode of that one line, exactly as the whole output was decoded before, while the section's bytes are kept as git wrote them.

```rust
//! Cutting a multi-section diff down to one row's sections (spec 7.2, 9.2), over bytes.

use crate::git::decode_git_patch_path;

// quoted_end, split_header and names_row: moved verbatim from branch.rs.

/// The sections whose `diff --git` header names the row: `b/<path>`, `a/<path>` for a deletion,
/// `a/<old> b/<path>` for a rename.
pub fn keep_sections(output: &[u8], path: &str, old: Option<&str>) -> Vec<u8> {
    let wanted_b = format!("b/{path}");
    let wanted_a = format!("a/{}", old.unwrap_or(path));
    let mut kept = Vec::new();
    for section in sections(output) {
        let first = section.split(|&b| b == b'\n').next().unwrap_or(&[]);
        // Only the header line is decoded, and only to match it; the section's bytes stay as read.
        let first = String::from_utf8_lossy(first);
        if let Some(header) = first.strip_prefix("diff --git ") {
            if names_row(header, &wanted_a, &wanted_b) {
                kept.extend_from_slice(section);
            }
        }
    }
    kept
}

/// Each section on its own: a `diff --git ` line at the start of the text or after a newline opens one.
pub fn sections(raw: &[u8]) -> Vec<&[u8]> {
    let marker = b"diff --git ";
    let mut starts = Vec::new();
    let mut i = 0;
    while i + marker.len() <= raw.len() {
        if &raw[i..i + marker.len()] == marker && (i == 0 || raw[i - 1] == b'\n') {
            starts.push(i);
        }
        i += 1;
    }
    starts
        .iter()
        .enumerate()
        .map(|(n, &start)| &raw[start..starts.get(n + 1).copied().unwrap_or(raw.len())])
        .collect()
}
```

Keep `split_header` public: Task 3 reuses it. The moved tests of `keep_sections` change their `&str` fixtures to `b"..."` and compare `String::from_utf8_lossy(&kept)` where they compared strings. Add `pub mod sections;` to `src/engine/mod.rs`, and replace the moved items in `branch.rs` with `use super::sections::{keep_sections, sections};`. In `branch::diff`, replace

```rust
    let raw_diff = keep_sections(&String::from_utf8_lossy(&output.stdout), path, old);
```

with

```rust
    let patch = keep_sections(&output.stdout, path, old);
    let raw_diff = String::from_utf8_lossy(&patch).into_owned();
```

and parse with `for section in sections(&patch) { let parsed = parse_git_diff(&String::from_utf8_lossy(section), path); ... }`. Make `branch::diff` return `Result<(GetGitDiffResponse, Vec<u8>), String>` and return `Ok((GetGitDiffResponse { ... raw_diff, ... }, patch))`. Add `--no-textconv` and `-U3` to its `args` after `--no-ext-diff` (spec 9.2: branch scope gains the same two flags).

- [ ] **Step 2: Run the moved tests**

Run: `cargo test --locked engine::sections`
Expected: the moved tests pass (they only changed type). `cargo build` fails at `request_diff` because `branch::diff` now returns a tuple: that is Step 5's job; for now run only the sections tests with `cargo test --locked --lib engine::sections --no-fail-fast 2>&1 | tail -5` and accept the build error until Step 5. If you prefer a green build at every step, change `request_diff`'s `Comparison::Branch` arm to `.await.map(|(r, _)| r)` temporarily.

- [ ] **Step 3: Write the failing worktree diff tests**

Add to `src/engine/worktree.rs` (the module does not exist yet; create it with the tests and `use` lines first):

```rust
//! Worktree scope's diffs, built by the engine (spec 9.2, D7): bytes an action can apply back.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::os::unix::fs::FileTypeExt;

use crate::engine::base;
use crate::engine::branch::parse_name_status;
use crate::engine::sections::{keep_sections, sections};
use crate::engine::{FileKey, PreImage, WorktreeKind};
use crate::git::{parse_git_diff, validate_file_path, FileDiff, GetGitDiffResponse};

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as Proc;

    fn git(dir: &std::path::Path, args: &[&str]) {
        assert!(
            Proc::new("git").arg("-C").arg(dir).args(args).status().unwrap().success(),
            "git {args:?}"
        );
    }

    fn git_out(dir: &std::path::Path, args: &[&str]) -> Vec<u8> {
        let out = Proc::new("git").arg("-C").arg(dir).args(args).output().unwrap();
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
        dir.path().canonicalize().unwrap().to_string_lossy().into_owned()
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
    }

    fn key(path: &str, staged: bool, untracked: bool) -> FileKey {
        FileKey { path: path.into(), staged, untracked }
    }

    /// The frozen function is the oracle: hunks must agree for every row kind of 5.2 item 4.
    #[test]
    fn hunks_agree_with_the_frozen_oracle_for_every_row_kind() {
        let dir = repo();
        let p = dir.path();
        for (name, body) in [("mm.txt", "1\n2\n3\n4\n5\n6\n7\n8\n"), ("del.txt", "d\n"), ("ren.txt", "r1\nr2\nr3\n")] {
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
        assert_eq!(renames.get("renamed.txt").map(String::as_str), Some("ren.txt"));
        for (path, staged, untracked) in [
            ("mm.txt", true, false), ("mm.txt", false, false), ("am.txt", true, false), ("am.txt", false, false),
            ("del.txt", true, false), ("renamed.txt", true, false), ("renamed.txt", false, false),
            ("newdir/deep/u.txt", false, true),
        ] {
            let k = key(path, staged, untracked);
            let old = renames.get(path).filter(|_| staged).map(String::as_str);
            let (ours, patch) = rt().block_on(diff(&toplevel, &k, old)).unwrap();
            let theirs = rt()
                .block_on(crate::git::get_git_diff_inner(cwd.clone(), path.into(), staged, Some(untracked)))
                .unwrap();
            let shape = |d: &FileDiff| d.hunks.iter().map(|h| (h.old_start, h.old_lines, h.new_start, h.new_lines, h.lines.len())).collect::<Vec<_>>();
            assert_eq!(shape(&ours.file_diff), shape(&theirs.file_diff), "{k:?}");
            let content = |d: &FileDiff| {
                d.hunks.iter().flat_map(|h| h.lines.iter().map(|l| {
                    let sign = match l.line_type { crate::git::DiffLineType::Added => '+', crate::git::DiffLineType::Removed => '-', crate::git::DiffLineType::Context => ' ' };
                    (sign, l.content.clone(), l.old_line_number, l.new_line_number)
                })).collect::<Vec<_>>()
            };
            assert_eq!(content(&ours.file_diff), content(&theirs.file_diff), "{k:?}");
            assert_eq!(ours.file_diff.old_path, theirs.file_diff.old_path, "{k:?}");
            assert_eq!(String::from_utf8_lossy(&patch), ours.raw_diff, "{k:?}: raw_diff is the lossy patch");
            // The bytes are what git prints with the same flags, run by the test itself.
            let mut args = vec!["diff"];
            if untracked { args.push("--no-index"); } else if staged { args.push("--cached"); }
            args.extend(["--no-color", "--no-ext-diff", "--no-textconv", "-U3", "--src-prefix=a/", "--dst-prefix=b/"]);
            if old.is_some() { args.push("-M"); }
            args.push("--");
            if untracked { args.push("/dev/null"); }
            if let Some(o) = old { args.push(o); }
            args.push(path);
            assert_eq!(patch, git_out(p, &args), "{k:?}: the patch bytes are git's own");
        }
    }

    #[test]
    fn the_patch_keeps_bytes_the_display_cannot() {
        let dir = repo();
        let p = dir.path();
        std::fs::write(p.join("latin.txt"), b"caf\xe9\n").unwrap();
        let toplevel = top(&dir);
        let (response, patch) = rt().block_on(diff(&toplevel, &key("latin.txt", false, true), None)).unwrap();
        assert!(patch.windows(5).any(|w| w == b"caf\xe9\n"), "the bytes survive");
        assert!(response.raw_diff.contains('\u{fffd}'), "the display is lossy");
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
        let (response, patch) = rt().block_on(diff(&toplevel, &key("tools", true, false), None)).unwrap();
        assert_eq!(sections(&patch).len(), 1, "{}", String::from_utf8_lossy(&patch));
        assert_eq!(response.file_diff.hunks.len(), 1);
        assert_eq!(response.file_diff.hunks[0].lines.len(), 1, "only `-x`");
        assert!(!response.raw_diff.contains("tools/run"));
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
        let (_, patch) = rt().block_on(diff(&toplevel, &key("a.txt", false, false), None)).unwrap();
        let text = String::from_utf8_lossy(&patch);
        assert!(text.contains("--- a/a.txt\n+++ b/a.txt\n"), "{text}");
        assert!(text.contains("@@ -1,4 +1,4 @@"), "three lines of context: {text}");
        let (response, patch) = rt().block_on(diff(&toplevel, &key("bin.dat", false, false), None)).unwrap();
        assert!(String::from_utf8_lossy(&patch).contains("Binary files"), "{}", String::from_utf8_lossy(&patch));
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
        let before = rt().block_on(pre_image(&toplevel, "f.txt")).unwrap();
        let listed = String::from_utf8_lossy(&git_out(p, &["ls-files", "-s", "--", "f.txt"])).trim().to_string();
        assert_eq!(before.index.as_deref(), Some(listed.split('\t').next().unwrap()));
        assert_eq!(before.worktree, WorktreeKind::File(hash_bytes(b"f\n")));
        std::fs::write(p.join("f.txt"), "F\n").unwrap();
        let after = rt().block_on(pre_image(&toplevel, "f.txt")).unwrap();
        assert_eq!(after.index, before.index);
        assert_ne!(after.worktree, before.worktree);
        std::fs::remove_file(p.join("f.txt")).unwrap();
        assert_eq!(rt().block_on(pre_image(&toplevel, "f.txt")).unwrap().worktree, WorktreeKind::Absent);
        std::os::unix::fs::symlink("elsewhere", p.join("f.txt")).unwrap();
        assert_eq!(rt().block_on(pre_image(&toplevel, "f.txt")).unwrap().worktree, WorktreeKind::Symlink(hash_bytes(b"elsewhere")));
        std::fs::remove_file(p.join("f.txt")).unwrap();
        std::fs::create_dir(p.join("f.txt")).unwrap();
        assert_eq!(rt().block_on(pre_image(&toplevel, "f.txt")).unwrap().worktree, WorktreeKind::Directory);
        let untracked = rt().block_on(pre_image(&toplevel, "nothere.txt")).unwrap();
        assert_eq!(untracked, PreImage { index: None, worktree: WorktreeKind::Absent });
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
        assert_eq!(renames.get("new.txt").map(String::as_str), Some("old.txt"));
        let (response, _) = rt().block_on(diff(&toplevel, &key("new.txt", false, false), Some("old.txt"))).unwrap();
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
        let (response, patch) = rt().block_on(diff(&toplevel, &key(name, false, false), None)).unwrap();
        assert_eq!(sections(&patch).len(), 1);
        assert_eq!(response.file_diff.hunks.len(), 1);
        assert_eq!(rt().block_on(pre_image(&toplevel, name)).unwrap().worktree, WorktreeKind::File(hash_bytes(b"A\n")));
    }
}
```

- [ ] **Step 4: Run them to see them fail**

Run: `cargo test --locked --lib engine::worktree`
Expected: compile errors, `diff`, `pre_image`, `rename_sources`, `hash_bytes` undefined (`PreImage`/`WorktreeKind` too until Step 5).

- [ ] **Step 5: Implement the types and the module**

In `src/engine/types.rs`, after `FileKey`, add the `WorktreeKind` and `PreImage` definitions from the interface block. Add `pub patch: Vec<u8>` and `pub pre_image: Option<PreImage>` to `LoadedDiff`, and the two arguments to `build` and `build_with_cap` (`patch: Vec<u8>, pre_image: Option<PreImage>` after `response`), storing them. Update every caller: `session.rs` (`Done::Diff` handling, Step 6), `types.rs` tests, `tui/state.rs` tests' `snapshot()` and `tui/rows.rs` tests pass `Vec::new(), None`.

Write the body of `src/engine/worktree.rs` above the tests:

```rust
const DIFF_FLAGS: [&str; 6] = [
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "-U3",
    "--src-prefix=a/",
    "--dst-prefix=b/",
];

pub fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
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

/// The index entry and the working-tree kind at `path`, both read now.
pub(crate) async fn pre_image(toplevel: &str, path: &str) -> Result<PreImage, String> {
    validate_file_path(path)?;
    let listed = base::git(toplevel, &["ls-files", "-s", "--", path]).await?;
    if !listed.status.success() {
        return Err(format!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&listed.stderr).trim()
        ));
    }
    let index = String::from_utf8_lossy(&listed.stdout)
        .lines()
        .next()
        .and_then(|line| line.split('\t').next())
        .map(str::to_string);
    let full = std::path::Path::new(toplevel).join(path);
    let worktree = match std::fs::symlink_metadata(&full) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => WorktreeKind::Absent,
        Err(e) => return Err(format!("cannot read {path}: {e}")),
        Ok(meta) if meta.file_type().is_symlink() => {
            let target = std::fs::read_link(&full).map_err(|e| format!("cannot read {path}: {e}"))?;
            WorktreeKind::Symlink(hash_bytes(target.as_os_str().as_encoded_bytes()))
        }
        Ok(meta) if meta.is_dir() => WorktreeKind::Directory,
        Ok(meta) if meta.is_file() => {
            let bytes = std::fs::read(&full).map_err(|e| format!("cannot read {path}: {e}"))?;
            WorktreeKind::File(hash_bytes(&bytes))
        }
        Ok(meta) => {
            let _ = meta.file_type().is_fifo();
            WorktreeKind::Other
        }
    };
    Ok(PreImage { index, worktree })
}

/// Renames on both sides, destination -> source. A failed probe contributes nothing: the row then shows a creation.
pub(crate) async fn rename_sources(toplevel: &str) -> BTreeMap<String, String> {
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
                        sources.insert(record.path, old);
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
```

Delete the `let _ = meta.file_type().is_fifo();` line and the `FileTypeExt` import if clippy objects; they exist only to show that `Other` covers fifos and sockets. Add `pub mod worktree;` to `src/engine/mod.rs`. In `branch.rs`, delete `untracked_diff` and its `get_git_diff_inner` import; `session.rs`'s branch untracked arm calls `worktree::untracked_diff(&toplevel, &key.path)` instead.

`DefaultHasher` is SipHash with fixed keys: stable within a process, which is all the pre-image needs (spec 9.2: it detects edits, not forgeries).

- [ ] **Step 6: The diff task: lane, supersession, bracket, pre-image**

In `session.rs`, add to `State` two fields, created in `run`'s `State { .. }` literal:

```rust
    /// K6: diff tasks run git one at a time.
    diff_lane: Arc<Semaphore>,
    /// The newest generation, read by waiting tasks to skip superseded work.
    latest_generation: Arc<std::sync::atomic::AtomicU64>,
```

initialised as `Arc::new(Semaphore::new(1))` and `Arc::new(AtomicU64::new(0))`. In `request_diff`, after `self.diff_generation += 1;` add `self.latest_generation.store(self.diff_generation, Ordering::SeqCst);`, clone `lane = self.diff_lane.clone()`, `latest = self.latest_generation.clone()` and `diffs_discarded` (add it as a parameter: `discarded: &Arc<AtomicUsize>`; the three call sites pass `&diffs_discarded`), and replace the spawned block with:

```rust
        tokio::spawn(async move {
            // One git at a time; a request superseded while it waited never spawns one.
            let _lane = lane.acquire().await.expect("diff lane closed");
            if latest.load(Ordering::SeqCst) != generation {
                discarded.fetch_add(1, Ordering::SeqCst);
                return;
            }
            // Open the bracket under the lane and before any delay or gate.
            let before = base::read_head(&toplevel).await;
            head_samples.fetch_add(1, Ordering::SeqCst);
            // The first pre-image reading precedes the delay and the gate, so the two readings
            // bracket everything that can wait; an edit landing in between leaves no pre-image.
            let pre = match &comparison {
                Comparison::Worktree => worktree::pre_image(&toplevel, &key.path).await.ok(),
                Comparison::Branch { .. } => None,
            };
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            if let Some(gate) = gate {
                gate.acquire().await.expect("diff gate closed").forget();
            }
            let result = match &comparison {
                Comparison::Branch { merge_base } if !key.untracked => {
                    branch::diff(&toplevel, merge_base, &key.path, old.as_deref()).await
                }
                Comparison::Branch { .. } => worktree::untracked_diff(&toplevel, &key.path).await,
                Comparison::Worktree => worktree::diff(&toplevel, &key, old.as_deref()).await,
            };
            let post = match &comparison {
                Comparison::Worktree => worktree::pre_image(&toplevel, &key.path).await.ok(),
                Comparison::Branch { .. } => None,
            };
            // Kept only when both readings agree; a disagreement means an edit landed mid-read.
            let pre_image = match (pre, post) {
                (Some(a), Some(b)) if a == b => Some(a),
                _ => None,
            };
            let read_at = match (before, base::read_head(&toplevel).await) {
                (Ok(Some(a)), Ok(Some(b))) if a == b => Some(a),
                _ => None,
            };
            let _ = results.send(Done::Diff { generation, key, comparison, read_at, pre_image, result });
        });
```

`Done::Diff` gains `pre_image: Option<PreImage>` and `result: Result<(GetGitDiffResponse, Vec<u8>), String>`. In the `Done::Diff` handler, build with `LoadedDiff::build(key, comparison, read_at.clone(), response, patch, pre_image)` where `Ok((response, patch))` is the result, and extend the `unchanged` test with `&& d.pre_image == pre_image` (spec 9.4: a working-tree edit can change a staged row's pre-image without changing its diff). `cwd` is no longer needed by the task: remove it from the closure (`toplevel` already falls back to `cwd` when there is no repository).

The `Comparison::Branch` untracked arm used `cwd`; `worktree::untracked_diff` takes the toplevel instead, which is what `get_git_diff_inner` resolved from the cwd anyway.

In `load_rows`, the worktree arm becomes:

```rust
        _ => (
            status.files.clone(),
            worktree::rename_sources(toplevel).await,
        ),
```

Add `use super::worktree;` next to `branch`.

- [ ] **Step 7: Run the engine tests**

Run: `cargo test --locked --lib engine -- --test-threads=1` with the `HOME` of the global constraints.
Expected: the new `engine::worktree` tests pass; every existing engine test passes. `rapid_selection_does_not_wait_for_superseded_diffs` still passes (the lane makes it faster, not slower). If `a_sample_that_could_not_run_keeps_the_previous_id` or another test counts `head_samples`, the count can only go down for superseded tasks; adjust the expected count with a one-line comment saying why.

- [ ] **Step 8: K6's own test**

Add to `session.rs` tests, after `rapid_selection_does_not_wait_for_superseded_diffs`:

```rust
    #[test]
    fn superseded_selections_never_reach_git() {
        let dir = fixture();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let h = spawn(
            rt.handle(),
            SessionConfig {
                scope: Scope::Worktree,
                base_ref: None,
                state_dir: None,
                path: dir.path().to_path_buf(),
                poll_interval: Duration::from_secs(3600),
                watcher: Arc::new(FlakyWatcher { allow: Arc::new(AtomicBool::new(true)) }),
                git_check: ok_git(),
                diff_delay: Some(Duration::from_millis(300)),
                diff_gate: None,
            },
        );
        wait_for(&h, "first", |s| ready(s).is_some());
        let samples_before = h.head_samples.load(Ordering::SeqCst);
        for _ in 0..5 {
            h.commands.send(Command::SelectNext).unwrap(); // ends on b.txt
        }
        wait_for(&h, "the last selection loaded", |s| {
            ready(s).map(|d| d.key.path == "b.txt").unwrap_or(false) && !s.refreshing
        });
        // The task in flight when the burst began and the last requested: two samples at most.
        let samples = h.head_samples.load(Ordering::SeqCst) - samples_before;
        assert!(samples <= 2, "{samples} diff tasks reached git");
        assert!(h.diffs_discarded.load(Ordering::SeqCst) >= 3, "the middle selections were not skipped");
    }
```

Run: `cargo test --locked --lib superseded_selections_never_reach_git`
Expected: PASS. Falsify it: temporarily remove the `if latest.load(...) != generation { ... return; }` block, run again, expect `5 diff tasks reached git` (or 6), then put it back.

- [ ] **Step 9: The read-only test covers D7's commands**

In `tests/readonly_guarantee.rs`, after the `--merge-base` assertion, add:

```rust
    // D7: every patch-producing diff carries the four fixed flags; the metadata commands do not.
    let patch_diffs: Vec<&str> = recorded
        .lines()
        .map(|l| l.split('\t').next().unwrap_or(""))
        .filter(|l| {
            let mut args = l.split_whitespace();
            while args.next() == Some("-C") {
                args.next();
            }
            l.contains(" diff ")
                && !l.contains("--name-status")
                && !l.contains("--numstat")
                && !l.contains("--name-only")
        })
        .collect();
    assert!(!patch_diffs.is_empty(), "no patch-producing diff was recorded");
    for line in &patch_diffs {
        for flag in ["--no-textconv", "-U3", "--src-prefix=a/", "--dst-prefix=b/"] {
            assert!(line.contains(flag), "`{line}` lacks {flag}");
        }
    }
    for side in ["diff --cached --name-status -M -z --", "diff --name-status -M -z --"] {
        assert!(
            recorded.lines().any(|l| l.contains(side)),
            "the worktree refresh never probed renames with `{side}`"
        );
    }
```

Note that the `-C <dir>` skipping loop in the filter is unnecessary; delete it and keep the three `contains` checks. Also assert that `ls-files -s --` was recorded (the pre-image): `assert!(recorded.lines().any(|l| l.contains("ls-files -s --")), "no pre-image was read");`.

Run: `HOME=... cargo test --locked --test readonly_guarantee -- --test-threads=1`
Expected: PASS, allow-list still `ALLOWED: [&str; 10]`. Falsify by removing `"-U3"` from `DIFF_FLAGS`: the test must fail naming the flag; put it back.

- [ ] **Step 10: Amend the spec's rename-key sentence**

In `docs/superpowers/specs/2026-09-30-hunk-actions-design.md`, 9.2's sentence `fills \`rename_sources\` in worktree scope too, keyed by \`(path, staged)\` there,` becomes `fills \`rename_sources\` in worktree scope too, keyed by path as in branch scope (a destination cannot be a rename on both sides at once),`; and 9.4's `keyed by \`(path, staged)\`,` is deleted. One commit carries the code and this amendment.

- [ ] **Step 11: Gates and commit**

Run the full gate line of the global constraints. Expected: all green, `port-check: src/git matches 91e45b1c + 3 patch(es)`.

```bash
git add src/engine tests/readonly_guarantee.rs src/tui docs/superpowers/specs/2026-09-30-hunk-actions-design.md
git commit -m "feat(engine): build the worktree diffs with bytes and a pre-image"
```

---

### Task 2: Patch 0004 (K5) and the port registry

Implements spec 9.5 whole: the K5 patch, the K1-K7 table, D7 and the D4 amendment in `PORT-SURFACE.md`, and 9.8 item 6. Read 9.5 and the "Port method" paragraph of spec 2.2 before starting. The frozen tree is edited only through the patch file: edit `src/git/mod.rs`, produce the patch with `git diff`, and let `port-check` prove they agree.

**Files:**
- Create: `port/patches/0004-status-two-halves.patch`
- Modify: `src/git/mod.rs` (by the patch: `parse_git_status`'s default arm and a test), `PORT-SURFACE.md`, `src/git_diff_response_tests.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces: `parse_git_status` yields two rows for `MD`, `AD`, `TM`, `MT` and one row with the right side for `T ` and ` T`.

- [ ] **Step 1: Write the failing frozen-module test**

In `src/git/mod.rs`'s test module, after `test_parse_git_status_renamed_deleted_dual_entry`:

```rust
    #[test]
    fn test_parse_git_status_splits_every_two_sided_code() {
        // X is the index side, Y the worktree side; each non-blank side is one row.
        let output = "MD md.txt\0AD ad.txt\0T  t1.txt\0 T t2.txt\0TM tm.txt\0MT mt.txt\0";
        let files = parse_git_status(output);
        let rows: Vec<(&str, bool, &str)> = files
            .iter()
            .map(|f| {
                let status = match f.status {
                    ChangedFileStatus::Modified => "M",
                    ChangedFileStatus::Added => "A",
                    ChangedFileStatus::Deleted => "D",
                    ChangedFileStatus::Renamed => "R",
                    ChangedFileStatus::Untracked => "?",
                };
                (f.path.as_str(), f.staged, status)
            })
            .collect();
        assert_eq!(
            rows,
            [
                ("md.txt", true, "M"),
                ("md.txt", false, "D"),
                ("ad.txt", true, "A"),
                ("ad.txt", false, "D"),
                ("t1.txt", true, "M"),
                ("t2.txt", false, "M"),
                ("tm.txt", true, "M"),
                ("tm.txt", false, "M"),
                ("mt.txt", true, "M"),
                ("mt.txt", false, "M"),
            ]
        );
        // A letter outside M A D T keeps the old single unstaged row.
        let files = parse_git_status("ZZ odd.txt\0");
        assert_eq!(files.len(), 1);
        assert!(!files[0].staged);
        assert!(matches!(files[0].status, ChangedFileStatus::Modified));
    }
```

Run: `cargo test --locked --lib git::tests::test_parse_git_status_splits_every_two_sided_code`
Expected: FAIL: `md.txt` is one unstaged `Modified` row (the K5 defect, seen before the fix).

- [ ] **Step 2: Replace the default arm**

In `parse_git_status`, the arm

```rust
            _ => {
                // Default to modified unstaged for unknown codes
                files.push(ChangedFile {
                    path,
                    status: ChangedFileStatus::Modified,
                    staged: false,
                    insertions: None,
                    deletions: None,
                });
            }
```

becomes

```rust
            _ => {
                // Each known side is its own row: X staged, Y unstaged. Unknown codes
                // keep the old single unstaged Modified row.
                let side = |code: u8| match code {
                    b'M' | b'T' => Some(ChangedFileStatus::Modified),
                    b'A' => Some(ChangedFileStatus::Added),
                    b'D' => Some(ChangedFileStatus::Deleted),
                    _ => None,
                };
                let bytes = xy.as_bytes();
                let staged_half = side(bytes[0]);
                let unstaged_half = side(bytes[1]);
                if staged_half.is_none() && unstaged_half.is_none() {
                    files.push(ChangedFile {
                        path,
                        status: ChangedFileStatus::Modified,
                        staged: false,
                        insertions: None,
                        deletions: None,
                    });
                } else {
                    if let Some(status) = staged_half {
                        files.push(ChangedFile {
                            path: path.clone(),
                            status,
                            staged: true,
                            insertions: None,
                            deletions: None,
                        });
                    }
                    if let Some(status) = unstaged_half {
                        files.push(ChangedFile {
                            path,
                            status,
                            staged: false,
                            insertions: None,
                            deletions: None,
                        });
                    }
                }
            }
```

`xy` is two bytes long (the parser checked `entry.len() >= 3`), so the indexing holds; a `' '` side maps to `None`, which is what makes `T ` one staged row and ` T` one unstaged row. The explicit arms above it (`MM`, `AM`, `M `, ` M`, `A `, ` A`, `D `, ` D`, the rename arm, the conflict arms) are untouched, so every existing status test keeps passing.

Run: `cargo test --locked --lib git::tests::test_parse_git_status` and `cargo fmt --check`.
Expected: all status tests pass. If `rustfmt` reflows the arm, the patch must match the formatted text: run `cargo fmt` before producing the patch (spec `AGENTS.md`: a patch matching unformatted text silently tests the wrong thing).

- [ ] **Step 3: Produce and register the patch**

```bash
git diff -- src/git/mod.rs > /tmp/0004.diff
{
  printf 'Reason: K5 (spec 9.5). parse_git_status split MM, AM and the rename codes into two rows but sent\n'
  printf 'MD, AD and the T codes to its default arm, which produced one unstaged Modified row and hid the\n'
  printf 'staged half. The default arm now splits X and Y into up to two rows; unknown letters keep the\n'
  printf 'old row. One test added beside the other parse_git_status tests.\n\n'
  cat /tmp/0004.diff
} > port/patches/0004-status-two-halves.patch
```

Check the patch applies to the pristine pin the way `port-check` applies it: `scripts/port-check.sh "$VIMEFLOW"` must print `... + 4 patch(es)` once `PORT-SURFACE.md` names it (next step); until then it fails with `patch 0004-status-two-halves.patch is not registered`, which is the registry doing its job.

- [ ] **Step 4: `PORT-SURFACE.md`**

Edit the registry:

1. "Pin" paragraph: `Four patches are registered: ... 0003-engine-visibility.patch (D6) and 0004-status-two-halves.patch (K5).`
2. "Port surface" list of called frozen functions: remove `get_git_diff_inner` and add the sentence `get_git_diff_inner is no longer called from the engine (D7); git_diff_response_tests keeps testing it and the engine's diff tests use it as their oracle.` The line `The mutating git functions are copied but are not called in Phase 1.` becomes `The mutating git functions are copied and are not called; every mutation runs through engine::actions (spec 9.2).`
3. After D6, add:

```markdown
**D7 (hunk actions): engine-built diffs.** Every diff the viewer shows is built
by the engine (`src/engine/worktree.rs`, `src/engine/branch.rs`) with
`--no-color --no-ext-diff --no-textconv -U3 --src-prefix=a/ --dst-prefix=b/`,
in both scopes, and the output bytes are kept beside the parsed hunks
(`LoadedDiff.patch`) so an action applies back exactly what was read: a
converted text (textconv) cannot be applied, a hunk cut from a diff without
context lands at the wrong line, and a prefix the user configured away cannot
be parsed by `git apply` (spec 9.2). The frozen `get_git_diff_inner` is no
longer called; it stays in the tree as the oracle of the engine's diff tests.
**D4 is amended by D7:** textconv filters are no longer left on. A binary file
with a textconv driver shows as binary and is refused by the actions; a text
file with one shows its raw text. D4's patch is unchanged because the frozen
calls it touches still exist.
```

4. "Known defects": retitle the paragraph `**K1-K7: known defects, closed in 0.0.4.**` and replace each bullet's last sentence with how 0.0.4 closes it, copying the "How 0.0.4 fixes it" column of spec 9.5's table: K1-K3 and K7 "not reached" (the engine's forms and diffs), K4 the slicer's header rewrite, K5 patch 0004, K6 the diff lane. Keep the original description of each defect so the history stays readable.

Run: `scripts/port-check.sh "$VIMEFLOW" && sh scripts/port-check-selftest.sh "$VIMEFLOW"`
Expected: `port-check: src/git matches 91e45b1c + 4 patch(es)` and the selftest's usual lines.

- [ ] **Step 5: The status fixture**

In `src/git_diff_response_tests.rs`, `status_rows_cover_the_criterion_one_fixture`: after the `AM` lines add

```rust
    std::fs::write(p.join("md.txt"), "m\n").unwrap();
    std::fs::write(p.join("ad.txt"), "x\n").unwrap();
    git(p, &["add", "md.txt"]);
    git(p, &["commit", "-q", "-m", "md"]);
    std::fs::write(p.join("md.txt"), "M\n").unwrap();
    git(p, &["add", "md.txt", "ad.txt"]);
    std::fs::remove_file(p.join("md.txt")).unwrap(); // MD
    std::fs::remove_file(p.join("ad.txt")).unwrap(); // AD
```

(the `md.txt` commit must come before `del.txt` and `ren.txt` are staged, so place these lines right after the `init` commit and move the `am.txt` lines accordingly; the row order asserted below is by membership, not position) and extend the expected rows with `("md.txt", true), ("md.txt", false), ("ad.txt", true), ("ad.txt", false)`.

Run: `cargo test --locked --lib git_diff_response_tests -- --test-threads=1` (with `HOME`).
Expected: PASS. Falsify: revert the patch (`git stash`-free: `git checkout -- src/git/mod.rs` temporarily), run again, expect `missing row ("md.txt", true)`, restore with `patch -p1 < port/patches/0004-status-two-halves.patch`.

- [ ] **Step 6: Gates and commit**

Run the full gate line. Expected: all green, four patches.

```bash
git add port/patches/0004-status-two-halves.patch src/git/mod.rs PORT-SURFACE.md src/git_diff_response_tests.rs
git commit -m "fix(git): split every two-sided status code into its rows"
```

---

### Task 3: The slicer and the apply runner

Implements spec 9.2 "The row decides the direction" (the forms), "Slicing a hunk" (sections, headers, coordinates), "What cannot be acted on" (the binary and submodule detection) and 9.4 "The runner". Read them before starting. Everything here is pure or runs git against a fixture; nothing touches the session yet.

**Files:**
- Create: `src/engine/actions.rs`
- Modify: `src/engine/mod.rs` (`pub mod actions;`)

**Interfaces:**
- Consumes: `sections::{sections, split_header}` (Task 1).
- Produces:

```rust
// src/engine/actions.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction { Forward, Reverse }

/// One `git apply` invocation of spec 9.2's table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form { StageHunk, UnstageHunk, DiscardUnstaged, DiscardStaged, StageUntracked, DeleteUntracked }
impl Form {
    pub fn flags(self) -> &'static [&'static str];   // ["--cached"], ["--cached","-R"], ["-R"], ["--index","-R"], ["--cached"], ["-R"]
    pub fn direction(self) -> Direction;             // Forward for the two stages, Reverse otherwise
    pub fn reads_index(self) -> bool;                // --cached or --index
    pub fn reads_worktree(self) -> bool;             // -R without --cached, or --index
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind { Text, Binary, Submodule }
pub fn classify(patch: &[u8]) -> Kind;

/// Hunk `index` of the kept sections (counted across sections) with its section's header, rewritten per spec 9.2.
pub fn hunk_patch(patch: &[u8], index: usize, direction: Direction) -> Option<Vec<u8>>;
/// How many hunks the kept sections hold, the same count the view shows.
pub fn hunk_count(patch: &[u8]) -> usize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// stderr named `index.lock`: worth a retry.
    Locked(String),
    /// git's first non-empty stderr line, or the spawn/timeout text.
    Failed(String),
}
pub async fn apply(toplevel: &str, form: Form, patch: &[u8]) -> Result<(), Refusal>;   // pub: no caller until the engine task, and clippy's dead-code gate runs per task
/// The same with the program and the timeout injected, for tests.
pub async fn apply_with(program: &str, timeout: Duration, toplevel: &str, form: Form, patch: &[u8]) -> Result<(), Refusal>;
```

- [ ] **Step 1: Write the slicer tests**

Create `src/engine/actions.rs` with the module doc, the `use` lines and the tests; the functions follow in Step 3:

```rust
//! Hunk patches and the apply runner (spec 9.2, 9.4). The only mutating git calls live here.

use std::time::Duration;

use tokio::io::AsyncWriteExt;

use crate::engine::sections::sections;

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
        old_text().replacen("2\n3\n", "2\nins\n3\n", 1).replacen("one\ntwo\n", "one\nTWO\n", 1)
    }

    #[test]
    fn a_hunk_carries_its_header_and_only_itself() {
        let one = hunk_patch(TWO_HUNKS, 1, Direction::Forward).unwrap();
        let text = String::from_utf8_lossy(&one);
        assert!(text.starts_with("diff --git a/f.txt b/f.txt\nindex 0ff3bbb..7647ea4 100644\n--- a/f.txt\n+++ b/f.txt\n@@ "));
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
        assert!(String::from_utf8_lossy(&r).contains("\n@@ -4 +4 @@\n"), "{}", String::from_utf8_lossy(&r));
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
        run(&hunk_patch(TWO_HUNKS, 1, Direction::Forward).unwrap(), false);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("one\nTWO\nthree\n") && !text.contains("ins\n"), "{text}");
        std::fs::write(&path, old_text()).unwrap();
        run(&hunk_patch(TWO_HUNKS, 0, Direction::Forward).unwrap(), false);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("2\nins\n3\n") && text.contains("one\ntwo\n"), "{text}");
        std::fs::write(&path, new_text()).unwrap();
        run(&hunk_patch(TWO_HUNKS, 1, Direction::Reverse).unwrap(), true);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("one\ntwo\nthree\n") && text.contains("2\nins\n3\n"), "{text}");
    }

    #[test]
    fn mode_lines_leave_a_hunk_patch_and_stay_in_the_whole_file() {
        let patch: &[u8] = b"diff --git a/r b/r\nold mode 100644\nnew mode 100755\nindex 088db5c..77a7a0a\n--- a/r\n+++ b/r\n@@ -1 +1 @@\n-a\n+b\n";
        let hunk = hunk_patch(patch, 0, Direction::Forward).unwrap();
        let text = String::from_utf8_lossy(&hunk);
        assert!(!text.contains("old mode") && !text.contains("new mode"), "{text}");
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
        assert!(String::from_utf8_lossy(&hunk).starts_with("diff --git a/new.txt b/new.txt\nindex ac33350..5dbdd84 100644\n--- a/new.txt\n+++ b/new.txt\n"));
    }

    #[test]
    fn a_type_change_counts_hunks_across_its_two_sections() {
        let patch: &[u8] = b"diff --git a/t b/t\ndeleted file mode 100644\nindex 1..2\n--- a/t\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\ndiff --git a/t b/t\nnew file mode 120000\nindex 0..3\n--- /dev/null\n+++ b/t\n@@ -0,0 +1 @@\n+target\n\\ No newline at end of file\n";
        assert_eq!(hunk_count(patch), 2);
        let second = hunk_patch(patch, 1, Direction::Forward).unwrap();
        let text = String::from_utf8_lossy(&second);
        assert!(text.starts_with("diff --git a/t b/t\nnew file mode 120000\n"), "{text}");
        assert!(text.ends_with("+target\n\\ No newline at end of file\n"));
        assert!(!text.contains("deleted file mode"));
    }

    #[test]
    fn an_empty_file_has_no_hunk_and_binary_and_submodules_are_told_apart() {
        let empty: &[u8] = b"diff --git a/e b/e\nnew file mode 100644\nindex 0000000..e69de29\n";
        assert_eq!(hunk_count(empty), 0);
        assert!(hunk_patch(empty, 0, Direction::Forward).is_none());
        assert_eq!(classify(empty), Kind::Text);
        assert_eq!(classify(b"diff --git a/b b/b\nindex 1..2\nBinary files a/b and b/b differ\n"), Kind::Binary);
        assert_eq!(classify(b"diff --git a/b b/b\nindex 1..2\nGIT binary patch\nliteral 3\n"), Kind::Binary);
        assert_eq!(classify(b"diff --git a/sub b/sub\nindex 1..2 160000\n--- a/sub\n+++ b/sub\n@@ -1 +1 @@\n-Subproject commit aaaa\n+Subproject commit bbbb\n"), Kind::Submodule);
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
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test --locked --lib engine::actions`
Expected: compile errors: `hunk_patch`, `hunk_count`, `classify`, `Form`, `Direction`, `Kind` undefined. (Add `pub mod actions;` to `src/engine/mod.rs` first so the module is compiled.)

- [ ] **Step 3: Implement the forms and the slicer**

Above the tests:

```rust
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
        matches!(self, Self::StageHunk | Self::UnstageHunk | Self::DiscardStaged | Self::StageUntracked)
    }

    pub fn reads_worktree(self) -> bool {
        matches!(self, Self::DiscardUnstaged | Self::DiscardStaged | Self::DeleteUntracked)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Binary,
    Submodule,
}

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

/// `Binary files` and `GIT binary patch` carry no payload git could apply; a gitlink is not a file.
pub fn classify(patch: &[u8]) -> Kind {
    for line in lines(patch) {
        if line.starts_with(b"Binary files ") || line.starts_with(b"GIT binary patch") {
            return Kind::Binary;
        }
        if line.starts_with(b"-Subproject commit ") || line.starts_with(b"+Subproject commit ") {
            return Kind::Submodule;
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
    let (old_start, old_count) = old.split_once(',').map(|(s, c)| (s, Some(c))).unwrap_or((old, None));
    let (new_start, new_count) = new.split_once(',').map(|(s, c)| (s, Some(c))).unwrap_or((new, None));
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
```

Run: `cargo test --locked --lib engine::actions`
Expected: the eight tests pass. If `a_type_change_counts_hunks_across_its_two_sections` fails on the trailing `\ No newline` line, check `lines()`: the last line without `\n` must still be returned (it is, by the final `if`).

- [ ] **Step 4: Write the runner tests**

Append to the tests module:

```rust
    use std::process::Command as Proc;

    fn git(dir: &std::path::Path, args: &[&str]) {
        assert!(Proc::new("git").arg("-C").arg(dir).args(args).status().unwrap().success(), "git {args:?}");
    }

    fn git_out(dir: &std::path::Path, args: &[&str]) -> String {
        String::from_utf8_lossy(&Proc::new("git").arg("-C").arg(dir).args(args).output().unwrap().stdout).into_owned()
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
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
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
        let patch = Proc::new("git").arg("-C").arg(dir.path()).args(["diff", "-U3", "--", "f.txt"]).output().unwrap().stdout;
        rt().block_on(apply(&top, Form::StageHunk, &patch)).unwrap();
        assert!(git_out(dir.path(), &["diff", "--cached"]).contains("+A\n"));
        // Staging it again finds the context gone from the index side.
        let again = rt().block_on(apply(&top, Form::StageHunk, &patch)).unwrap_err();
        match again {
            Refusal::Failed(line) => assert!(line.contains("patch failed") || line.contains("does not apply"), "{line}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_missing_program_a_lock_and_a_timeout_each_answer_their_own_way() {
        let (dir, top) = repo();
        let missing = rt().block_on(apply_with("/definitely/not/git", Duration::from_secs(5), &top, Form::StageHunk, b""));
        assert!(matches!(missing, Err(Refusal::Failed(ref m)) if m.contains("spawn")), "{missing:?}");
        let locked = fake_git(dir.path(), "echo 'fatal: Unable to create .git/index.lock: File exists.' >&2; exit 128");
        let locked = rt().block_on(apply_with(&locked, Duration::from_secs(5), &top, Form::StageHunk, b""));
        assert!(matches!(locked, Err(Refusal::Locked(ref m)) if m.contains("index.lock")), "{locked:?}");
        let slow = fake_git(dir.path(), "sleep 5");
        let started = std::time::Instant::now();
        let timed = rt().block_on(apply_with(&slow, Duration::from_millis(200), &top, Form::StageHunk, b""));
        assert_eq!(timed, Err(Refusal::Failed("git apply timed out after 0s".into())));
        assert!(started.elapsed() < Duration::from_secs(2), "the child was not killed");
        // A grandchild that keeps stdin open while the child dies: the writer must not outlive the timeout.
        let holder = fake_git(dir.path(), "sleep 5 & wait");
        let big: Vec<u8> = (0..120_000).map(|i| format!("line {i}\n")).collect::<String>().into_bytes();
        let started = std::time::Instant::now();
        let held = rt().block_on(apply_with(&holder, Duration::from_millis(200), &top, Form::StageHunk, &big));
        assert!(matches!(held, Err(Refusal::Failed(ref m)) if m.contains("timed out")), "{held:?}");
        assert!(started.elapsed() < Duration::from_secs(2), "the writer was left blocked on the grandchild's pipe");
        let silent = fake_git(dir.path(), "exit 3");
        let silent = rt().block_on(apply_with(&silent, Duration::from_secs(5), &top, Form::StageHunk, b""));
        assert_eq!(silent, Err(Refusal::Failed("git apply failed with status exit status: 3".into())));
    }

    #[test]
    fn a_large_patch_on_stdin_applies() {
        let (dir, top) = repo();
        let body: String = (0..120_000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(dir.path().join("big.txt"), &body).unwrap();
        let patch = Proc::new("git").arg("-C").arg(dir.path()).args(["diff", "--no-index", "-U3", "--", "/dev/null", "big.txt"]).output().unwrap().stdout;
        assert!(patch.len() > 1 << 20, "the fixture must exceed the pipe buffer");
        rt().block_on(apply(&top, Form::StageUntracked, &patch)).unwrap();
        assert!(git_out(dir.path(), &["ls-files", "--", "big.txt"]).contains("big.txt"));
    }
```

Run: `cargo test --locked --lib engine::actions`
Expected: the three new tests fail to compile (`apply`, `apply_with`, `Refusal` undefined).

- [ ] **Step 5: Implement the runner**

Between the slicer and the tests:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    Locked(String),
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
```

`wait_with_output` consumes the child; when the `timeout` future is dropped the child handle inside it is dropped too, and `kill_on_drop` sends SIGKILL. The D3 variables come from the process environment (`init_process_env` ran first), so nothing sets them here.

Run: `cargo test --locked --lib engine::actions` (with the `HOME` of the global constraints).
Expected: all eleven tests pass. If the timeout message in the test reads `after 0s` for a 200 ms timeout, that is the integer seconds of `Duration::as_secs`, which is what the production 30 s prints as `30s`.

- [ ] **Step 6: Gates and commit**

Run the full gate line. Expected: all green.

```bash
git add src/engine/actions.rs src/engine/mod.rs
git commit -m "feat(engine): slice hunk patches and run git apply"
```

---

### Task 4: `Command::Act`: carried by one refresh, answered once, guarded three ways

Implements spec 9.2 "The row decides the direction", "Discarding a file discards what the row shows", "What cannot be acted on" (the engine side), "Stale content", "After the action"; 9.4 "The command", "Carried by one refresh", "Answers", "Supersession"; and 9.7's engine rows. Read them before starting.

**Files:**
- Modify: `src/engine/types.rs` (`ActionKind`, `Action`, `Command::Act`, `Snapshot.{action_seq, action_error, action_applied}`)
- Modify: `src/engine/actions.rs` (`plan`, `check_pre_image`, `run`)
- Modify: `src/engine/session.rs` (`Change::Act`, `ActJob`, `Job.act`, `run_job`, `Done::Status.acted`, the `Command::Act` arm, the `Done::Status` and `Done::Diff` handlers, `State.{in_flight_act, acted, fresh_arc_pending, action_seq}`, `fingerprint`)

**Interfaces:**
- Consumes: Task 1's `LoadedDiff.{patch, pre_image}`, `worktree::pre_image`; Task 3's `Form`, `hunk_patch`, `hunk_count`, `classify`, `apply`, `Refusal`.
- Produces:

```rust
// src/engine/types.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind { Stage, Discard, DiscardFile }   // what the key means; the row gives the direction

#[derive(Debug, Clone)]
pub struct Action {
    pub kind: ActionKind,
    pub diff: Arc<LoadedDiff>,   // the diff the box was opened on
    pub hunk: Option<usize>,     // the hunk under the cursor; None for DiscardFile and for every untracked row
}
impl PartialEq for Action { /* kind, hunk, and Arc::ptr_eq on diff */ }
impl Eq for Action {}

// Command gains:
Act(Action),
// Snapshot gains (all in fingerprint):
pub action_seq: u64,
pub action_error: Option<String>,
pub action_applied: bool,

// src/engine/actions.rs
/// Spec 9.2's eligibility, decided from the Arc alone: the form and the patch, or the notice.
pub fn plan(action: &Action) -> Result<(Form, Vec<u8>), String>;
/// The pre-image the form reads must be the one the diff was read against (spec 9.2 "Stale content").
pub(crate) async fn check_pre_image(toplevel: &str, action: &Action, form: Form) -> Result<(), String>;
/// Check, apply, retry on `index.lock` for up to 2 s re-checking before each retry. `.1` is whether a form ran.
pub(crate) async fn run(toplevel: &str, action: &Action, form: Form, patch: &[u8]) -> (Result<(), String>, bool);

// notice texts, pub consts in actions.rs so the TUI (Task 5) shows the same words:
pub const NOTICE_SCOPE: &str = "switch to worktree scope (b) to stage or discard";
pub const NOTICE_NO_HUNK: &str = "no hunk under the cursor";
pub const NOTICE_CUT: &str = "diff cut by the size cap; use a shell";
pub const NOTICE_BINARY: &str = "binary file: not applied here";
pub const NOTICE_SUBMODULE: &str = "submodule: not applied here";
pub const NOTICE_NOT_A_FILE: &str = "not a regular file: not applied here";
pub const NOTICE_CHANGED: &str = "the diff changed; look again";
pub const NOTICE_RUNNING: &str = "an action is still running";
```

- [ ] **Step 1: The types**

In `src/engine/types.rs` add `ActionKind` and `Action` as above, with:

```rust
impl PartialEq for Action {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.hunk == other.hunk && Arc::ptr_eq(&self.diff, &other.diff)
    }
}
impl Eq for Action {}
```

Add `Act(Action)` to `Command` (its `derive(Debug, Clone, PartialEq, Eq)` holds through the manual impl), the three `Snapshot` fields with their doc lines (`/// Bumped once per answered Act; action_error and action_applied are that answer.`), and `action_seq: 0, action_error: None, action_applied: false` in `Snapshot::empty`. In `session.rs`'s `fingerprint`, append `s.action_seq, s.action_error, s.action_applied` with `{}|{:?}|{}` to the format string.

- [ ] **Step 2: Write the planner tests**

In `src/engine/actions.rs` tests, add a builder and the eligibility cases:

```rust
    use crate::engine::{Action, ActionKind, Comparison, FileKey, LoadedDiff, PreImage, WorktreeKind};
    use crate::git::{FileDiff, GetGitDiffResponse};
    use std::sync::Arc;

    fn loaded(path: &str, staged: bool, untracked: bool, comparison: Comparison, patch: &[u8], cap: usize) -> Arc<LoadedDiff> {
        let file_diff = crate::engine::worktree::parse_for_tests(patch, path);
        let response = GetGitDiffResponse {
            file_diff,
            old_text: String::new(),
            new_text: String::new(),
            raw_diff: String::from_utf8_lossy(patch).into_owned(),
            repo_root: "/r".into(),
        };
        let key = FileKey { path: path.into(), staged, untracked };
        let pre = Some(PreImage { index: None, worktree: WorktreeKind::Absent });
        Arc::new(LoadedDiff::build_with_cap(key, comparison, None, response, patch.to_vec(), pre, cap))
    }

    fn act(kind: ActionKind, diff: &Arc<LoadedDiff>, hunk: Option<usize>) -> Action {
        Action { kind, diff: diff.clone(), hunk }
    }

    #[test]
    fn the_plan_follows_the_row_and_refuses_what_the_spec_refuses() {
        let unstaged = loaded("f.txt", false, false, Comparison::Worktree, TWO_HUNKS, 1_000);
        let staged = loaded("f.txt", true, false, Comparison::Worktree, TWO_HUNKS, 1_000);
        assert_eq!(plan(&act(ActionKind::Stage, &unstaged, Some(1))).unwrap().0, Form::StageHunk);
        assert_eq!(plan(&act(ActionKind::Stage, &staged, Some(1))).unwrap().0, Form::UnstageHunk);
        assert_eq!(plan(&act(ActionKind::Discard, &unstaged, Some(0))).unwrap().0, Form::DiscardUnstaged);
        assert_eq!(plan(&act(ActionKind::Discard, &staged, Some(0))).unwrap().0, Form::DiscardStaged);
        let (form, whole) = plan(&act(ActionKind::DiscardFile, &staged, None)).unwrap();
        assert_eq!((form, whole.as_slice()), (Form::DiscardStaged, TWO_HUNKS));
        let (_, one) = plan(&act(ActionKind::Stage, &unstaged, Some(1))).unwrap();
        assert_eq!(one, hunk_patch(TWO_HUNKS, 1, Direction::Forward).unwrap());
        assert_eq!(plan(&act(ActionKind::Stage, &unstaged, None)).unwrap_err(), NOTICE_NO_HUNK);
        assert_eq!(plan(&act(ActionKind::Discard, &unstaged, Some(2))).unwrap_err(), NOTICE_NO_HUNK);
        let branch = loaded("f.txt", false, false, Comparison::Branch { merge_base: "m".repeat(40) }, TWO_HUNKS, 1_000);
        assert_eq!(plan(&act(ActionKind::DiscardFile, &branch, None)).unwrap_err(), NOTICE_SCOPE);
        let cut = loaded("f.txt", false, false, Comparison::Worktree, TWO_HUNKS, 3);
        assert!(cut.truncated_lines > 0);
        for (kind, hunk) in [(ActionKind::Stage, Some(0)), (ActionKind::DiscardFile, None)] {
            assert_eq!(plan(&act(kind, &cut, hunk)).unwrap_err(), NOTICE_CUT);
        }
        let binary = loaded("b", false, false, Comparison::Worktree, b"diff --git a/b b/b\nindex 1..2\nBinary files a/b and b/b differ\n", 1_000);
        assert_eq!(plan(&act(ActionKind::Stage, &binary, Some(0))).unwrap_err(), NOTICE_NO_HUNK);
        assert_eq!(plan(&act(ActionKind::DiscardFile, &binary, None)).unwrap_err(), NOTICE_BINARY);
        let untracked_binary = loaded("b", false, true, Comparison::Worktree, b"diff --git a/b b/b\nnew file mode 100644\nindex 0..2\nBinary files /dev/null and b/b differ\n", 1_000);
        for kind in [ActionKind::Stage, ActionKind::Discard, ActionKind::DiscardFile] {
            assert_eq!(plan(&act(kind, &untracked_binary, None)).unwrap_err(), NOTICE_BINARY);
        }
        let sub = loaded("sub", true, false, Comparison::Worktree, b"diff --git a/sub b/sub\nindex 1..2 160000\n--- a/sub\n+++ b/sub\n@@ -1 +1 @@\n-Subproject commit a\n+Subproject commit b\n", 1_000);
        assert_eq!(plan(&act(ActionKind::Stage, &sub, Some(0))).unwrap_err(), NOTICE_SUBMODULE);
        let untracked = loaded("u.txt", false, true, Comparison::Worktree, b"diff --git a/u.txt b/u.txt\nnew file mode 100644\nindex 0..1\n--- /dev/null\n+++ b/u.txt\n@@ -0,0 +1 @@\n+u\n", 1_000);
        assert_eq!(plan(&act(ActionKind::Stage, &untracked, None)).unwrap().0, Form::StageUntracked);
        assert_eq!(plan(&act(ActionKind::Discard, &untracked, Some(0))).unwrap().0, Form::DeleteUntracked);
        assert_eq!(plan(&act(ActionKind::DiscardFile, &untracked, None)).unwrap().0, Form::DeleteUntracked);
        let empty = loaded("e", false, true, Comparison::Worktree, b"diff --git a/e b/e\nnew file mode 100644\nindex 0000000..e69de29\n", 1_000);
        assert_eq!(plan(&act(ActionKind::Stage, &empty, None)).unwrap().0, Form::StageUntracked);
        let mode_only = loaded("m", false, false, Comparison::Worktree, b"diff --git a/m b/m\nold mode 100644\nnew mode 100755\n", 1_000);
        assert_eq!(plan(&act(ActionKind::Stage, &mode_only, None)).unwrap_err(), NOTICE_NO_HUNK);
        assert_eq!(plan(&act(ActionKind::DiscardFile, &mode_only, None)).unwrap().0, Form::DiscardUnstaged);
    }
```

`parse_for_tests` is a one-line wrapper at module level in `worktree.rs`, outside its test module so another module's tests can reach it: `#[cfg(test)] pub(crate) fn parse_for_tests(patch: &[u8], path: &str) -> FileDiff { parse_sections(patch, path) }`.

Run: `cargo test --locked --lib engine::actions::tests::the_plan`
Expected: compile error, `plan` and the `NOTICE_*` constants undefined.

- [ ] **Step 3: Implement `plan`, `check_pre_image` and `run`**

```rust
use std::time::Instant;

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

/// The side a form applies to must hold what the diff was read against; an `--index` form also needs the file present.
pub(crate) async fn check_pre_image(toplevel: &str, action: &Action, form: Form) -> Result<(), String> {
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
        if form == Form::DiscardStaged && now.worktree == WorktreeKind::Absent && now.index.is_some() {
            return Err(format!("{}: does not match index", action.diff.key.path));
        }
    }
    Ok(())
}

const LOCK_RETRY: Duration = Duration::from_secs(2);

/// Check, apply, and retry an `index.lock` collision for up to 2 s, re-checking before each retry.
pub(crate) async fn run(toplevel: &str, action: &Action, form: Form, patch: &[u8]) -> (Result<(), String>, bool) {
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
```

`applied` is true once git has been asked, including a lock refusal that is retried and then fails: a form ran, as spec 9.4 says.

Run: `cargo test --locked --lib engine::actions`
Expected: PASS.

- [ ] **Step 4: Write the session tests**

In `session.rs`'s test module, add a fixture with every row kind and a helper that sends an `Act` against the published diff and waits for its answer:

```rust
    use crate::engine::{Action, ActionKind, WorktreeKind};

    /// mm.txt: two hunks staged, two unstaged. am.txt: staged creation plus an unstaged edit.
    /// del.txt: worktree deletion. sdel.txt and sdel2.txt: staged deletions. ren.txt -> renamed.txt
    /// staged with two hunks. u.txt untracked, empty.txt untracked and empty, bin.dat untracked and binary.
    fn action_fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        git(p, &["config", "user.email", "t@example.com"]);
        git(p, &["config", "user.name", "t"]);
        let ctx = "c1\nc2\nc3\nc4\nc5\nc6\nc7\n";
        let base = format!("alpha\n{ctx}beta\n{ctx}gamma\n{ctx}delta\n");
        std::fs::write(p.join("mm.txt"), &base).unwrap();
        std::fs::write(p.join("del.txt"), "d\n").unwrap();
        std::fs::write(p.join("sdel.txt"), "s\n").unwrap();
        std::fs::write(p.join("sdel2.txt"), "s2\n").unwrap();
        std::fs::write(p.join("ren.txt"), format!("one\n{ctx}two\n")).unwrap();
        git(p, &["add", "-A"]);
        git(p, &["commit", "-q", "-m", "init"]);
        std::fs::write(p.join("mm.txt"), format!("ALPHA\n{ctx}BETA\n{ctx}gamma\n{ctx}delta\n")).unwrap();
        git(p, &["add", "mm.txt"]);
        std::fs::write(p.join("mm.txt"), format!("ALPHA\n{ctx}BETA\n{ctx}GAMMA\n{ctx}DELTA\n")).unwrap();
        std::fs::write(p.join("am.txt"), "a\n").unwrap();
        git(p, &["add", "am.txt"]);
        std::fs::write(p.join("am.txt"), "a\nb\n").unwrap();
        std::fs::remove_file(p.join("del.txt")).unwrap();
        git(p, &["rm", "-q", "sdel.txt", "sdel2.txt"]);
        git(p, &["mv", "ren.txt", "renamed.txt"]);
        std::fs::write(p.join("renamed.txt"), format!("ONE\n{ctx}TWO\n")).unwrap();
        git(p, &["add", "renamed.txt"]);
        std::fs::write(p.join("u.txt"), "u\n").unwrap();
        std::fs::write(p.join("empty.txt"), "").unwrap();
        std::fs::write(p.join("bin.dat"), [0u8, 1, 2]).unwrap();
        dir
    }

    // `git_out` already exists in this module (the branch-scope tests); reuse it.

    /// Like `start`, with the poll an hour away: only commands and the watcher move the engine.
    fn start_quiet(dir: &std::path::Path) -> (tokio::runtime::Runtime, EngineHandle) {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let handle = spawn(rt.handle(), SessionConfig {
            scope: Scope::Worktree, base_ref: None, state_dir: None, path: dir.to_path_buf(),
            poll_interval: Duration::from_secs(3600),
            watcher: Arc::new(FlakyWatcher { allow: Arc::new(AtomicBool::new(true)) }),
            git_check: ok_git(), diff_delay: None, diff_gate: None,
        });
        (rt, handle)
    }

    /// A `Refresh` first, so the listing is republished even when its last snapshot was already
    /// consumed (an unchanged repository publishes nothing); then a `Select` for a listed key.
    fn select(h: &EngineHandle, path: &str, staged: bool, untracked: bool) -> Arc<Snapshot> {
        let key = FileKey { path: path.into(), staged, untracked };
        h.commands.send(Command::Refresh).unwrap();
        wait_for(h, &format!("{key:?} listed"), |s| s.files.iter().any(|f| FileKey::of(f) == key));
        h.commands.send(Command::Select(key.clone())).unwrap();
        wait_for(h, &format!("{key:?} ready"), |s| !s.refreshing && ready(s).is_some_and(|d| d.key == key))
    }

    /// Sends the action against the very Arc the snapshot holds and returns the snapshot that answers it.
    fn act(h: &EngineHandle, s: &Snapshot, kind: ActionKind, hunk: Option<usize>) -> Arc<Snapshot> {
        let diff = match &s.diff { DiffState::Ready(d) => d.clone(), _ => panic!("no ready diff") };
        let seq = s.action_seq + 1;
        h.commands.send(Command::Act(Action { kind, diff, hunk })).unwrap();
        wait_for(h, "the answer", |n| n.action_seq == seq)
    }

    /// The post-action rows and diff: waits until the refresh and the diff reload have settled.
    fn settled(h: &EngineHandle, path: &str, staged: bool) -> Arc<Snapshot> {
        wait_for(h, "settled", |s| {
            !s.refreshing && ready(s).is_some_and(|d| d.key.path == path && d.key.staged == staged)
        })
    }

    #[test]
    fn a_hunk_is_staged_unstaged_and_discarded_and_the_rest_is_untouched() {
        let dir = action_fixture();
        let p = dir.path();
        let (_rt, h) = start_quiet(p);
        let s = select(&h, "mm.txt", false, false);
        assert_eq!(ready(&s).unwrap().file_diff.hunks.len(), 2);
        let answered = act(&h, &s, ActionKind::Stage, Some(0));
        assert_eq!(answered.action_error, None);
        assert!(answered.action_applied);
        let cached = git_out(p, &["diff", "--cached", "--", "mm.txt"]);
        assert!(cached.contains("+GAMMA") && !cached.contains("+DELTA"), "{cached}");
        let worktree = git_out(p, &["diff", "--", "mm.txt"]);
        assert!(!worktree.contains("+GAMMA") && worktree.contains("+DELTA"), "{worktree}");
        // The unstaged row survives (DELTA); the diff on screen is reloaded.
        let s = settled(&h, "mm.txt", false);
        assert_eq!(ready(&s).unwrap().file_diff.hunks.len(), 1);
        // Unstage it back from the staged row: it is the third hunk there.
        let s = select(&h, "mm.txt", true, false);
        assert_eq!(ready(&s).unwrap().file_diff.hunks.len(), 3);
        let answered = act(&h, &s, ActionKind::Stage, Some(2));
        assert_eq!(answered.action_error, None);
        assert!(!git_out(p, &["diff", "--cached", "--", "mm.txt"]).contains("+GAMMA"));
        assert!(git_out(p, &["diff", "--", "mm.txt"]).contains("+GAMMA"));
        // Discard the unstaged GAMMA hunk: the file keeps DELTA and the staged hunks.
        let s = select(&h, "mm.txt", false, false);
        assert_eq!(ready(&s).unwrap().file_diff.hunks.len(), 2);
        let answered = act(&h, &s, ActionKind::Discard, Some(0));
        assert_eq!(answered.action_error, None);
        let text = std::fs::read_to_string(p.join("mm.txt")).unwrap();
        assert!(text.contains("gamma\n") && text.contains("DELTA\n") && text.starts_with("ALPHA\n"), "{text}");
        assert!(git_out(p, &["diff", "--cached", "--", "mm.txt"]).contains("+BETA"));
    }

    #[test]
    fn a_staged_discard_is_atomic_and_refused_while_the_file_has_unstaged_changes() {
        let dir = action_fixture();
        let p = dir.path();
        let (_rt, h) = start_quiet(p);
        let s = select(&h, "mm.txt", true, false);
        let answered = act(&h, &s, ActionKind::Discard, Some(0));
        let error = answered.action_error.clone().unwrap();
        assert!(error.contains("does not match index"), "{error}");
        assert!(answered.action_applied, "git was asked and refused");
        assert!(git_out(p, &["diff", "--cached", "--", "mm.txt"]).contains("+ALPHA"));
        assert!(git_out(p, &["diff", "--", "mm.txt"]).contains("+GAMMA"));
        // A refused form on unchanged content still publishes a fresh Arc.
        let before = match &s.diff { DiffState::Ready(d) => Arc::as_ptr(d), _ => unreachable!() };
        let fresh = wait_for(&h, "a fresh arc", |n| matches!(&n.diff, DiffState::Ready(d) if d.key.staged && !std::ptr::eq(Arc::as_ptr(d), before)));
        assert_eq!(ready(&fresh).unwrap().file_diff.hunks.len(), 2);
        // Stage the rest, then the discard is clean and removes the hunk from both sides.
        git(p, &["add", "mm.txt"]);
        let s = select(&h, "mm.txt", true, false);
        assert_eq!(ready(&s).unwrap().file_diff.hunks.len(), 4);
        let answered = act(&h, &s, ActionKind::Discard, Some(1));
        assert_eq!(answered.action_error, None);
        assert!(!git_out(p, &["diff", "--cached", "--", "mm.txt"]).contains("+BETA"));
        assert!(std::fs::read_to_string(p.join("mm.txt")).unwrap().contains("beta\n"));
        assert_eq!(git_out(p, &["status", "--porcelain=v1", "--", "mm.txt"]).trim(), "M  mm.txt");
    }

    #[test]
    fn whole_file_discards_follow_the_row_and_an_md_path_keeps_its_file_absent() {
        let dir = action_fixture();
        let p = dir.path();
        let (_rt, h) = start_quiet(p);
        // D on the unstaged row of am.txt: the staged creation stays.
        let s = select(&h, "am.txt", false, false);
        assert_eq!(act(&h, &s, ActionKind::DiscardFile, None).action_error, None);
        assert_eq!(git_out(p, &["status", "--porcelain=v1", "--", "am.txt"]).trim(), "A  am.txt");
        assert_eq!(std::fs::read_to_string(p.join("am.txt")).unwrap(), "a\n");
        // D on the staged creation: index entry and file both go.
        let s = settled(&h, "am.txt", true);
        assert_eq!(act(&h, &s, ActionKind::DiscardFile, None).action_error, None);
        assert_eq!(git_out(p, &["status", "--porcelain=v1", "--", "am.txt"]).trim(), "");
        assert!(!p.join("am.txt").exists());
        // d on a worktree deletion recreates the file from the index.
        let s = select(&h, "del.txt", false, false);
        assert_eq!(act(&h, &s, ActionKind::Discard, Some(0)).action_error, None);
        assert_eq!(std::fs::read_to_string(p.join("del.txt")).unwrap(), "d\n");
        // D on a clean staged deletion: the index has no entry, so --index -R restores entry and file.
        let s = select(&h, "sdel.txt", true, false);
        assert_eq!(act(&h, &s, ActionKind::DiscardFile, None).action_error, None);
        assert_eq!(std::fs::read_to_string(p.join("sdel.txt")).unwrap(), "s\n");
        assert_eq!(git_out(p, &["status", "--porcelain=v1", "--", "sdel.txt"]).trim(), "");
        // s on a staged deletion moves it to the worktree side; D on that unstaged row recreates the file.
        let s = select(&h, "sdel2.txt", true, false);
        assert_eq!(act(&h, &s, ActionKind::Stage, Some(0)).action_error, None);
        assert_eq!(git_out(p, &["status", "--porcelain=v1", "--", "sdel2.txt"]).trim(), "D sdel2.txt");
        let s = select(&h, "sdel2.txt", false, false);
        assert_eq!(act(&h, &s, ActionKind::DiscardFile, None).action_error, None);
        assert_eq!(std::fs::read_to_string(p.join("sdel2.txt")).unwrap(), "s2\n");
        // MD: staged edit, file removed from the worktree. D on the staged row must not recreate it.
        std::fs::write(p.join("md.txt"), "m\n").unwrap();
        git(p, &["add", "md.txt"]);
        git(p, &["commit", "-q", "-m", "md"]);
        std::fs::write(p.join("md.txt"), "M\n").unwrap();
        git(p, &["add", "md.txt"]);
        std::fs::remove_file(p.join("md.txt")).unwrap();
        let s = select(&h, "md.txt", true, false);
        let answered = act(&h, &s, ActionKind::DiscardFile, None);
        assert_eq!(answered.action_error.as_deref(), Some("md.txt: does not match index"));
        assert!(!answered.action_applied, "refused before git");
        assert!(!p.join("md.txt").exists(), "git would have recreated it");
        assert!(git_out(p, &["diff", "--cached", "--", "md.txt"]).contains("+M"));
    }

    #[test]
    fn untracked_rows_are_staged_or_deleted_whole_and_a_binary_one_is_refused() {
        let dir = action_fixture();
        let p = dir.path();
        let (_rt, h) = start_quiet(p);
        let s = select(&h, "u.txt", false, true);
        assert_eq!(act(&h, &s, ActionKind::Stage, None).action_error, None);
        assert_eq!(git_out(p, &["status", "--porcelain=v1", "--", "u.txt"]).trim(), "A  u.txt");
        let s = select(&h, "empty.txt", false, true);
        assert_eq!(ready(&s).unwrap().file_diff.hunks.len(), 0);
        assert_eq!(act(&h, &s, ActionKind::Stage, None).action_error, None);
        assert_eq!(git_out(p, &["status", "--porcelain=v1", "--", "empty.txt"]).trim(), "A  empty.txt");
        let s = select(&h, "bin.dat", false, true);
        let answered = act(&h, &s, ActionKind::DiscardFile, None);
        assert_eq!(answered.action_error.as_deref(), Some(actions::NOTICE_BINARY));
        assert!(!answered.action_applied && p.join("bin.dat").exists());
        std::fs::write(p.join("gone.txt"), "g\n").unwrap();
        let s = select(&h, "gone.txt", false, true);
        assert_eq!(act(&h, &s, ActionKind::Discard, Some(0)).action_error, None);
        assert!(!p.join("gone.txt").exists());
        // The row is gone; the selection moved to a listed row.
        let s = settled_any(&h);
        assert!(s.files.iter().any(|f| Some(FileKey::of(f)) == s.selected));
    }

    fn settled_any(h: &EngineHandle) -> Arc<Snapshot> {
        wait_for(h, "settled", |s| !s.refreshing && ready(s).is_some())
    }

    #[test]
    fn one_hunk_of_a_staged_rename_is_unstaged_and_the_rename_stands() {
        let dir = action_fixture();
        let p = dir.path();
        let (_rt, h) = start_quiet(p);
        let s = select(&h, "renamed.txt", true, false);
        assert_eq!(ready(&s).unwrap().file_diff.old_path.as_deref(), Some("ren.txt"));
        assert_eq!(ready(&s).unwrap().file_diff.hunks.len(), 2);
        assert_eq!(act(&h, &s, ActionKind::Stage, Some(0)).action_error, None);
        assert_eq!(git_out(p, &["status", "--porcelain=v1", "--", "ren.txt", "renamed.txt"]).trim(), "RM ren.txt -> renamed.txt");
        assert!(git_out(p, &["diff", "--cached", "-M"]).contains("rename from ren.txt"));
        assert!(git_out(p, &["diff", "--", "renamed.txt"]).contains("+ONE"));
    }

    #[test]
    fn a_type_change_row_is_discarded_whole() {
        let dir = action_fixture();
        let p = dir.path();
        std::fs::remove_file(p.join("del.txt")).ok();
        std::os::unix::fs::symlink("u.txt", p.join("del.txt")).unwrap();
        let (_rt, h) = start_quiet(p);
        let s = select(&h, "del.txt", false, false);
        assert_eq!(ready(&s).unwrap().file_diff.hunks.len(), 2, "a deletion and a creation");
        assert_eq!(act(&h, &s, ActionKind::DiscardFile, None).action_error, None);
        assert!(p.join("del.txt").symlink_metadata().unwrap().is_file());
        assert_eq!(std::fs::read_to_string(p.join("del.txt")).unwrap(), "d\n");
    }

    #[test]
    fn every_act_is_answered_once_and_the_refusals_run_no_git() {
        let dir = action_fixture();
        let p = dir.path();
        let (_rt, h) = start_quiet(p);
        let s = select(&h, "mm.txt", false, false);
        let diff = match &s.diff { DiffState::Ready(d) => d.clone(), _ => unreachable!() };
        // Out of range: the engine decides eligibility itself.
        let a = act(&h, &s, ActionKind::Stage, Some(9));
        assert_eq!((a.action_seq, a.action_error.as_deref(), a.action_applied), (1, Some(actions::NOTICE_NO_HUNK), false));
        // Scope: a diff really published in branch scope (the fixture's base is refs/heads/main).
        h.commands.send(Command::SetScope(Scope::Branch)).unwrap();
        let b = wait_for(&h, "branch diff", |n| n.scope == Scope::Branch && ready(n).is_some() && !n.refreshing);
        let a = act(&h, &b, ActionKind::DiscardFile, None);
        assert_eq!((a.action_seq, a.action_error.as_deref()), (2, Some(actions::NOTICE_SCOPE)));
        h.commands.send(Command::SetScope(Scope::Worktree)).unwrap();
        wait_for(&h, "worktree back", |n| n.scope == Scope::Worktree && !n.refreshing);
        let s = select(&h, "mm.txt", false, false);
        let diff = match &s.diff { DiffState::Ready(d) => d.clone(), _ => unreachable!() };
        // A stale Arc: a clone with the same content is not the published one.
        h.commands.send(Command::Act(Action { kind: ActionKind::Stage, diff: Arc::new((*diff).clone()), hunk: Some(0) })).unwrap();
        let a = wait_for(&h, "stale answer", |n| n.action_seq == 3);
        assert_eq!(a.action_error.as_deref(), Some(actions::NOTICE_CHANGED));
        assert!(git_out(p, &["diff", "--cached", "--", "mm.txt"]).contains("+ALPHA") && !git_out(p, &["diff", "--cached"]).contains("+GAMMA"));
        // Two in a row: the second is answered without git while the first is queued or in flight.
        let first = Action { kind: ActionKind::Stage, diff: diff.clone(), hunk: Some(0) };
        h.commands.send(Command::Act(first.clone())).unwrap();
        h.commands.send(Command::Act(Action { hunk: Some(1), ..first })).unwrap();
        let a = wait_for(&h, "both answered", |n| n.action_seq == 5);
        // One of the two applied GAMMA, the other was refused as running; the index holds exactly one new hunk.
        let cached = git_out(p, &["diff", "--cached", "--", "mm.txt"]);
        assert!(cached.contains("+GAMMA") && !cached.contains("+DELTA"), "{cached}");
    }

    #[test]
    fn an_acted_on_arc_is_refused_until_its_replacement_arrives() {
        let dir = action_fixture();
        let p = dir.path();
        let gate = Arc::new(Semaphore::new(0));
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let h = spawn(rt.handle(), SessionConfig {
            scope: Scope::Worktree, base_ref: None, state_dir: None, path: p.to_path_buf(),
            poll_interval: Duration::from_secs(3600),
            watcher: Arc::new(FlakyWatcher { allow: Arc::new(AtomicBool::new(true)) }),
            git_check: ok_git(), diff_delay: None, diff_gate: Some(gate.clone()),
        });
        gate.add_permits(1);
        wait_for(&h, "first", |s| ready(s).is_some());
        gate.add_permits(2); // `select` requests the diff twice: once for its Refresh, once for the Select
        let s = select(&h, "u.txt", false, true);
        let diff = match &s.diff { DiffState::Ready(d) => d.clone(), _ => unreachable!() };
        // The reload the carrying refresh requests waits at the gate, so the acted Arc stays on screen.
        h.commands.send(Command::Act(Action { kind: ActionKind::Stage, diff: diff.clone(), hunk: None })).unwrap();
        let a = wait_for(&h, "answered", |n| n.action_seq == 1);
        assert_eq!((a.action_error.as_deref(), a.action_applied), (None, true));
        assert!(matches!(&a.diff, DiffState::Ready(d) if Arc::ptr_eq(d, &diff)), "the old Arc is still published");
        h.commands.send(Command::Act(Action { kind: ActionKind::Stage, diff: diff.clone(), hunk: None })).unwrap();
        let a = wait_for(&h, "the acted arc's answer", |n| n.action_seq == 2);
        assert_eq!((a.action_error.as_deref(), a.action_applied), (Some(actions::NOTICE_CHANGED), false));
        assert_eq!(git_out(p, &["status", "--porcelain=v1", "--", "u.txt"]).trim(), "A  u.txt", "staged once");
        gate.add_permits(1);
        wait_for(&h, "a fresh arc", |n| matches!(&n.diff, DiffState::Ready(d) if !Arc::ptr_eq(d, &diff)));
    }

    #[test]
    fn a_second_act_while_one_is_queued_is_answered_without_git() {
        let dir = action_fixture();
        let p = dir.path();
        let gate = Arc::new(Semaphore::new(0));
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let h = spawn(rt.handle(), SessionConfig {
            scope: Scope::Worktree, base_ref: None, state_dir: None, path: p.to_path_buf(),
            poll_interval: Duration::from_secs(3600),
            watcher: Arc::new(FlakyWatcher { allow: Arc::new(AtomicBool::new(true)) }),
            git_check: ok_git(), diff_delay: None, diff_gate: Some(gate.clone()),
        });
        gate.add_permits(1);
        let s = wait_for(&h, "first", |s| ready(s).is_some());
        let s = { gate.add_permits(1); select(&h, "mm.txt", false, false) };
        let diff = match &s.diff { DiffState::Ready(d) => d.clone(), _ => unreachable!() };
        let before = h.refreshes.load(Ordering::SeqCst);
        h.commands.send(Command::Act(Action { kind: ActionKind::Stage, diff: diff.clone(), hunk: Some(0) })).unwrap();
        h.commands.send(Command::Act(Action { kind: ActionKind::Stage, diff: diff.clone(), hunk: Some(1) })).unwrap();
        let second = wait_for(&h, "the running refusal", |n| n.action_error.as_deref() == Some(actions::NOTICE_RUNNING));
        assert_eq!(second.action_seq, 1, "the refusal is answered first, without a refresh");
        gate.add_permits(4);
        let first = wait_for(&h, "the first answer", |n| n.action_seq == 2);
        assert_eq!(first.action_error, None);
        assert!(h.refreshes.load(Ordering::SeqCst) - before >= 1);
    }

    #[test]
    fn a_pre_image_that_changed_refuses_the_form_before_git() {
        let dir = action_fixture();
        let p = dir.path();
        let (_rt, h) = start_quiet(p);
        let s = select(&h, "mm.txt", false, false);
        // The watcher never emits and the poll is an hour away (`start_quiet`): the engine does not see this edit.
        let ctx = "c1\nc2\nc3\nc4\nc5\nc6\nc7\n";
        std::fs::write(p.join("mm.txt"), format!("ALPHA\n{ctx}BETA\n{ctx}GAMMA\n{ctx}DELTA\nextra\n")).unwrap();
        let a = act(&h, &s, ActionKind::Discard, Some(0));
        assert_eq!(a.action_error.as_deref(), Some(actions::NOTICE_CHANGED));
        assert!(!a.action_applied);
        assert!(std::fs::read_to_string(p.join("mm.txt")).unwrap().contains("GAMMA\n"));
        // A `--cached` form reads the index: an index change refuses it the same way.
        let s = select(&h, "mm.txt", false, false);
        assert!(ready(&s).unwrap().raw_diff.contains("+extra"));
        git(p, &["add", "mm.txt"]);
        let a = act(&h, &s, ActionKind::Stage, Some(0));
        assert_eq!(a.action_error.as_deref(), Some(actions::NOTICE_CHANGED));
    }

    #[test]
    fn a_working_tree_form_on_a_directory_is_refused() {
        let dir = action_fixture();
        let p = dir.path();
        std::fs::write(p.join("tools"), "x\n").unwrap();
        git(p, &["add", "tools"]);
        git(p, &["commit", "-q", "-m", "tools"]);
        git(p, &["rm", "-q", "tools"]);
        std::fs::create_dir(p.join("tools")).unwrap();
        std::fs::write(p.join("tools/run"), "r\n").unwrap();
        git(p, &["add", "tools/run"]);
        let (_rt, h) = start_quiet(p);
        let s = select(&h, "tools", true, false);
        assert_eq!(ready(&s).unwrap().pre_image.as_ref().map(|i| i.worktree.clone()), Some(WorktreeKind::Directory));
        let a = act(&h, &s, ActionKind::DiscardFile, None);
        assert_eq!(a.action_error.as_deref(), Some(actions::NOTICE_NOT_A_FILE));
        assert!(!a.action_applied && p.join("tools/run").exists());
        // The index-only form is fine: unstaging the deletion needs no file.
        assert_eq!(act(&h, &settled(&h, "tools", true), ActionKind::Stage, Some(0)).action_error, None);
        assert!(git_out(p, &["ls-files", "--", "tools"]).contains("tools"));
    }

    #[test]
    fn an_edit_during_the_diff_read_leaves_no_pre_image_and_refuses_the_key() {
        let dir = action_fixture();
        let p = dir.path();
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let h = spawn(rt.handle(), SessionConfig {
            scope: Scope::Worktree, base_ref: None, state_dir: None, path: p.to_path_buf(),
            poll_interval: Duration::from_secs(3600),
            watcher: Arc::new(FlakyWatcher { allow: Arc::new(AtomicBool::new(true)) }),
            git_check: ok_git(), diff_delay: Some(Duration::from_millis(400)), diff_gate: None,
        });
        let s = select(&h, "u.txt", false, true);
        assert!(ready(&s).unwrap().pre_image.is_some());
        // The reload sleeps between its first pre-image read and the diff; the edit lands in that window.
        h.commands.send(Command::Refresh).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        std::fs::write(p.join("u.txt"), "u\nmore\n").unwrap();
        let s = wait_for(&h, "the reload", |n| !n.refreshing && ready(n).is_some_and(|d| d.raw_diff.contains("+more")));
        assert!(ready(&s).unwrap().pre_image.is_none(), "the two readings disagreed");
        let a = act(&h, &s, ActionKind::Stage, None);
        assert_eq!((a.action_error.as_deref(), a.action_applied), (Some(actions::NOTICE_CHANGED), false));
        // The next reload, with nothing moving, carries a pre-image again.
        h.commands.send(Command::Refresh).unwrap();
        let s = wait_for(&h, "settled reload", |n| !n.refreshing && ready(n).is_some_and(|d| d.pre_image.is_some()));
        assert_eq!(act(&h, &s, ActionKind::Stage, None).action_error, None);
    }

    #[test]
    fn an_index_lock_is_retried_and_given_up_after_two_seconds() {
        let dir = action_fixture();
        let p = dir.path();
        let (_rt, h) = start_quiet(p);
        let s = select(&h, "u.txt", false, true);
        // Held for half a second: the retry absorbs it.
        std::fs::write(p.join(".git/index.lock"), "").unwrap();
        let lock = p.join(".git/index.lock");
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            std::fs::remove_file(lock).unwrap();
        });
        let started = Instant::now();
        let a = act(&h, &s, ActionKind::Stage, None);
        release.join().unwrap();
        assert_eq!(a.action_error, None);
        assert!(started.elapsed() >= Duration::from_millis(400), "the first attempt must have hit the lock");
        assert!(git_out(p, &["ls-files", "--", "u.txt"]).contains("u.txt"));
        // Held past the bound: git's own message, after two seconds.
        std::fs::write(p.join("v.txt"), "v\n").unwrap();
        let s = select(&h, "v.txt", false, true);
        std::fs::write(p.join(".git/index.lock"), "").unwrap();
        let started = Instant::now();
        let a = act(&h, &s, ActionKind::Stage, None);
        std::fs::remove_file(p.join(".git/index.lock")).unwrap();
        assert!(a.action_error.as_deref().is_some_and(|e| e.contains("index.lock")), "{:?}", a.action_error);
        assert!(a.action_applied);
        assert!(started.elapsed() >= Duration::from_secs(2));
    }

    #[test]
    fn a_diff_read_before_the_action_is_never_published_after_it() {
        let dir = action_fixture();
        let p = dir.path();
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let h = spawn(rt.handle(), SessionConfig {
            scope: Scope::Worktree, base_ref: None, state_dir: None, path: p.to_path_buf(),
            poll_interval: Duration::from_secs(3600),
            watcher: Arc::new(FlakyWatcher { allow: Arc::new(AtomicBool::new(true)) }),
            git_check: ok_git(), diff_delay: Some(Duration::from_millis(400)), diff_gate: None,
        });
        let s = select(&h, "u.txt", false, true);
        let diff = match &s.diff { DiffState::Ready(d) => d.clone(), _ => unreachable!() };
        let discarded_before = h.diffs_discarded.load(Ordering::SeqCst);
        // A slow reload of the row is in flight when the action is queued.
        h.commands.send(Command::Refresh).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        h.commands.send(Command::Act(Action { kind: ActionKind::Stage, diff, hunk: None })).unwrap();
        let answered = wait_for(&h, "answered", |n| n.action_seq == 1);
        assert_eq!(answered.action_error, None);
        let after = wait_for(&h, "the post-action diff", |n| !n.refreshing && ready(n).is_some_and(|d| d.key.path == "u.txt" && d.key.staged));
        assert!(h.diffs_discarded.load(Ordering::SeqCst) > discarded_before, "the pre-action read was published");
        assert!(ready(&after).unwrap().raw_diff.contains("+u"));
    }

    #[test]
    fn an_act_outside_a_repository_is_answered_not_a_git_repository() {
        let dir = tempfile::tempdir().unwrap();
        let (_rt, h) = start_quiet(dir.path());
        let s = wait_for(&h, "not a repo", |s| matches!(s.repo, RepoState::NotARepo { .. }));
        let diff = Arc::new(LoadedDiff::build(
            FileKey { path: "x".into(), staged: false, untracked: true },
            Comparison::Worktree,
            None,
            crate::git::GetGitDiffResponse { file_diff: crate::git::FileDiff { file_path: "x".into(), old_path: None, new_path: None, hunks: Vec::new() }, old_text: String::new(), new_text: String::new(), raw_diff: String::new(), repo_root: String::new() },
            Vec::new(),
            None,
        ));
        h.commands.send(Command::Act(Action { kind: ActionKind::Stage, diff, hunk: None })).unwrap();
        let a = wait_for(&h, "answer", |n| n.action_seq == 1);
        assert_eq!(a.action_error.as_deref(), Some("the diff changed; look again"), "no diff is published, so identity fails first; {}", s.revision);
    }

    #[test]
    fn a_mark_and_an_action_queued_together_answer_in_order() {
        let dir = action_fixture();
        let p = dir.path();
        let (_rt, h) = start_quiet(p);
        let s = select(&h, "u.txt", false, true);
        let head = git_out(p, &["rev-parse", "HEAD"]).trim().to_string();
        let diff = match &s.diff { DiffState::Ready(d) => d.clone(), _ => unreachable!() };
        h.commands.send(Command::MarkReviewed(head)).unwrap();
        h.commands.send(Command::Act(Action { kind: ActionKind::Stage, diff, hunk: None })).unwrap();
        let s = wait_for(&h, "both", |n| n.mark_seq == 1 && n.action_seq == 1);
        assert_eq!(s.action_error, None);
        assert_eq!(s.mark_error.as_deref(), Some("mark not remembered: no state directory"));
    }

    #[test]
    fn the_watchers_trigger_for_the_actions_writes_coalesces() {
        let dir = action_fixture();
        let p = dir.path();
        let slot = Arc::new(std::sync::Mutex::new(None));
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let h = spawn(rt.handle(), SessionConfig {
            scope: Scope::Worktree, base_ref: None, state_dir: None, path: p.to_path_buf(),
            poll_interval: Duration::from_secs(3600),
            watcher: Arc::new(EmittingWatcher { sink: slot.clone() }),
            git_check: ok_git(), diff_delay: None, diff_gate: None,
        });
        let s = select(&h, "u.txt", false, true);
        let sink = wait_until(|| slot.lock().unwrap().clone());
        let before = h.refreshes.load(Ordering::SeqCst);
        let diff = match &s.diff { DiffState::Ready(d) => d.clone(), _ => unreachable!() };
        h.commands.send(Command::Act(Action { kind: ActionKind::Stage, diff, hunk: None })).unwrap();
        for _ in 0..5 {
            sink.emit_json("git-status-changed", serde_json::json!({ "cwds": [] })).unwrap();
        }
        let answered = wait_for(&h, "answered", |n| n.action_seq == 1);
        assert_eq!(answered.action_error, None);
        std::thread::sleep(Duration::from_millis(800));
        let runs = h.refreshes.load(Ordering::SeqCst) - before;
        assert!((1..=2).contains(&runs), "the carrying refresh and at most one follow-up, got {runs}");
        assert_eq!(wait_for(&h, "settled", |n| !n.refreshing).action_seq, 1, "answered once");
    }
```

Run: `cargo test --locked --lib engine::session -- --test-threads=1` (with `HOME`).
Expected: compile errors: `Command::Act` exists (Step 1), but the engine answers nothing: once compiled, every new test times out in `wait_for` with `last = ...` showing `action_seq: 0`. That is the failure to see before Step 5.

- [ ] **Step 5: Wire the engine**

In `session.rs`:

1. `Change` gains `Act(ActJob)`, where

```rust
/// A planned action: its form and patch are decided when the command arrives, the pre-image when the refresh runs it.
#[derive(Debug, Clone)]
struct ActJob {
    action: Action,
    form: actions::Form,
    patch: Vec<u8>,
    /// Decided when the refresh popped it: the Arc was replaced in the meantime.
    refusal: Option<String>,
}
```

   (`Change` derives `Debug, Clone`; `Action` is both.) `Job` gains `toplevel: Option<String>`.

2. `State` gains:

```rust
    /// An action is queued or being carried; a second one is answered at once.
    act_pending: bool,
    /// The Arc a form ran on: never eligible again until a later diff replaces it.
    acted: Option<Arc<LoadedDiff>>,
    /// The first diff after an attempted action skips the unchanged shortcut.
    fresh_arc_pending: bool,
    action_seq: u64,
```

3. The command arm, before the `selection =>` arm:

```rust
                    Command::Act(action) => {
                        let published = match &state.snapshot.diff {
                            DiffState::Ready(d) => Some(d),
                            _ => None,
                        };
                        let same = published.is_some_and(|d| Arc::ptr_eq(d, &action.diff));
                        let acted = state.acted.as_ref().is_some_and(|a| Arc::ptr_eq(a, &action.diff));
                        let toplevel = match &state.snapshot.repo {
                            RepoState::Repo { toplevel, .. } => Some(toplevel.clone()),
                            _ => None,
                        };
                        // Decided here, without git: concurrency, identity, eligibility, a repository.
                        let planned = if state.act_pending {
                            Err(actions::NOTICE_RUNNING.to_string())
                        } else if !same || acted {
                            Err(actions::NOTICE_CHANGED.to_string())
                        } else if toplevel.is_none() {
                            Err("not a git repository".to_string())
                        } else {
                            actions::plan(&action)
                        };
                        match planned {
                            Err(refusal) => {
                                state.action_seq += 1;
                                let mut next = state.snapshot.clone();
                                next.action_seq = state.action_seq;
                                next.action_error = Some(refusal);
                                next.action_applied = false;
                                publish(&mut state, next, &snapshots);
                            }
                            Ok((form, patch)) => {
                                state.act_pending = true;
                                state.changes.push_back(Change::Act(ActJob { action, form, patch, refusal: None }));
                                let mut next = state.snapshot.clone();
                                next.refreshing = true;
                                publish(&mut state, next, &snapshots);
                                state.request_status(&cwd, false, &results_tx, &refreshes);
                            }
                        }
                    }
```

4. In `request_status`, after `let change = self.changes.pop_front();`:

```rust
        let change = match change {
            Some(Change::Act(mut job)) => {
                // A diff result may have replaced the Arc while the action waited in the queue.
                let same = matches!(&self.snapshot.diff, DiffState::Ready(d) if Arc::ptr_eq(d, &job.action.diff));
                if !same {
                    job.refusal = Some(actions::NOTICE_CHANGED.to_string());
                }
                Some(Change::Act(job))
            }
            other => other,
        };
```

   then, still in `request_status`:

```rust
        if let Some(Change::Act(_)) = &change {
            // Nothing read before the mutation may be published after it: superseded now, before any form runs.
            self.diff_generation += 1;
            self.latest_generation.store(self.diff_generation, Ordering::SeqCst);
            self.diff_in_flight = None;
        }
```

   and in the `scope`/`base` matches treat `Change::Act(_)` like `Change::Mark(_)` (same scope, `Keep` unless `resolve_pending`). Set `toplevel` on the `Job` from `self.snapshot.repo`, and give `Job` a `lane: Arc<Semaphore>` field cloned from `self.diff_lane`.

5. In `run_job`, before `let sampled = ...`:

```rust
    let acted = match (&job.change, &job.toplevel) {
        (Some(Change::Act(act)), Some(toplevel)) => Some(match &act.refusal {
            Some(refusal) => (Err(refusal.clone()), false),
            None => {
                // No diff task reads git while a form runs; one started earlier was superseded at the pop.
                let _lane = job.lane.acquire().await.expect("diff lane closed");
                actions::run(toplevel, &act.action, act.form, &act.patch).await
            }
        }),
        (Some(Change::Act(_)), None) => Some((Err("not a git repository".to_string()), false)),
        _ => None,
    };
```

   and add `acted` to `Done::Status` (`acted: Option<(Result<(), String>, bool)>`). The forms run before the status read, as spec 9.4 orders it.

6. In the `Done::Status` handler, right after `state.in_flight_mark = None;`:

```rust
                        if let (Some(Change::Act(act)), Some((result, applied))) = (&change, &acted) {
                            state.act_pending = false;
                            state.action_seq += 1;
                            next.action_seq = state.action_seq;
                            next.action_error = result.clone().err();
                            next.action_applied = *applied;
                            if *applied {
                                state.acted = Some(act.action.diff.clone());
                                state.fresh_arc_pending = true;
                            }
                        }
```

   `load_rows` must not treat `Change::Act` as a `Mark` or `Base`: audit each `match &change` / `matches!(change, ...)` in `load_rows` and the handler; `Act` behaves as `None` there (its own branches above are the only ones that look at it). In the selection block, after `next.selected = ...` is computed, add the same-path rule:

```rust
                                        if let Some(Change::Act(act)) = &change {
                                            let key = &act.action.diff.key;
                                            let listed = next.files.iter().any(|f| &key_of(f) == key);
                                            if !listed {
                                                if let Some(sibling) = next.files.iter().find(|f| f.path == key.path) {
                                                    next.selected = Some(key_of(sibling));
                                                }
                                            }
                                        }
```

7. In the `Done::Diff` handler, the `unchanged` test becomes:

```rust
                                    let fresh = std::mem::take(&mut state.fresh_arc_pending);
                                    let unchanged = !fresh
                                        && matches!(&next.diff, DiffState::Ready(d) if d.key == key && d.comparison == comparison && d.raw_diff == response.raw_diff && d.read_at == read_at && d.pre_image == pre_image);
```

   and after a new `Ready` is published for that key, clear `state.acted` when the Arc it names is no longer the published one: `if state.acted.as_ref().is_some_and(|a| !matches!(&next.diff, DiffState::Ready(d) if Arc::ptr_eq(a, d))) { state.acted = None; }`.

8. Import `super::actions` and `Action` in `session.rs`.

Run: `cargo test --locked --lib engine -- --test-threads=1` (with `HOME`).
Expected: every test passes, including the seventeen new ones. Falsify three of them by hand before committing: comment out the `fresh_arc_pending` line (the staged-discard test must fail at "a fresh arc"); comment out the `DiscardStaged && Absent && index.is_some()` check in `check_pre_image` (the MD test must fail at "git would have recreated it"); comment out the `acted` check in the command arm (`an_acted_on_arc_is_refused_until_its_replacement_arrives` must fail at "the acted arc's answer", with the gate holding the reload so identity alone would have passed). Restore each.

Note: the spec's sentence on `--index` forms and absent files was amended during planning (review round 2), so it already says what `check_pre_image` does.

- [ ] **Step 6: Gates and commit**

Run the full gate line. Expected: all green; `readonly_guarantee` still passes with its ten subcommands, because nothing in its session sends an `Act`.

```bash
git add src/engine
git commit -m "feat(engine): stage, unstage and discard through one confirmed apply"
```

---

### Task 5: The keys, the y/n box, the toolbar group, the footer and the notices

Implements spec 9.3 whole, and the TUI side of 9.2 "What cannot be acted on" and 9.7. Read 9.3, then 8.5's notice rules and the invariant-3 paragraph of `AGENTS.md`, before starting.

**Files:**
- Create: `src/tui/confirm.rs`
- Modify: `src/tui/keys.rs`, `src/tui/state.rs`, `src/tui/input.rs`, `src/tui/view.rs`, `src/tui/shell.rs`, `src/tui/format.rs`, `src/tui/mod.rs`

**Interfaces:**
- Consumes: `actions::{plan, NOTICE_*}` (Task 4), `Command::Act`, `Snapshot.{action_seq, action_error, action_applied}`.
- Produces:

```rust
// src/tui/keys.rs: KeyAction gains StageHunk, DiscardHunk, DiscardFile; KEYS gains s, d, D (27 bindings);
pub const RESERVED: &[&str] = &["i", "I", "u", "U", "x", "X", "v", "y", "Y", "@", "c", "/"];

// src/tui/format.rs
/// `…` plus the last `max - 1` cells of `text` when it is wider than `max`.
pub fn truncate_left(text: &str, max: usize) -> String;

// src/tui/confirm.rs
pub struct Confirm {
    pub action: Action,
    pub title: &'static str,
    pub warning: Option<&'static str>,
    /// Set after each frame: the whole box was on screen (spec 9.3), so `y` may confirm.
    pub drawn: bool,
    /// The success notice (`staged hunk 2/3 of src/x`) and the failure prefix (`stage`).
    pub done: String,
    pub verb: &'static str,
}
impl Confirm {
    pub fn open(kind: ActionKind, diff: Arc<LoadedDiff>, hunk: Option<usize>, hunk_total: usize) -> Self;
    /// The panel at `width`: body at most two lines (the path shortened from the left), the warning row, footer `y yes · n no`.
    pub fn panel(&self, width: u16) -> Panel;
}

// src/tui/state.rs
pub struct PendingAction { pub diff: Arc<LoadedDiff>, pub done: String, pub verb: &'static str, pub answered: bool }
// ViewState gains: pub confirm: Option<Confirm>, pub pending_action: Option<PendingAction>, seen_action_seq: u64
impl ViewState {
    /// Spec 9.3's guard: unanswered, or answered as applied while its Arc is still on screen.
    pub fn action_running(&self, snapshot: &Snapshot) -> bool;
}

// src/tui/view.rs: Action gains StageHunk, DiscardHunk, DiscardFile (key_action maps them)
```

- [ ] **Step 1: Teach the test snapshot helper real patches**

`state::tests::snapshot(path, raw, hunks)` builds `LoadedDiff`s for every TUI test; the box needs a `patch` with real hunks and a pre-image. In `src/tui/state.rs` tests, change the helper to synthesize both:

```rust
        let mut patch = format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n");
        for (start, kinds) in hunks {
            let old = kinds.chars().filter(|c| *c != '+').count();
            let new = kinds.chars().filter(|c| *c != '-').count();
            patch.push_str(&format!("@@ -{start},{old} +{start},{new} @@\n"));
            for k in kinds.chars() {
                patch.push_str(&format!("{}line {k}\n", if k == '+' || k == '-' { k } else { ' ' }));
            }
        }
        let pre_image = Some(crate::engine::PreImage {
            index: Some("100644 0000000000000000000000000000000000000000 0".into()),
            worktree: crate::engine::WorktreeKind::File(0),
        });
        let loaded = LoadedDiff::build(key.clone(), Comparison::Worktree, None, GetGitDiffResponse { ... }, patch.into_bytes(), pre_image);
```

(the `GetGitDiffResponse` literal is the existing one). Run `cargo test --locked --lib tui` to confirm nothing else changed.

- [ ] **Step 2: Keys**

In `src/tui/keys.rs` add `StageHunk, DiscardHunk, DiscardFile` to `KeyAction`, the three bindings after the `r` binding:

```rust
    Binding { key: "s", label: "stage / unstage hunk", action: KeyAction::StageHunk, vimeflow: Some("diff-hunk-stage") },
    Binding { key: "d", label: "discard hunk", action: KeyAction::DiscardHunk, vimeflow: Some("diff-hunk-discard") },
    Binding { key: "D", label: "discard file", action: KeyAction::DiscardFile, vimeflow: Some("diff-file-discard") },
```

and the new `RESERVED`. Update the two asserts in `navigation_aliases_require_no_modifiers_and_do_not_extend_the_sheet` to 27, and add to `modifiers_preserve_uppercase...`: `assert_eq!(lookup(&KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT)), Some(KeyAction::DiscardFile));` and `assert_eq!(lookup(&KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE)), Some(KeyAction::DiscardHunk));` (replacing the line that asserted `d` is `None`).

Run: `cargo test --locked --lib tui::keys`. Expected: PASS. `tui::input::tests::every_key_on_the_sheet_does_something...` now fails for `s`, `d`, `D` ("is on the key sheet but does nothing"): the failure to see before Step 5.

- [ ] **Step 3: `truncate_left` and the box**

In `src/tui/format.rs`:

```rust
/// `…` plus the tail of `text` that fits `max` cells; the whole text when it already fits.
pub fn truncate_left(text: &str, max: usize) -> String {
    if width(text) <= max {
        return text.to_string();
    }
    let mut tail = String::new();
    let mut used = 1;
    for ch in text.chars().rev() {
        let w = width(&ch.to_string());
        if used + w > max {
            break;
        }
        used += w;
        tail.insert(0, ch);
    }
    format!("…{tail}")
}
```

with a test: `assert_eq!(truncate_left("src/values.ts", 8), "…lues.ts"); assert_eq!(truncate_left("ab", 8), "ab");`.

Create `src/tui/confirm.rs`:

```rust
//! The y/n box of spec 9.3: one reusable dialog panel, its keys handled in one place (input.rs).
use std::sync::Arc;

use crate::engine::{Action, ActionKind, LoadedDiff};
use crate::tui::dialog::{self, Panel, Row};
use crate::tui::format::{truncate_left, width};
use crate::tui::sanitize::sanitize;

pub const FOOTER: &str = "y yes · n no";
const CANNOT_UNDO: &str = "This cannot be undone.";
const NOT_IN_GIT: &str = "It is not in git and cannot be recovered.";

pub struct Confirm {
    pub action: Action,
    pub title: &'static str,
    pub warning: Option<&'static str>,
    pub drawn: bool,
    pub done: String,
    pub verb: &'static str,
    before: String,
    after: &'static str,
}

impl Confirm {
    pub fn open(kind: ActionKind, diff: Arc<LoadedDiff>, hunk: Option<usize>, hunk_total: usize) -> Self {
        let path = sanitize(&diff.key.path);
        let position = hunk.map(|h| format!("{}/{}", h + 1, hunk_total)).unwrap_or_default();
        let (title, before, after, warning, done, verb) = match (diff.key.untracked, kind, diff.key.staged) {
            (true, ActionKind::Stage, _) => ("Stage file?", "Add ".to_string(), " to the index?", None, format!("staged {path}"), "stage"),
            (true, _, _) => ("Delete untracked file?", "Delete ".to_string(), "?", Some(NOT_IN_GIT), format!("deleted {path}"), "delete"),
            (false, ActionKind::Stage, false) => ("Stage hunk?", format!("Stage hunk {position} of "), "?", None, format!("staged hunk {position} of {path}"), "stage"),
            (false, ActionKind::Stage, true) => ("Unstage hunk?", format!("Move hunk {position} of "), " out of the index?", None, format!("unstaged hunk {position} of {path}"), "unstage"),
            (false, ActionKind::Discard, _) => ("Discard hunk?", format!("Discard hunk {position} of "), "?", Some(CANNOT_UNDO), format!("discarded hunk {position} of {path}"), "discard"),
            (false, ActionKind::DiscardFile, _) => ("Discard file?", "Discard every change this row shows for ".to_string(), "?", Some(CANNOT_UNDO), format!("discarded {path}"), "discard"),
        };
        Self {
            action: Action { kind, diff, hunk },
            title,
            warning,
            drawn: false,
            done,
            verb,
            before,
            after,
        }
    }

    fn rows_with(&self, body: String) -> Vec<Row> {
        let mut rows = vec![Row::Note(body)];
        if let Some(warning) = self.warning {
            rows.push(Row::Warn(warning.into()));
        }
        rows
    }

    /// The body at `columns`: the path shortened from the left until the dialog's own wrapping
    /// of the body and the warning fits four lines, so the frame makes eight.
    pub fn body(&self, columns: u16) -> String {
        let full = sanitize(&self.action.diff.key.path);
        let mut keep = width(&full);
        loop {
            let body = format!("{}{}{}", self.before, truncate_left(&full, keep), self.after);
            let probe = Panel { title: String::new(), rows: self.rows_with(body.clone()), footer: String::new(), cursor: None, offset: 0 };
            if dialog::line_count(&probe, columns) <= 4 || keep <= 2 {
                return body;
            }
            keep -= 1;
        }
    }

    pub fn panel(&self, columns: u16) -> Panel {
        Panel {
            title: self.title.into(),
            rows: self.rows_with(self.body(columns)),
            footer: FOOTER.into(),
            cursor: None,
            offset: 0,
        }
    }

    /// Whether the whole box fits a frame of `columns` by `height`: the drawn rule of spec 9.3.
    pub fn fits(&self, columns: u16, height: u16) -> bool {
        let panel_width = columns.min(60);
        columns >= 40 && height >= 10 && dialog::line_count(&self.panel(panel_width), panel_width) + 4 <= usize::from(height) - 2
    }
}
```

Add `pub mod confirm;` to `src/tui/mod.rs`. Tests in the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::dialog;
    use crate::tui::state::tests::snapshot;
    use crate::engine::DiffState;

    fn diff(path: &str) -> Arc<LoadedDiff> {
        match snapshot(path, "r", &[(1, "-+"), (9, "+")]).diff {
            DiffState::Ready(d) => d,
            _ => unreachable!(),
        }
    }

    #[test]
    fn each_kind_has_its_title_body_and_warning() {
        let c = Confirm::open(ActionKind::Stage, diff("src/values.ts"), Some(1), 2);
        assert_eq!((c.title, c.body(60).as_str(), c.warning), ("Stage hunk?", "Stage hunk 2/2 of src/values.ts?", None));
        assert_eq!((c.done.as_str(), c.verb), ("staged hunk 2/2 of src/values.ts", "stage"));
        let c = Confirm::open(ActionKind::Discard, diff("src/values.ts"), Some(0), 2);
        assert_eq!((c.title, c.body(60).as_str(), c.warning), ("Discard hunk?", "Discard hunk 1/2 of src/values.ts?", Some(CANNOT_UNDO)));
        let c = Confirm::open(ActionKind::DiscardFile, diff("src/values.ts"), None, 2);
        assert_eq!(c.body(60), "Discard every change this row shows for src/values.ts?");
        let mut untracked = (*diff("u.txt")).clone();
        untracked.key.untracked = true;
        let c = Confirm::open(ActionKind::Discard, Arc::new(untracked.clone()), None, 1);
        assert_eq!((c.title, c.body(60).as_str(), c.warning, c.verb), ("Delete untracked file?", "Delete u.txt?", Some(NOT_IN_GIT), "delete"));
        let c = Confirm::open(ActionKind::Stage, Arc::new(untracked), None, 1);
        assert_eq!((c.title, c.body(60).as_str(), c.done.as_str()), ("Stage file?", "Add u.txt to the index?", "staged u.txt"));
    }

    #[test]
    fn the_box_is_at_most_eight_lines_at_the_minimum_width() {
        let long = "a/very/long/directory/name/that/goes/on/and/on/for/a/while/values.ts";
        let mut untracked = (*diff(long)).clone();
        untracked.key.untracked = true;
        let c = Confirm::open(ActionKind::DiscardFile, Arc::new(untracked), None, 1);
        let panel = c.panel(40);
        assert!(dialog::line_count(&panel, 40) <= 4, "{:?}", c.body(40));
        assert!(c.fits(40, 10) && !c.fits(40, 9) && !c.fits(39, 10));
        assert!(c.body(40).contains("…"));
        assert!(c.body(40).ends_with("values.ts?"));
        let lines = dialog::render(&panel, 40, 8);
        assert_eq!(lines.len(), 8);
        let text: Vec<String> = lines.iter().map(|l| l.iter().map(|s| s.text.as_str()).collect()).collect();
        assert!(text.iter().any(|l| l.contains("cannot be recovered")), "{text:?}");
        // The last line is the bottom border; the footer sits above it.
        assert!(text[text.len() - 2].contains(FOOTER), "{text:?}");
    }
}
```

Run: `cargo test --locked --lib tui::confirm` and then `cargo test --locked --lib tui::format` (cargo takes one filter). Expected: PASS. If the eight-line assertion fails because the frame draws fewer lines than asked, read `dialog::render`'s row loop: it pads to `height`; the test holds.

- [ ] **Step 4: State: the guard, the answers, the notices**

In `src/tui/state.rs` add the fields and `PendingAction`, initialise them in `new` (`confirm: None, pending_action: None, seen_action_seq: 0`), and:

```rust
    pub fn action_running(&self, snapshot: &Snapshot) -> bool {
        match &self.pending_action {
            None => false,
            Some(pending) if !pending.answered => true,
            Some(pending) => matches!(&snapshot.diff, DiffState::Ready(d) if std::sync::Arc::ptr_eq(d, &pending.diff)),
        }
    }
```

In `observe`, after the mark block and before the rewrite block:

```rust
        let mut answered_action = false;
        if snapshot.action_seq != self.seen_action_seq {
            self.seen_action_seq = snapshot.action_seq;
            answered_action = true;
            if let Some(pending) = self.pending_action.take() {
                // The answer to the key the user just pressed is shown at once (8.5's rule for M).
                if let Some(displaced) = self.notice.as_ref().filter(|n| n.urgent) {
                    match self.notice_kind {
                        NoticeKind::Rewrite => self.seen_rewrite = None,
                        NoticeKind::BaseError => self.pending_base_error = Some(displaced.text.clone()),
                        NoticeKind::Other => {}
                    }
                }
                match &snapshot.action_error {
                    Some(error) => {
                        let mut text = format!("{} failed: {}", pending.verb, crate::tui::sanitize::sanitize(error));
                        if error.contains("does not match index") {
                            text.push_str("; unstage it first (s)");
                        }
                        self.warn(text);
                    }
                    None => self.notify(pending.done.clone()),
                }
                if snapshot.action_applied {
                    self.pending_action = Some(PendingAction { answered: true, ..pending });
                }
            }
        }
        // An applied action holds the keys until its Arc has left the screen.
        if self.pending_action.as_ref().is_some_and(|p| p.answered) && !self.action_running(snapshot) {
            self.pending_action = None;
        }
```

and fold `answered_action` into the two conditions that already use `answered_mark` (`!answered_mark && !answered_action`), so a rewrite warning or base error waits behind an action answer exactly as behind a mark answer. The guard check above is written so that an unanswered pending action keeps `action_running` true and an answered one holds only while `snapshot.diff` is still the same `Arc`.

Add to `body_is_drawn` in `view.rs` (Step 6) the `state.confirm.is_none()` clause; `record_drawn` is unchanged.

- [ ] **Step 5: Input**

In `src/tui/input.rs`:

1. In `handle_key`, after the `help_open` block and before `if state.picker.is_some()`:

```rust
    if state.confirm.is_some() {
        return confirm_key(state, snapshot, key);
    }
```

```rust
/// The box of spec 9.3: `y` confirms once the box was drawn, `n` cancels, everything else is inert.
fn confirm_key(state: &mut ViewState, snapshot: &Snapshot, key: KeyEvent) -> Outcome {
    match (key.code, key.modifiers) {
        (KeyCode::Char('n'), KeyModifiers::NONE) => {
            state.confirm = None;
            Outcome::Redraw
        }
        (KeyCode::Char('y'), KeyModifiers::NONE) => {
            let Some(confirm) = state.confirm.take_if(|c| c.drawn) else {
                return Outcome::Inert;
            };
            state.pending_action = Some(crate::tui::state::PendingAction {
                diff: confirm.action.diff.clone(),
                done: confirm.done,
                verb: confirm.verb,
                answered: false,
            });
            let _ = snapshot;
            Outcome::Engine(Command::Act(confirm.action))
        }
        _ => Outcome::Inert,
    }
}
```

   (`Option::take_if` is stable since Rust 1.80; delete the `let _ = snapshot;` line and the parameter if unused.)

2. In `handle_mouse`, right after the `mouse_requested` check: `if state.confirm.is_some() { return Outcome::Inert; }`.

3. In `act`, new arms before `_ => return move_cursor(...)`:

```rust
        StageHunk | DiscardHunk | DiscardFile => return open_box(state, snapshot, action),
```

```rust
/// Spec 9.2's refusals in the TUI's order, then the box; the engine re-decides all of them (9.4).
fn open_box(state: &mut ViewState, snapshot: &Snapshot, action: KeyAction) -> Outcome {
    use crate::engine::actions::{self, NOTICE_NO_HUNK, NOTICE_RUNNING, NOTICE_SCOPE};
    use crate::engine::{ActionKind, Scope};
    if snapshot.scope == Scope::Branch {
        state.notify(NOTICE_SCOPE);
        return Outcome::Redraw;
    }
    if state.action_running(snapshot) {
        state.notify(NOTICE_RUNNING);
        return Outcome::Redraw;
    }
    let DiffState::Ready(diff) = &snapshot.diff else {
        state.notify(NOTICE_NO_HUNK);
        return Outcome::Redraw;
    };
    let kind = match action {
        KeyAction::StageHunk => ActionKind::Stage,
        KeyAction::DiscardHunk => ActionKind::Discard,
        _ => ActionKind::DiscardFile,
    };
    let hunk = if kind == ActionKind::DiscardFile || diff.key.untracked {
        None
    } else {
        state.cursor.and_then(|c| diff.targets.get(c)).map(|t| t.hunk_index)
    };
    let candidate = crate::engine::Action { kind, diff: diff.clone(), hunk };
    if let Err(notice) = actions::plan(&candidate) {
        state.notify(notice);
        return Outcome::Redraw;
    }
    let total = diff.file_diff.hunks.len();
    state.confirm = Some(crate::tui::confirm::Confirm::open(kind, diff.clone(), hunk, total));
    Outcome::Redraw
}
```

4. Tests, in the module:

```rust
    use crate::engine::{ActionKind, Scope};
    use crate::engine::actions::{NOTICE_CHANGED, NOTICE_CUT, NOTICE_NO_HUNK, NOTICE_RUNNING, NOTICE_SCOPE};

    fn press_y(st: &mut ViewState, snap: &crate::engine::Snapshot) -> Outcome {
        if let Some(c) = st.confirm.as_mut() {
            c.drawn = true;
        }
        handle_key(st, snap, key("y"), 120)
    }

    #[test]
    fn s_d_and_capital_d_open_their_boxes_and_y_sends_the_act() {
        for (k, kind, hunk, title) in [
            ("s", ActionKind::Stage, Some(0), "Stage hunk?"),
            ("d", ActionKind::Discard, Some(0), "Discard hunk?"),
            ("D", ActionKind::DiscardFile, None, "Discard file?"),
        ] {
            let (snap, mut st) = setup(&[(10, " --+ "), (40, "+")]);
            assert_eq!(handle_key(&mut st, &snap, key(k), 120), Outcome::Redraw);
            let confirm = st.confirm.as_ref().expect("a box");
            assert_eq!(confirm.title, title);
            assert_eq!((confirm.action.kind, confirm.action.hunk), (kind, hunk));
            let diff = match &snap.diff { DiffState::Ready(d) => d.clone(), _ => unreachable!() };
            let outcome = press_y(&mut st, &snap);
            assert_eq!(outcome, Outcome::Engine(Command::Act(crate::engine::Action { kind, diff, hunk })));
            assert!(st.confirm.is_none());
            assert!(st.action_running(&snap));
        }
        // On the staged row `s` is an unstage and the hunk follows the cursor.
        let (mut snap, mut st) = setup(&[(10, " --+ "), (40, "+")]);
        if let DiffState::Ready(d) = &mut snap.diff {
            std::sync::Arc::make_mut(d).key.staged = true;
        }
        st.reconcile(&snap);
        handle_key(&mut st, &snap, key("]"), 120);
        handle_key(&mut st, &snap, key("s"), 120);
        let confirm = st.confirm.as_ref().unwrap();
        assert_eq!((confirm.title, confirm.action.hunk), ("Unstage hunk?", Some(1)));
    }

    #[test]
    fn n_closes_the_box_and_every_other_key_and_mouse_is_inert_in_it() {
        let (snap, mut st) = setup(&[(1, "+")]);
        handle_key(&mut st, &snap, key("d"), 120);
        for k in ["j", "k", "q", "s", "d", "D", "M", "b", "?", "Y", "N"] {
            assert_eq!(handle_key(&mut st, &snap, key(k), 120), Outcome::Inert, "{k}");
            assert!(st.confirm.is_some(), "{k} closed the box");
        }
        assert_eq!(handle_key(&mut st, &snap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), 120), Outcome::Inert);
        let rendered = view::render(&snap, &st, 120, 24);
        let click = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 2, row: 0, modifiers: KeyModifiers::NONE };
        assert_eq!(handle_mouse(&mut st, &snap, &rendered, click), Outcome::Inert);
        assert_eq!(handle_key(&mut st, &snap, key("ctrl+c"), 120), Outcome::Quit);
        assert_eq!(handle_key(&mut st, &snap, key("n"), 120), Outcome::Redraw);
        assert!(st.confirm.is_none() && st.pending_action.is_none());
    }

    #[test]
    fn y_is_inert_until_the_box_was_drawn() {
        let (snap, mut st) = setup(&[(1, "+")]);
        handle_key(&mut st, &snap, key("s"), 120);
        assert_eq!(handle_key(&mut st, &snap, key("y"), 120), Outcome::Inert);
        assert!(st.confirm.is_some());
        assert!(matches!(press_y(&mut st, &snap), Outcome::Engine(Command::Act(_))));
    }

    #[test]
    fn branch_scope_and_the_refusals_show_a_notice_and_no_box() {
        let (mut snap, mut st) = setup(&[(1, "+")]);
        snap.scope = Scope::Branch;
        for k in ["s", "d", "D"] {
            assert_eq!(handle_key(&mut st, &snap, key(k), 120), Outcome::Redraw);
            assert_eq!(st.notice.as_ref().map(|n| n.text.as_str()), Some(NOTICE_SCOPE));
            assert!(st.confirm.is_none());
        }
        snap.scope = Scope::Worktree;
        snap.diff = DiffState::Loading;
        st.reconcile(&snap);
        handle_key(&mut st, &snap, key("s"), 120);
        assert_eq!(st.notice.as_ref().map(|n| n.text.as_str()), Some(NOTICE_NO_HUNK));
        assert!(st.confirm.is_none());
    }

    #[test]
    fn a_second_key_while_an_action_is_running_opens_nothing() {
        let (mut snap, mut st) = setup(&[(1, "+")]);
        handle_key(&mut st, &snap, key("s"), 120);
        assert!(matches!(press_y(&mut st, &snap), Outcome::Engine(_)));
        handle_key(&mut st, &snap, key("s"), 120);
        assert_eq!(st.notice.as_ref().map(|n| n.text.as_str()), Some(NOTICE_RUNNING));
        assert!(st.confirm.is_none());
        // Answered and applied, with the same Arc on screen: still running.
        snap.action_seq = 1;
        snap.action_applied = true;
        st.observe(&snap);
        assert_eq!(st.notice.as_ref().map(|n| n.text.as_str()), Some("staged hunk 1/1 of a.rs"));
        assert!(st.action_running(&snap));
        handle_key(&mut st, &snap, key("d"), 120);
        assert_eq!(st.notice.as_ref().map(|n| n.text.as_str()), Some(NOTICE_RUNNING));
        // A fresh Arc frees the keys.
        let fresh = snapshot("a.rs", "r2", &[(1, "+")]);
        snap.diff = fresh.diff.clone();
        st.observe(&snap);
        assert!(!st.action_running(&snap));
        st.reconcile(&snap);
        handle_key(&mut st, &snap, key("d"), 120);
        assert!(st.confirm.is_some());
    }

    #[test]
    fn a_refused_answer_frees_the_keys_at_once_and_carries_the_unstage_hint() {
        let (mut snap, mut st) = setup(&[(1, "+")]);
        handle_key(&mut st, &snap, key("d"), 120);
        press_y(&mut st, &snap);
        snap.action_seq = 1;
        snap.action_error = Some("f.txt: does not match index".into());
        snap.action_applied = true;
        st.observe(&snap);
        let notice = st.notice.as_ref().unwrap();
        assert!(notice.urgent);
        assert_eq!(notice.text, "discard failed: f.txt: does not match index; unstage it first (s)");
        assert!(st.action_running(&snap), "applied on the same Arc still holds");
        snap.action_seq = 2;
        snap.action_error = Some(NOTICE_CHANGED.into());
        snap.action_applied = false;
        st.pending_action = Some(crate::tui::state::PendingAction { diff: match &snap.diff { DiffState::Ready(d) => d.clone(), _ => unreachable!() }, done: "x".into(), verb: "stage", answered: false });
        st.observe(&snap);
        assert!(!st.action_running(&snap), "a pre-git refusal frees the keys");
        assert_eq!(st.notice.as_ref().map(|n| n.text.as_str()), Some("stage failed: the diff changed; look again"));
    }

    #[test]
    fn a_cut_diff_refuses_every_key() {
        let (mut snap, mut st) = setup(&[(1, "+++++")]);
        if let DiffState::Ready(d) = &mut snap.diff {
            let d = std::sync::Arc::make_mut(d);
            d.truncated_lines = 2;
        }
        st.reconcile(&snap);
        for k in ["s", "d", "D"] {
            handle_key(&mut st, &snap, key(k), 120);
            assert_eq!(st.notice.as_ref().map(|n| n.text.as_str()), Some(NOTICE_CUT), "{k}");
            assert!(st.confirm.is_none());
        }
    }

    #[test]
    fn an_action_answer_over_an_unread_warning_puts_it_back() {
        use crate::engine::{Mark, MarkState};
        let (mut snap, mut st) = setup(&[(1, "+")]);
        handle_key(&mut st, &snap, key("s"), 120);
        press_y(&mut st, &snap);
        // One snapshot carries both the answer and a rewritten classification: the answer speaks
        // first (8.5's rule), and the warning waits for the body key that acknowledges it.
        snap.head_seen = Some("h1".repeat(20));
        snap.mark = Some(Mark { commit: "m".repeat(40), at: 1, state: MarkState::Rewritten, classified_at: snap.head_seen.clone() });
        snap.action_seq = 1;
        snap.action_applied = true;
        st.observe(&snap);
        assert_eq!(st.notice.as_ref().map(|n| n.text.as_str()), Some("staged hunk 1/1 of a.rs"));
        handle_key(&mut st, &snap, key("t"), 120);
        let notice = st.notice.as_ref().expect("the warning follows the answer");
        assert!(notice.urgent && notice.text.contains("no longer on this branch"));
    }
```

Run: `cargo test --locked --lib tui::input`. Expected: PASS, including `every_key_on_the_sheet_does_something_and_reserved_keys_do_nothing` again (the three keys open a box from the first position; `X` is now checked inert).

- [ ] **Step 6: View and shell**

In `src/tui/view.rs`:

1. `Action` gains `StageHunk, DiscardHunk, DiscardFile`; `key_action` maps them to the `KeyAction`s of the same name.
2. `Hit::hovered` and `body_is_drawn` gain `&& state.confirm.is_none()`.
3. In `toolbar_items`, after the two steppers are pushed and before the scope chip (so the group sits between the steppers in reading order, insert it at index 1 of `items`):

```rust
    if snapshot.scope == Scope::Worktree {
        let ready = matches!(snapshot.diff, DiffState::Ready(_));
        let stage = if staged { "unstage" } else { "stage" };
        let chip = |text: &str, action: Action| (text.to_string(), ready.then_some(action));
        items.insert(
            1,
            (
                vec![
                    chip(stage, Action::StageHunk),
                    (" ".into(), None),
                    chip("discard", Action::DiscardHunk),
                    (" ".into(), None),
                    chip("discard file", Action::DiscardFile),
                ],
                8,
            ),
        );
    }
```

   `staged` is already computed above from the diff or the selected key. In `toolbar`, a piece with `None` stays plain; that is the dim state (no reverse video, no hit), which is what 4.2 asks of a disabled chip. Note the drop loop removes the item with the highest order first, so this group goes before every Phase 1 item.

4. Footer hints: build the vector as

```rust
    let mut hints = vec!["j/k line", "[ ] hunk", "n/p file", "t view", "e files", "r refresh"];
    if snapshot.scope == Scope::Worktree {
        hints.extend(["s stage", "d discard", "D file"]);
    }
    hints.extend(["? help", "q quit"]);
```

5. The overlay, after the picker block:

```rust
    if let Some(confirm) = &state.confirm {
        let panel_width = columns.min(60);
        let panel = confirm.panel(panel_width);
        let panel_height = (dialog::line_count(&panel, panel_width) + 4).min(usize::from(height) - 2) as u16;
        let x = usize::from((columns - panel_width) / 2);
        for (y, overlay) in dialog::render(&panel, panel_width, panel_height).into_iter().enumerate() {
            let background = &lines[y + 1];
            let mut line = clip_line(background, 0, x);
            line.extend(overlay);
            let right = x + usize::from(panel_width);
            line.extend(clip_line(background, right, usize::from(columns) - right));
            lines[y + 1] = line;
        }
        hits.clear();
    }
```

In `src/tui/shell.rs`, inside the draw closure after `drew_body = ...`, add `box_fits = state.confirm.as_ref().is_some_and(|c| c.fits(width, area.height));` (declare `let mut box_fits = false;` beside `drew_body`), and after `state.record_drawn(...)`: `if let Some(confirm) = state.confirm.as_mut() { confirm.drawn = box_fits; }`. A box that the frame clips, which the overlay's `.min(height - 2)` makes possible on a resize, is drawn but not confirmable.

Tests in `view.rs`:

```rust
    #[test]
    fn the_toolbar_group_sits_between_the_steppers_and_drops_first() {
        let (r, _) = rendered(120, 20, FilesPanel::Hidden);
        let bar = &r.plain()[0];
        let stepper = bar.find("‹  a.rs").unwrap();
        let group = bar.find(" stage ").unwrap();
        let hunks = bar.find("{} 1/2").unwrap();
        assert!(stepper < group && group < hunks, "{bar}");
        assert!(bar.contains(" discard ") && bar.contains(" discard file "), "{bar}");
        for action in [Action::StageHunk, Action::DiscardHunk, Action::DiscardFile] {
            assert!(r.hits.iter().any(|h| h.y == 0 && h.action == action), "{action:?}");
        }
        let (r, _) = rendered(80, 20, FilesPanel::Hidden);
        let bar = &r.plain()[0];
        assert!(!bar.contains("discard") && bar.contains("{} 1/2"), "{bar}");
    }

    #[test]
    fn the_group_reads_unstage_on_a_staged_row_and_is_absent_in_branch_scope_and_dim_while_loading() {
        let mut snap = files(snapshot("a.rs", "r1", &[(1, "+")]));
        if let DiffState::Ready(d) = &mut snap.diff {
            std::sync::Arc::make_mut(d).key.staged = true;
        }
        snap.selected.as_mut().unwrap().staged = true;
        let st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        let bar = render(&snap, &st, 120, 20).plain()[0].clone();
        assert!(bar.contains(" unstage ") && !bar.contains(" stage "), "{bar}");
        snap.scope = Scope::Branch;
        snap.base = Some(crate::engine::Base { requested: "refs/heads/main".into(), commit: "c".repeat(40), merge_base: Some("m".repeat(40)), source: crate::engine::BaseSource::Default });
        let r = render(&snap, &st, 120, 20);
        assert!(!r.plain()[0].contains("discard"));
        assert!(!r.plain().last().unwrap().contains("s stage"));
        snap.scope = Scope::Worktree;
        snap.diff = DiffState::Loading;
        let r = render(&snap, &st, 120, 20);
        assert!(r.plain()[0].contains("discard"));
        assert!(!r.hits.iter().any(|h| matches!(h.action, Action::StageHunk | Action::DiscardHunk | Action::DiscardFile)));
    }

    #[test]
    fn the_box_covers_the_body_and_fits_the_minimum_terminal() {
        let snap = files(snapshot("a/very/long/directory/name/that/goes/on/and/on/values.ts", "r1", &[(1, "+")]));
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        st.resize(40, body_height(&st, &snap, 10));
        st.reconcile(&snap);
        crate::tui::input::handle_key(&mut st, &snap, crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char('D'), crossterm::event::KeyModifiers::SHIFT), 40);
        assert!(st.confirm.is_some());
        let r = render(&snap, &st, 40, 10);
        let text = r.plain();
        assert!(text.iter().any(|l| l.contains("Discard file?")), "{text:?}");
        assert!(text.iter().any(|l| l.contains("cannot be undone")), "{text:?}");
        assert!(text.iter().any(|l| l.contains("y yes · n no")), "{text:?}");
        assert!(r.hits.is_empty());
        assert!(!body_is_drawn(&st, &snap, 40, 10));
        assert_eq!(render(&snap, &st, 39, 10).plain(), vec!["terminal too small"]);
    }
```

Update `footer_drops_whole_hints_and_preserves_help_and_quit` to expect the three new hints at 120 columns (the 40-column line is unchanged). If `the_toolbar_shows_both_steppers_and_every_item_is_clickable` or `a_narrow_toolbar_drops_items...` asserts a bar text that the group now interrupts, widen its fixture or assert the pieces separately; the pieces themselves are unchanged. `rendered()` and `files()` are the existing test helpers in that module.

Run: `cargo test --locked --lib tui`. Expected: PASS.

- [ ] **Step 7: Gates and commit**

Run the full gate line. Expected: all green.

```bash
git add src/tui src/engine/actions.rs
git commit -m "feat(tui): stage, discard and discard-file keys behind a y/n box"
```

---

### Task 6: The recording test, Tier B, documentation and version 0.0.4

Implements spec 9.6's second test paragraph, 9.8 items 9-11 and criteria 12-15, 9.5's version and README paragraph, and 9.7's documentation of the overridden settings. Read 9.6, 9.7 "Configuration" and 9.8 before starting.

**Files:**
- Create: `tests/hunk_actions.rs`
- Modify: `tests/e2e_real_herdr.rs`, `README.md`, `README.zh-CN.md`, `README.ja.md`, `AGENTS.md`, `docs/acceptance-p1.md`, `Cargo.toml`, `Cargo.lock`, `herdr-plugin.toml`

**Interfaces:**
- Consumes: everything above.
- Produces: the release.

- [ ] **Step 1: The recording test**

Create `tests/hunk_actions.rs`. Copy `real_git`, `git` and `tree_hash` verbatim from `tests/readonly_guarantee.rs` (they are private to that file; a shared `tests/support` module would also do, but the copy keeps the two guarantees independent). Then:

```rust
use herdr_hunks::engine::actions::{NOTICE_BINARY, NOTICE_CHANGED, NOTICE_SCOPE};
use herdr_hunks::engine::{
    init_process_env, spawn, Action, ActionKind, Command, DiffState, FileKey, Scope, SessionConfig, Snapshot,
};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as Proc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The reads of spec 7.6 plus the one mutating subcommand of 9.6.
const ALLOWED: [&str; 11] = [
    "--version", "rev-parse", "status", "diff", "ls-files", "show", "cat-file", "symbolic-ref", "merge-base", "for-each-ref", "apply",
];

struct Session {
    handle: herdr_hunks::engine::EngineHandle,
    log: PathBuf,
}

impl Session {
    fn recv(&self, what: &str, pred: impl Fn(&Snapshot) -> bool) -> Arc<Snapshot> {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut last = None;
        while Instant::now() < deadline {
            if let Ok(s) = self.handle.snapshots.recv_timeout(Duration::from_millis(200)) {
                if pred(&s) {
                    return s;
                }
                last = Some(s);
            }
        }
        panic!("timed out waiting for {what}; last = {last:?}");
    }

    /// A `Refresh` first, so the listing is republished even when its last snapshot was consumed;
    /// then a `Select` for a listed key, answered by a settled `Ready` diff for it.
    fn select(&self, path: &str, staged: bool, untracked: bool) -> Arc<Snapshot> {
        let key = FileKey { path: path.into(), staged, untracked };
        self.handle.commands.send(Command::Refresh).unwrap();
        self.recv(&format!("{key:?} listed"), |s| s.files.iter().any(|f| FileKey::of(f) == key));
        self.handle.commands.send(Command::Select(key.clone())).unwrap();
        self.recv(&format!("{key:?} ready"), |s| !s.refreshing && matches!(&s.diff, DiffState::Ready(d) if d.key == key))
    }

    /// Sends the action against the published diff; returns the answer and the `apply` lines it caused.
    fn act(&self, s: &Snapshot, kind: ActionKind, hunk: Option<usize>) -> (Arc<Snapshot>, Vec<String>) {
        let diff = match &s.diff { DiffState::Ready(d) => d.clone(), _ => panic!("no ready diff") };
        let before = std::fs::read_to_string(&self.log).unwrap_or_default().lines().count();
        let seq = s.action_seq + 1;
        self.handle.commands.send(Command::Act(Action { kind, diff, hunk })).unwrap();
        let answer = self.recv("the answer", |n| n.action_seq == seq);
        let applies: Vec<String> = std::fs::read_to_string(&self.log).unwrap()
            .lines()
            .skip(before)
            .map(|l| l.split('\t').next().unwrap_or("").to_string())
            .filter(|l| subcommand(l) == "apply")
            .collect();
        (answer, applies)
    }
}

fn subcommand(argv: &str) -> &str {
    let args: Vec<&str> = argv.split_whitespace().collect();
    let mut i = 0;
    while matches!(args.get(i), Some(&"-C") | Some(&"-c")) {
        i += 2;
    }
    args.get(i).copied().unwrap_or("")
}

/// The fixture of spec 9.8 item 4, plus the nested untracked file of the review focus.
fn fixture(real: &Path, p: &Path) {
    git(real, p, &["init", "-q", "-b", "main"]);
    git(real, p, &["config", "user.email", "t@example.com"]);
    git(real, p, &["config", "user.name", "t"]);
    let ctx = "c1\nc2\nc3\nc4\nc5\nc6\nc7\n";
    std::fs::write(p.join("mm.txt"), format!("alpha\n{ctx}beta\n{ctx}gamma\n{ctx}delta\n")).unwrap();
    std::fs::write(p.join("md.txt"), "m\n").unwrap();
    std::fs::write(p.join("bin.dat"), [0u8, 1, 2]).unwrap();
    git(real, p, &["add", "-A"]);
    git(real, p, &["commit", "-q", "-m", "init"]);
    std::fs::write(p.join("mm.txt"), format!("ALPHA\n{ctx}BETA\n{ctx}gamma\n{ctx}delta\n")).unwrap();
    git(real, p, &["add", "mm.txt"]);
    std::fs::write(p.join("mm.txt"), format!("ALPHA\n{ctx}BETA\n{ctx}GAMMA\n{ctx}DELTA\n")).unwrap();
    std::fs::write(p.join("md.txt"), "M\n").unwrap();
    git(real, p, &["add", "md.txt"]);
    std::fs::remove_file(p.join("md.txt")).unwrap();
    std::fs::write(p.join("bin.dat"), [0u8, 1, 3]).unwrap();
    std::fs::create_dir_all(p.join("newdir/deep")).unwrap();
    std::fs::write(p.join("newdir/deep/u.txt"), "u\n").unwrap();
    std::fs::write(p.join("u2.txt"), "two\n").unwrap();
}

#[test]
fn every_action_runs_exactly_its_apply_forms_and_changes_exactly_what_the_row_shows() {
    let home = PathBuf::from(std::env::var_os("HOME").expect("test HOME"));
    let real = real_git();
    let repo = tempfile::tempdir().unwrap();
    let p = repo.path();
    fixture(&real, p);
    // recording wrapper, first in PATH (the readonly test's, verbatim)
    let bin = tempfile::tempdir().unwrap();
    let log = bin.path().join("git.log");
    let wrapper = bin.path().join("git");
    std::fs::write(&wrapper, format!("#!/bin/sh\nprintf '%s\\t%s\\t%s\\t%s\\n' \"$*\" \"$GIT_OPTIONAL_LOCKS\" \"$GIT_LITERAL_PATHSPECS\" \"$GIT_NO_LAZY_FETCH\" >> '{}'\nexec '{}' \"$@\"\n", log.display(), real.display())).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::env::set_var("PATH", format!("{}:{}", bin.path().display(), std::env::var("PATH").unwrap()));
    init_process_env();
    let refs_before = git(&real, p, &["for-each-ref"]);
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let state = tempfile::tempdir().unwrap();
    let mut config = SessionConfig::production(p.to_path_buf());
    config.poll_interval = Duration::from_millis(100);
    config.state_dir = Some(state.path().to_path_buf());
    let session = Session { handle: spawn(rt.handle(), config), log: log.clone() };
    let top = p.canonicalize().unwrap();
    let top = top.to_string_lossy();
    let form = |flags: &str| format!("-c apply.ignoreWhitespace=no -C {top} apply --whitespace=nowarn{flags}");

    // After every action: the refs are untouched, and the side the form does not name is untouched.
    let refs_same = |what: &str| assert_eq!(git(&real, p, &["for-each-ref"]), refs_before, "refs changed after {what}");

    // 1. Stage one unstaged hunk: one --cached apply; the index gains GAMMA only, the worktree is untouched.
    let s = session.select("mm.txt", false, false);
    let index_before = std::fs::read(p.join(".git/index")).unwrap();
    let tree_before = tree_hash(p);
    let (a, applies) = session.act(&s, ActionKind::Stage, Some(0));
    assert_eq!((a.action_error.as_deref(), a.action_applied), (None, true));
    assert_eq!(applies, [form(" --cached")]);
    assert_ne!(std::fs::read(p.join(".git/index")).unwrap(), index_before, "the index changed");
    assert_eq!(tree_hash(p), tree_before, "a --cached form left the worktree alone");
    let cached = git(&real, p, &["diff", "--cached", "--", "mm.txt"]);
    assert!(cached.contains("+GAMMA") && !cached.contains("+DELTA"), "{cached}");
    assert_eq!(git(&real, p, &["status", "--porcelain=v1", "--", "mm.txt"]).trim(), "MM mm.txt");
    refs_same("stage");

    // 2. Unstage it again from the staged row (its third hunk): one --cached -R apply; the worktree is untouched.
    let s = session.select("mm.txt", true, false);
    assert!(matches!(&s.diff, DiffState::Ready(d) if d.file_diff.hunks.len() == 3));
    let tree_before = tree_hash(p);
    let (a, applies) = session.act(&s, ActionKind::Stage, Some(2));
    assert_eq!(a.action_error, None);
    assert_eq!(applies, [form(" --cached -R")]);
    assert_eq!(tree_hash(p), tree_before);
    assert!(!git(&real, p, &["diff", "--cached", "--", "mm.txt"]).contains("+GAMMA"));
    refs_same("unstage");

    // 3. Discard both unstaged hunks: two -R applies; the index is untouched, the worktree loses GAMMA then DELTA.
    let s = session.select("mm.txt", false, false);
    assert!(matches!(&s.diff, DiffState::Ready(d) if d.file_diff.hunks.len() == 2));
    let index_before = std::fs::read(p.join(".git/index")).unwrap();
    let (a, applies) = session.act(&s, ActionKind::Discard, Some(0));
    assert_eq!(a.action_error, None);
    assert_eq!(applies, [form(" -R")]);
    assert_eq!(std::fs::read(p.join(".git/index")).unwrap(), index_before, "a worktree form left the index alone");
    assert!(std::fs::read_to_string(p.join("mm.txt")).unwrap().contains("gamma\n"));
    let s = session.select("mm.txt", false, false);
    assert!(matches!(&s.diff, DiffState::Ready(d) if d.file_diff.hunks.len() == 1));
    let (a, applies) = session.act(&s, ActionKind::Discard, Some(0));
    assert_eq!(a.action_error, None);
    assert_eq!(applies, [form(" -R")]);
    assert_eq!(std::fs::read(p.join(".git/index")).unwrap(), index_before);
    assert_eq!(git(&real, p, &["status", "--porcelain=v1", "--", "mm.txt"]).trim(), "M  mm.txt");
    refs_same("discard");

    // 4. D on the staged row of the now-clean file: one --index -R apply; both sides return to HEAD.
    let s = session.select("mm.txt", true, false);
    let (a, applies) = session.act(&s, ActionKind::DiscardFile, None);
    assert_eq!(a.action_error, None);
    assert_eq!(applies, [form(" --index -R")]);
    assert_eq!(git(&real, p, &["status", "--porcelain=v1", "--", "mm.txt"]).trim(), "");
    refs_same("discard file");

    // 5. MD: refused by the engine before git; the file stays absent; no apply recorded.
    let s = session.select("md.txt", true, false);
    let (a, applies) = session.act(&s, ActionKind::DiscardFile, None);
    assert_eq!(a.action_error.as_deref(), Some("md.txt: does not match index"));
    assert!(applies.is_empty() && !a.action_applied && !p.join("md.txt").exists());

    // 6. A binary row: refused, no apply.
    let s = session.select("bin.dat", false, false);
    let (a, applies) = session.act(&s, ActionKind::DiscardFile, None);
    assert_eq!(a.action_error.as_deref(), Some(NOTICE_BINARY));
    assert!(applies.is_empty());

    // 7. Branch scope: refused, no apply, nothing changes.
    session.handle.commands.send(Command::SetScope(Scope::Branch)).unwrap();
    let s = session.recv("branch rows", |s| s.scope == Scope::Branch && matches!(s.diff, DiffState::Ready(_)));
    let (a, applies) = session.act(&s, ActionKind::DiscardFile, None);
    assert_eq!(a.action_error.as_deref(), Some(NOTICE_SCOPE));
    assert!(applies.is_empty());
    session.handle.commands.send(Command::SetScope(Scope::Worktree)).unwrap();

    // 8. A stale Arc: refused, no apply.
    session.recv("worktree back", |s| s.scope == Scope::Worktree && !s.refreshing);
    let s = session.select("md.txt", true, false);
    let stale = Action { kind: ActionKind::Stage, diff: Arc::new((**match &s.diff { DiffState::Ready(d) => d, _ => unreachable!() }).clone()), hunk: None };
    let before = std::fs::read_to_string(&log).unwrap().lines().count();
    session.handle.commands.send(Command::Act(stale)).unwrap();
    let a = session.recv("stale answer", |n| n.action_seq == s.action_seq + 1);
    assert_eq!(a.action_error.as_deref(), Some(NOTICE_CHANGED));
    assert!(!std::fs::read_to_string(&log).unwrap().lines().skip(before).any(|l| subcommand(l) == "apply"));

    // 9. Delete the nested untracked file: one -R apply; the file goes, the row goes, the index is untouched.
    let s = session.select("newdir/deep/u.txt", false, true);
    let index_before = std::fs::read(p.join(".git/index")).unwrap();
    let (a, applies) = session.act(&s, ActionKind::DiscardFile, None);
    assert_eq!(a.action_error, None);
    assert_eq!(applies, [form(" -R")]);
    assert!(!p.join("newdir/deep/u.txt").exists());
    assert_eq!(std::fs::read(p.join(".git/index")).unwrap(), index_before);
    let s = session.recv("row gone", |s| !s.files.iter().any(|f| f.path == "newdir/deep/u.txt") && !s.refreshing);
    assert!(s.files.iter().any(|f| Some(FileKey::of(f)) == s.selected));
    refs_same("delete");

    // 10. Stage an untracked file: one --cached apply; the worktree is untouched.
    let s = session.select("u2.txt", false, true);
    let tree_before = tree_hash(p);
    let (a, applies) = session.act(&s, ActionKind::Stage, None);
    assert_eq!(a.action_error, None);
    assert_eq!(applies, [form(" --cached")]);
    assert_eq!(tree_hash(p), tree_before);
    assert_eq!(git(&real, p, &["status", "--porcelain=v1", "--", "u2.txt"]).trim(), "A  u2.txt");
    refs_same("stage untracked");

    session.handle.commands.send(Command::Shutdown).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let recorded = std::fs::read_to_string(&log).unwrap();
    for entry in recorded.lines() {
        let fields: Vec<&str> = entry.split('\t').collect();
        let sub = subcommand(fields[0]);
        assert!(ALLOWED.contains(&sub), "unexpected git subcommand `{sub}` in `{}`", fields[0]);
        assert_eq!(&fields[1..], ["0", "1", "1"], "D3 variables missing in `{}`", fields[0]);
    }
    // Every apply is a confirmed action's form: exactly these seven ran, in this order.
    let applies: Vec<&str> = recorded.lines().map(|l| l.split('\t').next().unwrap()).filter(|l| subcommand(l) == "apply").collect();
    let expected = [" --cached", " --cached -R", " -R", " -R", " --index -R", " -R", " --cached"].map(|f| form(f));
    assert_eq!(applies, expected.iter().map(String::as_str).collect::<Vec<_>>());
    assert_eq!(git(&real, p, &["for-each-ref"]), refs_before, "refs changed");
    let mut written: Vec<String> = std::fs::read_dir(state.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    written.sort();
    assert!(written.iter().all(|f| ["bases.json", "marks.json", "split-panes.lock"].contains(&f.as_str())), "the viewer wrote something else: {written:?}");
    let _ = home;
}
```

Clean up the throwaway line `let _ = home;` once the test compiles (or drop `home` if the HOME check of the readonly test is not copied). Add `[[test]] name = "hunk_actions"` only if `Cargo.toml` lists tests explicitly (it does not; auto-discovery applies). `sha2` is already a dev-dependency for `tree_hash`.

Run: `HOME=... cargo test --locked --test hunk_actions -- --test-threads=1`
Expected: PASS. Falsify: in `actions::plan`, swap `Form::DiscardStaged` for `Form::DiscardUnstaged` in the whole-file arm; case 4 must fail on `applies` (`" -R"` instead of `" --index -R"`); restore.

- [ ] **Step 2: Tier B**

In `tests/e2e_real_herdr.rs`, after the marker-clears block and before the focus assertion, in worktree scope:

```rust
        // Phase 2: stage the one unstaged hunk of a.txt from the keyboard.
        std::fs::write(repo.join("a.txt"), "a\nSTAGE-ME\n").unwrap();
        iso.herdr(&["pane", "send-text", viewer_id, "b"]); // back to worktree scope
        iso.herdr(&["pane", "wait-output", viewer_id, "--match", "UNSTAGED", "--source", "visible", "--timeout", "15000"]);
        iso.herdr(&["pane", "send-text", viewer_id, "s"]);
        iso.herdr(&["pane", "wait-output", viewer_id, "--match", "Stage hunk?", "--source", "visible", "--timeout", "15000"]);
        iso.herdr(&["pane", "send-text", viewer_id, "y"]);
        wait_for("the row moves to the staged side", || {
            git_out(&["status", "--porcelain=v1", "--", "a.txt"]).trim() == "M  a.txt"
        });
        iso.herdr(&["pane", "wait-output", viewer_id, "--match", "staged hunk 1/1 of a.txt", "--source", "visible", "--timeout", "15000"]);
```

`git_out` is a one-line sibling of the file's `git` closure that returns stdout; add it beside it. The file's fixture must leave `a.txt` tracked and clean before this block (check the lines that build `repo`; add the file to the initial commit if it is not there, or use a file that is).

Run (needs a fresh release build and the two variables of `docs/acceptance-p1.md`): the Tier B command from that document. Expected: PASS in both hosts; record the output for the acceptance row.

- [ ] **Step 3: README, three languages**

In `README.md`:

1. First lines: `A git hunk viewer for [Herdr](https://herdr.dev), with changed files, unified/split diffs, a clickable navigation toolbar, live refresh, and hunk staging behind a y/n box.` and delete the sentence `Phase 1 does not stage, unstage, discard, add comments, or dispatch to agents.`, replacing it with `It does not add comments or dispatch to agents.`
2. "Keys" table, after the `r` row:

```markdown
| `s` | Stage the hunk under the cursor, or unstage it on a staged row; whole file on an untracked row (worktree scope) |
| `d` | Discard the hunk under the cursor; deletes an untracked file (worktree scope) |
| `D` | Discard every change the row shows (worktree scope) |
```

   and the closing line becomes `Phase 2 leaves \`i I u U x X v y Y @ c /\` unbound.`
3. A new section after "Review marks":

```markdown
## Hunk actions

In worktree scope, `s` stages the hunk under the cursor (or unstages it on a
staged row), `d` discards it and `D` discards every change the row shows. Each
opens a box: `y` confirms, `n` cancels, every other key is inert. The box names
the hunk and the file, and says so when the action cannot be undone: a
discarded hunk is gone, and `D` on an untracked row deletes a file git never
had.

Every action is one `git apply` on a patch cut from the diff on screen: what
you read is what git is given. A staged row's discard uses `--index`, so the
index and the working tree change together or not at all; while the file has
unstaged changes git refuses it, and the notice says to unstage first. The
viewer re-reads the file and the index entry before applying and refuses with
`the diff changed; look again` when they moved since the diff was read. Rows
cut by the size cap, binary files and submodule pointers are not acted on.

The viewer builds every diff it shows with three lines of context, `a/` and
`b/` prefixes and no textconv, so `diff.context`, `diff.noprefix`,
`diff.mnemonicPrefix` and textconv drivers do not apply to it;
`apply.ignoreWhitespace` is pinned off and `apply.whitespace` to `nowarn`. In
branch scope the three keys show `switch to worktree scope (b) to stage or
discard`.

The viewer runs eleven git subcommands: the ten reads of branch scope and
`apply`, which runs only from a confirmed key.
```

4. "Known limitations": replace the first two sentences (`This phase is read-only; ...` through `branch scope is unaffected.`) with `K1–K7 of PORT-SURFACE.md are closed in 0.0.4. Comments are not implemented.` Keep the rest.
5. "Roadmap": `P1: read-only viewing. P2: stage/unstage/discard actions and defect fixes (0.0.4).`
6. The `herdr-hunks 0.0.3` mention near line 41 becomes `0.0.4`.

`README.zh-CN.md`, after the `r` row of 按键 and a new section `## 差异块操作` after 审阅标记:

```markdown
| `s` | 暂存光标所在的差异块；在已暂存行上则取消暂存；未跟踪文件整体暂存（仅 worktree 范围） |
| `d` | 丢弃光标所在的差异块；对未跟踪文件则删除该文件（仅 worktree 范围） |
| `D` | 丢弃该行显示的全部改动（仅 worktree 范围） |
```

```markdown
## 差异块操作

在 worktree 范围内，`s` 暂存光标所在的差异块（在已暂存行上则取消暂存），`d`
丢弃它，`D` 丢弃该行显示的全部改动。每个按键都会弹出确认框：`y` 确认，`n`
取消，其他按键无效。确认框会写明差异块和文件，并在操作不可撤销时说明：丢弃的
差异块无法找回，在未跟踪行上按 `D` 会删除一个 git 从未记录的文件。

每个操作都是一次 `git apply`，补丁从屏幕上的 diff 中切出：你读到的就是交给
git 的。已暂存行的丢弃使用 `--index`，索引和工作区要么一起改变、要么都不变；
文件还有未暂存改动时 git 会拒绝，提示先取消暂存。应用前查看器会重新读取文件和
索引条目，若自读取 diff 以来有变化，则以 `the diff changed; look again` 拒绝。
被大小上限截断的行、二进制文件和子模块指针不会被操作。

查看器生成的每个 diff 都带三行上下文、`a/` 与 `b/` 前缀且不经过 textconv，
因此 `diff.context`、`diff.noprefix`、`diff.mnemonicPrefix` 和 textconv 驱动
对它不起作用；`apply.ignoreWhitespace` 固定关闭，`apply.whitespace` 固定为
`nowarn`。在 branch 范围内这三个按键显示 `switch to worktree scope (b) to
stage or discard`。

查看器运行十一个 git 子命令：分支范围的十个只读命令，以及仅在确认后运行的
`apply`。
```

The closing line of 按键 becomes `第二阶段保留 \`i I u U x X v y Y @ c /\`，不绑定任何操作。`; the first paragraph drops 只读 and says 支持在确认框后暂存差异块; 已知限制's first two sentences become `PORT-SURFACE.md 中的 K1–K7 已在 0.0.4 修复。评论尚未实现。`; 路线图's P2 gains `（0.0.4）`.

`README.ja.md`, after the `r` row of キー操作 and a new section `## ハンク操作` after レビューマーク:

```markdown
| `s` | カーソル位置のハンクをステージ。ステージ済みの行ではステージ解除。未追跡ファイルは丸ごとステージ（worktree スコープのみ） |
| `d` | カーソル位置のハンクを破棄。未追跡ファイルの場合はファイルを削除（worktree スコープのみ） |
| `D` | その行が示す変更をすべて破棄（worktree スコープのみ） |
```

```markdown
## ハンク操作

worktree スコープでは、`s` がカーソル位置のハンクをステージし（ステージ済みの
行ではステージ解除）、`d` がそれを破棄し、`D` がその行の示す変更をすべて破棄
します。いずれも確認ボックスを開きます。`y` で確定、`n` で取り消し、他のキー
は無効です。ボックスにはハンクとファイルが示され、取り消せない操作であれば
その旨も表示されます。破棄したハンクは戻せず、未追跡の行で `D` を押すと git
が一度も記録していないファイルが削除されます。

各操作は、画面上の diff から切り出したパッチに対する一度の `git apply` です。
読んだものがそのまま git に渡されます。ステージ済みの行の破棄は `--index` を
使うため、インデックスと作業ツリーは一緒に変わるか、どちらも変わりません。
ファイルに未ステージの変更が残っていると git が拒否し、先にステージ解除する
よう通知します。適用前にビューアはファイルとインデックスエントリを読み直し、
diff を読んだ時点から変わっていれば `the diff changed; look again` と表示して
拒否します。サイズ上限で切り詰められた行、バイナリファイル、サブモジュール
ポインタは操作できません。

ビューアが表示する diff はすべて、3行のコンテキスト、`a/` と `b/` の接頭辞、
textconv なしで生成されるため、`diff.context`、`diff.noprefix`、
`diff.mnemonicPrefix`、textconv ドライバは影響しません。
`apply.ignoreWhitespace` は無効に、`apply.whitespace` は `nowarn` に固定され
ます。branch スコープではこの3つのキーは `switch to worktree scope (b) to
stage or discard` を表示します。

ビューアが実行する git サブコマンドは11個です。ブランチスコープの10個の読み
取りコマンドと、確定したキーからのみ実行される `apply` です。
```

The closing line of キー操作 becomes `フェーズ2では \`i I u U x X v y Y @ c /\` を予約し、操作を割り当てていません。`; the first paragraph drops 読み取り専用 and says 確認ボックスの後にハンクをステージできます; 既知の制限's first two sentences become `PORT-SURFACE.md の K1–K7 は 0.0.4 で修正済みです。コメントは未実装です。`; ロードマップ's P2 gains `（0.0.4）`.

Check with `grep -n '0.0.3\|read-only\|只读\|読み取り専用' README*.md`: the remaining hits must be the "Branch scope is read-only like the rest of the viewer" paragraph, which becomes `Branch scope runs only reads: ...` in each language (`分支范围只运行只读命令`, `ブランチスコープは読み取りのみを実行します`).

- [ ] **Step 4: `AGENTS.md`, the manifest, the version and the acceptance row**

`AGENTS.md`: the reserved-key bullet becomes `Reserved keys remain unbound: \`i I u U x X v y Y @ c /\`. \`b\` and \`B\` are the scope keys; \`s\`, \`d\` and \`D\` are the Phase 2 write actions, bound in \`worktree\` scope only and, in \`branch\` scope, showing \`switch to worktree scope (b) to stage or discard\`.`; the `M` bullet's `the git allow-list remains unchanged at ten subcommands` becomes `the ten read subcommands are unchanged; \`apply\` is the eleventh and runs only from a confirmed \`s\`, \`d\` or \`D\` (\`tests/hunk_actions.rs\`)`; in "Three invariants", item 1's first sentence becomes `**Read-only except on confirmation (G7, spec 9.6).** The viewer runs only the git subcommands allow-listed in \`tests/readonly_guarantee.rs\`, plus \`apply\` from a confirmed action, which \`tests/hunk_actions.rs\` records form by form.`; the "Phase 1 is a read-only viewer" bullet at the top becomes `Phases 1-2 ship a viewer whose only mutations are the confirmed hunk actions of spec 9. Keep comments and agent dispatch out.`; the architecture paragraph "The engine is the only caller of the frozen tree" stays true. Add to the "State on disk" paragraph nothing (no new file).

`herdr-plugin.toml`: `version = "0.0.4"`, `description = "Git hunk viewer: changed files, hunks, a clickable navigation toolbar, and staging behind a y/n box."`. `Cargo.toml`: `version = "0.0.4"`; `Cargo.lock`: the `herdr-hunks` package's version line (`cargo check --locked` must still pass, so edit the lock's own line rather than regenerating it).

`docs/acceptance-p1.md`: add row 9 and update the closing paragraph (`after all nine rows pass`, `Rows 1–8 belong to the earlier releases; row 9 covers 0.0.4.`):

```markdown
| 9. Hunk actions | On an `MM` file with two hunks on each side, press `s` on an unstaged hunk, `s` on the staged row, `d` on an unstaged hunk, `d` on a staged hunk while the file has unstaged changes and again once it has none, comparing with `git diff` and `git diff --cached` after each step; `D` then `n` on an untracked row, `D` then `y`; the keys in branch scope; an edit by the agent between the frame and `y`; from a pane in a subdirectory (K1) a staged rename with edits (K4), an `MD` path (K5) and `tools` replaced by `tools/run` in the index (K7). | Each step changes exactly the hunk or row the box named and nothing else; the staged discard is refused with the unstage hint while unstaged edits remain and discards exactly that hunk once they are gone; `n` changes nothing and `y` deletes the file; branch scope shows the notice; the edited hunk is refused with `the diff changed; look again`; the four defect fixtures behave as spec 9 says. | PENDING | | | |
```

Change the first line to `Status: PENDING` in the same edit: the release guard reads that line, and 0.0.4 must not be publishable until row 9 is recorded. The orchestrator fills the row by hand and flips the line back to `Status: PASS` only then.

- [ ] **Step 5: Gates and commit**

Run the full gate line, `cargo test --locked --test version_sync` included (it pins the three version files to each other), and `cargo build --release` for the Tier B run.

```bash
git add tests/hunk_actions.rs tests/e2e_real_herdr.rs README.md README.zh-CN.md README.ja.md AGENTS.md docs/acceptance-p1.md Cargo.toml Cargo.lock herdr-plugin.toml
git commit -m "feat: hunk actions, documented and versioned 0.0.4"
```

---

## Self-review against the spec

- **9.1** is prose; nothing to build.
- **9.2 "The diff behind the row"**: Task 1 (`worktree::diff`, the flags, the cut, `patch`, `rename_sources` both sides, the branch command's two new flags, the oracle). The `-M <old>` for an unstaged intent-to-add rename: Task 1 test `an_intent_to_add_rename_is_found_on_the_unstaged_side`.
- **9.2 the table and the forms**: Task 3 (`Form`), Task 4 (`plan`, the engine tests per row kind), Task 6 (recorded forms).
- **9.2 "Discarding a file"**: Task 4 `whole_file_discards_follow_the_row...` and `a_staged_discard_is_atomic...`.
- **9.2 "Slicing a hunk"** items 1-3: Task 3 tests `a_hunk_carries_its_header...`, `a_type_change_counts...`, `mode_lines_leave...`, `a_rename_section_keeps...`, `coordinates_are_recounted...`.
- **9.2 "What cannot be acted on"**: Task 4 `plan` tests (every notice), Task 5 `open_box` and its tests; the binary rule on untracked rows (F7) in both.
- **9.2 "Stale content"**: Task 1 pre-image bracket, Task 4 `check_pre_image` (`a_pre_image_that_changed_refuses...`, the MD case, `a_working_tree_form_on_a_directory_is_refused` on the K7 fixture, `an_edit_during_the_diff_read_leaves_no_pre_image_and_refuses_the_key`).
- **9.2 "After the action"**: Task 4's same-path selection rule, tested in `untracked_rows_are_staged_or_deleted_whole...` (row gone) and `a_hunk_is_staged_unstaged_and_discarded...` (row survives).
- **9.3** whole: Task 5; the eight-line box and the left-shortened path in `confirm.rs` tests; the drawn rule in `y_is_inert_until_the_box_was_drawn`; the guard in `a_second_key_while...`; the notices and invariant 3 in the two `observe` tests; the toolbar group, footer, branch scope in the view tests; `X` reserved in `keys.rs`.
- **9.4** whole: Task 4 (`Command::Act`, the carrying refresh, answers decided before git, `action_applied`, the acted Arc, the fresh Arc, the runner's retry with re-check); Task 1 (the diffs, the lane). The `index.lock` retry and its 2 s bound are pinned by `an_index_lock_is_retried_and_given_up_after_two_seconds`, which holds a real `.git/index.lock`; the generation bump at the pop and the lane held across the forms by `a_diff_read_before_the_action_is_never_published_after_it`.
- **9.5**: Task 2 (patch 0004, `PORT-SURFACE.md`), Task 6 (version, README).
- **9.6**: Task 1 (readonly's D7 assertions, allow-list at ten), Task 6 (`hunk_actions.rs`: forms, refs, index and worktree differences, no apply on refusals, the state directory).
- **9.7**: the table's rows map to: branch scope (Task 5/3), no hunk (3/4), cut/binary/submodule (3/4), running (3/4), changed (3), eligibility (3), the box below 40x10 (4), git refusal (3, `a_staged_discard...`), `does not match index` with and without the file (3), not a regular file (3), index.lock (2), timeout (2), write failure (prose), no toplevel (4: `an_act_outside_a_repository_is_answered_not_a_git_repository`; outside a repository no diff is ever published, so the identity check answers first, which is why the test expects `the diff changed; look again` and the `not a git repository` arm stays a guard for a repository that vanishes between publication and the key), status failure after forms (3 by ordering), HEAD moves (8.2's tests stand). Cost and configuration: README (Task 6).
- **9.8 items 1-11**: 1 Task 3; 2 Task 3; 3 Task 1; 4 Task 4; 5 Task 1 (`superseded_selections_never_reach_git`); 6 Task 2; 7-8 Task 5; 9 Tasks 1 and 6; 10 Task 6; 11 Task 2. Criteria 12-15: acceptance row 9 (Task 6).

Placeholder scan: none. Type consistency: `Action`, `ActionKind`, `Form`, `Direction`, `Kind`, `Refusal`, `PreImage`, `WorktreeKind`, `Confirm`, `PendingAction` are defined once and used by those names throughout; `LoadedDiff::build` takes six arguments from Task 1 on and every test helper passes `patch, pre_image`. Two deliberate simplifications are recorded above: `rename_sources` keyed by path (Task 1), and the dim chip drawn as a plain span (Task 5).

## Review notes

Appended after each review round.

**Round 1 (codex, plan-complete, 2026-10-01).** Thirteen findings, all applied. The
patch task moved from fifth to second so the engine's `MD` tests have the rows they
need. The box now sizes itself by `dialog::line_count` and `confirm.drawn` is set only
when the whole box fits the frame. The diff generation is bumped when an `Act` is popped
from the queue and the carrying job holds the diff lane across the forms, so no read
started before the mutation is published after it (test
`a_diff_read_before_the_action_is_never_published_after_it`). Four tests that the first
draft left to inspection are now steps: the `index.lock` retry and bound, an edit during
the diff read, the directory refusal, and an `Act` outside a repository; the recording
test grew to seven applies with per-action refs, index and worktree checks. The
acceptance file goes to `Status: PENDING` with row 9. Five snippets were corrected to
compile: `parse_for_tests` at module level, the session tests' existing `git_out`
reused, `Arc::as_ptr` on the `Arc` not the reference, `columns` instead of a shadowed
`width`, and `observe` taking the pending action before it speaks. The runner is `pub`
so the slicer task passes clippy before the engine task calls it, and the large-patch
fixture is 120,000 lines.

**Round 2 (codex, plan-complete, 2026-10-01).** Ten findings, all applied. One was a spec
defect the plan surfaced: the absence guard for `--index` forms would have refused `D` on a
clean staged deletion, where `--index -R` is the restore the table promises; the guard now
applies only while the index still holds an entry (`MD`), the spec sentence is amended in
this planning commit, and `D` on a staged deletion is tested directly. The section cutter is moved
verbatim with its mixed-quoting tests instead of rewritten. The engine test helpers refresh
before they select and never wait for a snapshot that may already have been consumed; the
guard tests run with the poll an hour away and the acted-Arc case holds the reload behind the
gate so identity alone would pass. The runner's stdin writer now sits inside the timeout and
is aborted on expiry, with a grandchild-holds-the-pipe test. The slicer's fixture is a valid
two-hunk diff whose slices are applied forward and reverse against real files, and the diff
oracle compares line contents and the patch bytes against an independent `git diff`. The
dialog footer assertion reads the penultimate line, and the two test filters run separately.
