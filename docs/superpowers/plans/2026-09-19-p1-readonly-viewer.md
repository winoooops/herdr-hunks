# herdr-hunks Phase 1 (read-only hunk viewer) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship Phase 1 of `herdr-hunks`: a read-only, lazygit-like git hunk viewer TUI for herdr with a clickable navigation toolbar and live refresh, built on a frozen port of vimeflow's git module.

**Architecture:** One Rust crate with a library and a binary. `src/git/` is vimeflow's git module at a pinned commit plus two registered patches. `src/engine/` wraps it in a UI-agnostic session that publishes immutable snapshots and owns a pure navigation model. `src/tui/` is a pure view plus a thin ratatui shell. `src/herdr/` and `src/actions/` implement the `open`, `open-split` and `update` plugin actions.

**Tech Stack:** Rust 1.88, edition 2021. `tokio` 1, `notify` 6, `ignore` 0.4, `sha2` 0.10, `libc` 0.2, `dirs` 6, `thiserror` 2, `serde`/`serde_json` 1, `toml` 0.8, `log` 0.4. Behind the default `tui` feature: `ratatui` 0.30 (`default-features = false`, feature `crossterm_0_29`), `crossterm` 0.29, `unicode-width` 0.2. Dev: `tempfile` 3, `ts-rs` 10. Runtime: `git` 2.31 or newer.

**Spec:** `docs/superpowers/specs/2026-09-18-hunks-roadmap-p1-viewer-design.md`. Executors read the spec section named in each task before starting it.

## Global Constraints

