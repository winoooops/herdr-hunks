# herdr-hunks branch scope Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a second viewing scope to `herdr-hunks`, `branch`, that lists and diffs every change the current branch carries against the merge-base with a base ref (`main` by default, any commit through a lazygit-style picker), so a reviewer can read what an agent has already committed.

**Architecture:** The engine gains two modules: `src/engine/base.rs` resolves, validates, remembers and lists bases (`rev-parse`, `symbolic-ref`, `for-each-ref`, `bases.json`), and `src/engine/branch.rs` builds branch rows and per-row diffs against a merge-base pinned once per refresh, isolating the `diff --git` section of the row before the frozen parser sees it. The session loop publishes a scope, a base and a row list only together, tags every diff with the `Comparison` it was computed under, and drops results from a previous comparison. The TUI adds the `b` key and a scope chip, and `src/tui/picker.rs`, a modal dialog with one filtering input line over the dialog primitive. Nothing in `src/git/` changes except the visibility patch D6.

**Tech Stack:** Rust 1.88, edition 2021; the Phase 1 dependencies unchanged (`tokio` 1, `serde`/`serde_json` 1, `toml` 0.8, `libc` 0.2, `ratatui` 0.30 behind the `tui` feature, `crossterm` 0.29; dev `tempfile` 3). Runtime: `git` 2.31 or newer.

**Spec:** `docs/superpowers/specs/2026-09-22-branch-scope-design.md` (section 7 of `docs/superpowers/specs/2026-09-18-hunks-roadmap-p1-viewer-design.md`; both are read before any task). Each task names the subsections it implements.

## Global Constraints

- The vimeflow pin stays `91e45b1c`. Nobody edits `src/git/` by hand: a change there is a new file in `port/patches/`, generated as Task 1 describes. `scripts/port-check.sh` must pass after every task.
- Read-only guarantee G7 (spec 3.5 and 7.6): the engine spawns only `git --version rev-parse status diff ls-files show cat-file symbolic-ref merge-base for-each-ref`. Every base, ref or object id is one argv element, never interpolated into a shell, and every revision argument is followed by `--` before any path. A base that starts with `-` is refused before git is spawned.
- The merge-base is computed once per refresh with `git merge-base HEAD <commit>` and given to every row and diff command of that refresh; no command uses `git diff --merge-base`.
- The engine calls these frozen functions and no others: `git_status_inner`, `get_git_diff_inner`, `git_branch_inner`, `git_worktree_name_inner`, `start_git_watcher_backend`, `stop_git_watcher_backend`, and, after D6, `run_git_with_timeout`, `parse_git_diff`, `parse_numstat`, `decode_git_patch_path`, `validate_file_path`.
- `bases.json` is written only under an absolute state directory (`src/paths.rs` already returns `None` otherwise), under `reuse::with_lock`, by temp file and rename. Nothing is ever written relative to the repository.
- Reserved keys stay unbound: `s d D i I u U x v y Y @ c /`. `b` and `B` are the only keys added.
- Colours are the terminal's named ANSI colours only. Every string that came from git (labels, refs, error text) passes through `tui::sanitize` before it reaches a cell.
- Phase 1 behaviour with `[view] scope` unset is unchanged: every existing test keeps passing without weakening.
- Commits are conventional with a lowercase subject; inline comments are one short line and never reference a task or PR. The orchestrator makes every commit; the implementer leaves the tree uncommitted.
- `cargo test` is run with a writable `HOME` outside any git repository (the frozen test helpers create fixtures under `$HOME`); the coder's instructions say how.
- Run before every commit: `cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked && cargo check --locked --no-default-features && scripts/port-check.sh`.
- The version becomes `0.1.0` in Task 6 only; no other task touches `Cargo.toml`, `Cargo.lock` or `herdr-plugin.toml`.

## File Structure

```
port/patches/0003-engine-visibility.patch   D6: five frozen functions become pub(crate) (Task 1)
PORT-SURFACE.md                            D6 registered, K7 registered, port surface list extended (Task 1)
src/engine/types.rs                        FileKey.untracked, Scope, Base, BaseSource, Comparison, Snapshot and Command additions (Task 1)
src/engine/base.rs                         NEW: git runner, check_text, verify, merge_base, resolve, bases.json, list_refs (Task 2)
src/engine/branch.rs                       NEW: name-status/numstat rows, rename sources, section isolation, row diff (Task 3)
src/engine/session.rs                      scope/base state, Job/Loaded, SetScope/SetBase/LoadRefs, comparison-aware diffs (Tasks 2, 3)
src/engine/mod.rs                          module list (Task 2)
src/tui/keys.rs                            `b` (Task 4), `B` (Task 5)
src/tui/config.rs                          [view] scope, [base] ref (Task 4)
src/tui/state.rs                           observe(), seen_base_error (Task 4), picker field (Task 5)
src/tui/view.rs                            scope chip, STAGED hidden in branch scope (Task 4); picker overlay and hits (Task 5)
src/tui/input.rs                           b (Task 4); picker keys and mouse (Task 5)
src/tui/picker.rs                          NEW: Picker, PickerRow, panel() (Task 5)
src/tui/shell.rs                           config -> SessionConfig, observe() in the loop (Task 4)
tests/readonly_guarantee.rs                allow-list of 10, branch scope drive, state-directory assertion (Task 6)
tests/e2e_real_herdr.rs                    presses `b` through the real host (Task 6)
README.md, README.zh-CN.md, README.ja.md   "Branch scope" section, keys, config keys, K7 (Task 6)
AGENTS.md, docs/acceptance-p1.md           scope boundary; rows 6 and 7 (Task 6)
Cargo.toml, Cargo.lock, herdr-plugin.toml  0.1.0 (Task 6)
```

---

### Task 1: D6 visibility patch, K7, `FileKey.untracked`, scope and comparison types, allow-list

Implements spec 7.2 (types), 7.6 (allow-list), 7.7 (D6, K7). Read those before starting.

**Files:**
- Create: `port/patches/0003-engine-visibility.patch`
- Modify: `PORT-SURFACE.md` (Port surface list, Registered divergences, Known defects)
- Modify: `src/engine/types.rs`
- Modify: `src/engine/session.rs` (`key_of`, `LoadedDiff::build` call, `fingerprint`, tests)
- Modify: `src/tui/view.rs`, `src/tui/input.rs`, `src/tui/state.rs` (every `FileKey` construction or field comparison)
- Modify: `tests/readonly_guarantee.rs` (`ALLOWED`)

**Interfaces:**
- Consumes: the frozen `ChangedFile { path, status: ChangedFileStatus, staged, insertions, deletions }` and `GetGitDiffResponse`.
- Produces (every later task relies on these exact names):

```rust
// src/engine/types.rs
pub struct FileKey { pub path: String, pub staged: bool, pub untracked: bool }
impl FileKey { pub fn of(file: &ChangedFile) -> Self }
pub enum Scope { Worktree, Branch }            // Copy; Scope::other(self) -> Scope
pub enum BaseSource { Picked, Config, Default } // Copy
pub struct Base { pub requested: String, pub commit: String, pub merge_base: Option<String>, pub source: BaseSource }
impl Base { pub fn label(&self) -> &str }
pub fn ref_label(requested: &str) -> &str
pub enum Comparison { Worktree, Branch { merge_base: String } }   // PartialEq
pub const NO_BASE_NOTICE: &str = "no base branch: set [base] ref or press B";
// LoadedDiff gains `pub comparison: Comparison`; build(key, comparison, response), build_with_cap(key, comparison, response, cap)
// Snapshot gains: scope, base, base_error, default_base, rename_sources, refs, refs_overflow, pick_seq, pick_error
// Command gains: SetScope(Scope), SetBase(Option<String>), LoadRefs
```

One precision over the spec's sketch: `Base.merge_base` is `Option<String>`, `None` in worktree scope, where no merge-base has been computed for the current ids (7.5 computes it only in branch scope). A snapshot in branch scope always carries `Some`.

- [ ] **Step 1: Write the failing type tests**

Append to the `tests` module of `src/engine/types.rs` (keep the existing tests; they are updated in Step 3):

```rust
    #[test]
    fn a_recreated_untracked_path_is_a_different_key_from_its_deleted_row() {
        let deleted = ChangedFile {
            path: "f".into(),
            status: ChangedFileStatus::Deleted,
            staged: false,
            insertions: None,
            deletions: None,
        };
        let untracked = ChangedFile {
            status: ChangedFileStatus::Untracked,
            ..deleted.clone()
        };
        assert_ne!(FileKey::of(&deleted), FileKey::of(&untracked));
        assert_eq!(FileKey::of(&deleted).untracked, false);
        assert_eq!(FileKey::of(&untracked).untracked, true);
    }

    #[test]
    fn labels_strip_the_ref_namespace_and_leave_free_text_alone() {
        assert_eq!(ref_label("refs/heads/main"), "main");
        assert_eq!(ref_label("refs/remotes/origin/main"), "origin/main");
        assert_eq!(ref_label("refs/tags/v1"), "v1");
        assert_eq!(ref_label("HEAD~3"), "HEAD~3");
        let base = Base {
            requested: "refs/heads/feat/x".into(),
            commit: "0".repeat(40),
            merge_base: None,
            source: BaseSource::Default,
        };
        assert_eq!(base.label(), "feat/x");
        assert_eq!(Scope::Worktree.other(), Scope::Branch);
        assert_eq!(Scope::Branch.other(), Scope::Worktree);
    }

    #[test]
    fn a_loaded_diff_remembers_its_comparison() {
        let branch = Comparison::Branch {
            merge_base: "1".repeat(40),
        };
        let loaded = LoadedDiff::build(key(), branch.clone(), response(vec![hunk(1, 1)]));
        assert_eq!(loaded.comparison, branch);
        assert_ne!(loaded.comparison, Comparison::Worktree);
        let empty = Snapshot::empty("/r");
        assert_eq!(empty.scope, Scope::Worktree);
        assert!(empty.base.is_none() && empty.refs.is_none() && !empty.refs_overflow);
        assert_eq!(empty.pick_seq, 0);
        assert!(empty.rename_sources.is_empty());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib engine::types`
Expected: compile errors (`untracked`, `Scope`, `ref_label`, `Comparison` unknown).

- [ ] **Step 3: Write the types**

Replace the head of `src/engine/types.rs` (everything above `#[derive(Debug, Clone, PartialEq, Eq)] pub enum RepoState`) with:

```rust
//! Snapshot types the UI renders from. No terminal types here.
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::engine::nav::{targets_for_diff, unified_order, Target};
use crate::git::{ChangedFile, ChangedFileStatus, FileDiff, GetGitDiffResponse};

pub const MAX_DIFF_LINES: usize = 200_000;

/// Shown by `b` when nothing resolves as a base.
pub const NO_BASE_NOTICE: &str = "no base branch: set [base] ref or press B";

/// vimeflow's file identity: a partially staged path is two rows. In branch
/// scope a path deleted on the branch and recreated untracked is two rows too.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FileKey {
    pub path: String,
    pub staged: bool,
    pub untracked: bool,
}

impl FileKey {
    pub fn of(file: &ChangedFile) -> Self {
        Self {
            path: file.path.clone(),
            staged: file.staged,
            untracked: matches!(file.status, ChangedFileStatus::Untracked),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Worktree,
    Branch,
}

impl Scope {
    pub fn other(self) -> Self {
        match self {
            Self::Worktree => Self::Branch,
            Self::Branch => Self::Worktree,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseSource {
    Picked,
    Config,
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Base {
    /// Exactly what git is given: a qualified ref for a picked or resolved
    /// branch, the typed text for free text and config.
    pub requested: String,
    /// Object id of `requested` as of the last refresh that verified it.
    pub commit: String,
    /// merge-base(HEAD, commit) the current rows were computed against; `None` in worktree scope.
    pub merge_base: Option<String>,
    pub source: BaseSource,
}

impl Base {
    pub fn label(&self) -> &str {
        ref_label(&self.requested)
    }
}

/// `refs/heads/x` -> `x`, `refs/remotes/o/x` -> `o/x`, `refs/tags/v` -> `v`; anything else unchanged.
pub fn ref_label(requested: &str) -> &str {
    ["refs/heads/", "refs/remotes/", "refs/tags/"]
        .iter()
        .find_map(|prefix| requested.strip_prefix(prefix))
        .unwrap_or(requested)
}

/// What a diff's hunks were computed against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Comparison {
    Worktree,
    Branch { merge_base: String },
}
```

In `LoadedDiff` add `pub comparison: Comparison,` after `key`, and change the constructors:

```rust
impl LoadedDiff {
    pub fn build(key: FileKey, comparison: Comparison, response: GetGitDiffResponse) -> Self {
        Self::build_with_cap(key, comparison, response, MAX_DIFF_LINES)
    }

    pub fn build_with_cap(
        key: FileKey,
        comparison: Comparison,
        response: GetGitDiffResponse,
        cap: usize,
    ) -> Self {
```

and set `comparison,` in the returned struct. Replace `Snapshot` and `Command`:

```rust
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub revision: u64,
    pub repo: RepoState,
    /// `scope`, `base`, `files` and `rename_sources` always describe one comparison.
    pub scope: Scope,
    pub base: Option<Base>,
    /// A skipped resolution step, a `bases.json` problem or a pick that was not remembered; shown once.
    pub base_error: Option<String>,
    /// What steps 2-5 of the resolution order name, for the picker's reset row.
    pub default_base: Option<String>,
    pub files: Vec<ChangedFile>,
    /// Branch scope only: a renamed row's path -> its old path.
    pub rename_sources: Arc<BTreeMap<String, String>>,
    pub selected: Option<FileKey>,
    pub diff: DiffState,
    pub status_error: Option<String>,
    pub watcher_error: Option<String>,
    pub refreshing: bool,
    /// The picker's candidates, qualified, most recently created first; `None` until `LoadRefs`.
    pub refs: Option<Arc<Vec<String>>>,
    pub refs_overflow: bool,
    /// Bumped once per answered `SetBase`; `pick_error` is that answer.
    pub pick_seq: u64,
    pub pick_error: Option<String>,
}

impl Snapshot {
    pub fn empty(cwd: &str) -> Self {
        Self {
            revision: 0,
            repo: RepoState::NotARepo {
                cwd: cwd.to_string(),
            },
            scope: Scope::Worktree,
            base: None,
            base_error: None,
            default_base: None,
            files: Vec::new(),
            rename_sources: Arc::new(BTreeMap::new()),
            selected: None,
            diff: DiffState::Idle,
            status_error: None,
            watcher_error: None,
            refreshing: false,
            refs: None,
            refs_overflow: false,
            pick_seq: 0,
            pick_error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Select(FileKey),
    SelectNext,
    SelectPrev,
    Refresh,
    /// Load the rows of the other scope; published together with them.
    SetScope(Scope),
    /// `Some`: validate, load branch rows under it, persist, publish. `None`: forget the pick and re-resolve.
    SetBase(Option<String>),
    /// Answer with `refs` on the snapshot.
    LoadRefs,
    Shutdown,
}
```

Update the existing tests in the same file: `key()` returns `FileKey { path: "f".into(), staged: false, untracked: false }`; every `build_with_cap(key(), response(..), cap)` becomes `build_with_cap(key(), Comparison::Worktree, response(..), cap)`; the `hunk`/`response`/`key` helpers stay as they are.

- [ ] **Step 4: Follow the type change through the crate**

`src/engine/session.rs`:
- `key_of` becomes `fn key_of(file: &ChangedFile) -> FileKey { FileKey::of(file) }` (keep the name; every call site stays).
- `LoadedDiff::build(key, response)` becomes `LoadedDiff::build(key, Comparison::Worktree, response)`; import `Comparison` from `super`.
- In `request_diff`, delete the `untracked` lookup over `snapshot.files` and pass `Some(key.untracked)` to `get_git_diff_inner`.
- `fingerprint` appends the new fields so a scope, base or refs change is never deduplicated:

```rust
    format!(
        "{:?}|{}|{:?}|{}|{:?}|{:?}|{}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{}|{}|{:?}",
        s.repo,
        serde_json::to_string(&s.files).unwrap_or_default(),
        s.selected,
        diff,
        s.status_error,
        s.watcher_error,
        s.refreshing,
        s.scope,
        s.base,
        s.base_error,
        s.default_base,
        s.rename_sources,
        s.refs.as_ref().map(Arc::as_ptr),
        s.refs_overflow,
        s.pick_seq,
        s.pick_error
    )
```

- In the session tests, `FileKey { path: "a.txt".into(), staged: false }` gains `untracked: false`.

`src/tui/view.rs`: the two `position(|f| f.path == k.path && f.staged == k.staged)` / `position(|file| file.path == key.path && file.staged == key.staged)` lookups (in `toolbar_items` and `files_lines`) become `position(|f| FileKey::of(f) == *k)`; import `FileKey` from `crate::engine`.

`src/tui/input.rs`: the `SelectFile(index)` arm builds `Command::Select(FileKey::of(file))`.

`src/tui/state.rs` tests: the `snapshot` builder's key gains `untracked: false` and calls `LoadedDiff::build(key.clone(), Comparison::Worktree, GetGitDiffResponse { .. })`; import `Comparison`.

