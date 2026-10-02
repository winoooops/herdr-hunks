# Working on herdr-hunks

- Phases 1-2 ship a viewer whose only mutations are the confirmed hunk actions of spec 9. Keep comments and agent dispatch out.
- `src/git/` is frozen at vimeflow `91e45b1c8f381385093813d0b4eb1d9daeb2d563` plus registered patches. Never edit it by hand: change the pin or add a numbered patch in `port/patches/`, registered in `PORT-SURFACE.md`.
- Verify the frozen tree with `scripts/port-check.sh /path/to/vimeflow` and `sh scripts/port-check-selftest.sh /path/to/vimeflow`. Treat the reference checkout as read-only. The engine reaches five additional frozen functions through the D6 visibility patch; its read allow-list has ten subcommands, and `apply` is the eleventh.
- Before committing, run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, and `cargo check --no-default-features`, plus both port checks. Test HOME must be writable and outside a git repository; preserve CARGO_HOME and RUSTUP_HOME when overriding it.
- Keep inherited clippy allowances on the `git` module declaration; do not alter frozen sources to satisfy lints.
- Reserved keys remain unbound: `i I u U x X v y Y @ c /`. `b` and `B` are the scope keys; `s`, `d` and `D` are the Phase 2 write actions, bound in `worktree` scope only and, in `branch` scope, showing `switch to worktree scope (b) to stage or discard`.
- `M` marks a commit as reviewed and writes only `marks.json`; the ten read subcommands are unchanged; `apply` is the eleventh and runs only from a confirmed `s`, `d` or `D` (`tests/hunk_actions.rs`).
- Read the plugin ID from `HERDR_PLUGIN_ID`. Reach the host through `HERDR_BIN_PATH` or `HERDR_SOCKET_PATH`; never hardcode its executable name in Rust.
- Use the shared absolute config/state directory helpers. Never write state relative to the current directory. Sanitize git text before drawing; use named ANSI colours only.
- Keep Cargo.toml, Cargo.lock, and herdr-plugin.toml versions together, and mirror README changes in English, Simplified Chinese, and Japanese.
- Use conventional commits with lowercase subjects. Inline comments should be one short line and never refer to a task or PR.

## Commands

Rust 1.88 or newer, git 2.31 or newer.

```sh
cargo build --release                           # the binary herdr-plugin.toml runs
cargo run -- [PATH]                             # the viewer without a host
cargo test --locked -- --test-threads=1         # the whole suite, as CI runs it
cargo test --locked <substring>                 # tests whose name contains it
cargo test --locked --lib engine::marks         # one module of the library
cargo test --locked --test readonly_guarantee   # one file under tests/
```

- CI runs the gates listed above, with `--locked` on clippy, the tests and the check,
  and also runs `sh scripts/fetch-or-build-selftest.sh`.
- The viewer needs a terminal on stdin and stdout and exits 2 without one: run it in
  a pane or a pty, never through a pipe.
- `herdr plugin link "$PWD"` skips `[[build]]`, so build the release binary first.
- The frozen test helpers create their fixtures under `$HOME`. When HOME is not
  writable or lies inside a git checkout, as in most agent sandboxes, give the tests
  another one:

  ```sh
  CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" \
    HOME="$(mktemp -d)" cargo test --locked -- --test-threads=1
  ```

- Tier B (`tests/e2e_real_herdr.rs`) is `#[ignore]`d: it starts an isolated real host
  and needs a fresh release build. `docs/acceptance-p1.md` has the command and its
  two variables. Never point it at a live session.
- Test builds print `failed to parse serde attribute` warnings from ts-rs and write
  `bindings/` (gitignored). Both come from the frozen tree's derives and are expected.

## Architecture

One crate, Unix only: the library `herdr_hunks` and the binary `herdr-hunks`. The
`tui` feature (default) gates `src/tui/`, ratatui and crossterm. The engine and the
actions must keep compiling without it, because the fork may embed the engine.

### Two kinds of process

`src/main.rs` dispatches on its first argument.

- **The viewer** (`tui [PATH]`, a bare path, or no argument) is one long-lived process
  per open pane. There is no daemon.
- **The actions** (`open`, `open-split`, `update`) are short-lived processes the host
  starts with `HERDR_PLUGIN_ID`, `HERDR_PLUGIN_CONTEXT_JSON`, `HERDR_SOCKET_PATH` and
  `HERDR_BIN_PATH` in the environment. `open` and `open-split` ask the host over its
  socket (`src/herdr/`, one JSON line per connection) to open the `viewer` entry of
  `herdr-plugin.toml`, and the host starts the viewer. `update` shells out to the
  host binary and to `git ls-remote`.

An action hands the viewer its context through the pane's environment
(`HERDR_HUNKS_PLACEMENT`, `HERDR_HUNKS_OPENER_PANE`). After that the two share only
the state directory.

### The viewer

Dependencies point one way: `src/tui/` → `src/engine/` → `src/git/` (frozen). The
engine is the only caller of the frozen tree and holds no terminal types.

```
 main thread · tui/shell.rs                     tokio, 2 workers · engine/session.rs
 ──────────────────────────                     ────────────────────────────────────
 keys, mouse ─► input.rs ─► Outcome::Engine ──► Command ───────────┐
                                                watcher events ────┤
                                                5 s poll (D2) ─────┼─► select! ─► State
                                                job results ───────┘               │
 view::render(Snapshot, ViewState) ◄─ try_recv ◄─ Arc<Snapshot> ◄─── publish() ◄───┘
```

The main thread owns the terminal and never waits on the engine. What the session
loop guarantees, and what a change to it must keep:

- **One refresh at a time.** A trigger that arrives during a refresh sets a dirty
  flag, and exactly one follow-up runs. Scope, base and mark requests queue as
  `Change`s; each is carried by one refresh and published together with the rows it
  produced.
- **Stale results are dropped.** A diff task carries a generation and the
  `Comparison` it ran under. A result for another generation, comparison or row is
  discarded.
- **Nothing is published twice.** `publish` compares `fingerprint`s, so a quiet
  repository causes no redraw. A new `Snapshot` field must be added to `fingerprint`,
  or a change to it alone never reaches the screen.
- **Answers ride on the snapshot.** There is no reply channel: `pick_seq`, `mark_seq`
  and `refs_seq` advance once per answered request, with the error beside them.
- **The engine reads exit codes itself.** The frozen `run_git_with_timeout` returns
  `Ok` for every exit status (K2), so callers of `engine::base::git` check
  `output.status`.
- **Git policy is process-wide (D3).** `engine::init_process_env()` is the first
  statement of `main`, before any thread exists. A test that sets environment
  variables gets a process of its own: `tests/env_policy.rs` holds one test for that
  reason.

### The TUI

- `view::render` is pure: a `Snapshot`, a `ViewState` and a size in,
  `Rendered { lines, hits }` out. Tests assert on `Rendered::plain()`. `hits` are the
  mouse regions, and a click routes through the same `KeyAction` as its key.
- `input.rs` is pure too: it changes `ViewState` and returns an `Outcome` (`Quit`,
  `Redraw`, `Inert` or `Engine(Command)`).
- `ViewState` is everything the TUI owns that is not in the snapshot. `observe` runs
  once per snapshot received; `reconcile` runs before every frame and rebuilds the
  rows only when the `Arc<LoadedDiff>` or the mode changed.
- `keys.rs` is one table for routing and for the key sheet. Tests pin its length and
  prove that every reserved key is inert, so a new binding changes those tests too.
- `shell.rs` alone touches the terminal.
- Every string from git is untrusted: `tui::sanitize` before it reaches a cell,
  `crate::text::sanitize` before an action prints it.

### State on disk

`src/paths.rs` resolves the config and state directories (`HERDR_PLUGIN_*_DIR`, then
XDG, then HOME; absolute values only). The state directory holds `bases.json`,
`marks.json`, `split-panes.json` and `config-problems.log`. The three JSON files are
rewritten under `actions::reuse::with_lock`, a `flock` on `split-panes.lock`, through
a temporary file and a rename. `tests/readonly_guarantee.rs` asserts the exact list
of files a session writes, so a new state file changes that test.

### Three invariants

Each took several review rounds to get right. Read the comments at the places named
here before changing them.

1. **Read-only except on confirmation (G7, spec 9.6).** The viewer runs only the git
   subcommands allow-listed in `tests/readonly_guarantee.rs`, plus `apply` from a confirmed
   action, which `tests/hunk_actions.rs` records form by form. A revision is always one argv element: typed text
   passes `base::check_text` and `base::verify`, and anything read back from a state
   file is checked by shape (`base::is_object_id`) before it can become an argument.
   The test records every git invocation of a scripted session, so a new git call is
   covered only once that session reaches it.
2. **A commit is markable only when everything on screen was read at it.** A diff
   task brackets its read with `read_head` before and after (`LoadedDiff.read_at`).
   `State.rows_at` takes the refresh's bracketed sample, not its opening one.
   `markable` publishes `Snapshot.head` only when the rows and the diff name the same
   commit. `ViewState.drawn_head` records it only from frames where
   `view::body_is_drawn`, and `M` sends `drawn_head`. Where these disagree, `M`
   refuses for one refresh: a mark that is too old shows dots again, one that is too
   new hides changes.
3. **No unread notice is overwritten, and no deferred one is forgotten**
   (`ViewState::observe`). The one exception is the answer to `M`, shown at once;
   what it displaces comes back by the mechanism `notice_kind` names. A base error
   is held in `pending_base_error`, because a later snapshot may carry none.
   `input::apply_action` re-runs `observe` when a key clears a notice: in a settled
   repository no snapshot arrives to do it.

### Tests

- Engine tests run a real session against fixture repositories and wait on its
  snapshots. `SessionConfig` carries the seams (an injected watcher and git check,
  `poll_interval`, `diff_delay`, `diff_gate`), and the counters on `EngineHandle` are
  test hooks.
- Wait for the state the assertion needs, in one predicate, never for a proxy of it.
- A test that changes a repository the engine is polling must tolerate `index.lock`.
  The `git` helpers in the tests retry on exactly that message.
- A fix counts once its test has been seen to fail against the old code.

### Where decisions are recorded

- `PORT-SURFACE.md`: the frozen tree's pin and patches, the divergences D1-D6 and the
  known defects K1-K7.
- `docs/superpowers/specs/`: section numbers run on across the three files (1-6 the
  roadmap and Phase 1, 7 branch scope, 8 review marks), and code comments cite them
  by number.
- `docs/superpowers/plans/`: each plan ends with the notes of its review rounds.
- `docs/acceptance-p1.md`: manual acceptance, one row per criterion.

### Release

A `v*` tag starts `.github/workflows/release.yml`. It requires the first line of
`docs/acceptance-p1.md` to read `Status: PASS` and the tag to equal the version in
`Cargo.toml`, builds four targets, and publishes them under the names
`scripts/fetch-or-build.sh` downloads. The release notes are the tag's annotation
(`git tag -a -F <file>`), with the commit body as fallback.