- The vimeflow pin is commit `91e45b1c`. Commands below use `$VIMEFLOW` for a checkout at that commit (on the author's machine: `~/projects/vimeflow`) and `$WATCHER` for a checkout of `herdr-agent-watcher` (`~/projects/herdr-agent-watcher`).
- Nobody edits `src/git/` by hand. A change there is either a new pin or a new file in `port/patches/`. `scripts/port-check.sh` must pass after every task that touches it.
- The repository, crate and binary are named `herdr-hunks`; the library is `herdr_hunks`; the plugin id is `winoooops.hunks`. Code never hardcodes the plugin id: it reads `$HERDR_PLUGIN_ID`, `$HERDR_PLUGIN_CONFIG_DIR` and `$HERDR_PLUGIN_STATE_DIR`.
- The host binary is never named literally. Use `$HERDR_BIN_PATH` or the socket at `$HERDR_SOCKET_PATH`.
- Unix only. The one cargo feature is `tui` (default). `cargo check --no-default-features` must pass after every task.
- G7: Phase 1 code calls only `git_status_inner`, `get_git_diff_inner`, `git_branch_inner`, `git_worktree_name_inner`, `start_git_watcher_backend` and `stop_git_watcher_backend` from the frozen tree. The git floor is 2.31.
- These keys stay unbound in Phase 1: `s d D i I u U x v y Y @ c /`.
- Colours are the terminal's named ANSI colours only. No RGB, no background tints.
- Every string that came from git is untrusted and passes through `tui::sanitize` before it reaches a cell.
- Commits are conventional with a lowercase subject. Inline comments are one short line and never reference a task or PR.
- Run before every commit: `cargo fmt --check && cargo test && cargo check --no-default-features`.

## File Structure

```
herdr-plugin.toml               manifest (Task 15)
Cargo.toml                      deps and the `tui` feature (Task 1)
PORT-SURFACE.md                 port surface, D1-D5, K1-K5, copied files (Tasks 1, 2, 9, 14)
port/patches/                   0001-no-ext-diff.patch, 0002-drain-sync-output.patch (Task 2)
scripts/port-check.sh           pristine pin + patches == src/git (Task 1)
scripts/fetch-or-build.sh       [[build]] step (Task 15)
src/lib.rs                      module tree, unix guard
src/main.rs                     subcommand dispatch: tui (default), open, open-split, update
src/git/                        FROZEN: mod.rs, watcher.rs, test_helpers.rs
src/filesystem/{mod,scope}.rs   shim, D1 policy
src/runtime/                    byte-identical shim: mod.rs, event_sink.rs, test_event_sink.rs
src/engine/mod.rs               re-exports, init_process_env
src/engine/gitver.rs            `git --version` parse and the 2.31 floor
src/engine/nav.rs               Target and pure movement functions
src/engine/types.rs             FileKey, Snapshot, RepoState, DiffState, LoadedDiff, Command
src/engine/session.rs           session loop, EngineHandle, ChannelSink, WatcherControl
src/git_diff_response_tests.rs  #[cfg(test)] adapted vimeflow cases, D3/D4 fixtures
src/herdr/{mod,client,api}.rs   socket client, copied from herdr-agent-watcher
src/actions/{mod,open,reuse,update}.rs
src/tui/{style,layout,dialog,guard}.rs   copied or adapted from herdr-agent-watcher
src/tui/{sanitize,rows,state,view,keys,input,config,shell}.rs
tests/support/                  fake herdr socket, from herdr-agent-watcher
tests/actions_tier_a.rs         open / open-split / reuse against the fake socket
tests/readonly_guarantee.rs     G7 recording-git test
tests/e2e_real_herdr.rs         #[ignore] tier B
.github/workflows/{ci,release}.yml
```

---

### Task 1: Crate skeleton, frozen port, shims, port check

Spec: 2.1, 2.2, 2.4 D1.

**Files:**
- Create: `Cargo.toml`, `src/lib.rs`, `src/main.rs`, `src/git/` (copied), `src/runtime/` (copied), `src/filesystem/mod.rs`, `src/filesystem/scope.rs`, `PORT-SURFACE.md`, `scripts/port-check.sh`
- Modify: `.gitignore`

**Interfaces:**
- Consumes: nothing.
- Produces: `herdr_hunks::git` (frozen; `pub(crate)` read functions `git_status_inner(cwd: String) -> Result<GitStatusResponse, String>`, `get_git_diff_inner(cwd: String, file: String, staged: bool, untracked: Option<bool>) -> Result<GetGitDiffResponse, String>`, `git_branch_inner(cwd: String) -> Result<String, String>`, `git_worktree_name_inner(cwd: String) -> Result<Option<String>, String>`, and `watcher::{GitWatcherState, start_git_watcher_backend(cwd: String, events: Arc<dyn EventSink>, state: GitWatcherState), stop_git_watcher_backend(cwd: String, state: GitWatcherState)}`), `herdr_hunks::runtime::{EventSink, FakeEventSink}`, `herdr_hunks::filesystem::scope`.

- [ ] **Step 1: Write `Cargo.toml`**

```toml
[package]
name = "herdr-hunks"
version = "0.1.0"
description = "Read-only git hunk viewer plugin for herdr, ported from vimeflow."
license = "Apache-2.0"
edition = "2021"
rust-version = "1.88"

[lib]
name = "herdr_hunks"
path = "src/lib.rs"

[[bin]]
name = "herdr-hunks"
path = "src/main.rs"
required-features = ["tui"]

[features]
default = ["tui"]
tui = ["dep:ratatui", "dep:crossterm", "dep:unicode-width"]
# Unused. Kept so src/runtime/event_sink.rs stays byte-identical to vimeflow's.
e2e-test = []

[dependencies]
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
log = "0.4"
tokio = { version = "1", features = ["sync", "io-util", "io-std", "time", "rt", "rt-multi-thread", "macros", "process"] }
notify = "6"
ignore = "0.4"
sha2 = "0.10"
libc = "0.2"
dirs = "6"
thiserror = "2.0"
toml = "0.8"
ratatui = { version = "0.30", default-features = false, features = ["crossterm_0_29"], optional = true }
crossterm = { version = "0.29", optional = true }
unicode-width = { version = "0.2", optional = true }

[dev-dependencies]
tempfile = "3"
ts-rs = "10"
```

- [ ] **Step 2: Copy the frozen tree and the runtime shim**

```bash
git -C "$VIMEFLOW" rev-parse --short HEAD   # must print 91e45b1c
mkdir -p src/git src/runtime src/filesystem
cp "$VIMEFLOW"/crates/backend/src/git/{mod.rs,watcher.rs,test_helpers.rs} src/git/
cp "$VIMEFLOW"/crates/backend/src/runtime/{event_sink.rs,test_event_sink.rs} src/runtime/
printf '%s\n' '/target' '.lifeline-planner/' '/bindings' > .gitignore
```

- [ ] **Step 3: Write `src/runtime/mod.rs`**

```rust
pub mod event_sink;

pub(crate) use event_sink::serialize_event;
pub use event_sink::EventSink;

#[cfg(any(test, feature = "e2e-test"))]
pub use event_sink::FakeEventSink;
```

- [ ] **Step 4: Write the D1 shim `src/filesystem/mod.rs` and `src/filesystem/scope.rs`**

`src/filesystem/mod.rs`:

```rust
pub mod scope;
```

`src/filesystem/scope.rs`. `expand_home`, `home_canonical` and `reject_parent_refs` are copied unchanged from `$VIMEFLOW/crates/backend/src/filesystem/scope.rs:22-63`; only `ensure_within_home` differs:

```rust
//! Port-surface shim for the frozen git tree. Policy differs from vimeflow: D1.
use std::fs;
use std::path::{Component, Path, PathBuf};

// expand_home, home_canonical, reject_parent_refs: copy verbatim from the pin.
// They use `dirs::home_dir`, `fs::canonicalize` and `Component`, hence the imports above.
// `open_nofollow` (pin line 196) is not part of the port surface and is not copied.

/// Accepts any canonical absolute path. vimeflow restricts to $HOME because its
/// cwd arrives over IPC; here it comes from the user's own pane or command line.
pub(crate) fn ensure_within_home(canonical: &Path, _home_canonical: &Path) -> Result<(), String> {
    if canonical.is_absolute() {
        Ok(())
    } else {
        Err(format!("path is not absolute: {}", canonical.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_canonical_path_outside_home() {
        let home = home_canonical().expect("home");
        assert!(ensure_within_home(Path::new("/tmp"), &home).is_ok());
    }

    #[test]
    fn still_rejects_parent_refs() {
        assert!(reject_parent_refs(Path::new("/tmp/../etc")).is_err());
    }

    #[test]
    fn still_expands_tilde() {
        let home = std::env::var("HOME").expect("HOME");
        assert_eq!(expand_home("~/x"), PathBuf::from(home).join("x"));
    }
}
```

- [ ] **Step 5: Write `src/lib.rs` and a stub `src/main.rs`**

```rust
#[cfg(not(unix))]
compile_error!("herdr-hunks targets Unix (macOS and Linux) only");

pub mod filesystem;
#[allow(dead_code)]
pub mod git;
pub mod runtime;
```

```rust
fn main() {
    eprintln!("herdr-hunks: not implemented yet");
    std::process::exit(2);
}
```

- [ ] **Step 6: Write `scripts/port-check.sh`**

```sh
#!/bin/sh
# Verifies src/git/ == pinned vimeflow sources + port/patches/*.patch, byte for byte.
set -eu
PIN=91e45b1c
src="${1:?usage: port-check.sh <vimeflow-checkout>}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
git -C "$src" cat-file -e "$PIN^{commit}" || { echo "pin $PIN not found in $src" >&2; exit 2; }
mkdir -p "$work/src/git"
for f in mod.rs watcher.rs test_helpers.rs; do
  git -C "$src" show "$PIN:crates/backend/src/git/$f" > "$work/src/git/$f"
done
for p in port/patches/*.patch; do
  [ -e "$p" ] || continue
  patch -s -d "$work" -p1 < "$p"
done
diff -ru "$work/src/git" src/git
echo "port-check: src/git matches $PIN + $(ls port/patches/*.patch 2>/dev/null | wc -l | tr -d ' ') patch(es)"
```

- [ ] **Step 7: Write `PORT-SURFACE.md`**

Sections, in this order: **Pin** (`vimeflow 91e45b1c`, the three copied files); **Port surface** (the two imports quoted in spec 2.2 item 2); **Shims** (`src/runtime/` byte-identical; `src/filesystem/scope.rs` with D1 and its reason, copied from spec 2.4); **Registered divergences** D1-D3 (D4, D5 are added by Task 2); **Known defects** K1-K5, copied from spec 2.4; **Copied from herdr-agent-watcher** (empty table with columns `file | source | adaptation`, filled by Tasks 9 and 14).

- [ ] **Step 8: Run the frozen tests and the port check**

Run: `chmod +x scripts/port-check.sh && cargo test && cargo check --no-default-features && scripts/port-check.sh "$VIMEFLOW"`
Expected: the frozen inline tests of `git::tests` and `git::watcher::tests` pass, the three scope tests pass, and the last line reads `port-check: src/git matches 91e45b1c + 0 patch(es)`.

Two frozen tests, `test_git_branch_rejects_out_of_scope_cwd` and `test_git_worktree_name_rejects_out_of_scope_cwd`, call the functions with `/etc` and expect an error. Under D1 they still pass, because `/etc` is not a git repository, no longer because of the scope check. Record that in `PORT-SURFACE.md` under D1. On a machine where `/etc` is a repository (etckeeper) they fail; that is a property of the machine, not a regression.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat: port vimeflow git module as a frozen subtree with shims"
```

---

### Task 2: Registered patches D4 and D5

Spec: 2.2 item 1, 2.4 D4 and D5.

**Files:**
- Create: `port/patches/0001-no-ext-diff.patch`, `port/patches/0002-drain-sync-output.patch`, `src/git_patches_tests.rs`
- Modify: `src/git/mod.rs`, `src/git/watcher.rs` (only by applying the patches), `src/lib.rs`, `PORT-SURFACE.md`

**Interfaces:**
- Consumes: Task 1's frozen tree; the `#[cfg(test)]` wrapper `crate::git::get_git_diff(cwd, file, staged, untracked)`.
- Produces: a frozen tree whose patch-producing diff calls pass `--no-ext-diff`, and whose `run_sync_with_timeout` drains child output while waiting.

- [ ] **Step 1: Write the failing D4 test in `src/git_patches_tests.rs`** and add `#[cfg(test)] mod git_patches_tests;` to `src/lib.rs`

```rust
use std::process::Command;

fn git(dir: &std::path::Path, args: &[&str]) {
    let ok = Command::new("git").arg("-C").arg(dir).args(args).status().expect("git").success();
    assert!(ok, "git {args:?} failed");
}

fn repo_with_external_diff() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let p = dir.path();
    git(p, &["init", "-q", "-b", "main"]);
    git(p, &["config", "user.email", "t@example.com"]);
    git(p, &["config", "user.name", "t"]);
    std::fs::write(p.join("a.txt"), "one\ntwo\n").unwrap();
    git(p, &["add", "-A"]);
    git(p, &["commit", "-q", "-m", "init"]);
    git(p, &["config", "diff.external", "/bin/echo"]);
    std::fs::write(p.join("a.txt"), "one\nTWO\n").unwrap();
    std::fs::write(p.join("new.txt"), "fresh\n").unwrap();
    dir
}

#[tokio::test]
async fn external_diff_does_not_replace_the_unified_diff() {
    let dir = repo_with_external_diff();
    let cwd = dir.path().to_string_lossy().to_string();
    let tracked = crate::git::get_git_diff(cwd.clone(), "a.txt".into(), false, None).await.unwrap();
    assert_eq!(tracked.file_diff.hunks.len(), 1, "tracked file must parse to its real hunk");
    let untracked = crate::git::get_git_diff(cwd, "new.txt".into(), false, Some(true)).await.unwrap();
    assert_eq!(untracked.file_diff.hunks.len(), 1, "untracked file must parse to its real hunk");
}
```

- [ ] **Step 2: Run it and confirm it fails**

Run: `cargo test external_diff_does_not_replace`
Expected: FAIL, `left: 0, right: 1`.

- [ ] **Step 3: Create patch 0001**

Edit `src/git/mod.rs` in a scratch copy only to generate the patch. In both patch-producing calls, add `.arg("--no-ext-diff")` directly after `.arg("--no-color")`: the untracked `--no-index` diff in `get_untracked_diff` (pin line 1343 region) and the main diff in `get_git_diff_inner` (pin line 1500 region). Leave the numstat and name-status calls alone.

```bash
git -C "$VIMEFLOW" show 91e45b1c:crates/backend/src/git/mod.rs > /tmp/pristine-mod.rs
# after editing src/git/mod.rs as described:
{ printf '%s\n' 'Reason: D4. diff.external / GIT_EXTERNAL_DIFF must not replace the unified diff.' \
    'No environment variable disables external diffs, so this cannot be a D3 item.' ''
  diff -u --label a/src/git/mod.rs --label b/src/git/mod.rs /tmp/pristine-mod.rs src/git/mod.rs; } \
  > port/patches/0001-no-ext-diff.patch || true
```

- [ ] **Step 4: Run the D4 test and the port check**

Run: `cargo test external_diff_does_not_replace && scripts/port-check.sh "$VIMEFLOW"`
Expected: PASS, and `port-check: src/git matches 91e45b1c + 1 patch(es)`.

- [ ] **Step 5: Write the failing D5 test inside `src/git/watcher.rs`'s test module** (it becomes part of patch 0002, because `run_sync_with_timeout` is private)

```rust
    #[test]
    fn run_sync_with_timeout_drains_output_larger_than_the_pipe_buffer() {
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg("head -c 1048576 /dev/zero | tr '\\0' 'x'");
        let started = std::time::Instant::now();
        let output = run_sync_with_timeout(cmd, Duration::from_secs(10)).expect("must not time out");
        assert_eq!(output.stdout.len(), 1_048_576);
        assert!(started.elapsed() < Duration::from_secs(5));
    }
```

Run: `cargo test run_sync_with_timeout_drains`
Expected: FAIL after about 10 s with `git command timed out`.

- [ ] **Step 6: Fix `run_sync_with_timeout` in `src/git/watcher.rs`**

Replace the body between `.spawn()` and the end of the function with reader threads that drain both pipes while the loop waits:

```rust
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(pipe) = stdout.as_mut() {
            let _ = std::io::Read::read_to_end(pipe, &mut buf);
        }
        buf
    });
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(pipe) = stderr.as_mut() {
            let _ = std::io::Read::read_to_end(pipe, &mut buf);
        }
        buf
    });

    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("git command timed out after {:?}", timeout));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("try_wait failed: {}", e));
            }
        }
    };
    Ok(std::process::Output {
        status,
        stdout: out_reader.join().unwrap_or_default(),
        stderr: err_reader.join().unwrap_or_default(),
    })
```

- [ ] **Step 7: Create patch 0002 and verify**

```bash
git -C "$VIMEFLOW" show 91e45b1c:crates/backend/src/git/watcher.rs > /tmp/pristine-watcher.rs
{ printf '%s\n' 'Reason: D5. run_sync_with_timeout read the pipes only after exit, so output' \
    'larger than the pipe buffer blocked the child until the timeout killed it.' ''
  diff -u --label a/src/git/watcher.rs --label b/src/git/watcher.rs /tmp/pristine-watcher.rs src/git/watcher.rs; } \
  > port/patches/0002-drain-sync-output.patch || true
cargo test && scripts/port-check.sh "$VIMEFLOW"
```

Expected: all tests pass, including both new ones, and `port-check: src/git matches 91e45b1c + 2 patch(es)`.

- [ ] **Step 8: Register D4 and D5 in `PORT-SURFACE.md`** (text from spec 2.4) and commit

```bash
git add -A
git commit -m "fix: register frozen-tree patches for external diff and pipe draining"
```

---

### Task 3: Process environment policy and the git version floor

Spec: 2.4 D3, 3.3 "Version check", 3.5.

**Files:**
- Create: `src/engine/mod.rs`, `src/engine/gitver.rs`
- Modify: `src/lib.rs` (add `pub mod engine;`)

**Interfaces:**
- Consumes: nothing.
- Produces: `engine::init_process_env()`, `engine::GIT_CHILD_ENV: [(&str, &str); 3]`, `engine::gitver::{GitVersion, MIN_GIT, GitCheckError::{Missing(String), TooOld(String)}, parse(&str) -> Option<GitVersion>, check() -> Result<GitVersion, GitCheckError>}`.

- [ ] **Step 1: Write the failing tests in `src/engine/gitver.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_version_strings() {
        assert_eq!(parse("git version 2.43.0\n"), Some(GitVersion { major: 2, minor: 43 }));
        assert_eq!(parse("git version 2.39.5 (Apple Git-154)"), Some(GitVersion { major: 2, minor: 39 }));
        assert_eq!(parse("git version 2.50.1.windows.1"), Some(GitVersion { major: 2, minor: 50 }));
        assert_eq!(parse("not git"), None);
    }

    #[test]
    fn floor_is_2_31() {
        assert!(GitVersion { major: 2, minor: 31 } >= MIN_GIT);
        assert!(GitVersion { major: 2, minor: 30 } < MIN_GIT);
        assert!(GitVersion { major: 3, minor: 0 } >= MIN_GIT);
    }
}
```

Run: `cargo test gitver` — Expected: FAIL (does not compile).

- [ ] **Step 2: Implement `src/engine/gitver.rs`**

```rust
//! `git --version` parsing and the supported floor.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct GitVersion {
    pub major: u32,
    pub minor: u32,
}

/// The frozen watcher needs `rev-parse --path-format=absolute` (git 2.31).
pub const MIN_GIT: GitVersion = GitVersion { major: 2, minor: 31 };

pub fn parse(output: &str) -> Option<GitVersion> {
    let rest = output.trim().strip_prefix("git version ")?;
    let mut parts = rest.split(|c: char| !c.is_ascii_digit());
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some(GitVersion { major, minor })
}

/// `Missing` is retryable (the user can fix PATH and press `r`); `TooOld` is fatal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitCheckError {
    Missing(String),
    TooOld(String),
}

pub fn check() -> Result<GitVersion, GitCheckError> {
    let output = std::process::Command::new("git")
        .arg("--version")
        .output()
        .map_err(|e| GitCheckError::Missing(format!("Failed to spawn git: {e}")))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let version = parse(&text).ok_or_else(|| GitCheckError::Missing(format!("unrecognised git version: {}", text.trim())))?;
    if version < MIN_GIT {
        return Err(GitCheckError::TooOld(format!(
            "git {}.{} is too old: herdr-hunks needs git {}.{} or newer",
            version.major, version.minor, MIN_GIT.major, MIN_GIT.minor
        )));
    }
    Ok(version)
}
```

- [ ] **Step 3: Write `src/engine/mod.rs` with the environment policy and its test**

```rust
//! UI-agnostic engine. No ratatui or crossterm types appear here.
pub mod gitver;

/// D3. The frozen tree spawns git itself, so policy is process-wide.
pub const GIT_CHILD_ENV: [(&str, &str); 3] = [
    ("GIT_OPTIONAL_LOCKS", "0"),
    ("GIT_LITERAL_PATHSPECS", "1"),
    ("GIT_NO_LAZY_FETCH", "1"),
];

/// Call once, before any thread starts. Embedders must call it too.
pub fn init_process_env() {
    for (key, value) in GIT_CHILD_ENV {
        std::env::set_var(key, value);
    }
    std::env::remove_var("GIT_EXTERNAL_DIFF");
}

#[cfg(test)]
mod tests {
    // `init_process_env` must run before any thread exists, so it is never called from
    // a unit test: libtest runs tests on threads, next to other tests that spawn git.
    // Its process-level effect is asserted in tests/env_policy.rs (Task 7) and
    // tests/readonly_guarantee.rs (Task 8), each of which is a process of its own.
    #[test]
    fn the_policy_names_the_three_variables() {
        let keys: Vec<&str> = super::GIT_CHILD_ENV.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, ["GIT_OPTIONAL_LOCKS", "GIT_LITERAL_PATHSPECS", "GIT_NO_LAZY_FETCH"]);
    }
}
```

- [ ] **Step 4: Run and commit**

Run: `cargo test engine && cargo check --no-default-features` — Expected: PASS.

```bash
git add -A
git commit -m "feat: add git child environment policy and version floor"
```

---

### Task 4: Navigation model (`engine::nav`)

Spec: 3.4. Source: `$VIMEFLOW/src/features/diff/hooks/useReviewTargetNavigation.ts:74-176,633-644,663-762`.

**Files:**
- Create: `src/engine/nav.rs`
- Modify: `src/engine/mod.rs` (add `pub mod nav;`)

**Interfaces:**
- Consumes: `crate::git::{FileDiff, DiffHunk, DiffLine, DiffLineType}` (fields: `DiffLine { line_type, content, old_line_number: Option<u32>, new_line_number: Option<u32> }`, `DiffHunk { id, header, old_start, old_lines, new_start, new_lines, lines }`).
- Produces: `Side`, `ViewMode`, `Target`, `targets_for_diff`, `unified_order`, `target_index_for_hunk`, `move_line`, `move_side`, `find`, `nearest` with the signatures below.

- [ ] **Step 1: Write the failing tests at the bottom of `src/engine/nav.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{DiffHunk, DiffLine, DiffLineType, FileDiff};

    fn line(kind: char) -> DiffLine {
        DiffLine {
            line_type: match kind { '+' => DiffLineType::Added, '-' => DiffLineType::Removed, _ => DiffLineType::Context },
            content: String::new(),
            old_line_number: None,
            new_line_number: None,
        }
    }

    fn diff(hunks: &[(u32, u32, &str)]) -> FileDiff {
        FileDiff {
            file_path: "f".into(),
            old_path: None,
            new_path: None,
            hunks: hunks
                .iter()
                .map(|(old_start, new_start, kinds)| DiffHunk {
                    id: format!("hunk-{old_start}-{new_start}"),
                    header: String::new(),
                    old_start: *old_start,
                    old_lines: kinds.chars().filter(|c| *c != '+').count() as u32,
                    new_start: *new_start,
                    new_lines: kinds.chars().filter(|c| *c != '-').count() as u32,
                    lines: kinds.chars().map(line).collect(),
                })
                .collect(),
        }
    }

    fn shape(t: &Target) -> (u32, Side, usize, usize, bool) {
        (t.line_number, t.side, t.hunk_index, t.split_row_index, t.changed)
    }

    // vimeflow fixture: one context line, then one added line.
    #[test]
    fn context_then_addition() {
        let targets = targets_for_diff(&diff(&[(1, 1, " +")]));
        assert_eq!(
            targets.iter().map(shape).collect::<Vec<_>>(),
            vec![(1, Side::Additions, 0, 0, false), (2, Side::Additions, 0, 1, true)]
        );
    }

    // Two deletions replaced by one addition: rows pair up, the extra deletion gets its own row.
    #[test]
    fn replacement_block_pairs_rows() {
        let targets = targets_for_diff(&diff(&[(10, 10, " --+ ")]));
        assert_eq!(
            targets.iter().map(shape).collect::<Vec<_>>(),
            vec![
                (10, Side::Additions, 0, 0, false),
                (11, Side::Deletions, 0, 1, true),
                (11, Side::Additions, 0, 1, true),
                (12, Side::Deletions, 0, 2, true),
                (12, Side::Additions, 0, 3, false),
            ]
        );
    }

    #[test]
    fn unified_order_puts_a_blocks_deletions_first() {
        let targets = targets_for_diff(&diff(&[(10, 10, " --+ ")]));
        assert_eq!(unified_order(&targets), vec![0, 1, 3, 2, 4]);
    }

    #[test]
    fn hunk_navigation_lands_on_the_first_changed_row() {
        let targets = targets_for_diff(&diff(&[(1, 1, "  +"), (20, 21, " - ")]));
        assert_eq!(target_index_for_hunk(&targets, 0), Some(2));
        assert_eq!(target_index_for_hunk(&targets, 1), Some(4));
        assert_eq!(target_index_for_hunk(&targets, 9), None);
    }

    #[test]
    fn move_line_unified_follows_display_order_and_clamps() {
        let targets = targets_for_diff(&diff(&[(10, 10, " --+ ")]));
        let order = unified_order(&targets);
        assert_eq!(move_line(&targets, &order, 1, 1, ViewMode::Unified), 3);
        assert_eq!(move_line(&targets, &order, 3, 1, ViewMode::Unified), 2);
        assert_eq!(move_line(&targets, &order, 0, -1, ViewMode::Unified), 0);
        assert_eq!(move_line(&targets, &order, 4, 1, ViewMode::Unified), 4);
    }

    #[test]
    fn move_line_split_treats_a_pair_as_one_row_and_keeps_the_side() {
        let targets = targets_for_diff(&diff(&[(10, 10, " --+ ")]));
        let order = unified_order(&targets);
        // from the deletion in row 1, down goes to the deletion in row 2
        assert_eq!(move_line(&targets, &order, 1, 1, ViewMode::Split), 3);
        // from the addition in row 1, down: row 2 has no addition, so land on its deletion
        assert_eq!(move_line(&targets, &order, 2, 1, ViewMode::Split), 3);
        // from row 0 down lands on the additions side of row 1
        assert_eq!(move_line(&targets, &order, 0, 1, ViewMode::Split), 2);
    }

    #[test]
    fn move_side_switches_within_a_row_in_split_mode_only() {
        let targets = targets_for_diff(&diff(&[(10, 10, " --+ ")]));
        assert_eq!(move_side(&targets, 1, Side::Additions, ViewMode::Split), 2);
        assert_eq!(move_side(&targets, 3, Side::Additions, ViewMode::Split), 3);
        assert_eq!(move_side(&targets, 1, Side::Additions, ViewMode::Unified), 1);
    }

    #[test]
    fn find_and_nearest() {
        let targets = targets_for_diff(&diff(&[(10, 10, " --+ ")]));
        assert_eq!(find(&targets, Side::Deletions, 12), Some(3));
        assert_eq!(find(&targets, Side::Deletions, 99), None);
        assert_eq!(nearest(&targets, Side::Deletions, 99), Some(3));
        assert_eq!(nearest(&[], Side::Additions, 1), None);
    }

    #[test]
    fn a_hunk_without_lines_yields_one_target() {
        let mut d = diff(&[(5, 5, "")]);
        d.hunks[0].new_lines = 0;
        let targets = targets_for_diff(&d);
        assert_eq!(targets.iter().map(shape).collect::<Vec<_>>(), vec![(5, Side::Deletions, 0, 0, true)]);
    }
}
```

Run: `cargo test nav` — Expected: FAIL (does not compile).

- [ ] **Step 2: Implement `src/engine/nav.rs` above the tests**

```rust
//! Pure navigation model, ported from vimeflow's useReviewTargetNavigation.
use crate::git::{DiffLineType, FileDiff};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    /// New-file line numbers.
    Additions,
    /// Old-file line numbers.
    Deletions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Unified,
    Split,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub line_number: u32,
    pub side: Side,
    pub hunk_index: usize,
    pub split_row_index: usize,
    pub changed: bool,
}

/// Row-ordered targets; a changed block is emitted pair-interleaved (split order).
pub fn targets_for_diff(file_diff: &FileDiff) -> Vec<Target> {
    let mut targets = Vec::new();
    for (hunk_index, hunk) in file_diff.hunks.iter().enumerate() {
        let mut old_line = hunk.old_start;
        let mut new_line = hunk.new_start;
        let mut split_row = 0usize;
        let mut deletions: Vec<Target> = Vec::new();
        let mut additions: Vec<Target> = Vec::new();

        if hunk.lines.is_empty() {
            let deleted_only = hunk.new_lines == 0;
            targets.push(Target {
                line_number: if deleted_only { hunk.old_start } else { hunk.new_start },
                side: if deleted_only { Side::Deletions } else { Side::Additions },
                hunk_index,
                split_row_index: 0,
                changed: true,
            });
            continue;
        }

        let mut flush = |targets: &mut Vec<Target>, split_row: &mut usize, deletions: &mut Vec<Target>, additions: &mut Vec<Target>| {
            let rows = deletions.len().max(additions.len());
            for offset in 0..rows {
                if let Some(d) = deletions.get(offset) {
                    targets.push(Target { split_row_index: *split_row + offset, ..d.clone() });
                }
                if let Some(a) = additions.get(offset) {
                    targets.push(Target { split_row_index: *split_row + offset, ..a.clone() });
                }
            }
            *split_row += rows;
            deletions.clear();
            additions.clear();
        };

        for line in &hunk.lines {
            match line.line_type {
                DiffLineType::Removed => {
                    let number = line.old_line_number.unwrap_or(old_line);
                    deletions.push(Target { line_number: number, side: Side::Deletions, hunk_index, split_row_index: 0, changed: true });
                }
                DiffLineType::Added => {
                    let number = line.new_line_number.unwrap_or(new_line);
                    additions.push(Target { line_number: number, side: Side::Additions, hunk_index, split_row_index: 0, changed: true });
                }
                DiffLineType::Context => {
                    flush(&mut targets, &mut split_row, &mut deletions, &mut additions);
                    let number = line.new_line_number.unwrap_or(new_line);
                    targets.push(Target { line_number: number, side: Side::Additions, hunk_index, split_row_index: split_row, changed: false });
                    split_row += 1;
                }
            }
            if !matches!(line.line_type, DiffLineType::Added) {
                old_line += 1;
            }
            if !matches!(line.line_type, DiffLineType::Removed) {
                new_line += 1;
            }
        }
        flush(&mut targets, &mut split_row, &mut deletions, &mut additions);
    }
    targets
}

/// Indices into `targets` in unified display order: a block's deletions, then its additions.
pub fn unified_order(targets: &[Target]) -> Vec<usize> {
    let mut order = Vec::with_capacity(targets.len());
    let mut i = 0;
    while i < targets.len() {
        if !targets[i].changed {
            order.push(i);
            i += 1;
            continue;
        }
        let hunk = targets[i].hunk_index;
        let mut end = i;
        while end < targets.len() && targets[end].changed && targets[end].hunk_index == hunk {
            end += 1;
        }
        order.extend((i..end).filter(|&k| targets[k].side == Side::Deletions));
        order.extend((i..end).filter(|&k| targets[k].side == Side::Additions));
        i = end;
    }
    order
}

pub fn target_index_for_hunk(targets: &[Target], hunk_index: usize) -> Option<usize> {
    targets
        .iter()
        .position(|t| t.hunk_index == hunk_index && t.changed)
        .or_else(|| targets.iter().position(|t| t.hunk_index == hunk_index))
}

/// `delta` is +1 or -1. Clamps at both ends. Returns the new cursor index.
pub fn move_line(targets: &[Target], unified: &[usize], cursor: usize, delta: i32, mode: ViewMode) -> usize {
    if targets.is_empty() || delta == 0 {
        return cursor;
    }
    let current = cursor.min(targets.len() - 1);
    let step = i64::from(delta.signum());
    match mode {
        ViewMode::Unified => {
            let pos = unified.iter().position(|&i| i == current).unwrap_or(0) as i64;
            let next = pos + step;
            if next < 0 || next >= unified.len() as i64 {
                current
            } else {
                unified[next as usize]
            }
        }
        ViewMode::Split => {
            let base = targets[current].clone();
            let len = targets.len() as i64;
            let mut row = current as i64;
            if row + step < 0 || row + step >= len {
                return current;
            }
            while row + step >= 0 && row + step < len {
                row += step;
                let t = &targets[row as usize];
                if t.hunk_index != base.hunk_index || t.split_row_index != base.split_row_index {
                    break;
                }
            }
            let landed = &targets[row as usize];
            targets
                .iter()
                .position(|t| t.hunk_index == landed.hunk_index && t.split_row_index == landed.split_row_index && t.side == base.side)
                .unwrap_or(row as usize)
        }
    }
}

pub fn move_side(targets: &[Target], cursor: usize, side: Side, mode: ViewMode) -> usize {
    if mode != ViewMode::Split || targets.is_empty() {
        return cursor;
    }
    let current = &targets[cursor.min(targets.len() - 1)];
    targets
        .iter()
        .position(|t| t.hunk_index == current.hunk_index && t.split_row_index == current.split_row_index && t.side == side)
        .unwrap_or(cursor)
}

pub fn find(targets: &[Target], side: Side, line_number: u32) -> Option<usize> {
    targets.iter().position(|t| t.side == side && t.line_number == line_number)
}

/// Same side, smallest line distance; used when a refresh removed the cursor's line.
pub fn nearest(targets: &[Target], side: Side, line_number: u32) -> Option<usize> {
    targets
        .iter()
        .enumerate()
        .filter(|(_, t)| t.side == side)
        .min_by_key(|(_, t)| t.line_number.abs_diff(line_number))
        .map(|(i, _)| i)
        .or(if targets.is_empty() { None } else { Some(0) })
}
```

- [ ] **Step 3: Run and commit**

Run: `cargo test nav` — Expected: PASS (9 tests).

```bash
git add -A
git commit -m "feat: add pure navigation model ported from vimeflow"
```

---

### Task 5: Engine types and the size cap

Spec: 3.2.

**Files:**
- Create: `src/engine/types.rs`
- Modify: `src/engine/mod.rs` (add `pub mod types; pub use types::*;`)

**Interfaces:**
- Consumes: `engine::nav::{Target, targets_for_diff, unified_order}`, `crate::git::{ChangedFile, FileDiff, GetGitDiffResponse}`.
- Produces: the types below; `LoadedDiff::build(key: FileKey, response: GetGitDiffResponse) -> LoadedDiff`; `MAX_DIFF_LINES: usize = 200_000`.

- [ ] **Step 1: Write the failing tests in `src/engine/types.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{DiffHunk, DiffLine, DiffLineType, FileDiff, GetGitDiffResponse};

    fn hunk(start: u32, lines: usize) -> DiffHunk {
        DiffHunk {
            id: format!("hunk-{start}-{start}"),
            header: String::new(),
            old_start: start,
            old_lines: 0,
            new_start: start,
            new_lines: lines as u32,
            lines: (0..lines)
                .map(|_| DiffLine { line_type: DiffLineType::Added, content: "x".into(), old_line_number: None, new_line_number: None })
                .collect(),
        }
    }

    fn response(hunks: Vec<DiffHunk>) -> GetGitDiffResponse {
        GetGitDiffResponse {
            file_diff: FileDiff { file_path: "f".into(), old_path: None, new_path: None, hunks },
            old_text: String::new(),
            new_text: String::new(),
            raw_diff: "raw".into(),
            repo_root: "/r".into(),
        }
    }

    fn key() -> FileKey {
        FileKey { path: "f".into(), staged: false }
    }

    #[test]
    fn a_small_diff_is_kept_whole() {
        let loaded = LoadedDiff::build_with_cap(key(), response(vec![hunk(1, 3), hunk(50, 2)]), 10);
        assert_eq!(loaded.file_diff.hunks.len(), 2);
        assert_eq!(loaded.truncated_lines, 0);
        assert_eq!(loaded.targets.len(), 5);
        assert_eq!(loaded.unified_order.len(), 5);
        assert_eq!(loaded.raw_diff, "raw");
    }

    #[test]
    fn whole_hunks_are_kept_while_they_fit() {
        let loaded = LoadedDiff::build_with_cap(key(), response(vec![hunk(1, 6), hunk(50, 6), hunk(90, 1)]), 10);
        assert_eq!(loaded.file_diff.hunks.len(), 1);
        assert_eq!(loaded.truncated_lines, 7);
        assert_eq!(loaded.targets.len(), 6);
    }

    #[test]
    fn an_oversized_first_hunk_keeps_its_prefix() {
        let loaded = LoadedDiff::build_with_cap(key(), response(vec![hunk(1, 25)]), 10);
        assert_eq!(loaded.file_diff.hunks.len(), 1);
        assert_eq!(loaded.file_diff.hunks[0].lines.len(), 10);
        assert_eq!(loaded.truncated_lines, 15);
        assert_eq!(loaded.targets.last().map(|t| t.line_number), Some(10));
    }
}
```

Run: `cargo test types` — Expected: FAIL (does not compile).

- [ ] **Step 2: Implement `src/engine/types.rs`**

```rust
//! Snapshot types the UI renders from. No terminal types here.
use std::sync::Arc;

use crate::engine::nav::{targets_for_diff, unified_order, Target};
use crate::git::{ChangedFile, FileDiff, GetGitDiffResponse};

pub const MAX_DIFF_LINES: usize = 200_000;

/// vimeflow's file identity: a partially staged path is two rows.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FileKey {
    pub path: String,
    pub staged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoState {
    Repo { toplevel: String, branch: Option<String>, worktree: Option<String> },
    NotARepo { cwd: String },
    Unusable { reason: String },
}

#[derive(Debug, Clone)]
pub enum DiffState {
    Idle,
    Loading,
    Ready(Arc<LoadedDiff>),
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct LoadedDiff {
    pub key: FileKey,
    pub file_diff: FileDiff,
    pub raw_diff: String,
    pub targets: Vec<Target>,
    pub unified_order: Vec<usize>,
    pub truncated_lines: usize,
}

impl LoadedDiff {
    pub fn build(key: FileKey, response: GetGitDiffResponse) -> Self {
        Self::build_with_cap(key, response, MAX_DIFF_LINES)
    }

    pub fn build_with_cap(key: FileKey, response: GetGitDiffResponse, cap: usize) -> Self {
        let total: usize = response.file_diff.hunks.iter().map(|h| h.lines.len()).sum();
        let mut file_diff = response.file_diff;
        let mut kept = 0usize;
        let mut hunks = Vec::new();
        for (index, mut hunk) in file_diff.hunks.drain(..).enumerate() {
            if kept + hunk.lines.len() <= cap {
                kept += hunk.lines.len();
                hunks.push(hunk);
            } else {
                if index == 0 {
                    hunk.lines.truncate(cap);
                    kept = hunk.lines.len();
                    hunks.push(hunk);
                }
                break;
            }
        }
        file_diff.hunks = hunks;
        let targets = targets_for_diff(&file_diff);
        let unified_order = unified_order(&targets);
        Self { key, file_diff, raw_diff: response.raw_diff, targets, unified_order, truncated_lines: total - kept }
    }
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub revision: u64,
    pub repo: RepoState,
    pub files: Vec<ChangedFile>,
    pub selected: Option<FileKey>,
    pub diff: DiffState,
    pub status_error: Option<String>,
    pub watcher_error: Option<String>,
    pub refreshing: bool,
}

impl Snapshot {
    pub fn empty(cwd: &str) -> Self {
        Self {
            revision: 0,
            repo: RepoState::NotARepo { cwd: cwd.to_string() },
            files: Vec::new(),
            selected: None,
            diff: DiffState::Idle,
            status_error: None,
            watcher_error: None,
            refreshing: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Select(FileKey),
    SelectNext,
    SelectPrev,
    Refresh,
    Shutdown,
}
```

- [ ] **Step 3: Run and commit**

Run: `cargo test types` — Expected: PASS.

```bash
git add -A
git commit -m "feat: add engine snapshot types and the diff size cap"
```

---

### Task 6: Engine session loop

Spec: 3.3, 3.5, 3.6, 2.4 D2.

**Files:**
- Create: `src/engine/session.rs`
- Modify: `src/engine/mod.rs` (add `pub mod session; pub use session::{spawn, EngineHandle, SessionConfig};`)

**Interfaces:**
- Consumes: Task 1's frozen functions, Task 3's `gitver::check`, Task 5's types.
- Produces:

```rust
pub type BoxFut<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>;

pub trait WatcherControl: Send + Sync + 'static {
    fn start(&self, cwd: String, sink: std::sync::Arc<dyn crate::runtime::EventSink>) -> BoxFut<Result<(), String>>;
    fn stop(&self, cwd: String) -> BoxFut<Result<(), String>>;
}

pub struct SessionConfig {
    pub path: std::path::PathBuf,
    pub poll_interval: std::time::Duration,      // 5 s in production
    pub watcher: std::sync::Arc<dyn WatcherControl>, // FrozenWatcher in production
    /// `gitver::check` in production. Injected so tests never touch the process PATH.
    pub git_check: std::sync::Arc<dyn Fn() -> Result<gitver::GitVersion, gitver::GitCheckError> + Send + Sync>,
    pub diff_delay: Option<std::time::Duration>, // None in production; tests delay diff requests with it
}

impl SessionConfig { pub fn production(path: std::path::PathBuf) -> Self }

pub struct EngineHandle {
    pub commands: tokio::sync::mpsc::UnboundedSender<Command>,
    pub snapshots: std::sync::mpsc::Receiver<std::sync::Arc<Snapshot>>,
    /// Status refreshes started by this session. Per session, so parallel tests cannot disturb it.
    pub refreshes: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

/// Spawns the loop on `runtime` and returns immediately.
pub fn spawn(runtime: &tokio::runtime::Handle, config: SessionConfig) -> EngineHandle;

```

- [ ] **Step 1: Write the failing tests in `src/engine/session.rs`**

The helpers build a fixture repo and a controllable watcher. Tests pass `git_check: ok_git()`, which always succeeds, so none of them depends on or changes the process `PATH`.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as Proc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn git(dir: &std::path::Path, args: &[&str]) {
        assert!(Proc::new("git").arg("-C").arg(dir).args(args).status().unwrap().success(), "git {args:?}");
    }

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        git(p, &["config", "user.email", "t@example.com"]);
        git(p, &["config", "user.name", "t"]);
        std::fs::write(p.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(p.join("b.txt"), "b\n").unwrap();
        git(p, &["add", "-A"]);
        git(p, &["commit", "-q", "-m", "init"]);
        std::fs::write(p.join("a.txt"), "one\nTWO\n").unwrap();
        std::fs::write(p.join("b.txt"), "B\n").unwrap();
        dir
    }

    fn ok_git() -> Arc<dyn Fn() -> Result<gitver::GitVersion, gitver::GitCheckError> + Send + Sync> {
        Arc::new(|| Ok(gitver::GitVersion { major: 2, minor: 99 }))
    }

    /// Fails to start until `allow` is set; never emits events.
    struct FlakyWatcher { allow: Arc<AtomicBool> }
    impl WatcherControl for FlakyWatcher {
        fn start(&self, _cwd: String, _sink: Arc<dyn crate::runtime::EventSink>) -> BoxFut<Result<(), String>> {
            let ok = self.allow.load(Ordering::SeqCst);
            Box::pin(async move { if ok { Ok(()) } else { Err("inotify limit".to_string()) } })
        }
        fn stop(&self, _cwd: String) -> BoxFut<Result<(), String>> { Box::pin(async { Ok(()) }) }
    }

    fn start(dir: &std::path::Path, allow: Arc<AtomicBool>) -> (tokio::runtime::Runtime, EngineHandle) {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let handle = spawn(rt.handle(), SessionConfig {
            path: dir.to_path_buf(),
            poll_interval: Duration::from_millis(50),
            watcher: Arc::new(FlakyWatcher { allow }),
            git_check: ok_git(),
            diff_delay: None,
        });
        (rt, handle)
    }

    fn wait_for(handle: &EngineHandle, what: &str, pred: impl Fn(&Snapshot) -> bool) -> Arc<Snapshot> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut last = None;
        while Instant::now() < deadline {
            while let Ok(s) = handle.snapshots.try_recv() { last = Some(s); }
            if let Some(s) = &last { if pred(s) { return s.clone(); } }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for: {what}; last = {last:?}");
    }

    fn ready(s: &Snapshot) -> Option<&LoadedDiff> {
        match &s.diff { DiffState::Ready(d) => Some(d), _ => None }
    }

    #[test]
    fn first_load_selects_the_first_row_and_loads_its_diff() {
        let dir = fixture();
        let (_rt, h) = start(dir.path(), Arc::new(AtomicBool::new(true)));
        let s = wait_for(&h, "ready diff", |s| ready(s).is_some());
        assert_eq!(s.files.len(), 2);
        assert_eq!(s.selected, Some(FileKey { path: "a.txt".into(), staged: false }));
        assert_eq!(ready(&s).unwrap().file_diff.hunks.len(), 1);
        assert!(matches!(s.repo, RepoState::Repo { .. }));
    }

    #[test]
    fn select_next_wraps_and_loads_the_other_file() {
        let dir = fixture();
        let (_rt, h) = start(dir.path(), Arc::new(AtomicBool::new(true)));
        wait_for(&h, "first", |s| ready(s).is_some());
        h.commands.send(Command::SelectNext).unwrap();
        wait_for(&h, "b selected", |s| ready(s).map(|d| d.key.path == "b.txt").unwrap_or(false));
        h.commands.send(Command::SelectNext).unwrap();
        wait_for(&h, "wrapped to a", |s| ready(s).map(|d| d.key.path == "a.txt").unwrap_or(false));
    }

    #[test]
    fn degraded_mode_converges_on_a_repeated_edit_and_recovers_on_refresh() {
        let dir = fixture();
        let allow = Arc::new(AtomicBool::new(false));
        let (_rt, h) = start(dir.path(), allow.clone());
        let s = wait_for(&h, "degraded", |s| s.watcher_error.is_some() && ready(s).is_some());
        let before = ready(&s).unwrap().raw_diff.clone();
        std::fs::write(dir.path().join("a.txt"), "one\nTWO AGAIN\n").unwrap();
        wait_for(&h, "poll picks up the second edit", |s| ready(s).map(|d| d.raw_diff != before).unwrap_or(false));
        git(dir.path(), &["stash", "-q"]);
        git(dir.path(), &["switch", "-q", "-c", "other"]);
        wait_for(&h, "branch through the tick", |s| matches!(&s.repo, RepoState::Repo { branch: Some(b), .. } if b == "other"));
        allow.store(true, Ordering::SeqCst);
        h.commands.send(Command::Refresh).unwrap();
        wait_for(&h, "watcher recovered", |s| s.watcher_error.is_none());
    }

    #[test]
    fn an_unchanged_repository_publishes_nothing_more() {
        let dir = fixture();
        let (_rt, h) = start(dir.path(), Arc::new(AtomicBool::new(false)));
        let s = wait_for(&h, "settled", |s| ready(s).is_some() && s.watcher_error.is_some());
        std::thread::sleep(Duration::from_millis(400)); // several poll ticks
        let mut latest = s.revision;
        while let Ok(n) = h.snapshots.try_recv() { latest = n.revision; assert!(ready(&n).is_some(), "diff must stay Ready"); }
        assert_eq!(latest, s.revision, "no publication on an unchanged repository");
    }

    #[test]
    fn a_directory_that_is_not_a_repository_reports_not_a_repo() {
        let dir = tempfile::tempdir().unwrap();
        let (_rt, h) = start(dir.path(), Arc::new(AtomicBool::new(true)));
        wait_for(&h, "not a repo", |s| matches!(s.repo, RepoState::NotARepo { .. }) && s.revision > 0);
        git(dir.path(), &["init", "-q", "-b", "main"]);
        h.commands.send(Command::Refresh).unwrap();
        wait_for(&h, "upgraded", |s| matches!(s.repo, RepoState::Repo { .. }));
    }

    #[test]
    fn a_diff_is_never_published_for_a_deselected_row() {
        let dir = fixture();
        let (_rt, h) = start(dir.path(), Arc::new(AtomicBool::new(true)));
        wait_for(&h, "first", |s| ready(s).is_some());
        for _ in 0..6 {
            h.commands.send(Command::SelectNext).unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            while let Ok(s) = h.snapshots.try_recv() {
                if let Some(d) = ready(&s) {
                    assert_eq!(Some(&d.key), s.selected.as_ref(), "published a diff for a row that is not selected");
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Starts fine and hands the sink to the test, so it can emit watcher events.
    struct EmittingWatcher { sink: Arc<std::sync::Mutex<Option<Arc<dyn crate::runtime::EventSink>>>> }
    impl WatcherControl for EmittingWatcher {
        fn start(&self, _cwd: String, sink: Arc<dyn crate::runtime::EventSink>) -> BoxFut<Result<(), String>> {
            *self.sink.lock().unwrap() = Some(sink);
            Box::pin(async { Ok(()) })
        }
        fn stop(&self, _cwd: String) -> BoxFut<Result<(), String>> { Box::pin(async { Ok(()) }) }
    }

    #[test]
    fn a_burst_of_watcher_events_is_coalesced() {
        let dir = fixture();
        let slot = Arc::new(std::sync::Mutex::new(None));
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let h = spawn(rt.handle(), SessionConfig {
            path: dir.path().to_path_buf(),
            poll_interval: Duration::from_secs(3600),
            watcher: Arc::new(EmittingWatcher { sink: slot.clone() }),
            git_check: ok_git(),
            diff_delay: None,
        });
        wait_for(&h, "first", |s| ready(s).is_some());
        let sink = wait_until(|| slot.lock().unwrap().clone());
        let before = h.refreshes.load(Ordering::SeqCst);
        for _ in 0..20 {
            sink.emit_json("git-status-changed", serde_json::json!({ "cwds": [] })).unwrap();
        }
        std::thread::sleep(Duration::from_millis(1500));
        let runs = h.refreshes.load(Ordering::SeqCst) - before;
        assert!((1..=3).contains(&runs), "20 events must coalesce into at most 3 refreshes, got {runs}");
    }

    #[test]
    fn a_slow_diff_is_not_starved_by_frequent_polls() {
        let dir = fixture();
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        // the watcher never starts, so every 50 ms tick refreshes while each diff takes 400 ms
        let h = spawn(rt.handle(), SessionConfig {
            path: dir.path().to_path_buf(),
            poll_interval: Duration::from_millis(50),
            watcher: Arc::new(FlakyWatcher { allow: Arc::new(AtomicBool::new(false)) }),
            git_check: ok_git(),
            diff_delay: Some(Duration::from_millis(400)),
        });
        wait_for(&h, "a diff despite constant polling", |s| ready(s).is_some());
    }

    #[test]
    fn refresh_without_a_selected_row_clears_the_busy_flag() {
        let dir = fixture();
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "clean"]);
        let (_rt, h) = start(dir.path(), Arc::new(AtomicBool::new(true)));
        wait_for(&h, "clean repository", |s| matches!(s.repo, RepoState::Repo { .. }) && s.files.is_empty());
        h.commands.send(Command::Refresh).unwrap();
        wait_for(&h, "busy", |s| s.refreshing);
        wait_for(&h, "idle again", |s| !s.refreshing);
    }

    /// Takes 300 ms to start and counts calls.
    struct SlowWatcher { starts: Arc<std::sync::atomic::AtomicUsize>, stops: Arc<std::sync::atomic::AtomicUsize>, fail_first: AtomicBool }
    impl WatcherControl for SlowWatcher {
        fn start(&self, _cwd: String, _sink: Arc<dyn crate::runtime::EventSink>) -> BoxFut<Result<(), String>> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            let fail = self.fail_first.swap(false, Ordering::SeqCst);
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                if fail { Err("first start fails".to_string()) } else { Ok(()) }
            })
        }
        fn stop(&self, _cwd: String) -> BoxFut<Result<(), String>> {
            self.stops.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }
    }

    #[test]
    fn a_pending_watcher_start_is_not_repeated_and_is_stopped_exactly_once() {
        let dir = fixture();
        let starts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let stops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let h = spawn(rt.handle(), SessionConfig {
            path: dir.path().to_path_buf(),
            poll_interval: Duration::from_secs(3600),
            watcher: Arc::new(SlowWatcher { starts: starts.clone(), stops: stops.clone(), fail_first: AtomicBool::new(false) }),
            git_check: ok_git(),
            diff_delay: None,
        });
        for _ in 0..3 {
            h.commands.send(Command::Refresh).unwrap(); // arrives while the first start is pending
        }
        h.commands.send(Command::Shutdown).unwrap();     // also while it is pending
        std::thread::sleep(Duration::from_millis(1200));
        assert_eq!(starts.load(Ordering::SeqCst), 1, "a pending start must not be repeated");
        assert_eq!(stops.load(Ordering::SeqCst), 1, "a registered watcher must be stopped exactly once");
    }

    #[test]
    fn a_missing_git_is_retryable_and_an_old_git_is_fatal() {
        let dir = fixture();
        let fixed = Arc::new(AtomicBool::new(false));
        let flag = fixed.clone();
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let h = spawn(rt.handle(), SessionConfig {
            path: dir.path().to_path_buf(),
            poll_interval: Duration::from_millis(50),
            watcher: Arc::new(FlakyWatcher { allow: Arc::new(AtomicBool::new(true)) }),
            git_check: Arc::new(move || {
                if flag.load(Ordering::SeqCst) {
                    Ok(gitver::GitVersion { major: 2, minor: 99 })
                } else {
                    Err(gitver::GitCheckError::Missing("Failed to spawn git: not found".into()))
                }
            }),
            diff_delay: None,
        });
        let s = wait_for(&h, "missing git reported", |s| s.status_error.is_some());
        assert!(!matches!(s.repo, RepoState::Unusable { .. }), "a missing git must stay retryable");
        fixed.store(true, Ordering::SeqCst);
        h.commands.send(Command::Refresh).unwrap();
        wait_for(&h, "recovered", |s| s.status_error.is_none() && ready(s).is_some());

        let old = spawn(rt.handle(), SessionConfig {
            path: dir.path().to_path_buf(),
            poll_interval: Duration::from_millis(50),
            watcher: Arc::new(FlakyWatcher { allow: Arc::new(AtomicBool::new(true)) }),
            git_check: Arc::new(|| Err(gitver::GitCheckError::TooOld("git 2.20 is too old".into()))),
            diff_delay: None,
        });
        wait_for(&old, "unusable", |s| matches!(&s.repo, RepoState::Unusable { reason } if reason.contains("2.20")));
    }

    #[test]
    fn rapid_selection_does_not_wait_for_superseded_diffs() {
        let dir = fixture();
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let h = spawn(rt.handle(), SessionConfig {
            path: dir.path().to_path_buf(),
            poll_interval: Duration::from_secs(3600),
            watcher: Arc::new(FlakyWatcher { allow: Arc::new(AtomicBool::new(true)) }),
            git_check: ok_git(),
            diff_delay: Some(Duration::from_millis(400)),
        });
        wait_for(&h, "first", |s| ready(s).is_some());
        let started = Instant::now();
        for _ in 0..5 {
            h.commands.send(Command::SelectNext).unwrap(); // a -> b -> a -> b -> a -> b
        }
        wait_for(&h, "the last selection loaded", |s| {
            s.selected.as_ref().map(|k| k.path == "b.txt").unwrap_or(false) && ready(s).map(|d| d.key.path == "b.txt").unwrap_or(false)
        });
        // Back to back, five delayed requests would take at least 2 s.
        assert!(started.elapsed() < Duration::from_millis(1500), "superseded diffs were waited for: {:?}", started.elapsed());
    }

    fn wait_until<T>(mut f: impl FnMut() -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(v) = f() { return v; }
            assert!(Instant::now() < deadline, "timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_missing_path_is_unusable() {
        let (_rt, h) = start(std::path::Path::new("/definitely/not/here"), Arc::new(AtomicBool::new(true)));
        wait_for(&h, "unusable", |s| matches!(s.repo, RepoState::Unusable { .. }));
    }
}
```

Run: `cargo test session` — Expected: FAIL (does not compile).

- [ ] **Step 2: Implement the session**

Structure of `src/engine/session.rs`, top to bottom:

1. `BoxFut`, `WatcherControl`, `SessionConfig`, `EngineHandle` as in **Interfaces**.
2. `FrozenWatcher { state: crate::git::watcher::GitWatcherState }` implementing `WatcherControl` by calling `crate::git::watcher::start_git_watcher_backend(cwd, sink, self.state.clone())` and `stop_git_watcher_backend(cwd, self.state.clone())`. `SessionConfig::production(path)` uses it with `poll_interval = 5 s`, `git_check = Arc::new(gitver::check)` and `diff_delay = None`.
3. `ChannelSink { tx: tokio::sync::mpsc::UnboundedSender<Trigger> }` implementing `crate::runtime::EventSink`:

```rust
enum Trigger { Status, Head }