`tests/readonly_guarantee.rs`: `ALLOWED` becomes `[&str; 10]` with `"merge-base"` and `"for-each-ref"` appended (the drive that exercises them is Task 6).

Search for any other `FileKey {` literal: `grep -rn "FileKey {" src tests` must list only the sites above plus `FileKey::of` itself.

- [ ] **Step 5: Run the type tests and the whole suite**

Run: `cargo test`
Expected: PASS, including the three new tests.

- [ ] **Step 6: Generate the D6 patch**

Edit `src/git/mod.rs` and nothing else in the frozen tree, changing exactly five signatures (line numbers as of the current tree):

```
25:   async fn run_git_with_timeout(       ->  pub(crate) async fn run_git_with_timeout(
92:   fn validate_file_path(               ->  pub(crate) fn validate_file_path(
521:  fn parse_numstat(                    ->  pub(crate) fn parse_numstat(
909:  fn parse_git_diff(                   ->  pub(crate) fn parse_git_diff(
1031: fn decode_git_patch_path(            ->  pub(crate) fn decode_git_patch_path(
```

Then produce the patch from that edit and give it the same header form as `0001-no-ext-diff.patch`:

```bash
{
  printf '%s\n' 'Reason: D6. The engine builds the branch-scope commands itself (spec 7.2, 7.7) and needs the' \
    'frozen runner, both parsers, the patch-path decoder and the path check. Visibility only:' \
    'pub(crate) on five functions, no behaviour change, no frozen test affected.' ''
  git diff --no-color -- src/git/mod.rs | sed -n '/^--- a\//,$p'
} > port/patches/0003-engine-visibility.patch
```