impl crate::runtime::EventSink for ChannelSink {
    fn emit_json(&self, event: &str, _payload: serde_json::Value) -> Result<(), String> {
        let trigger = match event {
            "git-status-changed" => Trigger::Status,
            "git-head-changed" => Trigger::Head,
            _ => return Ok(()),
        };
        self.tx.send(trigger).map_err(|e| e.to_string())
    }
}
```

4. The loop state and the single publishing function. Equality is by fingerprint, because the frozen types derive `Serialize` but not `PartialEq`:

```rust
enum WatcherPhase { Starting, Running, Failed }

struct State {
    snapshot: Snapshot,                 // the last published value
    branch: Option<String>,             // retained between status results
    worktree: Option<String>,
    last_watcher_refresh: std::time::Instant,
    watcher: WatcherPhase,
    git_missing: bool,                  // set while the git check reports Missing
    status_in_flight: bool,
    status_dirty: bool,                 // a trigger arrived while a status task was running
    status_dirty_head: bool,
    diff_generation: u64,               // bumped only when a different key is requested
    diff_in_flight: Option<(u64, FileKey)>,
    diff_dirty: bool,                   // a refresh wanted the same key while it was loading
}

fn fingerprint(s: &Snapshot) -> String {
    let diff = match &s.diff {
        DiffState::Idle => "idle".to_string(),
        DiffState::Loading => "loading".to_string(),
        DiffState::Failed(e) => format!("failed:{e}"),
        DiffState::Ready(d) => format!("ready:{}:{}:{}", d.key.path, d.key.staged, d.raw_diff),
    };
    format!(
        "{:?}|{}|{:?}|{}|{:?}|{:?}|{}",
        s.repo,
        serde_json::to_string(&s.files).unwrap_or_default(),
        s.selected,
        diff,
        s.status_error,
        s.watcher_error,
        s.refreshing
    )
}

fn publish(state: &mut State, next: Snapshot, out: &std::sync::mpsc::Sender<std::sync::Arc<Snapshot>>) {
    if state.snapshot.revision > 0 && fingerprint(&state.snapshot) == fingerprint(&next) {
        return;
    }
    let mut next = next;
    next.revision = state.snapshot.revision + 1;
    state.snapshot = next.clone();
    let _ = out.send(std::sync::Arc::new(next));
}
```

5. `async fn run(config, commands_rx, snapshots_tx, refreshes)`. Git never runs inline in the loop. It runs in spawned tasks that report on an internal `results` channel, so the loop keeps serving commands while git is busy; this is what makes the spec's generation rule observable.

   ```rust
   enum Done {
       Watcher(Result<(), String>),
       Status { response: Result<crate::git::GitStatusResponse, String>, head: Option<(Option<String>, Option<String>)> },
       Diff { generation: u64, key: FileKey, result: Result<crate::git::GetGitDiffResponse, String> },
   }
   ```

   - Canonicalize `config.path`. If that fails or it is not a directory, publish `RepoState::Unusable { reason }` and return.
   - Run `(config.git_check)()` inside `tokio::task::spawn_blocking`. `Err(TooOld(reason))` publishes `Unusable` and returns. `Err(Missing(reason))` is not fatal: publish `status_error = Some(reason)`, set `git_missing = true`, and keep looping without starting the watcher or any git task; ticks do nothing while `git_missing`. `Command::Refresh` runs the check again, and on success clears `git_missing`, starts the watcher and calls `request_status(true)`.
   - Create the trigger channel and `ChannelSink`. Spawn the watcher start as a task that sends `Done::Watcher`; the first refresh does not wait for it.
   - `request_status(with_head)`: if `status_in_flight`, set `status_dirty = true` (and remember `with_head`); otherwise set `status_in_flight = true`, `refreshes.fetch_add(1, SeqCst)`, and spawn a task that runs `git_status_inner`, plus `git_branch_inner` and `git_worktree_name_inner` when `with_head`, and sends `Done::Status`.
   - `request_diff(key)`: if `diff_in_flight` already holds this `key`, set `diff_dirty = true` and return; a background refresh must never restart a diff that is still loading, or frequent polls would bump the generation faster than a slow diff can finish and every result would be discarded. Otherwise `diff_generation += 1`, `diff_in_flight = Some((diff_generation, key.clone()))`, and spawn a task that sleeps `config.diff_delay` when set, runs `get_git_diff_inner(cwd, key.path, key.staged, Some(untracked))`, and sends `Done::Diff { generation, key, result }`. Only a request for a different key supersedes the one in flight.
   - Start with `request_status(true)`, then loop on `tokio::select!` over `commands_rx`, the trigger receiver, `tokio::time::interval(config.poll_interval)` and `results`.
   - A trigger: drain the trigger channel with `try_recv` first (a burst becomes one request), call `request_status(any_head)`, set `last_watcher_refresh = now`.
   - The tick (D2): when the watcher is not `Running`, `request_status(true)` every tick; otherwise `request_status(false)` only when `last_watcher_refresh.elapsed() >= config.poll_interval`. `request_status` and `request_diff` both coalesce, so a tick during a slow refresh costs nothing.
   - `Done::Watcher(Ok)` sets `watcher = Running` and clears `watcher_error`; `Err(e)` sets `watcher = Failed` and `watcher_error = Some(e)`. Publish either way.
   - `Done::Status`: apply the status rules of item 6 and clear `status_in_flight`. If a row is selected and the status succeeded, `request_diff(selected)`; otherwise nothing more will arrive for this refresh, so set `refreshing = false`. Publish. If `status_dirty`, clear it and `request_status(status_dirty_head)` again.
   - `Done::Diff { generation, key, result }`: if `generation != diff_generation`, discard it; a newer request for another key is in flight and will report itself. Otherwise clear `diff_in_flight`; if `Some(&key) == snapshot.selected.as_ref()`, apply the diff rules of item 6, set `refreshing = false` and publish; then, if `diff_dirty`, clear it and `request_diff(selected)` once more.
   - `Command::Refresh`: if `watcher` is `Failed`, set it to `Starting` and spawn the start again; never while it is `Starting` or `Running`, because the frozen watcher counts every start as a subscription. Set `refreshing = true`, publish, `request_status(true)`.
   - `Command::Select(key)`, `SelectNext`, `SelectPrev` (wrapping with `rem_euclid`): publish `selected = key, diff = Loading` at once, then `request_diff(key)`. The loop does not wait, so five quick selections start five tasks and only the last result is applied.
   - `Command::Shutdown` or a closed command channel: if `watcher` is `Starting`, first wait for its `Done::Watcher` on `results` (at most 5 s) so a registration that lands late is not leaked; then call `watcher.stop(cwd)` exactly once if it is `Running`, and return.

6. The rules applied when results arrive:

```rust
// Done::Status
match response {
    Err(e) => next.status_error = Some(e), // next.files keeps the last good list
    Ok(resp) => {
        next.status_error = None;
        if let Some((branch, worktree)) = head { state.branch = branch; state.worktree = worktree; }
        next.repo = if resp.repo_root.is_empty() {
            RepoState::NotARepo { cwd: cwd.clone() }
        } else {
            RepoState::Repo { toplevel: resp.repo_root.clone(), branch: state.branch.clone(), worktree: state.worktree.clone() }
        };
        let old_index = next.files.iter().position(|f| Some(key_of(f)) == next.selected);
        next.files = resp.files;
        // same key if still listed, else the same index clamped, else the first row, else none
        next.selected = next.selected.clone().filter(|k| next.files.iter().any(|f| &key_of(f) == k))
            .or_else(|| old_index.and_then(|i| next.files.get(i.min(next.files.len().saturating_sub(1))).map(key_of)))
            .or_else(|| next.files.first().map(key_of));
        // Loading only when there is nothing to show for the selected key
        let same_key = matches!(&next.diff, DiffState::Ready(d) if Some(&d.key) == next.selected.as_ref());
        if !same_key {
            next.diff = if next.selected.is_some() { DiffState::Loading } else { DiffState::Idle };
        }
    }
}

// Done::Diff, after the generation and selection guard
match result {
    Ok(resp) => {
        let unchanged = matches!(&next.diff, DiffState::Ready(d) if d.key == key && d.raw_diff == resp.raw_diff);
        if !unchanged {
            next.diff = DiffState::Ready(Arc::new(LoadedDiff::build(key, resp)));
        }
    }
    Err(e) => next.diff = DiffState::Failed(e),
}
```

`key_of(f: &ChangedFile) -> FileKey { FileKey { path: f.path.clone(), staged: f.staged } }`. `untracked` for a diff request is `files.iter().any(|f| key_of(f) == key && matches!(f.status, ChangedFileStatus::Untracked))`.

7. `pub fn spawn(runtime, config)` creates the two channels and the `refreshes` counter, calls `runtime.spawn(run(...))` and returns the handle.

- [ ] **Step 3: Run the tests**

Run: `cargo test session -- --test-threads=1`
Expected: PASS (13 tests). They also pass under plain `cargo test`, because nothing in them is process-global. They spawn real git processes; single-threaded keeps timing stable.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat: add engine session loop with live refresh and degraded mode"
```

---

### Task 7: Adapted vimeflow diff tests and the pathspec fixture

Spec: 5.2 items 2 and 4 (literal pathspecs), 1.5 criterion 1.

**Files:**
- Create: `src/git_diff_response_tests.rs`, `tests/env_policy.rs`
- Modify: `src/lib.rs` (add `#[cfg(test)] mod git_diff_response_tests;`)

**Interfaces:**
- Consumes: the `#[cfg(test)]` wrappers `crate::git::get_git_diff` and `crate::git::git_status`; for `tests/env_policy.rs`, the public engine API.
- Produces: nothing for later tasks.

- [ ] **Step 1: Port the cases**

`$VIMEFLOW/crates/backend/tests/git_diff_response.rs` holds 15 synchronous `#[test]` cases. None of them touches `BackendState` directly: every request goes through one helper, `diff_value(state, repo, file, staged, untracked)`, which blocks on a private tokio runtime. So the port is mechanical:

1. Copy the whole file to `src/git_diff_response_tests.rs`.
2. Delete `use vimeflow_lib::runtime::{BackendState, EventSink};`, the `NullEventSink` type and `make_state()`.
3. Replace `diff_value` with the version below. Its signature loses the `state` parameter.
4. In each of the 15 tests delete the `let (state, _app_data) = make_state();` line and the `&state,` argument of every `diff_value` call. Change nothing else: fixtures, helper functions and assertions stay byte for byte.

```rust
fn diff_value(repo: &Path, file: &str, staged: bool, untracked: Option<bool>) -> Value {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let cwd = repo.to_string_lossy().to_string();
    let response = runtime
        .block_on(crate::git::get_git_diff(cwd, file.to_string(), staged, untracked))
        .expect("get_git_diff failed");
    serde_json::to_value(&response).expect("encode response")
}
```

Register the change in `PORT-SURFACE.md` under a heading **Adapted tests**. Then prove that all 15 were discovered, so the two tests added in Step 2 cannot mask a missing port:

Run: `cargo test git_diff_response_tests -- --list | grep -c ': test$'`
Expected: `15`.

- [ ] **Step 2: Add the criterion-1 fixture to the same file, and the literal-pathspec test to its own process**

Unit tests never call `init_process_env`: it must run before any thread exists, and libtest runs tests on threads beside other tests that spawn git. The literal-pathspec case needs the variable, so it lives in `tests/env_policy.rs`, an integration test with exactly one `#[test]`, which initializes the environment first and only then builds a runtime.

```rust
fn git(dir: &std::path::Path, args: &[&str]) {
    assert!(std::process::Command::new("git").arg("-C").arg(dir).args(args).status().unwrap().success(), "git {args:?}");
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-q", "-b", "main"]);
    git(p, &["config", "user.email", "t@example.com"]);
    git(p, &["config", "user.name", "t"]);
    dir
}

#[tokio::test]
async fn status_rows_cover_the_criterion_one_fixture() {
    let dir = repo();
    let p = dir.path();
    for (name, body) in [("mm.txt", "1\n2\n3\n4\n5\n6\n7\n8\n"), ("del.txt", "d\n"), ("ren.txt", "r1\nr2\nr3\n")] {
        std::fs::write(p.join(name), body).unwrap();
    }
    git(p, &["add", "-A"]);
    git(p, &["commit", "-q", "-m", "init"]);
    std::fs::write(p.join("mm.txt"), "ONE\n2\n3\n4\n5\n6\n7\n8\n").unwrap();
    git(p, &["add", "mm.txt"]);
    std::fs::write(p.join("mm.txt"), "ONE\n2\n3\n4\n5\n6\n7\nEIGHT\n").unwrap(); // MM
    std::fs::write(p.join("am.txt"), "a\n").unwrap();
    git(p, &["add", "am.txt"]);
    std::fs::write(p.join("am.txt"), "a\nb\n").unwrap(); // AM
    git(p, &["rm", "-q", "del.txt"]); // staged deletion
    git(p, &["mv", "ren.txt", "renamed.txt"]); // staged rename
    std::fs::create_dir_all(p.join("newdir/deep")).unwrap();
    std::fs::write(p.join("newdir/deep/u.txt"), "u\n").unwrap(); // nested untracked

    let status = crate::git::git_status(p.to_string_lossy().into()).await.unwrap();
    let rows: Vec<(String, bool)> = status.files.iter().map(|f| (f.path.clone(), f.staged)).collect();
    for expected in [("mm.txt", true), ("mm.txt", false), ("am.txt", true), ("am.txt", false), ("del.txt", true), ("renamed.txt", true), ("newdir/deep/u.txt", false)] {
        assert!(rows.contains(&(expected.0.to_string(), expected.1)), "missing row {expected:?} in {rows:?}");
    }
    let untracked = crate::git::get_git_diff(p.to_string_lossy().into(), "newdir/deep/u.txt".into(), false, Some(true)).await.unwrap();
    assert_eq!(untracked.file_diff.hunks.len(), 1);
}
```

`tests/env_policy.rs`:

```rust
use herdr_hunks::engine::{init_process_env, spawn, Command, DiffState, FileKey, SessionConfig, GIT_CHILD_ENV};
use std::time::{Duration, Instant};

fn git(dir: &std::path::Path, args: &[&str]) {
    assert!(std::process::Command::new("git").arg("-C").arg(dir).args(args).status().unwrap().success(), "git {args:?}");
}

// The only test in this binary, so nothing else reads the environment while it is set.
#[test]
fn the_environment_policy_is_applied_and_makes_pathspecs_literal() {
    init_process_env();
    for (key, value) in GIT_CHILD_ENV {
        assert_eq!(std::env::var(key).as_deref(), Ok(value));
    }
    assert!(std::env::var_os("GIT_EXTERNAL_DIFF").is_none());

    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-q", "-b", "main"]);
    git(p, &["config", "user.email", "t@example.com"]);
    git(p, &["config", "user.name", "t"]);
    std::fs::write(p.join("a*.txt"), "star\n").unwrap();
    std::fs::write(p.join("ab.txt"), "plain\n").unwrap();
    git(p, &["add", "-A"]);
    git(p, &["commit", "-q", "-m", "init"]);
    std::fs::write(p.join("a*.txt"), "STAR\n").unwrap();
    std::fs::write(p.join("ab.txt"), "PLAIN\n").unwrap();

    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let handle = spawn(rt.handle(), SessionConfig::production(p.to_path_buf()));
    handle.commands.send(Command::Select(FileKey { path: "a*.txt".into(), staged: false })).unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        assert!(Instant::now() < deadline, "the glob-named file never loaded");
        if let Ok(s) = handle.snapshots.recv_timeout(Duration::from_millis(200)) {
            if let DiffState::Ready(d) = &s.diff {
                if d.key.path == "a*.txt" {
                    assert!(d.raw_diff.contains("STAR"), "its own change is missing");
                    assert!(!d.raw_diff.contains("PLAIN"), "a neighbouring file leaked into the diff");
                    assert_eq!(d.file_diff.hunks.len(), 1);
                    break;
                }
            }
        }
    }
}
```

- [ ] **Step 3: Run and commit**

Run: `cargo test --test env_policy` — Expected: PASS.
Run: `cargo test git_diff_response_tests -- --list | grep -c ': test$'` — Expected: `16`.
Run: `cargo test git_diff_response_tests` — Expected: PASS.

```bash
git add -A
git commit -m "test: port vimeflow diff-response cases and add status fixtures"
```

---

### Task 8: Read-only guarantee test (G7)

Spec: 1.3 G7, 5.2 item 8.

**Files:**
- Create: `tests/readonly_guarantee.rs`

**Interfaces:**
- Consumes: the public engine API (`herdr_hunks::engine::{init_process_env, spawn, SessionConfig, Command, DiffState}`).
- Produces: nothing for later tasks.

- [ ] **Step 1: Write the test**

The test is alone in its integration-test binary because it rewrites `PATH` for the whole process.

```rust
use herdr_hunks::engine::{init_process_env, spawn, Command, DiffState, SessionConfig};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as Proc;
use std::time::{Duration, Instant};

const ALLOWED: [&str; 8] = ["--version", "rev-parse", "status", "diff", "ls-files", "show", "cat-file", "symbolic-ref"];

fn real_git() -> PathBuf {
    let out = Proc::new("sh").arg("-c").arg("command -v git").output().unwrap();
    PathBuf::from(String::from_utf8(out.stdout).unwrap().trim())
}

fn git(real: &Path, dir: &Path, args: &[&str]) -> String {
    let out = Proc::new(real).arg("-C").arg(dir).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// Content hash of every file outside `.git`, in path order. Pure Rust, so it is the same on macOS.
fn tree_hash(dir: &Path) -> String {
    use sha2::{Digest, Sha256};
    fn walk(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir).expect("read_dir").map(|e| e.expect("entry").path()).collect();
        entries.sort();
        for path in entries {
            if path.strip_prefix(root).map(|p| p.starts_with(".git")).unwrap_or(false) {
                continue;
            }
            if path.is_dir() { walk(&path, root, out) } else { out.push(path) }
        }
    }
    let mut files = Vec::new();
    walk(dir, dir, &mut files);
    assert!(!files.is_empty(), "nothing was hashed");
    let mut hasher = Sha256::new();
    for file in files {
        hasher.update(file.strip_prefix(dir).unwrap().to_string_lossy().as_bytes());
        hasher.update(std::fs::read(&file).expect("read file"));
    }
    format!("{:x}", hasher.finalize())
}

#[test]
fn the_engine_never_mutates_the_repository() {
    let real = real_git();
    let repo = tempfile::tempdir().unwrap();
    let p = repo.path();
    git(&real, p, &["init", "-q", "-b", "main"]);
    git(&real, p, &["config", "user.email", "t@example.com"]);
    git(&real, p, &["config", "user.name", "t"]);
    std::fs::write(p.join("a.txt"), "one\n").unwrap();
    git(&real, p, &["add", "-A"]);
    git(&real, p, &["commit", "-q", "-m", "init"]);
    std::fs::write(p.join("b.txt"), "b\n").unwrap();
    git(&real, p, &["add", "b.txt"]);
    git(&real, p, &["commit", "-q", "-m", "b"]);
    git(&real, p, &["branch", "other"]); // same commit, so switching HEAD touches neither index nor worktree
    std::fs::write(p.join("b.txt"), "B\n").unwrap();
    git(&real, p, &["add", "b.txt"]);     // a staged row
    std::fs::write(p.join("a.txt"), "ONE\n").unwrap(); // an unstaged row
    std::fs::write(p.join("new.txt"), "n\n").unwrap(); // an untracked row

    // recording wrapper, first in PATH
    let bin = tempfile::tempdir().unwrap();
    let log = bin.path().join("git.log");
    let wrapper = bin.path().join("git");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nprintf '%s\\t%s\\t%s\\t%s\\n' \"$*\" \"$GIT_OPTIONAL_LOCKS\" \"$GIT_LITERAL_PATHSPECS\" \"$GIT_NO_LAZY_FETCH\" >> '{}'\nexec '{}' \"$@\"\n",
            log.display(),
            real.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::env::set_var("PATH", format!("{}:{}", bin.path().display(), std::env::var("PATH").unwrap()));
    init_process_env();

    // taken after all harness setup, so only the engine's effect is measured
    let index_before = std::fs::read(p.join(".git/index")).unwrap();
    let refs_before = git(&real, p, &["for-each-ref"]);
    let tree_before = tree_hash(p);

    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let mut config = SessionConfig::production(p.to_path_buf());
    config.poll_interval = Duration::from_millis(100);
    let handle = spawn(rt.handle(), config);
    // Load every row, then switch branches underneath the engine.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut loaded = std::collections::BTreeSet::new();
    let mut rows = 0usize;
    let mut switched = false;
    let mut saw_other = false;
    while Instant::now() < deadline && !(rows > 0 && loaded.len() == rows && saw_other) {
        let Ok(s) = handle.snapshots.recv_timeout(Duration::from_millis(200)) else { continue };
        rows = s.files.len();
        saw_other |= matches!(&s.repo, herdr_hunks::engine::RepoState::Repo { branch: Some(b), .. } if b == "other");
        if let DiffState::Ready(d) = &s.diff {
            if loaded.insert((d.key.path.clone(), d.key.staged)) {
                handle.commands.send(Command::SelectNext).unwrap();
                handle.commands.send(Command::Refresh).unwrap();
            }
        }
        if rows > 0 && loaded.len() == rows && !switched {
            switched = true;
            git(&real, p, &["symbolic-ref", "HEAD", "refs/heads/other"]); // the harness, through the real git
        }
    }
    assert_eq!(rows, 3, "expected a staged, an unstaged and an untracked row");
    assert_eq!(loaded.len(), rows, "not every row was loaded: {loaded:?}");
    assert!(saw_other, "the branch switch never reached the engine");
    std::thread::sleep(Duration::from_millis(400)); // a few D2 ticks
    handle.commands.send(Command::Shutdown).unwrap();
    std::thread::sleep(Duration::from_millis(300));

    let recorded = std::fs::read_to_string(&log).unwrap();
    assert!(!recorded.is_empty(), "wrapper recorded nothing");
    for entry in recorded.lines() {
        let fields: Vec<&str> = entry.split('\t').collect();
        // the subcommand is the first argument after any `-C <dir>` pairs
        let args: Vec<&str> = fields[0].split_whitespace().collect();
        let mut i = 0;
        while args.get(i) == Some(&"-C") {
            i += 2;
        }
        let sub = args.get(i).copied().unwrap_or("");
        assert!(ALLOWED.contains(&sub), "unexpected git subcommand `{sub}` in `{}`", fields[0]);
        assert_eq!(&fields[1..], ["0", "1", "1"], "D3 variables missing in `{}`", fields[0]);
    }
    assert_eq!(std::fs::read(p.join(".git/index")).unwrap(), index_before, ".git/index changed");
    assert_eq!(git(&real, p, &["for-each-ref"]), refs_before, "refs changed");
    assert_eq!(tree_hash(p), tree_before, "worktree changed");
}
```

- [ ] **Step 2: Run and commit**

Run: `cargo test --test readonly_guarantee` — Expected: PASS. If it fails on an unexpected subcommand, that is a real G7 finding: fix the engine, do not extend `ALLOWED` without review.

```bash
git add -A
git commit -m "test: enforce the read-only guarantee with a recording git wrapper"
```

---

### Task 9: TUI primitives copied from herdr-agent-watcher

Spec: 4.1, 4.8 (scroll arithmetic), 2.2 item 3.

**Files:**
- Create: `src/tui/mod.rs`, `src/tui/style.rs`, `src/tui/format.rs`, `src/tui/dialog.rs`, `src/tui/layout.rs`, `src/tui/guard.rs`
- Modify: `src/lib.rs` (add `#[cfg(feature = "tui")] pub mod tui;`), `PORT-SURFACE.md`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `tui::style::{Role, Semantic, Style, Span, Line}`; `tui::format::{width, pad, truncate}`; `tui::dialog::{Panel, Row, line_count, render}`; `tui::layout::{LineSpan, clamp_scroll, ensure_visible, reanchor}` with `usize` content offsets; `tui::guard::{TerminalGuard, poll_terminal, mouse_transition}`.

- [ ] **Step 1: Copy the unchanged pieces**

```bash
mkdir -p src/tui
sed -n '1,84p' "$WATCHER/src/sidebar/style.rs" > src/tui/style.rs          # Role, Semantic, Style, Span, Line
cp "$WATCHER/src/sidebar/dialog.rs" src/tui/dialog.rs
```

Then, by hand:
- `src/tui/style.rs`: delete any `use` of `crate::sidebar::layout::LineSpan` left in the first 84 lines (it serves the `Rendered` type that is not copied).
- `src/tui/format.rs`: copy the three functions `width`, `pad` and `truncate` from `$WATCHER/src/sidebar/format.rs` (they start at lines 15, 32 and 40) together with the tests that cover them.
- `src/tui/dialog.rs`: it names `crate::sidebar::` seven times (two imports and five uses inside its tests). Rewrite all of them: `sed -i.bak 's/crate::sidebar::/crate::tui::/g' src/tui/dialog.rs && rm src/tui/dialog.rs.bak`, then check `rg -c 'crate::sidebar' src/tui/` prints nothing.
- `src/tui/guard.rs`: copy from `$WATCHER/src/sidebar/tui.rs` the `TerminalGuard` struct, its two `impl` blocks and its `Drop` (lines 22-75), `terminal_is_gone` and `poll_terminal` (lines 85-119), `mouse_transition` (lines 2176-2178), the `DisableRawMode` type they name, and the guard's tests (search for `FlakyWriter`). Make `TerminalGuard`, its constructor `TerminalGuard::enter`, `TerminalGuard::set_mouse`, `poll_terminal` and `mouse_transition` `pub`.

`src/tui/mod.rs`:

```rust
pub mod dialog;
pub mod format;
pub mod guard;
pub mod layout;
pub mod style;
```

- [ ] **Step 2: Write the failing layout tests in `src/tui/layout.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_keeps_the_last_page_reachable_past_u16() {
        assert_eq!(clamp_scroll(150_000, 100_000, 40), 99_960);
        assert_eq!(clamp_scroll(70_000, 100_000, 40), 70_000);
        assert_eq!(clamp_scroll(5, 10, 40), 0);
    }

    #[test]
    fn ensure_visible_scrolls_the_minimum() {
        let span = |start| LineSpan { start, height: 1 };
        assert_eq!(ensure_visible(0, span(90_000), 40, 100_000), 89_961);
        assert_eq!(ensure_visible(89_961, span(89_970), 40, 100_000), 89_961);
        assert_eq!(ensure_visible(89_961, span(10), 40, 100_000), 10);
    }

    #[test]
    fn reanchor_keeps_the_viewport_row() {
        // the cursor sat on viewport row 7 (row 107 with offset 100); it is now row 250
        assert_eq!(reanchor(100, 107, 250, 40, 100_000), 243);
        assert_eq!(reanchor(100, 107, 3, 40, 100_000), 0);
    }
}
```

Run: `cargo test layout` — Expected: FAIL (does not compile).

- [ ] **Step 3: Implement `src/tui/layout.rs`**

```rust
//! Scroll arithmetic after herdr-agent-watcher's layout.rs, with usize content offsets.
//! The original uses u16 and saturates at row 65,535, which a diff exceeds.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineSpan {
    pub start: usize,
    pub height: usize,
}

pub fn clamp_scroll(offset: usize, total_lines: usize, viewport_height: u16) -> usize {
    offset.min(total_lines.saturating_sub(usize::from(viewport_height)))
}

pub fn ensure_visible(offset: usize, span: LineSpan, viewport: u16, total_lines: usize) -> usize {
    let viewport = usize::from(viewport);
    let end = span.start + span.height;
    let next = if span.start < offset {
        span.start
    } else if end > offset + viewport {
        end.saturating_sub(viewport)
    } else {
        offset
    };
    clamp_scroll(next, total_lines, viewport as u16)
}

/// Keeps the row that was at `old_row` on the same viewport line now that it is `new_row`.
pub fn reanchor(offset: usize, old_row: usize, new_row: usize, viewport: u16, total_lines: usize) -> usize {
    let viewport_line = old_row.saturating_sub(offset);
    clamp_scroll(new_row.saturating_sub(viewport_line), total_lines, viewport)
}
```

- [ ] **Step 4: Register the copies**

Add to the **Copied from herdr-agent-watcher** table in `PORT-SURFACE.md`: `src/tui/style.rs` (lines 1-84, unchanged); `src/tui/format.rs` (`width`, `pad`, `truncate`, unchanged); `src/tui/dialog.rs` (imports changed); `src/tui/guard.rs` (visibility widened); `src/tui/layout.rs` (adapted: `usize` content offsets, reason as in spec 4.8).

- [ ] **Step 5: Run and commit**

Run: `cargo test tui:: && cargo check --no-default-features` — Expected: PASS; the no-default-features build does not compile `src/tui/`.

```bash
git add -A
git commit -m "feat: add tui primitives copied from herdr-agent-watcher"
```

---

### Task 10: Sanitizer and row model

Spec: 4.1, 4.2 "Diff body", 4.5.

**Files:**
- Create: `src/tui/sanitize.rs`, `src/tui/rows.rs`
- Modify: `src/tui/mod.rs` (add `pub mod rows; pub mod sanitize;`)

**Interfaces:**
- Consumes: `engine::{LoadedDiff, nav::{Side, ViewMode, find}}`, `crate::git::DiffLineType`.
- Produces:

```rust
pub fn sanitize(text: &str) -> String;                       // tui::sanitize

pub struct Cell { pub target: Option<usize>, pub no: Option<u32>, pub sign: char, pub text: String }
pub enum Row {
    FileHeader { path: String },
    HunkHeader { hunk_index: usize, text: String },
    Gap { lines: u32 },
    Unified { target: usize, old_no: Option<u32>, new_no: Option<u32>, sign: char, text: String },
    Split { left: Option<Cell>, right: Option<Cell> },
    Truncated { lines: usize },
}
pub struct Rows { pub rows: Vec<Row>, pub row_of_target: Vec<usize> }
pub fn build(diff: &LoadedDiff, mode: ViewMode) -> Rows;      // tui::rows
```