The `sed` keeps the patch from its `--- a/src/git/mod.rs` line on, as the other two patches are written. Verify: `scripts/port-check.sh` must report `src/git matches 91e45b1c + 3 patch(es)` (it applies the patches to the pristine pin in name order, so 0003's hunk offsets are relative to the tree after 0001 and 0002, which is the tree you edited).

- [ ] **Step 7: Register D6 and K7 in `PORT-SURFACE.md`**

In the `## Pin` paragraph that lists the patches, extend the list to `` `0001-no-ext-diff.patch` (D4), `0002-drain-sync-output.patch` (D5) and `0003-engine-visibility.patch` (D6) ``. In `## Port surface`, add the five D6 functions to the list of frozen functions the engine calls, each marked `(D6, pub(crate))`. Under `## Registered divergences`, after D5:

```markdown
**D6 (branch scope, patch): engine visibility.** Branch scope builds its own
`merge-base`, `name-status`, `numstat` and per-row `diff` commands (spec 7.2)
instead of the frozen `get_git_diff_inner`, which hard-codes its two bases.
`port/patches/0003-engine-visibility.patch` changes `run_git_with_timeout`,
`validate_file_path`, `parse_numstat`, `parse_git_diff` and
`decode_git_patch_path` from private to `pub(crate)`. Nothing else changes: the
30 s timeout, the D3-compatible spawn and both parsers behave exactly as
before, and every frozen test is unaffected.
```

Under `## Known defects`, retitle `**K1-K6: known defects.**` to `**K1-K7: known defects.**`, change "All five are fixed in Phase 2" to "K1-K5 and K7 are fixed in Phase 2", and append:

```markdown
- **K7.** A staged row whose path was a file and is now a directory (`D tools`
  next to `A tools/run` in the index) gets both patches from
  `git diff --cached -- tools`, because a pathspec matches its descendants, and
  `parse_git_diff` reads the second file's headers as content of the first.
  Rare, worktree scope only. Branch scope is unaffected: the engine cuts the
  output into `diff --git` sections and keeps only the row's own (spec 7.2).
```

- [ ] **Step 8: Verify**

Run: `scripts/port-check.sh && scripts/port-check-selftest.sh && cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked && cargo check --locked --no-default-features`
Expected: all pass; `port-check` reports 3 patches.

- [ ] **Step 9: Commit (orchestrator)**

```bash
git add -A
git commit -m "feat(engine): scope and base types, untracked file keys, d6 visibility patch"
```

---

### Task 2: Base resolution, validation, `bases.json` and the ref list (`src/engine/base.rs`)

Implements spec 7.3 (resolution order, validation, remembering a pick, the session override's inputs), 7.4 (the ref list) and 7.8 (`bases.json` problems). Read those before starting. This task is a library module with its own tests; Task 3 wires it into the session loop.

**Files:**
- Create: `src/engine/base.rs`
- Modify: `src/engine/mod.rs` (add `pub mod base;`)
- Modify: `src/engine/session.rs` (`SessionConfig` gains three fields; `production` and every test-built config set them)

**Interfaces:**
- Consumes: `crate::git::run_git_with_timeout` (D6), `crate::actions::reuse::with_lock`, `Base`, `BaseSource` (Task 1).
- Produces:

```rust
pub const REFS_CAP: usize = 200;
pub(crate) async fn git(toplevel: &str, args: &[&str]) -> Result<std::process::Output, String>;
pub fn check_text(text: &str) -> Result<(), String>;                         // `-`-led or empty -> Err("not a commit: <text>")
pub(crate) async fn verify(toplevel: &str, text: &str) -> Result<String, String>;   // object id or "not a commit: <text>"
pub(crate) async fn merge_base(toplevel: &str, commit: &str) -> Result<String, String>;
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolveInputs { pub session_pick: Option<Option<String>>, pub config: Option<String>, pub state_dir: Option<PathBuf> }
pub struct Resolution { pub base: Option<Base>, pub default: Option<String>, pub skipped: Vec<String> }
pub(crate) async fn resolve(toplevel: &str, inputs: &ResolveInputs) -> Resolution;
pub fn load_picks(state_dir: &Path) -> (BTreeMap<String, String>, Option<String>);
pub fn save_pick(state_dir: &Path, toplevel: &str, pick: Option<&str>) -> std::io::Result<()>;
pub fn note_problem(state_dir: &Path, line: &str);
pub(crate) async fn list_refs(toplevel: &str) -> Result<(Vec<String>, bool), String>;
// SessionConfig gains: pub scope: Scope, pub base_ref: Option<String>, pub state_dir: Option<PathBuf>
```

- [ ] **Step 1: Write the failing tests**

Create `src/engine/base.rs` with only a `tests` module for now (the implementation follows in Step 3):

```rust
//! Base resolution, validation, remembered picks and the picker's ref list (spec 7.3, 7.4).

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
        assert_eq!(check_text("--output=x"), Err("not a commit: --output=x".into()));
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
        run(dir.path(), &["update-ref", "refs/remotes/origin/trunk", "HEAD"]);
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
        assert_eq!((base.requested.as_str(), base.source), ("feat", BaseSource::Config));
        assert_eq!(r.default.as_deref(), Some("feat"));

        save_pick(state.path(), &top, Some("refs/tags/v1")).unwrap();
        let r = rt().block_on(resolve(&top, &config));
        let base = r.base.unwrap();
        assert_eq!((base.requested.as_str(), base.source), ("refs/tags/v1", BaseSource::Picked));
        assert_eq!(r.default.as_deref(), Some("feat"), "the reset row names steps 2-5");

        // A session override beats the file; a reset override ignores it.
        let over = ResolveInputs {
            session_pick: Some(Some("refs/heads/main".into())),
            ..config.clone()
        };
        assert_eq!(rt().block_on(resolve(&top, &over)).base.unwrap().requested, "refs/heads/main");
        let reset = ResolveInputs {
            session_pick: Some(None),
            ..config.clone()
        };
        assert_eq!(rt().block_on(resolve(&top, &reset)).base.unwrap().requested, "feat");

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
        assert_eq!(picks.get("/r/one").map(String::as_str), Some("refs/heads/x"));
        assert_eq!(picks.get("/r/two").map(String::as_str), Some("HEAD~2"));
        assert!(problem.is_none());
        save_pick(state.path(), "/r/one", None).unwrap();
        assert!(!load_picks(state.path()).0.contains_key("/r/one"));
        std::fs::write(state.path().join("bases.json"), "{ not json").unwrap();
        let (picks, problem) = load_picks(state.path());
        assert!(picks.is_empty());
        assert!(problem.unwrap().starts_with("bases.json: "));
        save_pick(state.path(), "/r/three", Some("v1")).unwrap();
        assert_eq!(load_picks(state.path()).0.len(), 1, "the next pick rewrites it");
        let mut names: Vec<_> = std::fs::read_dir(state.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["bases.json", "split-panes.lock"], "no temp file is left behind");
        note_problem(state.path(), "bases.json: bad");
        assert_eq!(
            std::fs::read_to_string(state.path().join("config-problems.log")).unwrap(),
            "bases.json: bad\n"
        );
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
        run(p, &["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/main"]);
        let (refs, overflow) = rt().block_on(list_refs(&top)).unwrap();
        assert!(!overflow);
        assert!(!refs.iter().any(|r| r == "refs/remotes/origin/HEAD"), "{refs:?}");
        let pos = |name: &str| refs.iter().position(|r| r == name).unwrap_or_else(|| panic!("{name} in {refs:?}"));
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
        let top = empty.path().canonicalize().unwrap().to_string_lossy().into_owned();
        let (refs, overflow) = rt().block_on(list_refs(&top)).unwrap();
        assert!(refs.is_empty() && !overflow);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib engine::base`
Expected: compile errors (nothing in the module is defined).

- [ ] **Step 3: Write the module**

Put this above the `tests` module of `src/engine/base.rs`:

```rust
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
        None => inputs
            .state_dir
            .as_deref()
            .and_then(|dir| {
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
            Ok(commit) => picked = Some(Base { requested: text, commit, merge_base: None, source: BaseSource::Picked }),
            Err(e) => skipped.push(format!("remembered pick: {e}")),
        }
    }
    let mut default = None;
    if let Some(text) = &inputs.config {
        match verify(toplevel, text).await {
            Ok(commit) => default = Some(Base { requested: text.clone(), commit, merge_base: None, source: BaseSource::Config }),
            Err(e) => skipped.push(format!("[base] ref: {e}")),
        }
    }
    if default.is_none() {
        let mut candidates = vec!["refs/heads/main".to_string()];
        if let Ok(output) = git(toplevel, &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"]).await {
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
                default = Some(Base { requested: text, commit, merge_base: None, source: BaseSource::Default });
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
```

Add `pub mod base;` to `src/engine/mod.rs` (after `pub mod gitver;`).

In `src/engine/session.rs`, add to `SessionConfig`:

```rust
    /// `[view] scope`: the scope loaded first; branch falls back to worktree when no base resolves.
    pub scope: Scope,
    /// `[base] ref`, step 2 of the resolution order.
    pub base_ref: Option<String>,
    /// Where `bases.json` lives; `None` keeps picks for the session only.
    pub state_dir: Option<PathBuf>,
```

`SessionConfig::production` sets `scope: Scope::Worktree, base_ref: None, state_dir: None`; every `SessionConfig { .. }` literal in the session tests gets the same three fields. Import `Scope` from `super`. (The loop does not read them until Task 3.)

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib engine::base`
Expected: PASS (6 tests).

- [ ] **Step 5: Verify**

Run: `cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked && cargo check --locked --no-default-features && scripts/port-check.sh`
Expected: all pass. `clippy` may flag `dead_code` for the `pub(crate)` async functions until Task 3 uses them; if it does, add `#[allow(dead_code)]` on the module line in `mod.rs` with the comment `// used by the session loop from Task 3` and remove it in Task 3.

- [ ] **Step 6: Commit (orchestrator)**

```bash
git add -A
git commit -m "feat(engine): resolve, validate, remember and list base refs"
```

---

### Task 3: Branch rows, pinned merge-base, section isolation and the comparison-aware session

Implements spec 7.2 (rows, per-row diff, section isolation, publication rule, selection on a scope switch, corner cases), 7.3 (when the preference and the ids are re-resolved, the session override), 7.5 (one spawned task per refresh) and the engine halves of 7.4 (`SetBase`, `LoadRefs`) and 7.8 (failure rows). Read those before starting. Tests 1-3 of 7.9 live here.

**Files:**
- Create: `src/engine/branch.rs`
- Modify: `src/engine/mod.rs` (add `pub mod branch;`)
- Modify: `src/engine/session.rs` (state, job, `Done`, command handling, diff requests, tests)

**Interfaces:**
- Consumes: Task 1 types; Task 2 `base::{git, verify, merge_base, resolve, save_pick, list_refs, ResolveInputs}`; frozen `parse_git_diff`, `parse_numstat`, `decode_git_patch_path`, `validate_file_path` (D6), `get_git_diff_inner`, `git_status_inner`.
- Produces:

```rust
// src/engine/branch.rs
pub struct NameStatus { pub status: char, pub old: Option<String>, pub path: String }
pub fn parse_name_status(output: &[u8]) -> Vec<NameStatus>;
pub struct BranchRows { pub files: Vec<ChangedFile>, pub rename_sources: BTreeMap<String, String> }
pub(crate) async fn rows(toplevel: &str, merge_base: &str, untracked: Vec<ChangedFile>) -> Result<BranchRows, String>;
pub fn split_header(header: &str) -> Option<(String, String)>;
pub fn keep_sections(output: &str, path: &str, old: Option<&str>) -> String;
pub(crate) async fn diff(toplevel: &str, merge_base: &str, path: &str, old: Option<&str>) -> Result<GetGitDiffResponse, String>;
// session.rs: the snapshot contract of spec 7.2 (scope/base/rows published together; diffs carry their comparison;
// pick_seq/pick_error answer every SetBase; refs/refs_overflow answer LoadRefs)
```

- [ ] **Step 1: Write the failing pure tests for `branch.rs`**

Create `src/engine/branch.rs` with only this `tests` module (the implementation follows in Step 3):

```rust
//! Branch scope: rows and diffs against a merge-base pinned once per refresh (spec 7.2).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_status_records_carry_the_old_path_for_renames_and_copies() {
        let out = b"M\0a.txt\0R100\0old.txt\0new.txt\0A\0d.txt\0T\0link\0C75\0src\0copy\0D\0gone\0";
        let records = parse_name_status(out);
        let brief: Vec<(char, Option<&str>, &str)> = records
            .iter()
            .map(|r| (r.status, r.old.as_deref(), r.path.as_str()))
            .collect();
        assert_eq!(
            brief,
            vec![
                ('M', None, "a.txt"),
                ('R', Some("old.txt"), "new.txt"),
                ('A', None, "d.txt"),
                ('T', None, "link"),
                ('C', Some("src"), "copy"),
                ('D', None, "gone"),
            ]
        );
        assert!(parse_name_status(b"").is_empty());
    }

    #[test]
    fn headers_split_plain_quoted_and_mixed_sides() {
        assert_eq!(split_header("a/x y b/x y"), None, "ambiguous without quoting");
        assert_eq!(
            split_header(r#""a/sp\303\244ce" "b/sp\303\244ce""#),
            Some(("a/späce".into(), "b/späce".into()))
        );
        assert_eq!(
            split_header(r#""a/q\"uote" b/plain"#),
            Some(("a/q\"uote".into(), "b/plain".into()))
        );
        assert_eq!(
            split_header(r#"a/plain "b/tab\there""#),
            Some(("a/plain".into(), "b/tab\there".into()))
        );
    }

    #[test]
    fn only_sections_naming_the_row_are_kept_and_type_changes_keep_both() {
        let two_paths = "diff --git a/tools b/tools\ndeleted file mode 100644\n--- a/tools\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\ndiff --git a/tools/run b/tools/run\nnew file mode 100644\n--- /dev/null\n+++ b/tools/run\n@@ -0,0 +1 @@\n+y\n";
        assert_eq!(
            keep_sections(two_paths, "tools", None),
            "diff --git a/tools b/tools\ndeleted file mode 100644\n--- a/tools\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\n"
        );
        let type_change = "diff --git a/f b/f\ndeleted file mode 100644\n@@ -1 +0,0 @@\n-x\ndiff --git a/f b/f\nnew file mode 120000\n@@ -0,0 +1 @@\n+target\n";
        assert_eq!(keep_sections(type_change, "f", None), type_change);
        let rename = "diff --git a/old b/new\nsimilarity index 90%\nrename from old\nrename to new\n@@ -1 +1 @@\n-x\n+y\ndiff --git a/new/inner b/new/inner\nnew file mode 100644\n@@ -0,0 +1 @@\n+z\n";
        assert_eq!(keep_sections(rename, "new", Some("old")), &rename[..rename.find("diff --git a/new/inner").unwrap()]);
        let quoted = "diff --git \"a/sp\\303\\244ce\" \"b/sp\\303\\244ce\"\n@@ -1 +1 @@\n-x\n+y\n";
        assert_eq!(keep_sections(quoted, "späce", None), quoted);
        assert_eq!(keep_sections(quoted, "space", None), "");
    }
}
```

(`\303\244` is how git quotes `ä`; the decoded expectation is the plain `ä`.)

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib engine::branch`
Expected: compile errors.

- [ ] **Step 3: Write `branch.rs`**

Above the `tests` module:

```rust
use std::collections::{BTreeMap, HashMap};

use crate::engine::base;
use crate::git::{
    decode_git_patch_path, get_git_diff_inner, parse_git_diff, parse_numstat, validate_file_path,
    ChangedFile, ChangedFileStatus, FileDiff, GetGitDiffResponse,
};

/// One `--name-status -z` record; `old` is set for `R` and `C`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameStatus {
    pub status: char,
    pub old: Option<String>,
    pub path: String,
}

/// `<status>\0<path>\0`, or `<status>\0<old>\0<new>\0` for renames and copies (the status may carry a score).
pub fn parse_name_status(output: &[u8]) -> Vec<NameStatus> {
    let mut out = Vec::new();
    let mut fields = output
        .split(|&b| b == 0)
        .map(|f| String::from_utf8_lossy(f).into_owned());
    while let Some(status) = fields.next() {
        let Some(code) = status.chars().next() else {
            break;
        };
        let first = fields.next().unwrap_or_default();
        if matches!(code, 'R' | 'C') {
            let path = fields.next().unwrap_or_default();
            out.push(NameStatus {
                status: code,
                old: Some(first),
                path,
            });
        } else {
            out.push(NameStatus {
                status: code,
                old: None,
                path: first,
            });
        }
    }
    out
}

fn status_of(code: char) -> ChangedFileStatus {
    match code {
        'A' | 'C' => ChangedFileStatus::Added,
        'D' => ChangedFileStatus::Deleted,
        'R' => ChangedFileStatus::Renamed,
        _ => ChangedFileStatus::Modified,
    }
}

pub struct BranchRows {
    pub files: Vec<ChangedFile>,
    pub rename_sources: BTreeMap<String, String>,
}

/// name-status against the pinned merge-base, plus the untracked rows of the same refresh, sorted by path.
pub(crate) async fn rows(
    toplevel: &str,
    merge_base: &str,
    untracked: Vec<ChangedFile>,
) -> Result<BranchRows, String> {
    let names = base::git(toplevel, &["diff", merge_base, "--name-status", "-M", "-z", "--"]).await?;
    if !names.status.success() {
        return Err(format!(
            "git diff --name-status failed: {}",
            String::from_utf8_lossy(&names.stderr).trim()
        ));
    }
    let stats = base::git(toplevel, &["diff", merge_base, "--numstat", "-M", "-z", "--"]).await?;
    let counts: HashMap<String, (u32, u32)> = if stats.status.success() {
        parse_numstat(&stats.stdout)
    } else {
        HashMap::new()
    };
    let mut rename_sources = BTreeMap::new();
    let mut files: Vec<ChangedFile> = parse_name_status(&names.stdout)
        .into_iter()
        .map(|record| {
            let status = status_of(record.status);
            if let (ChangedFileStatus::Renamed, Some(old)) = (&status, &record.old) {
                rename_sources.insert(record.path.clone(), old.clone());
            }
            let (insertions, deletions) = match counts.get(&record.path) {
                Some(&(added, removed)) => (Some(added), Some(removed)),
                None => (None, None),
            };
            ChangedFile {
                path: record.path,
                status,
                staged: false,
                insertions,
                deletions,
            }
        })
        .collect();
    files.extend(
        untracked
            .into_iter()
            .filter(|f| matches!(f.status, ChangedFileStatus::Untracked)),
    );
    // A path deleted on the branch and recreated untracked is two rows; the untracked one comes second.
    files.sort_by(|a, b| {
        a.path.cmp(&b.path).then_with(|| {
            matches!(a.status, ChangedFileStatus::Untracked)
                .cmp(&matches!(b.status, ChangedFileStatus::Untracked))
        })
    });
    Ok(BranchRows {
        files,
        rename_sources,
    })
}

/// Index just past the closing quote of a C-quoted token that starts at byte 0.
fn quoted_end(text: &str) -> Option<usize> {
    let mut escaped = false;
    for (i, ch) in text.char_indices().skip(1) {
        match ch {
            '\\' if !escaped => escaped = true,
            '"' if !escaped => return Some(i + 1),
            _ => escaped = false,
        }
    }
    None
}

/// `a/<old> b/<new>` from a `diff --git` header line, with a quoted side decoded.
/// `None` when both sides are plain: a plain path may contain spaces, so the
/// caller compares the whole header against what it expects instead.
pub fn split_header(header: &str) -> Option<(String, String)> {
    if header.starts_with('"') {
        let end = quoted_end(header)?;
        let a = decode_git_patch_path(&header[..end]);
        let b = header.get(end + 1..)?;
        let b = if b.starts_with('"') {
            decode_git_patch_path(b)
        } else {
            b.to_string()
        };
        return Some((a, b));
    }
    if header.ends_with('"') {
        let start = header.find('"')?;
        let a = header.get(..start.checked_sub(1)?)?.to_string();
        return Some((a, decode_git_patch_path(&header[start..])));
    }
    None
}

fn names_row(header: &str, wanted_a: &str, wanted_b: &str) -> bool {
    if header == format!("{wanted_a} {wanted_b}") {
        return true;
    }
    matches!(split_header(header), Some((a, b)) if a == wanted_a && b == wanted_b)
}

/// The `diff --git` sections whose header names the row: `a/<old-or-path> b/<path>`.
/// A pathspec matches its descendants, so `git diff -- tools` also prints `tools/run`.
pub fn keep_sections(output: &str, path: &str, old: Option<&str>) -> String {
    let wanted_b = format!("b/{path}");
    let wanted_a = format!("a/{}", old.unwrap_or(path));
    let mut kept = String::new();
    let mut keep = false;
    for line in output.split_inclusive('\n') {
        if let Some(header) = line.strip_prefix("diff --git ") {
            keep = names_row(header.trim_end_matches('\n'), &wanted_a, &wanted_b);
        }
        if keep {
            kept.push_str(line);
        }
    }
    kept
}

/// Each kept section on its own; a type change is two sections for one path.
fn sections(raw: &str) -> Vec<&str> {
    let starts: Vec<usize> = raw
        .match_indices("diff --git ")
        .map(|(i, _)| i)
        .filter(|&i| i == 0 || raw.as_bytes()[i - 1] == b'\n')
        .collect();
    starts
        .iter()
        .enumerate()
        .map(|(n, &start)| &raw[start..starts.get(n + 1).copied().unwrap_or(raw.len())])
        .collect()
}

/// A tracked row's diff against the pinned merge-base, parsed section by section by the frozen parser.
pub(crate) async fn diff(
    toplevel: &str,
    merge_base: &str,
    path: &str,
    old: Option<&str>,
) -> Result<GetGitDiffResponse, String> {
    validate_file_path(path)?;
    if let Some(old) = old {
        validate_file_path(old)?;
    }
    let mut args = vec![
        "diff",
        merge_base,
        "--no-color",
        "--no-ext-diff",
        "--src-prefix=a/",
        "--dst-prefix=b/",
    ];
    if old.is_some() {
        args.push("-M");
    }
    args.push("--");
    if let Some(old) = old {
        args.push(old);
    }
    args.push(path);
    let output = base::git(toplevel, &args).await?;
    if !output.status.success() {
        return Err(format!(
            "git diff failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let raw_diff = keep_sections(&String::from_utf8_lossy(&output.stdout), path, old);
    let mut file_diff = FileDiff {
        file_path: path.to_string(),
        old_path: None,
        new_path: None,
        hunks: Vec::new(),
    };
    for section in sections(&raw_diff) {
        let parsed = parse_git_diff(section, path);
        file_diff.old_path = file_diff.old_path.or(parsed.old_path);
        file_diff.new_path = file_diff.new_path.or(parsed.new_path);
        file_diff.hunks.extend(parsed.hunks);
    }
    Ok(GetGitDiffResponse {
        file_diff,
        old_text: String::new(),
        new_text: String::new(),
        raw_diff,
        repo_root: toplevel.to_string(),
    })
}

/// An untracked row in branch scope uses the Phase 1 path: `--no-index` against `/dev/null`.
pub(crate) async fn untracked_diff(cwd: String, path: String) -> Result<GetGitDiffResponse, String> {
    get_git_diff_inner(cwd, path, false, Some(true)).await
}
```

Add `pub mod branch;` to `src/engine/mod.rs`.

- [ ] **Step 4: Run the pure tests**

Run: `cargo test --lib engine::branch`
Expected: PASS (3 tests).

- [ ] **Step 5: Write the failing session tests**

Append to the `tests` module of `src/engine/session.rs`. The existing helpers (`git`, `fixture`, `ok_git`, `FlakyWatcher`, `start`, `wait_for`, `ready`) are reused; two new ones are added:

```rust
    /// `main` -> `feat`: a.txt edited and committed, then edited again; old.txt renamed to
    /// new.txt and committed; d.txt added and committed; u.txt untracked.
    fn branch_fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        git(p, &["config", "user.email", "t@example.com"]);
        git(p, &["config", "user.name", "t"]);
        std::fs::write(p.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(p.join("b.txt"), "b\n").unwrap();
        std::fs::write(p.join("old.txt"), "same\ncontent\nhere\n").unwrap();
        git(p, &["add", "-A"]);
        git(p, &["commit", "-q", "-m", "init"]);
        git(p, &["switch", "-q", "-c", "feat"]);
        std::fs::write(p.join("a.txt"), "one\nTWO\n").unwrap();
        std::fs::write(p.join("d.txt"), "d\n").unwrap();
        git(p, &["mv", "old.txt", "new.txt"]);
        git(p, &["add", "-A"]);
        git(p, &["commit", "-q", "-m", "feat"]);
        std::fs::write(p.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        std::fs::write(p.join("u.txt"), "u\n").unwrap();
        dir
    }

    fn start_with(
        dir: &std::path::Path,
        scope: Scope,
        state_dir: Option<std::path::PathBuf>,
        gate: Option<Arc<Semaphore>>,
    ) -> (tokio::runtime::Runtime, EngineHandle) {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let handle = spawn(
            rt.handle(),
            SessionConfig {
                path: dir.to_path_buf(),
                poll_interval: Duration::from_millis(50),
                watcher: Arc::new(FlakyWatcher {
                    allow: Arc::new(AtomicBool::new(true)),
                }),
                git_check: ok_git(),
                diff_delay: None,
                diff_gate: gate,
                scope,
                base_ref: None,
                state_dir,
            },
        );
        (rt, handle)
    }

    fn git_out(dir: &std::path::Path, args: &[&str]) -> String {
        let out = Proc::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn rows_of(s: &Snapshot) -> Vec<(String, bool)> {
        s.files
            .iter()
            .map(|f| (f.path.clone(), matches!(f.status, ChangedFileStatus::Untracked)))
            .collect()
    }

    fn select_and_wait(h: &EngineHandle, path: &str, untracked: bool) -> Arc<Snapshot> {
        h.commands
            .send(Command::Select(FileKey {
                path: path.into(),
                staged: false,
                untracked,
            }))
            .unwrap();
        wait_for(h, &format!("diff of {path}"), |s| {
            ready(s).map(|d| d.key.path == path && d.key.untracked == untracked).unwrap_or(false)
        })
    }

    #[test]
    fn branch_scope_lists_every_change_the_branch_carries_once() {
        let dir = branch_fixture();
        let (_rt, h) = start_with(dir.path(), Scope::Worktree, None, None);
        let s = wait_for(&h, "worktree rows", |s| ready(s).is_some());
        assert_eq!(rows_of(&s), [("a.txt".to_string(), false), ("u.txt".to_string(), true)]);
        assert_eq!(s.base.as_ref().map(|b| b.requested.as_str()), Some("refs/heads/main"));
        assert_eq!(s.base.as_ref().map(|b| b.source), Some(BaseSource::Default));
        assert_eq!(s.default_base.as_deref(), Some("refs/heads/main"));
        assert!(s.rename_sources.is_empty());

        h.commands.send(Command::SetScope(Scope::Branch)).unwrap();
        let s = wait_for(&h, "branch rows", |s| s.scope == Scope::Branch && ready(s).is_some());
        assert_eq!(
            rows_of(&s),
            [
                ("a.txt".to_string(), false),
                ("d.txt".to_string(), false),
                ("new.txt".to_string(), false),
                ("u.txt".to_string(), true)
            ]
        );
        let merge_base = git_out(dir.path(), &["merge-base", "HEAD", "refs/heads/main"]).trim().to_string();
        assert_eq!(s.base.as_ref().unwrap().merge_base.as_deref(), Some(merge_base.as_str()));
        assert_eq!(s.rename_sources.get("new.txt").map(String::as_str), Some("old.txt"));
        let renamed = s.files.iter().find(|f| f.path == "new.txt").unwrap();
        assert!(matches!(renamed.status, ChangedFileStatus::Renamed));
        assert_eq!((renamed.insertions, renamed.deletions), (Some(0), Some(0)));

        // The committed-then-edited file shows both changes in one diff, equal to git's own.
        let a = ready(&s).unwrap();
        assert_eq!(a.key.path, "a.txt");
        assert_eq!(a.comparison, Comparison::Branch { merge_base: merge_base.clone() });
        let expected = git_out(
            dir.path(),
            &["diff", &merge_base, "--no-color", "--no-ext-diff", "--src-prefix=a/", "--dst-prefix=b/", "--", "a.txt"],
        );
        assert_eq!(a.raw_diff, expected);
        let lines: Vec<&str> = a.file_diff.hunks.iter().flat_map(|h| h.lines.iter().map(|l| l.content.as_str())).collect();
        assert!(lines.contains(&"TWO") && lines.contains(&"three"), "{lines:?}");

        let s = select_and_wait(&h, "new.txt", false);
        let d = ready(&s).unwrap();
        assert_eq!(d.file_diff.old_path.as_deref(), Some("old.txt"));
        let expected = git_out(
            dir.path(),
            &["diff", &merge_base, "--no-color", "--no-ext-diff", "--src-prefix=a/", "--dst-prefix=b/", "-M", "--", "old.txt", "new.txt"],
        );
        assert_eq!(d.raw_diff, expected);
        let s = select_and_wait(&h, "u.txt", true);
        let lines: Vec<&str> = ready(&s).unwrap().file_diff.hunks.iter().flat_map(|h| h.lines.iter().map(|l| l.content.as_str())).collect();
        assert_eq!(lines, ["u"]);

        // After a commit the worktree is clean and the branch rows are the same four paths.
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "more"]);
        let s = wait_for(&h, "u.txt became an added row", |s| {
            s.scope == Scope::Branch && rows_of(s).iter().any(|(p, u)| p == "u.txt" && !u)
        });
        assert_eq!(
            rows_of(&s),
            [
                ("a.txt".to_string(), false),
                ("d.txt".to_string(), false),
                ("new.txt".to_string(), false),
                ("u.txt".to_string(), false)
            ]
        );
        h.commands.send(Command::SetScope(Scope::Worktree)).unwrap();
        let s = wait_for(&h, "clean worktree", |s| s.scope == Scope::Worktree && s.files.is_empty());
        assert!(matches!(s.diff, DiffState::Idle));
    }

    #[test]
    fn a_path_deleted_on_the_branch_and_recreated_untracked_is_two_rows() {
        let dir = branch_fixture();
        git(dir.path(), &["rm", "-q", "b.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "drop b"]);
        std::fs::write(dir.path().join("b.txt"), "again\n").unwrap();
        let (_rt, h) = start_with(dir.path(), Scope::Branch, None, None);
        let s = wait_for(&h, "branch rows", |s| s.scope == Scope::Branch && ready(s).is_some());
        let b: Vec<(String, bool)> = rows_of(&s).into_iter().filter(|(p, _)| p == "b.txt").collect();
        assert_eq!(b, [("b.txt".to_string(), false), ("b.txt".to_string(), true)]);
        let deleted = s.files.iter().find(|f| f.path == "b.txt" && !matches!(f.status, ChangedFileStatus::Untracked)).unwrap();
        assert!(matches!(deleted.status, ChangedFileStatus::Deleted));
        let s = select_and_wait(&h, "b.txt", false);
        let lines: Vec<&str> = ready(&s).unwrap().file_diff.hunks.iter().flat_map(|h| h.lines.iter().map(|l| l.content.as_str())).collect();
        assert_eq!(lines, ["b"], "the deletion diff ignores the untracked content");
        let s = select_and_wait(&h, "b.txt", true);
        let lines: Vec<&str> = ready(&s).unwrap().file_diff.hunks.iter().flat_map(|h| h.lines.iter().map(|l| l.content.as_str())).collect();
        assert_eq!(lines, ["again"]);
    }

    #[test]
    fn set_base_validates_loads_persists_and_switches_scope() {
        let dir = branch_fixture();
        git(dir.path(), &["branch", "other", "main"]);
        let state = tempfile::tempdir().unwrap();
        let (_rt, h) = start_with(dir.path(), Scope::Worktree, Some(state.path().to_path_buf()), None);
        wait_for(&h, "first", |s| ready(s).is_some());

        h.commands.send(Command::SetBase(Some("nope".into()))).unwrap();
        let s = wait_for(&h, "rejected pick", |s| s.pick_seq == 1);
        assert_eq!(s.pick_error.as_deref(), Some("not a commit: nope"));
        assert_eq!(s.scope, Scope::Worktree);
        assert!(!state.path().join("bases.json").exists());

        h.commands.send(Command::SetBase(Some("refs/heads/other".into()))).unwrap();
        let s = wait_for(&h, "picked", |s| s.pick_seq == 2);
        assert!(s.pick_error.is_none());
        assert_eq!(s.scope, Scope::Branch);
        let base = s.base.as_ref().unwrap();
        assert_eq!((base.requested.as_str(), base.source), ("refs/heads/other", BaseSource::Picked));
        assert!(base.merge_base.is_some());
        assert_eq!(s.files.len(), 4);
        let toplevel = dir.path().canonicalize().unwrap().to_string_lossy().into_owned();
        let picks: std::collections::BTreeMap<String, String> =
            serde_json::from_str(&std::fs::read_to_string(state.path().join("bases.json")).unwrap()).unwrap();
        assert_eq!(picks.get(&toplevel).map(String::as_str), Some("refs/heads/other"));

        // A new session on the same worktree starts from the remembered pick.
        drop(h);
        let (_rt2, h) = start_with(dir.path(), Scope::Worktree, Some(state.path().to_path_buf()), None);
        let s = wait_for(&h, "remembered", |s| s.base.is_some());
        assert_eq!(s.base.as_ref().unwrap().requested, "refs/heads/other");
        assert_eq!(s.default_base.as_deref(), Some("refs/heads/main"));

        h.commands.send(Command::SetBase(None)).unwrap();
        let s = wait_for(&h, "reset", |s| s.pick_seq == 1);
        assert!(s.pick_error.is_none());
        assert_eq!(s.scope, Scope::Branch, "picking the reset row keeps branch scope");
        assert_eq!(s.base.as_ref().map(|b| (b.requested.as_str(), b.source)), Some(("refs/heads/main", BaseSource::Default)));
        let picks: std::collections::BTreeMap<String, String> =
            serde_json::from_str(&std::fs::read_to_string(state.path().join("bases.json")).unwrap()).unwrap();
        assert!(!picks.contains_key(&toplevel));
    }

    #[test]
    fn a_pick_with_unrelated_history_is_reported_and_not_persisted() {
        let dir = branch_fixture();
        git(dir.path(), &["checkout", "-q", "--orphan", "lonely"]);
        git(dir.path(), &["commit", "-q", "--allow-empty", "-m", "lonely"]);
        git(dir.path(), &["switch", "-q", "feat"]);
        let state = tempfile::tempdir().unwrap();
        let (_rt, h) = start_with(dir.path(), Scope::Worktree, Some(state.path().to_path_buf()), None);
        wait_for(&h, "first", |s| ready(s).is_some());
        h.commands.send(Command::SetBase(Some("refs/heads/lonely".into()))).unwrap();
        let s = wait_for(&h, "answered", |s| s.pick_seq == 1);
        assert!(s.pick_error.as_deref().unwrap().starts_with("no merge-base with "), "{:?}", s.pick_error);
        assert_eq!(s.scope, Scope::Worktree);
        assert_eq!(s.base.as_ref().unwrap().requested, "refs/heads/main");
        assert!(!state.path().join("bases.json").exists());
    }

    #[test]
    fn a_pick_that_cannot_be_remembered_is_kept_for_the_session() {
        let dir = branch_fixture();
        git(dir.path(), &["branch", "other", "main"]);
        let (_rt, h) = start_with(dir.path(), Scope::Worktree, None, None);
        wait_for(&h, "first", |s| ready(s).is_some());
        h.commands.send(Command::SetBase(Some("refs/heads/other".into()))).unwrap();
        let s = wait_for(&h, "picked", |s| s.pick_seq == 1);
        assert!(s.pick_error.is_none());
        assert!(s.base_error.as_deref().unwrap().starts_with("pick not remembered: "), "{:?}", s.base_error);
        h.commands.send(Command::Refresh).unwrap();
        wait_for(&h, "refreshed", |s| !s.refreshing && s.revision > 1);
        h.commands.send(Command::SetScope(Scope::Worktree)).unwrap();
        let s = wait_for(&h, "worktree", |s| s.scope == Scope::Worktree);
        assert_eq!(s.base.as_ref().unwrap().requested, "refs/heads/other", "r and scope switches keep the override");
    }

    #[test]
    fn a_stale_diff_from_the_previous_comparison_is_discarded_and_the_key_reloaded() {
        let dir = branch_fixture();
        let gate = Arc::new(Semaphore::new(0));
        let (_rt, h) = start_with(dir.path(), Scope::Worktree, None, Some(gate.clone()));
        gate.add_permits(1);
        let s = wait_for(&h, "worktree diff", |s| ready(s).is_some());
        assert_eq!(ready(&s).unwrap().comparison, Comparison::Worktree);
        // D1: a worktree diff of a.txt blocked on the gate, requested by a refresh.
        h.commands.send(Command::Refresh).unwrap();
        wait_for(&h, "refresh in flight", |s| s.refreshing);
        // The scope switch changes the comparison while D1 is still blocked.
        h.commands.send(Command::SetScope(Scope::Branch)).unwrap();
        let s = wait_for(&h, "branch rows, diff loading", |s| s.scope == Scope::Branch && matches!(s.diff, DiffState::Loading));
        assert_eq!(s.selected.as_ref().map(|k| k.path.as_str()), Some("a.txt"), "the path survives the switch");
        gate.add_permits(1); // releases D1, whose result must be discarded
        gate.add_permits(1); // releases the branch diff of a.txt
        let s = wait_for(&h, "branch diff", |s| ready(s).is_some());
        assert!(matches!(ready(&s).unwrap().comparison, Comparison::Branch { .. }));
        assert_eq!(ready(&s).unwrap().key.path, "a.txt");
        // No snapshot after the switch carries a worktree-comparison diff.
        std::thread::sleep(Duration::from_millis(200));
        while let Ok(s) = h.snapshots.try_recv() {
            assert!(ready(&s).map(|d| d.comparison != Comparison::Worktree).unwrap_or(true));
        }
    }

    #[test]
    fn load_refs_answers_on_the_snapshot_and_no_base_falls_back_to_worktree_with_the_notice() {
        let dir = branch_fixture();
        let (_rt, h) = start_with(dir.path(), Scope::Worktree, None, None);
        wait_for(&h, "first", |s| ready(s).is_some());
        h.commands.send(Command::LoadRefs).unwrap();
        let s = wait_for(&h, "refs", |s| s.refs.is_some());
        let refs = s.refs.as_ref().unwrap();
        assert!(refs.contains(&"refs/heads/main".to_string()) && refs.contains(&"refs/heads/feat".to_string()));
        assert!(!s.refs_overflow);

        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "dev"]);
        git(dir.path(), &["config", "user.email", "t@example.com"]);
        git(dir.path(), &["config", "user.name", "t"]);
        std::fs::write(dir.path().join("a.txt"), "a\n").unwrap();
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "init"]);
        std::fs::write(dir.path().join("a.txt"), "A\n").unwrap();
        let (_rt2, h) = start_with(dir.path(), Scope::Branch, None, None);
        let s = wait_for(&h, "fell back", |s| ready(s).is_some());
        assert_eq!(s.scope, Scope::Worktree);
        assert!(s.base.is_none());
        assert_eq!(s.base_error.as_deref(), Some(NO_BASE_NOTICE));
    }
```

Add `use super::{BaseSource, Comparison, Scope, NO_BASE_NOTICE};` and `use crate::git::ChangedFileStatus;` to the test module's imports if `use super::*` does not already bring them in (it does for items re-exported through `super`; `ChangedFileStatus` is imported at the top of `session.rs` already).

- [ ] **Step 6: Run them to verify they fail**

Run: `cargo test --lib engine::session`
Expected: the new tests fail (no `SetScope` handling: the wait for `scope == Branch` times out; `pick_seq` never moves).

- [ ] **Step 7: Rework the session loop**

Imports at the top of `src/engine/session.rs`:

```rust
use std::collections::BTreeMap;
use super::base::{self, ResolveInputs};
use super::{branch, gitver, Base, BaseSource, Command, Comparison, DiffState, FileKey, LoadedDiff, RepoState, Scope, Snapshot, NO_BASE_NOTICE};
```

Replace the `State` struct, `Done`, the whole `impl State` (both methods) with:

```rust
/// A queued scope or base change; applied by the next refresh and published with its rows.
#[derive(Debug, Clone)]
enum Change {
    Scope(Scope),
    Base(Option<String>),
}

/// How the refresh obtains the base: keep and re-verify, run the resolution steps, or verify a pick
/// (`Pick(None)` is the reset row: forget the remembered pick and resolve from step 2).
enum BaseJob {
    Keep(Option<Base>),
    Resolve,
    Pick(Option<String>),
}

/// One refresh: the Phase 1 status, then the rows of `scope` under the base `base` yields.
struct Job {
    cwd: String,
    with_head: bool,
    known_branch: Option<String>,
    scope: Scope,
    base: BaseJob,
    inputs: ResolveInputs,
    default_base: Option<String>,
    change: Option<Change>,
}

/// What a refresh loaded, published only as a whole.
struct Loaded {
    scope: Scope,
    base: Option<Base>,
    default_base: Option<String>,
    base_error: Option<String>,
    files: Vec<ChangedFile>,
    rename_sources: BTreeMap<String, String>,
    /// `Some` after a pick or reset: whether `bases.json` took it.
    persisted: Option<Result<(), String>>,
}

struct State {
    snapshot: Snapshot,
    branch: Option<String>,
    worktree: Option<String>,
    last_watcher_refresh: Instant,
    watcher: WatcherPhase,
    git_missing: bool,
    status_in_flight: bool,
    status_dirty: bool,
    status_dirty_head: bool,
    diff_generation: u64,
    diff_in_flight: Option<(u64, FileKey, Comparison)>,
    diff_dirty: bool,
    /// The scope the next refresh loads; equals the published scope except before the first rows.
    requested_scope: Scope,
    inputs: ResolveInputs,
    /// Run the resolution steps in the next refresh (start, `r`, and after a pick).
    resolve_pending: bool,
    change: Option<Change>,
    pick_seq: u64,
}

fn comparison_of(snapshot: &Snapshot) -> Comparison {
    match (snapshot.scope, &snapshot.base) {
        (Scope::Branch, Some(Base { merge_base: Some(m), .. })) => Comparison::Branch { merge_base: m.clone() },
        _ => Comparison::Worktree,
    }
}

enum Done {
    GitCheck(Result<gitver::GitVersion, gitver::GitCheckError>),
    Watcher(Result<(), String>),
    Status {
        response: Result<GitStatusResponse, String>,
        head: Option<(Option<String>, Option<String>)>,
        /// `None` when the status failed or the directory is not a repository.
        loaded: Option<Result<Loaded, String>>,
        change: Option<Change>,
    },
    Diff {
        generation: u64,
        key: FileKey,
        comparison: Comparison,
        result: Result<GetGitDiffResponse, String>,
    },
    Refs(Result<(Vec<String>, bool), String>),
}

/// The rows of one refresh. `Err` means the requested comparison could not be loaded; the
/// caller keeps what it published.
async fn load_rows(
    job: &Job,
    status: &GitStatusResponse,
    head: Option<&(Option<String>, Option<String>)>,
) -> Result<Loaded, String> {
    let toplevel = status.repo_root.as_str();
    let branch_changed = matches!(head, Some((Some(b), _)) if Some(b) != job.known_branch.as_ref());
    let mut base_error = None;
    let mut default_base = job.default_base.clone();
    let mut base = match &job.base {
        BaseJob::Keep(base) if !branch_changed => base.clone(),
        BaseJob::Keep(_) | BaseJob::Resolve => {
            let r = base::resolve(toplevel, &job.inputs).await;
            base_error = r.skipped.first().cloned();
            default_base = r.default;
            r.base
        }
        BaseJob::Pick(None) => {
            let inputs = ResolveInputs { session_pick: Some(None), ..job.inputs.clone() };
            let r = base::resolve(toplevel, &inputs).await;
            base_error = r.skipped.first().cloned();
            default_base = r.default;
            r.base
        }
        BaseJob::Pick(Some(text)) => {
            let commit = base::verify(toplevel, text).await?;
            let inputs = ResolveInputs { session_pick: Some(None), ..job.inputs.clone() };
            default_base = base::resolve(toplevel, &inputs).await.default;
            Some(Base { requested: text.clone(), commit, merge_base: None, source: BaseSource::Picked })
        }
    };
    if job.scope == Scope::Branch {
        if let Some(b) = base.as_mut() {
            // Re-verified every refresh in branch scope, so a moved ref changes rows and diffs together.
            if matches!(job.base, BaseJob::Keep(_)) {
                b.commit = base::verify(toplevel, &b.requested).await?;
            }
            b.merge_base = Some(base::merge_base(toplevel, &b.commit).await?);
        }
    }
    let scope = if base.is_some() { job.scope } else { Scope::Worktree };
    if job.scope == Scope::Branch && base.is_none() {
        base_error = base_error.or_else(|| Some(NO_BASE_NOTICE.to_string()));
    }
    let (files, rename_sources) = match (scope, &base) {
        (Scope::Branch, Some(b)) => {
            let rows = branch::rows(toplevel, b.merge_base.as_deref().unwrap_or_default(), status.files.clone()).await?;
            (rows.files, rows.rename_sources)
        }
        _ => (status.files.clone(), BTreeMap::new()),
    };
    let persisted = match (&job.base, &job.inputs.state_dir) {
        (BaseJob::Pick(pick), Some(dir)) => Some(base::save_pick(dir, toplevel, pick.as_deref()).map_err(|e| e.to_string())),
        (BaseJob::Pick(_), None) => Some(Err("no state directory".to_string())),
        _ => None,
    };
    Ok(Loaded { scope, base, default_base, base_error, files, rename_sources, persisted })
}

async fn run_job(job: Job) -> Done {
    let response = git::git_status_inner(job.cwd.clone()).await;
    let head = if job.with_head {
        let (branch, worktree) = tokio::join!(
            git::git_branch_inner(job.cwd.clone()),
            git::git_worktree_name_inner(job.cwd.clone())
        );
        Some((branch.ok(), worktree.ok().flatten()))
    } else {
        None
    };
    let loaded = match &response {
        Ok(status) if !status.repo_root.is_empty() => Some(load_rows(&job, status, head.as_ref()).await),
        _ => None,
    };
    Done::Status { response, head, loaded, change: job.change }
}

impl State {
    /// One refresh: status, head when asked, and the rows of the current or requested comparison.
    fn request_status(
        &mut self,
        cwd: &str,
        with_head: bool,
        results: &UnboundedSender<Done>,
        refreshes: &AtomicUsize,
    ) {
        if self.status_in_flight {
            self.status_dirty = true;
            self.status_dirty_head |= with_head;
            return;
        }
        self.status_in_flight = true;
        refreshes.fetch_add(1, Ordering::SeqCst);
        let change = self.change.take();
        let scope = match &change {
            Some(Change::Scope(scope)) => *scope,
            Some(Change::Base(_)) => Scope::Branch,
            None => self.requested_scope,
        };
        // Resolution runs at start, on `r`, for a pick, and when `b` is pressed with nothing resolved yet.
        let base = match (&change, std::mem::take(&mut self.resolve_pending), &self.snapshot.base) {
            (Some(Change::Base(pick)), _, _) => BaseJob::Pick(pick.clone()),
            (_, true, _) | (Some(Change::Scope(Scope::Branch)), _, None) => BaseJob::Resolve,
            (_, _, base) => BaseJob::Keep(base.clone()),
        };
        let job = Job {
            cwd: cwd.to_string(),
            with_head,
            known_branch: self.branch.clone(),
            scope,
            base,
            inputs: self.inputs.clone(),
            default_base: self.snapshot.default_base.clone(),
            change,
        };
        let results = results.clone();
        tokio::spawn(async move {
            let _ = results.send(run_job(job).await);
        });
    }

    fn request_diff(
        &mut self,
        key: FileKey,
        cwd: &str,
        delay: Option<Duration>,
        gate: Option<Arc<Semaphore>>,
        results: &UnboundedSender<Done>,
    ) {
        let comparison = comparison_of(&self.snapshot);
        if matches!(&self.diff_in_flight, Some((_, pending, under)) if pending == &key && under == &comparison) {
            self.diff_dirty = true;
            return;
        }
        self.diff_generation += 1;
        let generation = self.diff_generation;
        self.diff_in_flight = Some((generation, key.clone(), comparison.clone()));
        self.diff_dirty = false;
        let toplevel = match &self.snapshot.repo {
            RepoState::Repo { toplevel, .. } => toplevel.clone(),
            _ => cwd.to_string(),
        };
        let old = self.snapshot.rename_sources.get(&key.path).cloned();
        let cwd = cwd.to_string();
        let results = results.clone();
        tokio::spawn(async move {
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
                Comparison::Branch { .. } => branch::untracked_diff(cwd, key.path.clone()).await,
                Comparison::Worktree => {
                    git::get_git_diff_inner(cwd, key.path.clone(), key.staged, Some(key.untracked)).await
                }
            };
            let _ = results.send(Done::Diff { generation, key, comparison, result });
        });
    }
}
```

In `run`, initialise the new fields: `requested_scope: config.scope, inputs: ResolveInputs { session_pick: None, config: config.base_ref.clone(), state_dir: config.state_dir.clone() }, resolve_pending: true, change: None, pick_seq: 0` (and `diff_in_flight: None` as before).

In the `commands.recv()` arm, `Command::Refresh` additionally sets `state.resolve_pending = true;` before requesting the status. Add three arms before the `selection =>` arm:

```rust
                    Command::SetScope(scope) => {
                        if scope != state.snapshot.scope || scope != state.requested_scope {
                            state.change = Some(Change::Scope(scope));
                            let mut next = state.snapshot.clone();
                            next.refreshing = true;
                            publish(&mut state, next, &snapshots);
                            state.request_status(&cwd, false, &results_tx, &refreshes);
                        }
                    }
                    Command::SetBase(pick) => {
                        state.change = Some(Change::Base(pick));
                        let mut next = state.snapshot.clone();
                        next.refreshing = true;
                        publish(&mut state, next, &snapshots);
                        state.request_status(&cwd, false, &results_tx, &refreshes);
                    }
                    Command::LoadRefs => {
                        let toplevel = match &state.snapshot.repo {
                            RepoState::Repo { toplevel, .. } => Some(toplevel.clone()),
                            _ => None,
                        };
                        let results = results_tx.clone();
                        tokio::spawn(async move {
                            let refs = match toplevel {
                                Some(toplevel) => base::list_refs(&toplevel).await,
                                None => Ok((Vec::new(), false)),
                            };
                            let _ = results.send(Done::Refs(refs));
                        });
                    }
```

Replace the `Done::Status` arm with:

```rust
                    Done::Status { response, head, loaded, change } => {
                        state.status_in_flight = false;
                        let before = comparison_of(&state.snapshot);
                        let mut succeeded = false;
                        let mut change_error = None;
                        match response {
                            Err(e) => {
                                change_error = Some(e.clone());
                                next.status_error = Some(e);
                            }
                            Ok(response) => {
                                if let Some((branch, worktree)) = head {
                                    state.branch = branch;
                                    state.worktree = worktree;
                                }
                                next.repo = if response.repo_root.is_empty() {
                                    RepoState::NotARepo { cwd: cwd.clone() }
                                } else {
                                    RepoState::Repo {
                                        toplevel: response.repo_root.clone(),
                                        branch: state.branch.clone(),
                                        worktree: state.worktree.clone(),
                                    }
                                };
                                match loaded {
                                    Some(Err(e)) => match &change {
                                        // A failed pick answers the picker; a failed switch is a one-time notice;
                                        // a failed refresh in branch scope is a status error with the rows kept.
                                        Some(Change::Base(_)) => change_error = Some(e),
                                        Some(Change::Scope(_)) => next.base_error = Some(e),
                                        None => next.status_error = Some(e),
                                    },
                                    other => {
                                        next.status_error = None;
                                        let loaded = match other {
                                            Some(Ok(loaded)) => loaded,
                                            _ => Loaded {
                                                scope: Scope::Worktree,
                                                base: None,
                                                default_base: None,
                                                base_error: None,
                                                files: response.files,
                                                rename_sources: BTreeMap::new(),
                                                persisted: None,
                                            },
                                        };
                                        let switched = loaded.scope != next.scope;
                                        let previous = next.selected.take();
                                        let old_index = previous.as_ref().and_then(|key| next.files.iter().position(|f| &key_of(f) == key));
                                        next.scope = loaded.scope;
                                        next.base = loaded.base;
                                        next.base_error = loaded.base_error;
                                        if loaded.default_base.is_some() {
                                            next.default_base = loaded.default_base;
                                        }
                                        next.files = loaded.files;
                                        next.rename_sources = Arc::new(loaded.rename_sources);
                                        next.selected = if switched {
                                            // The path survives a scope switch; the unstaged row wins when both exist.
                                            previous.as_ref().and_then(|key| {
                                                next.files.iter().filter(|f| f.path == key.path).min_by_key(|f| f.staged).map(key_of)
                                            })
                                        } else {
                                            previous.clone().filter(|key| next.files.iter().any(|f| &key_of(f) == key))
                                                .or_else(|| old_index.and_then(|i| next.files.get(i.min(next.files.len().saturating_sub(1))).map(key_of)))
                                        }
                                        .or_else(|| next.files.first().map(key_of));
                                        if let Some(Change::Base(pick)) = &change {
                                            match loaded.persisted {
                                                Some(Ok(())) => state.inputs.session_pick = None,
                                                Some(Err(e)) => {
                                                    state.inputs.session_pick = Some(pick.clone());
                                                    next.base_error = Some(format!("pick not remembered: {e}"));
                                                }
                                                None => {}
                                            }
                                        }
                                        succeeded = true;
                                    }
                                }
                            }
                        }
                        state.requested_scope = next.scope;
                        let comparison = comparison_of(&next);
                        if comparison != before {
                            // Results computed under the previous comparison are stale from here on.
                            state.diff_generation += 1;
                            state.diff_in_flight = None;
                        }
                        let same = matches!(&next.diff, DiffState::Ready(d) if Some(&d.key) == next.selected.as_ref() && d.comparison == comparison);
                        if succeeded && !same {
                            next.diff = if next.selected.is_some() { DiffState::Loading } else { DiffState::Idle };
                        }
                        if matches!(change, Some(Change::Base(_))) {
                            state.pick_seq += 1;
                            next.pick_seq = state.pick_seq;
                            next.pick_error = change_error;
                        }
                        let selected = next.selected.clone().filter(|_| succeeded);
                        if selected.is_none() {
                            next.refreshing = false;
                        }
                        publish(&mut state, next, &snapshots);
                        if let Some(key) = selected {
                            state.request_diff(key, &cwd, config.diff_delay, config.diff_gate.clone(), &results_tx);
                        }
                        if state.status_dirty {
                            state.status_dirty = false;
                            let with_head = std::mem::take(&mut state.status_dirty_head);
                            state.request_status(&cwd, with_head, &results_tx, &refreshes);
                        }
                    }
```

Replace the `Done::Diff` arm's head and build call:

```rust
                    Done::Diff { generation, key, comparison, result } => {
                        if generation != state.diff_generation || comparison != comparison_of(&state.snapshot) {
                            continue;
                        }
                        state.diff_in_flight = None;
                        if Some(&key) == next.selected.as_ref() {
                            match result {
                                Ok(response) => {
                                    let unchanged = matches!(&next.diff, DiffState::Ready(d) if d.key == key && d.comparison == comparison && d.raw_diff == response.raw_diff);
                                    if !unchanged {
                                        next.diff = DiffState::Ready(Arc::new(LoadedDiff::build(key, comparison, response)));
                                    }
                                }
                                Err(e) => next.diff = DiffState::Failed(e),
                            }
```

(the rest of the arm is unchanged), and add:

```rust
                    Done::Refs(result) => {
                        let (refs, overflow) = result.unwrap_or_default();
                        next.refs = Some(Arc::new(refs));
                        next.refs_overflow = overflow;
                        publish(&mut state, next, &snapshots);
                    }
```

`Change` must derive `Clone` only if the compiler asks for it in `match &change` (it does not); keep `#[derive(Debug, Clone)]` anyway, it is cheap. If Task 2 added `#[allow(dead_code)]` on `pub mod base;`, remove it now.

- [ ] **Step 8: Run the session tests**

Run: `cargo test --lib engine::session -- --test-threads=1` three times, then `cargo test`.
Expected: PASS every time. `a_stale_diff_from_the_previous_comparison_is_discarded_and_the_key_reloaded` must pass without sleeps other than its final drain.

- [ ] **Step 9: Verify**

Run: `cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked && cargo check --locked --no-default-features && scripts/port-check.sh`
Expected: all pass.

- [ ] **Step 10: Commit (orchestrator)**

```bash
git add -A
git commit -m "feat(engine): branch scope rows and diffs against a pinned merge-base"
```

---

### Task 4: The `b` key, the scope chip, config keys and shell wiring

Implements spec 7.4 (the `b` key, the chip, the hidden `STAGED`/`UNSTAGED` label, the drop order), 7.7 (Phase 2 keys stay unbound in both scopes) and 7.8 (the two config keys, the notice). Read those before starting. Tests 4 and 5 of 7.9, the parts that do not involve the picker, live here.

Drop order: 7.4 gives the scope chip drop order 2 and moves the view chip to 3. In `toolbar()` the highest order drops first, so the view chip goes before the scope chip when the toolbar narrows (the scope chip says what the rows are; the view mode is one `t` away). The sentence in 7.4 that reads "dropped before the view chip" describes the chip's position, left of the view chip; the numbers are what the code and the test follow.

**Files:**
- Modify: `src/tui/keys.rs` (`KeyAction::ToggleScope`, binding `b`, count 22)
- Modify: `src/tui/view.rs` (`Action::ToggleScope`, chip, drop orders, hidden label)
- Modify: `src/tui/input.rs` (`ToggleScope`)
- Modify: `src/tui/state.rs` (`seen_base_error`, `observe`)
- Modify: `src/tui/config.rs` (`Config.scope`, `Config.base`, parsing)
- Modify: `src/tui/shell.rs` (config -> `SessionConfig`, `observe` in the loop, tests' `Config` literals)

**Interfaces:**
- Consumes: `Snapshot { scope, base, base_error }`, `Scope`, `NO_BASE_NOTICE`, `Command::SetScope` (Tasks 1, 3); `base::check_text` (Task 2).
- Produces: `KeyAction::ToggleScope` (`b`, label `switch scope`), `Action::ToggleScope`, `ViewState::observe(&mut self, &Snapshot)`, `Config { scope: Scope, base: Option<String> }`.

- [ ] **Step 1: Write the failing tests**

`src/tui/keys.rs`, in the existing test that asserts `KEYS.len()`: change `21` to `22` in both assertions and add to `modifiers_preserve_uppercase_and_keep_unbound_combinations_inert`:

```rust
        assert_eq!(
            lookup(&KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE)),
            Some(KeyAction::ToggleScope)
        );
```

`src/tui/view.rs` tests (use the module's existing helpers; `snapshot` comes from `crate::tui::state::tests::snapshot`):

```rust
    #[test]
    fn the_scope_chip_names_the_base_and_is_dim_without_one() {
        use crate::engine::{Base, BaseSource, Scope};
        let mut snap = snapshot("a.rs", "r1", &[(1, "+")]);
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        st.resize(120, 20);
        st.reconcile(&snap);
        // No base: the chip reads `worktree`, is dim and has no hit region.
        let r = render(&snap, &st, 120, 24);
        let bar = &r.lines[0];
        let chip = bar.iter().find(|s| s.text == " worktree ").or_else(|| bar.iter().find(|s| s.text == "worktree"));
        assert_eq!(chip.map(|s| s.style.role), Some(Role::Label));
        assert!(!r.hits.iter().any(|h| h.action == Action::ToggleScope));
        assert!(r.plain()[0].contains("UNSTAGED"));
        // A base: clickable, and the hover hint names the key.
        snap.base = Some(Base {
            requested: "refs/heads/main".into(),
            commit: "0".repeat(40),
            merge_base: None,
            source: BaseSource::Default,
        });
        let r = render(&snap, &st, 120, 24);
        let hit = r.hits.iter().find(|h| h.action == Action::ToggleScope).expect("chip hit");
        assert_eq!(hit.y, 0);
        st.hover = Some((hit.x0, 0));
        let r = render(&snap, &st, 120, 24);
        assert_eq!(r.plain().last().unwrap(), "switch scope · b");
        // Branch scope: `vs main`, and the staged label is gone.
        snap.scope = Scope::Branch;
        snap.base.as_mut().unwrap().merge_base = Some("1".repeat(40));
        st.hover = None;
        let r = render(&snap, &st, 120, 24);
        let top = &r.plain()[0];
        assert!(top.contains("vs main"), "{top}");
        assert!(!top.contains("UNSTAGED") && !top.contains("STAGED"), "{top}");
        snap.base.as_mut().unwrap().requested = "refs/remotes/origin/a-very-long-branch-name".into();
        let r = render(&snap, &st, 120, 24);
        assert!(r.plain()[0].contains("vs origin/a-very-l…"), "{}", r.plain()[0]); // 15 cells + the ellipsis
    }

    #[test]
    fn the_view_chip_drops_before_the_scope_chip() {
        use crate::engine::{Base, BaseSource};
        let mut snap = snapshot("a.rs", "r1", &[(1, "+")]);
        snap.base = Some(Base {
            requested: "refs/heads/main".into(),
            commit: "0".repeat(40),
            merge_base: None,
            source: BaseSource::Default,
        });
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        st.resize(120, 20);
        st.reconcile(&snap);
        let mut width = 120u16;
        let mut saw_scope_without_view = false;
        while width >= 40 {
            let top = render(&snap, &st, width, 24).plain()[0].clone();
            let has_scope = top.contains("worktree");
            let has_view = top.contains("unified");
            assert!(!(has_view && !has_scope), "the view chip outlived the scope chip at {width}: {top}");
            saw_scope_without_view |= has_scope && !has_view;
            width -= 4;
        }
        assert!(saw_scope_without_view);
    }
```

`src/tui/input.rs` tests:

```rust
    #[test]
    fn b_switches_scope_or_says_there_is_no_base() {
        use crate::engine::{Base, BaseSource, Scope, NO_BASE_NOTICE};
        let (mut snap, mut st) = setup(&[(1, "+")]);
        assert_eq!(handle_key(&mut st, &snap, key("b"), 120), Outcome::Redraw);
        assert_eq!(st.notice.as_deref(), Some(NO_BASE_NOTICE));
        snap.base = Some(Base {
            requested: "refs/heads/main".into(),
            commit: "0".repeat(40),
            merge_base: None,
            source: BaseSource::Default,
        });
        assert_eq!(
            handle_key(&mut st, &snap, key("b"), 120),
            Outcome::Engine(Command::SetScope(Scope::Branch))
        );
        assert!(st.notice.is_none(), "any handled key clears the notice");
        snap.scope = Scope::Branch;
        assert_eq!(
            handle_key(&mut st, &snap, key("b"), 120),
            Outcome::Engine(Command::SetScope(Scope::Worktree))
        );
        for reserved in RESERVED {
            assert!(matches!(handle_key(&mut st, &snap, key(reserved), 120), Outcome::Inert), "{reserved} in branch scope");
        }
    }
```

`src/tui/state.rs` tests:

```rust
    #[test]
    fn a_new_base_error_becomes_a_notice_once() {
        let mut snap = snapshot("a.rs", "r1", &[(1, "+")]);
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        st.observe(&snap);
        assert!(st.notice.is_none());
        snap.base_error = Some("remembered pick: not a commit: gone\u{1b}".into());
        st.observe(&snap);
        assert_eq!(st.notice.as_deref(), Some("remembered pick: not a commit: gone\u{241b}"));
        st.notice = None;
        st.observe(&snap);
        assert!(st.notice.is_none(), "the same error is not repeated");
        snap.base_error = None;
        st.observe(&snap);
        assert!(st.notice.is_none());
    }
```

`src/tui/config.rs` tests:

```rust
    #[test]
    fn scope_and_base_keys_are_parsed_and_bad_values_cost_only_their_key() {
        use crate::engine::Scope;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[view]\nscope = \"branch\"\n[base]\nref = \"origin/main\"\n",
        )
        .unwrap();
        let (config, problems) = load(dir.path());
        assert_eq!((config.scope, config.base.as_deref()), (Scope::Branch, Some("origin/main")));
        assert!(problems.is_empty());
        std::fs::write(
            dir.path().join("config.toml"),
            "[view]\nscope = \"both\"\n[base]\nref = \"-x\"\n",
        )
        .unwrap();
        let (config, problems) = load(dir.path());
        assert_eq!((config.scope, config.base), (Scope::Worktree, None));
        assert_eq!(problems.len(), 2);
        assert!(problems[0].starts_with("view.scope: ") && problems[1].starts_with("base.ref: "), "{problems:?}");
        assert_eq!(load(tempfile::tempdir().unwrap().path()).0.scope, Scope::Worktree);
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib tui`
Expected: compile errors (`ToggleScope`, `observe`, `Config.scope` unknown).

- [ ] **Step 3: Keys, view and input**

`src/tui/keys.rs`: add `ToggleScope,` to `KeyAction` after `Refresh`, and insert this binding after the `r` binding:

```rust
    Binding {
        key: "b",
        label: "switch scope",
        action: KeyAction::ToggleScope,
        vimeflow: None,
    },
```

`src/tui/view.rs`:
- `Action` gains `ToggleScope`; `key_action` maps `Self::ToggleScope => KeyAction::ToggleScope`.
- Import `Scope` from `crate::engine`.
- In `toolbar_items`, build the vector explicitly so the staged label can be omitted:

```rust
    let scope_chip = match (snapshot.scope, &snapshot.base) {
        (Scope::Branch, Some(base)) => format!("vs {}", truncate(&sanitize(base.label()), 16)),
        _ => "worktree".to_string(),
    };
    let mut items: Vec<ToolbarItem> = vec![
        (
            vec![
                ("‹".into(), Some(Action::PrevFile)),
                (format!(" {name}{position}"), None),
                ("›".into(), Some(Action::NextFile)),
            ],
            0,
        ),
        (
            vec![
                (format!("{{}} {hunk_pos}/{hunk_total} "), None),
                ("↑".into(), Some(Action::PrevHunk)),
                (" ".into(), None),
                ("↓".into(), Some(Action::NextHunk)),
            ],
            1,
        ),
        (vec![(scope_chip, Some(Action::ToggleScope))], 2),
        (
            vec![(
                if state.mode == ViewMode::Split {
                    "split"
                } else {
                    "unified"
                }
                .into(),
                Some(Action::ToggleView),
            )],
            3,
        ),
    ];
    if snapshot.scope == Scope::Worktree {
        items.push((
            vec![(if staged { "STAGED" } else { "UNSTAGED" }.into(), None)],
            4,
        ));
    }
    items.push((vec![(stats, None)], 5));
    items.push((vec![("files".into(), Some(Action::ToggleFiles))], 6));
    items.push((vec![(busy.into(), Some(Action::Refresh))], 7));
    items
```

- In `toolbar`'s `enabled` match add `Action::ToggleScope => snapshot.base.is_some(),`. The disabled branch already draws the text with `Span::label` and no hit, which is the dim, unclickable chip the spec asks for.
- Existing toolbar tests that assert exact drop orders or the full toolbar text (`the_toolbar_shows_both_steppers_and_every_item_is_clickable`, `chip_widths_keep_file_steps_at_forty_and_drop_whole_items_in_order`, `a_narrow_toolbar_drops_items_from_the_right_and_keeps_the_steppers`, `disabled_chips_are_dim_without_reverse_or_hits`) need their expectations updated for the new chip (`worktree` sits between the hunk stepper and the view chip and is dim there because the test snapshots have no base); update the expected strings and counts, never the drop logic under test.

`src/tui/input.rs`, in `act`:

```rust
        ToggleScope => {
            return match snapshot.base {
                None => {
                    state.notice = Some(NO_BASE_NOTICE.into());
                    Outcome::Redraw
                }
                Some(_) => Outcome::Engine(Command::SetScope(snapshot.scope.other())),
            };
        }
```

Import `NO_BASE_NOTICE` from `crate::engine`. Note `apply_action` clears `state.notice` before `act` runs, so the notice set here survives and the next handled key clears it, as the test expects.

- [ ] **Step 4: `observe` on the view state**

`src/tui/state.rs`: add the field `seen_base_error: Option<String>,` (private, initialised to `None` in `new`) and:

```rust
    /// Called once per snapshot the shell receives, before the next frame.
    pub fn observe(&mut self, snapshot: &Snapshot) {
        if snapshot.base_error != self.seen_base_error {
            self.seen_base_error = snapshot.base_error.clone();
            if let Some(error) = &snapshot.base_error {
                self.notice = Some(crate::tui::sanitize::sanitize(error));
            }
        }
    }
```

(Task 5 extends it for the picker.) `src/tui/shell.rs`, in `run_terminal`'s loop:

```rust
        while let Ok(next) = handle.snapshots.try_recv() {
            state.observe(&next);
            snapshot = next;
            dirty = true;
        }
```

- [ ] **Step 5: Config keys and session wiring**

`src/tui/config.rs`: `Config` gains `pub scope: Scope, pub base: Option<String>` (`Default`: `Scope::Worktree`, `None`); import `crate::engine::Scope`. After the `input.mouse` match in `load`:

```rust
    match get("view", "scope").map(|v| v.as_str().map(str::to_owned)) {
        None => {}
        Some(Some(s)) if s == "worktree" => config.scope = Scope::Worktree,
        Some(Some(s)) if s == "branch" => config.scope = Scope::Branch,
        Some(other) => problems.push(format!(
            "view.scope: expected \"worktree\" or \"branch\", got {other:?}"
        )),
    }
    match get("base", "ref").map(|v| v.as_str().map(str::to_owned)) {
        None => {}
        Some(Some(s)) if crate::engine::base::check_text(&s).is_ok() => config.base = Some(s),
        Some(other) => problems.push(format!(
            "base.ref: expected a revision that does not start with '-', got {other:?}"
        )),
    }
```

The existing `one_bad_value_costs_one_key` test's problem count stays as it is (it does not set these keys). `src/tui/shell.rs`, in `run`:

```rust
    let mut session = SessionConfig::production(path.clone());
    session.scope = config.scope;
    session.base_ref = config.base.clone();
    session.state_dir = config::state_dir(lookup);
    let handle = engine::spawn(runtime.handle(), session);
```

Every `Config { mode, files, mouse }` literal in the shell tests gains `..Config::default()`.

- [ ] **Step 6: Run the TUI tests**

Run: `cargo test --lib tui`
Expected: PASS, including `every_key_on_the_sheet_does_something_and_reserved_keys_do_nothing` (`b` now shows the notice, which is a `Redraw`).

- [ ] **Step 7: Verify**

Run: `cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked && cargo check --locked --no-default-features && scripts/port-check.sh`
Expected: all pass.

- [ ] **Step 8: Commit (orchestrator)**

```bash
git add -A
git commit -m "feat(tui): scope key and chip, scope and base config keys"
```

---

### Task 5: The base picker (`B`)

Implements spec 7.4 (the picker: rows, filtering, markers, keys, mouse, errors, footers) and the picker rows of 7.8 (empty list, overflow). Read those before starting. The picker halves of tests 4 and 5 of 7.9 live here.

**Files:**
- Create: `src/tui/picker.rs`
- Modify: `src/tui/mod.rs` (add `pub mod picker;`)
- Modify: `src/tui/keys.rs` (`KeyAction::PickBase`, binding `B`, count 23)
- Modify: `src/tui/state.rs` (`picker: Option<Picker>`, `observe`)
- Modify: `src/tui/view.rs` (`Action::PickRow`, the overlay, `Hit::hovered`)
- Modify: `src/tui/input.rs` (`PickBase`, picker keys, picker mouse, popup `Esc` order)

**Interfaces:**
- Consumes: `Snapshot { refs, refs_overflow, default_base, base, pick_seq, pick_error, repo }`, `ref_label`, `BaseSource`, `Command::{SetBase, LoadRefs}`, `dialog::{Panel, Row, render, line_count}`.
- Produces:

```rust
// src/tui/picker.rs
pub const WIDTH: u16 = 60;
pub enum PickerRow { Reset(String), Typed(String), Ref { qualified: String, markers: Vec<&'static str> } }
impl PickerRow { pub fn submit(&self) -> Option<String> }          // None for the reset row
pub struct Picker { pub input: String, pub cursor: usize, pub offset: usize, pub error: Option<String>, pub pending: Option<u64>, pub done: bool }
impl Picker {
    pub fn new() -> Self;
    pub fn rows(&self, snapshot: &Snapshot) -> Vec<PickerRow>;
    pub fn visible(&self, height: u16) -> usize;                    // list rows that fit in a panel of `height` lines
    pub fn window(&self, visible: usize) -> usize;                  // first list row drawn, keeping the cursor inside
    pub fn panel(&self, snapshot: &Snapshot, height: u16) -> Panel;
    pub fn move_by(&mut self, delta: isize, len: usize, visible: usize) -> bool;
    pub fn retarget(&mut self, snapshot: &Snapshot);
    pub fn observe(&mut self, snapshot: &Snapshot);
}
// keys.rs: KeyAction::PickBase (`B`, label `compare against`)
// view.rs: Action::PickRow(usize)  (index into Picker::rows)
```

- [ ] **Step 1: Write the failing tests**

`src/tui/keys.rs`: the two `22` assertions become `23`; add `assert_eq!(lookup(&KeyEvent::new(KeyCode::Char('B'), KeyModifiers::SHIFT)), Some(KeyAction::PickBase));` next to the `E` assertion.

Create `src/tui/picker.rs` with only this test module for now:

```rust
//! The base picker (spec 7.4): one input line that filters the ref list and takes free text.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Base, BaseSource, RepoState, Scope, Snapshot};
    use std::sync::Arc;

    fn snap(refs: &[&str], base: Option<(&str, BaseSource)>) -> Snapshot {
        let mut s = Snapshot::empty("/r");
        s.revision = 1;
        s.repo = RepoState::Repo {
            toplevel: "/r".into(),
            branch: Some("main".into()),
            worktree: None,
        };
        s.refs = Some(Arc::new(refs.iter().map(|r| r.to_string()).collect()));
        s.default_base = Some("refs/heads/main".into());
        s.base = base.map(|(requested, source)| Base {
            requested: requested.into(),
            commit: "0".repeat(40),
            merge_base: None,
            source,
        });
        s.scope = Scope::Worktree;
        s
    }

    const REFS: [&str; 5] = [
        "refs/heads/feat",
        "refs/heads/main",
        "refs/remotes/origin/main",
        "refs/tags/main",
        "refs/heads/maint-2.1",
    ];

    fn labels(rows: &[PickerRow]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                PickerRow::Reset(t) => t.clone(),
                PickerRow::Typed(t) => format!("use \"{t}\""),
                PickerRow::Ref { qualified, markers } => {
                    let mut s = crate::engine::ref_label(qualified).to_string();
                    if !markers.is_empty() {
                        s = format!("{s} [{}]", markers.join(" · "));
                    }
                    s
                }
            })
            .collect()
    }

    #[test]
    fn rows_start_with_reset_then_the_current_base_then_matches_with_markers() {
        let s = snap(&REFS, Some(("refs/tags/main", BaseSource::Picked)));
        let p = Picker::new();
        assert_eq!(
            labels(&p.rows(&s)),
            [
                "default (main)",
                "main [picked · tag]",
                "feat",
                "main [current]",
                "origin/main",
                "maint-2.1"
            ]
        );
        let mut p = Picker::new();
        p.input = "main".into();
        assert_eq!(
            labels(&p.rows(&s)),
            ["default (main)", "main [picked · tag]", "main [current]", "origin/main", "maint-2.1"],
            "`main` is exactly a listed label, so no typed row"
        );
        p.input = "MAIN".into();
        assert_eq!(
            labels(&p.rows(&s)),
            ["default (main)", "use \"MAIN\"", "main [picked · tag]", "main [current]", "origin/main", "maint-2.1"],
            "filtering is case-insensitive; the exact-label check is not"
        );
        p.input = "HEAD~2".into();
        assert_eq!(labels(&p.rows(&s)), ["default (main)", "use \"HEAD~2\""]);
        assert_eq!(p.rows(&s)[1].submit().as_deref(), Some("HEAD~2"));
        assert_eq!(p.rows(&s)[0].submit(), None);
        let s = snap(&REFS, Some(("feat", BaseSource::Config)));
        assert_eq!(labels(&Picker::new().rows(&s))[1], "feat");
        let s = snap(&REFS, Some(("refs/heads/feat", BaseSource::Config)));
        assert_eq!(labels(&Picker::new().rows(&s))[1], "feat [config]");
        let mut s = snap(&[], None);
        s.default_base = None;
        assert_eq!(labels(&Picker::new().rows(&s)), ["default (none)"]);
    }

    #[test]
    fn the_cursor_follows_the_input_and_moves_within_bounds() {
        let s = snap(&REFS, None);
        let mut p = Picker::new();
        assert_eq!(p.cursor, 0);
        p.input = "ma".into();
        p.retarget(&s);
        assert_eq!(p.cursor, 2, "the first match, after the reset and typed rows");
        p.input = "zzz".into();
        p.retarget(&s);
        assert_eq!(p.cursor, 1, "the typed row");
        p.input.clear();
        p.retarget(&s);
        assert_eq!(p.cursor, 0);
        let len = p.rows(&s).len();
        assert!(!p.move_by(-1, len, 3));
        assert!(p.move_by(1, len, 3) && p.cursor == 1);
        assert!(p.move_by(10, len, 3) && p.cursor == len - 1);
        assert_eq!(p.offset, len - 3, "the window follows the cursor down");
        assert!(p.move_by(-(len as isize), len, 3) && p.cursor == 0 && p.offset == 0);
    }

    #[test]
    fn the_panel_shows_the_caret_error_markers_and_footers() {
        let s = snap(&REFS, Some(("refs/heads/main", BaseSource::Default)));
        let mut p = Picker::new();
        p.input = "ma".into();
        p.retarget(&s);
        let panel = p.panel(&s, 12);
        assert_eq!(panel.title, "Compare against");
        assert_eq!(panel.footer, "Enter pick · Esc cancel · type to filter");
        let lines: Vec<String> = crate::tui::dialog::render(&panel, WIDTH, 12)
            .iter()
            .map(|l| l.iter().map(|s| s.text.as_str()).collect::<String>())
            .collect();
        assert!(lines[1].contains("> ma_"), "{}", lines[1]);
        assert!(lines[2].contains("default (main)"), "{}", lines[2]);
        assert!(lines[3].contains("use \"ma\""), "{}", lines[3]);
        assert!(lines[4].contains("▸ main") && lines[4].contains("current"), "{}", lines[4]);
        p.error = Some("not a commit: ma\u{1b}".into());
        let panel = p.panel(&s, 12);
        assert!(matches!(&panel.rows[1], crate::tui::dialog::Row::Warn(w) if w == "not a commit: ma\u{241b}"));
        let mut s = snap(&REFS, None);
        s.refs_overflow = true;
        let mut p = Picker::new();
        p.input = "ma".into();
        assert_eq!(p.panel(&s, 12).footer, "4 shown, more exist · type to filter");
        let s = snap(&[], None);
        assert_eq!(Picker::new().panel(&s, 12).footer, "no refs listed; type a revision");
        let mut s = snap(&REFS, None);
        s.refs = None;
        assert_eq!(Picker::new().panel(&s, 12).footer, "no refs listed; type a revision");
        let mut p = Picker::new();
        p.pending = Some(0);
        assert_eq!(p.panel(&snap(&REFS, None), 12).footer, "picking…");
    }

    #[test]
    fn the_reply_closes_the_picker_or_shows_the_error() {
        let mut s = snap(&REFS, None);
        let mut p = Picker::new();
        p.pending = Some(s.pick_seq);
        p.observe(&s);
        assert!(p.pending.is_some() && !p.done, "no answer yet");
        s.pick_seq += 1;
        s.pick_error = Some("not a commit: x".into());
        p.observe(&s);
        assert!(p.pending.is_none() && !p.done);
        assert_eq!(p.error.as_deref(), Some("not a commit: x"));
        p.pending = Some(s.pick_seq);
        s.pick_seq += 1;
        s.pick_error = None;
        p.observe(&s);
        assert!(p.done);
    }
}
```

`src/tui/input.rs` tests:

```rust
    #[test]
    fn capital_b_opens_the_picker_and_picker_keys_edit_move_pick_and_close() {
        use crate::engine::{RepoState, Scope};
        use crate::tui::picker::PickerRow;
        let (mut snap, mut st) = setup(&[(1, "+")]);
        snap.refs = Some(std::sync::Arc::new(vec![
            "refs/heads/main".into(),
            "refs/heads/feat".into(),
            "refs/tags/v1".into(),
        ]));
        snap.default_base = Some("refs/heads/main".into());
        snap.repo = RepoState::Repo { toplevel: "/r".into(), branch: Some("feat".into()), worktree: None };
        assert_eq!(handle_key(&mut st, &snap, key("B"), 120), Outcome::Engine(Command::LoadRefs));
        assert!(st.picker.is_some());
        // Body keys are inert while the picker is open; `j` and `?` are text.
        assert_eq!(handle_key(&mut st, &snap, key("j"), 120), Outcome::Redraw);
        assert_eq!(st.picker.as_ref().unwrap().input, "j");
        assert_eq!(handle_key(&mut st, &snap, KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE), 120), Outcome::Redraw);
        assert_eq!(handle_key(&mut st, &snap, KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE), 120), Outcome::Inert);
        assert_eq!(handle_key(&mut st, &snap, key("?"), 120), Outcome::Redraw);
        assert!(!st.help_open);
        assert_eq!(handle_key(&mut st, &snap, KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE), 120), Outcome::Redraw);
        assert_eq!(handle_key(&mut st, &snap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), 120), Outcome::Redraw);
        assert_eq!(handle_key(&mut st, &snap, key("ctrl+n"), 120), Outcome::Redraw);
        assert_eq!(handle_key(&mut st, &snap, key("ctrl+p"), 120), Outcome::Redraw);
        assert_eq!(st.picker.as_ref().unwrap().cursor, 1);
        let rows = st.picker.as_ref().unwrap().rows(&snap);
        assert!(matches!(&rows[1], PickerRow::Ref { qualified, .. } if qualified == "refs/heads/main"));
        assert_eq!(
            handle_key(&mut st, &snap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), 120),
            Outcome::Engine(Command::SetBase(Some("refs/heads/main".into())))
        );
        assert_eq!(st.picker.as_ref().unwrap().pending, Some(snap.pick_seq));
        assert_eq!(handle_key(&mut st, &snap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), 120), Outcome::Inert, "one pick at a time");
        // The engine answers: a failure keeps the picker open with the error; a success closes it.
        snap.pick_seq += 1;
        snap.pick_error = Some("not a commit: refs/heads/main".into());
        st.observe(&snap);
        assert_eq!(st.picker.as_ref().unwrap().error.as_deref(), Some("not a commit: refs/heads/main"));
        assert_eq!(handle_key(&mut st, &snap, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), 120), Outcome::Redraw);
        assert_eq!(
            handle_key(&mut st, &snap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), 120),
            Outcome::Engine(Command::SetBase(None)),
            "the reset row"
        );
        snap.pick_seq += 1;
        snap.pick_error = None;
        snap.scope = Scope::Branch;
        st.observe(&snap);
        assert!(st.picker.is_none());
        // Esc closes without a change; Ctrl+C still quits; the popup's Esc stays with the picker.
        handle_key(&mut st, &snap, key("B"), 120);
        st.popup = true;
        assert_eq!(handle_key(&mut st, &snap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), 120), Outcome::Redraw);
        assert!(st.picker.is_none());
        assert_eq!(handle_key(&mut st, &snap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), 120), Outcome::Quit);
        handle_key(&mut st, &snap, key("B"), 120);
        assert_eq!(handle_key(&mut st, &snap, key("ctrl+c"), 120), Outcome::Quit);
    }

    #[test]
    fn a_click_on_a_picker_row_picks_it_and_the_wheel_moves_the_cursor() {
        use crate::engine::RepoState;
        let (mut snap, mut st) = setup(&[(1, "+")]);
        snap.refs = Some(std::sync::Arc::new((0..30).map(|i| format!("refs/heads/b{i:02}")).collect()));
        snap.default_base = Some("refs/heads/b00".into());
        snap.repo = RepoState::Repo { toplevel: "/r".into(), branch: Some("b00".into()), worktree: None };
        handle_key(&mut st, &snap, key("B"), 120);
        let rendered = render(&snap, &st, 120, 24);
        let hit = rendered.hits.iter().find(|h| matches!(h.action, Action::PickRow(2))).expect("row hit");
        let click = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: hit.x0, row: hit.y, modifiers: KeyModifiers::NONE };
        assert_eq!(
            handle_mouse(&mut st, &snap, &rendered, click),
            Outcome::Engine(Command::SetBase(Some("refs/heads/b01".into())))
        );
        st.picker.as_mut().unwrap().pending = None;
        let wheel = MouseEvent { kind: MouseEventKind::ScrollDown, column: 10, row: 10, modifiers: KeyModifiers::NONE };
        assert_eq!(handle_mouse(&mut st, &snap, &rendered, wheel), Outcome::Redraw);
        assert_eq!(st.picker.as_ref().unwrap().cursor, 5);
        let toolbar = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 1, row: 0, modifiers: KeyModifiers::NONE };
        assert_eq!(handle_mouse(&mut st, &snap, &rendered, toolbar), Outcome::Inert, "toolbar hits are inert under the picker");
    }
```

`src/tui/view.rs` test:

```rust
    #[test]
    fn the_picker_overlays_the_body_and_clears_other_hits() {
        use crate::engine::RepoState;
        let mut snap = snapshot("a.rs", "r1", &[(1, "+")]);
        snap.refs = Some(std::sync::Arc::new(vec!["refs/heads/main".into()]));
        snap.default_base = Some("refs/heads/main".into());
        snap.repo = RepoState::Repo { toplevel: "/r".into(), branch: Some("main".into()), worktree: None };
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        st.resize(120, 20);
        st.reconcile(&snap);
        st.picker = Some(crate::tui::picker::Picker::new());
        let r = render(&snap, &st, 120, 24);
        let text = r.plain();
        assert!(text[1].contains("Compare against"), "{}", text[1]);
        assert!(text[2].contains("> _"), "{}", text[2]);
        assert!(text[3].contains("default (main)"), "{}", text[3]);
        assert!(text[4].contains("main") && text[4].contains("current"), "{}", text[4]);
        assert!(r.hits.iter().all(|h| matches!(h.action, Action::PickRow(_))));
        assert_eq!(r.hits.len(), 2);
        assert_eq!(r.hits[0].y, 3);
        assert_eq!(text.len(), 24);
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib tui`
Expected: compile errors (`picker`, `PickBase`, `PickRow` unknown).

- [ ] **Step 3: Write `picker.rs`**

Above the test module:

```rust
use crate::engine::{ref_label, BaseSource, RepoState, Snapshot};
use crate::tui::dialog::{Panel, Row};
use crate::tui::format::truncate;
use crate::tui::sanitize::sanitize;

pub const WIDTH: u16 = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerRow {
    /// `default (<label>)`: forget the pick and re-resolve.
    Reset(String),
    /// `use "<input>"`: the typed text as a base.
    Typed(String),
    /// A listed ref: the qualified name it submits and its markers.
    Ref {
        qualified: String,
        markers: Vec<&'static str>,
    },
}

impl PickerRow {
    /// What `SetBase` is sent; `None` is the reset row.
    pub fn submit(&self) -> Option<String> {
        match self {
            Self::Reset(_) => None,
            Self::Typed(text) | Self::Ref { qualified: text, .. } => Some(text.clone()),
        }
    }
}

#[derive(Debug, Default)]
pub struct Picker {
    pub input: String,
    /// Index into `rows()`.
    pub cursor: usize,
    /// First list row drawn; kept so the window follows the cursor.
    pub offset: usize,
    /// Shown under the input line.
    pub error: Option<String>,
    /// `pick_seq` seen when `SetBase` was sent; the answer is the first snapshot beyond it.
    pub pending: Option<u64>,
    /// Set when the answer was a success: the shell drops the picker.
    pub done: bool,
}

impl Picker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn rows(&self, snapshot: &Snapshot) -> Vec<PickerRow> {
        let default_label = snapshot
            .default_base
            .as_deref()
            .map(ref_label)
            .unwrap_or("none");
        let mut rows = vec![PickerRow::Reset(format!("default ({default_label})"))];
        let refs: &[String] = snapshot.refs.as_deref().map(Vec::as_slice).unwrap_or(&[]);
        let listed = refs.iter().any(|r| ref_label(r) == self.input);
        if !self.input.is_empty() && !listed {
            rows.push(PickerRow::Typed(self.input.clone()));
        }
        let needle = self.input.to_lowercase();
        let current_base = snapshot.base.as_ref().map(|b| b.requested.as_str());
        let checked_out = match &snapshot.repo {
            RepoState::Repo {
                branch: Some(branch),
                ..
            } => Some(format!("refs/heads/{branch}")),
            _ => None,
        };
        let mut matching: Vec<PickerRow> = refs
            .iter()
            .filter(|r| ref_label(r).to_lowercase().contains(&needle))
            .map(|r| {
                let mut markers = Vec::new();
                if current_base == Some(r.as_str()) {
                    match snapshot.base.as_ref().map(|b| b.source) {
                        Some(BaseSource::Picked) => markers.push("picked"),
                        Some(BaseSource::Config) => markers.push("config"),
                        _ => {}
                    }
                }
                if checked_out.as_deref() == Some(r.as_str()) {
                    markers.push("current");
                }
                if r.starts_with("refs/tags/") {
                    markers.push("tag");
                }
                PickerRow::Ref {
                    qualified: r.clone(),
                    markers,
                }
            })
            .collect();
        // The current base's row comes right after the reset row.
        if let Some(i) = matching.iter().position(
            |row| matches!(row, PickerRow::Ref { qualified, .. } if Some(qualified.as_str()) == current_base),
        ) {
            let row = matching.remove(i);
            matching.insert(0, row);
        }
        rows.extend(matching);
        rows
    }

    fn head(&self) -> usize {
        1 + usize::from(self.error.is_some())
    }

    /// List rows that fit: the frame and footer take four lines, the input and error lines the rest.
    pub fn visible(&self, height: u16) -> usize {
        usize::from(height).saturating_sub(4 + self.head()).max(1)
    }

    /// First list row drawn for a window of `visible` rows: the cursor stays inside it.
    pub fn window(&self, visible: usize) -> usize {
        self.offset
            .min(self.cursor)
            .max(self.cursor.saturating_sub(visible - 1))
    }

    pub fn panel(&self, snapshot: &Snapshot, height: u16) -> Panel {
        let rows = self.rows(snapshot);
        let listed = rows
            .iter()
            .filter(|r| matches!(r, PickerRow::Ref { .. }))
            .count();
        let footer = if self.pending.is_some() {
            "picking…".to_string()
        } else if snapshot.refs.as_ref().is_none_or(|r| r.is_empty()) {
            "no refs listed; type a revision".to_string()
        } else if snapshot.refs_overflow {
            format!("{listed} shown, more exist · type to filter")
        } else {
            "Enter pick · Esc cancel · type to filter".to_string()
        };
        let mut panel_rows = vec![Row::Text(truncate(
            &format!("> {}_", self.input),
            usize::from(WIDTH) - 6,
        ))];
        if let Some(error) = &self.error {
            panel_rows.push(Row::Warn(sanitize(error)));
        }
        let head = panel_rows.len();
        let visible = self.visible(height);
        let offset = self.window(visible);
        for row in rows.iter().skip(offset).take(visible) {
            let (label, value) = match row {
                PickerRow::Reset(text) => (text.clone(), String::new()),
                PickerRow::Typed(text) => (format!("use \"{text}\""), String::new()),
                PickerRow::Ref { qualified, markers } => {
                    (sanitize(ref_label(qualified)), markers.join(" · "))
                }
            };
            panel_rows.push(Row::Entry {
                label: truncate(&label, 30),
                value,
                enabled: true,
            });
        }
        Panel {
            title: "Compare against".into(),
            rows: panel_rows,
            footer,
            cursor: Some(head + self.cursor - offset),
            offset: 0,
        }
    }

    /// Moves the cursor, keeping it inside `[0, len)` and the window around it; false when nothing moved.
    pub fn move_by(&mut self, delta: isize, len: usize, visible: usize) -> bool {
        if len == 0 {
            return false;
        }
        let next = (self.cursor as isize + delta).clamp(0, len as isize - 1) as usize;
        if next == self.cursor {
            return false;
        }
        self.cursor = next;
        self.offset = self.window(visible.max(1));
        true
    }

    /// After an edit: the first match, else the typed row.
    pub fn retarget(&mut self, snapshot: &Snapshot) {
        let rows = self.rows(snapshot);
        self.cursor = rows
            .iter()
            .position(|r| matches!(r, PickerRow::Ref { .. }))
            .or_else(|| rows.iter().position(|r| matches!(r, PickerRow::Typed(_))))
            .unwrap_or(0);
        self.offset = 0;
        self.error = None;
    }

    /// The engine's answer to the pending `SetBase`.
    pub fn observe(&mut self, snapshot: &Snapshot) {
        if let Some(sent) = self.pending {
            if snapshot.pick_seq > sent {
                self.pending = None;
                match &snapshot.pick_error {
                    Some(error) => self.error = Some(error.clone()),
                    None => self.done = true,
                }
            }
        }
    }
}
```

Add `pub mod picker;` to `src/tui/mod.rs`. (`Option::is_none_or` is stable since Rust 1.82; the toolchain is 1.88.)

- [ ] **Step 4: Keys, state, view and input**

`src/tui/keys.rs`: `KeyAction::PickBase` after `ToggleScope`; insert after the `b` binding:

```rust
    Binding {
        key: "B",
        label: "compare against",
        action: KeyAction::PickBase,
        vimeflow: None,
    },
```

`src/tui/state.rs`: add `pub picker: Option<crate::tui::picker::Picker>,` (initialised `None`), and extend `observe`:

```rust
    pub fn observe(&mut self, snapshot: &Snapshot) {
        if snapshot.base_error != self.seen_base_error {
            self.seen_base_error = snapshot.base_error.clone();
            if let (Some(error), None) = (&snapshot.base_error, &self.picker) {
                self.notice = Some(crate::tui::sanitize::sanitize(error));
            }
        }
        if let Some(picker) = &mut self.picker {
            picker.observe(snapshot);
            if picker.done {
                self.picker = None;
            }
        }
    }
```

`src/tui/view.rs`:
- `Action` gains `PickRow(usize)`; `key_action` returns `None` for it (add it to the `SelectFile(_) | CursorToRow(_)` arm).
- `Hit::hovered` also requires `state.picker.is_none()`.
- In `render`, after the `if state.help_open { .. }` block:

```rust
    if let Some(picker) = &state.picker {
        let panel_width = columns.min(picker::WIDTH);
        let panel_height = height.saturating_sub(2);
        let x = usize::from((columns - panel_width) / 2);
        let panel = picker.panel(snapshot, panel_height);
        for (y, overlay) in dialog::render(&panel, panel_width, panel_height)
            .into_iter()
            .enumerate()
        {
            let background = &lines[y + 1];
            let mut line = clip_line(background, 0, x);
            line.extend(overlay);
            let right = x + usize::from(panel_width);
            line.extend(clip_line(background, right, usize::from(columns) - right));
            lines[y + 1] = line;
        }
        hits.clear();
        // One hit per drawn list row: the first list row sits under the input (and error) line.
        let head = panel.rows.iter().take_while(|r| !matches!(r, dialog::Row::Entry { .. })).count();
        let first = picker.window(picker.visible(panel_height));
        let listed = panel.rows.len() - head;
        for i in 0..listed {
            hits.push(Hit {
                y: (2 + head + i) as u16,
                x0: x as u16,
                x1: (x + usize::from(panel_width)) as u16,
                action: Action::PickRow(first + i),
            });
        }
    }
```

Import `picker` in the module's `use crate::tui::{dialog, keys, layout, picker};`. The hit's `y` is the overlay line index plus one for the toolbar and one for the panel's top edge.

`src/tui/input.rs`:
- In `handle_key`, after the `help_open` block and before the popup `Esc` check:

```rust
    if state.picker.is_some() {
        return picker_key(state, snapshot, key);
    }
```

- In `act`, add `PickBase => { state.picker = Some(crate::tui::picker::Picker::new()); return Outcome::Engine(Command::LoadRefs); }`.
- Add:

```rust
fn picker_key(state: &mut ViewState, snapshot: &Snapshot, key: KeyEvent) -> Outcome {
    let visible = state
        .picker
        .as_ref()
        .map(|p| p.visible(state.body_height.saturating_add(u16::from(view::notice(state, snapshot).is_some()))))
        .unwrap_or(1);
    let Some(picker) = state.picker.as_mut() else {
        return Outcome::Inert;
    };
    let rows = picker.rows(snapshot);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let moved = |picker: &mut crate::tui::picker::Picker, delta: isize| {
        if picker.move_by(delta, rows.len(), visible) {
            Outcome::Redraw
        } else {
            Outcome::Inert
        }
    };
    match (key.code, ctrl) {
        (KeyCode::Esc, false) => {
            state.picker = None;
            Outcome::Redraw
        }
        (KeyCode::Enter, false) => {
            if picker.pending.is_some() {
                return Outcome::Inert;
            }
            let Some(row) = rows.get(picker.cursor) else {
                return Outcome::Inert;
            };
            picker.pending = Some(snapshot.pick_seq);
            picker.error = None;
            Outcome::Engine(Command::SetBase(row.submit()))
        }
        (KeyCode::Down, false) | (KeyCode::Char('n'), true) => moved(picker, 1),
        (KeyCode::Up, false) | (KeyCode::Char('p'), true) => moved(picker, -1),
        (KeyCode::Backspace, false) => {
            if picker.input.pop().is_some() {
                picker.retarget(snapshot);
                Outcome::Redraw
            } else {
                Outcome::Inert
            }
        }
        (KeyCode::Char(ch), false) if !ch.is_control() => {
            picker.input.push(ch);
            picker.retarget(snapshot);
            Outcome::Redraw
        }
        _ => Outcome::Inert,
    }
}
```

(`body_height` plus the notice line is the height the overlay is drawn against, as `scroll_help` computes it; `panel_height` in `render` is `height - 2`, and `body_height` is `height - 2 - notice`, so adding the notice back gives the same number.)

- In `handle_mouse`: the wheel branch, before the `help_open` case, moves the picker's cursor by three:

```rust
        let overlay = state
            .body_height
            .saturating_add(u16::from(view::notice(state, snapshot).is_some()));
        if let Some(picker) = state.picker.as_mut() {
            let visible = picker.visible(overlay);
            let len = picker.rows(snapshot).len();
            return if picker.move_by(delta, len, visible) { Outcome::Redraw } else { Outcome::Inert };
        }
```

(`overlay` is computed before the mutable borrow of the picker.) The click branch: `if state.help_open || ev.kind != MouseEventKind::Down(MouseButton::Left) { return Outcome::Inert; }` stays; after it, before the toolbar dispatch:

```rust
    if let Some(picker) = state.picker.as_mut() {
        return match rendered.hit(ev.column, ev.row) {
            Some(Action::PickRow(index)) if picker.pending.is_none() => {
                let rows = picker.rows(snapshot);
                match rows.get(*index) {
                    Some(row) => {
                        picker.cursor = *index;
                        picker.pending = Some(snapshot.pick_seq);
                        picker.error = None;
                        Outcome::Engine(Command::SetBase(row.submit()))
                    }
                    None => Outcome::Inert,
                }
            }
            _ => Outcome::Inert,
        };
    }
```

- [ ] **Step 5: Run the TUI tests**

Run: `cargo test --lib tui`
Expected: PASS. `every_binding_is_reachable_on_the_key_sheet_in_a_short_terminal` now covers 23 rows; `popup_escape_closes_only_the_topmost_view` keeps passing because the picker's `Esc` is handled before the popup's.

- [ ] **Step 6: Verify**

Run: `cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked && cargo check --locked --no-default-features && scripts/port-check.sh`
Expected: all pass.

- [ ] **Step 7: Commit (orchestrator)**

```bash
git add -A
git commit -m "feat(tui): base picker with a filtering input line"
```

---

### Task 6: Read-only extension, tier B, documentation, acceptance rows, version 0.1.0

Implements spec 7.6 (the G7 extension), 7.7 (the version), 7.8 (README), 7.9 items 6-8 (G7, tier B, acceptance rows 6 and 7). Read those before starting.

**Files:**
- Modify: `tests/readonly_guarantee.rs`
- Modify: `tests/e2e_real_herdr.rs`
- Modify: `README.md`, `README.zh-CN.md`, `README.ja.md`, `AGENTS.md`
- Modify: `docs/acceptance-p1.md`
- Modify: `Cargo.toml`, `Cargo.lock`, `herdr-plugin.toml`

**Interfaces:**
- Consumes: `Command::{SetScope, SetBase, LoadRefs}`, `Snapshot { scope, base, refs, pick_seq }`, `SessionConfig.state_dir` (Tasks 1-3); the `b` key through the host (Task 4).
- Produces: nothing new in code.

- [ ] **Step 1: Extend the G7 test**

In `tests/readonly_guarantee.rs`, `the_engine_never_mutates_the_repository`:

1. Imports: add `Scope` to the `herdr_hunks::engine` import.
2. Fixture: after the `b` commit and before `git branch other`, put the work on a feature branch with one committed change, so branch scope has something to list:

```rust
    git(&real, p, &["switch", "-q", "-c", "feat"]);
    std::fs::write(p.join("c.txt"), "c\n").unwrap();
    git(&real, p, &["add", "c.txt"]);
    git(&real, p, &["commit", "-q", "-m", "c"]);
    git(&real, p, &["branch", "other", "main"]); // the harness switches HEAD here later; index and worktree untouched
```

and delete the old `git(&real, p, &["branch", "other"]);` line. The `symbolic-ref HEAD refs/heads/other` switch later in the test now moves HEAD from `feat` to `other`, which is `main`'s tip; with `c.txt` committed on `feat` only, that switch changes neither index nor worktree, exactly as before.

3. A state directory the engine may write to, given before `spawn`:

```rust
    let state = tempfile::tempdir().unwrap();
    let mut config = SessionConfig::production(p.to_path_buf());
    config.poll_interval = Duration::from_millis(100);
    config.state_dir = Some(state.path().to_path_buf());
```

4. After the existing loop has loaded every worktree row (the `assert_eq!(rows, 3, ..)` and `loaded.len() == rows` assertions stay; move the `saw_other` assertion after the branch-scope drive, and move the `symbolic-ref HEAD` switch out of the first loop: the first loop now ends when `rows > 0 && loaded.len() == rows`), drive branch scope with the same recv loop shape:

```rust
    // Branch scope: every row, the ref list, a different base, and back.
    handle.commands.send(Command::SetScope(Scope::Branch)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut branch_loaded = std::collections::BTreeSet::new();
    let mut branch_rows = 0usize;
    while Instant::now() < deadline && !(branch_rows > 0 && branch_loaded.len() == branch_rows) {
        let Ok(s) = handle.snapshots.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        if s.scope != Scope::Branch {
            continue;
        }
        branch_rows = s.files.len();
        if let DiffState::Ready(d) = &s.diff {
            if branch_loaded.insert((d.key.path.clone(), d.key.untracked)) {
                handle.commands.send(Command::SelectNext).unwrap();
            }
        }
    }
    assert_eq!(branch_rows, 4, "a.txt, b.txt, c.txt and new.txt against main");
    assert_eq!(branch_loaded.len(), branch_rows, "not every branch row was loaded: {branch_loaded:?}");
    handle.commands.send(Command::LoadRefs).unwrap();
    handle.commands.send(Command::SetBase(Some("refs/heads/other".into()))).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let (mut saw_refs, mut picked) = (false, false);
    while Instant::now() < deadline && !(saw_refs && picked) {
        let Ok(s) = handle.snapshots.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        saw_refs |= s.refs.as_ref().is_some_and(|r| r.iter().any(|r| r == "refs/heads/other"));
        picked |= s.pick_seq == 1 && s.base.as_ref().is_some_and(|b| b.requested == "refs/heads/other") && s.pick_error.is_none();
    }
    assert!(saw_refs, "the ref list never listed refs/heads/other");
    assert!(picked, "the pick was not published");
    handle.commands.send(Command::SetScope(Scope::Worktree)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut back = false;
    while Instant::now() < deadline && !back {
        let Ok(s) = handle.snapshots.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        back = s.scope == Scope::Worktree && s.files.len() == 3;
    }
    assert!(back, "worktree scope did not come back");
    // Now the harness switches HEAD underneath the engine, through the real git.
    git(&real, p, &["symbolic-ref", "HEAD", "refs/heads/other"]);
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut saw_other = false;
    while Instant::now() < deadline && !saw_other {
        let Ok(s) = handle.snapshots.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        saw_other = matches!(&s.repo, herdr_hunks::engine::RepoState::Repo { branch: Some(b), .. } if b == "other");
    }
    assert!(saw_other, "the branch switch never reached the engine");
```

5. After the existing subcommand and D3 assertions, assert that the branch-scope commands ran and the state directory holds exactly what 7.6 allows:

```rust
    let subs: std::collections::BTreeSet<&str> = recorded
        .lines()
        .map(|entry| {
            let args: Vec<&str> = entry.split('\t').next().unwrap_or("").split_whitespace().collect();
            let mut i = 0;
            while args.get(i) == Some(&"-C") {
                i += 2;
            }
            args.get(i).copied().unwrap_or("")
        })
        .collect();
    for expected in ["merge-base", "for-each-ref", "symbolic-ref", "rev-parse"] {
        assert!(subs.contains(expected), "{expected} never ran: {subs:?}");
    }
    assert!(
        recorded.lines().any(|l| l.contains("diff ") && l.contains("--name-status -M -z --")),
        "no name-status against the merge-base"
    );
    assert!(
        !recorded.lines().any(|l| l.contains("--merge-base")),
        "the merge-base must be pinned, never recomputed by git diff"
    );
    let mut written: Vec<String> = std::fs::read_dir(state.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    written.sort();
    assert_eq!(written, ["bases.json", "split-panes.lock"], "the viewer wrote something else");
    let picks: std::collections::BTreeMap<String, String> =
        serde_json::from_str(&std::fs::read_to_string(state.path().join("bases.json")).unwrap()).unwrap();
    assert_eq!(picks.values().collect::<Vec<_>>(), [&"refs/heads/other".to_string()]);
```

Keep the index, refs and worktree assertions at the end as they are. `serde_json` is already a dependency of the crate and usable from integration tests.

- [ ] **Step 2: Run it**

Run: `cargo test --test readonly_guarantee -- --nocapture`
Expected: PASS. If a subcommand outside `ALLOWED` appears, the failure names it; that is a G7 violation to fix in the engine, never by extending the list.

- [ ] **Step 3: Tier B presses `b`**

In `tests/e2e_real_herdr.rs`, `open_split_creates_one_viewer_and_reuses_it`, right after the fixture's `git(&["commit", "-q", "-m", "init"]);` add a branch with a committed change before the working-tree edit:

```rust
        git(&["switch", "-q", "-c", "feat"]);
        std::fs::write(repo.join("c.txt"), "c\n").unwrap();
        git(&["add", "c.txt"]);
        git(&["commit", "-q", "-m", "c"]);
```

After the viewer has opened and been asserted once (the `wait_for("one viewer pane", ..)` and the `viewer_id` binding), press `b` through the host and wait for the branch chip and the committed row:

```rust
        iso.herdr(&["pane", "send-text", &viewer_id, "b"]);
        iso.herdr(&[
            "pane", "wait-output", &viewer_id, "--match", "vs main", "--source", "visible", "--timeout", "15000",
        ]);
        let screen = iso.herdr(&["pane", "read", &viewer_id, "--source", "visible"]).to_string();
        assert!(screen.contains("c.txt"), "the committed row is missing in branch scope: {screen}");
```

(`iso.herdr` asserts the command's success and returns its JSON, so a timed-out `wait-output` fails the test.) The rest of the test is unchanged; it still runs against both hosts as `docs/acceptance-p1.md` describes.

Run: `cargo build --release && HERDR_BIN_PATH=<host> cargo test --test e2e_real_herdr -- --ignored --nocapture` once for each host the acceptance document names (the orchestrator does this; the implementer runs `cargo test --test e2e_real_herdr` to confirm it still compiles and stays ignored).

- [ ] **Step 4: README, three languages**

`README.md`:
- In `## Keys`, add two rows to the key table after `r`: `b` — `switch scope: worktree <-> branch`, `B` — `compare against: pick the base`.
- In `## Configuration`, extend the example to:

```toml
[view]
mode = "auto"      # auto, unified, split
files = "auto"     # auto, pinned, hidden
scope = "worktree" # worktree, branch

[input]
mouse = true

[base]
ref = "main"       # any revision; see "Branch scope"

[popup]
width = "80%"
height = "80%"
```

- Add a section `## Branch scope` before `## Requirements`:

```markdown
## Branch scope

The viewer has two scopes. `worktree` (the default) lists what `git status`
lists: staged, unstaged and untracked changes against `HEAD`. `branch` lists
every change the current branch carries against its base: the working tree
against `merge-base(HEAD, base)`, one row per path, committed or not, plus
untracked files. Press `b` to switch (the toolbar chip reads `worktree` or
`vs <base>`); with `prefix+f` bound to `open-split`, the gesture is `prefix`,
`f`, `b`. A path deleted on the branch and recreated untracked is two rows.

The base is resolved in this order, taking the first that names a commit:

1. the base you picked with `B` on this worktree (remembered in `bases.json`
   under the state directory);
2. `[base] ref` from `config.toml`;
3. `refs/heads/main`;
4. the remote's default branch (`refs/remotes/origin/HEAD`);
5. `refs/heads/master`.

When nothing resolves, `b` says `no base branch: set [base] ref or press B`.

`B` opens "Compare against": type to filter local branches, remote branches
and tags (newest first, 200 at most; type to reach the rest), or type any
revision such as `origin/main` or `HEAD~3`. `Enter` picks, `Esc` cancels, the
first row `default (...)` forgets the pick. Picked and listed refs are passed
to git fully qualified; free text and `[base] ref` are passed as written, so
write `refs/heads/x` when a tag shares the name. A pick is remembered per
worktree; when the state directory is unusable it lasts for the session.

Branch scope is read-only like the rest of the viewer: it adds `merge-base`
and `for-each-ref` to the git commands the viewer runs. The base is
re-checked on every refresh, so a base that moved (a merge into `main`, a
fetch) is reflected within the poll interval.
```

- In `## Known limitations`, add: `In worktree scope, a staged row whose path turned from a file into a directory parses the directory's patch as its own (K7 in PORT-SURFACE.md); branch scope is unaffected.`

`README.zh-CN.md` and `README.ja.md` mirror every change section for section, as they mirror the rest (translate; keep the key names, config keys, notices and the ref names verbatim).

- [ ] **Step 5: `AGENTS.md` and the acceptance document**

`AGENTS.md`: where the reserved keys are stated, add one sentence: `b` and `B` are the scope keys; the Phase 2 write actions bind in `worktree` scope only and, in `branch` scope, show `switch to worktree scope (b) to stage or discard`. Where the port-check command is stated, mention that the engine reaches five frozen functions through the D6 visibility patch and that the allow-list has ten subcommands.

`docs/acceptance-p1.md`: change the first line to `Status: PENDING` and add two rows to the table:

```markdown
| 6. Branch scope parity | On a branch with committed and uncommitted changes (the fixture of `branch_scope_lists_every_change_the_branch_carries_once`, or a real worktree), press `b`. Compare the row list with `git diff $(git merge-base HEAD main) --name-status -M --` plus the `??` rows of `git status --porcelain --untracked-files=all`; compare one tracked row's hunks with `git diff <M> -- <path>`, the rename with `git diff <M> -M -- <old> <new>`, and an untracked row with `git diff --no-index -- /dev/null <path>`. | Every changed path is listed once (twice only for a path deleted on the branch and recreated untracked) and each diff matches hunk for hunk. | pending | | | |
| 7. Default base and remembered pick | On a repository with `main` and no configuration, press `b`: the chip reads `vs main`. Press `B`, pick another ref, close the viewer, reopen it on the same worktree and press `b`. | The base is `main` without configuration; the reopened viewer compares against the picked ref. | pending | | | |
```

The document's own instruction (`change the first line to Status: PASS after all rows pass`) applies again once the orchestrator records the evidence for rows 6 and 7; the release workflow refuses to publish until then, which is the guard doing its job.

- [ ] **Step 6: Version 0.1.0**

Set `version = "0.1.0"` in `Cargo.toml` and `herdr-plugin.toml`, then run `cargo check` (without `--locked`) once so `Cargo.lock` records the new version of the root package, and commit the three files together.

- [ ] **Step 7: Verify**

Run: `cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked && cargo check --locked --no-default-features && scripts/port-check.sh && cargo build --release --locked`
Expected: all pass; `cargo test --locked` accepts the refreshed `Cargo.lock`.

- [ ] **Step 8: Commit (orchestrator)**

```bash
git add -A
git commit -m "docs: branch scope, acceptance rows 6 and 7, version 0.1.0"
```

---

## Self-review against the spec

- 7.1: two scopes, rows per scope, no staged half in branch scope: Tasks 3 (rows), 4 (label hidden).
- 7.2: pinned merge-base (Task 3 `load_rows`), the three row commands and the mapping of status letters (`status_of`), `rename_sources`, numstat keyed by new path, untracked rows appended, deleted-and-recreated as two keys (Task 1 `FileKey.untracked`, Task 3 sort and test), per-row diff argv with `--src-prefix`/`--dst-prefix`, section isolation with decoded quoted headers, type changes concatenated (`sections` + `parse_git_diff` per section), untracked rows through `get_git_diff_inner` (`untracked_diff`), `Comparison` on every diff and result, generation bump, publication rule (`Loaded` published whole; failures keep the snapshot), selection on a scope switch (unstaged row preferred), corner cases (same branch as base: `merge-base` of `HEAD` and itself; detached `HEAD`: `HEAD` is a commit; unrelated histories: `merge_base` fails into `status_error`/`pick_error`).
- 7.3: resolution order and validation (Task 2 `resolve`, `verify`, `check_text`), fully qualified defaults, free text as written, `bases.json` under `with_lock` with atomic rename, session override (`inputs.session_pick`, Task 3), reset row semantics (`Pick(None)`), ids re-verified every refresh in branch scope only, preference re-resolved at start, on `r`, on a pick and on a branch-name change (`branch_changed`).
- 7.4: `b`, `B`, chip text and truncation, disabled chip, hover hint, drop order 2/3, hidden staged label (Task 4); the picker's rows, reset row, typed row, filtering, ordering, markers, keys, mouse, cap and overflow, `LoadRefs` on open, error under the input, pick on Enter with `SetBase`, modality (Task 5; `hits.clear()` under the overlay).
- 7.5: one spawned task per refresh (`run_job`), triggers unchanged, cost as stated.
- 7.6: allow-list of ten (Task 1), G7 drive and state-directory assertion (Task 6).
- 7.7: D6 patch and K7 (Task 1), Phase 2 keys stay unbound in both scopes (Task 4 test), version 0.1.0 (Task 6).
- 7.8: every failure row has a home: no base (`NO_BASE_NOTICE` from the engine and the key), skipped steps (`skipped` -> `base_error` -> notice once), base disappears (`verify` fails in `load_rows` -> `status_error`, rows kept), unrelated histories (`merge_base` error), typed base fails (`pick_error` under the input), `for-each-ref` fails (`Done::Refs` -> empty list -> footer), `bases.json` malformed (`load_picks` problem -> `note_problem` + `base_error`), overflow footer; config keys (Task 4); README (Task 6).
- 7.9: tests 1-3 in Task 3, 4-5 in Tasks 4 and 5, 6 and 7 in Task 6; criteria 6-8 in the acceptance rows.

Resolved ambiguity: 7.4's prose "dropped before the view chip" versus its numbers (scope 2, view 3, higher drops first). The plan follows the numbers; the prose should be corrected with the plan-review findings.

Type consistency: `FileKey::of`, `ref_label`, `Comparison`, `NO_BASE_NOTICE`, `base::{check_text, verify, merge_base, resolve, ResolveInputs, load_picks, save_pick, note_problem, list_refs, REFS_CAP}`, `branch::{parse_name_status, rows, split_header, keep_sections, diff, untracked_diff}`, `Snapshot.{scope, base, base_error, default_base, rename_sources, refs, refs_overflow, pick_seq, pick_error}`, `Command::{SetScope, SetBase, LoadRefs}`, `KeyAction::{ToggleScope, PickBase}`, `Action::{ToggleScope, PickRow}`, `ViewState::{observe, picker}`, `Picker::{new, rows, visible, panel, move_by, retarget, observe}`, `Config.{scope, base}`, `SessionConfig.{scope, base_ref, state_dir}` are the names used throughout.