- [ ] **Step 1: Write the failing sanitizer tests in `src/tui/sanitize.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn replaces_c0_controls_with_control_pictures() {
        assert_eq!(sanitize("a\x1b[31mb"), "a\u{241b}[31mb");
        assert_eq!(sanitize("x\x00y\x07z"), "x\u{2400}y\u{2407}z");
    }

    #[test]
    fn expands_tabs_to_the_next_multiple_of_eight() {
        assert_eq!(sanitize("\tx"), "        x");
        assert_eq!(sanitize("ab\tx"), "ab      x");
    }

    #[test]
    fn replaces_del_and_c1_with_the_replacement_character() {
        assert_eq!(sanitize("a\x7fb\u{9b}c"), "a\u{fffd}b\u{fffd}c");
    }

    #[test]
    fn strips_a_trailing_newline_only() {
        assert_eq!(sanitize("line\n"), "line");
        assert_eq!(sanitize("a\nb"), "a\u{240a}b");
    }
}
```

- [ ] **Step 2: Implement `src/tui/sanitize.rs`**

```rust
//! Every string from git is untrusted. Nothing reaches a cell without passing here.
use unicode_width::UnicodeWidthChar;

pub fn sanitize(text: &str) -> String {
    let text = text.strip_suffix('\n').unwrap_or(text);
    let text = text.strip_suffix('\r').unwrap_or(text);
    let mut out = String::with_capacity(text.len());
    let mut column = 0usize;
    for ch in text.chars() {
        match ch {
            '\t' => {
                let pad = 8 - (column % 8);
                out.extend(std::iter::repeat(' ').take(pad));
                column += pad;
            }
            c if (c as u32) < 0x20 => {
                out.push(char::from_u32(0x2400 + c as u32).unwrap_or('\u{fffd}'));
                column += 1;
            }
            '\x7f' => {
                out.push('\u{fffd}');
                column += 1;
            }
            c if (0x80..=0x9f).contains(&(c as u32)) => {
                out.push('\u{fffd}');
                column += 1;
            }
            c => {
                out.push(c);
                column += c.width().unwrap_or(0);
            }
        }
    }
    out
}
```

Run: `cargo test sanitize` — Expected: PASS.

- [ ] **Step 3: Write the failing row tests in `src/tui/rows.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::nav::ViewMode;
    use crate::engine::{FileKey, LoadedDiff};
    use crate::git::{DiffHunk, DiffLine, DiffLineType, FileDiff, GetGitDiffResponse};

    fn loaded(hunks: &[(u32, u32, &[(char, &str)])]) -> LoadedDiff {
        let hunks = hunks
            .iter()
            .map(|(old_start, new_start, lines)| DiffHunk {
                id: String::new(),
                header: format!("@@ -{old_start} +{new_start} @@"),
                old_start: *old_start,
                old_lines: lines.iter().filter(|(k, _)| *k != '+').count() as u32,
                new_start: *new_start,
                new_lines: lines.iter().filter(|(k, _)| *k != '-').count() as u32,
                lines: lines
                    .iter()
                    .map(|(k, text)| DiffLine {
                        line_type: match k { '+' => DiffLineType::Added, '-' => DiffLineType::Removed, _ => DiffLineType::Context },
                        content: text.to_string(),
                        old_line_number: None,
                        new_line_number: None,
                    })
                    .collect(),
            })
            .collect();
        LoadedDiff::build(
            FileKey { path: "src/f.rs".into(), staged: false },
            GetGitDiffResponse {
                file_diff: FileDiff { file_path: "src/f.rs".into(), old_path: None, new_path: None, hunks },
                old_text: String::new(),
                new_text: String::new(),
                raw_diff: String::new(),
                repo_root: String::new(),
            },
        )
    }

    fn kinds(rows: &Rows) -> String {
        rows.rows
            .iter()
            .map(|r| match r {
                Row::FileHeader { .. } => 'F',
                Row::HunkHeader { .. } => 'H',
                Row::Gap { .. } => 'G',
                Row::Unified { sign, .. } => *sign,
                Row::Split { .. } => 'S',
                Row::Truncated { .. } => 'T',
            })
            .collect()
    }

    #[test]
    fn unified_rows_follow_the_hunk_with_a_leading_gap() {
        let d = loaded(&[(10, 10, &[(' ', "a"), ('-', "b"), ('-', "c"), ('+', "B"), (' ', "d")])]);
        let rows = build(&d, ViewMode::Unified);
        assert_eq!(kinds(&rows), "FGH --+ ");
        assert!(matches!(rows.rows[1], Row::Gap { lines: 9 }));
        // every target maps to the row that shows it
        for (target, row) in rows.row_of_target.iter().enumerate() {
            match &rows.rows[*row] {
                Row::Unified { target: t, .. } => assert_eq!(*t, target),
                other => panic!("target {target} maps to {:?}", std::mem::discriminant(other)),
            }
        }
    }

    #[test]
    fn a_gap_between_hunks_counts_the_unmodified_lines() {
        let d = loaded(&[(1, 1, &[('+', "x")]), (30, 31, &[('-', "y")])]);
        let rows = build(&d, ViewMode::Unified);
        assert_eq!(kinds(&rows), "FH+GH-");
        assert!(matches!(rows.rows[3], Row::Gap { lines: 29 }));
    }

    #[test]
    fn split_rows_pair_a_replacement_and_fill_the_short_side() {
        let d = loaded(&[(10, 10, &[(' ', "a"), ('-', "b"), ('-', "c"), ('+', "B"), (' ', "d")])]);
        let rows = build(&d, ViewMode::Split);
        assert_eq!(kinds(&rows), "FGHSSSS");
        match &rows.rows[4] {
            Row::Split { left: Some(l), right: Some(r) } => {
                assert_eq!((l.no, l.sign, l.text.as_str()), (Some(11), '-', "b"));
                assert_eq!((r.no, r.sign, r.text.as_str()), (Some(11), '+', "B"));
            }
            _ => panic!("row 4 must be a full pair"),
        }
        assert!(matches!(&rows.rows[5], Row::Split { left: Some(_), right: None }));
    }

    #[test]
    fn a_truncated_diff_ends_with_a_truncated_row() {
        let mut d = loaded(&[(1, 1, &[('+', "x")])]);
        d.truncated_lines = 7;
        let rows = build(&d, ViewMode::Unified);
        assert!(matches!(rows.rows.last(), Some(Row::Truncated { lines: 7 })));
    }

    #[test]
    fn text_is_sanitized() {
        let d = loaded(&[(1, 1, &[('+', "a\x1bb")])]);
        let rows = build(&d, ViewMode::Unified);
        match &rows.rows[2] {
            Row::Unified { text, .. } => assert_eq!(text, "a\u{241b}b"),
            _ => panic!(),
        }
    }
}
```

Run: `cargo test rows` — Expected: FAIL (does not compile).

- [ ] **Step 4: Implement `src/tui/rows.rs`**

```rust
//! Display rows for one LoadedDiff in one view mode. Built once per diff or mode change.
use std::collections::HashMap;

use crate::engine::nav::{Side, ViewMode};
use crate::engine::LoadedDiff;
use crate::git::DiffLineType;
use crate::tui::sanitize::sanitize;

#[derive(Debug, Clone)]
pub struct Cell {
    pub target: Option<usize>,
    pub no: Option<u32>,
    pub sign: char,
    pub text: String,
}

#[derive(Debug, Clone)]
pub enum Row {
    FileHeader { path: String },
    HunkHeader { hunk_index: usize, text: String },
    Gap { lines: u32 },
    Unified { target: usize, old_no: Option<u32>, new_no: Option<u32>, sign: char, text: String },
    Split { left: Option<Cell>, right: Option<Cell> },
    Truncated { lines: usize },
}

pub struct Rows {
    pub rows: Vec<Row>,
    /// Row index that displays each target.
    pub row_of_target: Vec<usize>,
}

pub fn build(diff: &LoadedDiff, mode: ViewMode) -> Rows {
    let index: HashMap<(usize, Side, u32), usize> = diff
        .targets
        .iter()
        .enumerate()
        .map(|(i, t)| ((t.hunk_index, t.side, t.line_number), i))
        .collect();
    let mut rows = vec![Row::FileHeader { path: sanitize(&diff.file_diff.file_path) }];
    let mut row_of_target = vec![0usize; diff.targets.len()];
    let mut previous_end = 1u32;

    for (hunk_index, hunk) in diff.file_diff.hunks.iter().enumerate() {
        if hunk.new_start > previous_end {
            rows.push(Row::Gap { lines: hunk.new_start - previous_end });
        }
        previous_end = hunk.new_start + hunk.new_lines;
        rows.push(Row::HunkHeader { hunk_index, text: sanitize(&hunk.header) });

        let mut old_no = hunk.old_start;
        let mut new_no = hunk.new_start;
        let mut deletions: Vec<Cell> = Vec::new();
        let mut additions: Vec<Cell> = Vec::new();

        let mut flush = |rows: &mut Vec<Row>, row_of_target: &mut Vec<usize>, deletions: &mut Vec<Cell>, additions: &mut Vec<Cell>| {
            for i in 0..deletions.len().max(additions.len()) {
                let left = deletions.get(i).cloned();
                let right = additions.get(i).cloned();
                for cell in [&left, &right].into_iter().flatten() {
                    if let Some(t) = cell.target {
                        row_of_target[t] = rows.len();
                    }
                }
                rows.push(Row::Split { left, right });
            }
            deletions.clear();
            additions.clear();
        };

        for line in &hunk.lines {
            let text = sanitize(&line.content);
            match line.line_type {
                DiffLineType::Removed => {
                    let no = line.old_line_number.unwrap_or(old_no);
                    let target = index.get(&(hunk_index, Side::Deletions, no)).copied();
                    match mode {
                        ViewMode::Unified => {
                            if let Some(t) = target {
                                row_of_target[t] = rows.len();
                                rows.push(Row::Unified { target: t, old_no: Some(no), new_no: None, sign: '-', text });
                            }
                        }
                        ViewMode::Split => deletions.push(Cell { target, no: Some(no), sign: '-', text }),
                    }
                    old_no += 1;
                }
                DiffLineType::Added => {
                    let no = line.new_line_number.unwrap_or(new_no);
                    let target = index.get(&(hunk_index, Side::Additions, no)).copied();
                    match mode {
                        ViewMode::Unified => {
                            if let Some(t) = target {
                                row_of_target[t] = rows.len();
                                rows.push(Row::Unified { target: t, old_no: None, new_no: Some(no), sign: '+', text });
                            }
                        }
                        ViewMode::Split => additions.push(Cell { target, no: Some(no), sign: '+', text }),
                    }
                    new_no += 1;
                }
                DiffLineType::Context => {
                    if mode == ViewMode::Split {
                        flush(&mut rows, &mut row_of_target, &mut deletions, &mut additions);
                    }
                    let shown_new = line.new_line_number.unwrap_or(new_no);
                    let shown_old = line.old_line_number.unwrap_or(old_no);
                    let target = index.get(&(hunk_index, Side::Additions, shown_new)).copied();
                    if let Some(t) = target {
                        row_of_target[t] = rows.len();
                        rows.push(match mode {
                            ViewMode::Unified => Row::Unified { target: t, old_no: Some(shown_old), new_no: Some(shown_new), sign: ' ', text },
                            ViewMode::Split => Row::Split {
                                left: Some(Cell { target: None, no: Some(shown_old), sign: ' ', text: text.clone() }),
                                right: Some(Cell { target: Some(t), no: Some(shown_new), sign: ' ', text }),
                            },
                        });
                    }
                    old_no += 1;
                    new_no += 1;
                }
            }
        }
        if mode == ViewMode::Split {
            flush(&mut rows, &mut row_of_target, &mut deletions, &mut additions);
        }
    }
    if diff.truncated_lines > 0 {
        rows.push(Row::Truncated { lines: diff.truncated_lines });
    }
    Rows { rows, row_of_target }
}
```

- [ ] **Step 5: Run and commit**

Run: `cargo test tui::rows && cargo test tui::sanitize` — Expected: PASS. (Cargo accepts one test filter per invocation.)

```bash
git add -A
git commit -m "feat: add diff row model and untrusted-text sanitizer"
```

---

### Task 11: View state, reconciliation and the pure view

Spec: 4.2, 4.5, 4.6, 4.8.

**Files:**
- Create: `src/tui/state.rs`, `src/tui/view.rs`
- Modify: `src/tui/mod.rs` (add `pub mod state; pub mod view;`)

**Interfaces:**
- Consumes: `engine::{Snapshot, DiffState, RepoState, LoadedDiff, FileKey, nav::{self, Side, ViewMode}}`, `tui::{rows, layout, format, style}`.
- Produces:

```rust
// tui::state
pub enum FilesPanel { Hidden, Shown, Pinned }
pub struct ViewState {
    pub mode: ViewMode,
    pub cursor: Option<usize>,             // index into LoadedDiff.targets
    pub cursor_id: Option<(Side, u32)>,    // identity that survives a refresh
    pub offset: usize,                     // first visible body row
    pub hscroll: usize,
    pub files_panel: FilesPanel,
    pub mouse_requested: bool,
    pub help_open: bool,
    pub notice: Option<String>,
    pub rows: Option<rows::Rows>,
    pub body_height: u16,
    pub help_offset: usize,                // first key-sheet row drawn
    built_from: Option<(std::sync::Arc<LoadedDiff>, ViewMode)>, // held so identity compares are safe
}
impl ViewState {
    pub fn new(mode: ViewMode, files_panel: FilesPanel, mouse: bool) -> Self;
    pub fn reconcile(&mut self, snapshot: &Snapshot);      // spec 4.8, call before every render
    pub fn set_cursor(&mut self, diff: &LoadedDiff, target: usize);
    pub fn resize(&mut self, body_height: u16);
}

// tui::view
pub enum Action { PrevFile, NextFile, PrevHunk, NextHunk, ToggleView, ToggleFiles, Refresh, SelectFile(usize), CursorToRow(usize) }
pub struct Hit { pub y: u16, pub x0: u16, pub x1: u16, pub action: Action }   // x1 exclusive
pub struct Rendered { pub lines: Vec<style::Line>, pub hits: Vec<Hit> }
impl Rendered { pub fn plain(&self) -> Vec<String>; pub fn hit(&self, x: u16, y: u16) -> Option<&Action>; }
pub const FILES_WIDTH: u16 = 18;
pub fn body_height(state: &ViewState, snapshot: &Snapshot, height: u16) -> u16;
pub fn render(snapshot: &Snapshot, state: &ViewState, width: u16, height: u16) -> Rendered;
```

- [ ] **Step 1: Write the failing reconciliation tests in `src/tui/state.rs`**

```rust
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::engine::{DiffState, LoadedDiff, RepoState, Snapshot};
    use crate::git::{DiffHunk, DiffLine, DiffLineType, FileDiff, GetGitDiffResponse};
    use std::sync::Arc;

    pub(crate) fn snapshot(path: &str, raw: &str, hunks: &[(u32, &str)]) -> Snapshot {
        let hunks = hunks
            .iter()
            .map(|(start, kinds)| DiffHunk {
                id: String::new(),
                header: format!("@@ -{start} +{start} @@"),
                old_start: *start,
                old_lines: kinds.chars().filter(|c| *c != '+').count() as u32,
                new_start: *start,
                new_lines: kinds.chars().filter(|c| *c != '-').count() as u32,
                lines: kinds
                    .chars()
                    .map(|k| DiffLine {
                        line_type: match k { '+' => DiffLineType::Added, '-' => DiffLineType::Removed, _ => DiffLineType::Context },
                        content: format!("line {k}"),
                        old_line_number: None,
                        new_line_number: None,
                    })
                    .collect(),
            })
            .collect();
        let key = FileKey { path: path.into(), staged: false };
        let loaded = LoadedDiff::build(
            key.clone(),
            GetGitDiffResponse {
                file_diff: FileDiff { file_path: path.into(), old_path: None, new_path: None, hunks },
                old_text: String::new(),
                new_text: String::new(),
                raw_diff: raw.into(),
                repo_root: "/r".into(),
            },
        );
        let mut s = Snapshot::empty("/r");
        s.revision = 1;
        s.repo = RepoState::Repo { toplevel: "/r".into(), branch: Some("main".into()), worktree: None };
        s.selected = Some(key);
        s.diff = DiffState::Ready(Arc::new(loaded));
        s
    }

    fn state() -> ViewState {
        let mut s = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        s.resize(10);
        s
    }

    #[test]
    fn a_new_file_puts_the_cursor_on_the_first_changed_row_and_resets_offsets() {
        let mut st = state();
        st.offset = 7;
        st.hscroll = 16;
        st.reconcile(&snapshot("a.rs", "r1", &[(10, "  +  ")]));
        assert_eq!(st.cursor, Some(2));
        assert_eq!((st.offset, st.hscroll), (0, 0));
    }

    #[test]
    fn a_refresh_keeps_the_cursor_on_its_line_or_moves_to_the_nearest() {
        let mut st = state();
        st.reconcile(&snapshot("a.rs", "r1", &[(10, " + + ")]));
        let snap = snapshot("a.rs", "r1", &[(10, " + + ")]);
        if let DiffState::Ready(d) = &snap.diff { st.set_cursor(d, 3); } // additions line 13
        st.reconcile(&snapshot("a.rs", "r2", &[(10, "  + + ")]));        // line 13 still exists
        assert_eq!(st.cursor_id, Some((Side::Additions, 13)));
        st.reconcile(&snapshot("a.rs", "r3", &[(10, " +")]));            // line 13 is gone
        assert_eq!(st.cursor_id, Some((Side::Additions, 11)));
    }

    #[test]
    fn toggling_the_mode_keeps_the_line() {
        let mut st = state();
        let snap = snapshot("a.rs", "r1", &[(10, " --+ ")]);
        st.reconcile(&snap);
        if let DiffState::Ready(d) = &snap.diff { st.set_cursor(d, 3); } // deletions line 12
        st.mode = ViewMode::Split;
        st.reconcile(&snap);
        assert_eq!(st.cursor_id, Some((Side::Deletions, 12)));
        assert!(st.rows.is_some());
    }

    #[test]
    fn a_diff_without_targets_clears_the_cursor() {
        let mut st = state();
        st.reconcile(&snapshot("bin.dat", "r1", &[]));
        assert_eq!(st.cursor, None);
        assert_eq!((st.offset, st.hscroll), (0, 0));
    }
}
```

Run: `cargo test tui::state` — Expected: FAIL (does not compile).

- [ ] **Step 2: Implement `src/tui/state.rs`**

```rust
//! Everything the TUI owns that is not in the snapshot, and the 4.8 reconciliation rules.
use crate::engine::nav::{self, Side, ViewMode};
use crate::engine::{DiffState, FileKey, LoadedDiff, Snapshot};
use crate::tui::layout::{clamp_scroll, ensure_visible, reanchor, LineSpan};
use crate::tui::rows::{self, Rows};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesPanel {
    Hidden,
    Shown,
    Pinned,
}

pub struct ViewState {
    pub mode: ViewMode,
    pub cursor: Option<usize>,
    pub cursor_id: Option<(Side, u32)>,
    pub offset: usize,
    pub hscroll: usize,
    pub files_panel: FilesPanel,
    pub mouse_requested: bool,
    pub help_open: bool,
    pub notice: Option<String>,
    pub rows: Option<Rows>,
    pub body_height: u16,
    pub help_offset: usize,
    /// The diff the rows were built from. Holding the Arc makes `Arc::ptr_eq` a safe,
    /// allocation-free "did anything change" test; the engine swaps the Arc only on change.
    built_from: Option<(std::sync::Arc<LoadedDiff>, ViewMode)>,
}

impl ViewState {
    pub fn new(mode: ViewMode, files_panel: FilesPanel, mouse: bool) -> Self {
        Self {
            mode,
            cursor: None,
            cursor_id: None,
            offset: 0,
            hscroll: 0,
            files_panel,
            mouse_requested: mouse,
            help_open: false,
            notice: None,
            rows: None,
            body_height: 0,
            help_offset: 0,
            built_from: None,
        }
    }

    /// Acts only when the height changed; the run loop calls it before every frame.
    pub fn resize(&mut self, body_height: u16) {
        if self.body_height == body_height {
            return;
        }
        self.body_height = body_height;
        if let Some(rows) = &self.rows {
            self.offset = clamp_scroll(self.offset, rows.rows.len(), body_height);
            self.keep_cursor_visible();
        }
    }

    pub fn set_cursor(&mut self, diff: &LoadedDiff, target: usize) {
        if let Some(t) = diff.targets.get(target) {
            self.cursor = Some(target);
            self.cursor_id = Some((t.side, t.line_number));
            self.keep_cursor_visible();
        }
    }

    fn keep_cursor_visible(&mut self) {
        if let (Some(rows), Some(cursor)) = (&self.rows, self.cursor) {
            if let Some(&row) = rows.row_of_target.get(cursor) {
                self.offset = ensure_visible(self.offset, LineSpan { start: row, height: 1 }, self.body_height, rows.rows.len());
            }
        }
    }

    pub fn reconcile(&mut self, snapshot: &Snapshot) {
        let DiffState::Ready(diff) = &snapshot.diff else {
            self.rows = None;
            self.cursor = None;
            self.cursor_id = None;
            self.offset = 0;
            self.hscroll = 0;
            self.built_from = None;
            return;
        };
        // Runs before every frame, so the unchanged case must cost nothing: no clone, no compare of text.
        if let Some((built, mode)) = &self.built_from {
            if std::sync::Arc::ptr_eq(built, diff) && *mode == self.mode {
                return;
            }
        }
        let same_file = self.built_from.as_ref().map(|(built, _)| built.key == diff.key).unwrap_or(false);
        let old_row = match (&self.rows, self.cursor) {
            (Some(rows), Some(c)) => rows.row_of_target.get(c).copied(),
            _ => None,
        };
        let rows = rows::build(diff, self.mode);

        if !same_file {
            self.offset = 0;
            self.hscroll = 0;
            self.cursor = nav::target_index_for_hunk(&diff.targets, 0);
        } else {
            self.cursor = self
                .cursor_id
                .and_then(|(side, line)| nav::find(&diff.targets, side, line).or_else(|| nav::nearest(&diff.targets, side, line)));
            if self.cursor.is_none() {
                self.cursor = nav::target_index_for_hunk(&diff.targets, 0);
            }
        }
        self.cursor_id = self.cursor.and_then(|c| diff.targets.get(c)).map(|t| (t.side, t.line_number));
        if self.cursor.is_none() {
            self.offset = 0;
            self.hscroll = 0;
        } else if let (true, Some(old), Some(new)) = (same_file, old_row, self.cursor.and_then(|c| rows.row_of_target.get(c).copied())) {
            self.offset = reanchor(self.offset, old, new, self.body_height, rows.rows.len());
        }
        self.rows = Some(rows);
        self.keep_cursor_visible();
        self.built_from = Some((diff.clone(), self.mode));
    }
}
```

Run: `cargo test tui::state` — Expected: PASS.

- [ ] **Step 3: Write the failing view tests in `src/tui/view.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{RepoState, Snapshot};
    use crate::git::{ChangedFile, ChangedFileStatus};
    use crate::tui::state::tests::snapshot;
    use crate::tui::state::{FilesPanel, ViewState};

    fn files(mut s: Snapshot) -> Snapshot {
        s.files = vec![
            ChangedFile { path: "src/a.rs".into(), status: ChangedFileStatus::Modified, staged: true, insertions: Some(1), deletions: Some(1) },
            ChangedFile { path: "a.rs".into(), status: ChangedFileStatus::Modified, staged: false, insertions: Some(4), deletions: Some(3) },
        ];
        s
    }

    fn rendered(width: u16, height: u16, panel: FilesPanel) -> (Rendered, ViewState) {
        let snap = files(snapshot("a.rs", "r1", &[(10, " --+ ")]));
        let mut st = ViewState::new(ViewMode::Unified, panel, true);
        st.resize(body_height(&st, &snap, height));
        st.reconcile(&snap);
        (render(&snap, &st, width, height), st)
    }

    #[test]
    fn the_toolbar_shows_both_steppers_and_every_item_is_clickable() {
        let (r, _) = rendered(120, 20, FilesPanel::Hidden);
        let bar = &r.plain()[0];
        for piece in ["‹ a.rs 2/2 ›", "{} 1/1 ↑ ↓", "unified", "UNSTAGED", "+4 −3", "files", "⟳"] {
            assert!(bar.contains(piece), "toolbar `{bar}` lacks `{piece}`");
        }
        for action in [Action::PrevFile, Action::NextFile, Action::PrevHunk, Action::NextHunk, Action::ToggleView, Action::ToggleFiles, Action::Refresh] {
            assert!(r.hits.iter().any(|h| h.y == 0 && std::mem::discriminant(&h.action) == std::mem::discriminant(&action)));
        }
    }

    #[test]
    fn a_narrow_toolbar_drops_items_from_the_right_and_keeps_the_steppers() {
        let (r, _) = rendered(50, 20, FilesPanel::Hidden);
        let bar = &r.plain()[0];
        assert!(bar.contains("‹ a.rs 2/2 ›") && bar.contains("{} 1/1"));
        assert!(!bar.contains("⟳") && !bar.contains("files"));
    }

    #[test]
    fn the_unified_body_shows_numbers_signs_gap_and_cursor() {
        let (r, st) = rendered(80, 20, FilesPanel::Hidden);
        let text = r.plain();
        assert!(text[1].starts_with("a.rs"));
        assert!(text[2].contains("··· 9 unmodified lines ···"));
        assert!(text[3].contains("@@ -10 +10 @@"));
        assert!(text[5].contains("11") && text[5].contains("- line -"));
        assert!(text[7].contains("11") && text[7].contains("+ line +"));
        assert_eq!(st.cursor, Some(1));
        assert!(r.lines[5].iter().all(|span| span.style.reverse), "the cursor row is reverse video");
    }

    #[test]
    fn the_pinned_files_panel_lists_rows_and_is_clickable() {
        let (r, _) = rendered(120, 20, FilesPanel::Pinned);
        let text = r.plain();
        assert!(text[1].starts_with("CHANGED 2"));
        assert!(text[2].contains("M a.rs src/") && text[2].contains("S"), "{}", text[2]);
        assert!(text[3].starts_with("▸M a.rs ./"), "{}", text[3]);
        assert!(r.hits.iter().any(|h| matches!(h.action, Action::SelectFile(0)) && h.y == 2));
    }

    #[test]
    fn empty_and_error_states_are_one_centred_line() {
        let mut snap = Snapshot::empty("/x");
        snap.revision = 1;
        let st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        assert!(render(&snap, &st, 80, 12).plain().iter().any(|l| l.contains("not a git repository")));
        snap.repo = RepoState::Repo { toplevel: "/x".into(), branch: None, worktree: None };
        assert!(render(&snap, &st, 80, 12).plain().iter().any(|l| l.contains("working tree clean")));
        snap.repo = RepoState::Unusable { reason: "git 2.20 is too old".into() };
        assert!(render(&snap, &st, 80, 12).plain().iter().any(|l| l.contains("git 2.20 is too old")));
        snap.watcher_error = Some("inotify limit".into());
        assert!(render(&snap, &st, 80, 12).plain().iter().any(|l| l.contains("live refresh degraded: inotify limit")));
    }

    #[test]
    fn a_conflict_is_named_and_a_status_failure_stays_visible() {
        let mut snap = files(snapshot("a.rs", "diff --cc a.rs\nindex 1,2..3\n@@@ -1,1 -1,1 +1,3 @@@\n", &[]));
        let st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        assert!(render(&snap, &st, 80, 12).plain().iter().any(|l| l.contains("unmerged path")));
        snap.status_error = Some("git command timed out after 30s".into());
        let text = render(&snap, &st, 80, 12).plain();
        assert!(text.iter().any(|l| l.contains("status failed: git command timed out after 30s")), "{text:?}");
    }

    #[test]
    fn a_tiny_terminal_draws_one_line() {
        let (r, _) = rendered(30, 5, FilesPanel::Hidden);
        assert_eq!(r.plain(), vec!["terminal too small".to_string()]);
    }
}
```

Run: `cargo test tui::view` — Expected: FAIL (does not compile).

- [ ] **Step 4: Implement `src/tui/view.rs`**

Fixed geometry: line 0 is the toolbar; the last line is the footer; when `snapshot.watcher_error` or `state.notice` is set, the line above the footer is the notice; everything between is the body. With the files panel shown, the body's first `FILES_WIDTH` columns are the panel and column `FILES_WIDTH` is a `│` separator. Below 40x10, return the single line `terminal too small`.

```rust
//! Pure view: snapshot + view state in, styled lines and hit regions out.
use crate::engine::nav::ViewMode;
use crate::engine::{DiffState, RepoState, Snapshot};
use crate::git::ChangedFileStatus;
use crate::tui::format::{pad, truncate, width};
use crate::tui::rows::Row;
use crate::tui::sanitize::sanitize;
use crate::tui::state::{FilesPanel, ViewState};
use crate::tui::style::{Line, Role, Semantic, Span, Style};

pub const FILES_WIDTH: u16 = 18;
pub const MIN_SPLIT_WIDTH: u16 = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    PrevFile,
    NextFile,
    PrevHunk,
    NextHunk,
    ToggleView,
    ToggleFiles,
    Refresh,
    SelectFile(usize),
    CursorToRow(usize),
}

pub struct Hit {
    pub y: u16,
    pub x0: u16,
    pub x1: u16,
    pub action: Action,
}

pub struct Rendered {
    pub lines: Vec<Line>,
    pub hits: Vec<Hit>,
}

impl Rendered {
    pub fn plain(&self) -> Vec<String> {
        self.lines.iter().map(|l| l.iter().map(|s| s.text.as_str()).collect::<String>().trim_end().to_string()).collect()
    }

    pub fn hit(&self, x: u16, y: u16) -> Option<&Action> {
        self.hits.iter().find(|h| h.y == y && x >= h.x0 && x < h.x1).map(|h| &h.action)
    }
}

/// The one-line notice above the footer, by priority.
pub fn notice(state: &ViewState, snapshot: &Snapshot) -> Option<String> {
    if let (Some(error), false) = (&snapshot.status_error, snapshot.files.is_empty()) {
        return Some(format!("status failed: {} · showing the last good list · r retries", sanitize(error)));
    }
    if let Some(reason) = &snapshot.watcher_error {
        return Some(format!("live refresh degraded: {} · polling every 5 s · r retries", sanitize(reason)));
    }
    state.notice.clone()
}

pub fn body_height(state: &ViewState, snapshot: &Snapshot, height: u16) -> u16 {
    height.saturating_sub(2 + u16::from(notice(state, snapshot).is_some()))
}

/// The frozen parser yields zero hunks for a combined diff; the raw text still says so.
fn is_conflict(raw_diff: &str) -> bool {
    raw_diff.lines().any(|l| l.starts_with("diff --cc") || l.starts_with("diff --combined") || l.starts_with("@@@"))
}

/// Toolbar items left to right; the third field is the drop order (higher drops first).
fn toolbar_items(snapshot: &Snapshot, state: &ViewState) -> Vec<(Vec<(String, Option<Action>)>, u8)> {
    let total = snapshot.files.len();
    let index = snapshot.selected.as_ref().and_then(|k| snapshot.files.iter().position(|f| f.path == k.path && f.staged == k.staged));
    let name = snapshot.selected.as_ref().map(|k| sanitize(k.path.rsplit('/').next().unwrap_or(&k.path))).unwrap_or_else(|| "no file".into());
    let (hunk_pos, hunk_total, stats, staged) = match &snapshot.diff {
        DiffState::Ready(d) => {
            let pos = state.cursor.and_then(|c| d.targets.get(c)).map(|t| t.hunk_index + 1).unwrap_or(0);
            let file = index.and_then(|i| snapshot.files.get(i));
            let stats = file.map(|f| format!("+{} −{}", f.insertions.unwrap_or(0), f.deletions.unwrap_or(0))).unwrap_or_default();
            (pos, d.file_diff.hunks.len(), stats, d.key.staged)
        }
        _ => (0, 0, String::new(), snapshot.selected.as_ref().map(|k| k.staged).unwrap_or(false)),
    };
    let busy = if snapshot.refreshing || matches!(snapshot.diff, DiffState::Loading) { "…" } else { "⟳" };
    vec![
        (vec![("‹".into(), Some(Action::PrevFile)), (format!(" {name} {}/{} ", index.map(|i| i + 1).unwrap_or(0), total), None), ("›".into(), Some(Action::NextFile))], 0),
        (vec![(format!("{{}} {hunk_pos}/{hunk_total} "), None), ("↑".into(), Some(Action::PrevHunk)), (" ".into(), None), ("↓".into(), Some(Action::NextHunk))], 1),
        (vec![(if state.mode == ViewMode::Split { "split" } else { "unified" }.into(), Some(Action::ToggleView))], 2),
        (vec![(if staged { "STAGED" } else { "UNSTAGED" }.into(), None)], 3),
        (vec![(stats, None)], 4),
        (vec![("files".into(), Some(Action::ToggleFiles))], 5),
        (vec![(busy.into(), Some(Action::Refresh))], 6),
    ]
}

fn toolbar(snapshot: &Snapshot, state: &ViewState, total_width: u16) -> (Line, Vec<Hit>) {
    let mut items = toolbar_items(snapshot, state);
    let item_width = |item: &Vec<(String, Option<Action>)>| item.iter().map(|(t, _)| width(t)).sum::<usize>();
    // drop from the right until it fits; steppers (drop order 0 and 1) go last
    while items.len() > 1 && items.iter().map(|(i, _)| item_width(i) + 3).sum::<usize>() > usize::from(total_width) {
        let worst = items.iter().enumerate().max_by_key(|(_, (_, order))| *order).map(|(i, _)| i).unwrap();
        items.remove(worst);
    }
    let mut line: Line = vec![Span::body(" ")];
    let mut hits = Vec::new();
    let mut x = 1u16;
    for (item, _) in items {
        for (text, action) in item {
            let w = width(&text) as u16;
            if let Some(action) = action {
                hits.push(Hit { y: 0, x0: x, x1: x + w, action });
                line.push(Span::emphasis(text));
            } else {
                line.push(Span::body(text));
            }
            x += w;
        }
        line.push(Span::body("   "));
        x += 3;
    }
    (line, hits)
}
```

The remaining functions, in this order, complete the file:

- `fn state_message(snapshot: &Snapshot) -> Option<String>`: `RepoState::Unusable { reason }` gives `reason`; `NotARepo` gives `not a git repository`; a `status_error` with no files gives the error; no files gives `working tree clean`; `DiffState::Loading` gives `loading…`; `DiffState::Failed(e)` gives `e`; a `Ready` diff with zero hunks gives `unmerged path: resolve conflicts to see hunks` when `is_conflict(&diff.raw_diff)`, else `binary file or no textual changes`. Otherwise `None`. (The frozen parser reports `UU`, `AA`, `AU` and `UA` as one unstaged `Modified` row, so the status list cannot tell a conflict apart; the raw diff can.)
- `fn files_lines(snapshot, state, height) -> (Vec<String>, Vec<Hit>)`: line 0 `CHANGED n`; one line per file `{marker}{status} {name}` padded to `FILES_WIDTH - 2` plus `S` for staged rows, where marker is `▸` for the selected row else a space, and status is `M A D R ?`. `name` is the basename; when another row has the same basename and a different directory, it is `{basename} {dir}/` with the directory in `Role::Label` (`./` for a file at the repository root), so `src/a.rs` and `a.rs` read `a.rs src/` and `a.rs ./`. Two halves of one path share a directory and are told apart by the `S`. The name is `sanitize`d and `truncate`d; the last line is `+a −d · n files`. Each file line gets `Hit { action: SelectFile(i) }` spanning the panel width.
- `fn body_line(row: &Row, width: u16, hscroll: usize, mode: ViewMode, cursor: bool) -> Line`. Unified format: `{old:>5} {new:>5} {sign} {text}`; split format: two halves of `(width - 1) / 2` columns, each `{no:>5} {sign} {text}`, separated by `│`. `Gap` renders `··· n unmodified lines ···`, `HunkHeader` renders its text, `Truncated` renders `… n more lines not shown`, `FileHeader` renders the path. Text is cut with `hscroll` then `truncate`d to the remaining width. Styles: `+` rows `Style::semantic(Role::Body, Semantic::Good)`, `-` rows `Semantic::Bad`, hunk headers `Semantic::Accent`, numbers and gaps `Role::Label`, the file header `Role::Emphasis`. When `cursor` is true every span of the line gets `reverse = true`, and the line is padded to the full width so the bar spans the pane.
- `pub fn render(...)`: assemble toolbar, body (panel + separator + diff rows `state.offset .. state.offset + body_height`, or the centred `state_message`), the optional line from `notice(state, snapshot)`, and the footer `j/k line  [ ] hunk  n/p file  t view  e files  r refresh  ? help  q quit`, cut from the right with `truncate`. Every diff row at screen line `y` adds `Hit { y, x0: panel_width, x1: width, action: CursorToRow(row_index) }`.

Run: `cargo test tui::view` — Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: add view state reconciliation and the pure view"
```

---

### Task 12: Keys, mouse and outcomes

Spec: 4.1 (handler contract), 4.3, 4.4.

**Files:**
- Create: `src/tui/keys.rs`, `src/tui/input.rs`
- Modify: `src/tui/mod.rs` (add `pub mod input; pub mod keys;`), `src/tui/view.rs` (key-sheet overlay)

**Interfaces:**
- Consumes: `ViewState`, `Rendered`, `Action`, `engine::{Command, Snapshot, DiffState, nav}`, `crossterm::event::{KeyEvent, KeyCode, KeyModifiers, MouseEvent, MouseEventKind, MouseButton}`.
- Produces:

```rust
// tui::keys
pub enum KeyAction { LineDown, LineUp, HalfPageDown, HalfPageUp, HunkPrev, HunkNext, FileNext, FilePrev, SideDeletions, SideAdditions, ToggleView, ToggleFiles, PinFiles, Refresh, First, Last, ScrollLeft, ScrollRight, ToggleMouse, Help, Quit }
pub struct Binding { pub key: &'static str, pub label: &'static str, pub action: KeyAction, pub vimeflow: Option<&'static str> }
pub const KEYS: &[Binding];
pub const RESERVED: &[&str] = &["s", "d", "D", "i", "I", "u", "U", "x", "v", "y", "Y", "@", "c", "/"];
pub fn lookup(key: &crossterm::event::KeyEvent) -> Option<KeyAction>;
pub fn help_panel() -> crate::tui::dialog::Panel;

// tui::input
pub enum Outcome { Quit, Redraw, Inert, Engine(crate::engine::Command) }
pub fn handle_key(state: &mut ViewState, snapshot: &Snapshot, key: crossterm::event::KeyEvent, width: u16) -> Outcome;
pub fn handle_mouse(state: &mut ViewState, snapshot: &Snapshot, rendered: &Rendered, ev: crossterm::event::MouseEvent) -> Outcome;
```

- [ ] **Step 1: Write the key table in `src/tui/keys.rs`**

```rust
pub const KEYS: &[Binding] = &[
    Binding { key: "j", label: "next row", action: KeyAction::LineDown, vimeflow: Some("diff-line-next") },
    Binding { key: "k", label: "previous row", action: KeyAction::LineUp, vimeflow: Some("diff-line-previous") },
    Binding { key: "ctrl+d", label: "half page down", action: KeyAction::HalfPageDown, vimeflow: Some("diff-scroll-page-down") },
    Binding { key: "ctrl+u", label: "half page up", action: KeyAction::HalfPageUp, vimeflow: Some("diff-scroll-page-up") },
    Binding { key: "[", label: "previous hunk", action: KeyAction::HunkPrev, vimeflow: Some("diff-hunk-previous") },
    Binding { key: "]", label: "next hunk", action: KeyAction::HunkNext, vimeflow: Some("diff-hunk-next") },
    Binding { key: "n", label: "next file", action: KeyAction::FileNext, vimeflow: Some("diff-file-next") },
    Binding { key: "p", label: "previous file", action: KeyAction::FilePrev, vimeflow: Some("diff-file-previous") },
    Binding { key: "h", label: "deletions side", action: KeyAction::SideDeletions, vimeflow: Some("diff-side-deletions") },
    Binding { key: "l", label: "additions side", action: KeyAction::SideAdditions, vimeflow: Some("diff-side-additions") },
    Binding { key: "t", label: "unified / split", action: KeyAction::ToggleView, vimeflow: Some("diff-view-toggle") },
    Binding { key: "e", label: "toggle files panel", action: KeyAction::ToggleFiles, vimeflow: Some("diff-files-toggle") },
    Binding { key: "E", label: "pin files panel", action: KeyAction::PinFiles, vimeflow: Some("diff-files-pin") },
    Binding { key: "r", label: "refresh", action: KeyAction::Refresh, vimeflow: Some("diff-refresh") },
    Binding { key: "g", label: "first row", action: KeyAction::First, vimeflow: None },
    Binding { key: "G", label: "last row", action: KeyAction::Last, vimeflow: None },
    Binding { key: "H", label: "scroll left", action: KeyAction::ScrollLeft, vimeflow: None },
    Binding { key: "L", label: "scroll right", action: KeyAction::ScrollRight, vimeflow: None },
    Binding { key: "m", label: "toggle mouse capture", action: KeyAction::ToggleMouse, vimeflow: None },
    Binding { key: "?", label: "this key sheet", action: KeyAction::Help, vimeflow: None },
    Binding { key: "q", label: "quit", action: KeyAction::Quit, vimeflow: None },
];
```

`lookup` renders a `KeyEvent` to the same string form and finds it in `KEYS`: `ctrl+d` for `KeyCode::Char('d')` with `CONTROL`; otherwise the character itself. It ignores `SHIFT` on a character key, because terminals report `E` as `Char('E')` with or without the modifier, and it rejects any event carrying `ALT` or `SUPER`. `help_panel` builds a `dialog::Panel` titled `Keys` with one `Row::Entry` per binding (`key`, `label`) and the footer `esc closes`. Derive `Clone, Copy, Debug, PartialEq, Eq` on `KeyAction`.

- [ ] **Step 2: Write the failing tests in `src/tui/input.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::nav::ViewMode;
    use crate::engine::{Command, DiffState};
    use crate::tui::keys::{KEYS, RESERVED};
    use crate::tui::state::tests::snapshot;
    use crate::tui::state::{FilesPanel, ViewState};
    use crate::tui::view::{body_height, render};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    fn key(text: &str) -> KeyEvent {
        match text.strip_prefix("ctrl+") {
            Some(c) => KeyEvent::new(KeyCode::Char(c.chars().next().unwrap()), KeyModifiers::CONTROL),
            None => KeyEvent::new(KeyCode::Char(text.chars().next().unwrap()), KeyModifiers::NONE),
        }
    }

    fn setup(hunks: &[(u32, &str)]) -> (crate::engine::Snapshot, ViewState) {
        let snap = snapshot("a.rs", "r1", hunks);
        let mut st = ViewState::new(ViewMode::Split, FilesPanel::Hidden, true);
        st.resize(body_height(&st, &snap, 24));
        st.reconcile(&snap);
        (snap, st)
    }

    #[test]
    fn every_key_on_the_sheet_does_something_and_reserved_keys_do_nothing() {
        // A key may be inert at an edge, so each one is tried from four positions:
        // the start (a deletion of a replacement pair), its additions side, the
        // middle of the diff, and a scrolled viewport.
        let long = "+".repeat(80);
        let prefixes: [&[&str]; 4] = [&[], &["l"], &["]", "L"], &["ctrl+d"]];
        for binding in KEYS {
            let live = prefixes.iter().any(|prefix| {
                let (snap, mut st) = setup(&[(10, " --+ "), (40, long.as_str())]);
                for p in prefix.iter() {
                    handle_key(&mut st, &snap, key(p), 120);
                }
                !matches!(handle_key(&mut st, &snap, key(binding.key), 120), Outcome::Inert)
            });
            assert!(live, "`{}` is on the key sheet but does nothing", binding.key);
        }
        for reserved in RESERVED {
            let (snap, mut st) = setup(&[(10, " --+ ")]);
            assert!(matches!(handle_key(&mut st, &snap, key(reserved), 120), Outcome::Inert), "`{reserved}` must stay unbound");
            assert!(crate::tui::keys::KEYS.iter().all(|b| b.key != *reserved));
        }
    }

    #[test]
    fn hunk_keys_land_on_the_first_changed_row() {
        let (snap, mut st) = setup(&[(10, "  + "), (40, " - ")]);
        assert!(matches!(handle_key(&mut st, &snap, key("]"), 120), Outcome::Redraw));
        assert_eq!(st.cursor_id, Some((crate::engine::nav::Side::Deletions, 41)));
        assert!(matches!(handle_key(&mut st, &snap, key("["), 120), Outcome::Redraw));
        assert_eq!(st.cursor_id, Some((crate::engine::nav::Side::Additions, 12)));
    }

    #[test]
    fn file_keys_and_refresh_become_engine_commands() {
        let (snap, mut st) = setup(&[(10, " + ")]);
        assert!(matches!(handle_key(&mut st, &snap, key("n"), 120), Outcome::Engine(Command::SelectNext)));
        assert!(matches!(handle_key(&mut st, &snap, key("p"), 120), Outcome::Engine(Command::SelectPrev)));
        assert!(matches!(handle_key(&mut st, &snap, key("r"), 120), Outcome::Engine(Command::Refresh)));
        assert!(matches!(handle_key(&mut st, &snap, key("q"), 120), Outcome::Quit));
    }

    #[test]
    fn split_is_refused_below_100_columns() {
        let (snap, mut st) = setup(&[(10, " + ")]);
        st.mode = ViewMode::Unified;
        st.reconcile(&snap);
        handle_key(&mut st, &snap, key("t"), 90);
        assert_eq!(st.mode, ViewMode::Unified);
        assert!(st.notice.as_deref().unwrap_or("").contains("100 columns"));
    }

    #[test]
    fn last_row_is_reachable_in_a_diff_longer_than_u16() {
        let kinds = "+".repeat(70_000);
        let (snap, mut st) = setup(&[(1, kinds.as_str())]);
        handle_key(&mut st, &snap, key("G"), 120);
        assert_eq!(st.cursor, Some(69_999));
        let rows = st.rows.as_ref().unwrap().rows.len();
        assert!(st.offset + usize::from(st.body_height) >= rows, "the last row is on screen");
    }

    #[test]
    fn every_binding_is_reachable_on_the_key_sheet_in_a_short_terminal() {
        let (snap, mut st) = setup(&[(10, " + ")]);
        handle_key(&mut st, &snap, key("?"), 120);
        let mut seen = String::new();
        for _ in 0..40 {
            seen.push_str(&render(&snap, &st, 120, 12).plain().join("\n"));
            if matches!(handle_key(&mut st, &snap, key("j"), 120), Outcome::Inert) {
                break;
            }
        }
        for binding in KEYS {
            assert!(seen.contains(binding.label), "`{}` never appears on the sheet at 12 rows", binding.label);
        }
        assert!(matches!(handle_key(&mut st, &snap, key("q"), 120), Outcome::Redraw), "q closes the sheet");
        assert!(!st.help_open);
    }

    #[test]
    fn a_diff_without_targets_makes_movement_inert() {
        let (snap, mut st) = setup(&[]);
        for k in ["j", "k", "[", "]", "h", "l", "g", "G"] {
            assert!(matches!(handle_key(&mut st, &snap, key(k), 120), Outcome::Inert), "`{k}`");
        }
    }

    #[test]
    fn a_toolbar_click_acts_like_its_key_and_the_wheel_scrolls_three_rows() {
        let kinds = "+".repeat(200);
        let (snap, mut st) = setup(&[(1, kinds.as_str())]);
        let r = render(&snap, &st, 120, 24);
        let hit = r.hits.iter().find(|h| matches!(h.action, crate::tui::view::Action::NextFile)).unwrap();
        let click = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: hit.x0, row: hit.y, modifiers: KeyModifiers::NONE };
        assert!(matches!(handle_mouse(&mut st, &snap, &r, click), Outcome::Engine(Command::SelectNext)));
        let wheel = MouseEvent { kind: MouseEventKind::ScrollDown, column: 50, row: 10, modifiers: KeyModifiers::NONE };
        assert!(matches!(handle_mouse(&mut st, &snap, &r, wheel), Outcome::Redraw));
        assert_eq!(st.offset, 3);
        // the run loop's redraw preparation must not pull the viewport back to the cursor
        st.resize(body_height(&st, &snap, 24));
        st.reconcile(&snap);
        assert_eq!(st.offset, 3, "wheel scrolling was undone by the next frame");
        let shifted = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: hit.x0, row: hit.y, modifiers: KeyModifiers::SHIFT };
        assert!(matches!(handle_mouse(&mut st, &snap, &r, shifted), Outcome::Inert));
        let _ = DiffState::Idle;
    }
}
```

Run: `cargo test tui::input` — Expected: FAIL (does not compile).

- [ ] **Step 3: Implement `src/tui/input.rs`**

Rules, each a few lines over `ViewState` and `engine::nav`:

- When `state.help_open`: `Esc`, `?` and `q` close it and reset `help_offset` (`Redraw`). The sheet has 21 rows and may not fit, so `j` / `k` and the wheel move `state.help_offset` by one and three rows, clamped to `dialog::line_count(&help_panel(), panel_width).saturating_sub(panel_height)`; they are `Inert` at the ends. Every other key is `Inert`.
- In `view::render` (this task wires it; Task 11 drew no overlay): when `state.help_open`, take `let mut panel = keys::help_panel(); panel.offset = state.help_offset;` (its `cursor` is `None`, which is the watcher dialog's scrolling mode), call `dialog::render(&panel, min(width, 60), height.saturating_sub(2))` and overlay the result centred over the body.
- Resolve the key with `keys::lookup`; an unknown key is `Inert`. Clear `state.notice` on any handled key.
- With `DiffState::Ready(diff)` and `Some(cursor)`: `LineDown`/`LineUp` call `nav::move_line(&diff.targets, &diff.unified_order, cursor, ±1, state.mode)`; `SideDeletions`/`SideAdditions` call `nav::move_side`; `HunkNext`/`HunkPrev` call `nav::target_index_for_hunk` for `hunk_index ± 1`; `First`/`Last` pick the first or last entry of the display order (`unified_order` in unified mode, `0` and `len - 1` in split mode). If the target did not change, return `Inert`; otherwise `state.set_cursor(diff, target)` and `Redraw`.
- `HalfPageDown`/`HalfPageUp`: move `state.offset` by `body_height / 2` through `layout::clamp_scroll`; then set the cursor to the target whose row is nearest `offset + body_height / 2` (scan `rows.row_of_target`); `Inert` when the offset did not change.
- `ScrollRight` adds 8 to `hscroll`; `ScrollLeft` subtracts 8 with saturation and is `Inert` at 0.
- `ToggleView`: switching to split with `width < view::MIN_SPLIT_WIDTH` sets `state.notice = Some("split view needs 100 columns")` and keeps unified; otherwise flip `state.mode` (the next `reconcile` rebuilds rows and keeps the line).
- `ToggleFiles` flips `Hidden` and `Shown` (a `Pinned` panel becomes `Hidden`); `PinFiles` flips `Pinned` and `Shown`.
- `ToggleMouse` flips `state.mouse_requested`; the run loop applies it to the terminal.
- `Help` sets `help_open`; `Refresh`, `FileNext`, `FilePrev` return `Engine(Command::Refresh | SelectNext | SelectPrev)`; `Quit` returns `Quit`.
- Without a ready diff or without a cursor, every movement action is `Inert`.
- `handle_mouse`: ignore everything unless `modifiers` is empty. `ScrollDown`/`ScrollUp` move `offset` by 3 through `clamp_scroll` (`Inert` when unchanged). `Down(Left)` looks up `rendered.hit(column, row)`: `PrevFile`/`NextFile`/`Refresh`/`SelectFile(i)` become `Engine` commands (`SelectFile(i)` is `Command::Select` of `snapshot.files[i]`); `PrevHunk`, `NextHunk`, `ToggleView`, `ToggleFiles` reuse the key path; `CursorToRow(row)` sets the cursor to the target whose `row_of_target` equals `row`, if any. With the key sheet open, mouse events are `Inert`.

Run: `cargo test tui::input` — Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat: add key table, mouse handling and handler outcomes"
```

---

### Task 13: Configuration, the shell and the `tui` subcommand

Spec: 2.3, 4.1 (shell), 4.4, 4.7, 5.1.

**Files:**
- Create: `src/tui/config.rs`, `src/tui/shell.rs`
- Modify: `src/tui/mod.rs`, `src/main.rs`

**Interfaces:**
- Consumes: everything above.
- Produces: `tui::config::{Config, load(dir: &Path) -> (Config, Vec<String>)}`; `tui::shell::run(path: PathBuf) -> i32`; `main` dispatching `tui [PATH]` (the default).

- [ ] **Step 1: Write the failing config tests in `src/tui/config.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_the_file_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let (config, problems) = load(dir.path());
        assert_eq!((config.mode, config.files, config.mouse), (ModeSetting::Auto, FilesSetting::Auto, true));
        assert!(problems.is_empty());
    }

    #[test]
    fn one_bad_value_costs_one_key() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "[view]\nmode = \"sideways\"\nfiles = \"pinned\"\n[input]\nmouse = false\n").unwrap();
        let (config, problems) = load(dir.path());
        assert_eq!((config.mode, config.files, config.mouse), (ModeSetting::Auto, FilesSetting::Pinned, false));
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("view.mode"));
    }

    #[test]
    fn unparseable_toml_falls_back_entirely() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "[view\n").unwrap();
        let (config, problems) = load(dir.path());
        assert!(config.mouse && problems.len() == 1);
    }
}
```

- [ ] **Step 2: Implement `src/tui/config.rs`**

```rust
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeSetting { Auto, Unified, Split }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesSetting { Auto, Pinned, Hidden }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub mode: ModeSetting,
    pub files: FilesSetting,
    pub mouse: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self { mode: ModeSetting::Auto, files: FilesSetting::Auto, mouse: true }
    }
}

/// `$HERDR_PLUGIN_CONFIG_DIR`, else `${XDG_CONFIG_HOME:-~/.config}/herdr-hunks`.
pub fn config_dir() -> std::path::PathBuf {
    if let Some(dir) = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR") {
        return dir.into();
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    base.join("herdr-hunks")
}

pub fn load(dir: &Path) -> (Config, Vec<String>) {
    let mut config = Config::default();
    let mut problems = Vec::new();
    let Ok(text) = std::fs::read_to_string(dir.join("config.toml")) else {
        return (config, problems);
    };
    let table: toml::Table = match text.parse() {
        Ok(t) => t,
        Err(e) => {
            problems.push(format!("config.toml: {e}"));
            return (config, problems);
        }
    };
    let get = |section: &str, key: &str| table.get(section).and_then(|s| s.get(key)).cloned();
    match get("view", "mode").map(|v| v.as_str().map(str::to_owned)) {
        None => {}
        Some(Some(s)) if s == "auto" => config.mode = ModeSetting::Auto,
        Some(Some(s)) if s == "unified" => config.mode = ModeSetting::Unified,
        Some(Some(s)) if s == "split" => config.mode = ModeSetting::Split,
        Some(other) => problems.push(format!("view.mode: expected \"auto\", \"unified\" or \"split\", got {other:?}")),
    }
    match get("view", "files").map(|v| v.as_str().map(str::to_owned)) {
        None => {}
        Some(Some(s)) if s == "auto" => config.files = FilesSetting::Auto,
        Some(Some(s)) if s == "pinned" => config.files = FilesSetting::Pinned,
        Some(Some(s)) if s == "hidden" => config.files = FilesSetting::Hidden,
        Some(other) => problems.push(format!("view.files: expected \"auto\", \"pinned\" or \"hidden\", got {other:?}")),
    }
    match get("input", "mouse").map(|v| v.as_bool()) {
        None => {}
        Some(Some(b)) => config.mouse = b,
        Some(None) => problems.push("input.mouse: expected true or false".to_string()),
    }
    (config, problems)
}
```

Run: `cargo test tui::config` — Expected: PASS.

- [ ] **Step 3: Implement `src/tui/shell.rs`**

`pub fn run(path: PathBuf) -> i32`, in this order:

1. If stdout is not a terminal (`std::io::IsTerminal`), print `herdr-hunks: stdout is not a terminal` to stderr and return 2.
2. `crate::engine::init_process_env()` first, before any thread exists.
3. Load the config; write any problems to `config-problems.log` in `$HERDR_PLUGIN_STATE_DIR` (else `${XDG_STATE_HOME:-~/.local/state}/herdr-hunks`) and keep `config: N problem(s), see config-problems.log` as the initial `state.notice`.
4. Build a two-worker tokio runtime and `engine::spawn(rt.handle(), SessionConfig::production(path))`.
5. Install a panic hook that leaves the alternate screen, disables mouse capture and raw mode, then calls the previous hook.
6. Create `TerminalGuard` and a ratatui `Terminal<CrosstermBackend<Stdout>>`. Resolve `Auto` settings from the first frame size: split at 120 columns or more, files panel pinned at 100 or more.
7. Loop, mirroring `$WATCHER/src/sidebar/tui.rs` `run` (line 2265 onward):
   - apply `guard::mouse_transition(state.mouse_requested, last_attempted)` through `guard.set_mouse`;
   - drain `handle.snapshots` with `try_recv`, keeping the newest; mark dirty when one arrived;
   - if dirty: `state.resize(view::body_height(..))`, `state.reconcile(&snapshot)`, `let rendered = view::render(..)`, draw each `style::Line` as a ratatui `Line` at its row, clear dirty;
   - `guard::poll_terminal(Duration::from_millis(100))`: `Err` or a hang-up exits the loop; `Ok(true)` reads one `crossterm::event::read()`;
   - `Event::Key` goes to `input::handle_key`, `Event::Mouse` to `input::handle_mouse`, `Event::Resize` marks dirty;
   - `Outcome::Quit` breaks, `Redraw` marks dirty, `Engine(cmd)` sends it on `handle.commands` and marks dirty, `Inert` does nothing.
8. On exit send `Command::Shutdown`, drop the terminal and the guard, return 0.

Style mapping (`fn to_ratatui(style: &Style) -> ratatui::style::Style`), named ANSI colours only: `Semantic::Good` is `Color::Green`, `Bad` is `Color::Red`, `Accent` is `Color::Cyan`, `Warn` is `Color::Yellow`; `Role::Label` and `Role::Rule` add `Modifier::DIM`; `Role::Emphasis` adds `Modifier::BOLD`; `reverse` adds `Modifier::REVERSED`. Never emit `Color::Rgb` or a background colour.

- [ ] **Step 4: Wire `src/main.rs`**

```rust
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let first = args.next();
    let code = match first.as_deref() {
        None => herdr_hunks::tui::shell::run(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))),
        Some("tui") => {
            let path = args.next().map(PathBuf::from).or_else(|| std::env::current_dir().ok()).unwrap_or_else(|| PathBuf::from("."));
            herdr_hunks::tui::shell::run(path)
        }
        Some("--version") | Some("-V") => {
            println!("herdr-hunks {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Some(flag) if flag.starts_with('-') => {
            eprintln!("herdr-hunks: unknown option `{flag}` (expected: tui [PATH], open, open-split, update)");
            2
        }
        // Any other word is a path. The engine validates it, so a missing path shows the error state.
        Some(path) => herdr_hunks::tui::shell::run(PathBuf::from(path)),
    };
    std::process::exit(code);
}
```

- [ ] **Step 5: Manual smoke test, then commit**

```bash
cargo build --release
cd "$(mktemp -d)" && git init -q -b main && printf 'a\nb\nc\n' > f.txt && git add -A && git -c user.email=t@e -c user.name=t commit -qm init \
  && printf 'a\nB\nc\nd\n' > f.txt && printf 'new\n' > n.txt && "$OLDPWD/target/release/herdr-hunks"
```

Check by hand: the toolbar reads `‹ f.txt 1/2 ›`; `j k [ ] n p t e r ? q` behave as in spec 4.3; clicking `›` selects `n.txt`; the wheel scrolls; `m` restores native text selection; editing `f.txt` in another terminal updates the view without a keypress; `herdr-hunks /nonexistent` shows an error state and quits with `q`; `herdr-hunks | cat` exits 2.

```bash
git add -A
git commit -m "feat: add configuration, the terminal shell and the tui subcommand"
```

---

### Task 14: herdr client and the `open` / `open-split` actions

Spec: 2.5, 5.2 item 9.

**Files:**
- Create: `src/herdr/mod.rs`, `src/herdr/client.rs`, `src/herdr/api.rs`, `src/actions/mod.rs`, `src/actions/open.rs`, `src/actions/reuse.rs`, `tests/support/mod.rs`, `tests/actions_tier_a.rs`, `tests/fixtures/herdr-0.8.0-schema.json`
- Modify: `src/lib.rs` (add `pub mod actions; pub mod herdr;`), `src/main.rs`, `PORT-SURFACE.md`

**Interfaces:**
- Consumes: nothing from the engine or the TUI.
- Produces:

```rust
// herdr::client (copied): HerdrClient::from_env(), HerdrClient::request(&self, method: &str, params: Value) -> Result<Value, HerdrClientError>
// herdr::api
pub struct PaneInfo { pub pane_id: String, pub label: Option<String>, pub cwd: Option<String>, pub foreground_cwd: Option<String> }
impl HerdrClient {
    pub fn pane_get(&self, pane_id: &str) -> Result<PaneInfo, HerdrClientError>;
    pub fn plugin_pane_open(&self, params: serde_json::Value) -> Result<Option<String>, HerdrClientError>; // returns the new pane id
    pub fn plugin_pane_focus(&self, pane_id: &str) -> Result<(), HerdrClientError>;
}
// actions
pub const VIEWER_TITLE: &str = "Hunks";
pub enum Placement { Overlay, Split }
pub fn open_params(plugin_id: &str, placement: Placement, opener: Option<&str>, repo_cwd: &str) -> serde_json::Value;
pub fn run_open(placement: Placement) -> i32;
// actions::reuse
pub struct Record { pub viewer_pane_id: String, pub repo_cwd: String, pub toplevel: String }
pub fn key(socket_path: &str, opener: &str) -> String;
pub fn load(state_dir: &Path) -> BTreeMap<String, Record>;
pub fn save(state_dir: &Path, records: &BTreeMap<String, Record>) -> std::io::Result<()>;
pub fn may_reuse(record: &Record, toplevel: &str, viewer: Option<&PaneInfo>) -> bool;
/// Holds an exclusive flock on `<state_dir>/split-panes.lock` while `f` runs.
pub fn with_lock<T>(state_dir: &Path, f: impl FnOnce() -> T) -> std::io::Result<T>;
```

- [ ] **Step 1: Copy the client and the test support**

```bash
mkdir -p src/herdr src/actions tests/support tests/fixtures
cp "$WATCHER/src/herdr/client.rs" src/herdr/client.rs
cp "$WATCHER/tests/fixtures/ping-response.json" tests/fixtures/ping-response.json   # client.rs includes it in a test
cp "$WATCHER/tests/support/mod.rs" tests/support/mod.rs
"$HERDR_BIN_PATH" api schema --json > tests/fixtures/herdr-0.8.0-schema.json   # from a herdr 0.8.0 session
```

`src/herdr/mod.rs` is `pub mod api; pub mod client;`. In `tests/support/mod.rs` keep `FakeHerdr` (socket accept loop, object-`params` enforcement, `calls_named`, `stop`) and `wait_for`. Delete line 1 (`pub mod fake_herdr;`, a fake CLI this plan does not use), the `use rusqlite::{params, Connection};` and `use sha2::{Digest, Sha256};` imports, the agent fixtures that needed them (`write_claude_fixture` and the three after it) and `state_snapshot`. `cargo test --test actions_tier_a --no-run` must compile without `rusqlite`. Replace its response table with: `pane.get` returns `{"type":"pane","pane": <the pane set by set_panes whose pane_id matches>}` or `{"error":{"code":"pane_not_found","message":"no such pane"}}`; `plugin.pane.open` returns `{"type":"plugin_pane_opened","plugin_pane":{"pane":{"pane_id":"w1:p9"}}}`; `plugin.pane.focus` returns `{"type":"ok"}` unless `fail_focus(true)` was called, then an error. Register both copies in `PORT-SURFACE.md`.

- [ ] **Step 2: Write the failing pure tests in `src/actions/mod.rs` and `src/actions/reuse.rs`**

```rust
// src/actions/mod.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_params_carry_no_target_or_direction() {
        let p = open_params("winoooops.hunks", Placement::Overlay, Some("w1:p1"), "/repo");
        assert_eq!(p["placement"], "overlay");
        assert!(p.get("target_pane_id").is_none() && p.get("direction").is_none());
        assert_eq!(p["cwd"], "/repo");
        assert_eq!(p["env"]["HERDR_HUNKS_OPENER_PANE"], "w1:p1");
        assert_eq!(p["entrypoint"], "viewer");
        assert_eq!(p["focus"], true);
    }

    #[test]
    fn split_params_target_the_opener_to_the_right() {
        let p = open_params("winoooops.hunks", Placement::Split, Some("w1:p1"), "/repo");
        assert_eq!((p["placement"].as_str(), p["target_pane_id"].as_str(), p["direction"].as_str()), (Some("split"), Some("w1:p1"), Some("right")));
    }
}
```

```rust
// src/actions/reuse.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::api::PaneInfo;

    fn record() -> Record {
        Record { viewer_pane_id: "w1:p9".into(), repo_cwd: "/repo/sub".into(), toplevel: "/repo".into() }
    }

    fn viewer(label: &str, cwd: &str) -> PaneInfo {
        PaneInfo { pane_id: "w1:p9".into(), label: Some(label.into()), cwd: Some(cwd.into()), foreground_cwd: None }
    }

    #[test]
    fn reuse_needs_the_same_toplevel_title_and_cwd() {
        assert!(may_reuse(&record(), "/repo", Some(&viewer("Hunks", "/repo/sub"))));
        assert!(!may_reuse(&record(), "/other", Some(&viewer("Hunks", "/repo/sub"))));
        assert!(!may_reuse(&record(), "/repo", Some(&viewer("zsh", "/repo/sub"))));
        assert!(!may_reuse(&record(), "/repo", Some(&viewer("Hunks", "/elsewhere"))));
        assert!(!may_reuse(&record(), "/repo", None));
    }

    #[test]
    fn keys_are_scoped_by_socket_path() {
        assert_ne!(key("/a/herdr.sock", "w1:p1"), key("/b/herdr.sock", "w1:p1"));
    }

    #[test]
    fn concurrent_updates_under_the_lock_keep_every_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let workers: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    with_lock(&path, || {
                        let mut map = load(&path);
                        std::thread::sleep(std::time::Duration::from_millis(15));
                        map.insert(key("/s/herdr.sock", &format!("w1:p{i}")), record());
                        save(&path, &map).unwrap();
                    })
                    .unwrap();
                })
            })
            .collect();
        for w in workers {
            w.join().unwrap();
        }
        assert_eq!(load(&path).len(), 8, "a read-modify-write was lost");
    }

    #[test]
    fn an_unreadable_file_is_an_empty_map_and_save_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("split-panes.json"), "{not json").unwrap();
        assert!(load(dir.path()).is_empty());
        let mut map = std::collections::BTreeMap::new();
        map.insert(key("/a/herdr.sock", "w1:p1"), record());
        save(dir.path(), &map).unwrap();
        assert_eq!(load(dir.path()).len(), 1);
    }
}
```

Run: `cargo test actions` — Expected: FAIL (does not compile).

- [ ] **Step 3: Implement**

`src/herdr/api.rs`: `PaneInfo` derives `Deserialize` with every field but `pane_id` `#[serde(default)]`. `pane_get` sends `pane.get {"pane_id": id}` and deserializes `result["pane"]`. `plugin_pane_open` sends `plugin.pane.open` and returns `result["plugin_pane"]["pane"]["pane_id"]` as `Option<String>`. `plugin_pane_focus` sends `plugin.pane.focus {"pane_id": id}`.

`src/actions/reuse.rs`:

```rust
use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::herdr::api::PaneInfo;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Record {
    pub viewer_pane_id: String,
    pub repo_cwd: String,
    pub toplevel: String,
}

/// Pane ids are session-local and the state directory is shared, so the socket scopes the key.
pub fn key(socket_path: &str, opener: &str) -> String {
    format!("{socket_path}\u{1f}{opener}")
}

pub fn load(state_dir: &Path) -> BTreeMap<String, Record> {
    std::fs::read_to_string(state_dir.join("split-panes.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save(state_dir: &Path, records: &BTreeMap<String, Record>) -> std::io::Result<()> {
    std::fs::create_dir_all(state_dir)?;
    let tmp = state_dir.join(format!("split-panes.json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(records).unwrap_or_default())?;
    std::fs::rename(tmp, state_dir.join("split-panes.json"))
}

pub fn with_lock<T>(state_dir: &Path, f: impl FnOnce() -> T) -> std::io::Result<T> {
    use std::os::unix::io::AsRawFd;
    std::fs::create_dir_all(state_dir)?;
    let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(state_dir.join("split-panes.lock"))?;
    // flock is released when `file` drops, including on panic.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(f())
}

pub fn may_reuse(record: &Record, toplevel: &str, viewer: Option<&PaneInfo>) -> bool {
    let Some(viewer) = viewer else { return false };
    record.toplevel == toplevel
        && viewer.label.as_deref() == Some(crate::actions::VIEWER_TITLE)
        && viewer.cwd.as_deref() == Some(record.repo_cwd.as_str())
}
```

`src/actions/mod.rs`: `open_params` builds the two shapes of spec 2.5 with `serde_json::json!`. `run_open(placement)`:

1. Parse `$HERDR_PLUGIN_CONTEXT_JSON`; `opener = context["focused_pane_id"]`.
2. `pane = client.pane_get(opener)` when an opener exists. If that pane's label is `VIEWER_TITLE`, the focused pane is already a viewer: print `herdr-hunks: already in the hunk viewer` and return 0 without opening anything. Otherwise `repo_cwd = pane.foreground_cwd`, else `pane.cwd`, else `context["focused_pane_cwd"]`, else `context["workspace_cwd"]`. With none of them, print `herdr-hunks: no working directory for the focused pane` and return 1.
3. `Placement::Overlay` goes straight to step 4. For `Placement::Split`, steps 3 and 4 run inside one `reuse::with_lock(state_dir, ...)`, so two invocations cannot both miss the record and open two viewers, and two sessions cannot overwrite each other's records. `toplevel` is the trimmed stdout of `git -C <repo_cwd> rev-parse --show-toplevel` (or `repo_cwd` when that fails); `records = reuse::load(state_dir)`; if a record exists under `reuse::key(socket, opener)` and `may_reuse(record, toplevel, client.pane_get(&record.viewer_pane_id).ok().as_ref())` and `client.plugin_pane_focus(..)` is `Ok`, return 0.
4. `client.plugin_pane_open(open_params(plugin_id, placement, opener, repo_cwd))`, where `plugin_id` is `$HERDR_PLUGIN_ID`. herdr always sets it for an action; when it is absent, print `herdr-hunks: HERDR_PLUGIN_ID is not set (run this through a herdr plugin action)` and return 1. The id is never hardcoded. For `Split`, store `Record { viewer_pane_id, repo_cwd, toplevel }` and `save`, still under the lock.
5. Any client error prints `herdr-hunks: <error>` to stderr and returns 1.

`src/main.rs`: add arms `Some("open") => herdr_hunks::actions::run_open(Placement::Overlay)` and `Some("open-split") => herdr_hunks::actions::run_open(Placement::Split)`, and call `herdr_hunks::engine::init_process_env()` at the top of `main`.

- [ ] **Step 4: Write the tier A tests in `tests/actions_tier_a.rs`**

Each test starts `support::FakeHerdr`, sets `HERDR_SOCKET_PATH`, `HERDR_PLUGIN_ID=winoooops.hunks`, `HERDR_PLUGIN_STATE_DIR=<tempdir>` and `HERDR_PLUGIN_CONTEXT_JSON={"focused_pane_id":"w1:p1"}`, and calls `herdr_hunks::actions::run_open`. The tests share process environment, so guard them with one `static LOCK: std::sync::Mutex<()>`. Cases:

1. `open` sends one `plugin.pane.open` whose params equal the overlay shape and whose `cwd` is the opener's `foreground_cwd`; no `plugin.pane.focus`.
2. `open-split` twice from the same opener and repository: the first call opens and writes `split-panes.json`; with the fake now reporting pane `w1:p9` labelled `Hunks` at the recorded cwd, the second call sends `plugin.pane.focus {"pane_id":"w1:p9"}` and no second open.
3. The same record under a different `HERDR_SOCKET_PATH`: a second open, no focus.
4. The recorded pane relabelled `zsh`: a second open.
5. `fail_focus(true)`: a second open after the failed focus.
5b. With `HERDR_PLUGIN_ID=someone.else`, the open request carries `"plugin_id":"someone.else"`; with the variable unset, no request is sent and the exit code is 1.
5a. The focused pane is itself labelled `Hunks`: no `plugin.pane.open` and no `plugin.pane.focus` is sent, and the exit code is 0.
6. Every recorded request's params validate against `tests/fixtures/herdr-0.8.0-schema.json`: assert each param key exists in the schema's definition for that method, and each `required` key is present.
7. `rg -n '"herdr"' src/` finds nothing (run it through `std::process::Command` and assert an empty stdout): the host binary is never named literally.

Run: `cargo test --test actions_tier_a && cargo test actions` — Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: add herdr client and the open and open-split actions"
```

---

### Task 15: Manifest, distribution, CI and documentation

Spec: 2.5 (manifest), 6.1, 6.2.

**Files:**
- Create: `herdr-plugin.toml`, `scripts/fetch-or-build.sh`, `src/actions/update.rs`, `tests/version_sync.rs`, `.github/workflows/ci.yml`, `.github/workflows/release.yml`, `README.md`, `README.zh-CN.md`, `README.ja.md`, `AGENTS.md`, `CLAUDE.md`, `LICENSE`
- Modify: `src/actions/mod.rs`, `src/main.rs`

**Interfaces:**
- Consumes: `actions::run_open`.
- Produces: `actions::update::{latest_tag(ls_remote_output: &str) -> Option<String>, run_with(host: &Path, git: &str, plugin_id: &str) -> i32, run() -> i32}`. `run()` is `run_with($HERDR_BIN_PATH, "git", $HERDR_PLUGIN_ID)`.

- [ ] **Step 1: Write `herdr-plugin.toml`**

```toml
id = "winoooops.hunks"
name = "hunks"
version = "0.1.0"
min_herdr_version = "0.8.0"
description = "Read-only git hunk viewer: changed files, hunks, and a clickable navigation toolbar."
platforms = ["macos", "linux"]

[[build]]
command = ["sh", "scripts/fetch-or-build.sh"]
platforms = ["macos", "linux"]

[[panes]]
id = "viewer"
title = "Hunks"
description = "Git hunk viewer for the focused pane's worktree"
placement = "overlay"
command = ["/bin/sh", "-lc", "exec \"$HERDR_PLUGIN_ROOT/target/release/herdr-hunks\" tui"]

[[actions]]
id = "open"
title = "Open the hunk viewer over the focused pane"
command = ["target/release/herdr-hunks", "open"]

[[actions]]
id = "open-split"
title = "Open the hunk viewer in a split beside the focused pane"
command = ["target/release/herdr-hunks", "open-split"]

[[actions]]
id = "update"
title = "Install the latest release"
command = ["target/release/herdr-hunks", "update"]
```

- [ ] **Step 2: Write the failing tests**

`tests/version_sync.rs`:

```rust
#[test]
fn manifest_and_crate_versions_match() {
    let manifest: toml::Table = std::fs::read_to_string("herdr-plugin.toml").unwrap().parse().unwrap();
    assert_eq!(manifest["version"].as_str(), Some(env!("CARGO_PKG_VERSION")));
    assert_eq!(manifest["id"].as_str(), Some("winoooops.hunks"));
    assert_eq!(manifest["panes"][0]["title"].as_str(), Some(herdr_hunks::actions::VIEWER_TITLE));
}
```

In `src/actions/update.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::latest_tag;

    #[test]
    fn picks_the_highest_semver_tag() {
        let out = "aaa\trefs/tags/v0.1.0\nbbb\trefs/tags/v0.10.0\nccc\trefs/tags/v0.9.3\nddd\trefs/tags/v0.10.0^{}\neee\trefs/tags/nightly\n";
        assert_eq!(latest_tag(out).as_deref(), Some("v0.10.0"));
        assert_eq!(latest_tag("eee\trefs/tags/nightly\n"), None);
    }

    /// Writes an executable shell script and returns its path.
    fn script(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn host(dir: &std::path::Path, kind: &str, install_exit: i32) -> std::path::PathBuf {
        let log = dir.join("host.log");
        script(dir, "host", &format!(
            "echo \"$*\" >> '{}'\nif [ \"$1 $2\" = 'plugin list' ]; then echo '{{\"result\":{{\"plugins\":[{{\"plugin_id\":\"winoooops.hunks\",\"source\":{{\"kind\":\"{kind}\"}}}}]}}}}'; exit 0; fi\nexit {install_exit}",
            log.display()
        ))
    }

    #[test]
    fn a_linked_install_is_refused_and_nothing_is_installed() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path(), "local", 0);
        let git = script(dir.path(), "git", "echo 'aaa\trefs/tags/v9.9.9'");
        assert_ne!(super::run_with(&host, git.to_str().unwrap(), "winoooops.hunks"), 0);
        assert!(!std::fs::read_to_string(dir.path().join("host.log")).unwrap().contains("plugin install"));
    }

    #[test]
    fn a_github_install_installs_the_newest_tag_and_propagates_failure() {
        let dir = tempfile::tempdir().unwrap();
        let git = script(dir.path(), "git", "printf 'aaa\\trefs/tags/v9.9.9\\nbbb\\trefs/tags/v9.10.0\\n'");
        let ok = host(dir.path(), "github", 0);
        assert_eq!(super::run_with(&ok, git.to_str().unwrap(), "winoooops.hunks"), 0);
        let log = std::fs::read_to_string(dir.path().join("host.log")).unwrap();
        assert!(log.contains("plugin install winoooops/herdr-hunks --ref v9.10.0 --yes"), "{log}");
        let failing = host(dir.path(), "github", 7);
        assert_eq!(super::run_with(&failing, git.to_str().unwrap(), "winoooops.hunks"), 7);
    }
}
```

Run: `cargo test --test version_sync && cargo test update` — Expected: FAIL.

- [ ] **Step 3: Implement `src/actions/update.rs`**

```rust
const REPO: &str = "winoooops/herdr-hunks";

pub fn latest_tag(ls_remote_output: &str) -> Option<String> {
    ls_remote_output
        .lines()
        .filter_map(|l| l.split('\t').nth(1))
        .filter_map(|r| r.strip_prefix("refs/tags/"))
        .filter(|t| !t.ends_with("^{}"))
        .filter_map(|t| {
            let mut parts = t.strip_prefix('v')?.split('.').map(|p| p.parse::<u64>());
            let version = (parts.next()?.ok()?, parts.next()?.ok()?, parts.next()?.ok()?);
            parts.next().is_none().then(|| (version, t.to_string()))
        })
        .max_by_key(|(version, _)| *version)
        .map(|(_, tag)| tag)
}
```

`run_with(host, git, plugin_id)`: run `<host> plugin list --json`, find the entry whose `plugin_id` equals `plugin_id`, and refuse (print why, return 1) unless its `source.kind` is `github`. Run `<git> ls-remote --tags https://github.com/winoooops/herdr-hunks`; `latest_tag`; with no tag, print the reason and return 1; if the tag equals `v` + `CARGO_PKG_VERSION`, print `already up to date` and return 0; otherwise run `<host> plugin install winoooops/herdr-hunks --ref <tag> --yes` and return its exit code. `run()` reads `$HERDR_BIN_PATH` and `$HERDR_PLUGIN_ID` (missing: print the reason, return 1) and calls `run_with`. Add the `Some("update")` arm to `main`.

- [ ] **Step 4: Distribution files**

```bash
cp "$WATCHER/scripts/fetch-or-build.sh" scripts/fetch-or-build.sh
cp "$WATCHER/.github/workflows/release.yml" .github/workflows/release.yml
cp "$WATCHER/LICENSE" LICENSE
```

In both copied files replace `herdr-agent-watcher` with `herdr-hunks` and `HERDR_AGENT_WATCHER_RELEASE_BASE` with `HERDR_HUNKS_RELEASE_BASE`; keep the four targets, the `SHA256SUMS` two-space format and the tag-equals-version guard. Register both in `PORT-SURFACE.md`.

`.github/workflows/ci.yml`:

```yaml
name: ci
on: [push, pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { components: "rustfmt, clippy" }
      - run: git --version
      - run: cargo fmt --check
      - run: cargo clippy --all-targets
      - run: cargo test -- --test-threads=1
      - run: cargo check --no-default-features
  port-check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/checkout@v4
        with: { repository: winoooops/vimeflow, path: _vimeflow, fetch-depth: 0 }
      - run: scripts/port-check.sh _vimeflow
```

- [ ] **Step 5: Documentation**

`README.md`, in `herdr-agent-watcher`'s section order: Install (`herdr plugin install winoooops/herdr-hunks`; fork users run the same command with their `vimeflow` binary, because the registries are separate), Commands (the three actions and `herdr-hunks [PATH]`), Keybinding (the snippet below), Keys (the table of spec 4.3), Mouse (`m`), Configuration (spec 4.7), Requirements (git 2.31 or newer; macOS or Linux; herdr 0.8.0 or newer), Known limitations (K5; `GIT_NO_LAZY_FETCH` from git 2.45; read-only in this release), Roadmap (spec 1.2), Local development (`herdr plugin link "$PWD"` skips `[[build]]`, so run `cargo build --release` first), Licence.

```toml
[[keys.command]]
key = "prefix+d"
type = "plugin_action"
command = "winoooops.hunks.open"
description = "Open the hunk viewer"
```

`README.zh-CN.md` and `README.ja.md` mirror it section for section. `AGENTS.md` states the frozen-tree rule, the port-check command, the pre-commit gate from **Global Constraints**, and the reserved keys. `CLAUDE.md` is one line: `See AGENTS.md.`

- [ ] **Step 6: Run everything and commit**

Run: `cargo fmt --check && cargo test -- --test-threads=1 && cargo check --no-default-features && scripts/port-check.sh "$VIMEFLOW" && sh -n scripts/fetch-or-build.sh` — Expected: PASS.

```bash
git add -A
git commit -m "feat: add manifest, distribution scripts, ci and documentation"
```

---

### Task 16: Tier B end-to-end test and release acceptance

Spec: 5.2 item 10, 5.3, 1.5.

**Files:**
- Create: `tests/e2e_real_herdr.rs`, `docs/acceptance-p1.md`

**Interfaces:**
- Consumes: the built binary and a real `herdr` 0.8.0 on `PATH`.
- Produces: nothing.

- [ ] **Step 1: Build the release binary the manifest points at**

`herdr plugin link` skips `[[build]]`, the manifest's commands point at `target/release/herdr-hunks`, and `cargo test` never rebuilds that file. The last release build was in Task 13, before the actions existed.

Run: `cargo build --release && target/release/herdr-hunks --version`
Expected: `herdr-hunks 0.1.0`.

- [ ] **Step 2: Write the ignored test in `tests/e2e_real_herdr.rs`**

The isolation pattern is the one in `$WATCHER/tests/e2e_real_herdr.rs` (a temp dir under `/tmp` because macOS caps Unix socket paths at 103 bytes; isolated `HOME`, `XDG_CONFIG_HOME` and `XDG_STATE_HOME`; a named session; the user's real `session.json` mtime asserted unchanged). That file builds its environment inline, so nothing is copied from it except the two small items marked below.

```rust
use herdr_hunks::herdr::client::HerdrClient;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

struct ProcessGuard(Child); // as in the watcher's test
impl Drop for ProcessGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn modified(path: &Path) -> Option<SystemTime> { // as in the watcher's test
    std::fs::metadata(path).ok()?.modified().ok()
}

fn wait_for(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

struct Isolated { home: PathBuf, config: PathBuf, state: PathBuf, socket: PathBuf }

impl Isolated {
    /// Runs the herdr CLI against the isolated session and returns its JSON output.
    fn herdr(&self, args: &[&str]) -> Value {
        let out = Command::new("herdr")
            .args(args)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.config)
            .env("XDG_STATE_HOME", &self.state)
            .env("HERDR_SOCKET_PATH", &self.socket)
            .env_remove("HERDR_ENV")
            .env_remove("HERDR_SESSION")
            .env_remove("HERDR_CLIENT_SOCKET_PATH")
            .output()
            .expect("run herdr");
        assert!(out.status.success(), "herdr {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    }

    fn viewers(&self) -> Vec<Value> {
        let list = self.herdr(&["pane", "list"]);
        list["result"]["panes"].as_array().cloned().unwrap_or_default().into_iter().filter(|p| p["label"] == "Hunks").collect()
    }
}

#[test]
#[ignore = "needs a real herdr 0.8.0 on PATH: cargo build --release && cargo test --test e2e_real_herdr -- --ignored"]
fn open_split_creates_one_viewer_and_reuses_it() {
    let real_session = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/herdr/session.json"));
    let real_mtime = real_session.as_deref().and_then(modified);

    let tmp = tempfile::Builder::new().prefix("hh-e2e-").tempdir_in("/tmp").unwrap();
    let session = format!("hh-e2e-{}", std::process::id());
    let config = tmp.path().join("xdg-config");
    let socket = config.join("herdr/sessions").join(&session).join("herdr.sock");
    let iso = Isolated { home: tmp.path().join("home"), config, state: tmp.path().join("xdg-state"), socket };
    std::fs::create_dir_all(&iso.home).unwrap();
    let _server = ProcessGuard(
        Command::new("herdr")
            .args(["--session", &session, "server"])
            .env("HOME", &iso.home)
            .env("XDG_CONFIG_HOME", &iso.config)
            .env("XDG_STATE_HOME", &iso.state)
            .env("TERM", "xterm-256color")
            .env_remove("HERDR_ENV")
            .env_remove("HERDR_SOCKET_PATH")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn isolated herdr server"),
    );
    wait_for("the session socket", || iso.socket.exists());

    // a repository with one modified file
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    for args in [&["init", "-q", "-b", "main"][..], &["config", "user.email", "t@example.com"], &["config", "user.name", "t"]] {
        assert!(Command::new("git").arg("-C").arg(&repo).args(args).status().unwrap().success());
    }
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    assert!(Command::new("git").arg("-C").arg(&repo).args(["add", "-A"]).status().unwrap().success());
    assert!(Command::new("git").arg("-C").arg(&repo).args(["commit", "-q", "-m", "init"]).status().unwrap().success());
    std::fs::write(repo.join("a.txt"), "ONE\n").unwrap();

    iso.herdr(&["plugin", "link", env!("CARGO_MANIFEST_DIR")]);
    let created = iso.herdr(&["workspace", "create", "--cwd", repo.to_str().unwrap(), "--focus"]);
    let opener = created["result"]["root_pane"]["pane_id"].as_str().expect("root pane id").to_string();

    iso.herdr(&["plugin", "action", "invoke", "open-split", "--plugin", "winoooops.hunks"]);
    wait_for("one viewer pane", || iso.viewers().len() == 1);
    let viewer = &iso.viewers()[0];
    assert_eq!(viewer["cwd"].as_str().map(PathBuf::from), Some(std::fs::canonicalize(&repo).unwrap()));

    // The viewer took focus. Refocus the opener, or the second invoke would treat the viewer as the opener.
    std::env::set_var("HERDR_SOCKET_PATH", &iso.socket);
    HerdrClient::from_env().request("pane.focus", json!({ "pane_id": opener })).expect("refocus the opener");
    iso.herdr(&["plugin", "action", "invoke", "open-split", "--plugin", "winoooops.hunks"]);
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(iso.viewers().len(), 1, "the second invoke must reuse the viewer");

    let state_file = iso.state.join("herdr/plugins/winoooops.hunks/split-panes.json");
    let records: Value = serde_json::from_str(&std::fs::read_to_string(&state_file).expect("split-panes.json")).unwrap();
    assert_eq!(records.as_object().map(|m| m.len()), Some(1));

    iso.herdr(&["server", "stop"]);
    assert_eq!(real_session.as_deref().and_then(modified), real_mtime, "the real herdr session was touched");
}
```

- [ ] **Step 3: Write `docs/acceptance-p1.md`**

A release gate, not a note. It is a table with one row per success criterion of spec 1.5 and the columns `criterion | how to check | expected | result | date | herdr version | host`. The file's first line is `Status: PENDING`. It becomes `Status: PASS` only when every row's `result` reads `pass` with its date and versions filled in, and the release workflow's guard job refuses to build a `v*` tag while that line is missing. The rows: criterion 1 (`cargo test git_diff_response`), criterion 2 (open from an agent pane inside a linked worktree; the toolbar file list shows that worktree's changes), criterion 3 (an agent edits the same file twice; the view follows; repeat with `fs.inotify.max_user_watches` exhausted and confirm the degraded notice and convergence), criterion 4 (link the same build into upstream herdr 0.8.0 and into the fork with its `vimeflow` binary; `open` works in both), criterion 5 (`cargo test --test readonly_guarantee`).

- [ ] **Step 4: Run, record the results, and commit**

Run: `cargo build --release && cargo test --test e2e_real_herdr -- --ignored` on a machine with herdr 0.8.0 — Expected: PASS.

Then carry out every row of `docs/acceptance-p1.md` by hand, in upstream herdr 0.8.0 and in the fork, and fill in its `result`, `date`, `herdr version` and `host` columns. Change the first line to `Status: PASS`. Add this step to the guard job of `.github/workflows/release.yml`: `grep -qx 'Status: PASS' docs/acceptance-p1.md || { echo 'docs/acceptance-p1.md is not marked PASS'; exit 1; }`. Phase 1 is not release-ready until all five rows read `pass`.

```bash
git add -A
git commit -m "test: add real-herdr end-to-end test and the phase 1 acceptance checklist"
```

<!-- codex-reviewed: 2026-09-20T03:47:52Z -->
