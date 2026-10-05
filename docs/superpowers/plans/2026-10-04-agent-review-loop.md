# herdr-hunks agent review loop, part 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The reviewer picks the agent pane a review goes to (`A`), writes comments as cards under diff lines (`i I v u U x X`), and sends them (`Y`) or a review request (`@`) to that pane as Vimeflow's prompt through the host's `agent.prompt`, or copies them (`c`), with every comment shared through `comments.json` and every send claimed before the host is called; as release 0.0.5, on top of 0.0.4's hunk actions.

**Architecture:** Three new engine modules own the state: `engine::target` (the chosen pane, `targets.json`, the per-refresh `pane.get` check), `engine::comments` (the records, `comments.json` and `requests.json` transactions under the state lock, the journal, the 60 s expiry) and `engine::dispatch` (the send: gates, `send.lock`, claim, host call, settlement by nonce; the copy). `engine::prompt` builds Vimeflow's two texts. The host is reached through one trait, `engine::host::HostClient`, injected via `SessionConfig` and wrapped in `spawn_blocking` in production, with one five-second deadline per request. The TUI adds a pane picker (`tui::panes`), cards and an orphan section as body rows (`tui::rows`), an inline editor and a visual selection (`tui::review`), and the Finish and Request boxes as `dialog::Panel`s following `tui::confirm`'s y/n convention; answers ride on the snapshot as `target_seq`, `panes_seq`, `comment_seq`, `send_seq` and `copy_seq`.

**Tech Stack:** Rust 1.88, edition 2021; the 0.0.4 dependencies unchanged (`tokio` 1, `serde`/`serde_json` 1, `sha2` 0.10 already present, `libc` 0.2, `thiserror` 2, `ratatui` 0.30 behind the `tui` feature, `crossterm` 0.29; dev `tempfile` 3). Runtime: `git` 2.31 or newer; a herdr 0.8.0 or vimeflow-terminal host for pane targets, none for the clipboard target.

**Spec:** `docs/superpowers/specs/2026-09-30-agent-review-loop-design.md` (section 10), on top of sections 1-6 (`2026-09-18-hunks-roadmap-p1-viewer-design.md`), 7 (`2026-09-22-branch-scope-design.md`), 8 (`2026-09-23-review-marks-design.md`) and 9 (`2026-09-30-hunk-actions-design.md`, shipped as 0.0.4). Each task names the subsections it implements; read them before starting it. Where the spec uses section 9's vocabulary, this plan names the code 0.0.4 merged: "the Y/N box" is `tui::confirm::Confirm` and its `dialog::Panel`; "the dialog layer of 4.3" is `tui::dialog`; "`Command::Act`" and "the carrying refresh" are `session.rs`'s `Change::Act` path.

## Global Constraints

- The vimeflow pin stays `91e45b1c` and `src/git/` is frozen; this section adds no patch. `scripts/port-check.sh "$VIMEFLOW"` and `sh scripts/port-check-selftest.sh "$VIMEFLOW"` must pass after every task; Task 4 teaches `port-check.sh` one more byte comparison, `src/engine/prompts/delegated-review.md` against `$PIN:src/features/diff/prompts/delegated-review.prompt.md`. `$VIMEFLOW` is a read-only checkout of vimeflow at that pin (on the author's machine `~/projects/vimeflow`).
- The guarantee of spec 10.6: the engine spawns only the eleven git subcommands of 9.6 (`--version rev-parse status diff ls-files show cat-file symbolic-ref merge-base for-each-ref apply`); this section adds none. The only git this section runs is the diff loads of an all-changes review request, through `worktree::diff`, `worktree::untracked_diff` and `branch::diff`. The host is reached by exactly three methods, `pane.get`, `pane.list` and `agent.prompt`; the engine never sends keys, reads a pane or focuses anything.
- Every string from the host (pane ids, agent kinds, titles, directories, error messages) and every string from git is untrusted: `tui::sanitize` before it reaches a cell, `engine::prompt::strip_controls` before it enters a prompt. A pane id reaches the host only as a JSON request parameter, never as an argv element; a socket path read from a state file is compared, never opened.
- Keys bound by this section: `A i I v u U x X y Y @ c`, plus the editor's `ctrl+h ctrl+l ctrl+j Enter Esc`, the boxes' `y n Y A c f a Esc`. `RESERVED` becomes `["/"]`. `n` keeps its binding (next file) outside a box; `y` is bound only inside a box and on a visual selection; `c` only inside the two boxes.
- Comments, the target, the boxes and the picker work in both scopes. Branch-scope anchors carry the merge-base they were made against and never move.
- State files: `targets.json`, `comments.json`, `requests.json`, `clipboard.md` and `send.lock` join `bases.json`, `marks.json` and `split-panes.lock` in the state directory, written only under an absolute state directory through a temporary file and a rename, under `reuse::with_lock` (the state lock) or `send.lock` (the send lock), never relative to the repository.
- Colours are the terminal's named ANSI colours only (`Role`/`Semantic` of `tui::style`); the "dim" of spec 10.2 and 10.3 is `Role::Label`.
- Behaviour without a target or a comment is 0.0.4's: every existing test keeps passing without being weakened, except where this plan names the test and the reason (the key count, the reserved set, the state-directory file list of the read-only test, and the toolbar chip count and style in `toolbar_chips_are_padded_reversed_accent_bold_and_plain_text_is_not`, Task 6).
- Commits are conventional with a lowercase subject; inline comments are one short line and never reference a task or PR. The orchestrator makes every commit; the implementer leaves the tree uncommitted.
- `cargo test` needs a writable `HOME` outside any git repository: `CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" HOME="$(mktemp -d)" cargo test --locked -- --test-threads=1`.
- The version becomes `0.0.5` in Task 9 only; no other task touches `Cargo.toml`, `Cargo.lock` or `herdr-plugin.toml`.
- Every fix is falsified: the task's test is seen to fail against the old code before the code changes (each task's steps say which test and what failure to expect).
- Run before every commit: `cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked -- --test-threads=1 && cargo check --locked --no-default-features && scripts/port-check.sh "$VIMEFLOW" && sh scripts/port-check-selftest.sh "$VIMEFLOW"` (with the `HOME` of the line above on the test). `cargo check --no-default-features` matters more than before: `engine::host`, `engine::target`, `engine::comments`, `engine::dispatch` and `engine::prompt` must compile without the `tui` feature.

## Review Focus

Inputs the spec implies but no task's tests would otherwise exercise; each has its test added to the owning task.

1. A pane whose `cwd` is a *prefix* of the toplevel but not a parent (`/home/u/repo` vs `/home/u/repo2`): the picker's "this worktree" group must compare path components, not string prefixes (Task 2 test `a_sibling_directory_with_a_shared_prefix_is_another_pane`).
2. A comment text that is 4,000 characters of multi-byte and combining characters: the cap counts `chars`, the card wraps by terminal cells, and the prompt carries the text unchanged (Task 3 test `the_caps_count_characters_not_bytes`, Task 7 test `a_card_wraps_wide_characters_by_cells`).
3. A `comments.json` written by a newer version with an unknown `state` variant or an extra field: the unknown record is dropped with a problem line and every other record survives, so two plugin versions on one state directory degrade one record at a time (Task 3 test `an_unknown_state_drops_that_record_alone`).
4. `Y` pressed while the pane picker opened by `Y` is still loading its rows, then `Enter`: the box must open once, with the new target, and send nothing by itself (Task 8 test `a_pick_made_for_the_finish_box_returns_to_it_without_sending`).
5. The host answering `agent.prompt` with success for a pane whose agent is `blocked` at that very moment because the check and the prompt straddled a dialog: the engine cannot see it, and the settlement must still be `Sent` with `to` the verified session, not `Unconfirmed` (Task 5 test `a_success_reply_is_sent_whatever_happened_after_the_check`, which pins that the gates run once, before the call, and that nothing is re-read after).

## File Structure

```
src/herdr/client.rs          HerdrClientError gains Write/Read/Deadline phases; request() gets one deadline:
                             a non-blocking connect polled against it, per-call timeouts set to the time
                             left, a bounded read loop (Task 1)
src/engine/host.rs           NEW: PaneRecord, SessionRef, HostFailure, trait HostClient, HerdrHost, from_env,
                             DEADLINE, ENGINE_WAIT (Task 1)
src/engine/target.rs         NEW: Target, TargetState, Destination, targets.json load/save with shape checks,
                             compare(), the picker's grouping rule (Task 2)
src/engine/comments.rs       NEW: Comment, Anchor, Category, CommentState, Stamp, Store (comments.json
                             transactions, journal, expiry, mtime), RequestRecord (requests.json) (Task 3)
src/engine/prompt.rs         NEW: review(), request(), strip_controls(), quote_toplevel(), nonce() (Task 4)
src/engine/prompts/delegated-review.md  NEW: byte-identical to the pin's prompt file (Task 4)
scripts/port-check.sh        the prompt-file comparison (Task 4)
src/engine/dispatch.rs       NEW: SendRequest, Accepted, SendKind, ReviewScope, CopyRequest, CopyOut, the
                             send task (gates, send.lock, claim, call, settle), the copy (Task 5)
src/engine/types.rs          Snapshot.{target, target_state, target_seq, target_error, panes, panes_seq,
                             panes_error, comments, comment_seq, comment_error, send_seq, send_error,
                             copy_seq, copy}; Command::{LoadPanes, SetTarget, AddComment, EditComment,
                             DeleteComment, Send, Copy} (Tasks 2, 3, 5)
src/engine/session.rs        SessionConfig.{host, opener_pane, socket_path, nonce}; the target check in
                             run_job; Done::{Panes, Target, Comment, Sent, Copied}; the new commands;
                             fingerprint (Tasks 2, 3, 5)
src/engine/mod.rs            module list (Tasks 1-5)
src/actions/reuse.rs         with_lock_at(path, wait, f); with_lock delegates to it (Task 5)
src/tui/panes.rs             NEW: PanePicker (rows from Snapshot.panes, three groups, filter, ReturnTo) (Task 6)
src/tui/keys.rs              A i I v u U x X Y @ bindings; RESERVED = ["/"]; help rows (Tasks 6, 7, 8)
src/tui/rows.rs              Row::Card, Row::Orphans; build() takes the comments and the body width;
                             orphan cards are targets (Task 7)
src/tui/cards.rs             NEW: card titles, tones, wrapping; editor rows (Task 7)
src/tui/review.rs            NEW: Editor, Visual, FinishBox/RequestBox panels and their keys (Tasks 7, 8)
src/tui/state.rs             target chip inputs, panes picker, editor, visual, box, orphan cursor,
                             observe for the five new answers, reconcile on comments/width (Tasks 6, 7, 8)
src/tui/input.rs             the new keys and the modal routing (Tasks 6, 7, 8)
src/tui/view.rs              the chip, cards, the editor splice, ✎ marks and counts, the footer, the boxes,
                             body_is_drawn (Tasks 6, 7, 8)
src/tui/shell.rs             SessionConfig wiring (host, opener, socket, nonce); the OSC 52 write per
                             copy_seq (Tasks 2, 8)
tests/support/mod.rs         the fake host: pane.list, agent.prompt, agent_status/agent_session on panes,
                             scripted failures and delays, recorded requests (Task 1)
tests/host_env.rs            NEW: from_env against the environment, alone in its process (Task 1)
tests/review_loop_guarantee.rs  NEW: the review-loop recording test: fake host, the three methods, the file list (Task 9)
tests/e2e_real_herdr.rs      A, the no-agent picker, a clipboard send (Task 9)
README.md, .zh-CN, .ja       "Review loop" section (Task 9)
AGENTS.md                    the first rule, the reserved-key line, the state-file list (Task 9)
docs/superpowers/specs/2026-09-18-hunks-roadmap-p1-viewer-design.md  the 1.2 roadmap order note (Task 9)
docs/acceptance-p1.md        row 10 (Task 9)
Cargo.toml, Cargo.lock, herdr-plugin.toml  0.0.5 (Task 9)
```

---

### Task 1: One host surface with one deadline

Implements spec 10.6 "The host client" and 10.7's `NoHost` rows; prepares 10.9 tests 7 and 10. Read 10.6 and the `HERDR_SOCKET_PATH` sentences of 10.2 before starting.

**Files:**
- Create: `src/engine/host.rs`, `tests/host_env.rs` (one test: it edits the environment)
- Modify: `src/herdr/client.rs` (`HerdrClientError`, `request`, a non-blocking connect, a bounded read), `src/herdr/api.rs` (no change in behaviour; `pane_get` keeps compiling), `src/engine/mod.rs`, `src/engine/session.rs` (`SessionConfig.host`, `SessionConfig.socket_path`), `src/tui/shell.rs` (fills both from the environment), `tests/support/mod.rs` (the fake host's three methods and its failure script)

**Interfaces:**
- Consumes: 0.0.4's `HerdrClient::request`, `HerdrClient::from_env`, `tests/support::FakeHerdr`.
- Produces:

```rust
// src/herdr/client.rs
/// One request may take this long, connect to complete reply.
pub const DEADLINE: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum HerdrClientError {
    /// Before anything was written: no socket file, a refused connection, a connect that did
    /// not complete by the deadline.
    #[error("herdr socket unavailable at {path}: {source}")]
    Connect { path: PathBuf, #[source] source: std::io::Error },
    /// The request line was not written in full.
    #[error("could not write to herdr: {0}")]
    Write(std::io::Error),
    /// The line was written in full and no complete reply arrived by the deadline.
    #[error("no complete reply from herdr: {0}")]
    Read(std::io::Error),
    #[error("herdr returned malformed json: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("herdr error response: {0}")]
    Api(String),
}

impl HerdrClient {
    pub fn new(socket_path: PathBuf) -> Self;
    pub fn socket_path(&self) -> &Path;
    /// Unchanged signature; one deadline from the first byte of the connect.
    pub fn request(&self, method: &str, params: Value) -> Result<Value, HerdrClientError>;
}

// src/engine/host.rs
/// The agent's session as the host reports it: `kind` is `id` or `path`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionRef { pub kind: String, pub value: String }

/// What `pane.get` and a `pane.list` row carry that this section reads.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize)]
pub struct PaneRecord {
    pub pane_id: String,
    #[serde(default)] pub agent: Option<String>,
    #[serde(default)] pub agent_status: Option<String>,
    #[serde(default)] pub agent_session: Option<SessionRef>,
    #[serde(default)] pub cwd: Option<String>,
    #[serde(default)] pub foreground_cwd: Option<String>,
    #[serde(default)] pub label: Option<String>,
    #[serde(default, rename = "terminal_title_stripped")] pub title: Option<String>,
}

/// Why a host call failed, in the terms 10.4's settlement and 10.2's check need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostFailure {
    /// The socket path is unset, the file is missing, or the connection was refused.
    NoHost(String),
    /// Failed before the request line was written in full; the host never saw it.
    Before(String),
    /// Written in full; no complete, well-formed reply by the deadline. The host may have acted.
    After(String),
    /// The host's own error reply.
    Api { code: String, message: String },
}

pub trait HostClient: Send + Sync + 'static {
    fn pane_get(&self, pane: &str) -> Result<PaneRecord, HostFailure>;
    fn pane_list(&self) -> Result<Vec<PaneRecord>, HostFailure>;
    /// `agent.prompt` without `wait`.
    fn agent_prompt(&self, pane: &str, text: &str) -> Result<(), HostFailure>;
}

/// The production client over `HERDR_SOCKET_PATH`.
pub struct HerdrHost { client: HerdrClient }
impl HerdrHost { pub fn new(socket_path: PathBuf) -> Self; }
impl HostClient for HerdrHost { /* maps HerdrClientError to HostFailure, see Step 3 */ }

/// `None` when `HERDR_SOCKET_PATH` is unset: every pane target is then `NoHost` (10.6).
pub fn from_env() -> Option<Arc<dyn HostClient>>;
/// How long the engine waits on a host call before the outcome is uncertain (10.6).
pub const ENGINE_WAIT: Duration = Duration::from_secs(10);

/// A scripted host for tests: a pane table the test edits mid-run, and a result per method.
#[doc(hidden)]
pub struct Scripted { /* see Step 4 */ }

// src/engine/session.rs: SessionConfig gains
pub host: Option<Arc<dyn HostClient>>,   // None in production() and in every existing test
pub socket_path: Option<String>,         // what HERDR_SOCKET_PATH said; compared with a target record's socket
```

- [ ] **Step 1: The error phases and the deadline, test first**

Append to `src/herdr/client.rs`'s test module. Each test starts a real listener on a temp path and a thread that plays one server:

```rust
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixListener;
    use std::time::Instant;

    /// A server playing one script, on a fresh socket.
    fn server(script: impl FnOnce(std::os::unix::net::UnixStream) + Send + 'static) -> (tempfile::TempDir, HerdrClient) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("h.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                script(stream);
            }
        });
        (dir, HerdrClient::new(path))
    }

    #[test]
    fn a_missing_socket_and_a_refused_connection_fail_before_writing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = HerdrClient::new(dir.path().join("none.sock"));
        assert!(matches!(missing.request("ping", json!({})), Err(HerdrClientError::Connect { .. })));
        // A path that exists but nobody listens on.
        let stale = dir.path().join("stale.sock");
        drop(UnixListener::bind(&stale).unwrap());
        let refused = HerdrClient::new(stale);
        let started = Instant::now();
        assert!(matches!(refused.request("ping", json!({})), Err(HerdrClientError::Connect { .. })));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn a_server_that_reads_and_never_answers_fails_after_writing_at_the_deadline() {
        let (_dir, client) = server(|stream| {
            let mut line = String::new();
            let _ = BufReader::new(stream.try_clone().unwrap()).read_line(&mut line);
            std::thread::sleep(Duration::from_secs(8));
        });
        let started = Instant::now();
        let error = client.request("ping", json!({})).unwrap_err();
        assert!(matches!(error, HerdrClientError::Read(_)), "{error:?}");
        let took = started.elapsed();
        assert!(took >= DEADLINE && took < DEADLINE + Duration::from_secs(1), "{took:?}");
    }

    #[test]
    fn a_byte_every_two_seconds_still_fails_at_the_deadline() {
        let (_dir, client) = server(|mut stream| {
            let mut line = String::new();
            let _ = BufReader::new(stream.try_clone().unwrap()).read_line(&mut line);
            // Never a newline: the reply is never complete.
            for b in br#"{"id":"1","result":{}}"# {
                if stream.write_all(&[*b]).is_err() { return; }
                std::thread::sleep(Duration::from_secs(2));
            }
        });
        let started = Instant::now();
        let error = client.request("ping", json!({})).unwrap_err();
        assert!(matches!(error, HerdrClientError::Read(_)), "{error:?}");
        let took = started.elapsed();
        // A per-call socket timeout alone would run on for 2 s × 22 bytes.
        assert!(took < DEADLINE + Duration::from_secs(1), "{took:?}");
    }

    #[test]
    fn a_server_that_never_reads_fails_the_write_at_the_deadline() {
        // The kernel's socket buffer absorbs a small request; a four-megabyte one blocks the writer
        // until the peer reads, and the peer never does.
        let (_dir, client) = server(|_stream| std::thread::sleep(Duration::from_secs(8)));
        let big = "x".repeat(4 << 20);
        let started = Instant::now();
        let error = client.request("agent.prompt", json!({ "target": "w1:p2", "text": big })).unwrap_err();
        assert!(matches!(error, HerdrClientError::Write(_)), "{error:?}");
        let took = started.elapsed();
        assert!(took >= DEADLINE && took < DEADLINE + Duration::from_secs(1), "{took:?}");
    }

    #[test]
    fn a_complete_reply_within_the_deadline_decodes() {
        let (_dir, client) = server(|mut stream| {
            let mut line = String::new();
            let _ = BufReader::new(stream.try_clone().unwrap()).read_line(&mut line);
            let request: Value = serde_json::from_str(&line).unwrap();
            let reply = json!({ "id": request["id"], "result": { "type": "pong" } });
            std::thread::sleep(Duration::from_millis(300));
            let _ = stream.write_all(format!("{reply}\n").as_bytes());
        });
        let value = client.request("ping", json!({})).unwrap();
        assert_eq!(value["result"]["type"], "pong");
    }
```

- [ ] **Step 2: Run them to watch them fail**

Run: `cargo test --locked --lib herdr::client`
Expected: compile errors (`HerdrClient::new`, `DEADLINE`, `HerdrClientError::Read`/`Write` do not exist); after stubbing the names, `a_byte_every_two_seconds_still_fails_at_the_deadline` must fail on the elapsed assertion with the old per-read timeout (44 s), and `a_server_that_reads_and_never_answers...` must report `Io`, not `Read`.

- [ ] **Step 3: One deadline per request**

Replace the body of `src/herdr/client.rs` above its tests with:

```rust
use std::io::{Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// One request may take this long, connect to complete reply.
pub const DEADLINE: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum HerdrClientError {
    /// Before anything was written: no socket file, a refused connection, a connect that did
    /// not complete by the deadline.
    #[error("herdr socket unavailable at {path}: {source}")]
    Connect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The request line was not written in full.
    #[error("could not write to herdr: {0}")]
    Write(std::io::Error),
    /// The line was written in full and no complete reply arrived by the deadline.
    #[error("no complete reply from herdr: {0}")]
    Read(std::io::Error),
    #[error("herdr returned malformed json: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("herdr error response: {0}")]
    Api(String),
}

pub fn decode_response(line: &str) -> Result<Value, HerdrClientError> {
    let value: Value = serde_json::from_str(line)?;
    if let Some(error) = value.get("error") {
        return Err(HerdrClientError::Api(error.to_string()));
    }
    Ok(value)
}

pub struct HerdrClient {
    socket_path: PathBuf,
    next_id: AtomicU64,
}

fn timed_out() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::TimedOut, "the deadline passed")
}

/// Time left before `deadline`, or an error once it has passed.
fn left(deadline: Instant) -> std::io::Result<Duration> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err(timed_out())
    } else {
        Ok(left)
    }
}

/// A connect that cannot outlive the deadline: `UnixStream::connect` blocks while the host's
/// listen backlog is full, so the socket is made non-blocking and polled instead.
fn connect_within(path: &Path, deadline: Instant) -> std::io::Result<UnixStream> {
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.len() >= address.sun_path.len() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "socket path too long"));
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (slot, byte) in address.sun_path.iter_mut().zip(bytes) {
        *slot = *byte as libc::c_char;
    }
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // Owned from here: dropped on every early return below.
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    stream.set_nonblocking(true)?;
    let length = std::mem::size_of::<libc::sa_family_t>() + bytes.len() + 1;
    let connected = unsafe {
        libc::connect(
            stream.as_raw_fd(),
            &address as *const libc::sockaddr_un as *const libc::sockaddr,
            length as libc::socklen_t,
        )
    };
    if connected != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EINPROGRESS) && error.raw_os_error() != Some(libc::EAGAIN) {
            return Err(error);
        }
        let mut poll = libc::pollfd { fd: stream.as_raw_fd(), events: libc::POLLOUT, revents: 0 };
        let wait = left(deadline)?.as_millis().min(i32::MAX as u128) as libc::c_int;
        let ready = unsafe { libc::poll(&mut poll, 1, wait) };
        if ready < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if ready == 0 {
            return Err(timed_out());
        }
        // The pending connect has an outcome now; read it the way std does.
        if let Some(error) = stream.take_error()? {
            return Err(error);
        }
    }
    stream.set_nonblocking(false)?;
    Ok(stream)
}

impl HerdrClient {
    pub fn from_env() -> Self {
        let socket_path = std::env::var_os("HERDR_SOCKET_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("XDG_CONFIG_HOME")
                    .map(PathBuf::from)
                    .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
                    .unwrap_or_else(|| PathBuf::from(".config"))
                    .join("herdr/herdr.sock")
            });
        Self::new(socket_path)
    }

    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            next_id: AtomicU64::new(1),
        }
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// One request per connection, one deadline for the whole exchange: every timeout below is
    /// the time the deadline has left, and the read loop stops at it however many bytes have
    /// trickled in. The deadline is one part of the incumbent daemon's worst-case shutdown,
    /// which the eight-second singleton takeover deadline must clear.
    pub fn request(&self, method: &str, params: Value) -> Result<Value, HerdrClientError> {
        let deadline = Instant::now() + DEADLINE;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed).to_string();
        let request = json!({ "id": id, "method": method, "params": params });
        let mut line = serde_json::to_string(&request)?;
        line.push('\n');
        let mut stream = connect_within(&self.socket_path, deadline).map_err(|source| {
            HerdrClientError::Connect {
                path: self.socket_path.clone(),
                source,
            }
        })?;
        let mut written = 0;
        while written < line.len() {
            stream
                .set_write_timeout(Some(left(deadline).map_err(HerdrClientError::Write)?))
                .map_err(HerdrClientError::Write)?;
            match stream.write(&line.as_bytes()[written..]) {
                Ok(0) => return Err(HerdrClientError::Write(std::io::ErrorKind::WriteZero.into())),
                Ok(n) => written += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(HerdrClientError::Write(e)),
            }
        }
        let mut reply = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            stream
                .set_read_timeout(Some(left(deadline).map_err(HerdrClientError::Read)?))
                .map_err(HerdrClientError::Read)?;
            match stream.read(&mut byte) {
                Ok(0) => return Err(HerdrClientError::Read(std::io::ErrorKind::UnexpectedEof.into())),
                Ok(_) if byte[0] == b'\n' => break,
                Ok(_) => reply.push(byte[0]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(HerdrClientError::Read(e)),
            }
            if reply.len() > 1 << 20 {
                return Err(HerdrClientError::Read(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "reply longer than a megabyte",
                )));
            }
        }
        decode_response(&String::from_utf8_lossy(&reply))
    }
}
```

The byte-at-a-time read is deliberate: a `BufReader` would hold its own timeout per `read` syscall and a trickling host would stretch the exchange past the deadline, which `a_byte_every_two_seconds_still_fails_at_the_deadline` pins; a reply is one line of at most a megabyte (the host's line bound), so the syscall count is bounded too. `src/herdr/api.rs` compiles unchanged: its `?` on `request` keeps working because the error type is the same enum. In `src/actions/open.rs:14-16`, the downcast to `HerdrClientError::Api` is untouched.

- [ ] **Step 4: Run the client tests**

Run: `cargo test --locked --lib herdr::client`
Expected: all seven pass (the two captured-fixture tests, the five new ones); the three deadline tests take about five seconds each.

- [ ] **Step 5: The trait, the production wrapper and the scripted host**

Create `src/engine/host.rs`:

```rust
//! The host surface of spec 10.6: three methods behind one trait, injected like the watcher.
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use crate::herdr::client::{HerdrClient, HerdrClientError};

/// The agent's session as the host reports it: `kind` is `id` or `path`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionRef {
    pub kind: String,
    pub value: String,
}

/// What `pane.get` and a `pane.list` row carry that this section reads.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize)]
pub struct PaneRecord {
    pub pane_id: String,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub agent_status: Option<String>,
    #[serde(default)]
    pub agent_session: Option<SessionRef>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub foreground_cwd: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default, rename = "terminal_title_stripped")]
    pub title: Option<String>,
}

/// Why a host call failed, in the terms 10.4's settlement and 10.2's check need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostFailure {
    /// The socket path is unset, the file is missing, or the connection was refused.
    NoHost(String),
    /// Failed before the request line was written in full; the host never saw it.
    Before(String),
    /// Written in full; no complete, well-formed reply by the deadline. The host may have acted.
    After(String),
    /// The host's own error reply.
    Api { code: String, message: String },
}

impl HostFailure {
    pub fn message(&self) -> String {
        match self {
            Self::NoHost(m) | Self::Before(m) | Self::After(m) => m.clone(),
            Self::Api { message, .. } => message.clone(),
        }
    }
}

pub trait HostClient: Send + Sync + 'static {
    fn pane_get(&self, pane: &str) -> Result<PaneRecord, HostFailure>;
    fn pane_list(&self) -> Result<Vec<PaneRecord>, HostFailure>;
    /// `agent.prompt` without `wait`: the host pastes and presses Enter 300 ms later.
    fn agent_prompt(&self, pane: &str, text: &str) -> Result<(), HostFailure>;
}

/// How long the engine waits on a host call before the outcome is uncertain (10.6).
pub const ENGINE_WAIT: Duration = Duration::from_secs(10);

pub struct HerdrHost {
    client: HerdrClient,
}

impl HerdrHost {
    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            client: HerdrClient::new(socket_path),
        }
    }
}

fn classify(error: HerdrClientError) -> HostFailure {
    use std::io::ErrorKind;
    match error {
        HerdrClientError::Connect { source, path }
            if matches!(source.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) =>
        {
            HostFailure::NoHost(format!("no host at {}: {source}", path.display()))
        }
        HerdrClientError::Connect { source, .. } => HostFailure::Before(source.to_string()),
        HerdrClientError::Write(e) => HostFailure::Before(e.to_string()),
        HerdrClientError::Read(e) => HostFailure::After(e.to_string()),
        HerdrClientError::Decode(e) => HostFailure::After(e.to_string()),
        HerdrClientError::Api(body) => {
            let value: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            HostFailure::Api {
                code: value["code"].as_str().unwrap_or("error").to_string(),
                message: value["message"].as_str().unwrap_or(&body).to_string(),
            }
        }
    }
}

impl HostClient for HerdrHost {
    fn pane_get(&self, pane: &str) -> Result<PaneRecord, HostFailure> {
        let response = self
            .client
            .request("pane.get", json!({ "pane_id": pane }))
            .map_err(classify)?;
        serde_json::from_value(response["result"]["pane"].clone())
            .map_err(|e| HostFailure::After(format!("pane.get reply: {e}")))
    }

    fn pane_list(&self) -> Result<Vec<PaneRecord>, HostFailure> {
        let response = self.client.request("pane.list", json!({})).map_err(classify)?;
        serde_json::from_value(response["result"]["panes"].clone())
            .map_err(|e| HostFailure::After(format!("pane.list reply: {e}")))
    }

    fn agent_prompt(&self, pane: &str, text: &str) -> Result<(), HostFailure> {
        let response = self
            .client
            .request("agent.prompt", json!({ "target": pane, "text": text }))
            .map_err(classify)?;
        // The envelope the host documents: `{"id", "result": {"type": "agent_prompted", ...}}`. Anything
        // else arrived after the line was written and proves nothing: uncertain, never success.
        if response["result"]["type"].as_str() == Some("agent_prompted") {
            Ok(())
        } else {
            Err(HostFailure::After(format!("agent.prompt reply not understood: {response}")))
        }
    }
}

/// `None` when `HERDR_SOCKET_PATH` is unset: every pane target is then `NoHost` (10.6).
pub fn from_env() -> Option<Arc<dyn HostClient>> {
    std::env::var_os("HERDR_SOCKET_PATH")
        .map(PathBuf::from)
        .map(|path| Arc::new(HerdrHost::new(path)) as Arc<dyn HostClient>)
}

/// A scripted host for tests: panes answered from a table the test edits mid-run, a result
/// for each `agent.prompt`, every call recorded. Compiled always so integration tests can use it.
#[doc(hidden)]
#[derive(Default)]
pub struct Scripted {
    pub panes: Mutex<BTreeMap<String, PaneRecord>>,
    /// `Err` fails every `pane.get` and `pane.list` with it until cleared.
    pub list_failure: Mutex<Option<HostFailure>>,
    /// `Some(record)` makes every `pane.get` answer that record whatever pane was asked for.
    pub answer_pane_get_with: Mutex<Option<PaneRecord>>,
    /// Pushed results for `agent_prompt`, consumed first to last; empty means `Ok(())`.
    pub prompt_results: Mutex<Vec<Result<(), HostFailure>>>,
    /// Every `agent_prompt` call as `(pane, text)`, and when each began (before its delay).
    pub prompts: Mutex<Vec<(String, String)>>,
    pub prompt_started: Mutex<Vec<std::time::Instant>>,
    /// Every method called, in order.
    pub calls: Mutex<Vec<String>>,
    /// A delay before each `agent_prompt` answers, for tests of the engine's wait.
    pub prompt_delay: Mutex<Option<Duration>>,
    /// Observes the comment store at the moment of the call (Task 5's claim-before-call test).
    pub on_prompt: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl Scripted {
    pub fn with_panes(panes: Vec<PaneRecord>) -> Arc<Self> {
        let scripted = Self::default();
        *scripted.panes.lock().unwrap() = panes.into_iter().map(|p| (p.pane_id.clone(), p)).collect();
        Arc::new(scripted)
    }

    pub fn set_pane(&self, pane: PaneRecord) {
        self.panes.lock().unwrap().insert(pane.pane_id.clone(), pane);
    }

    pub fn remove_pane(&self, pane: &str) {
        self.panes.lock().unwrap().remove(pane);
    }
}

impl HostClient for Scripted {
    fn pane_get(&self, pane: &str) -> Result<PaneRecord, HostFailure> {
        self.calls.lock().unwrap().push("pane.get".into());
        if let Some(failure) = self.list_failure.lock().unwrap().clone() {
            return Err(failure);
        }
        if let Some(record) = self.answer_pane_get_with.lock().unwrap().clone() {
            return Ok(record);
        }
        self.panes.lock().unwrap().get(pane).cloned().ok_or(HostFailure::Api {
            code: "pane_not_found".into(),
            message: "no such pane".into(),
        })
    }

    fn pane_list(&self) -> Result<Vec<PaneRecord>, HostFailure> {
        self.calls.lock().unwrap().push("pane.list".into());
        if let Some(failure) = self.list_failure.lock().unwrap().clone() {
            return Err(failure);
        }
        Ok(self.panes.lock().unwrap().values().cloned().collect())
    }

    fn agent_prompt(&self, pane: &str, text: &str) -> Result<(), HostFailure> {
        self.calls.lock().unwrap().push("agent.prompt".into());
        self.prompt_started.lock().unwrap().push(std::time::Instant::now());
        if let Some(observe) = self.on_prompt.lock().unwrap().as_ref() {
            observe();
        }
        if let Some(delay) = *self.prompt_delay.lock().unwrap() {
            std::thread::sleep(delay);
        }
        self.prompts.lock().unwrap().push((pane.to_string(), text.to_string()));
        let mut results = self.prompt_results.lock().unwrap();
        if results.is_empty() {
            Ok(())
        } else {
            results.remove(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_failures_are_classified_by_phase() {
        let missing = HerdrClientError::Connect {
            path: "/nowhere/h.sock".into(),
            source: std::io::ErrorKind::NotFound.into(),
        };
        assert!(matches!(classify(missing), HostFailure::NoHost(_)));
        let refused = HerdrClientError::Connect {
            path: "/x".into(),
            source: std::io::ErrorKind::ConnectionRefused.into(),
        };
        assert!(matches!(classify(refused), HostFailure::NoHost(_)));
        let slow = HerdrClientError::Connect {
            path: "/x".into(),
            source: std::io::ErrorKind::TimedOut.into(),
        };
        assert!(matches!(classify(slow), HostFailure::Before(_)));
        assert!(matches!(classify(HerdrClientError::Write(std::io::ErrorKind::BrokenPipe.into())), HostFailure::Before(_)));
        assert!(matches!(classify(HerdrClientError::Read(std::io::ErrorKind::TimedOut.into())), HostFailure::After(_)));
        let api = classify(HerdrClientError::Api(r#"{"code":"agent_not_ready","message":"blocked"}"#.into()));
        assert_eq!(api, HostFailure::Api { code: "agent_not_ready".into(), message: "blocked".into() });
    }

    #[test]
    fn a_pane_record_reads_the_hosts_shape() {
        let pane: PaneRecord = serde_json::from_value(serde_json::json!({
            "pane_id": "w4:p2", "agent": "codex", "agent_status": "idle",
            "agent_session": { "agent": "codex", "kind": "id", "source": "herdr:codex", "value": "01a0" },
            "cwd": "/home/u/repo", "terminal_title_stripped": "fix the cart", "focused": false, "revision": 3
        }))
        .unwrap();
        assert_eq!(pane.agent.as_deref(), Some("codex"));
        assert_eq!(pane.agent_session.as_ref().map(|s| s.value.as_str()), Some("01a0"));
        assert_eq!(pane.title.as_deref(), Some("fix the cart"));
        // A shell pane: no agent, status unknown.
        let shell: PaneRecord = serde_json::from_value(serde_json::json!({ "pane_id": "w4:p1", "agent_status": "unknown" })).unwrap();
        assert!(shell.agent.is_none() && shell.agent_session.is_none());
    }
}
```

`from_env` reads the environment, and a test of it edits the environment, so it follows `tests/env_policy.rs`'s rule and owns a process: `tests/host_env.rs`, one test, with `cargo test` free to run its threads as it likes:

```rust
use herdr_hunks::engine::host::from_env;

// The only test in this binary: it edits the environment, which no other test may read meanwhile.
#[test]
fn from_env_follows_the_socket_path_variable() {
    std::env::remove_var("HERDR_SOCKET_PATH");
    assert!(from_env().is_none(), "no path, no host: every pane target is NoHost");
    std::env::set_var("HERDR_SOCKET_PATH", "/run/nowhere/herdr.sock");
    assert!(from_env().is_some(), "a path is enough here; the socket is opened per call");
}
```

Add `pub mod host;` to `src/engine/mod.rs`.

- [ ] **Step 6: The seams on `SessionConfig`**

In `src/engine/session.rs`, add to `SessionConfig` after `status_delay`:

```rust
    /// The host, `None` outside one: every pane target is then `NoHost` (spec 10.6).
    pub host: Option<Arc<dyn host::HostClient>>,
    /// `HERDR_SOCKET_PATH` as the shell read it; a remembered target names the socket it was picked on.
    pub socket_path: Option<String>,
```

with `host: None, socket_path: None` in `production()` and `use super::host;`. In `src/tui/shell.rs::run`, after `session.state_dir = ...`:

```rust
    session.host = engine::host::from_env();
    session.socket_path = std::env::var("HERDR_SOCKET_PATH").ok();
```

`production()` reads no environment, so every existing engine test stays hermetic (inside a herdr pane the harness exports `HERDR_SOCKET_PATH`, and a test that reached the live host would be a test of the wrong thing).

The session tests build `SessionConfig` as a struct literal in `start` (`src/engine/session.rs`, the `tests` module). Every task of this plan adds fields, so change that helper once, now, to start from `production` and override:

```rust
    fn test_config(dir: &std::path::Path, allow: Arc<AtomicBool>) -> SessionConfig {
        let mut config = SessionConfig::production(dir.to_path_buf());
        config.poll_interval = Duration::from_millis(50);
        config.watcher = Arc::new(FlakyWatcher { allow });
        config.git_check = ok_git();
        config
    }

    fn start(dir: &std::path::Path, allow: Arc<AtomicBool>) -> (tokio::runtime::Runtime, EngineHandle) {
        start_from(test_config(dir, allow))
    }

    // `start_with` and `wait_until` already exist in this module with other signatures; these are new names.
    fn start_from(config: SessionConfig) -> (tokio::runtime::Runtime, EngineHandle) {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let handle = spawn(rt.handle(), config);
        (rt, handle)
    }
```

Every existing test keeps calling `start`; the new tests of Tasks 2-5 call `start_from(test_config(..))` after setting `host`, `state_dir` and the other seams (the helper is `test_config`, not `config`, because those tests bind a local named `config` and a local shadows a function of the same name; it is `start_from`, not `start_with`, because the module already has a four-argument `start_with`). The session tests also hold nineteen other complete `SessionConfig { .. }` literals (`grep -n 'SessionConfig {' src/engine/session.rs`): give each one `..SessionConfig::production(dir.to_path_buf())` as its last line (struct update syntax, with `dir` being whatever path that literal uses), so every field this plan adds defaults there without further edits. `tests/readonly_guarantee.rs` and `tests/hunk_actions.rs` use `production` already, so nothing changes there.

- [ ] **Step 7: The fake host learns the three methods**

In `tests/support/mod.rs`, teach `FakeHerdr` the two new methods and the failure script Task 9's recording test and the Tier A tests need. Add fields and controls:

```rust
    prompt_failure: Arc<Mutex<Option<serde_json::Value>>>,
    reply_delay: Arc<Mutex<Option<Duration>>>,
    /// Accept, read the request, answer nothing (the client's `Read` phase).
    silent: Arc<AtomicBool>,
```

`silent` keeps the accepted stream open, unanswered, for six seconds (longer than the client's deadline) rather than dropping it, so what is tested is the deadline, not an EOF; `reply_raw: Arc<Mutex<Option<serde_json::Value>>>` makes the fake answer every request with the given JSON verbatim (an envelope the client must not mistake for success), with `pub fn reply_raw(&self, value: Option<serde_json::Value>)` to set and clear it. In the match on the method, before `_ =>`:

```rust
                                "pane.list" => Ok(serde_json::json!({
                                    "type": "pane_list",
                                    "panes": pane_response.lock().unwrap()["panes"].clone(),
                                })),
                                "agent.prompt" => match prompt_fail.lock().unwrap().clone() {
                                    Some(error) => Err(error),
                                    None => pane_response.lock().unwrap()["panes"]
                                        .as_array().unwrap().iter()
                                        .find(|pane| pane["pane_id"] == request["params"]["target"])
                                        .map(|pane| serde_json::json!({ "type": "agent_prompted", "agent": pane }))
                                        .ok_or_else(|| serde_json::json!({ "code": "agent_not_found", "message": "no agent in that pane" })),
                                },
```

Before writing the reply: `if silent_flag.load(Ordering::Relaxed) { std::thread::sleep(Duration::from_secs(6)); continue; }` (the stream stays open and unanswered past the client's deadline, then drops), `if let Some(raw) = raw_reply.lock().unwrap().clone() { write raw as the whole reply line; continue; }`, and `if let Some(delay) = *delay.lock().unwrap() { std::thread::sleep(delay); }`. Public controls:

```rust
    pub fn fail_prompt(&self, code: &str, message: &str) { *self.prompt_failure.lock().unwrap() = Some(serde_json::json!({ "code": code, "message": message })); }
    pub fn clear_prompt_failure(&self) { *self.prompt_failure.lock().unwrap() = None; }
    pub fn delay_replies(&self, delay: Option<Duration>) { *self.reply_delay.lock().unwrap() = delay; }
    pub fn go_silent(&self, silent: bool) { self.silent.store(silent, Ordering::Relaxed); }
```

`set_panes` already accepts any pane fields; a test passes `"agent": "codex", "agent_status": "idle", "agent_session": {...}` in its JSON and `pane_info` keeps them (`agent_status` defaults to `unknown` only when absent). Add one Tier A test to `tests/actions_tier_a.rs` proving the production wrapper speaks the protocol over a real socket:

```rust
#[test]
fn the_host_client_speaks_the_three_methods_over_the_socket() {
    use herdr_hunks::engine::host::{HerdrHost, HostClient, HostFailure};
    let dir = tempfile::tempdir().unwrap();
    let fake = support::FakeHerdr::start(dir.path());
    fake.set_panes(serde_json::json!([
        { "pane_id": "w1:p2", "agent": "codex", "agent_status": "idle", "cwd": "/r",
          "agent_session": { "kind": "id", "value": "s-1" } }
    ]));
    let host = HerdrHost::new(fake.socket_path.clone());
    let pane = host.pane_get("w1:p2").unwrap();
    assert_eq!((pane.agent.as_deref(), pane.agent_status.as_deref()), (Some("codex"), Some("idle")));
    assert_eq!(host.pane_list().unwrap().len(), 1);
    host.agent_prompt("w1:p2", "> hello\n").unwrap();
    let prompts = fake.calls_named("agent.prompt");
    assert_eq!(prompts[0]["params"], serde_json::json!({ "target": "w1:p2", "text": "> hello\n" }));
    assert!(prompts[0]["params"].get("wait").is_none());
    fake.fail_prompt("agent_not_ready", "blocked");
    assert_eq!(
        host.agent_prompt("w1:p2", "x"),
        Err(HostFailure::Api { code: "agent_not_ready".into(), message: "blocked".into() })
    );
    fake.clear_prompt_failure();
    fake.go_silent(true);
    let started = std::time::Instant::now();
    assert!(matches!(host.agent_prompt("w1:p2", "x"), Err(HostFailure::After(_))));
    assert!(started.elapsed() >= herdr_hunks::herdr::client::DEADLINE, "the silent host held the stream open and the deadline decided");
    fake.go_silent(false);
    // A reply the client cannot understand after writing is uncertain, never a success.
    fake.reply_raw(Some(serde_json::json!({})));
    assert!(matches!(host.agent_prompt("w1:p2", "x"), Err(HostFailure::After(_))));
    fake.reply_raw(Some(serde_json::Value::Null));
    assert!(matches!(host.agent_prompt("w1:p2", "x"), Err(HostFailure::After(_))));
    fake.reply_raw(Some(serde_json::json!({ "result": { "type": "pong" } })));
    assert!(matches!(host.agent_prompt("w1:p2", "x"), Err(HostFailure::After(_))), "another method's success is not this one's");
    fake.reply_raw(None);
    assert!(matches!(host.pane_get("w9:p9"), Err(HostFailure::Api { code, .. }) if code == "pane_not_found"));
    fake.stop();
    // The socket file is gone: NoHost, not a phase.
    assert!(matches!(host.pane_get("w1:p2"), Err(HostFailure::NoHost(_))));
}
```

The silent branch takes the full five-second deadline; the test is therefore the one slow test of the file, and says so in a comment.

- [ ] **Step 8: Gates and commit**

Run the gate line of the Global Constraints. Expected: all green; `cargo check --no-default-features` compiles `engine::host` without the TUI.

```bash
git add src/herdr/client.rs src/engine/host.rs src/engine/mod.rs src/engine/session.rs src/tui/shell.rs tests/support/mod.rs tests/actions_tier_a.rs tests/host_env.rs
git commit -m "feat(host): one host surface behind a trait, one deadline per request"
```

### Task 2: The target: chosen, remembered, re-verified

Implements spec 10.2 whole (the record, how it is chosen, the picker's rows, re-verification, what is remembered) and 10.7's target rows; 10.9 tests 1 and 2. Read 10.2 before starting.

**Files:**
- Create: `src/engine/target.rs`
- Modify: `src/engine/types.rs` (`Snapshot`, `Command`), `src/engine/session.rs` (`SessionConfig.{opener_pane, own_pane}`, `State`, `Job`, `run_job`, `Done`, the two commands, `fingerprint`), `src/engine/mod.rs`, `src/tui/shell.rs` (the two environment variables)

**Interfaces:**
- Consumes: Task 1's `HostClient`, `PaneRecord`, `SessionRef`, `HostFailure`, `Scripted`; 0.0.4's `reuse::with_lock`, `base::note_problem`, `base::is_object_id` (for nothing here, but the shape-check style).
- Produces:

```rust
// src/engine/target.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Pane { pane: String, socket: String, agent: String, session: Option<SessionRef>, title: String },
    Clipboard,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetState {
    Unverified,
    /// The status word the host reported: idle, working, blocked, done, unknown.
    Live(String),
    Restarted(String),
    Left,
    Gone,
    NoHost,
    Clipboard,
}

/// Where a send went, as the send verified it (10.3's `to`, 10.4's `requests.json` target).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum Destination {
    Pane { pane: String, agent: String, session: Option<SessionRef> },
    Clipboard { clipboard: bool },   // always `true`; the shape `{"clipboard": true}` of 10.2
}

/// How the target was chosen; only `Picked` is on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source { Remembered, Opener, Picked }

pub fn is_pane_id(text: &str) -> bool;                 // w<digits>:p<alphanumerics>
pub fn printable(text: &str) -> bool;                  // no control characters
pub fn check_session(session: &SessionRef) -> bool;    // kind id|path; value printable; absolute when path
/// Keyed by canonical toplevel; a record failing its shape is dropped with the reason.
pub fn load_targets(state_dir: &Path) -> (BTreeMap<String, Target>, Option<String>);
pub fn save_target(state_dir: &Path, toplevel: &str, target: &Target) -> std::io::Result<()>;
/// `save_target` that writes only while `latest` still equals `generation` (checked under the lock),
/// so a slow write for an older pick can never overwrite a newer one. `Ok(false)` means skipped.
pub fn save_target_if(state_dir: &Path, toplevel: &str, target: &Target, generation: u64, latest: &AtomicU64) -> std::io::Result<bool>;
/// The state a fresh check yields; `None` when the check could not run, so the previous state stands.
pub fn compare(target: &Target, socket_path: Option<&str>, fresh: &Result<PaneRecord, HostFailure>) -> Option<TargetState>;
/// `cwd` equals `toplevel` or lies under it, by path components.
pub fn is_under(cwd: &str, toplevel: &str) -> bool;
/// A picker row: the host's record and its group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneRow { pub record: PaneRecord, pub this_worktree: bool }
/// Agent panes other than the viewer's own, this worktree's first, the host's order within a group.
pub fn rows(panes: Vec<PaneRecord>, toplevel: &str, own_pane: Option<&str>) -> Vec<PaneRow>;
pub fn destination_of(record: &PaneRecord) -> Destination;

// src/engine/types.rs: Snapshot gains
pub target: Option<Target>,
pub target_state: TargetState,          // `NoHost` with no target and no host, `Unverified` otherwise (10.2's chip rule)
pub target_seq: u64,                    // bumped once per answered SetTarget
pub target_error: Option<String>,       // `target not remembered: <reason>` or None
pub panes: Option<Arc<Vec<PaneRow>>>,
pub panes_seq: u64,                     // the token of the LoadPanes answered last
pub panes_error: Option<String>,        // `could not list panes: <reason>`
// Command gains
LoadPanes(u64),
SetTarget(Target),

// src/engine/session.rs: SessionConfig gains
pub opener_pane: Option<String>,        // HERDR_HUNKS_OPENER_PANE
pub own_pane: Option<String>,           // HERDR_PANE_ID when the viewer is a split
// EngineHandle gains one test hook
pub target_checks: Arc<AtomicUsize>,    // pane.get calls made by refreshes
```

- [ ] **Step 1: The target module, tests first**

Create `src/engine/target.rs` with its tests module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn pane(agent: Option<&str>, status: &str, session: Option<&str>) -> PaneRecord {
        PaneRecord {
            pane_id: "w4:p2".into(),
            agent: agent.map(str::to_string),
            agent_status: Some(status.into()),
            agent_session: session.map(|v| SessionRef { kind: "id".into(), value: v.into() }),
            cwd: Some("/home/u/repo".into()),
            ..PaneRecord::default()
        }
    }

    fn target(session: Option<&str>) -> Target {
        Target::Pane {
            pane: "w4:p2".into(),
            socket: "/run/h.sock".into(),
            agent: "codex".into(),
            session: session.map(|v| SessionRef { kind: "id".into(), value: v.into() }),
            title: "fix".into(),
        }
    }

    #[test]
    fn every_state_of_the_table_is_reached() {
        let socket = Some("/run/h.sock");
        let t = target(Some("s1"));
        assert_eq!(compare(&t, socket, &Ok(pane(Some("codex"), "idle", Some("s1")))), Some(TargetState::Live("idle".into())));
        assert_eq!(compare(&t, socket, &Ok(pane(Some("codex"), "blocked", Some("s2")))), Some(TargetState::Restarted("blocked".into())));
        assert_eq!(compare(&t, socket, &Ok(pane(Some("claude"), "idle", Some("s1")))), Some(TargetState::Left));
        assert_eq!(compare(&t, socket, &Ok(pane(None, "unknown", None))), Some(TargetState::Left));
        let gone = Err(HostFailure::Api { code: "pane_not_found".into(), message: "x".into() });
        assert_eq!(compare(&t, socket, &gone), Some(TargetState::Gone));
        assert_eq!(compare(&t, socket, &Err(HostFailure::NoHost("off".into()))), Some(TargetState::NoHost));
        assert_eq!(compare(&t, None, &Ok(pane(Some("codex"), "idle", Some("s1")))), Some(TargetState::NoHost));
        // A check that could not run changes nothing.
        assert_eq!(compare(&t, socket, &Err(HostFailure::After("timeout".into()))), None);
        assert_eq!(compare(&t, socket, &Err(HostFailure::Before("eintr".into()))), None);
        assert_eq!(compare(&t, socket, &Err(HostFailure::Api { code: "internal".into(), message: "x".into() })), None);
        // Another socket: this host never had that pane.
        assert_eq!(compare(&t, Some("/run/other.sock"), &Ok(pane(Some("codex"), "idle", Some("s1")))), Some(TargetState::Gone));
        // A record without a session matches by pane and kind, and learns the session it now sees.
        let fresh = pane(Some("codex"), "working", Some("s9"));
        assert_eq!(compare(&target(None), socket, &Ok(fresh.clone())), Some(TargetState::Live("working".into())));
        assert_eq!(adopted_session(&target(None), &fresh).map(|s| s.value), Some("s9".to_string()));
        assert_eq!(adopted_session(&target(Some("s1")), &fresh), None, "a known session is never replaced by a check");
        // A known session that the host no longer reports is a restart, not continuity.
        assert_eq!(compare(&target(Some("s1")), socket, &Ok(pane(Some("codex"), "idle", None))), Some(TargetState::Restarted("idle".into())));
        assert_eq!(compare(&Target::Clipboard, None, &Err(HostFailure::NoHost("off".into()))), Some(TargetState::Clipboard));
        // A status the host did not name reads as unknown.
        let mut nameless = pane(Some("codex"), "idle", Some("s1"));
        nameless.agent_status = None;
        assert_eq!(compare(&t, socket, &Ok(nameless)), Some(TargetState::Live("unknown".into())));
        // A reply about another pane is no answer about this one.
        let mut other_pane = pane(Some("codex"), "idle", Some("s1"));
        other_pane.pane_id = "w4:p9".into();
        assert_eq!(compare(&t, socket, &Ok(other_pane)), None);
    }

    #[test]
    fn records_are_checked_by_shape_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let good = serde_json::json!({ "pane": "w4:p2", "socket": "/run/h.sock", "agent": "codex",
            "session": { "kind": "id", "value": "01a0" }, "title": "fix", "at": 1 });
        let clip = serde_json::json!({ "clipboard": true, "at": 2 });
        let bad = [
            serde_json::json!({ "pane": "w4;p2", "socket": "/run/h.sock", "agent": "codex", "session": null, "title": "", "at": 1 }),
            serde_json::json!({ "pane": "w4:p2", "socket": "run/h.sock", "agent": "codex", "session": null, "title": "", "at": 1 }),
            serde_json::json!({ "pane": "w4:p2", "socket": "/run/h.sock", "agent": "Codex", "session": null, "title": "", "at": 1 }),
            serde_json::json!({ "pane": "w4:p2", "socket": "/run/h.sock", "agent": "codex", "session": { "kind": "pid", "value": "1" }, "title": "", "at": 1 }),
            serde_json::json!({ "pane": "w4:p2", "socket": "/run/h.sock", "agent": "codex", "session": { "kind": "path", "value": "rel/x" }, "title": "", "at": 1 }),
            serde_json::json!({ "pane": "w4:p2", "socket": "/run/h.sock", "agent": "codex", "session": { "kind": "id", "value": "a\u{1b}b" }, "title": "", "at": 1 }),
            serde_json::json!({ "at": 1 }),
        ];
        let mut all = serde_json::Map::new();
        all.insert("/good".into(), good);
        all.insert("/clip".into(), clip);
        for (i, record) in bad.iter().enumerate() {
            all.insert(format!("/bad{i}"), record.clone());
        }
        std::fs::write(dir.path().join("targets.json"), serde_json::to_vec(&all).unwrap()).unwrap();
        let (targets, problem) = load_targets(dir.path());
        assert_eq!(targets.len(), 2, "{targets:?}");
        assert!(matches!(targets.get("/good"), Some(Target::Pane { pane, .. }) if pane == "w4:p2"));
        assert_eq!(targets.get("/clip"), Some(&Target::Clipboard));
        assert_eq!(problem.as_deref(), Some("targets.json: 7 unusable record(s)"));
        // Malformed file: absent with the reason; a later save rewrites it.
        std::fs::write(dir.path().join("targets.json"), b"{").unwrap();
        let (targets, problem) = load_targets(dir.path());
        assert!(targets.is_empty() && problem.is_some());
        save_target(dir.path(), "/repo", &Target::Clipboard).unwrap();
        assert_eq!(load_targets(dir.path()).0.get("/repo"), Some(&Target::Clipboard));
        assert!(save_target(std::path::Path::new("relative"), "/repo", &Target::Clipboard).is_err());
        // A write for an older pick is skipped under the lock; the newer pick's record stands.
        let latest = std::sync::atomic::AtomicU64::new(2);
        assert!(!save_target_if(dir.path(), "/repo", &target(Some("old")), 1, &latest).unwrap());
        assert_eq!(load_targets(dir.path()).0.get("/repo"), Some(&Target::Clipboard));
        assert!(save_target_if(dir.path(), "/repo", &target(Some("new")), 2, &latest).unwrap());
        assert!(matches!(load_targets(dir.path()).0.get("/repo"), Some(Target::Pane { session: Some(s), .. }) if s.value == "new"));
    }

    #[test]
    fn a_sibling_directory_with_a_shared_prefix_is_another_pane() {
        assert!(is_under("/home/u/repo", "/home/u/repo"));
        assert!(is_under("/home/u/repo/src", "/home/u/repo"));
        assert!(!is_under("/home/u/repo2", "/home/u/repo"));
        assert!(!is_under("/home/u", "/home/u/repo"));
        let mut here = PaneRecord { pane_id: "w1:p1".into(), agent: Some("codex".into()), cwd: Some("/home/u/repo2".into()), ..PaneRecord::default() };
        let listed = rows(vec![here.clone()], "/home/u/repo", None);
        assert!(!listed[0].this_worktree);
        here.foreground_cwd = Some("/home/u/repo/sub".into());
        assert!(rows(vec![here], "/home/u/repo", None)[0].this_worktree);
    }

    #[test]
    fn rows_drop_shells_and_the_viewer_and_put_this_worktree_first() {
        let p = |id: &str, agent: Option<&str>, cwd: &str| PaneRecord {
            pane_id: id.into(), agent: agent.map(str::to_string), cwd: Some(cwd.into()), ..PaneRecord::default()
        };
        let listed = rows(
            vec![p("w1:p1", None, "/r"), p("w1:p2", Some("codex"), "/other"), p("w1:p3", Some("claude"), "/r/x"), p("w1:p4", Some("kimi"), "/r")],
            "/r",
            Some("w1:p4"),
        );
        let ids: Vec<_> = listed.iter().map(|r| r.record.pane_id.as_str()).collect();
        assert_eq!(ids, ["w1:p3", "w1:p2"]);
        assert!(listed[0].this_worktree && !listed[1].this_worktree);
    }

    #[test]
    fn pane_ids_follow_the_hosts_grammar() {
        assert!(is_pane_id("w4:p2") && is_pane_id("w12:pA9"));
        assert!(!is_pane_id("w4") && !is_pane_id("4:p2") && !is_pane_id("w4:p") && !is_pane_id("w4:p2 ") && !is_pane_id("w4:p2;rm"));
    }
}
```

- [ ] **Step 2: Run to watch them fail**

Add `pub mod target;` to `src/engine/mod.rs` first (a filter that matches no registered module runs zero tests and passes). Run: `cargo test --locked --lib engine::target`
Expected: compile errors, every name undefined.

- [ ] **Step 3: The module**

Above the tests:

```rust
//! The target of a review (spec 10.2): chosen once, remembered per worktree, re-verified every refresh.
use std::collections::BTreeMap;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

use super::host::{HostFailure, PaneRecord, SessionRef};
use crate::actions::reuse;

const TARGETS_FILE: &str = "targets.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Pane {
        pane: String,
        socket: String,
        agent: String,
        session: Option<SessionRef>,
        title: String,
    },
    Clipboard,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetState {
    Unverified,
    /// The status word the host reported: idle, working, blocked, done, unknown.
    Live(String),
    Restarted(String),
    Left,
    Gone,
    NoHost,
    Clipboard,
}

/// Where a send went, as the send verified it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Destination {
    Pane {
        pane: String,
        agent: String,
        session: Option<SessionRef>,
    },
    Clipboard {
        clipboard: bool,
    },
}

impl Destination {
    pub fn clipboard() -> Self {
        Self::Clipboard { clipboard: true }
    }

    /// The shape checks of a target record, on the three fields a destination carries.
    pub fn is_well_formed(&self) -> bool {
        match self {
            Self::Clipboard { clipboard } => *clipboard,
            Self::Pane { pane, agent, session } => {
                is_pane_id(pane) && is_agent(agent) && session.as_ref().is_none_or(check_session)
            }
        }
    }
}

/// How the target was chosen; only `Picked` is on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Remembered,
    Opener,
    Picked,
}

/// `w<digits>:p<alphanumerics>`, the host's grammar; a pane id is only ever a request parameter.
pub fn is_pane_id(text: &str) -> bool {
    let Some(rest) = text.strip_prefix('w') else { return false };
    let Some((workspace, pane)) = rest.split_once(":p") else { return false };
    !workspace.is_empty()
        && workspace.bytes().all(|b| b.is_ascii_digit())
        && !pane.is_empty()
        && pane.bytes().all(|b| b.is_ascii_alphanumeric())
}

pub fn printable(text: &str) -> bool {
    !text.chars().any(|c| c.is_control())
}

fn is_agent(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

pub fn check_session(session: &SessionRef) -> bool {
    match session.kind.as_str() {
        "id" => !session.value.is_empty() && printable(&session.value),
        "path" => printable(&session.value) && Path::new(&session.value).is_absolute(),
        _ => false,
    }
}

/// The on-disk record: a pane with its socket, or `{"clipboard": true}`; `at` in both.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Record {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pane: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    socket: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<SessionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    clipboard: Option<bool>,
    #[serde(default)]
    at: u64,
}

impl Record {
    fn of(target: &Target) -> Self {
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        match target {
            Target::Clipboard => Self {
                clipboard: Some(true),
                at,
                ..Self::default()
            },
            Target::Pane {
                pane,
                socket,
                agent,
                session,
                title,
            } => Self {
                pane: Some(pane.clone()),
                socket: Some(socket.clone()),
                agent: Some(agent.clone()),
                session: session.clone(),
                title: Some(title.clone()),
                clipboard: None,
                at,
            },
        }
    }

    /// Every field checked by shape before any of it is used.
    fn target(self) -> Option<Target> {
        if self.clipboard == Some(true) {
            return Some(Target::Clipboard);
        }
        let (pane, socket, agent) = (self.pane?, self.socket?, self.agent?);
        let title = self.title.unwrap_or_default();
        let well_formed = is_pane_id(&pane)
            && Path::new(&socket).is_absolute()
            && printable(&socket)
            && is_agent(&agent)
            && self.session.as_ref().is_none_or(check_session)
            && printable(&title);
        well_formed.then_some(Target::Pane {
            pane,
            socket,
            agent,
            session: self.session,
            title,
        })
    }
}

/// Keyed by canonical toplevel; a record failing its shape is dropped, as is an unreadable
/// or malformed file, with the reason.
pub fn load_targets(state_dir: &Path) -> (BTreeMap<String, Target>, Option<String>) {
    let text = match std::fs::read_to_string(state_dir.join(TARGETS_FILE)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (BTreeMap::new(), None),
        Err(e) => return (BTreeMap::new(), Some(format!("{TARGETS_FILE}: {e}"))),
        Ok(text) => text,
    };
    // Each record is decoded on its own, so one wrong field type drops that record, not every worktree's.
    let records: BTreeMap<String, serde_json::Value> = match serde_json::from_str(&text) {
        Ok(records) => records,
        Err(e) => return (BTreeMap::new(), Some(format!("{TARGETS_FILE}: {e}"))),
    };
    let total = records.len();
    let kept: BTreeMap<String, Target> = records
        .into_iter()
        .filter_map(|(toplevel, value)| {
            serde_json::from_value::<Record>(value)
                .ok()
                .and_then(Record::target)
                .map(|t| (toplevel, t))
        })
        .collect();
    let dropped = total - kept.len();
    let problem = (dropped > 0).then(|| format!("{TARGETS_FILE}: {dropped} unusable record(s)"));
    (kept, problem)
}

/// Read-modify-write under the state lock, then an atomic replace, as `save_mark` does.
pub fn save_target(state_dir: &Path, toplevel: &str, target: &Target) -> std::io::Result<()> {
    if !state_dir.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "state directory must be absolute",
        ));
    }
    reuse::with_lock(state_dir, || {
        // Records that fail their shape are rewritten out, as `load_marks` drops a bad mark.
        let (targets, _) = load_targets(state_dir);
        let mut records: BTreeMap<String, Record> = targets
            .iter()
            .map(|(k, t)| (k.clone(), Record::of(t)))
            .collect();
        records.insert(toplevel.to_string(), Record::of(target));
        let tmp = state_dir.join(format!("{TARGETS_FILE}.{}.tmp", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec_pretty(&records).unwrap_or_default())?;
        std::fs::rename(tmp, state_dir.join(TARGETS_FILE))
    })?
}

/// `save_target` guarded by the selection generation: the comparison happens under the lock, so two
/// writers for different picks land in pick order whatever order their tasks ran in.
pub fn save_target_if(
    state_dir: &Path,
    toplevel: &str,
    target: &Target,
    generation: u64,
    latest: &std::sync::atomic::AtomicU64,
) -> std::io::Result<bool> {
    if !state_dir.is_absolute() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "state directory must be absolute"));
    }
    reuse::with_lock(state_dir, || -> std::io::Result<bool> {
        if latest.load(std::sync::atomic::Ordering::SeqCst) != generation {
            return Ok(false);
        }
        let (targets, _) = load_targets(state_dir);
        let mut records: BTreeMap<String, Record> = targets.iter().map(|(k, t)| (k.clone(), Record::of(t))).collect();
        records.insert(toplevel.to_string(), Record::of(target));
        let tmp = state_dir.join(format!("{TARGETS_FILE}.{}.tmp", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec_pretty(&records).unwrap_or_default())?;
        std::fs::rename(tmp, state_dir.join(TARGETS_FILE))?;
        Ok(true)
    })?
}

/// The state a fresh check yields; `None` when the check could not run, so the previous state
/// stands (spec 10.2 "Re-verification").
pub fn compare(
    target: &Target,
    socket_path: Option<&str>,
    fresh: &Result<PaneRecord, HostFailure>,
) -> Option<TargetState> {
    let Target::Pane {
        pane,
        socket,
        agent,
        session,
        ..
    } = target
    else {
        return Some(TargetState::Clipboard);
    };
    let Some(socket_path) = socket_path else {
        return Some(TargetState::NoHost);
    };
    if socket_path != socket {
        // The record was picked on another host; this one never had that pane.
        return Some(TargetState::Gone);
    }
    match fresh {
        Err(HostFailure::NoHost(_)) => Some(TargetState::NoHost),
        Err(HostFailure::Api { code, .. })
            if code == "pane_not_found" || code == "target_pane_not_found" =>
        {
            Some(TargetState::Gone)
        }
        Err(_) => None,
        // A reply about another pane is no answer about this one: the check did not run.
        Ok(record) if record.pane_id != *pane => None,
        Ok(record) => {
            if record.agent.as_deref() != Some(agent.as_str()) {
                return Some(TargetState::Left);
            }
            let status = record
                .agent_status
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            // A known session matches only itself: a different one, or none, is a restart. A record
            // without one matches by pane and kind; `adopted_session` says when it learns one.
            match (session, &record.agent_session) {
                (Some(known), Some(now)) if known == now => Some(TargetState::Live(status)),
                (Some(_), _) => Some(TargetState::Restarted(status)),
                (None, _) => Some(TargetState::Live(status)),
            }
        }
    }
}

/// The session a record without one learns from a fresh check (spec 10.2 "Session matching");
/// the engine rewrites the record with it on its next write.
pub fn adopted_session(target: &Target, fresh: &PaneRecord) -> Option<SessionRef> {
    match target {
        Target::Pane { session: None, agent, .. } if fresh.agent.as_deref() == Some(agent.as_str()) => {
            fresh.agent_session.clone()
        }
        _ => None,
    }
}

/// `cwd` equals `toplevel` or lies under it, by path components: `/r2` is not under `/r`.
pub fn is_under(cwd: &str, toplevel: &str) -> bool {
    let cwd: Vec<Component<'_>> = Path::new(cwd).components().collect();
    let top: Vec<Component<'_>> = Path::new(toplevel).components().collect();
    cwd.len() >= top.len() && cwd.iter().zip(&top).all(|(a, b)| a == b)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneRow {
    pub record: PaneRecord,
    pub this_worktree: bool,
}

/// Agent panes other than the viewer's own, this worktree's first, the host's order within a group.
pub fn rows(panes: Vec<PaneRecord>, toplevel: &str, own_pane: Option<&str>) -> Vec<PaneRow> {
    let mut rows: Vec<PaneRow> = panes
        .into_iter()
        .filter(|p| p.agent.is_some() && Some(p.pane_id.as_str()) != own_pane)
        .map(|record| {
            let this_worktree = [&record.cwd, &record.foreground_cwd]
                .into_iter()
                .flatten()
                .any(|dir| is_under(dir, toplevel));
            PaneRow {
                record,
                this_worktree,
            }
        })
        .collect();
    // A stable sort keeps the host's order inside each group.
    rows.sort_by_key(|r| !r.this_worktree);
    rows
}

pub fn destination_of(record: &PaneRecord) -> Destination {
    Destination::Pane {
        pane: record.pane_id.clone(),
        agent: record.agent.clone().unwrap_or_default(),
        session: record.agent_session.clone(),
    }
}
```

Add `pub mod target;` to `src/engine/mod.rs` and `pub use target::{Destination, PaneRow, Target, TargetState};` beside `pub use types::*;`. `is_none_or` is in Rust 1.82; the floor is 1.88.

- [ ] **Step 4: Run the module tests**

Run: `cargo test --locked --lib engine::target`
Expected: five pass. The interfaces block above gains `pub fn adopted_session(target: &Target, fresh: &PaneRecord) -> Option<SessionRef>`.

- [ ] **Step 5: Snapshot, commands and the session: the tests**

Add to `src/engine/session.rs`'s tests, using Task 1's `test_config`/`start_from` and `host::Scripted`, and one more helper beside `wait_for`, for conditions no snapshot announces (a counter, a file), because `wait_for`'s predicate runs only when a snapshot arrives and an unchanged state is never republished (the module already has a `wait_until<T>` with another shape; this one is `wait_cond`):

```rust
    fn wait_cond(what: &str, mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if check() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for: {what}");
    }
```

The waits below follow the rule of AGENTS.md: wait for the state the assertion needs, in one predicate, never for a proxy of it, because a wait that is satisfied early can consume the snapshot the next wait needs.

```rust
    fn agent_pane(id: &str, agent: &str, status: &str, session: Option<&str>, cwd: &str) -> host::PaneRecord {
        host::PaneRecord {
            pane_id: id.into(),
            agent: Some(agent.into()),
            agent_status: Some(status.into()),
            agent_session: session.map(|v| host::SessionRef { kind: "id".into(), value: v.into() }),
            cwd: Some(cwd.into()),
            ..host::PaneRecord::default()
        }
    }

    fn pane_target(id: &str, agent: &str, session: Option<&str>) -> Target {
        Target::Pane {
            pane: id.into(),
            socket: "/run/fake.sock".into(),
            agent: agent.into(),
            session: session.map(|v| host::SessionRef { kind: "id".into(), value: v.into() }),
            title: String::new(),
        }
    }

    #[test]
    fn a_target_is_written_published_unverified_and_then_checked_every_refresh() {
        let dir = fixture();
        let state = tempfile::tempdir().unwrap();
        let top = dir.path().canonicalize().unwrap().to_string_lossy().into_owned();
        let host = host::Scripted::with_panes(vec![agent_pane("w4:p2", "codex", "idle", Some("s1"), &top)]);
        let mut config = test_config(dir.path(), Arc::new(AtomicBool::new(true)));
        config.state_dir = Some(state.path().to_path_buf());
        config.host = Some(host.clone());
        config.socket_path = Some("/run/fake.sock".into());
        let (_rt, handle) = start_from(config);
        wait_for(&handle, "rows", |s| !s.files.is_empty());
        assert_eq!(handle.target_checks.load(Ordering::SeqCst), 0, "no target, no check");
        handle.commands.send(Command::SetTarget(pane_target("w4:p2", "codex", Some("s1")))).unwrap();
        // One predicate for the state the assertion needs: a wait for a proxy could eat the snapshot
        // the next wait needs (the pick, its answer and its first check can arrive in two frames).
        let s = wait_for(&handle, "the pick answered and checked", |s| {
            s.target_seq == 1 && s.target.is_some() && s.target_state == TargetState::Live("idle".into())
        });
        assert!(s.target_error.is_none());
        let (targets, _) = target::load_targets(state.path());
        assert!(matches!(targets.get(&top), Some(Target::Pane { pane, .. }) if pane == "w4:p2"));
        // The table changes; the next refresh sees it.
        host.set_pane(agent_pane("w4:p2", "codex", "blocked", Some("s1"), &top));
        wait_for(&handle, "blocked", |s| s.target_state == TargetState::Live("blocked".into()));
        host.set_pane(agent_pane("w4:p2", "codex", "idle", Some("s2"), &top));
        wait_for(&handle, "restarted", |s| s.target_state == TargetState::Restarted("idle".into()));
        host.set_pane(host::PaneRecord { pane_id: "w4:p2".into(), agent_status: Some("unknown".into()), ..host::PaneRecord::default() });
        wait_for(&handle, "left", |s| s.target_state == TargetState::Left);
        host.remove_pane("w4:p2");
        wait_for(&handle, "gone", |s| s.target_state == TargetState::Gone);
        // A check that cannot run keeps the state: two more checks run (the counter says so, no
        // snapshot does, because an unchanged state is never republished) and nothing new arrives.
        *host.list_failure.lock().unwrap() = Some(host::HostFailure::After("deadline".into()));
        let before = handle.target_checks.load(Ordering::SeqCst);
        wait_cond("two more checks", || handle.target_checks.load(Ordering::SeqCst) >= before + 2);
        assert!(handle.snapshots.try_iter().all(|s| s.target_state == TargetState::Gone), "a failed check changed the state");
        *host.list_failure.lock().unwrap() = Some(host::HostFailure::NoHost("socket gone".into()));
        wait_for(&handle, "no host", |s| s.target_state == TargetState::NoHost);
    }

    #[test]
    fn a_clipboard_target_needs_no_host_and_a_pane_target_without_one_is_no_host() {
        let dir = fixture();
        let mut config = test_config(dir.path(), Arc::new(AtomicBool::new(true)));
        config.state_dir = None;
        let (_rt, handle) = start_from(config);
        wait_for(&handle, "rows", |s| !s.files.is_empty());
        let s = wait_for(&handle, "no target, no host", |s| s.target_state == TargetState::NoHost);
        assert!(s.target.is_none());
        handle.commands.send(Command::SetTarget(Target::Clipboard)).unwrap();
        let s = wait_for(&handle, "clipboard", |s| s.target_seq == 1);
        assert_eq!((s.target.clone(), s.target_state.clone()), (Some(Target::Clipboard), TargetState::Clipboard));
        assert_eq!(s.target_error.as_deref(), Some("target not remembered: no state directory"));
        handle.commands.send(Command::SetTarget(pane_target("w4:p2", "codex", None))).unwrap();
        let s = wait_for(&handle, "pane without host", |s| s.target_seq == 2);
        assert_eq!(s.target_state, TargetState::NoHost);
    }

    #[test]
    fn a_remembered_target_beats_the_opener_and_the_opener_is_not_written() {
        let dir = fixture();
        let state = tempfile::tempdir().unwrap();
        let top = dir.path().canonicalize().unwrap().to_string_lossy().into_owned();
        let host = host::Scripted::with_panes(vec![
            agent_pane("w4:p1", "claude", "idle", Some("c1"), &top),
            agent_pane("w4:p2", "codex", "idle", Some("s1"), &top),
        ]);
        // Nothing remembered: the opener pane is preselected, unwritten.
        let mut config = test_config(dir.path(), Arc::new(AtomicBool::new(true)));
        config.state_dir = Some(state.path().to_path_buf());
        config.host = Some(host.clone());
        config.socket_path = Some("/run/fake.sock".into());
        config.opener_pane = Some("w4:p1".into());
        let (rt, handle) = start_from(config);
        let s = wait_for(&handle, "opener preselected", |s| s.target.is_some());
        assert!(matches!(&s.target, Some(Target::Pane { pane, agent, .. }) if pane == "w4:p1" && agent == "claude"));
        wait_for(&handle, "live", |s| matches!(s.target_state, TargetState::Live(_)));
        assert!(target::load_targets(state.path()).0.is_empty(), "the opener was written");
        drop(handle);
        drop(rt);
        // Remembered: it wins over the opener.
        target::save_target(state.path(), &top, &pane_target("w4:p2", "codex", Some("s1"))).unwrap();
        let mut config = test_config(dir.path(), Arc::new(AtomicBool::new(true)));
        config.state_dir = Some(state.path().to_path_buf());
        config.host = Some(host.clone());
        config.socket_path = Some("/run/fake.sock".into());
        config.opener_pane = Some("w4:p1".into());
        let (_rt, handle) = start_from(config);
        let s = wait_for(&handle, "remembered target", |s| s.target.is_some());
        assert!(matches!(&s.target, Some(Target::Pane { pane, .. }) if pane == "w4:p2"));
        // An opener that runs a shell is nothing.
        let shell_host = host::Scripted::with_panes(vec![host::PaneRecord { pane_id: "w4:p1".into(), ..host::PaneRecord::default() }]);
        let empty_state = tempfile::tempdir().unwrap();
        let mut config = test_config(dir.path(), Arc::new(AtomicBool::new(true)));
        config.state_dir = Some(empty_state.path().to_path_buf());
        config.host = Some(shell_host);
        config.socket_path = Some("/run/fake.sock".into());
        config.opener_pane = Some("w4:p1".into());
        let (_rt, handle) = start_from(config);
        let s = wait_for(&handle, "rows", |s| !s.files.is_empty() && !s.refreshing);
        assert!(s.target.is_none());
        assert_eq!(s.target_state, TargetState::Unverified, "no target and a host: the chip reads no agent");
    }

    #[test]
    fn a_check_answered_after_a_repick_is_dropped() {
        let dir = fixture();
        let top = dir.path().canonicalize().unwrap().to_string_lossy().into_owned();
        let host = host::Scripted::with_panes(vec![agent_pane("w4:p2", "codex", "idle", Some("s1"), &top)]);
        let mut config = test_config(dir.path(), Arc::new(AtomicBool::new(true)));
        config.host = Some(host.clone());
        config.socket_path = Some("/run/fake.sock".into());
        config.poll_interval = Duration::from_secs(3600);
        // Holds every refresh after its status read, so a check is in flight when the re-pick lands.
        config.status_delay = Some(Duration::from_millis(400));
        let (_rt, handle) = start_from(config);
        wait_for(&handle, "rows", |s| !s.files.is_empty() && !s.refreshing);
        handle.commands.send(Command::SetTarget(pane_target("w4:p2", "codex", Some("s1")))).unwrap();
        wait_for(&handle, "live", |s| s.target_state == TargetState::Live("idle".into()));
        // The agent restarts and the reviewer picks the same pane again while a check runs.
        host.set_pane(agent_pane("w4:p2", "codex", "idle", Some("s2"), &top));
        handle.commands.send(Command::Refresh).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        handle.commands.send(Command::SetTarget(pane_target("w4:p2", "codex", Some("s2")))).unwrap();
        let s = wait_for(&handle, "re-pick answered", |s| s.target_seq == 2);
        assert_eq!(s.target_state, TargetState::Unverified);
        let s = wait_for(&handle, "its own check", |s| s.target_state != TargetState::Unverified);
        assert_eq!(s.target_state, TargetState::Live("idle".into()), "the stale check marked the new pick restarted");
    }

    #[test]
    fn an_opener_reply_about_another_pane_preselects_nothing() {
        let dir = fixture();
        let state = tempfile::tempdir().unwrap();
        let top = dir.path().canonicalize().unwrap().to_string_lossy().into_owned();
        let host = host::Scripted::with_panes(vec![agent_pane("w4:p2", "codex", "idle", Some("s1"), &top)]);
        // The opener is w4:p2; the host answers about w4:p9, an agent pane too.
        *host.answer_pane_get_with.lock().unwrap() = Some(agent_pane("w4:p9", "claude", "idle", Some("s9"), &top));
        let mut config = test_config(dir.path(), Arc::new(AtomicBool::new(true)));
        config.state_dir = Some(state.path().to_path_buf());
        config.host = Some(host.clone());
        config.socket_path = Some("/run/fake.sock".into());
        config.opener_pane = Some("w4:p2".into());
        let (_rt, handle) = start_from(config);
        wait_for(&handle, "rows", |s| !s.files.is_empty());
        wait_cond("the opener was asked about", || host.calls.lock().unwrap().iter().filter(|c| *c == "pane.get").count() >= 1);
        // A pane listing publishes a snapshot (this task's own command); the target in it is none, and
        // the file has no record.
        handle.commands.send(Command::LoadPanes(1)).unwrap();
        let s = wait_for(&handle, "a snapshot after the resolution", |s| s.panes_seq == 1);
        assert_eq!(s.target, None, "a reply about w4:p9 preselected it for an opener in w4:p2");
        assert!(target::load_targets(state.path()).0.get(&top).is_none());
    }

    #[test]
    fn a_reply_about_another_pane_teaches_the_target_nothing() {
        let dir = fixture();
        let state = tempfile::tempdir().unwrap();
        let top = dir.path().canonicalize().unwrap().to_string_lossy().into_owned();
        // The remembered record has no session; the host answers every `pane.get` about another pane.
        target::save_target(state.path(), &top, &pane_target("w4:p2", "codex", None)).unwrap();
        let host = host::Scripted::with_panes(vec![agent_pane("w4:p2", "codex", "idle", Some("s1"), &top)]);
        *host.answer_pane_get_with.lock().unwrap() = Some(agent_pane("w4:p9", "codex", "idle", Some("s9"), &top));
        let mut config = test_config(dir.path(), Arc::new(AtomicBool::new(true)));
        config.state_dir = Some(state.path().to_path_buf());
        config.host = Some(host.clone());
        config.socket_path = Some("/run/fake.sock".into());
        let (_rt, handle) = start_from(config);
        wait_for(&handle, "resolved", |s| matches!(&s.target, Some(Target::Pane { pane, .. }) if pane == "w4:p2"));
        wait_cond("two replies about the other pane", || handle.target_checks.load(Ordering::SeqCst) >= 2);
        *host.answer_pane_get_with.lock().unwrap() = None;
        // Every snapshot published from the pick to the first check about this pane: nothing was learned
        // from the other pane's replies, neither a state nor a session.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            assert!(Instant::now() < deadline, "no check about this pane was published");
            let Ok(s) = handle.snapshots.try_recv() else {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            };
            if s.target_state == TargetState::Live("idle".into()) {
                assert!(matches!(&s.target, Some(Target::Pane { session: Some(sess), .. }) if sess.value == "s1"), "learned from its own reply");
                break;
            }
            assert_eq!(s.target_state, TargetState::Unverified, "a reply about w4:p9 gave w4:p2 a state");
            assert!(matches!(&s.target, Some(Target::Pane { session: None, .. })), "a reply about w4:p9 gave w4:p2 a session");
        }
    }

    #[test]
    fn a_pick_answered_after_a_newer_pick_is_dropped() {
        let dir = fixture();
        let state = tempfile::tempdir().unwrap();
        let top = dir.path().canonicalize().unwrap().to_string_lossy().into_owned();
        let host = host::Scripted::with_panes(vec![agent_pane("w4:p2", "codex", "idle", Some("s1"), &top)]);
        let mut config = test_config(dir.path(), Arc::new(AtomicBool::new(true)));
        config.state_dir = Some(state.path().to_path_buf());
        config.host = Some(host.clone());
        config.socket_path = Some("/run/fake.sock".into());
        // Both picks' writes wait at the gate; released in order, the older one runs first, finds the
        // newer generation, and its answer must reach no one: the picker waits for its own pick's.
        let gate = Arc::new(Semaphore::new(0));
        config.target_write_gate = Some(gate.clone());
        let (_rt, handle) = start_from(config);
        wait_for(&handle, "rows", |s| !s.files.is_empty());
        handle.commands.send(Command::SetTarget(pane_target("w4:p2", "codex", Some("s1")))).unwrap();
        wait_for(&handle, "first pick published", |s| matches!(&s.target, Some(Target::Pane { .. })));
        handle.commands.send(Command::SetTarget(Target::Clipboard)).unwrap();
        wait_for(&handle, "second pick published", |s| s.target == Some(Target::Clipboard));
        gate.add_permits(1);
        wait_cond("the older write ran", || handle.target_writes_done.load(Ordering::SeqCst) == 1);
        std::thread::sleep(Duration::from_millis(100));
        while let Ok(s) = handle.snapshots.try_recv() {
            assert_eq!(s.target_seq, 0, "an older pick's answer reached the picker");
        }
        gate.add_permits(1);
        let s = wait_for(&handle, "the newer pick answered", |s| s.target_seq == 1);
        assert_eq!((s.target.clone(), s.target_error.clone()), (Some(Target::Clipboard), None));
        assert_eq!(target::load_targets(state.path()).0.get(&top), Some(&Target::Clipboard));
    }

    #[test]
    fn load_panes_lists_agent_panes_in_groups_and_drops_stale_tokens() {
        let dir = fixture();
        let top = dir.path().canonicalize().unwrap().to_string_lossy().into_owned();
        let host = host::Scripted::with_panes(vec![
            host::PaneRecord { pane_id: "w1:p1".into(), cwd: Some(top.clone()), ..host::PaneRecord::default() },
            agent_pane("w1:p2", "codex", "idle", None, "/elsewhere"),
            agent_pane("w1:p3", "claude", "working", None, &format!("{top}/src")),
            agent_pane("w1:p9", "kimi", "idle", None, &top),
        ]);
        let mut config = test_config(dir.path(), Arc::new(AtomicBool::new(true)));
        config.host = Some(host.clone());
        config.socket_path = Some("/run/fake.sock".into());
        config.own_pane = Some("w1:p9".into());
        let (_rt, handle) = start_from(config);
        wait_for(&handle, "rows", |s| !s.files.is_empty());
        handle.commands.send(Command::LoadPanes(7)).unwrap();
        let s = wait_for(&handle, "panes", |s| s.panes_seq == 7);
        let ids: Vec<_> = s.panes.as_ref().unwrap().iter().map(|r| r.record.pane_id.clone()).collect();
        assert_eq!(ids, ["w1:p3", "w1:p2"]);
        assert!(s.panes_error.is_none());
        // A failure names its reason and keeps the answer flowing.
        *host.list_failure.lock().unwrap() = Some(host::HostFailure::After("deadline".into()));
        handle.commands.send(Command::LoadPanes(8)).unwrap();
        let s = wait_for(&handle, "failure", |s| s.panes_seq == 8);
        assert_eq!(s.panes_error.as_deref(), Some("could not list panes: deadline"));
        assert!(s.panes.as_ref().unwrap().is_empty());
        // An older token's reply never overwrites a newer opening.
        *host.list_failure.lock().unwrap() = None;
        handle.commands.send(Command::LoadPanes(3)).unwrap();
        handle.commands.send(Command::LoadPanes(9)).unwrap();
        let s = wait_for(&handle, "newest", |s| s.panes_seq == 9);
        assert_eq!(s.panes.as_ref().unwrap().len(), 2);
        std::thread::sleep(Duration::from_millis(200));
        assert!(handle.snapshots.try_iter().all(|s| s.panes_seq == 9));
        // No host: an empty list, no error; the picker then offers the clipboard alone.
        let mut config = test_config(dir.path(), Arc::new(AtomicBool::new(true)));
        config.host = None;
        let (_rt2, handle2) = start_from(config);
        handle2.commands.send(Command::LoadPanes(1)).unwrap();
        let s = wait_for(&handle2, "no host panes", |s| s.panes_seq == 1);
        assert!(s.panes.as_ref().unwrap().is_empty() && s.panes_error.is_none());
    }
```

- [ ] **Step 6: Run them to watch them fail**

Run: `cargo test --locked --lib engine::session::tests::a_target_is_written`
Expected: compile errors (`Command::SetTarget`, `Snapshot.target`, `EngineHandle.target_checks`).

- [ ] **Step 7: Types**

In `src/engine/types.rs`, `nav::Target` is already imported (it types `LoadedDiff.targets`), so the review target is written by path there: `use crate::engine::target::{PaneRow, TargetState};` and `crate::engine::target::Target` in the field; `engine/mod.rs`'s `pub use target::Target` is what everyone else imports. Add to `Snapshot`, after `action_applied`:

```rust
    /// The pane or clipboard a review goes to; `None` until one is chosen (spec 10.2).
    pub target: Option<crate::engine::target::Target>,
    /// The last check's answer; `NoHost` with no target and no host, `Unverified` with no target and a host.
    pub target_state: TargetState,
    /// Bumped once per answered `SetTarget`; `target_error` is that answer.
    pub target_seq: u64,
    pub target_error: Option<String>,
    /// The pane picker's rows under the opening's token, as `refs` are for the base picker.
    pub panes: Option<Arc<Vec<PaneRow>>>,
    pub panes_seq: u64,
    pub panes_error: Option<String>,
```

with `target: None, target_state: TargetState::Unverified, target_seq: 0, target_error: None, panes: None, panes_seq: 0, panes_error: None` in `empty`. Add to `Command`:

```rust
    /// Answer with the agent panes and this opening's token on the snapshot (spec 10.2).
    LoadPanes(u64),
    /// Remember and publish the target; the next refresh checks it (`target::Target`, by path: `nav::Target` is imported here).
    SetTarget(crate::engine::target::Target),
```

- [ ] **Step 8: The session**

In `src/engine/session.rs`:

1. `SessionConfig` gains `pub opener_pane: Option<String>` and `pub own_pane: Option<String>` (both `None` in `production`). `EngineHandle` gains `pub target_checks: Arc<AtomicUsize>`, threaded through `spawn` and `run` like `pre_images`.

2. `State` gains:

```rust
    /// How the published target was chosen; an opener preselection is written on the first comment.
    target_source: Option<target::Source>,
    /// Advanced by every `SetTarget`; a check answered under an older value is dropped. `latest_selection`
    /// is the same number behind an `Arc`, read by tasks that must notice a newer pick (Task 5).
    selection_generation: u64,
    latest_selection: Arc<AtomicU64>,
    target_seq: u64,
    /// The first refresh loads `targets.json` and asks about the opener; later ones only re-verify.
    target_unresolved: bool,
    /// The record in memory differs from the file (an opener preselection, an adopted session): the
    /// next write of a pick or a comment carries it.
    target_write_pending: bool,
```

initialised `None, 0, Arc::new(AtomicU64::new(0)), 0, true, false` (`SetTarget` stores the new generation in both). `Job` carries `generation: u64`, the selection generation `request_status` read when it built the job, so a resolution answers under the generation it started for.

3. `Job` gains `target: Option<(Target, u64)>` (the published pane target and the generation it was selected under; `None` for no target or a clipboard target), `resolve_target: Option<Option<String>>` (`Some(opener)` on the first refresh), `host: Option<Arc<dyn host::HostClient>>`, `socket_path: Option<String>`, `state_dir: Option<PathBuf>`. `Done::Status` gains:

```rust
        /// The check's answer under its selection generation, and a session a record without one
        /// learned; `None` when no check ran.
        target_check: Option<(u64, Option<TargetState>, Option<SessionRef>)>,
        /// The first refresh's resolution under the generation it was started for: the target it found and how.
        target_found: Option<(u64, Option<(Target, target::Source)>)>,
```

4. In `run_job`, after the status read and `load_rows`, before `confirmed`:

```rust
    let toplevel = response.as_ref().ok().map(|r| r.repo_root.clone()).filter(|t| !t.is_empty());
    let target_found = match (&job.resolve_target, &toplevel) {
        (Some(opener), Some(toplevel)) => Some((job.generation, resolve_target(&job, toplevel, opener.as_deref()).await)),
        _ => None,
    };
    let checked = match (&job.target, &target_found) {
        // A resolution's own check is its verification; nothing runs twice.
        (_, Some((_, Some((Target::Pane { .. }, _))))) => None,
        (Some((target @ Target::Pane { pane, .. }, generation)), _) => {
            let fresh = call_host(job.host.clone(), {
                let pane = pane.clone();
                move |h| h.pane_get(&pane)
            })
            .await;
            let checked = target::compare(target, job.socket_path.as_deref(), &fresh);
            // A session is learned only from a reply the comparison accepted as this pane, alive: a reply
            // about another pane or socket, or one the check could not read, teaches nothing.
            let adopted = match (&checked, &fresh) {
                (Some(TargetState::Live(_)), Ok(record)) => target::adopted_session(target, record),
                _ => None,
            };
            Some((*generation, checked, adopted))
        }
        _ => None,
    };
```

with, above `run_job`:

```rust
/// One host call on the blocking pool, bounded by `ENGINE_WAIT`: the engine never waits on the socket
/// longer than that, and a call that outlives it is an uncertain outcome (spec 10.6).
async fn call_host<T: Send + 'static>(
    host: Option<Arc<dyn host::HostClient>>,
    call: impl FnOnce(&dyn host::HostClient) -> Result<T, host::HostFailure> + Send + 'static,
) -> Result<T, host::HostFailure> {
    let Some(host) = host else {
        return Err(host::HostFailure::NoHost("no host: HERDR_SOCKET_PATH is unset".into()));
    };
    let handle = tokio::task::spawn_blocking(move || call(host.as_ref()));
    match tokio::time::timeout(host::ENGINE_WAIT, handle).await {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => Err(host::HostFailure::After(format!("host call failed: {e}"))),
        Err(_) => Err(host::HostFailure::After("the host did not answer in time".into())),
    }
}

/// Spec 10.2's resolution order on the first refresh: remembered, else the opener pane when it runs
/// an agent (preselected, not written), else nothing.
async fn resolve_target(job: &Job, toplevel: &str, opener: Option<&str>) -> Option<(Target, target::Source)> {
    if let Some(dir) = &job.state_dir {
        let (targets, problem) = target::load_targets(dir);
        if let Some(problem) = problem {
            base::note_problem(dir, &problem);
        }
        if let Some(target) = targets.get(toplevel) {
            return Some((target.clone(), target::Source::Remembered));
        }
    }
    let (opener, socket) = (opener?, job.socket_path.clone()?);
    if !target::is_pane_id(opener) {
        return None;
    }
    let pane = opener.to_string();
    let record = call_host(job.host.clone(), move |h| h.pane_get(&pane)).await.ok()?;
    // A reply about another pane preselects nothing: the id typed into the environment is the one asked about.
    if record.pane_id != opener {
        return None;
    }
    let agent = record.agent.clone()?;
    Some((
        Target::Pane {
            pane: record.pane_id,
            socket,
            agent,
            session: record.agent_session,
            title: record.title.unwrap_or_default(),
        },
        target::Source::Opener,
    ))
}
```

`target_checks.fetch_add(1)` where the check runs (pass the counter into `Job` as `checks: Arc<AtomicUsize>`). `target_writes_done` counts every finished target write, a pick's included, and `target_write_gate` holds every target write; both are declared here (`SessionConfig.target_write_gate: Option<Arc<Semaphore>>`, `None` in production; `EngineHandle.target_writes_waiting`, `target_writes_done: Arc<AtomicUsize>`), and Task 3's `write_target` uses the same three. The counter and `job.target` are filled in `request_status` from `self.snapshot.target` and `self.selection_generation`; `resolve_target` is `Some(self.config_opener.clone())` while `target_unresolved` (store `opener_pane`, `own_pane`, `host`, `socket_path` on `State` from the config at start).

5. On `Done::Status`, after the `acted` block:

```rust
                        if let Some((generation, found)) = target_found {
                            state.target_unresolved = false;
                            // A pick made while the first refresh ran wins over what it resolved.
                            if let (Some((target, source)), true) = (found, generation == state.selection_generation) {
                                next.target = Some(target.clone());
                                state.target_source = Some(source);
                                // Published unverified; the next refresh's check answers for it.
                                next.target_state = match (&target, &state.host) {
                                    (Target::Clipboard, _) => TargetState::Clipboard,
                                    (Target::Pane { .. }, None) => TargetState::NoHost,
                                    _ => TargetState::Unverified,
                                };
                            }
                        }
                        if let Some((generation, checked, adopted)) = target_check {
                            if generation == state.selection_generation {
                                if let Some(checked) = checked {
                                    next.target_state = checked;
                                }
                                if let (Some(session), Some(Target::Pane { session: known @ None, .. })) = (adopted, next.target.as_mut()) {
                                    // Learned from the host; written with the record's next write (a pick or a comment).
                                    *known = Some(session);
                                    state.target_write_pending = true;
                                }
                            }
                        }
                        if next.target.is_none() {
                            // 10.2's chip precedence: host absence outranks target absence.
                            next.target_state = if state.host.is_none() { TargetState::NoHost } else { TargetState::Unverified };
                        }
```

A resolved pane target is published `Unverified` and the refresh after it answers, which `a_remembered_target_beats_the_opener...` accepts (it waits for `Live`); the opener's own `pane.get` is not reused as that check, so the check path has one shape. `state.target_unresolved` is cleared by the first `Done::Status` that carries a resolution (`target_found.is_some()`), and `run_job` resolves only when the status found a toplevel, so a directory that is not a repository keeps asking until one appears (`git init` under an open viewer), at no cost: `load_targets` runs only with a toplevel to look up.

6. The commands, beside `MarkReviewed`:

```rust
                    Command::SetTarget(target) => {
                        state.selection_generation += 1;
                        state.target_source = Some(target::Source::Picked);
                        let mut next = state.snapshot.clone();
                        next.target = Some(target.clone());
                        next.target_state = match (&target, &state.host) {
                            (Target::Clipboard, _) => TargetState::Clipboard,
                            (_, None) => TargetState::NoHost,
                            _ => TargetState::Unverified,
                        };
                        publish(&mut state, next, &snapshots);
                        let toplevel = match &state.snapshot.repo {
                            RepoState::Repo { toplevel, .. } => Some(toplevel.clone()),
                            _ => None,
                        };
                        let dir = state.state_dir.clone();
                        let results = results_tx.clone();
                        let (generation, latest) = (state.selection_generation, state.latest_selection.clone());
                        let (gate, done) = (state.target_write_gate.clone(), state.target_writes_done.clone());
                        tokio::spawn(async move {
                            // The test seam shared with `write_target`: a write waits here while a test lets another land.
                            if let Some(gate) = gate {
                                gate.acquire().await.expect("gate").forget();
                            }
                            let written = match (dir, toplevel) {
                                (Some(dir), Some(toplevel)) => tokio::task::spawn_blocking(move || {
                                    // Skipped, not failed, when a newer pick landed meanwhile: that pick writes its own.
                                    target::save_target_if(&dir, &toplevel, &target, generation, &latest).map(|_| ()).map_err(|e| e.to_string())
                                })
                                .await
                                .unwrap_or_else(|e| Err(e.to_string())),
                                (None, _) => Err("no state directory".to_string()),
                                (_, None) => Err("not a git repository".to_string()),
                            };
                            done.fetch_add(1, Ordering::SeqCst);
                            let _ = results.send(Done::Target { generation, written });
                        });
                        // The new target is checked by the refresh this asks for.
                        state.request_status(&cwd, false, &results_tx, &refreshes);
                    }
                    Command::LoadPanes(token) => {
                        let toplevel = match &state.snapshot.repo {
                            RepoState::Repo { toplevel, .. } => toplevel.clone(),
                            _ => cwd.clone(),
                        };
                        let own = state.own_pane.clone();
                        let host = state.host.clone();
                        let results = results_tx.clone();
                        tokio::spawn(async move {
                            let result = match host {
                                None => Ok(Vec::new()),
                                Some(_) => call_host(host, |h| h.pane_list()).await,
                            };
                            let rows = result.map(|panes| target::rows(panes, &toplevel, own.as_deref()));
                            let _ = results.send(Done::Panes { token, rows });
                        });
                    }
```

and the two `Done` arms:

```rust
                    Done::Target { generation, written } => {
                        // An answer for an older pick is nobody's: the newer pick writes its own record and
                        // answers for itself, so neither its error nor its sequence may reach the picker.
                        if generation != state.selection_generation {
                            continue;
                        }
                        state.target_seq += 1;
                        next.target_seq = state.target_seq;
                        next.target_error = written.err().map(|e| format!("target not remembered: {e}"));
                        // The file still differs from memory: the next record write (a comment, Task 3) tries again.
                        state.target_write_pending |= next.target_error.is_some();
                        publish(&mut state, next, &snapshots);
                    }
                    Done::Panes { token, rows } => {
                        if token > next.panes_seq {
                            let (rows, error) = match rows {
                                Ok(rows) => (rows, None),
                                Err(failure) => (Vec::new(), Some(format!("could not list panes: {}", failure.message()))),
                            };
                            next.panes = Some(Arc::new(rows));
                            next.panes_error = error;
                            next.panes_seq = token;
                            publish(&mut state, next, &snapshots);
                        }
                    }
```

`Done` gains `Target { generation: u64, written: Result<(), String> }` (the selection generation the write was made under, so a stale answer is told from the current pick's) and `Panes { token: u64, rows: Result<Vec<PaneRow>, host::HostFailure> }`. A `NoHost` failure from `pane_list` with a host configured is a listing failure like any other and is reported.

7. `fingerprint` gains `s.target, s.target_state, s.target_seq, s.target_error, s.panes.as_ref().map(Arc::as_ptr), s.panes_seq, s.panes_error` (seven more `{:?}`/`{}` slots).

8. `src/tui/shell.rs::run`: `session.opener_pane = std::env::var("HERDR_HUNKS_OPENER_PANE").ok().filter(|p| engine::target::is_pane_id(p)); session.own_pane = std::env::var("HERDR_PANE_ID").ok().filter(|p| engine::target::is_pane_id(p));`. A popup viewer has no `HERDR_PANE_ID`, which is why it is `Option`.

- [ ] **Step 9: Run the session tests**

Run: `cargo test --locked --lib engine::session::tests -- --test-threads=1`
Expected: the five new tests pass with every existing one. `a_check_answered_after_a_repick_is_dropped` fails if the generation is not compared (the state reads `Restarted` from the stale check): run it once with the comparison removed to see that failure, then restore it.

- [ ] **Step 10: Gates and commit**

```bash
git add src/engine/target.rs src/engine/types.rs src/engine/session.rs src/engine/mod.rs src/tui/shell.rs
git commit -m "feat(engine): a chosen, remembered and re-verified target pane"
```

### Task 3: The comments store

Implements spec 10.3 "The record", "The anchor", "Marks and counts" (the cap), "Remembered" (the transactions, the journal, the shape checks, the expiry, the mtime reread) and 10.7's store rows; 10.9 test 3. The cards and keys are Task 7. Read 10.3 before starting.

**Files:**
- Create: `src/engine/comments.rs`
- Modify: `src/engine/types.rs` (`Snapshot`, `Command`), `src/engine/session.rs` (`State.store`, the three commands, the reread and expiry in the refresh, `Done::Comment`, `fingerprint`), `src/engine/mod.rs`

**Interfaces:**
- Consumes: Task 2's `Destination`; 0.0.4's `FileKey`, `nav::Side`, `reuse::with_lock`, `base::note_problem`, `base::is_object_id`.
- Produces:

```rust
// src/engine/comments.rs
pub const CAP: usize = 50;                 // unsent comments per worktree
pub const MAX_CHARS: usize = 4_000;
pub const MAX_LINES: usize = 100;
pub const EXPIRY: Duration = Duration::from_secs(60);
pub const NOTICE_CAP: &str = "50 comments unsent; send or delete some first.";
pub const NOTICE_LIMIT: &str = "comment limit: 4,000 characters, 100 lines";
/// The prefix of 10.7's journal notice; the session and the editor tell a journaled operation from a refused one by it.
pub const NOTICE_NOT_REMEMBERED: &str = "comments not remembered: ";
pub const NOTICE_NOTHING: &str = "nothing to send: another viewer sent these comments";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "lowercase")]
pub enum Category { Question, Change, Bug, Suggestion }
impl Category { pub fn label(self) -> &'static str /* Question, Change request, Bug, Suggestion */;
                pub fn short(self) -> &'static str /* Question, Change, Bug, Suggestion: the card and editor word */;
                pub fn instruction(self) -> &'static str; pub fn next(self) -> Self; pub fn previous(self) -> Self; }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)] #[serde(tag = "span", rename_all = "lowercase")]
pub enum Span { Line, Range { end: u32 }, File }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)] #[serde(tag = "kind", rename_all = "lowercase")]
pub enum AnchorComparison { Worktree, Branch { merge_base: String, label: String } }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor { pub key: FileKey /* path, staged, untracked */, pub side: Side, pub line: u32,
                    #[serde(flatten)] pub span: Span, pub comparison: AnchorComparison }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp { pub at: u64, pub nonce: String, pub item: u32, pub to: Destination }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)] #[serde(tag = "state", rename_all = "lowercase")]
pub enum CommentState { Pending, Sending { #[serde(flatten)] stamp: Stamp, before: Vec<Stamp> },
                        Unconfirmed { #[serde(flatten)] stamp: Stamp, before: Vec<Stamp> }, Sent(Stamp) }
// `before` on Sending and Unconfirmed is every earlier uncertain stamp the retries replaced, newest first (a chain, so
// that a third attempt's failure and the second's late failure restore the first, never `Pending`); it survives the timeout, so a
// late definite failure restores it rather than Pending. The newtype variant Sent merges the stamp's fields beside the tag.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment { pub id: String, pub anchor: Anchor, pub category: Category, pub text: String,
                     pub created_at: u64, #[serde(flatten)] pub state: CommentState }
impl Comment { pub fn is_unsent(&self) -> bool; pub fn is_pending(&self) -> bool; pub fn is_editable(&self) -> bool /* pending or unconfirmed */; pub fn stamp(&self) -> Option<&Stamp>; }

/// A journaled operation carries the record as the viewer saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation { Add(Comment), Edit { id: String, category: Category, text: String, seen: Comment }, Delete { id: String, seen: Comment } }

pub struct Store { /* state_dir: Option<PathBuf>, toplevel: String, comments: Vec<Comment>, journal: Vec<Operation>, mtime: Option<SystemTime> */ }
impl Store {
    pub fn open(state_dir: Option<PathBuf>, toplevel: &str, now: u64) -> (Store, Vec<String> /* problems */);
    pub fn comments(&self) -> &[Comment];
    pub fn journal_len(&self) -> usize;
    /// One transaction of 10.3: read, replay the journal, apply, check the cap (adds), write, keep.
    /// `Ok(Some(notice))` when the journal dropped an entry; `Err` with the notice when refused.
    pub fn transact(&mut self, op: Operation, now: u64) -> Result<Option<String>, String>;
    /// Reread when the file's mtime moved, and expire stale `Sending` records either way.
    pub fn refresh(&mut self, now: u64) -> Vec<String>;
    /// `comments.json`'s path, for tests and the TUI notice.
    pub fn path(&self) -> Option<PathBuf>;
}
pub fn new_id() -> String;                           // 32 hex: 16 of the time in nanoseconds, 16 of a hash; lexicographic order is creation order
pub fn check_text(text: &str) -> Result<String, String>;   // control characters stripped, caps enforced
pub fn line_count(text: &str) -> usize;                      // `split('\n').count()`: the one line rule
pub fn within_caps(text: &str) -> bool;
pub fn check_record(comment: &Comment) -> bool;      // every shape rule of 10.3
/// `Sending` older than EXPIRY becomes `Unconfirmed`, by nonce and only while still `Sending`.
pub fn expire(comments: &mut [Comment], now: u64) -> bool;

// requests.json
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestFile { pub key: FileKey, pub comparison: AnchorComparison, pub additions: Vec<(u32, u32)>, pub deletions: Vec<(u32, u32)> }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestRecord { pub nonce: String, pub at: u64, pub target: Destination, pub files: Vec<RequestFile> }
pub fn load_requests(state_dir: &Path, toplevel: &str) -> (Vec<RequestRecord>, Option<String>);
pub fn record_request(state_dir: &Path, toplevel: &str, record: &RequestRecord) -> std::io::Result<()>;
/// Nonces any retained record of either file carries, for 10.4's uniqueness check.
pub fn nonces_of(comments: &[Comment]) -> BTreeSet<String>;                                         // the stamps and the chains behind them; shared by every nonce check
pub fn nonces_in_use(state_dir: &Path, own: &[Comment]) -> std::io::Result<BTreeSet<String>>;   // every worktree's records in both files, plus `own`; a read error is an error

// src/engine/types.rs: Snapshot gains
pub comments: Arc<Vec<Comment>>,
pub comment_seq: u64,
pub comment_error: Option<String>,
// Snapshot also gains
/// The `token` of the add or edit this `comment_seq` answers; `None` for a delete or a refresh's notice.
pub comment_token: Option<u64>,
// Command gains
/// `token` is the TUI's own number for the save; the answer carries it back, so an editor waits for its
/// answer and no other (a notice from the refresh, or another save, advances `comment_seq` too).
AddComment { token: u64, anchor: Anchor, category: Category, text: String },
/// `seen` is the record as the viewer showed it when the key was pressed: the transaction applies the
/// change only to a record still equal to it, so an edit made elsewhere in the meantime is never overwritten.
EditComment { token: u64, seen: Comment, category: Category, text: String },
DeleteComment { seen: Comment },
```

On the serde shapes: `FileKey` and `nav::Side` gain `Serialize`/`Deserialize` derives (`Side` as `"additions"`/`"deletions"`); `Destination` is Task 2's. A `Sending` record serialises as `{"state":"sending","at":…,"nonce":…,"item":…,"to":{…},"before":[{…},…]}` (`before` omitted when empty, read as empty when absent); `Unconfirmed` and `Sent` flatten their stamp the same way. The file is `{"<toplevel>": [ <comment>, … ]}`, one array per worktree, as `marks.json` is one record per worktree.

- [ ] **Step 1: Tests first**

Create `src/engine/comments.rs` with its tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::nav::Side;

    fn anchor(path: &str, line: u32) -> Anchor {
        Anchor {
            key: FileKey { path: path.into(), staged: false, untracked: false },
            side: Side::Additions,
            line,
            span: Span::Line,
            comparison: AnchorComparison::Worktree,
        }
    }

    fn comment(id: &str, text: &str, created_at: u64) -> Comment {
        Comment {
            id: id.into(),
            anchor: anchor("src/cart.py", 16),
            category: Category::Bug,
            text: text.into(),
            created_at,
            state: CommentState::Pending,
        }
    }

    fn stamp(nonce: &str) -> Stamp {
        Stamp { at: 100, nonce: nonce.into(), item: 1, to: Destination::clipboard() }
    }

    fn open(dir: &std::path::Path) -> Store {
        Store::open(Some(dir.to_path_buf()), "/repo", 1_000).0
    }

    #[test]
    fn adds_edits_and_deletes_are_transactions_two_viewers_interleave() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = open(dir.path());
        let mut b = open(dir.path());
        a.transact(Operation::Add(comment("a1", "first", 1)), 1).unwrap();
        b.transact(Operation::Add(comment("b1", "second", 2)), 2).unwrap();
        a.transact(Operation::Add(comment("a2", "third", 3)), 3).unwrap();
        let ids: Vec<_> = a.comments().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["a1", "b1", "a2"], "a's write kept b's add");
        a.transact(Operation::Edit { id: "b1".into(), category: Category::Question, text: "why".into(), seen: comment("b1", "second", 2) }, 4).unwrap();
        b.refresh(5);
        assert_eq!(b.comments()[1].text, "why");
        assert_eq!(b.comments()[1].category, Category::Question);
        b.transact(Operation::Delete { id: "a1".into(), seen: comment("a1", "first", 1) }, 6).unwrap();
        a.refresh(7);
        assert_eq!(a.comments().len(), 2);
        // An edit of a record that is no longer editable does nothing, and says so.
        let mut sent = comment("a2", "third", 3);
        sent.state = CommentState::Sent(stamp("abc123"));
        a.transact(Operation::Edit { id: "a2".into(), category: Category::Bug, text: "x".into(), seen: sent.clone() }, 8).unwrap_err();
        assert_eq!(a.comments()[1].text, "third");
        // An unconfirmed record is still the reviewer's to edit or delete; an edit makes it pending
        // again, so a late success for the old nonce cannot mark text the host never saw as sent.
        let mut unconfirmed = comment("u1", "maybe", 9);
        unconfirmed.state = CommentState::Unconfirmed { before: Vec::new(), stamp: stamp("abc123") };
        a.transact(Operation::Add(unconfirmed.clone()), 9).unwrap();
        a.transact(Operation::Edit { id: "u1".into(), category: Category::Bug, text: "maybe not".into(), seen: unconfirmed.clone() }, 10).unwrap();
        let edited = a.comments().iter().find(|c| c.id == "u1").unwrap().clone();
        assert_eq!((edited.text.as_str(), edited.is_pending()), ("maybe not", true));
        // (Task 5's settlement test proves a late success for `abc123` then settles nothing.)
        a.transact(Operation::Delete { id: "u1".into(), seen: edited }, 11).unwrap();
        assert!(a.comments().iter().all(|c| c.id != "u1"));
    }

    #[test]
    fn the_cap_counts_unsent_records_as_the_file_holds_them() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = open(dir.path());
        let mut b = open(dir.path());
        for i in 0..49 {
            a.transact(Operation::Add(comment(&format!("c{i}"), "t", i)), 10).unwrap();
        }
        // Two viewers at 49 cannot both add.
        a.transact(Operation::Add(comment("a50", "t", 50)), 10).unwrap();
        assert_eq!(b.transact(Operation::Add(comment("b50", "t", 50)), 10).unwrap_err(), NOTICE_CAP);
        // Fifty claimed as Sending still count.
        for c in a.comments_mut_for_tests() {
            c.state = CommentState::Sending { stamp: stamp("aaaaaa"), before: Vec::new() };
        }
        a.write_for_tests();
        b.refresh(11);
        assert_eq!(b.transact(Operation::Add(comment("b51", "t", 51)), 11).unwrap_err(), NOTICE_CAP);
        // Sent records do not; a settlement never checks the cap (Task 5 proves the rollback).
        for c in a.comments_mut_for_tests() {
            c.state = CommentState::Sent(stamp("aaaaaa"));
        }
        a.write_for_tests();
        b.refresh(12);
        b.transact(Operation::Add(comment("b51", "t", 51)), 12).unwrap();
    }

    #[test]
    fn every_shape_rule_drops_one_record_and_the_next_write_rewrites_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let good = comment("good", "fine", 1);
        let mut bad = Vec::new();
        let mut c = comment("path", "x", 1); c.anchor.key.path = "../x".into(); bad.push(c);
        let mut c = comment("empty", "x", 1); c.anchor.key.path = String::new(); bad.push(c);
        let mut c = comment("range", "x", 1); c.anchor.span = Span::Range { end: 3 }; c.anchor.line = 9; bad.push(c);
        let mut c = comment("file", "x", 1); c.anchor.span = Span::File; c.anchor.line = 4; bad.push(c);
        bad.push(comment("ctrl", "a\u{1b}b", 1));
        bad.push(comment("long", &"x".repeat(MAX_CHARS + 1), 1));
        let mut c = comment("mb", "x", 1); c.anchor.comparison = AnchorComparison::Branch { merge_base: "nothex".into(), label: "main".into() }; bad.push(c);
        let mut c = comment("nonce", "x", 1); c.state = CommentState::Sent(stamp("ab")); bad.push(c);
        let mut c = comment("to", "x", 1); c.state = CommentState::Sent(Stamp { to: Destination::Pane { pane: "nope".into(), agent: "codex".into(), session: None }, ..stamp("abcdef") }); bad.push(c);
        let mut all = vec![good.clone()];
        all.extend(bad.clone());
        let mut file = serde_json::Map::new();
        file.insert("/repo".into(), serde_json::to_value(&all).unwrap());
        // A fifth category and an unknown state are not expressible as `Comment`; write them as JSON.
        let mut fifth = serde_json::to_value(&good).unwrap(); fifth["id"] = "fifth".into(); fifth["category"] = "praise".into();
        let mut unknown = serde_json::to_value(&good).unwrap(); unknown["id"] = "unknown".into(); unknown["state"] = "queued".into();
        file["/repo"].as_array_mut().unwrap().extend([fifth, unknown]);
        std::fs::write(dir.path().join("comments.json"), serde_json::to_vec(&file).unwrap()).unwrap();
        let (store, problems) = Store::open(Some(dir.path().to_path_buf()), "/repo", 1);
        assert_eq!(store.comments().iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["good"]);
        assert_eq!(problems, vec!["comments.json: 11 unusable record(s)".to_string()]);
        let mut store = store;
        store.transact(Operation::Add(comment("new", "n", 2)), 2).unwrap();
        let text = std::fs::read_to_string(dir.path().join("comments.json")).unwrap();
        assert!(!text.contains("praise") && !text.contains("queued") && !text.contains("../x"));
    }

    #[test]
    fn an_unknown_state_drops_that_record_alone() {
        let dir = tempfile::tempdir().unwrap();
        let mut keep = serde_json::to_value(comment("keep", "k", 1)).unwrap();
        keep["future_field"] = serde_json::json!({ "x": 1 });   // extra fields are ignored
        let mut drop = serde_json::to_value(comment("drop", "d", 1)).unwrap();
        drop["state"] = "replied".into();
        std::fs::write(dir.path().join("comments.json"), serde_json::json!({ "/repo": [keep, drop] }).to_string()).unwrap();
        let (store, problems) = Store::open(Some(dir.path().to_path_buf()), "/repo", 1);
        assert_eq!(store.comments().len(), 1);
        assert_eq!(store.comments()[0].id, "keep");
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn the_caps_count_characters_not_bytes() {
        let wide = "字".repeat(MAX_CHARS);           // 12,000 bytes, 4,000 chars
        assert_eq!(check_text(&wide).unwrap(), wide);
        assert_eq!(check_text(&format!("{wide}x")).unwrap_err(), NOTICE_LIMIT);
        let lines = "a\n".repeat(MAX_LINES).trim_end().to_string();
        assert!(check_text(&lines).is_ok());
        assert_eq!(check_text(&format!("{lines}\nb")).unwrap_err(), NOTICE_LIMIT);
        // A trailing newline is a 101st, empty line: the same rule the editor and the cards use.
        assert_eq!(check_text(&format!("{lines}\n")).unwrap_err(), NOTICE_LIMIT);
        assert_eq!(line_count(""), 1);
        // Control characters other than newline are removed, not refused; printable text stays.
        assert_eq!(check_text("a\u{1b}[31mb\r\nc\x7f").unwrap(), "a[31mb\nc");
    }

    #[test]
    fn sending_expires_after_sixty_seconds_on_load_and_on_refresh_without_an_mtime_change() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        let mut young = comment("young", "y", 1);
        young.state = CommentState::Sending { stamp: Stamp { at: 1_010, ..stamp("aaaaaa") }, before: Vec::new() };
        let mut old = comment("old", "o", 1);
        old.state = CommentState::Sending { stamp: Stamp { at: 950, ..stamp("bbbbbb") }, before: vec![stamp("cccccc")] };
        // Neither is sixty seconds old at 1_000, so the adds write them as they are.
        store.transact(Operation::Add(young), 1_000).unwrap();
        store.transact(Operation::Add(old), 1_000).unwrap();
        // On load at 1_061: `old` (claimed at 950) expires, `young` (1_010) does not.
        let (loaded, _) = Store::open(Some(dir.path().to_path_buf()), "/repo", 1_061);
        assert!(matches!(&loaded.comments()[0].state, CommentState::Sending { .. }));
        assert!(matches!(&loaded.comments()[1].state, CommentState::Unconfirmed { stamp: s, .. } if s.nonce == "bbbbbb"));
        // In a refresh whose mtime did not move: `young` expires once its 60 s pass.
        let mut store = loaded;
        let before = std::fs::metadata(dir.path().join("comments.json")).unwrap().modified().unwrap();
        store.refresh(1_071);
        assert!(matches!(&store.comments()[0].state, CommentState::Unconfirmed { stamp: s, .. } if s.nonce == "aaaaaa"));
        assert!(std::fs::metadata(dir.path().join("comments.json")).unwrap().modified().unwrap() >= before);
        // Expiry writes through, so another viewer reads it.
        let (other, _) = Store::open(Some(dir.path().to_path_buf()), "/repo", 1_071);
        assert!(other.comments().iter().all(|c| matches!(c.state, CommentState::Unconfirmed { .. })));
    }

    #[test]
    fn a_failed_expiry_write_on_load_still_shows_the_loaded_comments() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        store.transact(Operation::Add(comment("fine", "kept", 1)), 1_000).unwrap();
        let mut stale = comment("stale", "x", 2);
        // Claimed fifty seconds ago: not expired when added, expired when reopened a minute later.
        stale.state = CommentState::Sending { stamp: Stamp { at: 950, ..stamp("aaaaaa") }, before: Vec::new() };
        store.transact(Operation::Add(stale), 1_000).unwrap();
        assert!(matches!(store.comments()[1].state, CommentState::Sending { .. }));
        // Opened later, the expiry must be written; when it cannot be, the records are still shown.
        let mut reopened = Store::open_for_tests_with_failing_writes(dir.path(), "/repo", 1_100);
        assert_eq!(reopened.comments().len(), 2, "a failed expiry write hid the loaded comments");
        assert!(matches!(reopened.comments()[1].state, CommentState::Sending { .. }), "shown as the file holds it");
        reopened.fail_writes_for_tests(false);
        reopened.refresh(1_101);
        assert!(matches!(reopened.comments()[1].state, CommentState::Unconfirmed { .. }));
    }

    #[test]
    fn a_moved_mtime_is_reread_and_a_still_one_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = open(dir.path());
        let mut b = open(dir.path());
        a.transact(Operation::Add(comment("a1", "x", 1)), 1).unwrap();
        assert!(b.refresh(2).is_empty());
        assert_eq!(b.comments().len(), 1);
        let reads = b.reads_for_tests();
        b.refresh(3);
        assert_eq!(b.reads_for_tests(), reads, "an unchanged file was read again");
    }

    #[test]
    fn without_a_state_directory_the_array_is_the_store() {
        let (mut store, problems) = Store::open(None, "/repo", 1);
        assert!(problems.is_empty());
        assert_eq!(store.transact(Operation::Add(comment("a", "x", 1)), 1).unwrap_err(), "comments not remembered: no state directory");
        assert_eq!(store.comments().len(), 1, "the comment is held all the same");
        store.transact(Operation::Add(comment("b", "x", 2)), 2).unwrap();
        assert_eq!(store.comments().len(), 2);
        assert_eq!(store.journal_len(), 0, "there is nothing to journal: nothing could ever be written");
        // A stale edit (the record changed since) is refused here as it is on disk.
        let seen_then = comment("a", "x", 1);
        store.transact(Operation::Edit { id: "a".into(), category: Category::Bug, text: "first edit".into(), seen: seen_then.clone() }, 3).unwrap();
        assert_eq!(store.transact(Operation::Edit { id: "a".into(), category: Category::Bug, text: "stale".into(), seen: seen_then }, 4).unwrap_err(), "No comment selected.");
        assert_eq!(store.comments()[0].text, "first edit");
    }

    #[test]
    fn a_failing_store_journals_and_replays_without_undoing_another_viewers_work() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = open(dir.path());
        let mut b = open(dir.path());
        a.transact(Operation::Add(comment("a1", "mine", 1)), 1).unwrap();
        b.refresh(2);
        // The directory becomes unwritable for a: its operations go to the journal.
        a.fail_writes_for_tests(true);
        let notice = a.transact(Operation::Add(comment("a2", "later", 3)), 3).unwrap_err();
        assert!(notice.starts_with("comments not remembered: "));
        a.transact(Operation::Edit { id: "a1".into(), category: Category::Bug, text: "mine, edited".into(), seen: comment("a1", "mine", 1) }, 4).unwrap_err();
        assert_eq!(a.journal_len(), 2);
        assert_eq!(a.comments().len(), 2, "the earlier unsaved add is still on screen after a second failed write");
        assert!(a.comments().iter().any(|c| c.id == "a2" && c.text == "later"));
        // Meanwhile b edits a1 and adds b1.
        b.transact(Operation::Edit { id: "a1".into(), category: Category::Question, text: "b's edit".into(), seen: comment("a1", "mine", 1) }, 5).unwrap();
        b.transact(Operation::Add(comment("b1", "b", 6)), 6).unwrap();
        // a can write again: the replay keeps b's edit (a's edit saw an older record), keeps a2, keeps b1.
        a.fail_writes_for_tests(false);
        let dropped = a.transact(Operation::Add(comment("a3", "third", 7)), 7).unwrap();
        assert_eq!(dropped.as_deref(), Some("a comment changed under you; your edit was dropped"));
        assert_eq!(a.journal_len(), 0);
        let texts: Vec<_> = a.comments().iter().map(|c| (c.id.as_str(), c.text.as_str())).collect();
        // The replayed add lands after what the file held, b1 included.
        assert_eq!(texts, [("a1", "b's edit"), ("b1", "b"), ("a2", "later"), ("a3", "third")]);
        b.refresh(8);
        assert_eq!(b.comments().len(), 4);
    }

    #[test]
    fn a_journaled_add_the_cap_refuses_stays_visible_and_journaled() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = open(dir.path());
        let mut b = open(dir.path());
        a.fail_writes_for_tests(true);
        a.transact(Operation::Add(comment("mine", "unsaved", 1)), 1_001).unwrap_err();
        for i in 0..50 {
            b.transact(Operation::Add(comment(&format!("b{i}"), "t", 2)), 1_002).unwrap();
        }
        a.fail_writes_for_tests(false);
        // The replay meets the cap: the add stays journaled, and stays on a's screen.
        let _ = a.refresh(1_003);
        assert_eq!(a.journal_len(), 1);
        assert!(a.comments().iter().any(|c| c.id == "mine"));
        assert_eq!(a.comments().len(), 51);
        // Deleting it clears the journal entry rather than journaling a deletion of a record the file never had.
        a.transact(Operation::Delete { id: "mine".into(), seen: comment("mine", "unsaved", 1) }, 1_004).unwrap();
        assert_eq!(a.journal_len(), 0);
        assert!(a.comments().iter().all(|c| c.id != "mine"));
    }

    #[test]
    fn a_malformed_record_met_by_a_transaction_is_logged_before_it_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = open(dir.path());
        a.transact(Operation::Add(comment("a1", "x", 1)), 1).unwrap();
        // A newer version's record lands in the file after the store opened: an unknown state.
        let mut file: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("comments.json")).unwrap()).unwrap();
        let mut odd = serde_json::to_value(comment("odd", "y", 2)).unwrap();
        odd["state"] = "resolved".into();
        file["/repo"].as_array_mut().unwrap().push(odd);
        std::fs::write(dir.path().join("comments.json"), file.to_string()).unwrap();
        // The next transaction rewrites the array without it, and the log says so before the write.
        a.transact(Operation::Add(comment("a2", "z", 3)), 3).unwrap();
        let written = std::fs::read_to_string(dir.path().join("comments.json")).unwrap();
        assert!(!written.contains("resolved"));
        let log = std::fs::read_to_string(dir.path().join("config-problems.log")).unwrap();
        assert!(log.contains("comments.json: 1 unusable record(s); rewritten without them"), "{log}");
        // The same for a request record met by the next request write.
        let bad = serde_json::json!({ "/repo": [{ "nonce": "x", "at": 1, "target": { "clipboard": true }, "files": [] }] });
        std::fs::write(dir.path().join("requests.json"), bad.to_string()).unwrap();
        record_request(dir.path(), "/repo", &RequestRecord { nonce: "abcdef".into(), at: 2, target: Destination::clipboard(), files: Vec::new() }).unwrap();
        assert_eq!(load_requests(dir.path(), "/repo").0.len(), 1);
        let log = std::fs::read_to_string(dir.path().join("config-problems.log")).unwrap();
        assert!(log.contains("requests.json: 1 unusable record(s); rewritten without them"), "{log}");
    }

    #[test]
    fn a_failing_store_refuses_what_the_rules_refuse_instead_of_journaling_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = open(dir.path());
        let mut b = open(dir.path());
        for i in 0..50 {
            b.transact(Operation::Add(comment(&format!("b{i}"), "t", 1)), 1_001).unwrap();
        }
        a.refresh(1_002);
        a.fail_writes_for_tests(true);
        // The cap is already met in a's own array: the add is refused, not remembered for later.
        assert_eq!(a.transact(Operation::Add(comment("mine", "x", 2)), 1_003).unwrap_err(), NOTICE_CAP);
        assert_eq!(a.journal_len(), 0);
        assert_eq!(a.comments().len(), 50);
        // An edit whose record changed under it is refused the same way.
        let mut changed = comment("b0", "t", 1);
        changed.text = "not what a saw".into();
        assert_eq!(a.transact(Operation::Edit { id: "b0".into(), category: Category::Bug, text: "mine".into(), seen: changed }, 1_004).unwrap_err(), "No comment selected.");
        assert_eq!(a.journal_len(), 0);
        // An edit that passes is journaled, with the usual notice.
        let notice = a.transact(Operation::Edit { id: "b0".into(), category: Category::Bug, text: "mine".into(), seen: comment("b0", "t", 1) }, 1_005).unwrap_err();
        assert!(notice.starts_with(NOTICE_NOT_REMEMBERED));
        assert_eq!(a.journal_len(), 1);
    }

    #[test]
    fn a_journaled_add_stays_on_screen_through_a_failed_edit_on_a_full_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = open(dir.path());
        let mut b = open(dir.path());
        a.fail_writes_for_tests(true);
        a.transact(Operation::Add(comment("mine", "unsaved", 1)), 1_001).unwrap_err();
        for i in 0..50 {
            b.transact(Operation::Add(comment(&format!("b{i}"), "t", 2)), 1_002).unwrap();
        }
        // a's edit of b0 passes the rules (the record is as a saw it after a reload) and fails to write:
        // the add refused by the full file is still on a's screen, and both are journaled.
        let _ = a.refresh(1_003);
        assert!(a.comments().iter().any(|c| c.id == "mine"));
        let notice = a.transact(Operation::Edit { id: "b0".into(), category: Category::Bug, text: "mine too".into(), seen: comment("b0", "t", 2) }, 1_004).unwrap_err();
        assert!(notice.starts_with(NOTICE_NOT_REMEMBERED));
        assert_eq!(a.journal_len(), 2);
        assert!(a.comments().iter().any(|c| c.id == "mine"), "the journaled add vanished behind a failed edit");
        assert_eq!(a.comments().iter().find(|c| c.id == "b0").map(|c| c.text.as_str()), Some("mine too"));
        // Further failed refreshes keep it; a successful one replays the edit, keeps the add journaled and shown.
        let _ = a.refresh(1_005);
        assert!(a.comments().iter().any(|c| c.id == "mine"));
        a.fail_writes_for_tests(false);
        let _ = a.refresh(1_006);
        assert_eq!(a.journal_len(), 1, "the edit was written; the add still waits for room");
        assert!(a.comments().iter().any(|c| c.id == "mine"));
        b.refresh(1_007);
        assert_eq!(b.comments().iter().find(|c| c.id == "b0").map(|c| c.text.as_str()), Some("mine too"));
    }

    #[test]
    fn a_journaled_deletion_of_a_record_since_sent_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = open(dir.path());
        let mut b = open(dir.path());
        a.transact(Operation::Add(comment("a1", "x", 1)), 1).unwrap();
        b.refresh(2);
        a.fail_writes_for_tests(true);
        a.transact(Operation::Delete { id: "a1".into(), seen: comment("a1", "x", 1) }, 3).unwrap_err();
        let mut sent = comment("a1", "x", 1);
        sent.state = CommentState::Sent(stamp("zzzzzz"));
        b.set_for_tests(vec![sent]);
        a.fail_writes_for_tests(false);
        let dropped = a.transact(Operation::Add(comment("a2", "y", 4)), 4).unwrap();
        assert_eq!(dropped.as_deref(), Some("a comment changed under you; your deletion was dropped"));
        assert!(matches!(a.comments()[0].state, CommentState::Sent(_)));
    }

    #[test]
    fn ids_are_32_hex_and_sort_in_creation_order_within_a_second() {
        let ids: Vec<String> = (0..50).map(|_| new_id()).collect();
        for id in &ids {
            assert!(id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "{id}");
            assert!(check_record(&Comment { id: id.clone(), ..comment("x", "t", 1) }));
        }
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(sorted, ids, "later ids sort later");
        assert_eq!(ids.iter().collect::<BTreeSet<_>>().len(), 50, "all distinct");
        // Task 5's `a_claim_takes_pending_and_unconfirmed_in_order_and_never_anothers_sending` relies on
        // this order for two comments made in the same second.
    }

    #[test]
    fn requests_are_recorded_and_nonces_in_use_are_known() {
        let dir = tempfile::tempdir().unwrap();
        let record = RequestRecord {
            nonce: "wvpx71".into(),
            at: 1,
            target: Destination::Pane { pane: "w4:p2".into(), agent: "codex".into(), session: None },
            files: vec![RequestFile {
                key: FileKey { path: "src/cart.py".into(), staged: false, untracked: false },
                comparison: AnchorComparison::Worktree,
                additions: vec![(10, 12), (30, 31)],
                deletions: vec![],
            }],
        };
        record_request(dir.path(), "/repo", &record).unwrap();
        let (loaded, problem) = load_requests(dir.path(), "/repo");
        assert_eq!((loaded, problem), (vec![record.clone()], None));
        let mut sent = comment("c", "x", 1);
        sent.state = CommentState::Sent(stamp("oqzpww"));
        let used = nonces_in_use(dir.path(), &[sent.clone()]).unwrap();
        assert_eq!(used, ["oqzpww", "wvpx71"].into_iter().map(String::from).collect::<BTreeSet<_>>());
        // Another worktree's records count too: two worktrees can share one agent pane.
        let mut elsewhere = Store::open(Some(dir.path().to_path_buf()), "/other", 1_000).0;
        let mut theirs = comment("t", "x", 1);
        theirs.state = CommentState::Sent(stamp("zzzzz9"));
        elsewhere.transact(Operation::Add(theirs), 1_001).unwrap();
        assert!(nonces_in_use(dir.path(), &[]).unwrap().contains("zzzzz9"));
        // An unreadable file is an error, not an empty set: a blind nonce could collide with what it holds.
        let unreadable = tempfile::tempdir().unwrap();
        std::fs::create_dir(unreadable.path().join("comments.json")).unwrap();   // a directory where a file should be
        assert!(nonces_in_use(unreadable.path(), &[]).is_err());
        // A fresh request reads the comments file under the lock, not a captured array.
        let mut store = open(dir.path());
        store.transact(Operation::Add(sent), 1_001).unwrap();
        let mut counter = 0;
        let make = |c: u64| if c == 1 { "oqzpww".to_string() } else { format!("r{c:05}") };
        let (nonce, text) = record_request_fresh(dir.path(), "/repo", &make, &mut counter, &|| Ok(()), |nonce| Ok((RequestRecord { nonce: nonce.into(), ..record.clone() }, format!("text for {nonce}")))).unwrap();
        assert_eq!((nonce.as_str(), text.as_str(), counter), ("r00002", "text for r00002", 2), "the nonce a comment carries was passed over");
        assert!(record_request_fresh(dir.path(), "/repo", &make, &mut counter, &|| Err("changed".into()), |nonce| Ok((RequestRecord { nonce: nonce.into(), ..record.clone() }, String::new()))).is_err());
        assert_eq!(load_requests(dir.path(), "/repo").0.len(), 2, "a refused guard records nothing");
        assert!(record_request_fresh(dir.path(), "/repo", &make, &mut counter, &|| Ok(()), |_| Err("review too large".to_string())).is_err());
        assert_eq!(load_requests(dir.path(), "/repo").0.len(), 2, "a refused build records nothing");
        assert!(record_request_fresh(unreadable.path(), "/repo", &make, &mut counter, &|| Ok(()), |nonce| Ok((RequestRecord { nonce: nonce.into(), ..record.clone() }, String::new()))).is_err(), "no nonce is chosen blind");
        // A malformed record is dropped alone.
        let mut bad = serde_json::to_value(&record).unwrap();
        bad["nonce"] = "x".into();
        let file = serde_json::json!({ "/repo": [serde_json::to_value(&record).unwrap(), bad] });
        std::fs::write(dir.path().join("requests.json"), file.to_string()).unwrap();
        let (loaded, problem) = load_requests(dir.path(), "/repo");
        assert_eq!(loaded.len(), 1);
        assert_eq!(problem.as_deref(), Some("requests.json: 1 unusable record(s)"));
        // Another worktree's entry that is no list costs nobody anything: this worktree still reads and
        // writes its own, and the odd entry is written back as it was.
        let file = serde_json::json!({ "/repo": [serde_json::to_value(&record).unwrap()], "/other": "garbage", "/third": [{ "nonce": "abcdef", "at": 1, "target": { "clipboard": true }, "files": [] }] });
        std::fs::write(dir.path().join("requests.json"), file.to_string()).unwrap();
        assert_eq!(load_requests(dir.path(), "/repo").0.len(), 1);
        assert_eq!(load_requests(dir.path(), "/other").1.as_deref(), Some("requests.json: this worktree's entry is not a list"));
        record_request(dir.path(), "/repo", &RequestRecord { nonce: "zz9zz9".into(), ..record.clone() }).unwrap();
        assert_eq!(load_requests(dir.path(), "/repo").0.len(), 2);
        let written: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("requests.json")).unwrap()).unwrap();
        assert_eq!(written["/other"], "garbage");
        assert_eq!(written["/third"].as_array().map(Vec::len), Some(1), "the valid neighbour kept its record");
        assert!(nonces_in_use(dir.path(), &[]).unwrap().contains("abcdef"), "its nonce still counts");
        remove_request(dir.path(), "/repo", "zz9zz9").unwrap();
        let written: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("requests.json")).unwrap()).unwrap();
        assert_eq!(written["/third"].as_array().map(Vec::len), Some(1));
        // The shape rules of 10.7, one record each: a path with `..`, a merge-base that is no object id,
        // a label with a control character, a zero-based range, an inverted range, a bad nonce.
        let file_of = |f: RequestFile| RequestRecord { files: vec![f], ..record.clone() };
        let ok_file = record.files[0].clone();
        let bad: Vec<RequestRecord> = vec![
            file_of(RequestFile { key: FileKey { path: "../etc/passwd".into(), staged: false, untracked: false }, ..ok_file.clone() }),
            file_of(RequestFile { key: FileKey { path: "/abs".into(), staged: false, untracked: false }, ..ok_file.clone() }),
            file_of(RequestFile { comparison: AnchorComparison::Branch { merge_base: "main".into(), label: "main".into() }, ..ok_file.clone() }),
            file_of(RequestFile { comparison: AnchorComparison::Branch { merge_base: "0123456789abcdef0123456789abcdef01234567".into(), label: "ma\u{1b}in".into() }, ..ok_file.clone() }),
            file_of(RequestFile { additions: vec![(0, 3)], ..ok_file.clone() }),
            file_of(RequestFile { deletions: vec![(5, 2)], ..ok_file.clone() }),
            RequestRecord { nonce: "no spaces!".into(), ..record.clone() },
        ];
        let good = file_of(RequestFile { comparison: AnchorComparison::Branch { merge_base: "0123456789abcdef0123456789abcdef01234567".into(), label: "origin/main".into() }, ..ok_file.clone() });
        let mut entries: Vec<serde_json::Value> = bad.iter().map(|r| serde_json::to_value(r).unwrap()).collect();
        entries.push(serde_json::to_value(&good).unwrap());
        std::fs::write(dir.path().join("requests.json"), serde_json::json!({ "/repo": entries }).to_string()).unwrap();
        let (loaded, problem) = load_requests(dir.path(), "/repo");
        assert_eq!(loaded, vec![good]);
        assert_eq!(problem.as_deref(), Some("requests.json: 7 unusable record(s)"));
        // A file that is no object at all: nothing is read, the problem says so, and the next write replaces it.
        std::fs::write(dir.path().join("requests.json"), "[1, 2]").unwrap();
        let (loaded, problem) = load_requests(dir.path(), "/repo");
        assert!(loaded.is_empty());
        assert!(problem.as_deref().is_some_and(|p| p.starts_with("requests.json: not a JSON object")));
        record_request(dir.path(), "/repo", &record).unwrap();
        assert_eq!(load_requests(dir.path(), "/repo"), (vec![record.clone()], None));
    }

    #[test]
    fn the_wire_shape_is_stable() {
        let mut c = comment("0123456789abcdef0123456789abcdef", "a\nb", 7);
        c.state = CommentState::Sending {
            stamp: Stamp { at: 9, nonce: "abc123".into(), item: 2, to: Destination::Pane { pane: "w4:p2".into(), agent: "codex".into(), session: Some(crate::engine::host::SessionRef { kind: "id".into(), value: "s".into() }) } },
            before: vec![Stamp { at: 3, nonce: "zzz999".into(), item: 1, to: Destination::clipboard() }],
        };
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(json["state"], "sending");
        assert_eq!(json["nonce"], "abc123");
        assert_eq!(json["to"]["pane"], "w4:p2");
        assert_eq!(json["before"][0]["to"]["clipboard"], true);
        let mut plain = comment("0123456789abcdef0123456789abcdef", "x", 1);
        plain.state = CommentState::Unconfirmed { stamp: stamp("abc123"), before: Vec::new() };
        let value = serde_json::to_value(&plain).unwrap();
        assert!(value.get("before").is_none(), "an empty chain is not written");
        let back: Comment = serde_json::from_value(value).unwrap();
        assert!(matches!(back.state, CommentState::Unconfirmed { before, .. } if before.is_empty()), "and reads back as empty");
        assert_eq!(json["anchor"]["side"], "additions");
        assert_eq!(json["anchor"]["span"], "line");
        assert_eq!(json["category"], "bug");
        let back: Comment = serde_json::from_value(json).unwrap();
        assert_eq!(back, c);
    }
}
```

The test hooks `comments_mut_for_tests`, `write_for_tests`, `reads_for_tests`, `fail_writes_for_tests`, `set_for_tests` are `#[cfg(test)]` methods on `Store` (Step 3). `set_for_tests` writes the given array as the file and takes it as the store's own.

- [ ] **Step 2: Run to watch them fail**

Add `pub mod comments;` to `src/engine/mod.rs` first. Run: `cargo test --locked --lib engine::comments`
Expected: compile errors.

- [ ] **Step 3: The module**

```rust
//! The comments of a review (spec 10.3): records beside the diff, shared through comments.json.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use super::base;
use super::nav::Side;
use super::target::Destination;
use super::FileKey;
use crate::actions::reuse;

pub const CAP: usize = 50;
pub const MAX_CHARS: usize = 4_000;
pub const MAX_LINES: usize = 100;
pub const EXPIRY: Duration = Duration::from_secs(60);
pub const NOTICE_CAP: &str = "50 comments unsent; send or delete some first.";
pub const NOTICE_LIMIT: &str = "comment limit: 4,000 characters, 100 lines";
/// The prefix of 10.7's journal notice; the session and the editor tell a journaled operation from a refused one by it.
pub const NOTICE_NOT_REMEMBERED: &str = "comments not remembered: ";
/// A claim that found every eligible record taken by another viewer (spec 10.4 step 3).
pub const NOTICE_NOTHING: &str = "nothing to send: another viewer sent these comments";
const COMMENTS_FILE: &str = "comments.json";
const REQUESTS_FILE: &str = "requests.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Question,
    Change,
    Bug,
    Suggestion,
}

impl Category {
    pub const ALL: [Category; 4] = [Self::Question, Self::Change, Self::Bug, Self::Suggestion];

    /// The prompt's label (spec 10.5).
    pub fn label(self) -> &'static str {
        match self {
            Self::Question => "Question",
            Self::Change => "Change request",
            Self::Bug => "Bug",
            Self::Suggestion => "Suggestion",
        }
    }

    /// The card's and the editor's word.
    pub fn short(self) -> &'static str {
        match self {
            Self::Question => "Question",
            Self::Change => "Change",
            Self::Bug => "Bug",
            Self::Suggestion => "Suggestion",
        }
    }

    pub fn instruction(self) -> &'static str {
        match self {
            Self::Question => "Answer inline in your reply. Do not edit files.",
            Self::Change => "Make this change.",
            Self::Bug => "Fix this.",
            Self::Suggestion => "Apply this if you agree.",
        }
    }

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|c| *c == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub fn previous(self) -> Self {
        let i = Self::ALL.iter().position(|c| *c == self).unwrap_or(0);
        Self::ALL[(i + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "span", rename_all = "lowercase")]
pub enum Span {
    Line,
    Range { end: u32 },
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum AnchorComparison {
    Worktree,
    Branch { merge_base: String, label: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    pub key: FileKey,
    pub side: Side,
    pub line: u32,
    #[serde(flatten)]
    pub span: Span,
    pub comparison: AnchorComparison,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    pub at: u64,
    pub nonce: String,
    pub item: u32,
    pub to: Destination,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum CommentState {
    Pending,
    Sending {
        #[serde(flatten)]
        stamp: Stamp,
        /// The uncertain stamps this claim replaced, newest first: the `Unconfirmed` one it found and
        /// that one's own `before`. A definite failure restores the first of them, with the rest; a
        /// chain, because a third attempt's failure and the second's late failure must land on the
        /// first, which may have arrived, and never on `Pending`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        before: Vec<Stamp>,
    },
    /// Kept through the timeout that made it: a late definite failure restores `before`'s first,
    /// because that failure says nothing about the earlier sends that may have arrived.
    Unconfirmed {
        #[serde(flatten)]
        stamp: Stamp,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        before: Vec<Stamp>,
    },
    // A newtype variant of an internally tagged enum serialises its struct's fields beside the tag.
    Sent(Stamp),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    pub anchor: Anchor,
    pub category: Category,
    pub text: String,
    pub created_at: u64,
    /// Flattened: `"state":"sending"` and the stamp's fields sit beside `id` and `text`.
    #[serde(flatten)]
    pub state: CommentState,
}

impl Comment {
    /// Pending, sending or unconfirmed: what the cap counts.
    pub fn is_unsent(&self) -> bool {
        !matches!(self.state, CommentState::Sent(_))
    }

    pub fn is_pending(&self) -> bool {
        matches!(self.state, CommentState::Pending)
    }

    /// Pending or unconfirmed: what `u` and `x` may change (spec 10.3; sent comments are never edited).
    pub fn is_editable(&self) -> bool {
        matches!(self.state, CommentState::Pending | CommentState::Unconfirmed { .. })
    }

    pub fn stamp(&self) -> Option<&Stamp> {
        match &self.state {
            CommentState::Pending => None,
            CommentState::Sending { stamp, .. } | CommentState::Unconfirmed { stamp, .. } => Some(stamp),
            CommentState::Sent(s) => Some(s),
        }
    }
}

/// A random 32-hex id; once sent it is also the thread id (spec 10.3).
pub fn new_id() -> String {
    use sha2::Digest;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // The first half is the time, so ids sort in creation order within one second (`created_at` is in
    // seconds, and a claim orders by `(created_at, id)`); it is made strictly increasing within this
    // process whatever the clock's granularity. The second half keeps two viewers apart. Still 32
    // lowercase hex, which is all the shape rule and the thread id ask for.
    static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut stamp = nanos as u64;
    loop {
        let last = LAST.load(std::sync::atomic::Ordering::SeqCst);
        if stamp <= last {
            stamp = last + 1;
        }
        if LAST.compare_exchange(last, stamp, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst).is_ok() {
            break;
        }
    }
    let mut hasher = sha2::Sha256::new();
    hasher.update(stamp.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hasher.update(counter.to_le_bytes());
    let digest = hasher.finalize();
    let tail: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("{stamp:016x}{tail}")
}

/// The one line count of this section: a line is what `split('\n')` yields, so a trailing newline
/// opens a 101st line, as the editor and the cards show it.
pub fn line_count(text: &str) -> usize {
    text.split('\n').count()
}

/// Within both caps, by characters and by `line_count`.
pub fn within_caps(text: &str) -> bool {
    text.chars().count() <= MAX_CHARS && line_count(text) <= MAX_LINES
}

/// Control characters other than newline removed; then the caps.
pub fn check_text(text: &str) -> Result<String, String> {
    let clean: String = text
        .chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .collect();
    if !within_caps(&clean) {
        return Err(NOTICE_LIMIT.to_string());
    }
    Ok(clean)
}

fn is_nonce(text: &str) -> bool {
    (6..=16).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn stamp_ok(stamp: &Stamp) -> bool {
    is_nonce(&stamp.nonce) && stamp.item >= 1 && stamp.to.is_well_formed()
}

/// Every shape rule of spec 10.3 "Remembered".
/// A repository-relative path with no `..` component: the one shape rule for a path in either file.
fn path_ok(path: &str) -> bool {
    let p = Path::new(path);
    !path.is_empty()
        && !path.contains('\0')
        && p.is_relative()
        && !p.components().any(|c| matches!(c, std::path::Component::ParentDir))
}

/// `Worktree`, or a `Branch` whose merge-base is an object id and whose label is printable.
fn comparison_ok(comparison: &AnchorComparison) -> bool {
    match comparison {
        AnchorComparison::Worktree => true,
        AnchorComparison::Branch { merge_base, label } => base::is_object_id(merge_base) && super::target::printable(label),
    }
}

pub fn check_record(comment: &Comment) -> bool {
    let path_valid = path_ok(&comment.anchor.key.path);
    let span_ok = match comment.anchor.span {
        Span::Line => comment.anchor.line >= 1,
        Span::Range { end } => comment.anchor.line >= 1 && end >= comment.anchor.line,
        Span::File => comment.anchor.line == 0 && comment.anchor.side == Side::Additions,
    };
    let comparison_valid = comparison_ok(&comment.anchor.comparison);
    let text_ok = comment.text.chars().all(|c| c == '\n' || !c.is_control()) && within_caps(&comment.text);
    // Production ids are 32 hex characters; tests name comments `a1`, so the shape is alphanumeric.
    let id_ok = (comment.id.len() == 32 && comment.id.bytes().all(|b| b.is_ascii_hexdigit()))
        || (!comment.id.is_empty() && comment.id.len() <= 32 && comment.id.bytes().all(|b| b.is_ascii_alphanumeric()));
    let state_ok = match &comment.state {
        CommentState::Pending => true,
        CommentState::Sending { stamp, before } | CommentState::Unconfirmed { stamp, before } => {
            stamp_ok(stamp) && before.iter().all(stamp_ok)
        }
        CommentState::Sent(s) => stamp_ok(s),
    };
    path_valid && span_ok && comparison_valid && text_ok && id_ok && state_ok
}

/// `Sending` older than `EXPIRY` becomes `Unconfirmed`, by nonce and only while still `Sending`.
pub fn expire(comments: &mut [Comment], now: u64) -> bool {
    let mut changed = false;
    for comment in comments {
        if let CommentState::Sending { stamp, before } = &comment.state {
            if now.saturating_sub(stamp.at) >= EXPIRY.as_secs() {
                comment.state = CommentState::Unconfirmed { stamp: stamp.clone(), before: before.clone() };
                changed = true;
            }
        }
    }
    changed
}

/// A journaled operation carries the record as the viewer saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    Add(Comment),
    Edit {
        id: String,
        category: Category,
        text: String,
        seen: Comment,
    },
    Delete {
        id: String,
        seen: Comment,
    },
}

impl Operation {
    fn is_add(&self) -> bool {
        matches!(self, Self::Add(_))
    }

    fn describe(&self) -> &'static str {
        match self {
            Self::Add(_) => "add",
            Self::Edit { .. } => "edit",
            Self::Delete { .. } => "deletion",
        }
    }
}

/// Why a transaction did not go through: refused by a rule, or the file could not be read or written.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TxError {
    Refused(String),
    Io(String),
}

/// Applies one operation to an array read from the file. `Err` is a refusal; `Ok(false)` means the
/// operation met a record that changed under it and was dropped (the other viewer's version stays).
fn apply(comments: &mut Vec<Comment>, op: &Operation) -> Result<bool, String> {
    match op {
        Operation::Add(comment) => {
            if comments.iter().any(|c| c.id == comment.id) {
                return Ok(true);
            }
            if comments.iter().filter(|c| c.is_unsent()).count() >= CAP {
                return Err(NOTICE_CAP.to_string());
            }
            comments.push(comment.clone());
            Ok(true)
        }
        Operation::Edit { id, category, text, seen } => {
            let Some(current) = comments.iter_mut().find(|c| &c.id == id) else {
                return Ok(false);
            };
            // Pending and unconfirmed records may change; a sending or sent one never does.
            if current != seen || !current.is_editable() {
                return Ok(false);
            }
            current.category = *category;
            current.text = text.clone();
            // An edited unconfirmed comment is pending again: the old text is what may have arrived,
            // and a late success for that nonce must not mark the new text sent. The next finish sends
            // the new text under a new nonce; the agent, if it has the old one, sees the difference.
            current.state = CommentState::Pending;
            Ok(true)
        }
        Operation::Delete { id, seen } => {
            let Some(index) = comments.iter().position(|c| &c.id == id) else {
                return Ok(false);
            };
            if &comments[index] != seen || !comments[index].is_editable() {
                return Ok(false);
            }
            comments.remove(index);
            Ok(true)
        }
    }
}

fn read_file(path: &Path) -> Result<Option<(String, SystemTime)>, String> {
    match std::fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.file_name().unwrap_or_default().to_string_lossy())),
        Ok(text) => {
            let mtime = std::fs::metadata(path)
                .and_then(|m| m.modified())
                .map_err(|e| e.to_string())?;
            Ok(Some((text, mtime)))
        }
    }
}

/// Every worktree's array, this one's checked by shape; `(all, mine, problem)`.
fn parse_comments(text: &str, toplevel: &str) -> (BTreeMap<String, serde_json::Value>, Vec<Comment>, Option<String>) {
    let mut all: BTreeMap<String, serde_json::Value> = match serde_json::from_str(text) {
        Ok(all) => all,
        Err(e) => return (BTreeMap::new(), Vec::new(), Some(format!("{COMMENTS_FILE}: {e}"))),
    };
    let raw = all.remove(toplevel).and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let total = raw.len();
    let mine: Vec<Comment> = raw
        .into_iter()
        .filter_map(|v| serde_json::from_value::<Comment>(v).ok())
        .filter(check_record)
        .collect();
    let dropped = total - mine.len();
    let problem = (dropped > 0).then(|| format!("{COMMENTS_FILE}: {dropped} unusable record(s)"));
    (all, mine, problem)
}

pub struct Store {
    state_dir: Option<PathBuf>,
    toplevel: String,
    comments: Vec<Comment>,
    journal: Vec<Operation>,
    /// Why the journal is not empty, for the refusal to claim (spec 10.4).
    journal_reason: Option<String>,
    /// The other worktrees' arrays as last read, written back unchanged.
    others: BTreeMap<String, serde_json::Value>,
    mtime: Option<SystemTime>,
    #[cfg(test)]
    reads: usize,
    #[cfg(test)]
    fail_writes: bool,
}

impl Store {
    pub fn open(state_dir: Option<PathBuf>, toplevel: &str, now: u64) -> (Self, Vec<String>) {
        let mut store = Self {
            state_dir,
            toplevel: toplevel.to_string(),
            comments: Vec::new(),
            journal: Vec::new(),
            journal_reason: None,
            others: BTreeMap::new(),
            mtime: None,
            #[cfg(test)]
            reads: 0,
            #[cfg(test)]
            fail_writes: false,
        };
        let mut problems = Vec::new();
        if store.state_dir.is_some() {
            // Expiry on load is a transaction too: whichever viewer notices first writes it.
            match store.locked(|store| store.read_and_apply(None, now)) {
                Ok(Ok((_, problem))) => problems.extend(problem),
                Ok(Err(TxError::Refused(e) | TxError::Io(e))) | Err(TxError::Refused(e) | TxError::Io(e)) => problems.push(e),
            }
        }
        (store, problems)
    }

    pub fn comments(&self) -> &[Comment] {
        &self.comments
    }

    pub fn journal_len(&self) -> usize {
        self.journal.len()
    }

    pub fn path(&self) -> Option<PathBuf> {
        self.state_dir.as_ref().map(|d| d.join(COMMENTS_FILE))
    }

    fn locked<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> Result<T, TxError> {
        let Some(dir) = self.state_dir.clone() else {
            return Err(TxError::Io("no state directory".to_string()));
        };
        if !dir.is_absolute() {
            return Err(TxError::Io("state directory must be absolute".to_string()));
        }
        // `with_lock` borrows nothing of `self`, so the closure may.
        let mut result = None;
        reuse::with_lock(&dir, || result = Some(f(self)))
            .map_err(|e| TxError::Io(format!("{COMMENTS_FILE}: {e}")))?;
        Ok(result.expect("the locked closure ran"))
    }

    /// An add the cap still refuses stays visible, journaled: the reviewer can shorten or drop it, and
    /// a later failure (another viewer filled the file; this viewer's edit could not be written) never
    /// hides what this viewer typed. Run after every reload of `comments`, on success and on failure.
    fn show_journaled_adds(&mut self) {
        for entry in &self.journal {
            if let Operation::Add(comment) = entry {
                if !self.comments.iter().any(|c| c.id == comment.id) {
                    self.comments.push(comment.clone());
                }
            }
        }
    }

    /// Under the lock: read the file, replay the journal, apply `op`, expire, write when anything
    /// changed, keep the merged array. `Ok((dropped, problem))`: the notice for a dropped journal
    /// entry and the load problem, if any. The journal is replaced only after a successful write.
    fn read_and_apply(&mut self, op: Option<&Operation>, now: u64) -> Result<(Option<String>, Option<String>), TxError> {
        let dir = self.state_dir.clone().expect("locked() checked the directory");
        let path = dir.join(COMMENTS_FILE);
        #[cfg(test)]
        {
            self.reads += 1;
        }
        let (mut all, mut comments, problem) = match read_file(&path).map_err(TxError::Io)? {
            None => (BTreeMap::new(), Vec::new(), None),
            Some((text, _)) => parse_comments(&text, &self.toplevel),
        };
        // The other worktrees' arrays, kept for this write and for the next claim or settlement's.
        self.others = all.clone();
        // What the file holds, kept if the write below fails: a store that cannot persist an expiry
        // must still show the comments it read.
        let loaded = comments.clone();
        let mut changed = false;
        let mut dropped = None;
        let mut kept_journal = Vec::new();
        for entry in self.journal.clone() {
            match apply(&mut comments, &entry) {
                Ok(true) => changed = true,
                Ok(false) => dropped = Some(format!("a comment changed under you; your {} was dropped", entry.describe())),
                // A refused add (the cap) stays journaled: nothing of the other viewer's is touched.
                Err(_) => kept_journal.push(entry),
            }
        }
        let refusal = match op {
            Some(op) => match apply(&mut comments, op) {
                Ok(true) => {
                    changed = true;
                    None
                }
                Ok(false) => Some(match op {
                    Operation::Edit { .. } | Operation::Delete { .. } => "No comment selected.".to_string(),
                    Operation::Add(_) => unreachable!("an add never meets a changed record"),
                }),
                Err(e) => Some(e),
            },
            None => None,
        };
        changed |= expire(&mut comments, now);
        if changed {
            // The write below rewrites this worktree's array without its unusable records: say so first
            // (10.7's drop-and-report), whatever brought the write about.
            if let Some(problem) = &problem {
                base::note_problem(&dir, &format!("{problem}; rewritten without them"));
            }
            let written = (|| -> Result<(), TxError> {
                #[cfg(test)]
                if self.fail_writes {
                    return Err(TxError::Io(format!("{COMMENTS_FILE}: write failed (test)")));
                }
                all.insert(
                    self.toplevel.clone(),
                    serde_json::to_value(&comments).map_err(|e| TxError::Io(e.to_string()))?,
                );
                let tmp = dir.join(format!("{COMMENTS_FILE}.{}.tmp", std::process::id()));
                std::fs::write(&tmp, serde_json::to_vec_pretty(&all).unwrap_or_default())
                    .map_err(|e| TxError::Io(format!("{COMMENTS_FILE}: {e}")))?;
                std::fs::rename(&tmp, &path).map_err(|e| TxError::Io(format!("{COMMENTS_FILE}: {e}")))
            })();
            if let Err(e) = written {
                // The screen shows what the file holds plus what this viewer still owes it: the journal's
                // operations are reapplied, so an earlier unsaved add is not hidden by a later failure,
                // and one the cap refuses on a full file is overlaid all the same (`show_journaled_adds`).
                self.comments = loaded;
                for entry in &self.journal {
                    let _ = apply(&mut self.comments, entry);
                }
                self.show_journaled_adds();
                self.mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                return Err(e);
            }
        }
        self.journal = kept_journal;
        self.mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        self.comments = comments;
        self.show_journaled_adds();
        if let Some(refusal) = refusal {
            return Err(TxError::Refused(refusal));
        }
        Ok((dropped, problem))
    }

    /// One transaction of spec 10.3. `Ok(Some(notice))` when the replay dropped an entry; `Err`
    /// with the notice when the operation was refused or could not be written (then journaled).
    pub fn transact(&mut self, op: Operation, now: u64) -> Result<Option<String>, String> {
        if self.state_dir.is_none() {
            // The array is the store: nothing can ever be written, so nothing is journaled; the
            // refusal rules are the persisted store's.
            let first = self.comments.is_empty();
            let applied = apply(&mut self.comments, &op)?;
            if !applied {
                return Err("No comment selected.".to_string());
            }
            return if first && op.is_add() {
                Err(format!("{NOTICE_NOT_REMEMBERED}no state directory"))
            } else {
                Ok(None)
            };
        }
        // A delete or edit of a record only the journal holds changes the journal, not the file.
        if let Operation::Delete { id, .. } | Operation::Edit { id, .. } = &op {
            if let Some(index) = self.journal.iter().position(|e| matches!(e, Operation::Add(c) if &c.id == id)) {
                match &op {
                    Operation::Delete { .. } => {
                        self.journal.remove(index);
                        self.comments.retain(|c| &c.id != id);
                    }
                    Operation::Edit { category, text, .. } => {
                        if let Operation::Add(c) = &mut self.journal[index] {
                            c.category = *category;
                            c.text = text.clone();
                        }
                        if let Some(c) = self.comments.iter_mut().find(|c| &c.id == id) {
                            c.category = *category;
                            c.text = text.clone();
                        }
                    }
                    Operation::Add(_) => unreachable!(),
                }
                return Ok(None);
            }
        }
        match self.locked(|store| store.read_and_apply(Some(&op), now)) {
            Ok(Ok((dropped, _))) => Ok(dropped),
            Ok(Err(TxError::Refused(notice))) | Err(TxError::Refused(notice)) => Err(notice),
            Ok(Err(TxError::Io(e))) | Err(TxError::Io(e)) => {
                // The store could not be read or written: the rules still hold against the array in
                // memory, and only what passes them is held here and replayed later. What they refuse
                // is refused now, as a journaled "accepted" would close the editor on a lost change.
                match apply(&mut self.comments, &op) {
                    Err(refusal) => return Err(refusal),
                    Ok(false) => return Err("No comment selected.".to_string()),
                    Ok(true) => {}
                }
                self.journal.push(op);
                self.journal_reason = Some(e.clone());
                Err(format!("{NOTICE_NOT_REMEMBERED}{e}"))
            }
        }
    }

    /// Each refresh: reread when the file moved, replay the journal when there is one, expire.
    pub fn refresh(&mut self, now: u64) -> Vec<String> {
        let Some(path) = self.path() else {
            expire(&mut self.comments, now);
            return Vec::new();
        };
        let moved = std::fs::metadata(&path).and_then(|m| m.modified()).ok() != self.mtime;
        let expiring = self.comments.iter().any(|c| matches!(&c.state, CommentState::Sending { stamp, .. } if now.saturating_sub(stamp.at) >= EXPIRY.as_secs()));
        if !moved && !expiring && self.journal.is_empty() {
            return Vec::new();
        }
        match self.locked(|store| store.read_and_apply(None, now)) {
            Ok(Ok((dropped, problem))) => dropped.into_iter().chain(problem).collect(),
            Ok(Err(TxError::Refused(e) | TxError::Io(e))) | Err(TxError::Refused(e) | TxError::Io(e)) => vec![e],
        }
    }

    #[cfg(test)]
    pub fn comments_mut_for_tests(&mut self) -> &mut Vec<Comment> {
        &mut self.comments
    }

    #[cfg(test)]
    pub fn write_for_tests(&mut self) {
        let comments = self.comments.clone();
        self.set_for_tests(comments);
    }

    #[cfg(test)]
    pub fn set_for_tests(&mut self, comments: Vec<Comment>) {
        let dir = self.state_dir.clone().unwrap();
        let path = dir.join(COMMENTS_FILE);
        let mut all: BTreeMap<String, serde_json::Value> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        all.insert(self.toplevel.clone(), serde_json::to_value(&comments).unwrap());
        std::fs::write(&path, serde_json::to_vec_pretty(&all).unwrap()).unwrap();
        self.comments = comments;
        self.mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    }

    #[cfg(test)]
    pub fn reads_for_tests(&self) -> usize {
        self.reads
    }

    /// `open` with the write failure injected before the first read, for the startup case.
    #[cfg(test)]
    pub fn open_for_tests_with_failing_writes(state_dir: &Path, toplevel: &str, now: u64) -> Self {
        let mut store = Self { state_dir: Some(state_dir.to_path_buf()), toplevel: toplevel.to_string(), comments: Vec::new(), journal: Vec::new(), journal_reason: None, others: BTreeMap::new(), mtime: None, reads: 0, fail_writes: true };
        let _ = store.locked(|store| store.read_and_apply(None, now));
        store
    }

    #[cfg(test)]
    pub fn fail_writes_for_tests(&mut self, fail: bool) {
        self.fail_writes = fail;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestFile {
    pub key: FileKey,
    pub comparison: AnchorComparison,
    pub additions: Vec<(u32, u32)>,
    pub deletions: Vec<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestRecord {
    pub nonce: String,
    pub at: u64,
    pub target: Destination,
    pub files: Vec<RequestFile>,
}

/// The shape rules of 10.7 for a request record: the comment record's rules for a path and a comparison
/// (`path_ok`, `comparison_ok`, shared with `check_record`), one-based ordered ranges, a nonce, a destination.
fn check_request(record: &RequestRecord) -> bool {
    is_nonce(&record.nonce)
        && record.target.is_well_formed()
        && record.files.iter().all(|f| {
            path_ok(&f.key.path)
                && comparison_ok(&f.comparison)
                && f.additions.iter().chain(&f.deletions).all(|(a, b)| *a >= 1 && a <= b)
        })
}

/// The request file as a map of raw per-worktree values. Another worktree's entry is kept whatever its
/// shape, so one malformed entry never costs the others their records (and their reply correlations); a
/// missing file is an empty map; a file that cannot be read is an error. A top level that is no object
/// holds nothing anyone can keep: it reads as empty, with the problem beside it for the caller to report.
fn read_request_map(path: &Path) -> std::io::Result<(BTreeMap<String, serde_json::Value>, Option<String>)> {
    match std::fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((BTreeMap::new(), None)),
        Err(e) => Err(e),
        Ok(text) => match serde_json::from_str::<BTreeMap<String, serde_json::Value>>(&text) {
            Ok(all) => Ok((all, None)),
            Err(e) => Ok((BTreeMap::new(), Some(format!("{REQUESTS_FILE}: not a JSON object ({e}); its records are discarded")))),
        },
    }
}

/// This worktree's records out of the map: each decoded alone, the unusable ones counted.
fn parse_requests(all: &BTreeMap<String, serde_json::Value>, toplevel: &str) -> (Vec<RequestRecord>, Option<String>) {
    let raw: Vec<serde_json::Value> = match all.get(toplevel) {
        None => Vec::new(),
        Some(serde_json::Value::Array(items)) => items.clone(),
        Some(_) => return (Vec::new(), Some(format!("{REQUESTS_FILE}: this worktree's entry is not a list"))),
    };
    let total = raw.len();
    let kept: Vec<RequestRecord> = raw
        .into_iter()
        .filter_map(|v| serde_json::from_value::<RequestRecord>(v).ok())
        .filter(check_request)
        .collect();
    let dropped = total - kept.len();
    (kept, (dropped > 0).then(|| format!("{REQUESTS_FILE}: {dropped} unusable record(s)")))
}

/// This worktree's requests, checked by shape; a malformed record is dropped alone, a malformed file whole.
pub fn load_requests(state_dir: &Path, toplevel: &str) -> (Vec<RequestRecord>, Option<String>) {
    match read_request_map(&state_dir.join(REQUESTS_FILE)) {
        Err(e) => (Vec::new(), Some(format!("{REQUESTS_FILE}: {e}"))),
        Ok((_, Some(problem))) => (Vec::new(), Some(problem)),
        Ok((all, None)) => parse_requests(&all, toplevel),
    }
}

fn write_request_map(state_dir: &Path, all: &BTreeMap<String, serde_json::Value>) -> std::io::Result<()> {
    let tmp = state_dir.join(format!("{REQUESTS_FILE}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(all).unwrap_or_default())?;
    std::fs::rename(tmp, state_dir.join(REQUESTS_FILE))
}

/// The read-merge-write without the lock: `record_request` takes it, `record_request_fresh` holds it already.
fn append_request(state_dir: &Path, toplevel: &str, record: &RequestRecord) -> std::io::Result<()> {
    // A file that cannot be read aborts the write, or the rename would replace every worktree's
    // records with this one's. Malformed records of this worktree are rewritten out; every other
    // entry is written back as read.
    let (mut all, problem) = read_request_map(&state_dir.join(REQUESTS_FILE))?;
    if let Some(problem) = problem {
        // The file is replaced by a valid one below; the problem would otherwise never be seen.
        base::note_problem(state_dir, &problem);
    }
    let (mut mine, dropped) = parse_requests(&all, toplevel);
    if let Some(dropped) = dropped {
        base::note_problem(state_dir, &format!("{dropped}; rewritten without them"));
    }
    mine.push(record.clone());
    all.insert(toplevel.to_string(), serde_json::Value::Array(mine.iter().map(|r| serde_json::to_value(r).unwrap_or_default()).collect()));
    write_request_map(state_dir, &all)
}

/// Appends under the state lock, through a temporary file and a rename.
pub fn record_request(state_dir: &Path, toplevel: &str, record: &RequestRecord) -> std::io::Result<()> {
    if !state_dir.is_absolute() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "state directory must be absolute"));
    }
    reuse::with_lock(state_dir, || append_request(state_dir, toplevel, record))?
}

/// A request recorded under a nonce no retained record of either file carries (spec 10.4 step 2).
/// Both files are read under the lock: a captured array could miss a nonce a concurrent send stamped.
pub fn record_request_fresh(
    state_dir: &Path,
    toplevel: &str,
    make: &dyn Fn(u64) -> String,
    counter: &mut u64,
    guard: &dyn Fn() -> Result<(), String>,
    build: impl FnOnce(&str) -> Result<(RequestRecord, String), String>,
) -> std::io::Result<(String, String)> {
    if !state_dir.is_absolute() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "state directory must be absolute"));
    }
    let mut build = Some(build);
    let mut chosen = (String::new(), String::new());
    reuse::with_lock(state_dir, || -> std::io::Result<()> {
        guard().map_err(|e| std::io::Error::new(std::io::ErrorKind::Interrupted, e))?;
        // Both files are read under the lock, and a read that fails fails the request: see `nonces_in_use`.
        let used = nonces_in_use(state_dir, &[])?;
        let nonce = loop {
            *counter += 1;
            let candidate = make(*counter);
            if !used.contains(&candidate) {
                break candidate;
            }
        };
        // The text is built, and its bound checked, before anything is written: a refusal records nothing.
        let (record, text) = (build.take().expect("built once"))(&nonce)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Interrupted, e))?;
        append_request(state_dir, toplevel, &record)?;
        chosen = (nonce, text);
        Ok(())
    })??;
    Ok(chosen)
}

/// `clipboard.md` replaced under the state lock through a temporary file named by pid and a counter,
/// so two copies of one viewer, or a copy and a clipboard send, never truncate each other's work.
pub fn write_clipboard(state_dir: &Path, text: &str) -> std::io::Result<PathBuf> {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if !state_dir.is_absolute() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "state directory must be absolute"));
    }
    reuse::with_lock(state_dir, || -> std::io::Result<PathBuf> {
        let path = state_dir.join("clipboard.md");
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = state_dir.join(format!("clipboard.md.{}.{n}.tmp", std::process::id()));
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &path)?;
        Ok(path)
    })?
}

/// Removes this worktree's request under `nonce`: a request whose send definitely failed.
pub fn remove_request(state_dir: &Path, toplevel: &str, nonce: &str) -> std::io::Result<()> {
    reuse::with_lock(state_dir, || {
        let (mut all, problem) = read_request_map(&state_dir.join(REQUESTS_FILE))?;
        if let Some(problem) = problem {
            base::note_problem(state_dir, &problem);
        }
        let (mine, dropped) = parse_requests(&all, toplevel);
        if let Some(dropped) = dropped {
            base::note_problem(state_dir, &format!("{dropped}; rewritten without them"));
        }
        let kept: Vec<_> = mine.into_iter().filter(|r| r.nonce != nonce).map(|r| serde_json::to_value(r).unwrap_or_default()).collect();
        all.insert(toplevel.to_string(), serde_json::Value::Array(kept));
        write_request_map(state_dir, &all)
    })?
}

/// Nonces any retained record of either file carries, every worktree's (two worktrees can share an
/// agent pane, and a reply is matched by nonce alone), plus `own`, the array the caller holds. A file
/// that cannot be read is an error: a nonce chosen blind could collide with one it holds.
/// Every nonce an array's records carry: the current stamps and the chains behind them. The one
/// collection the file-backed and the in-memory nonce checks share, so neither forgets an attempt.
pub fn nonces_of(comments: &[Comment]) -> BTreeSet<String> {
    comments
        .iter()
        .flat_map(|c| {
            let before: Vec<String> = match &c.state {
                CommentState::Sending { before, .. } | CommentState::Unconfirmed { before, .. } => before.iter().map(|b| b.nonce.clone()).collect(),
                _ => Vec::new(),
            };
            c.stamp().map(|s| s.nonce.clone()).into_iter().chain(before)
        })
        .collect()
}

pub fn nonces_in_use(state_dir: &Path, own: &[Comment]) -> std::io::Result<BTreeSet<String>> {
    let mut used: BTreeSet<String> = nonces_of(own);
    match std::fs::read_to_string(state_dir.join(COMMENTS_FILE)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
        Ok(text) => {
            // A malformed file retains nothing readable; its records are dropped on the next write anyway.
            if let Ok(all) = serde_json::from_str::<BTreeMap<String, serde_json::Value>>(&text) {
                for toplevel in all.keys() {
                    used.extend(nonces_of(&parse_comments(&text, toplevel).1));
                }
            }
        }
    }
    match std::fs::read_to_string(state_dir.join(REQUESTS_FILE)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
        Ok(text) => {
            // Per worktree: an entry that is no list is skipped, the others still count.
            if let Ok(all) = serde_json::from_str::<BTreeMap<String, serde_json::Value>>(&text) {
                used.extend(all.values().filter_map(|v| v.as_array()).flatten().filter_map(|v| v["nonce"].as_str().map(str::to_string)));
            }
        }
    }
    Ok(used)
}
```

An edit of an `Unconfirmed` record makes it `Pending` (its old text is what may have arrived; a late answer for the old nonce then settles nothing, and the new text goes out under a new nonce at the next finish); this is the one place a state moves backwards without a settlement, and `settle`'s nonce match is what keeps it safe. `record_request` is the lock-taking wrapper around a private `append_request(state_dir, toplevel, record)` that does the read-merge-write without taking the lock, so `record_request_fresh` can call it under the lock it already holds; `write_clipboard` writes under the same lock through a counter-named temporary file, so two copies, or a copy and a clipboard send, never truncate each other's work. Points the implementer must not smooth over: `apply` on an `Edit`/`Delete` compares the whole seen record (`current != seen`), which is the journal's conflict rule and, for a live transaction, catches an edit another viewer made between this viewer's last reread and now; the TUI always passes the comment as the snapshot shows it. A refused add from the journal (the cap) stays journaled rather than dropped, so a cap reached by another viewer never destroys this viewer's unsent text. The `id_ok` branch accepts short alphanumeric ids so the tests above and Task 5's fixtures can name comments `a1`; production ids are `new_id()`'s 32 hex characters and pass the first arm. Add `pub mod comments;` to `src/engine/mod.rs` and the serde derives on `FileKey` (`types.rs`) and `Side` (`nav.rs`: `#[derive(Serialize, Deserialize)] #[serde(rename_all = "lowercase")]`).

- [ ] **Step 4: Run the module tests**

Run: `cargo test --locked --lib engine::comments`
Expected: twelve pass.

- [ ] **Step 5: Snapshot, commands, session**

In `types.rs`: `Snapshot` gains

```rust
    /// This worktree's comments as last read or written (spec 10.3), every state.
    pub comments: Arc<Vec<Comment>>,
    /// Bumped once per answered add, edit or delete; `comment_error` is that answer.
    pub comment_seq: u64,
    pub comment_error: Option<String>,
    /// The answer refused the operation and nothing changed (the cap, the limit, a record that changed
    /// under the edit); false for an accepted one, journaled or not. The editor keeps its draft on a refusal.
    pub comment_refused: bool,
    /// The `token` of the add or edit this answer is for (`None` for a delete or a refresh's notice): the
    /// editor that sent it closes or keeps its draft; another editor's save is not its answer.
    pub comment_token: Option<u64>,
```

(`comments: Arc::new(Vec::new()), comment_seq: 0, comment_error: None, comment_refused: false, comment_token: None` in `empty`), and `Command` gains:

```rust
    /// One transaction each (spec 10.3); answered on `comment_seq`. `seen` is the record as the viewer
    /// showed it when the key was pressed, which the transaction compares against the file's.
    AddComment { token: u64, anchor: Anchor, category: Category, text: String },
    EditComment { token: u64, seen: Comment, category: Category, text: String },
    DeleteComment { seen: Comment },
```

In `session.rs`, `State` gains `store: Option<comments::Store>`, `store_opened: bool` and `store_opening: bool`. The first `Done::Status` that finds a toplevel, with `!store_opened && !store_opening`, sets `store_opening` and runs `comments::Store::open(state_dir, &toplevel, now())` on the blocking pool (`Store::open` takes the state lock and may write expiries: never on the loop), answering `Done::StoreOpened(store, problems)`, whose handler sets `store`, `store_opened`, clears `store_opening`, notes the problems and publishes `comments`; commands that arrive before it is open are queued (`store_queue`, below) and run when it is. The helper the arms share:

```rust
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
```

The three commands run the transaction on the blocking pool, because `with_lock` can wait two seconds, and answer with `Done::Comment`:

```rust
                    Command::AddComment { token, anchor, category, text } => { /* see below */ }
                    Command::EditComment { token, seen, category, text } => { /* see below */ }
                    Command::DeleteComment { seen } => { /* see below */ }
```

Write it as one arm per command that builds an `Operation` and calls a shared method `State::transact(op, token, results)` (`Some(token)` for an add or edit, `None` for a delete):

```rust
    /// The store moves to the blocking pool for the transaction and comes back with the answer;
    /// a store that is out on another transaction (or a send, Task 5), or still opening, queues the
    /// operation; only a directory that is no repository refuses.
    fn transact(&mut self, op: comments::Operation, token: Option<u64>, results: &UnboundedSender<Done>) {
        if !self.store_opened && !self.store_opening {
            let _ = results.send(Done::Comment { token, store: None, outcome: Err("not a git repository".into()) });
            return;
        }
        self.store_queue.push_back(StoreOp::Comment(op, token));
        self.run_store_queue_comments(results);
    }

    /// Runs the queue's front entry when the store is here (Task 5 generalises this to sends).
    fn run_store_queue_comments(&mut self, results: &UnboundedSender<Done>) {
        let Some(store) = self.store.take() else { return };
        let Some(StoreOp::Comment(op, token)) = self.store_queue.pop_front() else {
            self.store = Some(store);
            return;
        };
        let mut store = store;
        let results = results.clone();
        tokio::spawn(async move {
            let (store, outcome) = tokio::task::spawn_blocking(move || {
                let outcome = store.transact(op, now());
                (store, outcome)
            })
            .await
            .expect("transaction task");
            let _ = results.send(Done::Comment { token, store: Some(store), outcome });
        });
    }
```

`State` gains `store_opened: bool` and `store_queue: VecDeque<StoreOp>` with `enum StoreOp { Comment(comments::Operation, Option<u64> /* the save token */) }` (Task 5 adds its variants). `Done::Comment` puts the store back and calls `run_store_queue_comments` again. The one writer of a target record outside a pick is also this task's:

```rust
    /// Writes the target record when something in memory is newer than the file (an opener
    /// preselection on its first comment; an adopted session; Task 5's accepted restart), on the
    /// blocking pool, guarded by the selection generation so an older write never lands on a newer pick.
    fn write_target(&mut self, next: &Snapshot, results: &UnboundedSender<Done>) {
        if !std::mem::take(&mut self.target_write_pending) {
            return;
        }
        let (Some(target), Some(dir), RepoState::Repo { toplevel, .. }) = (next.target.clone(), self.state_dir.clone(), &next.repo) else {
            return;
        };
        let results = results.clone();
        let (toplevel, generation, latest) = (toplevel.clone(), self.selection_generation, self.latest_selection.clone());
        let (gate, waiting, done) = (self.target_write_gate.clone(), self.target_writes_waiting.clone(), self.target_writes_done.clone());
        tokio::spawn(async move {
            // The test seam: a write waits here while a test lets a newer pick land first.
            if let Some(gate) = gate {
                waiting.fetch_add(1, Ordering::SeqCst);
                gate.acquire().await.expect("gate").forget();
            }
            let written = tokio::task::spawn_blocking(move || {
                target::save_target_if(&dir, &toplevel, &target, generation, &latest).map(|_| ()).map_err(|e| e.to_string())
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
            done.fetch_add(1, Ordering::SeqCst);
            // The same answer a pick's write gives: the notice of 10.7 when it fails, and the flag back on.
            let _ = results.send(Done::Target { generation, written });
        });
    }
```

`SessionConfig.target_write_gate: Option<Arc<Semaphore>>` (`None` in production; Task 2's `SetTarget` arm waits at the same gate, so a test can hold any target write) and two `EngineHandle` counters, `target_writes_waiting` and `target_writes_done`, are the seams; with the guard replaced by a plain `save_target`, the test fails on its last assertion every time, because the old write is held until after the new pick's own write. Every target write, a pick's or this method's, answers `Done::Target { generation, written }`: the handler (Task 2) drops an answer whose generation is not the current selection's, publishes the others under `target_seq` with `target not remembered: <reason>` on failure (10.7's notice, for a pick and for the opener's first-comment write alike), and on failure sets `target_write_pending` again, so the next comment's write tries once more; `an_opener_write_that_fails_says_so_and_the_next_comment_retries` below pins the failure and the recovery, and `a_pick_answered_after_a_newer_pick_is_dropped` in Task 2 the stale answer.

The regression for it is this task's, because it needs the comment command and the write; it sits in `session.rs`'s tests with the three below. Its race is made, not hoped for: the adoption's write is held at a seam while the re-pick lands and writes, then released, so it is always the later writer and only the generation guard stops it (the orchestrator should see this test fail with `save_target` in place of `save_target_if`):

```rust
    #[test]
    fn a_delayed_adoption_write_never_overwrites_a_newer_pick() {
        let dir = fixture();
        let state = tempfile::tempdir().unwrap();
        let top = dir.path().canonicalize().unwrap().to_string_lossy().into_owned();
        // The remembered record has no session; the host reports one, which the check adopts.
        target::save_target(state.path(), &top, &pane_target("w4:p2", "codex", None)).unwrap();
        let host = host::Scripted::with_panes(vec![agent_pane("w4:p2", "codex", "idle", Some("s1"), &top)]);
        let mut config = test_config(dir.path(), Arc::new(AtomicBool::new(true)));
        config.state_dir = Some(state.path().to_path_buf());
        config.host = Some(host.clone());
        config.socket_path = Some("/run/fake.sock".into());
        // The seam: a target write started by `write_target` waits for a permit before it takes the lock.
        let gate = Arc::new(Semaphore::new(0));
        config.target_write_gate = Some(gate.clone());
        let (_rt, handle) = start_from(config);
        wait_for(&handle, "adopted in memory", |s| matches!(&s.target, Some(Target::Pane { session: Some(sess), .. }) if sess.value == "s1"));
        // The adoption's write is started by a comment and held at the seam while a re-pick lands;
        // released, it must find the newer generation under the lock and skip itself.
        handle.commands.send(pending(2, "x")).unwrap();
        wait_for(&handle, "comment", |s| s.comment_seq == 1);
        wait_cond("the adoption's write is waiting at the gate", || handle.target_writes_waiting.load(Ordering::SeqCst) == 1);
        handle.commands.send(Command::SetTarget(Target::Clipboard)).unwrap();
        wait_for(&handle, "re-pick answered", |s| s.target_seq == 1 && s.target == Some(Target::Clipboard) && s.target_error.is_none());
        assert_eq!(target::load_targets(state.path()).0.get(&top), Some(&Target::Clipboard));
        gate.add_permits(1);
        wait_cond("the adoption's write ran", || handle.target_writes_done.load(Ordering::SeqCst) == 1);
        assert_eq!(target::load_targets(state.path()).0.get(&top), Some(&Target::Clipboard), "an older write replaced the newer pick");
    }
```

with `Done::Comment { token: Option<u64>, store: Option<comments::Store>, outcome: Result<Option<String>, String> }` handled as:

```rust
                    Done::Comment { token, store, outcome } => {
                        if let Some(store) = store {
                            next.comments = Arc::new(store.comments().to_vec());
                            state.store = Some(store);
                        }
                        state.comment_seq += 1;
                        next.comment_seq = state.comment_seq;
                        next.comment_token = token;
                        // Journaled is not refused: the comment is on screen and will be written later.
                        next.comment_refused = matches!(&outcome, Err(e) if !e.starts_with(comments::NOTICE_NOT_REMEMBERED));
                        next.comment_error = match outcome {
                            Ok(notice) => notice,
                            Err(e) => Some(e),
                        };
                        if next.comment_error.as_deref().is_none_or(|e| !e.starts_with(comments::NOTICE_NOT_REMEMBERED)) {
                            // The first comment under an opener preselection makes it the remembered target; an
                            // adopted session rides along (Task 5's `write_target` writes on the blocking pool).
                            if state.target_source == Some(target::Source::Opener) {
                                state.target_write_pending = true;
                                state.target_source = Some(target::Source::Remembered);
                            }
                            if state.target_write_pending {
                                state.write_target(&next, &results_tx);
                            }
                        }
                        publish(&mut state, next, &snapshots);
                    }
```

The `AddComment` arm builds the comment: `comments::Comment { id: comments::new_id(), anchor, category, text: checked, created_at: now(), state: Pending }` after `comments::check_text(&text)`, answering a refusal at once (`comment_seq += 1; comment_error = Some(NOTICE_LIMIT); comment_refused = true; comment_token = Some(token)`) without a transaction; the refresh's dropped-journal notice publishes `comment_refused = false, comment_token = None`. `EditComment { seen, category, text }` becomes `Operation::Edit { id: seen.id.clone(), category, text: checked, seen }` and `DeleteComment { seen }` becomes `Operation::Delete { id: seen.id.clone(), seen }`: the TUI sends the record it showed, not the engine's latest, so an edit another viewer saved while the editor was open is met by the `current != seen` rule and refused, never overwritten.

The refresh: `run_job` cannot carry the store (it lives on `State`), so on every `Done::Status` with a toplevel, if `state.store` is `Some` and no transaction is in flight, run `store.refresh(now())` on the pool the same way and publish `next.comments` when the array changed (compare by `!=`; the `Arc` is replaced only then, which keeps `Arc::ptr_eq` honest for Task 7's reconcile). Problems from `refresh` go to `note_problem` and, for a dropped journal entry, to a notice: carry it on `comment_error` with a bumped `comment_seq`, so the TUI shows it once.

`fingerprint` gains `s.comments.as_ref() as *const _ as usize` (the Arc's pointer: `Arc::as_ptr(&s.comments)`), `s.comment_seq`, `s.comment_error`, `s.comment_refused`, `s.comment_token`.

- [ ] **Step 6: Session tests**

```rust
    fn pending(anchor_line: u32, text: &str) -> Command {
        Command::AddComment {
            token: 0,
            anchor: comments::Anchor {
                key: FileKey { path: "a.txt".into(), staged: false, untracked: false },
                side: crate::engine::nav::Side::Additions,
                line: anchor_line,
                span: comments::Span::Line,
                comparison: comments::AnchorComparison::Worktree,
            },
            category: comments::Category::Bug,
            text: text.into(),
        }
    }

    #[test]
    fn comments_are_added_edited_deleted_and_shared_between_two_sessions() {
        let dir = fixture();
        let state = tempfile::tempdir().unwrap();
        let mut config = test_config(dir.path(), Arc::new(AtomicBool::new(true)));
        config.state_dir = Some(state.path().to_path_buf());
        let (_rt, a) = start_from(config);
        let mut config = test_config(dir.path(), Arc::new(AtomicBool::new(true)));
        config.state_dir = Some(state.path().to_path_buf());
        let (_rt2, b) = start_from(config);
        wait_for(&a, "rows", |s| !s.files.is_empty());
        wait_for(&b, "rows", |s| !s.files.is_empty());
        a.commands.send(pending(2, "floor division")).unwrap();
        let s = wait_for(&a, "added", |s| s.comment_seq == 1);
        assert!(s.comment_error.is_none());
        assert_eq!(s.comments.len(), 1);
        let id = s.comments[0].id.clone();
        let s = wait_for(&b, "b sees it", |s| s.comments.len() == 1);
        assert_eq!(s.comments[0].text, "floor division");
        let seen = s.comments[0].clone();
        b.commands.send(Command::EditComment { token: 7, seen: seen.clone(), category: comments::Category::Question, text: "why floor?".into() }).unwrap();
        wait_for(&b, "edited", |s| s.comment_seq == 1 && s.comment_error.is_none());
        let s = wait_for(&a, "a sees the edit", |s| s.comments.first().is_some_and(|c| c.text == "why floor?"));
        // An edit against the record as a *stale* viewer saw it is refused: b's edit stands.
        a.commands.send(Command::EditComment { token: 8, seen: seen.clone(), category: comments::Category::Bug, text: "mine".into() }).unwrap();
        let s2 = wait_for(&a, "stale edit refused", |s| s.comment_seq == 2);
        assert_eq!(s2.comment_error.as_deref(), Some("No comment selected."));
        assert_eq!(s2.comments[0].text, "why floor?");
        let edited = s.comments[0].clone();
        a.commands.send(Command::DeleteComment { seen: edited }).unwrap();
        wait_for(&a, "deleted", |s| s.comment_seq == 3 && s.comments.is_empty());
        wait_for(&b, "b sees the deletion", |s| s.comments.is_empty());
        // A stale edit (b's snapshot still named the deleted record) answers the notice and changes nothing.
        b.commands.send(Command::EditComment { token: 9, seen, category: comments::Category::Bug, text: "x".into() }).unwrap();
        let s = wait_for(&b, "stale edit answered", |s| s.comment_seq == 2);
        assert_eq!(s.comment_error.as_deref(), Some("No comment selected."));
        // The limit is answered without a transaction.
        a.commands.send(pending(2, &"x".repeat(comments::MAX_CHARS + 1))).unwrap();
        let s = wait_for(&a, "limit", |s| s.comment_seq == 4);
        assert_eq!(s.comment_error.as_deref(), Some(comments::NOTICE_LIMIT));
    }

    #[test]
    fn an_opener_write_that_fails_says_so_and_the_next_comment_retries() {
        let dir = fixture();
        let state = tempfile::tempdir().unwrap();
        let top = dir.path().canonicalize().unwrap().to_string_lossy().into_owned();
        let host = host::Scripted::with_panes(vec![agent_pane("w4:p1", "claude", "idle", Some("c1"), &top)]);
        // A directory where targets.json should be: the record cannot be written until it is gone.
        std::fs::create_dir(state.path().join("targets.json")).unwrap();
        let mut config = test_config(dir.path(), Arc::new(AtomicBool::new(true)));
        config.state_dir = Some(state.path().to_path_buf());
        config.host = Some(host.clone());
        config.socket_path = Some("/run/fake.sock".into());
        config.opener_pane = Some("w4:p1".into());
        let (_rt, handle) = start_from(config);
        wait_for(&handle, "opener preselected", |s| s.target.is_some());
        // The first comment writes the opener's record; the write fails and says so (10.7).
        handle.commands.send(pending(2, "x")).unwrap();
        let s = wait_for(&handle, "the write answered", |s| s.target_seq == 1);
        assert!(s.target_error.as_deref().is_some_and(|e| e.starts_with("target not remembered: ")), "{:?}", s.target_error);
        assert!(matches!(&s.target, Some(Target::Pane { pane, .. }) if pane == "w4:p1"), "the target stays in memory");
        // Room again: the next comment carries the write, which now lands without a notice.
        std::fs::remove_dir(state.path().join("targets.json")).unwrap();
        handle.commands.send(pending(1, "y")).unwrap();
        let s = wait_for(&handle, "written", |s| s.target_seq == 2);
        assert_eq!(s.target_error, None);
        assert!(matches!(target::load_targets(state.path()).0.get(&top), Some(Target::Pane { pane, .. }) if pane == "w4:p1"));
    }

    #[test]
    fn a_comment_sent_while_the_store_opens_is_kept_and_lands() {
        let dir = fixture();
        let state = tempfile::tempdir().unwrap();
        // The state lock is held while the session starts: `Store::open` must wait for it on the pool,
        // and a comment sent meanwhile must be queued, not refused as "not a git repository".
        let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(state.path().join("split-panes.lock")).unwrap();
        use std::os::unix::io::AsRawFd;
        assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
        let mut config = test_config(dir.path(), Arc::new(AtomicBool::new(true)));
        config.state_dir = Some(state.path().to_path_buf());
        let (_rt, handle) = start_from(config);
        wait_for(&handle, "rows", |s| !s.files.is_empty());
        handle.commands.send(pending(2, "early")).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(handle.snapshots.try_iter().all(|s| s.comment_seq == 0), "answered before the store was open");
        drop(lock);
        let s = wait_for(&handle, "landed", |s| s.comment_seq == 1);
        assert!(s.comment_error.is_none(), "{:?}", s.comment_error);
        assert_eq!(s.comments[0].text, "early");
    }

    #[test]
    fn comments_hold_for_the_session_without_a_state_directory() {
        let dir = fixture();
        let (_rt, handle) = start(dir.path(), Arc::new(AtomicBool::new(true)));
        wait_for(&handle, "rows", |s| !s.files.is_empty());
        handle.commands.send(pending(2, "one")).unwrap();
        let s = wait_for(&handle, "first", |s| s.comment_seq == 1);
        assert_eq!(s.comment_error.as_deref(), Some("comments not remembered: no state directory"));
        handle.commands.send(pending(2, "two")).unwrap();
        let s = wait_for(&handle, "second", |s| s.comment_seq == 2);
        assert!(s.comment_error.is_none(), "the notice shows once");
        assert_eq!(s.comments.len(), 2);
    }

    #[test]
    fn a_sending_record_older_than_a_minute_is_published_unconfirmed_by_a_refresh() {
        let dir = fixture();
        let state = tempfile::tempdir().unwrap();
        let top = dir.path().canonicalize().unwrap().to_string_lossy().into_owned();
        // Another viewer's claim, made 61 seconds ago, left behind.
        let mut stale = comments::Comment {
            id: comments::new_id(),
            anchor: comments::Anchor { key: FileKey { path: "a.txt".into(), staged: false, untracked: false }, side: crate::engine::nav::Side::Additions, line: 2, span: comments::Span::Line, comparison: comments::AnchorComparison::Worktree },
            category: comments::Category::Bug,
            text: "stale".into(),
            created_at: 1,
            state: comments::CommentState::Pending,
        };
        stale.state = comments::CommentState::Sending {
            stamp: comments::Stamp { at: now() - 61, nonce: "abcdef".into(), item: 1, to: target::Destination::clipboard() },
            before: Vec::new(),
        };
        std::fs::write(state.path().join("comments.json"), serde_json::json!({ top.as_str(): [stale] }).to_string()).unwrap();
        let mut config = test_config(dir.path(), Arc::new(AtomicBool::new(true)));
        config.state_dir = Some(state.path().to_path_buf());
        let (_rt, handle) = start_from(config);
        let s = wait_for(&handle, "unconfirmed", |s| matches!(s.comments.first().map(|c| &c.state), Some(comments::CommentState::Unconfirmed { .. })));
        assert_eq!(s.comments[0].stamp().unwrap().nonce, "abcdef");
    }
```

Run: `cargo test --locked --lib engine::session::tests::comments -- --test-threads=1` and the other two by name. Expected: pass; before Step 5 they fail to compile.

- [ ] **Step 7: Gates and commit**

```bash
git add src/engine/comments.rs src/engine/types.rs src/engine/nav.rs src/engine/session.rs src/engine/mod.rs
git commit -m "feat(engine): comments shared through comments.json, with a journal and an expiry"
```

### Task 4: What is sent

Implements spec 10.5 whole and 10.4's nonce (step 2); 10.9 test 6. Read 10.5 before starting, with `vimeflow:src/features/diff/services/feedbackDispatch.ts` open beside it: the texts are its, line for line, except where 10.5 names a departure.

**Files:**
- Create: `src/engine/prompt.rs`, `src/engine/prompts/delegated-review.md`
- Modify: `src/engine/mod.rs`, `src/engine/session.rs` (`SessionConfig.nonce`), `scripts/port-check.sh`, `scripts/port-check-selftest.sh`

**Interfaces:**
- Consumes: Task 3's `Comment`, `Anchor`, `Span`, `AnchorComparison`, `Category`, `RequestFile`; 0.0.4's `FileKey`.
- Produces:

```rust
// src/engine/prompt.rs
/// The pin's `delegated-review.prompt.md`, byte for byte, `{{NONCE}}` included; `port-check.sh` proves it.
pub const DELEGATED_REVIEW: &str = include_str!("prompts/delegated-review.md");

/// C0 controls and DEL removed; `keep_newlines` keeps `\n` (comment lines), nothing else does.
pub fn strip_controls(text: &str, keep_newlines: bool) -> String;
/// `'<toplevel>'` with every `'` written as `'\''`.
pub fn quote_toplevel(toplevel: &str) -> String;
/// `unstaged`, `staged`, or `vs <label> @ <7 hex>`.
pub fn comparison_label(anchor: &Anchor) -> String;

pub struct Item<'a> { pub number: u32, pub comment: &'a Comment }
/// The review prompt of 10.5 for the claimed items, in claim order.
pub fn review(toplevel: &str, items: &[Item<'_>], nonce: &str) -> String;

/// One file of a review request, as the prompt names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestLine { pub path: String, pub staged: bool, pub untracked: bool }
pub enum RequestScope<'a> { Worktree, Branch { merge_base: &'a str } }
/// The review-request prompt of 10.5: the scope's files in their groups, then the pin's contract.
pub fn request(toplevel: &str, files: &[RequestLine], scope: RequestScope<'_>, nonce: &str) -> String;

/// Six characters of `[a-z0-9]` from a SHA-256 of the time, the pid and `counter`.
pub fn nonce(counter: u64) -> String;
pub fn is_nonce(text: &str) -> bool;   // six to sixteen alphanumerics, the shape the store checks

// src/engine/session.rs: SessionConfig gains
pub nonce: Arc<dyn Fn(u64) -> String + Send + Sync>,   // prompt::nonce in production; tests make it collide
```

- [ ] **Step 1: The prompt file and the port check**

Copy the pin's file: `git -C "$VIMEFLOW" show 91e45b1c:src/features/diff/prompts/delegated-review.prompt.md > src/engine/prompts/delegated-review.md` (1,919 bytes, 24 lines, `{{NONCE}}` appearing twice). In `scripts/port-check.sh`, after the `diff -ru "$work/src/git" src/git` line:

```sh
prompt=src/features/diff/prompts/delegated-review.prompt.md
git -C "$src" show "$PIN:$prompt" > "$work/delegated-review.md"
if ! cmp -s "$work/delegated-review.md" src/engine/prompts/delegated-review.md; then
  echo "port-check: src/engine/prompts/delegated-review.md differs from $PIN:$prompt" >&2
  exit 1
fi
```

and change the final echo to `echo "port-check: src/git and the review prompt match $PIN + ... patch(es)"`. In `scripts/port-check-selftest.sh`, the work directory must carry the file too: after `cp -R "$root/src/git" "$work/src/"` add `mkdir -p "$work/src/engine/prompts" && cp "$root/src/engine/prompts/delegated-review.md" "$work/src/engine/prompts/"`, and after the `extra file` case:

```sh
printf '\n> one more line\n' >> src/engine/prompts/delegated-review.md
check 'edited review prompt' fail
cp "$root/src/engine/prompts/delegated-review.md" src/engine/prompts/delegated-review.md
check 'restored review prompt' pass
```

Run: `scripts/port-check.sh "$VIMEFLOW" && sh scripts/port-check-selftest.sh "$VIMEFLOW"`. Expected: both pass; change one byte of the crate's file and watch `port-check.sh` fail, then restore it.

- [ ] **Step 2: Tests first**

Create `src/engine/prompt.rs` with its tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::comments::{AnchorComparison, Category, Comment, CommentState, Span};
    use crate::engine::nav::Side;
    use crate::engine::FileKey;

    fn comment(path: &str, staged: bool, side: Side, line: u32, span: Span, comparison: AnchorComparison, category: Category, text: &str) -> Comment {
        Comment {
            id: "c".into(),
            anchor: Anchor { key: FileKey { path: path.into(), staged, untracked: false }, side, line, span, comparison },
            category,
            text: text.into(),
            created_at: 1,
            state: CommentState::Pending,
        }
    }

    const MB: &str = "1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b";

    #[test]
    fn the_review_prompt_is_vimeflows_line_for_line_with_the_merge_base_footer() {
        let bug = comment("src/cart.py", false, Side::Additions, 16, Span::Line, AnchorComparison::Worktree, Category::Bug,
            "Floor division drops the cents: 19.99 with SAVE10 comes out wrong.\nUse true division and round to 2 decimals.");
        let question = comment("src/cart.py", false, Side::Additions, 15, Span::Line,
            AnchorComparison::Branch { merge_base: MB.into(), label: "main".into() }, Category::Question,
            "What happens when the code is not in DISCOUNT_CODES?");
        let text = review("/home/will/demo", &[Item { number: 1, comment: &bug }, Item { number: 2, comment: &question }], "oqzpww");
        let expected = "\
> Inline review — 2 items. Reply to each by its [#n].
>
> [#1 · Bug] /home/will/demo/src/cart.py:16 (additions) [unstaged]
> ─ Floor division drops the cents: 19.99 with SAVE10 comes out wrong.
> ─ Use true division and round to 2 decimals.
> → Fix this.
>
> [#2 · Question] /home/will/demo/src/cart.py:15 (additions) [vs main @ 1a2b3c4]
> ─ What happens when the code is not in DISCOUNT_CODES?
> → Answer inline in your reply. Do not edit files.
>
> ―
> Items marked [vs <base> @ <id>] compare the working tree with that merge-base: `git -C '/home/will/demo' diff <id> -- <path>` shows what I see.
> When done, end your reply with this exact block, echoing the nonce verbatim.
> status is one of: \"reply\" (answers a question), \"clarify\" (you need the user to answer — the thread awaits them), \"resolved\" (you made the change), \"deferred\" (punted for later; cite the issue # in text), \"rejected\" (declined).
> <<<VIMEFLOW_REPLY
> {\"v\":1,\"nonce\":\"oqzpww\",\"replies\":[{\"id\":1,\"status\":\"reply\",\"text\":\"...\"}]}
> VIMEFLOW_REPLY>>>";
        assert_eq!(text, expected);
    }

    #[test]
    fn every_place_side_and_label_and_no_merge_base_line_without_a_branch_item() {
        let range = comment("a.rs", true, Side::Deletions, 3, Span::Range { end: 9 }, AnchorComparison::Worktree, Category::Change, "tighten");
        let file = comment("b.rs", false, Side::Additions, 0, Span::File, AnchorComparison::Worktree, Category::Suggestion, "split this module");
        let text = review("/r", &[Item { number: 1, comment: &range }, Item { number: 2, comment: &file }], "abc123");
        assert!(text.starts_with("> Inline review — 2 items. Reply to each by its [#n].\n>\n"));
        assert!(text.contains("> [#1 · Change request] /r/a.rs:3-9 (deletions) [staged]\n> ─ tighten\n> → Make this change.\n>\n"));
        assert!(text.contains("> [#2 · Suggestion] /r/b.rs (file) [unstaged]\n> ─ split this module\n> → Apply this if you agree.\n>\n> ―\n> When done"));
        assert!(!text.contains("Items marked"));
        let one = review("/r", &[Item { number: 1, comment: &file }], "abc123");
        assert!(one.starts_with("> Inline review — 1 item. Reply"));
    }

    #[test]
    fn control_characters_leave_paths_labels_and_comments_and_newlines_stay() {
        let odd = comment("src/\u{1b}[201~evil.rs", false, Side::Additions, 1, Span::Line,
            AnchorComparison::Branch { merge_base: MB.into(), label: "ma\rin".into() }, Category::Bug, "line one\r\nline\ttwo\u{7f}");
        let text = review("/r", &[Item { number: 1, comment: &odd }], "abc123");
        assert!(text.contains("> [#1 · Bug] /r/src/[201~evil.rs:1 (additions) [vs main @ 1a2b3c4]"));
        assert!(text.contains("> ─ line one\n> ─ linetwo\n"));
        assert!(!text.chars().any(|c| c.is_control() && c != '\n'));
    }

    #[test]
    fn the_toplevel_is_single_quoted_with_quotes_escaped() {
        assert_eq!(quote_toplevel("/home/o'brien/repo"), "'/home/o'\\''brien/repo'");
        let file = comment("b.rs", false, Side::Additions, 0, Span::File, AnchorComparison::Branch { merge_base: MB.into(), label: "main".into() }, Category::Bug, "x");
        let text = review("/home/o'brien/repo", &[Item { number: 1, comment: &file }], "abc123");
        assert!(text.contains("`git -C '/home/o'\\''brien/repo' diff <id> -- <path>`"));
        assert!(text.contains("] /home/o'brien/repo/b.rs (file) [vs main @ 1a2b3c4]"));
    }

    #[test]
    fn the_request_prompt_groups_worktree_files_and_ends_with_the_pins_contract() {
        let files = [
            RequestLine { path: "src/cart.py".into(), staged: false, untracked: false },
            RequestLine { path: "notes.txt".into(), staged: false, untracked: true },
            RequestLine { path: "src/util.py".into(), staged: true, untracked: false },
        ];
        let text = request("/home/will/demo", &files, RequestScope::Worktree, "wvpx71");
        let head = "\
> Delegate a code review of these 3 changes:
> unstaged diff (`git -C '/home/will/demo' diff`):
> ─ src/cart.py (/home/will/demo/src/cart.py)
> ─ notes.txt (/home/will/demo/notes.txt) (untracked — not in git diff; read the file, all lines are additions)
> staged diff (`git -C '/home/will/demo' diff --cached`):
> ─ src/util.py (/home/will/demo/src/util.py)
>
";
        assert!(text.starts_with(head), "{text}");
        let contract = DELEGATED_REVIEW.trim_end().replace("{{NONCE}}", "wvpx71");
        assert_eq!(&text[head.len()..], contract);
        assert!(!text.contains("{{NONCE}}"));
        assert!(text.ends_with("> VIMEFLOW_REVIEW>>>"));
        // Branch scope: one group with the merge-base; one file reads "this 1 change".
        let one = request("/r", &files[..1], RequestScope::Branch { merge_base: MB }, "wvpx71");
        assert!(one.starts_with(&format!("> Delegate a code review of this 1 change:\n> branch diff (`git -C '/r' diff {MB}`):\n> ─ src/cart.py (/r/src/cart.py)\n>\n")));
        // The pinned file is the one the request quotes, byte for byte (port-check.sh proves it against the pin).
        assert_eq!(DELEGATED_REVIEW.matches("{{NONCE}}").count(), 2);
        assert!(DELEGATED_REVIEW.starts_with("> Anchor each finding with diff-side line numbers"));
    }

    #[test]
    fn nonces_are_six_lowercase_alphanumerics_and_differ_by_counter() {
        let a = nonce(1);
        let b = nonce(2);
        assert_eq!(a.len(), 6);
        assert!(a.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()));
        assert_ne!(a, b);
        assert!(is_nonce(&a) && is_nonce("abcdef0123456789") && !is_nonce("abcde") && !is_nonce("abcdef0123456789x") && !is_nonce("abc-12"));
    }
}
```

- [ ] **Step 3: Run to watch them fail**

Add `pub mod prompt;` to `src/engine/mod.rs` first. Run: `cargo test --locked --lib engine::prompt`
Expected: compile errors.

- [ ] **Step 4: The module**

```rust
//! The two prompts of spec 10.5: Vimeflow's, line for line, with `git -C` where a command is quoted.
use super::comments::{Anchor, AnchorComparison, Comment, Span};
use super::nav::Side;

/// The pin's `delegated-review.prompt.md`, byte for byte, `{{NONCE}}` included.
pub const DELEGATED_REVIEW: &str = include_str!("prompts/delegated-review.md");

/// C0 controls and DEL removed before any user or repository text enters the payload
/// (`vimeflow:src/features/diff/services/feedbackDispatch.ts:14-28`); a comment keeps its newlines.
pub fn strip_controls(text: &str, keep_newlines: bool) -> String {
    text.chars()
        .filter(|c| (keep_newlines && *c == '\n') || !(c.is_control()))
        .collect()
}

/// `'<toplevel>'` with every `'` written as `'\''`, so the quoted command survives any shell.
pub fn quote_toplevel(toplevel: &str) -> String {
    format!("'{}'", strip_controls(toplevel, false).replace('\'', "'\\''"))
}

/// `unstaged`, `staged`, or `vs <label> @ <7 hex of the merge-base>`.
pub fn comparison_label(anchor: &Anchor) -> String {
    match &anchor.comparison {
        AnchorComparison::Worktree if anchor.key.staged => "staged".to_string(),
        AnchorComparison::Worktree => "unstaged".to_string(),
        AnchorComparison::Branch { merge_base, label } => format!(
            "vs {} @ {}",
            strip_controls(label, false),
            merge_base.chars().take(7).collect::<String>()
        ),
    }
}

fn side_word(side: Side) -> &'static str {
    match side {
        Side::Additions => "additions",
        Side::Deletions => "deletions",
    }
}

/// `<absolute path><place> (<side>) [<comparison>]`, the file form without a side.
fn target_of(toplevel: &str, anchor: &Anchor) -> String {
    let path = format!("{}/{}", strip_controls(toplevel, false), strip_controls(&anchor.key.path, false));
    let label = comparison_label(anchor);
    match anchor.span {
        Span::File => format!("{path} (file) [{label}]"),
        Span::Line => format!("{path}:{} ({}) [{label}]", anchor.line, side_word(anchor.side)),
        Span::Range { end } => format!("{path}:{}-{end} ({}) [{label}]", anchor.line, side_word(anchor.side)),
    }
}

pub struct Item<'a> {
    pub number: u32,
    pub comment: &'a Comment,
}

/// The review prompt: `formatFeedbackPayload` of the pin, plus one footer line when an item was
/// made in branch scope, because an absolute path alone selects no repository.
pub fn review(toplevel: &str, items: &[Item<'_>], nonce: &str) -> String {
    let count = items.len();
    let mut lines = vec![
        format!(
            "> Inline review — {count} item{}. Reply to each by its [#n].",
            if count == 1 { "" } else { "s" }
        ),
        ">".to_string(),
    ];
    let mut branch_item = false;
    for item in items {
        let comment = item.comment;
        branch_item |= matches!(comment.anchor.comparison, AnchorComparison::Branch { .. });
        lines.push(format!(
            "> [#{} · {}] {}",
            item.number,
            comment.category.label(),
            target_of(toplevel, &comment.anchor)
        ));
        for line in strip_controls(&comment.text, true).split('\n') {
            lines.push(format!("> ─ {line}"));
        }
        lines.push(format!("> → {}", comment.category.instruction()));
        lines.push(">".to_string());
    }
    lines.push("> ―".to_string());
    if branch_item {
        lines.push(format!(
            "> Items marked [vs <base> @ <id>] compare the working tree with that merge-base: `git -C {} diff <id> -- <path>` shows what I see.",
            quote_toplevel(toplevel)
        ));
    }
    lines.push("> When done, end your reply with this exact block, echoing the nonce verbatim.".to_string());
    lines.push("> status is one of: \"reply\" (answers a question), \"clarify\" (you need the user to answer — the thread awaits them), \"resolved\" (you made the change), \"deferred\" (punted for later; cite the issue # in text), \"rejected\" (declined).".to_string());
    lines.push("> <<<VIMEFLOW_REPLY".to_string());
    lines.push(format!(
        "> {{\"v\":1,\"nonce\":\"{}\",\"replies\":[{{\"id\":1,\"status\":\"reply\",\"text\":\"...\"}}]}}",
        strip_controls(nonce, false)
    ));
    lines.push("> VIMEFLOW_REPLY>>>".to_string());
    lines.join("\n")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestLine {
    pub path: String,
    pub staged: bool,
    pub untracked: bool,
}

pub enum RequestScope<'a> {
    Worktree,
    Branch { merge_base: &'a str },
}

fn request_line(toplevel: &str, file: &RequestLine) -> String {
    let path = strip_controls(&file.path, false);
    let base = format!("> ─ {path} ({}/{path})", strip_controls(toplevel, false));
    if file.untracked {
        format!("{base} (untracked — not in git diff; read the file, all lines are additions)")
    } else {
        base
    }
}

/// The review-request prompt: `formatReviewRequest` of the pin with `-C '<toplevel>'` in each
/// quoted command, one `branch diff` group in branch scope, then the pinned contract.
pub fn request(toplevel: &str, files: &[RequestLine], scope: RequestScope<'_>, nonce: &str) -> String {
    let count = files.len();
    let mut lines = vec![format!(
        "> Delegate a code review of {} {count} change{}:",
        if count == 1 { "this" } else { "these" },
        if count == 1 { "" } else { "s" }
    )];
    let quoted = quote_toplevel(toplevel);
    match scope {
        RequestScope::Branch { merge_base } => {
            lines.push(format!("> branch diff (`git -C {quoted} diff {merge_base}`):"));
            lines.extend(files.iter().map(|f| request_line(toplevel, f)));
        }
        RequestScope::Worktree => {
            let unstaged: Vec<_> = files.iter().filter(|f| !f.staged).collect();
            let staged: Vec<_> = files.iter().filter(|f| f.staged).collect();
            if !unstaged.is_empty() {
                lines.push(format!("> unstaged diff (`git -C {quoted} diff`):"));
                lines.extend(unstaged.iter().map(|f| request_line(toplevel, f)));
            }
            if !staged.is_empty() {
                lines.push(format!("> staged diff (`git -C {quoted} diff --cached`):"));
                lines.extend(staged.iter().map(|f| request_line(toplevel, f)));
            }
        }
    }
    lines.push(">".to_string());
    lines.push(
        DELEGATED_REVIEW
            .trim_end()
            .replace("{{NONCE}}", &strip_controls(nonce, false)),
    );
    lines.join("\n")
}

const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// Six characters of `[a-z0-9]`, Vimeflow's length and alphabet: a tag, not a secret.
pub fn nonce(counter: u64) -> String {
    use sha2::Digest;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let digest = sha2::Sha256::digest(format!("{nanos}:{}:{counter}", std::process::id()).as_bytes());
    digest
        .iter()
        .take(6)
        .map(|b| ALPHABET[usize::from(*b) % ALPHABET.len()] as char)
        .collect()
}

pub fn is_nonce(text: &str) -> bool {
    (6..=16).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_alphanumeric())
}
```

Replace `comments.rs`'s private `is_nonce` with `super::prompt::is_nonce` so there is one definition. Add `pub mod prompt;` to `src/engine/mod.rs`. `SessionConfig` gains `pub nonce: Arc<dyn Fn(u64) -> String + Send + Sync>` with `Arc::new(prompt::nonce)` in `production()`. A subtlety the tests pin: in `the_request_prompt_groups...`, `&text[head.len()..]` must equal the contract exactly, so `request` must not add a trailing newline and must join with `\n` only; and `DELEGATED_REVIEW.trim_end()` drops the file's final newline as `trimEnd()` does upstream.

- [ ] **Step 5: Run the module tests**

Run: `cargo test --locked --lib engine::prompt`
Expected: six pass. Then `scripts/port-check.sh "$VIMEFLOW"`: passes with the prompt comparison.

- [ ] **Step 6: Gates and commit**

```bash
git add src/engine/prompt.rs src/engine/prompts/delegated-review.md src/engine/mod.rs src/engine/comments.rs src/engine/session.rs scripts/port-check.sh scripts/port-check-selftest.sh
git commit -m "feat(engine): vimeflow's two prompts, with git -C and the pinned review contract"
```

### Task 5: Finishing, sending and copying, in the engine

Implements spec 10.4 whole (the send's six steps, the send lock, copy, the review request) and 10.7's send rows; 10.9 tests 4 and 5, and Review Focus item 5. The boxes are Task 8. Read 10.4 and 10.6 "Where the work runs" before starting.

**Files:**
- Create: `src/engine/dispatch.rs`
- Modify: `src/engine/comments.rs` (`Store::{claim, settle}`, `remove_request`), `src/actions/reuse.rs` (`with_lock_at`), `src/engine/types.rs` (`Snapshot`, `Command`), `src/engine/session.rs` (`Command::Send`, `Command::Copy`, `Done::Sent`, `Done::Copied`, the shared selection generation, `fingerprint`), `src/engine/mod.rs`

**Interfaces:**
- Consumes: Tasks 1-4; 0.0.4's `worktree::diff`, `worktree::untracked_diff`, `branch::diff`, the diff lane.
- Produces:

```rust
// src/actions/reuse.rs
/// `with_lock` on any lock file, waiting up to `wait` for it; `with_lock` is this on `split-panes.lock` for two seconds.
pub fn with_lock_at<T>(lock: &Path, wait: Duration, f: impl FnOnce() -> T) -> std::io::Result<T>;

// src/engine/dispatch.rs
pub const SEND_LOCK: &str = "send.lock";
/// Longer than one host call can take (ENGINE_WAIT) plus the Enter delay the lock outlives.
pub const LOCK_WAIT: Duration = Duration::from_secs(12);
/// The lock is held this long after the host answered: longer than the host's 300 ms Enter delay.
pub const ENTER_MARGIN: Duration = Duration::from_millis(500);
pub const REQUEST_BOUND: usize = 512 * 1024;
pub const OSC_LIMIT: usize = 100_000;
pub const CLIPBOARD_FILE: &str = "clipboard.md";
pub const NOTICE_IN_PROGRESS: &str = "a send is in progress";
pub const NOTICE_WAITING: &str = "another viewer is sending";
pub use super::comments::NOTICE_NOTHING;   // defined in Task 3's module, used by both
pub const NOTICE_NO_PENDING: &str = "nothing to send: no pending comments";

#[derive(Debug, Clone, PartialEq, Eq)] pub enum ReviewScope { File(FileKey), All }
#[derive(Debug, Clone, PartialEq, Eq)] pub enum SendKind { Feedback, Review { scope: ReviewScope } }
/// What a "send anyway" row stands for (spec 10.4): the status it named, the session it found restarted.
/// `restarted`: `None` accepts no restart; `Some(session)` accepts a restart into exactly that session, `Some(None)` one into a pane whose agent reports no session.
#[derive(Debug, Clone, PartialEq, Eq, Default)] pub struct Accepted { pub busy: bool, pub restarted: Option<Option<SessionRef>> }
#[derive(Debug, Clone, PartialEq, Eq)] pub struct SendRequest { pub kind: SendKind, pub accepted: Accepted }
#[derive(Debug, Clone, PartialEq, Eq)] pub enum CopyWhat { Review, Request { scope: ReviewScope }, Selection(String) }
#[derive(Debug, Clone, PartialEq, Eq)] pub struct CopyRequest { pub what: CopyWhat }
/// What the shell writes to the terminal once per `copy_seq`, and the notice to show.
#[derive(Debug, Clone, PartialEq, Eq)] pub struct CopyOut { pub osc: Option<String>, pub notice: String, pub urgent: bool }
/// A successful send's answer; the TUI words the notice from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendOutcome { pub kind: SendKind, pub items: u32, pub to: Destination, pub unconfirmed: bool, pub copy: Option<CopyOut> }

/// Diff-side hunk ranges of a parsed diff before the display cap: `(additions, deletions)`, each `(start, end)` inclusive.
pub fn ranges_of(file_diff: &FileDiff) -> (Vec<(u32, u32)>, Vec<(u32, u32)>);
/// The agent.prompt request line's length in bytes, the bound of 10.4 step 3.
pub fn encoded_len(pane: &str, text: &str) -> usize;
/// `ESC ] 52 ; c ; <base64> BEL`, or None above OSC_LIMIT bytes of payload.
pub fn osc52(text: &str) -> Option<String>;

// src/engine/comments.rs: Store gains
/// Under the lock: reread and replay, take every Pending or Unconfirmed record in creation order,
/// number them, choose a nonce no retained record of either file carries (`make` is called with a
/// rising counter until one is free), let `build` make the text and check the bound, stamp them
/// Sending, write. Without a state directory the array is the store and nothing is locked.
pub fn claim(&mut self, now: u64, to: &Destination, make: &dyn Fn(u64) -> String, counter: &mut u64,
             guard: &dyn Fn() -> Result<(), String>,
             build: impl FnOnce(&[(u32, &Comment)], &str) -> Result<String, String>) -> Result<Claimed, String>;
// `guard` runs under the lock before anything is stamped: the send task passes its selection-generation check.
#[derive(Debug, Clone, PartialEq, Eq)] pub struct Claimed { pub comments: Vec<Comment>, pub text: String, pub nonce: String }
/// Under the lock (or in memory): every record still Sending under `nonce` becomes `Sent`,
/// `Unconfirmed`, or what it was.
pub fn settle(&mut self, now: u64, nonce: &str, outcome: Settlement) -> Result<(), String>;
#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum Settlement { Sent, Unconfirmed, Failed }
/// Under the lock: `guard`, both files reread, a nonce no retained record carries, then `build(nonce)`, which returns
/// the record and the text it goes with or a refusal (the bound) that writes nothing; the record is appended.
pub fn record_request_fresh(state_dir: &Path, toplevel: &str, make: &dyn Fn(u64) -> String, counter: &mut u64, guard: &dyn Fn() -> Result<(), String>, build: impl FnOnce(&str) -> Result<(RequestRecord, String), String>) -> std::io::Result<(String /* the nonce */, String /* the text */)>;
pub fn remove_request(state_dir: &Path, toplevel: &str, nonce: &str) -> std::io::Result<()>;
/// `clipboard.md` replaced under the state lock, so two copies never truncate each other's temporary file.
pub fn write_clipboard(state_dir: &Path, text: &str) -> std::io::Result<PathBuf>;

// src/engine/types.rs: Snapshot gains
pub send_seq: u64,
pub send_error: Option<String>,
pub send_refusal: Option<Refusal>,      // what kind of refusal send_error is; None on success (the box reads it, Task 8)
pub send_outcome: Option<SendOutcome>,
pub send_waiting: bool,                 // another viewer holds send.lock; the box reads NOTICE_WAITING
pub copy_seq: u64,
pub copy: Option<Arc<CopyOut>>,
// Command gains
Send(SendRequest),
Copy(CopyRequest),
```

- [ ] **Step 1: The lock helper**

In `src/actions/reuse.rs`, rename the body of `with_lock` into `with_lock_at` and keep `with_lock` as the two-second call on `split-panes.lock`:

```rust
pub fn with_lock<T>(state_dir: &Path, f: impl FnOnce() -> T) -> std::io::Result<T> {
    std::fs::create_dir_all(state_dir)?;
    with_lock_at(&state_dir.join("split-panes.lock"), Duration::from_secs(2), f)
}

/// An exclusive `flock` on `lock`, waited for up to `wait`; released when the file drops, panics included.
pub fn with_lock_at<T>(lock: &Path, wait: Duration, f: impl FnOnce() -> T) -> std::io::Result<T> {
    use std::os::unix::io::AsRawFd;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock)?;
    let start = Instant::now();
    while unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let error = std::io::Error::last_os_error();
        if !matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) {
            return Err(error);
        }
        if start.elapsed() >= wait {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                format!("{} is busy", lock.file_name().unwrap_or_default().to_string_lossy()),
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Ok(f())
}
```

(`use std::time::{Duration, Instant};` at the top.) The busy message changes from `split reuse lock is busy` to `split-panes.lock is busy`; grep the tests for the old text (`tests/actions_tier_a.rs`) and update the one assertion if it exists.

- [ ] **Step 2: Claim and settle on the store, tests first**

Add to `comments.rs`'s tests:

```rust
    #[test]
    fn retries_keep_every_uncertain_stamp_and_a_late_word_settles_the_right_one() {
        // A was uncertain; retry B times out; retry C fails; then B's late failure arrives, then A's late success.
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        let mut a = comment("c", "x", 1);
        a.state = CommentState::Unconfirmed { stamp: stamp("aaaaaa"), before: Vec::new() };
        store.transact(Operation::Add(a), 1_000).unwrap();
        let to = Destination::clipboard();
        let ok = || Ok(());
        let mut counter = 0;
        let make_b = |_: u64| "bbbbbb".to_string();
        store.claim(1_001, &to, &make_b, &mut counter, &ok, |_, _| Ok("b".into())).unwrap();
        assert!(matches!(&store.comments()[0].state, CommentState::Sending { stamp, before } if stamp.nonce == "bbbbbb" && before.iter().map(|b| b.nonce.as_str()).collect::<Vec<_>>() == ["aaaaaa"]));
        store.settle(1_002, "bbbbbb", Settlement::Unconfirmed).unwrap();
        let make_c = |_: u64| "cccccc".to_string();
        store.claim(1_003, &to, &make_c, &mut counter, &ok, |_, _| Ok("c".into())).unwrap();
        assert!(matches!(&store.comments()[0].state, CommentState::Sending { stamp, before } if stamp.nonce == "cccccc" && before.iter().map(|b| b.nonce.as_str()).collect::<Vec<_>>() == ["bbbbbb", "aaaaaa"]), "the whole chain is kept");
        store.settle(1_004, "cccccc", Settlement::Failed).unwrap();
        assert!(matches!(&store.comments()[0].state, CommentState::Unconfirmed { stamp, before } if stamp.nonce == "bbbbbb" && before.len() == 1), "C's failure restores B, with A behind it");
        store.settle(1_005, "bbbbbb", Settlement::Failed).unwrap();
        assert!(matches!(&store.comments()[0].state, CommentState::Unconfirmed { stamp, before } if stamp.nonce == "aaaaaa" && before.is_empty()), "B's late failure restores A, not Pending: A may have arrived");
        store.settle(1_006, "aaaaaa", Settlement::Sent).unwrap();
        assert!(matches!(&store.comments()[0].state, CommentState::Sent(st) if st.nonce == "aaaaaa"));
        // A late success for an attempt deeper in the chain settles the record as sent under that attempt.
        let mut d = comment("d", "y", 2);
        d.state = CommentState::Unconfirmed { stamp: stamp("eeeeee"), before: vec![stamp("dddddd")] };
        store.transact(Operation::Add(d), 1_007).unwrap();
        store.settle(1_008, "dddddd", Settlement::Sent).unwrap();
        assert!(matches!(&store.comments()[1].state, CommentState::Sent(st) if st.nonce == "dddddd"));
        // While a retry is still sending: the earlier attempt's late success makes the record sent under
        // it, and the retry's own failure then changes nothing; its late failure instead drops it from the
        // chain, so the retry's failure lands on Pending, not on an attempt that never arrived.
        let mut f = comment("f", "z", 3);
        f.state = CommentState::Unconfirmed { stamp: stamp("ffffff"), before: Vec::new() };
        store.transact(Operation::Add(f), 1_009).unwrap();
        let make_g = |_: u64| "gggggg".to_string();
        store.claim(1_010, &to, &make_g, &mut counter, &ok, |_, _| Ok("g".into())).unwrap();
        store.settle(1_011, "ffffff", Settlement::Sent).unwrap();
        assert!(matches!(&store.comments()[2].state, CommentState::Sent(st) if st.nonce == "ffffff"), "F arrived while G was sending");
        store.settle(1_012, "gggggg", Settlement::Failed).unwrap();
        assert!(matches!(&store.comments()[2].state, CommentState::Sent(st) if st.nonce == "ffffff"), "G's failure undoes nothing");
        let mut h = comment("h", "w", 4);
        h.state = CommentState::Unconfirmed { stamp: stamp("hhhhhh"), before: Vec::new() };
        store.transact(Operation::Add(h), 1_013).unwrap();
        let make_i = |_: u64| "iiiiii".to_string();
        store.claim(1_014, &to, &make_i, &mut counter, &ok, |_, _| Ok("i".into())).unwrap();
        store.settle(1_015, "hhhhhh", Settlement::Failed).unwrap();
        assert!(matches!(&store.comments()[3].state, CommentState::Sending { stamp, before } if stamp.nonce == "iiiiii" && before.is_empty()), "H's late failure left the chain");
        store.settle(1_016, "iiiiii", Settlement::Failed).unwrap();
        assert!(store.comments()[3].is_pending(), "neither attempt arrived");
        // Without a state directory the array is the store, and its chains count against a new nonce too.
        let mut bare = Store::open(None, "/repo", 1_000).0;
        let mut j = comment("j", "v", 5);
        j.state = CommentState::Unconfirmed { stamp: stamp("kkkkkk"), before: vec![stamp("jjjjjj")] };
        // The first add of a session without a state directory says so once, and keeps the record.
        assert_eq!(bare.transact(Operation::Add(j), 1_017).unwrap_err(), format!("{NOTICE_NOT_REMEMBERED}no state directory"));
        assert_eq!(bare.comments().len(), 1);
        bare.transact(Operation::Add(comment("l", "u", 6)), 1_017).unwrap();
        let colliding = |c: u64| ["jjjjjj", "kkkkkk", "llllll"][c as usize - 1].to_string();
        let mut counter = 0;
        let claimed = bare.claim(1_018, &to, &colliding, &mut counter, &ok, |_, _| Ok("l".into())).unwrap();
        assert_eq!((claimed.nonce.as_str(), counter), ("llllll", 3), "a historical nonce is as taken as a current one");
    }

    #[test]
    fn a_claim_takes_pending_and_unconfirmed_in_order_and_never_anothers_sending() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        store.transact(Operation::Add(comment("p1", "first", 1)), 1_010).unwrap();
        let mut unconfirmed = comment("u1", "second", 2);
        unconfirmed.state = CommentState::Unconfirmed { before: Vec::new(), stamp: stamp("oldone") };
        store.transact(Operation::Add(unconfirmed), 1_010).unwrap();
        let mut theirs = comment("s1", "third", 3);
        // Claimed by another viewer a second ago: still theirs, not expired.
        theirs.state = CommentState::Sending { stamp: Stamp { at: 1_010, ..stamp("theirs") }, before: Vec::new() };
        store.transact(Operation::Add(theirs), 1_010).unwrap();
        store.transact(Operation::Add(comment("p2", "fourth", 4)), 1_010).unwrap();
        // An older comment that landed late in the array (a journal replay) is numbered by its creation time.
        store.transact(Operation::Add(comment("p0", "zeroth", 0)), 1_010).unwrap();
        // Two made in the same second (`created_at` is in seconds) are ordered by their ids, which
        // `new_id` makes increase with time: the one made first goes first, whatever the array order.
        let (first, second) = (new_id(), new_id());
        store.transact(Operation::Add(Comment { id: second.clone(), ..comment("x", "made second", 5) }), 1_010).unwrap();
        store.transact(Operation::Add(Comment { id: first.clone(), ..comment("x", "made first", 5) }), 1_010).unwrap();
        let to = Destination::Pane { pane: "w4:p2".into(), agent: "codex".into(), session: None };
        let make = |counter: u64| format!("n{counter:05}");
        let ok = || Ok(());
        let mut counter = 0;
        let claimed = store
            .claim(1_011, &to, &make, &mut counter, &ok, |items, nonce| {
                let numbered: Vec<_> = items.iter().map(|(n, c)| format!("{n}:{}", c.id)).collect();
                Ok(format!("{nonce}|{}", numbered.join(",")))
            })
            .unwrap();
        // `theirs` and `oldone` are retained nonces, but `n00001` is free: the first candidate is taken.
        assert_eq!(claimed.nonce, "n00001");
        // Numbered by creation time, not array order: p0 first, p2 and the same-second pair last, in order.
        assert_eq!(claimed.text, format!("n00001|1:p0,2:p1,3:u1,4:p2,5:{first},6:{second}"));
        assert_eq!(claimed.comments.len(), 6);
        for c in &claimed.comments {
            match &c.state {
                CommentState::Sending { stamp, before } => {
                    assert_eq!((stamp.nonce.as_str(), &stamp.to), ("n00001", &to));
                    let expected_item = ["p0", "p1", "u1", "p2", first.as_str(), second.as_str()].iter().position(|id| *id == c.id).unwrap() as u32 + 1;
                    assert_eq!(stamp.item, expected_item);
                    assert_eq!(!before.is_empty(), c.id == "u1", "only the unconfirmed one remembers its earlier stamp");
                }
                other => panic!("{other:?}"),
            }
        }
        assert!(matches!(&store.comments()[2].state, CommentState::Sending { stamp, .. } if stamp.nonce == "theirs"));
        // Nothing eligible: another viewer took it all.
        let mut other = open(dir.path());
        assert_eq!(other.claim(1_012, &to, &make, &mut counter, &ok, |_, _| Ok(String::new())).unwrap_err(), NOTICE_NOTHING);
        // With something to claim: a nonce a retained record carries is passed over inside the same lock,
        // a refusal from `build` stamps nothing, and a guard that refuses stamps nothing either.
        other.transact(Operation::Add(comment("p3", "fifth", 5)), 1_012).unwrap();
        let colliding = |counter: u64| if counter == 1 { "n00001".to_string() } else { format!("m{counter:05}") };
        let mut counter = 0;
        assert_eq!(other.claim(1_013, &to, &colliding, &mut counter, &ok, |_, _| Err("too large".to_string())).unwrap_err(), "too large");
        assert_eq!(counter, 2, "n00001 was tried and passed over before build refused");
        assert!(other.comments().iter().all(|c| !matches!(&c.state, CommentState::Sending { stamp, .. } if stamp.nonce == "m00002")), "a refusal inside build stamps nothing");
        let changed = || Err("the target changed; press Y again".to_string());
        assert_eq!(other.claim(1_014, &to, &make, &mut counter, &changed, |_, _| Ok(String::new())).unwrap_err(), "the target changed; press Y again");
        assert!(other.comments().iter().find(|c| c.id == "p3").unwrap().is_pending());
        // A write that fails leaves the array as the file has it: nothing shows Sending.
        other.fail_writes_for_tests(true);
        assert!(other.claim(1_015, &to, &make, &mut counter, &ok, |_, _| Ok(String::new())).is_err());
        assert!(other.comments().iter().find(|c| c.id == "p3").unwrap().is_pending(), "a failed write never shows a claim the file does not have");
        other.fail_writes_for_tests(false);
        // Another worktree's array survives a claim and a settlement.
        let mut elsewhere = Store::open(Some(dir.path().to_path_buf()), "/other", 1_000).0;
        elsewhere.transact(Operation::Add(comment("e1", "theirs", 1)), 1_015).unwrap();
        other.transact(Operation::Add(comment("p4", "sixth", 6)), 1_016).unwrap();
        let claimed = other.claim(1_017, &to, &make, &mut counter, &ok, |_, nonce| Ok(nonce.to_string())).unwrap();
        other.settle(1_018, &claimed.nonce, Settlement::Sent).unwrap();
        elsewhere.refresh(1_019);
        assert_eq!(elsewhere.comments().iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["e1"], "the other worktree's records were dropped by a write");
        // Without a state directory the claim runs on the array alone.
        let mut bare = Store::open(None, "/repo", 1_000).0;
        bare.transact(Operation::Add(comment("x", "bare", 1)), 1_000).unwrap_err();
        let claimed = bare.claim(1_001, &Destination::clipboard(), &make, &mut 0, &ok, |_, nonce| Ok(nonce.to_string())).unwrap();
        assert_eq!(claimed.comments.len(), 1);
        bare.settle(1_002, &claimed.nonce, Settlement::Unconfirmed).unwrap();
        assert!(matches!(bare.comments()[0].state, CommentState::Unconfirmed { .. }));
    }

    #[test]
    fn settlement_touches_only_this_nonce_and_restores_what_a_retry_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        store.transact(Operation::Add(comment("p1", "a", 1)), 10).unwrap();
        let mut unconfirmed = comment("u1", "b", 2);
        unconfirmed.state = CommentState::Unconfirmed { before: Vec::new(), stamp: stamp("oldone") };
        store.transact(Operation::Add(unconfirmed), 10).unwrap();
        let to = Destination::clipboard();
        let fixed = |name: &'static str| move |_: u64| name.to_string();
        let ok = || Ok(());
        store.claim(1_011, &to, &fixed("aaa111"), &mut 0, &ok, |_, _| Ok(String::new())).unwrap();
        // Another viewer's claim in between is never settled by this one.
        let mut other = open(dir.path());
        other.transact(Operation::Add(comment("p9", "c", 9)), 1_012).unwrap();
        other.claim(1_012, &to, &fixed("bbb222"), &mut 0, &ok, |_, _| Ok(String::new())).unwrap();
        store.settle(1_013, "aaa111", Settlement::Failed).unwrap();
        let by_id = |store: &Store, id: &str| store.comments().iter().find(|c| c.id == id).unwrap().state.clone();
        assert_eq!(by_id(&store, "p1"), CommentState::Pending);
        assert!(matches!(by_id(&store, "u1"), CommentState::Unconfirmed { stamp: s, .. } if s.nonce == "oldone"), "the earlier stamp came back");
        assert!(matches!(by_id(&store, "p9"), CommentState::Sending { stamp, .. } if stamp.nonce == "bbb222"));
        store.claim(1_014, &to, &fixed("ccc333"), &mut 0, &ok, |_, _| Ok(String::new())).unwrap();
        store.settle(1_015, "ccc333", Settlement::Unconfirmed).unwrap();
        assert!(matches!(by_id(&store, "p1"), CommentState::Unconfirmed { stamp: s, .. } if s.nonce == "ccc333"));
        // A late answer settles records the timeout already marked unconfirmed (spec 10.6).
        store.settle(1_016, "ccc333", Settlement::Sent).unwrap();
        assert!(matches!(by_id(&store, "p1"), CommentState::Sent(s) if s.nonce == "ccc333"));
        store.transact(Operation::Add(comment("p2", "again", 16)), 1_016).unwrap();
        store.claim(1_016, &to, &fixed("ddd444"), &mut 0, &ok, |_, _| Ok(String::new())).unwrap();
        store.settle(1_017, "ddd444", Settlement::Sent).unwrap();
        assert!(matches!(by_id(&store, "p2"), CommentState::Sent(s) if s.nonce == "ddd444" && s.item == 1));
        assert!(matches!(by_id(&store, "u1"), CommentState::Sent(s) if s.nonce == "ccc333"), "a sent record is never claimed again");
        // An unconfirmed record that was edited is pending again under its new text; a late success
        // for its old nonce finds no record and settles nothing.
        let mut unsure = comment("e1", "old text", 17);
        unsure.state = CommentState::Unconfirmed { stamp: stamp("ggg777"), before: Vec::new() };
        store.transact(Operation::Add(unsure.clone()), 1_017).unwrap();
        store.transact(Operation::Edit { id: "e1".into(), category: Category::Bug, text: "new text".into(), seen: unsure }, 1_017).unwrap();
        store.settle(1_017, "ggg777", Settlement::Sent).unwrap();
        assert!(by_id(&store, "e1").is_pending_state(), "the late success must not mark the new text sent");
        // A failed settlement write leaves the array as the file has it.
        store.transact(Operation::Add(comment("p3", "later", 18)), 1_018).unwrap();
        store.claim(1_018, &to, &fixed("fff666"), &mut 0, &ok, |_, _| Ok(String::new())).unwrap();
        store.fail_writes_for_tests(true);
        assert!(store.settle(1_019, "fff666", Settlement::Sent).is_err());
        assert!(matches!(by_id(&store, "p3"), CommentState::Sending { .. }), "the screen never shows a settlement the file does not have");
        store.fail_writes_for_tests(false);
        // The cap never refuses a rollback: fifty pending plus fifty returning is allowed.
        let mut full = open(dir.path());
        for i in 0..50 {
            let mut c = comment(&format!("x{i}"), "t", 20);
            c.state = CommentState::Sending { stamp: Stamp { at: 1_020, ..stamp("eee555") }, before: Vec::new() };
            full.comments_mut_for_tests().push(c);
        }
        full.write_for_tests();
        full.settle(1_021, "eee555", Settlement::Failed).unwrap();
        assert_eq!(full.comments().iter().filter(|c| c.is_pending()).count(), 50);
    }

    #[test]
    fn a_claim_is_refused_while_the_journal_is_not_empty() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        store.transact(Operation::Add(comment("p1", "a", 1)), 10).unwrap();
        store.fail_writes_for_tests(true);
        store.transact(Operation::Add(comment("p2", "b", 2)), 11).unwrap_err();
        store.fail_writes_for_tests(false);
        let error = store.claim(1_012, &Destination::clipboard(), &|c| format!("n{c:05}"), &mut 0, &|| Ok(()), |_, _| Ok(String::new())).unwrap_err();
        assert!(error.starts_with("comments not saved: "), "{error}");
        assert!(store.comments().iter().all(|c| c.is_pending()));
    }
```

Then the methods, after `refresh`:

```rust
    /// Spec 10.4 steps 2 and 3 in one transaction: the nonce is chosen against the records as they
    /// are under the lock, so no viewer that waited on `send.lock` can have introduced it since.
    /// `build` receives the eligible records numbered `[#1..n]` and the nonce, and returns the text
    /// to send, or a refusal (the bound) that stamps nothing.
    pub fn claim(
        &mut self,
        now: u64,
        to: &Destination,
        make: &dyn Fn(u64) -> String,
        counter: &mut u64,
        guard: &dyn Fn() -> Result<(), String>,
        build: impl FnOnce(&[(u32, &Comment)], &str) -> Result<String, String>,
    ) -> Result<Claimed, String> {
        if !self.journal.is_empty() {
            return Err(format!(
                "comments not saved: {}; fix it before sending",
                self.journal_reason.clone().unwrap_or_else(|| "the store could not be written".into())
            ));
        }
        let mut build = Some(build);
        let mut body = |store: &mut Self| -> Result<Claimed, TxError> {
            if store.state_dir.is_some() {
                store.read_and_apply(None, now)?;
            } else {
                expire(&mut store.comments, now);
            }
            // The caller's last word before anything is stamped: the send's selection generation.
            guard().map_err(TxError::Refused)?;
            let used = match &store.state_dir {
                Some(dir) => nonces_in_use(dir, &store.comments).map_err(|e| TxError::Io(format!("nonces: {e}")))?,
                None => nonces_of(&store.comments),
            };
            let nonce = loop {
                *counter += 1;
                let candidate = make(*counter);
                if !used.contains(&candidate) {
                    break candidate;
                }
            };
            // Creation order, by `created_at` then id: a journal replay can leave the array out of order.
            let mut ordered: Vec<&Comment> = store
                .comments
                .iter()
                .filter(|c| matches!(c.state, CommentState::Pending | CommentState::Unconfirmed { .. }))
                .collect();
            ordered.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
            let eligible: Vec<(u32, &Comment)> = ordered.into_iter().enumerate().map(|(i, c)| (i as u32 + 1, c)).collect();
            if eligible.is_empty() {
                return Err(TxError::Refused(NOTICE_NOTHING.to_string()));
            }
            let text = (build.take().expect("built once"))(&eligible, &nonce).map_err(TxError::Refused)?;
            let ids: Vec<(String, u32)> = eligible.iter().map(|(n, c)| (c.id.clone(), *n)).collect();
            // Stamped on a working copy: the array shows the claim only once the file holds it.
            let mut working = store.comments.clone();
            let mut claimed = Vec::new();
            for comment in &mut working {
                if let Some((_, item)) = ids.iter().find(|(id, _)| id == &comment.id) {
                    // The stamps a retry replaces are the ones that may have arrived: the unconfirmed one
                    // it found, then that one's own chain, newest first.
                    let before = match &comment.state {
                        CommentState::Unconfirmed { stamp, before } => std::iter::once(stamp.clone()).chain(before.iter().cloned()).collect(),
                        _ => Vec::new(),
                    };
                    comment.state = CommentState::Sending {
                        stamp: Stamp { at: now, nonce: nonce.clone(), item: *item, to: to.clone() },
                        before,
                    };
                    claimed.push(comment.clone());
                }
            }
            if store.state_dir.is_some() {
                store.write_locked_with(&working)?;
            }
            store.comments = working;
            Ok(Claimed { comments: claimed, text, nonce })
        };
        let outcome = if self.state_dir.is_some() {
            self.locked(|store| body(store))
        } else {
            // The array is the store (spec 10.3): a claim and its settlement run in memory.
            Ok(body(self))
        };
        match outcome {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(TxError::Refused(e) | TxError::Io(e))) | Err(TxError::Refused(e) | TxError::Io(e)) => Err(e),
        }
    }

    /// Spec 10.4 step 5: records still `Sending` under `nonce`, and `Unconfirmed` ones when a late
    /// answer arrives after the engine's wait (spec 10.6); never the cap. On a working copy, so a
    /// failed write leaves the array as the file has it.
    pub fn settle(&mut self, now: u64, nonce: &str, outcome: Settlement) -> Result<(), String> {
        let body = |store: &mut Self| -> Result<(), TxError> {
            if store.state_dir.is_some() {
                store.read_and_apply(None, now)?;
            }
            let mut working = store.comments.clone();
            let mut changed = false;
            for comment in &mut working {
                // A late word about an earlier attempt, while a newer one is sending or unconfirmed: a
                // success means that text reached the agent (the record is sent under that attempt; a
                // newer attempt arriving too is a duplicate, not a loss), a definite failure drops the
                // attempt from the chain, so a later rollback never restores what never arrived.
                if let CommentState::Sending { before, .. } | CommentState::Unconfirmed { before, .. } = &mut comment.state {
                    if let Some(index) = before.iter().position(|b| b.nonce == nonce) {
                        let arrived = match outcome {
                            Settlement::Sent => Some(before[index].clone()),
                            Settlement::Failed => {
                                before.remove(index);
                                None
                            }
                            Settlement::Unconfirmed => None,
                        };
                        if let Some(earlier) = arrived {
                            comment.state = CommentState::Sent(earlier);
                        }
                        changed = true;
                        continue;
                    }
                }
                let (stamp, before) = match &comment.state {
                    CommentState::Sending { stamp, before } if stamp.nonce == nonce => (stamp.clone(), before.clone()),
                    // A late answer: the timeout marked it unconfirmed and kept `before`; the host's word is final.
                    CommentState::Unconfirmed { stamp, before } if stamp.nonce == nonce && outcome != Settlement::Unconfirmed => (stamp.clone(), before.clone()),
                    _ => continue,
                };
                comment.state = match outcome {
                    Settlement::Sent => CommentState::Sent(stamp),
                    Settlement::Unconfirmed => CommentState::Unconfirmed { stamp, before },
                    // Nothing arrived this time; the earlier sends may have: back to the newest of them,
                    // with the rest of the chain, or to pending when there were none.
                    Settlement::Failed => match before.split_first() {
                        Some((earlier, rest)) => CommentState::Unconfirmed { stamp: earlier.clone(), before: rest.to_vec() },
                        None => CommentState::Pending,
                    },
                };
                changed = true;
            }
            if changed {
                if store.state_dir.is_some() {
                    store.write_locked_with(&working)?;
                }
                store.comments = working;
            }
            Ok(())
        };
        let result = if self.state_dir.is_some() {
            self.locked(body)
        } else {
            Ok(body(self))
        };
        match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(TxError::Refused(e) | TxError::Io(e))) | Err(TxError::Refused(e) | TxError::Io(e)) => Err(e),
        }
    }
```

`write_locked_with(&working)` is the write half of `read_and_apply`, factored out to take the array to write: it writes `self.others` (every other worktree's array as `read_and_apply` last read it) plus `{toplevel: working}`, so a claim or a settlement never drops another worktree's records; `read_and_apply` sets `self.others` from `parse_comments`'s remainder on every read. A late `Failed` settlement of an `Unconfirmed` record restores the `before` it kept through the timeout, or `Pending` when it had none: the failure says nothing about the earlier send that may have arrived, and that correlation is what a reply would still match. `journal_reason: Option<String>` is set by `transact` when it journals an operation (the `e` of the `Io` arm) and cleared when the journal empties. Add `Settlement` beside `Operation`. `Claimed { comments, text, nonce }` is a plain struct beside it, deriving `Debug, Clone, PartialEq, Eq` (the tests `unwrap_err` a `Result<Claimed, _>`). `record_request_fresh`, `remove_request` and `write_clipboard` are Task 3's (its Step 3 defines them), used here unchanged.

Run: `cargo test --locked --lib engine::comments`. Expected: the three new tests pass with the twelve of Task 3.

- [ ] **Step 3: The dispatch module**

Create `src/engine/dispatch.rs`:

```rust
//! Sending and copying (spec 10.4): two gates, one lock, a claim before the call, a settlement by nonce.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::Semaphore;

use super::comments::{self, RequestFile, RequestRecord, Settlement, Store};
use super::host::{self, HostClient, SessionRef};
use super::prompt;
use super::target::{self, Destination, Target, TargetState};
use super::{branch, worktree, Comparison, DiffState, FileKey, LoadedDiff, Snapshot};
use crate::actions::reuse;
use crate::git::ChangedFileStatus;

pub const SEND_LOCK: &str = "send.lock";
pub const LOCK_WAIT: Duration = Duration::from_secs(12);
pub const ENTER_MARGIN: Duration = Duration::from_millis(500);
pub const REQUEST_BOUND: usize = 512 * 1024;
pub const OSC_LIMIT: usize = 100_000;
pub const CLIPBOARD_FILE: &str = "clipboard.md";
pub const NOTICE_IN_PROGRESS: &str = "a send is in progress";
pub const NOTICE_WAITING: &str = "another viewer is sending";
pub use super::comments::NOTICE_NOTHING;
pub const NOTICE_NO_PENDING: &str = "nothing to send: no pending comments";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewScope {
    File(FileKey),
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendKind {
    Feedback,
    Review { scope: ReviewScope },
}

/// What a "send anyway" row stands for: the status it named, the session it found restarted.
/// `restarted` is `None` for no restart accepted, `Some(session)` for a restart into exactly that
/// session, `Some(None)` for one into a pane whose agent reports no session at all.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Accepted {
    pub busy: bool,
    pub restarted: Option<Option<SessionRef>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendRequest {
    pub kind: SendKind,
    pub accepted: Accepted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyWhat {
    Review,
    Request { scope: ReviewScope },
    Selection(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyRequest {
    pub what: CopyWhat,
}

/// What the shell writes to the terminal once per `copy_seq`, and the notice to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyOut {
    pub osc: Option<String>,
    pub notice: String,
    pub urgent: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendOutcome {
    pub kind: SendKind,
    pub items: u32,
    pub to: Destination,
    pub unconfirmed: bool,
    pub copy: Option<CopyOut>,
}

/// Everything a send or copy needs from the session, taken when the command arrives (and kept with a
/// queued send, so the send runs against the target it was confirmed for).
pub struct Context {
    pub toplevel: String,
    pub state_dir: Option<PathBuf>,
    pub host: Option<Arc<dyn HostClient>>,
    /// How long a host call may run before its outcome is uncertain: `ENGINE_WAIT`, or a test's seam.
    pub host_wait: Duration,
    /// Where a late answer to a timed-out prompt reports, settled by the session by nonce.
    pub late: tokio::sync::mpsc::UnboundedSender<(String, Settlement)>,
    pub socket_path: Option<String>,
    pub target: Option<Target>,
    /// The session's side map for worktree renames, `(path, staged) -> source` (`State.worktree_renames`).
    pub worktree_renames: std::collections::BTreeMap<(String, bool), String>,
    /// The generation this command was issued under, and the live one to compare with at the claim.
    pub generation: u64,
    pub latest_generation: Arc<AtomicU64>,
    pub nonce: Arc<dyn Fn(u64) -> String + Send + Sync>,
    pub nonce_counter: u64,
    pub snapshot: Arc<Snapshot>,
    pub lane: Arc<Semaphore>,
    /// The time, sampled where a stamp is made, not when the command arrived: a send may queue first.
    pub clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    /// A test's seam, run on the blocking pool after the transaction and before the last generation check.
    pub send_gate: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// The engine's part of the answer, beside the store it borrowed.
pub struct Finished {
    pub store: Option<Store>,
    pub outcome: Result<SendOutcome, (String, Refusal)>,
    /// A copy that reached the terminal or the file although the outcome is an error (the stamps could
    /// not be written after it): the session publishes it so the shell still writes the sequence.
    pub copy: Option<CopyOut>,
    /// A `Restarted` the reviewer accepted: the target record adopts this session (spec 10.4 step 1),
    /// if the selection is still the one this send was made for.
    pub adopt_session: Option<Option<SessionRef>>,
    /// The selection generation the send was issued under.
    pub generation: u64,
    /// Where the nonce counter stands after this send; the session continues from it.
    pub counter: u64,
}

/// One blocking store or file operation, off the async executor.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.expect("blocking task")
}

pub fn encoded_len(pane: &str, text: &str) -> usize {
    serde_json::to_string(&json!({ "id": "0", "method": "agent.prompt", "params": { "target": pane, "text": text } }))
        .map(|s| s.len() + 1)
        .unwrap_or(usize::MAX)
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}

/// `ESC ] 52 ; c ; <base64> BEL`, or `None` when the text is over what terminals take.
pub fn osc52(text: &str) -> Option<String> {
    (text.len() <= OSC_LIMIT).then(|| format!("\x1b]52;c;{}\x07", base64(text.as_bytes())))
}

/// Diff-side hunk ranges, inclusive: `(additions, deletions)`, from the parsed file before the
/// display cap of 3.2 cuts hunks, so a request's ranges cover the whole file.
pub fn ranges_of(file_diff: &crate::git::FileDiff) -> (Vec<(u32, u32)>, Vec<(u32, u32)>) {
    let mut additions = Vec::new();
    let mut deletions = Vec::new();
    for hunk in &file_diff.hunks {
        if hunk.new_lines > 0 {
            additions.push((hunk.new_start, hunk.new_start + hunk.new_lines - 1));
        }
        if hunk.old_lines > 0 {
            deletions.push((hunk.old_start, hunk.old_start + hunk.old_lines - 1));
        }
    }
    (additions, deletions)
}

fn comparison_of(snapshot: &Snapshot) -> comments::AnchorComparison {
    match (&snapshot.scope, &snapshot.base) {
        (super::Scope::Branch, Some(base)) if base.merge_base.is_some() => comments::AnchorComparison::Branch {
            merge_base: base.merge_base.clone().unwrap_or_default(),
            label: base.label().to_string(),
        },
        _ => comments::AnchorComparison::Worktree,
    }
}

/// One file of a review request: its prompt line and its ranges, the diff loaded when it is not the
/// one on screen (spec 10.4 "Request review"), under the diff lane, one row after another.
pub async fn request_files(ctx: &Context, scope: &ReviewScope) -> Result<(Vec<prompt::RequestLine>, Vec<RequestFile>), String> {
    let snapshot = &ctx.snapshot;
    let comparison = comparison_of(snapshot);
    let ready = match &snapshot.diff {
        DiffState::Ready(d) => Some(d.clone()),
        _ => None,
    };
    let keys: Vec<FileKey> = match scope {
        ReviewScope::File(key) => vec![key.clone()],
        ReviewScope::All => snapshot.files.iter().map(FileKey::of).collect(),
    };
    if keys.is_empty() {
        return Err("nothing to review".to_string());
    }
    let mut lines = Vec::new();
    let mut files = Vec::new();
    for key in keys {
        let file = snapshot.files.iter().find(|f| FileKey::of(f) == key);
        let untracked = file.is_some_and(|f| matches!(f.status, ChangedFileStatus::Untracked));
        // The diff on screen serves only when the cap cut nothing from it; otherwise the row is read
        // again like the others, so the recorded ranges cover the whole file.
        let on_screen = ready.as_ref().filter(|d| d.key == key && d.truncated_lines == 0).map(|d| ranges_of(&d.file_diff));
        let ranges = match on_screen {
            Some(ranges) => Some(ranges),
            None => {
                if matches!(scope, ReviewScope::File(_)) && !ready.as_ref().is_some_and(|d| d.key == key) {
                    return Err("no diff loaded for this file".to_string());
                }
                let _lane = ctx.lane.acquire().await.map_err(|e| e.to_string())?;
                // The diff task's rule (session.rs): worktree renames are looked up by side, branch ones by path.
                let old = match &comparison {
                    comments::AnchorComparison::Worktree => ctx.worktree_renames.get(&(key.path.clone(), key.staged)).cloned(),
                    comments::AnchorComparison::Branch { .. } => snapshot.rename_sources.get(&key.path).cloned(),
                };
                let result = match &comparison {
                    comments::AnchorComparison::Branch { merge_base, .. } if !key.untracked => {
                        branch::diff(&ctx.toplevel, merge_base, &key.path, old.as_deref()).await
                    }
                    comments::AnchorComparison::Branch { .. } => worktree::untracked_diff(&ctx.toplevel, &key.path).await,
                    comments::AnchorComparison::Worktree => worktree::diff(&ctx.toplevel, &key, old.as_deref()).await,
                };
                // A row whose diff fails is listed without ranges; the request goes out.
                result.ok().map(|(response, _patch)| ranges_of(&response.file_diff))
            }
        };
        let (additions, deletions) = ranges.unwrap_or_default();
        lines.push(prompt::RequestLine { path: key.path.clone(), staged: key.staged, untracked });
        files.push(RequestFile { key, comparison: comparison.clone(), additions, deletions });
    }
    Ok((lines, files))
}

/// Spec 10.4 step 1: the fresh check and its two gates. `Ok(record)` passes; `Err` is the refusal.
/// What kind of refusal the gates gave, so the box knows what the next Y may accept (Task 8 reads it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    Busy,
    Restarted(Option<SessionRef>),
    Other,
}

fn gates(target: &Target, state: Option<TargetState>, record: Option<&host::PaneRecord>, accepted: &Accepted, failure: Option<&host::HostFailure>) -> Result<(), (String, Refusal)> {
    let Target::Pane { pane, agent, .. } = target else { return Ok(()) };
    let Some(state) = state else {
        // No state: the check failed, or the host answered about some other pane (`compare` rejects it).
        let reason = match (failure, record) {
            (Some(f), _) => f.message(),
            (None, Some(r)) if r.pane_id != *pane => format!("the host answered about {}", r.pane_id),
            (None, _) => "no answer".to_string(),
        };
        return Err((format!("could not verify {pane}: {reason}"), Refusal::Other));
    };
    // The continuity gate.
    match &state {
        TargetState::Left | TargetState::Gone => return Err((format!("{agent} · {pane} is gone · pick a pane"), Refusal::Other)),
        TargetState::NoHost => return Err(("No host: this viewer runs outside herdr.".to_string(), Refusal::Other)),
        TargetState::Restarted(_) => {
            let now = record.and_then(|r| r.agent_session.clone());
            // Accepted only for the very session the check found, a missing one included.
            if accepted.restarted.as_ref() != Some(&now) {
                return Err((
                    format!("{agent} in {pane} was restarted since you picked it and has not seen earlier messages."),
                    Refusal::Restarted(now),
                ));
            }
        }
        TargetState::Live(_) | TargetState::Unverified | TargetState::Clipboard => {}
    }
    // The status gate, whatever the continuity.
    let status = match &state {
        TargetState::Live(s) | TargetState::Restarted(s) => s.as_str(),
        _ => "unknown",
    };
    // Only `idle` and `done` pass unconditionally (10.4); a status this viewer does not know is as good
    // as unknown and asks once, the same way.
    match status {
        "blocked" => Err((format!("{agent} is waiting for an approval in {pane}. Answer it there, or press A to pick another pane."), Refusal::Other)),
        "idle" | "done" => Ok(()),
        _ if accepted.busy => Ok(()),
        "working" => Err((format!("{agent} is working in {pane}; the review would queue behind its current turn."), Refusal::Busy)),
        "unknown" => Err((format!("{agent}'s state in {pane} is unknown to the host."), Refusal::Busy)),
        other => Err((format!("{agent} reports {} in {pane}, a state this viewer does not know.", strip_status(other)), Refusal::Busy)),
    }
}

/// A status the host invented, in a message: letters, digits and a few marks, cut short.
fn strip_status(status: &str) -> String {
    status.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).take(24).collect()
}

fn now_label(kind: &SendKind) -> &'static str {
    match kind {
        SendKind::Feedback => "Y",
        SendKind::Review { .. } => "@",
    }
}

/// `clipboard.md` under the state lock; `Err` is the reason.
fn write_clipboard(state_dir: Option<&Path>, text: &str) -> Result<PathBuf, String> {
    let dir = state_dir.ok_or_else(|| "no state directory".to_string())?;
    comments::write_clipboard(dir, text).map_err(|e| e.to_string())
}

fn shown(path: &Path) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let text = path.to_string_lossy();
    match text.strip_prefix(home.as_str()).filter(|_| !home.is_empty()) {
        Some(rest) => format!("~{rest}"),
        None => text.into_owned(),
    }
}

/// The next nonce `make` yields that `used` does not hold: the loop `Store::claim` and
/// `record_request_fresh` run under their locks, for the paths that have no file to lock.
fn fresh_nonce(make: &dyn Fn(u64) -> String, counter: &mut u64, used: &std::collections::BTreeSet<String>) -> String {
    loop {
        *counter += 1;
        let candidate = make(*counter);
        if !used.contains(&candidate) {
            return candidate;
        }
    }
}

/// The copy of 10.4: the sequence when the text fits, the file when it can be written; the notice names both.
fn copy_out(state_dir: Option<&Path>, text: &str, what: &str) -> (CopyOut, Result<PathBuf, String>) {
    let file = write_clipboard(state_dir, text);
    let osc = osc52(text);
    let notice = match (&file, &osc) {
        (Ok(path), Some(_)) => format!("copied {what} · also in {}", shown(path)),
        (Ok(path), None) => format!("copied {what} to {} · too long for the terminal's clipboard", shown(path)),
        (Err(reason), Some(_)) => format!("copied {what} · not saved to clipboard.md: {reason}"),
        (Err(reason), None) => format!("nothing could receive the copy: {reason}"),
    };
    (CopyOut { osc, notice, urgent: file.is_err() }, file)
}

/// The send task of spec 10.4, one at a time per viewer (the session refuses a second) and per user
/// (the send lock). Returns the store it borrowed with the answer.
pub async fn send(ctx: Context, request: SendRequest, store: Store, waiting: impl Fn(bool) + Send + Sync + 'static) -> Finished {
    let generation = ctx.generation;
    let mut counter = ctx.nonce_counter;
    let waiting = Arc::new(waiting);
    let mut salvaged = None;
    let (store, outcome) = send_inner(&ctx, &request, store, &mut counter, waiting, &mut salvaged).await;
    Finished {
        store: Some(store),
        adopt_session: outcome.as_ref().ok().and_then(|(_, adopt)| adopt.clone()),
        outcome: outcome.map(|(o, _)| o),
        copy: salvaged,
        generation,
        counter,
    }
}

/// The store travels into every blocking step and back, so no transaction runs on an async worker.
async fn send_inner(
    ctx: &Context,
    request: &SendRequest,
    mut store: Store,
    counter: &mut u64,
    waiting: Arc<dyn Fn(bool) + Send + Sync>,
    salvaged: &mut Option<CopyOut>,
) -> (Store, Result<(SendOutcome, Option<Option<SessionRef>>), (String, Refusal)>) {
    // Every refusal that is not one of the gates' is `Refusal::Other`.
    macro_rules! bail {
        ($store:expr, $err:expr) => {
            return ($store, Err(($err, Refusal::Other)));
        };
    }
    let Some(target) = ctx.target.clone() else {
        bail!(store, "no target: press A".to_string());
    };
    // The reads come first (spec 10.4 "Request review"): the check below is then a moment from the dispatch.
    let request_files = match &request.kind {
        SendKind::Review { scope } => match request_files(ctx, scope).await {
            Ok(files) => Some(files),
            Err(e) => bail!(store, e),
        },
        SendKind::Feedback => None,
    };
    // Step 1a, the lock: every send of every viewer of this user takes it, so two never paste at once.
    let mut _held = match ctx.state_dir.as_ref().map(|dir| dir.join(SEND_LOCK)) {
        Some(path) => match hold_send_lock(path, waiting.clone()).await {
            Ok(lock) => Some(lock),
            Err(e) => bail!(store, e),
        },
        None => None,
    };
    let again = || format!("the target changed; press {} again", now_label(&request.kind));
    // Step 1b, the fresh check, under the selection generation.
    if ctx.latest_generation.load(Ordering::SeqCst) != ctx.generation {
        bail!(store, again());
    }
    let (state, record, failure) = match &target {
        Target::Clipboard => (Some(TargetState::Clipboard), None, None),
        Target::Pane { pane, .. } => {
            let pane = pane.clone();
            let fresh = call_host(ctx.host.clone(), move |h| h.pane_get(&pane)).await;
            let state = target::compare(&target, ctx.socket_path.as_deref(), &fresh);
            let (record, failure) = match fresh {
                Ok(r) => (Some(r), None),
                Err(f) => (None, Some(f)),
            };
            (state, record, failure)
        }
    };
    if let Err((message, refusal)) = gates(&target, state.clone(), record.as_ref(), &request.accepted, failure.as_ref()) {
        return (store, Err((message, refusal)));
    }
    if ctx.latest_generation.load(Ordering::SeqCst) != ctx.generation {
        bail!(store, again());
    }
    let adopt = match (&state, &record) {
        (Some(TargetState::Restarted(_)), Some(r)) => Some(r.agent_session.clone()),
        _ => None,
    };
    let to = match (&target, &record) {
        (Target::Clipboard, _) => Destination::clipboard(),
        (Target::Pane { .. }, Some(r)) => target::destination_of(r),
        (Target::Pane { pane, agent, session, .. }, None) => Destination::Pane { pane: pane.clone(), agent: agent.clone(), session: session.clone() },
    };
    let pane_for_bound = match &to {
        Destination::Pane { pane, .. } => pane.clone(),
        Destination::Clipboard { .. } => String::new(),
    };
    // Shared by the closures below, each of which runs on the blocking pool with its own clone.
    let bound: Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync> = Arc::new(move |text: &str| {
        let len = encoded_len(&pane_for_bound, text);
        if len > REQUEST_BOUND {
            return Err(format!(
                "review too large to send at once ({} KiB of 512): delete or shorten comments",
                len.div_ceil(1024)
            ));
        }
        Ok(())
    });
    // Steps 2 and 3: the claim (feedback), or the text and its record (request), on the blocking pool.
    // The time is sampled here, after the lock and the host's answer, never at the command: a send that
    // queued behind other work must not make a claim that is already old; the claim samples it again.
    let toplevel = ctx.toplevel.clone();
    let make = ctx.nonce.clone();
    let clock = ctx.clock.clone();
    let now = clock();
    // The guard every transaction runs under the lock: the pick this send was confirmed against still stands.
    let (latest, generation) = (ctx.latest_generation.clone(), ctx.generation);
    let label = now_label(&request.kind);
    let guard = Arc::new(move || -> Result<(), String> {
        if latest.load(Ordering::SeqCst) != generation {
            Err(format!("the target changed; press {label} again"))
        } else {
            Ok(())
        }
    });
    let (text, items, nonce, claimed) = match (&request.kind, request_files) {
        (SendKind::Feedback, _) => {
            let (to2, guard2, bound2, clock2) = (to.clone(), guard.clone(), bound.clone(), clock.clone());
            let mut c = *counter;
            let (returned, result) = blocking(move || {
                // The stamp's time is the claim's own: the expiry of 10.4 runs from it.
                let result = store.claim(clock2(), &to2, make.as_ref(), &mut c, guard2.as_ref(), |eligible, nonce| {
                    let items: Vec<prompt::Item<'_>> = eligible.iter().map(|(n, c)| prompt::Item { number: *n, comment: c }).collect();
                    let text = prompt::review(&toplevel, &items, nonce);
                    bound2(&text)?;
                    Ok(text)
                });
                (store, result.map(|claimed| (claimed, c)))
            })
            .await;
            store = returned;
            match result {
                Ok((claimed, c)) => {
                    *counter = c;
                    (claimed.text, claimed.comments.len() as u32, claimed.nonce, true)
                }
                Err(e) => bail!(store, e),
            }
        }
        (SendKind::Review { .. }, Some((lines, files))) => {
            let scope = match comparison_of(&ctx.snapshot) {
                comments::AnchorComparison::Branch { merge_base, .. } => Some(merge_base),
                comments::AnchorComparison::Worktree => None,
            };
            // Built per nonce inside the reservations below; `Arc`'d so each blocking closure can own a copy.
            let text_of = Arc::new(move |nonce: &str| {
                prompt::request(
                    &toplevel,
                    &lines,
                    match &scope {
                        Some(mb) => prompt::RequestScope::Branch { merge_base: mb },
                        None => prompt::RequestScope::Worktree,
                    },
                    nonce,
                )
            });
            let items = files.len() as u32;
            // The bound is checked on the text as it will go out, nonce included, inside the reservation
            // where there is one, so a refused request is never recorded.
            let over_bound = |text: &str| bound(text).err();
            match (&to, &ctx.state_dir) {
                // A pane: the record precedes the call, as the claim does; a definite failure removes it.
                (Destination::Pane { .. }, Some(dir)) => {
                    // The text is built and bounded inside the reservation: an oversized request leaves the file untouched.
                    let (dir2, top, make, to2, guard2, bound2, mut c) = (dir.clone(), ctx.toplevel.clone(), ctx.nonce.clone(), to.clone(), guard.clone(), bound.clone(), *counter);
                    let recorded = blocking(move || {
                        let (nonce, text) = comments::record_request_fresh(&dir2, &top, make.as_ref(), &mut c, guard2.as_ref(), |nonce| {
                            let text = text_of(nonce);
                            bound2(&text)?;
                            Ok((RequestRecord { nonce: nonce.to_string(), at: now, target: to2, files }, text))
                        })?;
                        Ok::<_, std::io::Error>((nonce, text, c))
                    })
                    .await;
                    match recorded {
                        Ok((nonce, text, c)) => {
                            *counter = c;
                            (text, items, nonce, false)
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => bail!(store, e.into_inner().map(|e| e.to_string()).unwrap_or_default()),
                        Err(e) => bail!(store, format!("request not recorded: {e}")),
                    }
                }
                // No state directory: nothing can be recorded; the request still goes out, under a nonce
                // no stamp in memory carries (the array is the store, and its chains count).
                (Destination::Pane { .. }, None) => {
                    if let Err(e) = guard() {
                        bail!(store, e);
                    }
                    let nonce = fresh_nonce(ctx.nonce.as_ref(), counter, &comments::nonces_of(store.comments()));
                    let text = text_of(&nonce);
                    if let Some(e) = over_bound(&text) {
                        bail!(store, e);
                    }
                    (text, items, nonce, false)
                }
                // The clipboard: the record takes its nonce under the lock, the text carries it, the copy
                // follows, and a copy that reached nothing removes the record again (spec 10.4's rule).
                (Destination::Clipboard { .. }, dir) => {
                    let (nonce, text) = match dir {
                        Some(dir) => {
                            let (dir, top, make, guard2, bound2, mut c, files2) = (dir.clone(), ctx.toplevel.clone(), ctx.nonce.clone(), guard.clone(), bound.clone(), *counter, files.clone());
                            let text_of2 = text_of.clone();
                            let recorded = blocking(move || {
                                let (nonce, text) = comments::record_request_fresh(&dir, &top, make.as_ref(), &mut c, guard2.as_ref(), |nonce| {
                                    let text = text_of2(nonce);
                                    bound2(&text)?;
                                    Ok((RequestRecord { nonce: nonce.to_string(), at: now, target: Destination::clipboard(), files: files2 }, text))
                                })?;
                                Ok::<_, std::io::Error>((nonce, text, c))
                            })
                            .await;
                            match recorded {
                                Ok((nonce, text, c)) => {
                                    *counter = c;
                                    (nonce, text)
                                }
                                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => bail!(store, e.into_inner().map(|e| e.to_string()).unwrap_or_default()),
                                Err(e) => bail!(store, format!("request not recorded: {e}")),
                            }
                        }
                        None => {
                            if let Err(e) = guard() {
                                bail!(store, e);
                            }
                            let nonce = fresh_nonce(ctx.nonce.as_ref(), counter, &comments::nonces_of(store.comments()));
                            let text = text_of(&nonce);
                            if let Some(e) = over_bound(&text) {
                                bail!(store, e);
                            }
                            (nonce, text)
                        }
                    };
                    // The pick, once more after the reservation (step 3b below, which this early return
                    // would skip): a request confirmed for one target is not copied under another.
                    if let Some(gate) = ctx.send_gate.clone() {
                        blocking(move || gate()).await;
                    }
                    if ctx.latest_generation.load(Ordering::SeqCst) != ctx.generation {
                        if let Some(dir) = dir.clone() {
                            let (top, n) = (ctx.toplevel.clone(), nonce.clone());
                            let _ = blocking(move || comments::remove_request(&dir, &top, &n)).await;
                        }
                        bail!(store, again());
                    }
                    let (dir2, text2) = (dir.clone(), text.clone());
                    let (copy, file) = blocking(move || copy_out(dir2.as_deref(), &text2, "a review request")).await;
                    if file.is_err() && copy.osc.is_none() {
                        if let Some(dir) = dir.clone() {
                            let (top, n) = (ctx.toplevel.clone(), nonce.clone());
                            let _ = blocking(move || comments::remove_request(&dir, &top, &n)).await;
                        }
                        bail!(store, copy.notice);
                    }
                    return (
                        store,
                        Ok((SendOutcome { kind: request.kind.clone(), items, to, unconfirmed: file.is_err(), copy: Some(copy) }, adopt)),
                    );
                }
            }
        }
        (SendKind::Review { .. }, None) => unreachable!("request files are read first"),
    };
    // Step 3b: the pick this send was confirmed for, once more after the transaction. A re-pick that landed
    // while the claim or the record was written is met here, before anything reaches a destination: the
    // claim is settled as failed (the records return to what they were), the request's record is removed.
    // (The clipboard request's arm above returned already, after the same check.)
    if let Some(gate) = ctx.send_gate.clone() {
        blocking(move || gate()).await;
    }
    if ctx.latest_generation.load(Ordering::SeqCst) != ctx.generation {
        if claimed {
            let n = nonce.clone();
            let (returned, _) = blocking(move || {
                let settled = store.settle(now, &n, Settlement::Failed);
                (store, settled)
            })
            .await;
            store = returned;
        } else if let Some(dir) = ctx.state_dir.clone() {
            let (top, n) = (ctx.toplevel.clone(), nonce.clone());
            let _ = blocking(move || comments::remove_request(&dir, &top, &n)).await;
        }
        bail!(store, again());
    }
    // Steps 4 and 5, the call and the settlement.
    match &to {
        Destination::Clipboard { .. } => {
            let (dir, text2) = (ctx.state_dir.clone(), text.clone());
            let (copy, file) = blocking(move || copy_out(dir.as_deref(), &text2, &format!("{items} comments"))).await;
            let settlement = match (&file, &copy.osc) {
                (Ok(_), _) => Settlement::Sent,
                (Err(_), Some(_)) => Settlement::Unconfirmed,
                (Err(_), None) => Settlement::Failed,
            };
            if claimed {
                let n = nonce.clone();
                let (returned, settled) = blocking(move || {
                    let settled = store.settle(now, &n, settlement);
                    (store, settled)
                })
                .await;
                store = returned;
                if let Err(e) = settled {
                    if settlement == Settlement::Failed {
                        bail!(store, format!("{} · {e}", copy.notice));
                    }
                    // The text went out; only the stamps failed. The sequence is kept for the shell, the
                    // notice says both, and the records stay `Sending` until they expire to unconfirmed.
                    let notice = format!("{} · but the comments were not marked sent: {e}", copy.notice);
                    *salvaged = Some(copy);
                    bail!(store, notice);
                }
            }
            if settlement == Settlement::Failed {
                bail!(store, copy.notice);
            }
            (
                store,
                Ok((SendOutcome { kind: request.kind.clone(), items, to, unconfirmed: settlement == Settlement::Unconfirmed, copy: Some(copy) }, adopt)),
            )
        }
        Destination::Pane { pane, .. } => {
            let (pane_id, text_sent) = (pane.clone(), text.clone());
            // The lock travels with the call: a call that outlives the engine's wait keeps it until the
            // host has answered and the Enter delay has passed, so no second viewer pastes meanwhile.
            let (answer, held_back) = call_host_late(ctx, move |h| h.agent_prompt(&pane_id, &text_sent), nonce.clone(), claimed, _held.take()).await;
            _held = held_back;
            // The lock outlives the host's Enter delay, so the next sender pastes into an empty line.
            tokio::time::sleep(ENTER_MARGIN).await;
            let settlement = match &answer {
                Ok(()) => Settlement::Sent,
                Err(host::HostFailure::After(_)) => Settlement::Unconfirmed,
                Err(host::HostFailure::NoHost(_) | host::HostFailure::Before(_) | host::HostFailure::Api { .. }) => Settlement::Failed,
            };
            if claimed {
                let n = nonce.clone();
                let (returned, settled) = blocking(move || {
                    let settled = store.settle(now, &n, settlement);
                    (store, settled)
                })
                .await;
                store = returned;
                if let Err(e) = settled {
                    bail!(store, e);
                }
            } else if settlement == Settlement::Failed {
                if let Some(dir) = ctx.state_dir.clone() {
                    let (top, n) = (ctx.toplevel.clone(), nonce.clone());
                    let _ = blocking(move || comments::remove_request(&dir, &top, &n)).await;
                }
            }
            match (answer, settlement) {
                (Err(failure), Settlement::Failed) => (store, Err((failure.message(), Refusal::Other))),
                (_, settlement) => (
                    store,
                    Ok((SendOutcome { kind: request.kind.clone(), items, to, unconfirmed: settlement == Settlement::Unconfirmed, copy: None }, adopt)),
                ),
            }
        }
    }
}

/// The send lock, taken on the blocking pool; `waiting(true)` while another viewer holds it.
async fn hold_send_lock(path: PathBuf, waiting: Arc<dyn Fn(bool) + Send + Sync>) -> Result<SendLock, String> {
    // Try at once (on the pool: it creates the directory and opens the file); only a contended lock is worth a notice.
    let first = path.clone();
    match blocking(move || SendLock::try_take(&first)).await {
        Ok(Some(lock)) => return Ok(lock),
        Ok(None) => {}
        Err(e) => return Err(format!("{SEND_LOCK}: {e}")),
    }
    waiting(true);
    let taken = tokio::task::spawn_blocking(move || SendLock::take(&path, LOCK_WAIT))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r.map_err(|e| format!("{SEND_LOCK}: {e}")));
    waiting(false);
    taken
}

/// An exclusive `flock` on `send.lock`, released on drop. `Send`, so a late call can carry it.
pub struct SendLock {
    _file: std::fs::File,
}

impl SendLock {
    fn open(path: &Path) -> std::io::Result<std::fs::File> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(path)
    }

    fn try_take(path: &Path) -> std::io::Result<Option<Self>> {
        use std::os::unix::io::AsRawFd;
        let file = Self::open(path)?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(Some(Self { _file: file }));
        }
        let error = std::io::Error::last_os_error();
        if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) {
            Ok(None)
        } else {
            Err(error)
        }
    }

    fn take(path: &Path, wait: Duration) -> std::io::Result<Self> {
        let start = std::time::Instant::now();
        loop {
            if let Some(lock) = Self::try_take(path)? {
                return Ok(lock);
            }
            if start.elapsed() >= wait {
                return Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "another viewer is still sending"));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// One host call on the blocking pool, bounded by the engine's wait (`ctx.host_wait`; `ENGINE_WAIT`
/// in production, shared with session.rs through `SessionConfig.host_wait`).
pub async fn call_host<T: Send + 'static>(
    host: Option<Arc<dyn HostClient>>,
    call: impl FnOnce(&dyn HostClient) -> Result<T, host::HostFailure> + Send + 'static,
) -> Result<T, host::HostFailure> {
    call_host_within(host, host::ENGINE_WAIT, call).await.0
}

/// `call_host` with the wait given; on a timeout the blocking task's handle comes back, so its late
/// answer can still be consumed.
pub async fn call_host_within<T: Send + 'static>(
    host: Option<Arc<dyn HostClient>>,
    wait: Duration,
    call: impl FnOnce(&dyn HostClient) -> Result<T, host::HostFailure> + Send + 'static,
) -> (Result<T, host::HostFailure>, Option<tokio::task::JoinHandle<Result<T, host::HostFailure>>>) {
    let Some(host) = host else {
        return (Err(host::HostFailure::NoHost("no host: HERDR_SOCKET_PATH is unset".into())), None);
    };
    let mut handle = tokio::task::spawn_blocking(move || call(host.as_ref()));
    match tokio::time::timeout(wait, &mut handle).await {
        Ok(Ok(result)) => (result, None),
        Ok(Err(e)) => (Err(host::HostFailure::After(format!("host call failed: {e}"))), None),
        Err(_) => (Err(host::HostFailure::After("the host did not answer in time".into())), Some(handle)),
    }
}

/// The prompt call: when the engine's wait passes, the records are settled `Unconfirmed` by the
/// caller, and the late answer, when it comes, settles them again by nonce (spec 10.6): `Sent` on
/// success, their earlier stamp or `Pending` on a definite failure; an `After` failure says nothing
/// new and leaves them. The send lock goes with a call that outlives the wait and is released only
/// after that call has answered and the Enter delay has passed; a call that answered in time gives
/// the lock back to the caller.
async fn call_host_late(
    ctx: &Context,
    call: impl FnOnce(&dyn HostClient) -> Result<(), host::HostFailure> + Send + 'static,
    nonce: String,
    claimed: bool,
    held: Option<SendLock>,
) -> (Result<(), host::HostFailure>, Option<SendLock>) {
    let (answer, late) = call_host_within(ctx.host.clone(), ctx.host_wait, call).await;
    let Some(handle) = late else {
        return (answer, held);
    };
    let (state_dir, toplevel, late_tx) = (ctx.state_dir.clone(), ctx.toplevel.clone(), ctx.late.clone());
    tokio::spawn(async move {
        let result = handle.await;
        // The host has answered, or the task died: the paste, if any, is done once the Enter delay passes.
        tokio::time::sleep(ENTER_MARGIN).await;
        drop(held);
        let Ok(result) = result else { return };
        let settlement = match result {
            Ok(()) => Settlement::Sent,
            Err(host::HostFailure::After(_)) => return,
            Err(_) => Settlement::Failed,
        };
        if claimed {
            // The store is the session's; the late settlement goes through it (Done::LateAnswer).
            let _ = late_tx.send((nonce, settlement));
        } else if settlement == Settlement::Failed {
            if let Some(dir) = state_dir {
                let _ = blocking(move || comments::remove_request(&dir, &toplevel, &nonce)).await;
            }
        }
    });
    (answer, None)
}

/// `c` in a box, `y` on a selection: nothing is claimed; a request copy is recorded once the text
/// reached a destination (spec 10.4). Returns the copy and where the nonce counter stands.
pub async fn copy(ctx: Context, request: CopyRequest, comments: Vec<comments::Comment>) -> Result<(CopyOut, u64), String> {
    let mut counter = ctx.nonce_counter;
    let (text, what) = match &request.what {
        CopyWhat::Selection(text) => (text.clone(), "selection".to_string()),
        CopyWhat::Review => {
            let eligible: Vec<&comments::Comment> = comments
                .iter()
                .filter(|c| matches!(c.state, comments::CommentState::Pending | comments::CommentState::Unconfirmed { .. }))
                .collect();
            if eligible.is_empty() {
                return Err(NOTICE_NO_PENDING.to_string());
            }
            // Nothing is stamped by a copy, so the nonce only has to be fresh against what is retained now;
            // a store that cannot be read is the journal's to report (10.7: `c` still copies), so the copy
            // falls back to what is in memory.
            let in_memory = || comments::nonces_of(&comments);
            let used: std::collections::BTreeSet<String> = match ctx.state_dir.clone() {
                Some(dir) => {
                    let own = comments.clone();
                    blocking(move || comments::nonces_in_use(&dir, &own)).await.unwrap_or_else(|_| in_memory())
                }
                None => in_memory(),
            };
            let nonce = loop {
                counter += 1;
                let candidate = (ctx.nonce)(counter);
                if !used.contains(&candidate) {
                    break candidate;
                }
            };
            let mut eligible = eligible;
            eligible.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
            let items: Vec<prompt::Item<'_>> = eligible.iter().enumerate().map(|(i, c)| prompt::Item { number: i as u32 + 1, comment: c }).collect();
            (prompt::review(&ctx.toplevel, &items, &nonce), format!("{} comments", eligible.len()))
        }
        CopyWhat::Request { scope } => {
            let (lines, files) = request_files(&ctx, scope).await?;
            let scope_of = match comparison_of(&ctx.snapshot) {
                comments::AnchorComparison::Branch { merge_base, .. } => Some(merge_base),
                comments::AnchorComparison::Worktree => None,
            };
            // The record takes its nonce under the lock; the text carries it; a copy that reached
            // nothing removes the record again, so a record exists only for a text that went somewhere.
            let (toplevel, scope_of2, lines2) = (ctx.toplevel.clone(), scope_of.clone(), lines.clone());
            let text_of = Arc::new(move |nonce: &str| {
                prompt::request(
                    &toplevel,
                    &lines2,
                    match &scope_of2 {
                        Some(mb) => prompt::RequestScope::Branch { merge_base: mb },
                        None => prompt::RequestScope::Worktree,
                    },
                    nonce,
                )
            });
            let (nonce, text) = match &ctx.state_dir {
                Some(dir) => {
                    let (dir, top, make, mut c, files2, now, text_of2) = (dir.clone(), ctx.toplevel.clone(), ctx.nonce.clone(), counter, files.clone(), (ctx.clock)(), text_of.clone());
                    let (latest, generation) = (ctx.latest_generation.clone(), ctx.generation);
                    let (nonce, text, c) = blocking(move || {
                        // A copy made for one target must not be recorded against another picked meanwhile.
                        let guard = || if latest.load(Ordering::SeqCst) == generation { Ok(()) } else { Err("the target changed; press @ again".to_string()) };
                        let (nonce, text) = comments::record_request_fresh(&dir, &top, make.as_ref(), &mut c, &guard, |nonce| {
                            Ok((RequestRecord { nonce: nonce.to_string(), at: now, target: Destination::clipboard(), files: files2 }, text_of2(nonce)))
                        })?;
                        Ok::<_, std::io::Error>((nonce, text, c))
                    })
                    .await
                    .map_err(|e| format!("request not recorded: {e}"))?;
                    counter = c;
                    (nonce, text)
                }
                None => {
                    let nonce = fresh_nonce(ctx.nonce.as_ref(), &mut counter, &comments::nonces_of(&comments));
                    (nonce.clone(), text_of(&nonce))
                }
            };
            let (dir, text2) = (ctx.state_dir.clone(), text.clone());
            let (out, file) = blocking(move || copy_out(dir.as_deref(), &text2, &format!("a review request for {} files", lines.len()))).await;
            if file.is_err() && out.osc.is_none() {
                if let Some(dir) = ctx.state_dir.clone() {
                    let (top, n) = (ctx.toplevel.clone(), nonce.clone());
                    let _ = blocking(move || comments::remove_request(&dir, &top, &n)).await;
                }
                return Err(out.notice);
            }
            return Ok((out, counter));
        }
    };
    let (dir, text2) = (ctx.state_dir.clone(), text.clone());
    let (out, file) = blocking(move || copy_out(dir.as_deref(), &text2, &what)).await;
    if file.is_err() && out.osc.is_none() {
        return Err(out.notice);
    }
    Ok((out, counter))
}
```

`call_host` moves here from Task 2's session.rs (one definition; `session.rs` calls `dispatch::call_host`). `store.claim`, `store.settle`, `record_request_fresh`, `remove_request` and `write_clipboard` are Step 2's; every one of them runs through `blocking`, never on an async worker, and the store travels into the closure and back. The `waiting` callback is how the session publishes `send_waiting` without the task touching `State`: the session passes a closure that sends `Done::SendWaiting(bool)` on the results channel. On the two orders that look inverted: a request's record is written *before* its copy or call because the record is where the nonce is reserved under the lock, and a copy that reached neither destination, or a call that definitely failed, removes it again, which is what spec 10.4's "recorded only when the text reached a destination" protects.

- [ ] **Step 4: Snapshot, commands and the session**

`types.rs`: `Snapshot` gains `send_seq: u64, send_error: Option<String>, send_refusal: Option<dispatch::Refusal>, send_outcome: Option<SendOutcome>, send_waiting: bool, copy_seq: u64, copy: Option<Arc<CopyOut>>` (zero/None/false in `empty`); `Command` gains `Send(SendRequest)` and `Copy(CopyRequest)`; `fingerprint` gains all seven (`copy` by `Arc::as_ptr`).

`session.rs`: `State` gains `send_in_flight: bool`, `nonce_counter: u64`, `latest_selection: Arc<AtomicU64>` (stored by `SetTarget` alongside `selection_generation`; Task 2's field becomes this Arc's value), `send_seq`, `copy_seq`. The command arm:

```rust
                    Command::Send(request) => {
                        let refusal = if state.send_in_flight {
                            Some(dispatch::NOTICE_IN_PROGRESS.to_string())
                        } else if !state.store_opened {
                            Some("not a git repository".to_string())
                        } else if matches!(request.kind, dispatch::SendKind::Feedback)
                            && !state.snapshot.comments.iter().any(|c| matches!(c.state, comments::CommentState::Pending | comments::CommentState::Unconfirmed { .. }))
                        {
                            Some(dispatch::NOTICE_NO_PENDING.to_string())
                        } else {
                            None
                        };
                        if let Some(refusal) = refusal {
                            state.send_seq += 1;
                            let mut next = state.snapshot.clone();
                            next.send_seq = state.send_seq;
                            next.send_error = Some(refusal);
                            next.send_outcome = None;
                            publish(&mut state, next, &snapshots);
                        } else {
                            // One send at a time; it waits its turn behind a comment transaction that holds the
                            // store, with the context of the moment it was confirmed (its target and generation).
                            state.send_in_flight = true;
                            let ctx = state.dispatch_context(&cwd);
                            state.store_queue.push_back(StoreOp::Send(request, ctx));
                            state.run_store_queue(&cwd, &results_tx);
                        }
                    }
                    Command::Copy(request) => {
                        // A copy reads the published comments, never the store: it needs no turn and runs beside a send.
                        let ctx = state.dispatch_context(&cwd);
                        let comments = state.snapshot.comments.to_vec();
                        let results = results_tx.clone();
                        tokio::spawn(async move {
                            let outcome = dispatch::copy(ctx, request, comments).await;
                            let _ = results.send(Done::Copied(outcome));
                        });
                    }
```

The store has one owner at a time. Task 3's `store_opened`, `store_queue` and `run_store_queue_comments` grow into `enum StoreOp { Comment(comments::Operation, Option<u64>), Send(dispatch::SendRequest, dispatch::Context), Refresh }` and one `run_store_queue`, which pops the front entry whenever `state.store.is_some()` and dispatches it (a comment transaction, a send task with the context captured when the send was confirmed, a refresh) on the pool; each `Done::Comment`/`Done::Sent` returns the store and calls it again. Nothing is ever refused for being borrowed, and a send that waited its turn still runs against the pick it was confirmed for: the generation it carries is compared with `latest_selection` inside the claim's transaction (the `guard`), under the state lock, after every wait. `SessionConfig` gains `host_wait: Duration` (`host::ENGINE_WAIT` in `production`; a test's seam) and `send_gate: Option<Arc<dyn Fn() + Send + Sync>>` (`None` in production; a test's seam that `send_inner` runs on the blocking pool after its transaction and before its last generation check, copied onto `State` and into every `Context`), and `State` a `late_tx`/`late_rx` pair (`unbounded_channel::<(String, Settlement)>()`) polled in `run`'s `select!`: a late answer becomes `StoreOp::LateAnswer(nonce, settlement)` on the queue, settled through `store.settle` on the pool and published like any other change to the comments. And:

```rust
    fn dispatch_context(&self, cwd: &str) -> dispatch::Context {
        dispatch::Context {
            toplevel: match &self.snapshot.repo {
                RepoState::Repo { toplevel, .. } => toplevel.clone(),
                _ => cwd.to_string(),
            },
            state_dir: self.state_dir.clone(),
            host: self.host.clone(),
            host_wait: self.host_wait,
            late: self.late_tx.clone(),
            worktree_renames: self.worktree_renames.clone(),
            socket_path: self.socket_path.clone(),
            target: self.snapshot.target.clone(),
            generation: self.selection_generation,
            latest_generation: self.latest_selection.clone(),
            nonce: self.nonce.clone(),
            nonce_counter: self.nonce_counter,
            snapshot: Arc::new(self.snapshot.clone()),
            lane: self.diff_lane.clone(),
            clock: Arc::new(now),
            send_gate: self.send_gate.clone(),
        }
    }
```

The three `Done` arms:

```rust
                    Done::SendWaiting(waiting) => {
                        next.send_waiting = waiting;
                        publish(&mut state, next, &snapshots);
                    }
                    Done::Sent(finished) => {
                        state.send_in_flight = false;
                        state.nonce_counter = state.nonce_counter.max(finished.counter);
                        if let Some(store) = finished.store {
                            next.comments = Arc::new(store.comments().to_vec());
                            state.store = Some(store);
                        }
                        state.send_seq += 1;
                        next.send_seq = state.send_seq;
                        next.send_waiting = false;
                        match finished.outcome {
                            Ok(outcome) => {
                                if let Some(copy) = &outcome.copy {
                                    state.copy_seq += 1;
                                    next.copy_seq = state.copy_seq;
                                    next.copy = Some(Arc::new(copy.clone()));
                                }
                                next.send_error = None;
                                next.send_refusal = None;
                                next.send_outcome = Some(outcome);
                            }
                            Err((message, refusal)) => {
                                if let Some(copy) = finished.copy {
                                    // The text reached the terminal or the file before the send failed: the
                                    // shell still writes the sequence; the error names the copy.
                                    state.copy_seq += 1;
                                    next.copy_seq = state.copy_seq;
                                    next.copy = Some(Arc::new(copy));
                                }
                                next.send_error = Some(message);
                                next.send_refusal = Some(refusal);
                                next.send_outcome = None;
                            }
                        }
                        if let (Some(session), true) = (finished.adopt_session, finished.generation == state.selection_generation) {
                            // An accepted restart: the record names the session that received the review,
                            // unless the reviewer picked another target while the send ran.
                            if let Some(Target::Pane { session: known, .. }) = next.target.as_mut() {
                                *known = session;
                            }
                            state.target_write_pending = true;
                            state.write_target(&next, &results_tx);
                        }
                        publish(&mut state, next, &snapshots);
                        state.run_store_queue(&cwd, &results_tx);
                    }
                    Done::Copied(outcome) => {
                        state.copy_seq += 1;
                        next.copy_seq = state.copy_seq;
                        next.copy = Some(Arc::new(match outcome {
                            Ok((out, counter)) => {
                                state.nonce_counter = state.nonce_counter.max(counter);
                                out
                            }
                            Err(notice) => dispatch::CopyOut { osc: None, notice, urgent: true },
                        }));
                        publish(&mut state, next, &snapshots);
                    }
```

`Done::Sent(dispatch::Finished)`, `Done::Copied(Result<(CopyOut, u64), String>)`, `Done::SendWaiting(bool)`. `State::write_target` is Task 3's; the accepted restart sets `target_write_pending` and calls it. The nonce counter is monotonic: the session takes the larger of its own and what a send or copy returned.

- [ ] **Step 5: Session tests**

```rust
    fn sending_fixture() -> (tempfile::TempDir, tempfile::TempDir, String, Arc<host::Scripted>) {
        let dir = fixture();
        let state = tempfile::tempdir().unwrap();
        let top = dir.path().canonicalize().unwrap().to_string_lossy().into_owned();
        let host = host::Scripted::with_panes(vec![agent_pane("w4:p2", "codex", "idle", Some("s1"), &top)]);
        (dir, state, top, host)
    }

    fn sending_config(dir: &std::path::Path, state: &std::path::Path, host: Arc<host::Scripted>) -> SessionConfig {
        let mut config = test_config(dir, Arc::new(AtomicBool::new(true)));
        config.state_dir = Some(state.to_path_buf());
        config.host = Some(host);
        config.socket_path = Some("/run/fake.sock".into());
        config.nonce = Arc::new(|counter| format!("n{counter:05}"));
        config
    }

    fn feedback(accepted: dispatch::Accepted) -> Command {
        Command::Send(dispatch::SendRequest { kind: dispatch::SendKind::Feedback, accepted })
    }

    /// A session with a target and two pending comments, ready to send.
    fn ready_to_send(handle: &EngineHandle) {
        wait_for(handle, "rows", |s| !s.files.is_empty());
        handle.commands.send(Command::SetTarget(pane_target("w4:p2", "codex", Some("s1")))).unwrap();
        wait_for(handle, "live", |s| s.target_state == TargetState::Live("idle".into()));
        handle.commands.send(pending(2, "first")).unwrap();
        handle.commands.send(pending(1, "second")).unwrap();
        wait_for(handle, "two pending", |s| s.comments.len() == 2 && s.comment_seq == 2);
    }

    #[test]
    fn a_send_claims_before_the_call_sends_once_without_wait_and_stamps_sent_with_the_destination() {
        let (dir, state, top, host) = sending_fixture();
        let claimed_at_call = Arc::new(std::sync::Mutex::new(None));
        {
            let seen = claimed_at_call.clone();
            let path = state.path().join("comments.json");
            *host.on_prompt.lock().unwrap() = Some(Box::new(move || {
                *seen.lock().unwrap() = Some(std::fs::read_to_string(&path).unwrap());
            }));
        }
        let (_rt, handle) = start_from(sending_config(dir.path(), state.path(), host.clone()));
        ready_to_send(&handle);
        handle.commands.send(feedback(dispatch::Accepted::default())).unwrap();
        let s = wait_for(&handle, "sent", |s| s.send_seq == 1);
        assert_eq!(s.send_error, None);
        let outcome = s.send_outcome.clone().unwrap();
        assert_eq!((outcome.items, outcome.unconfirmed), (2, false));
        assert_eq!(outcome.to, target::Destination::Pane { pane: "w4:p2".into(), agent: "codex".into(), session: Some(host::SessionRef { kind: "id".into(), value: "s1".into() }) });
        let prompts = host.prompts.lock().unwrap().clone();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].0, "w4:p2");
        assert!(prompts[0].1.starts_with("> Inline review — 2 items."));
        assert!(prompts[0].1.contains(&format!("] {top}/a.txt:2 (additions) [unstaged]\n> ─ first\n")));
        assert!(prompts[0].1.contains("\"nonce\":\"n00001\""));
        // The file already carried the claim when the host was called.
        let at_call = claimed_at_call.lock().unwrap().clone().unwrap();
        assert!(at_call.contains("\"sending\"") && at_call.contains("n00001"), "{at_call}");
        for c in s.comments.iter() {
            assert!(matches!(&c.state, comments::CommentState::Sent(st) if st.nonce == "n00001" && matches!(&st.to, target::Destination::Pane { pane, .. } if pane == "w4:p2")));
        }
        // Nothing pending: a second Y is refused without a call.
        handle.commands.send(feedback(dispatch::Accepted::default())).unwrap();
        let s = wait_for(&handle, "nothing pending", |s| s.send_seq == 2);
        assert_eq!(s.send_error.as_deref(), Some(dispatch::NOTICE_NO_PENDING));
        assert_eq!(host.prompts.lock().unwrap().len(), 1);
    }

    #[test]
    fn every_gate_refuses_or_passes_as_the_table_says() {
        let (dir, state, top, host) = sending_fixture();
        let (_rt, handle) = start_from(sending_config(dir.path(), state.path(), host.clone()));
        ready_to_send(&handle);
        let mut seq = 0;
        let mut send = |accepted: dispatch::Accepted| -> Arc<Snapshot> {
            handle.commands.send(feedback(accepted)).unwrap();
            seq += 1;
            wait_for(&handle, "answer", |s| s.send_seq == seq)
        };
        host.set_pane(agent_pane("w4:p2", "codex", "blocked", Some("s1"), &top));
        assert!(send(dispatch::Accepted { busy: true, restarted: None }).send_error.as_deref().unwrap().starts_with("codex is waiting for an approval in w4:p2"));
        host.set_pane(agent_pane("w4:p2", "codex", "working", Some("s1"), &top));
        let s = send(dispatch::Accepted::default());
        assert!(s.send_error.as_deref().unwrap().starts_with("codex is working in w4:p2"));
        assert_eq!(s.send_refusal, Some(dispatch::Refusal::Busy));
        host.set_pane(agent_pane("w4:p2", "codex", "unknown", Some("s1"), &top));
        assert!(send(dispatch::Accepted::default()).send_error.as_deref().unwrap().contains("unknown to the host"));
        // A status outside the five the host documents asks like unknown does, and `busy` accepts it.
        host.set_pane(agent_pane("w4:p2", "codex", "sleeping", Some("s1"), &top));
        let s = send(dispatch::Accepted::default());
        assert_eq!(s.send_error.as_deref(), Some("codex reports sleeping in w4:p2, a state this viewer does not know."));
        assert_eq!(s.send_refusal, Some(dispatch::Refusal::Busy));
        assert_eq!(host.prompts.lock().unwrap().len(), 0);
        assert!(send(dispatch::Accepted { busy: true, restarted: None }).send_error.is_none());
        assert_eq!(host.prompts.lock().unwrap().len(), 1, "accepted, it sends");
        host.set_pane(agent_pane("w4:p2", "codex", "idle", Some("s1"), &top));
        handle.commands.send(pending(1, "again")).unwrap();
        wait_for(&handle, "pending again", |s| s.comments.iter().any(|c| c.is_pending()));
        // A restart behind an accepted `working` is refused and shown.
        host.set_pane(agent_pane("w4:p2", "codex", "working", Some("s2"), &top));
        let s = send(dispatch::Accepted { busy: true, restarted: None });
        assert!(s.send_error.as_deref().unwrap().contains("was restarted since you picked it"));
        assert!(matches!(&s.send_refusal, Some(dispatch::Refusal::Restarted(Some(sess))) if sess.value == "s2"));
        host.set_pane(agent_pane("w4:p2", "codex", "idle", Some("s2"), &top));
        assert!(send(dispatch::Accepted { busy: false, restarted: Some(Some(host::SessionRef { kind: "id".into(), value: "s9".into() })) }).send_error.is_some(), "the wrong session is not the one the box showed");
        host.set_pane(host::PaneRecord { pane_id: "w4:p2".into(), agent_status: Some("unknown".into()), ..host::PaneRecord::default() });
        let s = send(dispatch::Accepted::default());
        assert_eq!(s.send_error.as_deref(), Some("codex · w4:p2 is gone · pick a pane"));
        assert_eq!(s.send_refusal, Some(dispatch::Refusal::Other));
        *host.list_failure.lock().unwrap() = Some(host::HostFailure::After("deadline".into()));
        assert_eq!(send(dispatch::Accepted::default()).send_error.as_deref(), Some("could not verify w4:p2: deadline"));
        *host.list_failure.lock().unwrap() = None;
        // A reply naming another pane is refused as a check that could not run, and nothing is adopted.
        *host.answer_pane_get_with.lock().unwrap() = Some(agent_pane("w4:p9", "codex", "idle", Some("s1"), &top));
        assert_eq!(send(dispatch::Accepted::default()).send_error.as_deref(), Some("could not verify w4:p2: the host answered about w4:p9"));
        *host.answer_pane_get_with.lock().unwrap() = None;
        assert_eq!(host.prompts.lock().unwrap().len(), 1, "nothing was sent by a refusal: only the accepted send above");
        // The accepted restart sends, and the target record adopts the new session.
        host.set_pane(agent_pane("w4:p2", "codex", "idle", Some("s2"), &top));
        let s = send(dispatch::Accepted { busy: false, restarted: Some(Some(host::SessionRef { kind: "id".into(), value: "s2".into() })) });
        assert!(s.send_error.is_none(), "{:?}", s.send_error);
        assert!(matches!(&s.target, Some(Target::Pane { session: Some(sess), .. }) if sess.value == "s2"));
        wait_cond("s2 remembered", || matches!(target::load_targets(state.path()).0.get(&top), Some(Target::Pane { session: Some(sess), .. }) if sess.value == "s2"));
        wait_for(&handle, "live again", |s| s.target_state == TargetState::Live("idle".into()));
        // A restart into an agent that reports no session is accepted as that absence, and adopted as it.
        host.set_pane(host::PaneRecord { pane_id: "w4:p2".into(), agent: Some("codex".into()), agent_status: Some("idle".into()), ..host::PaneRecord::default() });
        wait_for(&handle, "restarted again", |s| s.target_state == TargetState::Restarted("idle".into()));
        handle.commands.send(pending(1, "more")).unwrap();
        wait_for(&handle, "pending again", |s| s.comments.iter().any(|c| c.is_pending()));
        assert!(matches!(send(dispatch::Accepted::default()).send_refusal, Some(dispatch::Refusal::Restarted(None))));
        let s = send(dispatch::Accepted { busy: false, restarted: Some(None) });
        assert!(s.send_error.is_none(), "{:?}", s.send_error);
        assert!(matches!(&s.target, Some(Target::Pane { session: None, .. })), "the record adopted the absence");
        wait_cond("absence remembered", || matches!(target::load_targets(state.path()).0.get(&top), Some(Target::Pane { session: None, .. })));
        wait_for(&handle, "live without a session", |s| s.target_state == TargetState::Live("idle".into()));
    }

    #[test]
    fn host_answers_settle_the_claim_each_their_way() {
        let (dir, state, _top, host) = sending_fixture();
        let (_rt, handle) = start_from(sending_config(dir.path(), state.path(), host.clone()));
        ready_to_send(&handle);
        let state_of = |s: &Snapshot, i: usize| s.comments[i].state.clone();
        // A definite refusal returns the records to Pending with the host's words.
        host.prompt_results.lock().unwrap().push(Err(host::HostFailure::Api { code: "agent_not_ready".into(), message: "agent is blocked".into() }));
        handle.commands.send(feedback(dispatch::Accepted::default())).unwrap();
        let s = wait_for(&handle, "refused", |s| s.send_seq == 1);
        assert_eq!(s.send_error.as_deref(), Some("agent is blocked"));
        assert!(s.comments.iter().all(|c| c.is_pending()));
        // An uncertain outcome stamps Unconfirmed.
        host.prompt_results.lock().unwrap().push(Err(host::HostFailure::After("no reply".into())));
        handle.commands.send(feedback(dispatch::Accepted::default())).unwrap();
        let s = wait_for(&handle, "unconfirmed", |s| s.send_seq == 2);
        assert!(s.send_error.is_none());
        assert!(s.send_outcome.as_ref().unwrap().unconfirmed);
        assert!(matches!(state_of(&s, 0), comments::CommentState::Unconfirmed { stamp: st, .. } if st.nonce == "n00002"));
        // A retry that definitely fails restores the earlier Unconfirmed stamp, not Pending.
        host.prompt_results.lock().unwrap().push(Err(host::HostFailure::Before("connection refused".into())));
        handle.commands.send(feedback(dispatch::Accepted::default())).unwrap();
        let s = wait_for(&handle, "retry failed", |s| s.send_seq == 3);
        assert!(s.send_error.is_some());
        assert!(matches!(state_of(&s, 0), comments::CommentState::Unconfirmed { stamp: st, .. } if st.nonce == "n00002"), "{:?}", state_of(&s, 0));
        // A success after that is Sent under the newest nonce, both items.
        handle.commands.send(feedback(dispatch::Accepted::default())).unwrap();
        let s = wait_for(&handle, "sent", |s| s.send_seq == 4);
        assert!(s.comments.iter().all(|c| matches!(&c.state, comments::CommentState::Sent(st) if st.nonce == "n00004")));
        assert_eq!(host.prompts.lock().unwrap().len(), 4);
    }

    #[test]
    fn a_success_reply_is_sent_whatever_happened_after_the_check() {
        let (dir, state, top, host) = sending_fixture();
        // The pane goes blocked the moment the prompt arrives: the host accepted it all the same.
        {
            let host2 = host.clone();
            let top2 = top.clone();
            *host.on_prompt.lock().unwrap() = Some(Box::new(move || {
                host2.set_pane(agent_pane("w4:p2", "codex", "blocked", Some("s1"), &top2));
            }));
        }
        let (_rt, handle) = start_from(sending_config(dir.path(), state.path(), host.clone()));
        ready_to_send(&handle);
        handle.commands.send(feedback(dispatch::Accepted::default())).unwrap();
        // One predicate: the chip may turn blocked before or after the send answers (a refresh can run
        // during the Enter margin), and a snapshot taken for one condition is gone for the next.
        let s = wait_for(&handle, "sent, and the chip follows the pane", |s| s.send_seq == 1 && s.target_state == TargetState::Live("blocked".into()));
        assert!(s.send_error.is_none());
        assert!(s.comments.iter().all(|c| matches!(c.state, comments::CommentState::Sent(_))));
        // The gates ran before the call and the host's yes was final; the chip, not the send, follows the pane.
        let calls = host.calls.lock().unwrap().clone();
        let prompt_at = calls.iter().position(|c| c == "agent.prompt").unwrap();
        assert!(calls[..prompt_at].iter().any(|c| c == "pane.get"), "{calls:?}");
    }

    /// A `send_gate` that holds the send on the blocking pool until `released`, counting arrivals in `waiting`.
    fn gate_until(released: &Arc<AtomicBool>, waiting: &Arc<AtomicUsize>) -> Arc<dyn Fn() + Send + Sync> {
        let (released, waiting) = (released.clone(), waiting.clone());
        Arc::new(move || {
            waiting.fetch_add(1, Ordering::SeqCst);
            while !released.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(10));
            }
        })
    }

    #[test]
    fn a_repick_during_the_claim_is_met_before_the_call() {
        let (dir, state, top, host) = sending_fixture();
        let mut config = sending_config(dir.path(), state.path(), host.clone());
        let (released, waiting) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
        config.send_gate = Some(gate_until(&released, &waiting));
        let (_rt, handle) = start_from(config);
        ready_to_send(&handle);
        handle.commands.send(feedback(dispatch::Accepted::default())).unwrap();
        wait_cond("the claim is written", || waiting.load(Ordering::SeqCst) == 1);
        assert!(comments::Store::open(Some(state.path().to_path_buf()), &top, now()).0.comments().iter().all(|c| matches!(c.state, comments::CommentState::Sending { .. })));
        // The reviewer picks another target while the send sits between its claim and its call.
        handle.commands.send(Command::SetTarget(Target::Clipboard)).unwrap();
        wait_for(&handle, "re-picked", |s| s.target_seq == 2 && s.target == Some(Target::Clipboard));
        released.store(true, Ordering::SeqCst);
        let s = wait_for(&handle, "refused", |s| s.send_seq == 1);
        assert_eq!(s.send_error.as_deref(), Some("the target changed; press Y again"));
        assert!(s.comments.iter().all(|c| c.is_pending()), "the claim was undone: {:?}", s.comments.iter().map(|c| &c.state).collect::<Vec<_>>());
        assert_eq!(host.prompts.lock().unwrap().len(), 0, "nothing went to the pane the reviewer left");
    }

    #[test]
    fn a_repick_during_a_clipboard_request_records_and_copies_nothing() {
        let (dir, state, top, host) = sending_fixture();
        let mut config = sending_config(dir.path(), state.path(), host.clone());
        let (released, waiting) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
        config.send_gate = Some(gate_until(&released, &waiting));
        let (_rt, handle) = start_from(config);
        ready_to_send(&handle);
        handle.commands.send(Command::SetTarget(Target::Clipboard)).unwrap();
        wait_for(&handle, "clipboard, diff ready", |s| s.target_state == TargetState::Clipboard && matches!(s.diff, DiffState::Ready(_)));
        handle.commands.send(Command::Send(dispatch::SendRequest { kind: dispatch::SendKind::Review { scope: dispatch::ReviewScope::All }, accepted: Default::default() })).unwrap();
        wait_cond("the request is recorded", || waiting.load(Ordering::SeqCst) == 1);
        assert_eq!(comments::load_requests(state.path(), &top).0.len(), 1);
        handle.commands.send(Command::SetTarget(pane_target("w4:p2", "codex", Some("s1")))).unwrap();
        wait_for(&handle, "re-picked", |s| s.target_seq == 3);
        released.store(true, Ordering::SeqCst);
        let s = wait_for(&handle, "refused", |s| s.send_seq == 1);
        assert_eq!(s.send_error.as_deref(), Some("the target changed; press @ again"));
        assert_eq!(s.copy_seq, 0, "nothing was copied");
        assert_eq!(comments::load_requests(state.path(), &top).0.len(), 0, "the record was removed");
        assert!(!state.path().join("clipboard.md").exists());
    }

    #[test]
    fn a_copy_that_went_out_survives_a_failed_settlement() {
        let (dir, state, _top, host) = sending_fixture();
        let mut config = sending_config(dir.path(), state.path(), host.clone());
        let (released, waiting) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
        config.send_gate = Some(gate_until(&released, &waiting));
        let (_rt, handle) = start_from(config);
        ready_to_send(&handle);
        handle.commands.send(Command::SetTarget(Target::Clipboard)).unwrap();
        wait_for(&handle, "clipboard", |s| s.target_state == TargetState::Clipboard);
        handle.commands.send(feedback(dispatch::Accepted::default())).unwrap();
        wait_cond("claimed", || waiting.load(Ordering::SeqCst) == 1);
        // The directory turns read-only between the claim and the copy: neither clipboard.md nor the stamps
        // can be written, and the OSC sequence must still reach the shell. (As root the directory stays
        // writable and the send succeeds outright; the sequence is asserted either way.)
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(state.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        released.store(true, Ordering::SeqCst);
        let s = wait_for(&handle, "answered", |s| s.send_seq == 1);
        std::fs::set_permissions(state.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(s.copy_seq, 1, "the copy rode on the answer");
        assert!(s.copy.as_ref().unwrap().osc.is_some(), "the sequence survived the failed stamps");
        match &s.send_error {
            Some(error) => assert!(error.starts_with("copied 2 comments") && error.contains("not marked sent"), "{error}"),
            None => assert!(s.send_outcome.as_ref().unwrap().copy.is_some()),
        }
    }

    #[test]
    fn c_still_copies_while_the_store_cannot_be_read() {
        let (dir, state, _top, host) = sending_fixture();
        let (_rt, handle) = start_from(sending_config(dir.path(), state.path(), host.clone()));
        ready_to_send(&handle);
        // comments.json becomes unreadable: refreshes keep what is in memory, transactions journal, and
        // `c` copies with a nonce fresh against the array it holds (10.7). (Root reads it anyway.)
        use std::os::unix::fs::PermissionsExt;
        let file = state.path().join("comments.json");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
        handle.commands.send(Command::Copy(dispatch::CopyRequest { what: dispatch::CopyWhat::Review })).unwrap();
        let s = wait_for(&handle, "copied", |s| s.copy_seq == 1);
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        let copy = s.copy.as_ref().unwrap();
        assert!(copy.notice.starts_with("copied 2 comments"), "{}", copy.notice);
        assert!(copy.osc.is_some());
    }

    #[test]
    fn a_second_send_in_flight_is_refused_and_a_claim_that_finds_nothing_sends_nothing() {
        let (dir, state, top, host) = sending_fixture();
        *host.prompt_delay.lock().unwrap() = Some(Duration::from_millis(600));
        let (_rt, handle) = start_from(sending_config(dir.path(), state.path(), host.clone()));
        ready_to_send(&handle);
        handle.commands.send(feedback(dispatch::Accepted::default())).unwrap();
        handle.commands.send(feedback(dispatch::Accepted::default())).unwrap();
        let s = wait_for(&handle, "second refused", |s| s.send_seq == 1);
        assert_eq!(s.send_error.as_deref(), Some(dispatch::NOTICE_IN_PROGRESS));
        wait_for(&handle, "first sent", |s| s.send_seq == 2 && s.send_error.is_none());
        // Two sessions on one worktree: the second finds every comment claimed by the first. The second
        // never refreshes (an hour's poll, no watcher events), so its snapshot still shows the comments
        // pending when it sends, and the conflict is met in the claim, not in the command's early check.
        handle.commands.send(pending(2, "third")).unwrap();
        wait_for(&handle, "third pending", |s| s.comments.len() == 3);
        let state2 = state.path().to_path_buf();
        let mut config = sending_config(dir.path(), &state2, host.clone());
        config.nonce = Arc::new(|counter| format!("m{counter:05}"));
        config.poll_interval = Duration::from_secs(3600);
        let (_rt2, other) = start_from(config);
        wait_for(&other, "shares the comments", |s| s.comments.len() == 3 && s.target.is_some());
        *host.prompt_delay.lock().unwrap() = Some(Duration::from_millis(900));
        handle.commands.send(feedback(dispatch::Accepted::default())).unwrap();
        wait_cond("the first claim is in the file", || comments::Store::open(Some(state.path().to_path_buf()), &top, now()).0.comments().iter().all(|c| !c.is_pending()));
        other.commands.send(feedback(dispatch::Accepted::default())).unwrap();
        let s = wait_for(&other, "nothing left", |s| s.send_seq == 1);
        assert_eq!(s.send_error.as_deref(), Some(dispatch::NOTICE_NOTHING));
        wait_for(&handle, "third sent", |s| s.send_seq == 3 && s.send_error.is_none());
        assert_eq!(host.prompts.lock().unwrap().len(), 2, "one prompt per claim that held something");
    }

    #[test]
    fn two_viewers_sending_to_one_pane_take_the_send_lock_in_turn() {
        let (dir, state, _top, host) = sending_fixture();
        *host.prompt_delay.lock().unwrap() = Some(Duration::from_millis(700));
        let (_rt, a) = start_from(sending_config(dir.path(), state.path(), host.clone()));
        ready_to_send(&a);
        // A second worktree of the same user shares the state directory and the pane.
        let dir2 = fixture();
        let mut config = sending_config(dir2.path(), state.path(), host.clone());
        config.nonce = Arc::new(|counter| format!("m{counter:05}"));
        let (_rt2, b) = start_from(config);
        wait_for(&b, "rows", |s| !s.files.is_empty());
        b.commands.send(Command::SetTarget(pane_target("w4:p2", "codex", Some("s1")))).unwrap();
        wait_for(&b, "live", |s| s.target_state == TargetState::Live("idle".into()));
        b.commands.send(pending(2, "from b")).unwrap();
        wait_for(&b, "pending", |s| s.comments.len() == 1);
        a.commands.send(feedback(dispatch::Accepted::default())).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        b.commands.send(feedback(dispatch::Accepted::default())).unwrap();
        let waited = wait_for(&b, "b waits", |s| s.send_waiting);
        assert!(waited.send_waiting);
        wait_for(&a, "a sent", |s| s.send_seq == 1 && s.send_error.is_none());
        let s = wait_for(&b, "b sent", |s| s.send_seq == 1);
        assert!(s.send_error.is_none(), "{:?}", s.send_error);
        assert!(!s.send_waiting);
        let prompts = host.prompts.lock().unwrap().clone();
        assert_eq!(prompts.len(), 2);
        assert!(prompts[0].1.contains("first") && prompts[1].1.contains("from b"), "a's paste came first");
        // The second call began after the first answered plus the Enter margin: the gap between the two
        // call starts is the first's delay (its answer) plus the margin, which the lock alone would not add.
        let starts = host.prompt_started.lock().unwrap().clone();
        assert_eq!(starts.len(), 2);
        assert!(starts[1].duration_since(starts[0]) >= Duration::from_millis(700 + 500), "{:?}", starts[1].duration_since(starts[0]));
    }

    /// `dispatch::request_files` on a snapshot the test shapes: the three request paths the
    /// session test does not reach.
    #[tokio::test]
    async fn request_files_reads_failed_selected_rows_keeps_unreadable_rows_and_pins_the_branch_base() {
        let dir = fixture();
        let top = dir.path().canonicalize().unwrap().to_string_lossy().into_owned();
        let (late_tx, _late_rx) = unbounded_channel();
        let ctx = |snapshot: Snapshot| dispatch::Context {
            toplevel: top.clone(),
            state_dir: None,
            host: None,
            host_wait: Duration::from_secs(1),
            late: late_tx.clone(),
            socket_path: None,
            target: None,
            worktree_renames: BTreeMap::new(),
            generation: 0,
            latest_generation: Arc::new(AtomicU64::new(0)),
            nonce: Arc::new(|c| format!("n{c:05}")),
            nonce_counter: 0,
            snapshot: Arc::new(snapshot),
            lane: Arc::new(Semaphore::new(1)),
            clock: Arc::new(now),
            send_gate: None,
        };
        let row = |path: &str| ChangedFile { path: path.into(), status: ChangedFileStatus::Modified, staged: false, insertions: None, deletions: None };
        // A selected row whose diff is `Failed` on screen is loaded by the task like the others.
        let mut snapshot = Snapshot::empty(&top);
        snapshot.repo = RepoState::Repo { toplevel: top.clone(), branch: Some("main".into()), worktree: None };
        snapshot.files = vec![row("a.txt"), row("b.txt")];
        snapshot.selected = Some(FileKey::of(&snapshot.files[0]));
        snapshot.diff = DiffState::Failed("boom".into());
        let (lines, files) = dispatch::request_files(&ctx(snapshot.clone()), &dispatch::ReviewScope::All).await.unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(files[0].additions, vec![(1, 2)], "the failed selected row was read from disk");
        // A row the loader cannot read is recorded with no ranges and still listed.
        snapshot.files.push(row("vanished.txt"));
        let (lines, files) = dispatch::request_files(&ctx(snapshot.clone()), &dispatch::ReviewScope::All).await.unwrap();
        assert_eq!(lines[2].path, "vanished.txt");
        assert!(files[2].additions.is_empty() && files[2].deletions.is_empty());
        // Branch scope: the ranges and the record are against the merge-base the snapshot carries.
        let head = String::from_utf8(std::process::Command::new("git").args(["-C", &top, "rev-parse", "HEAD"]).output().unwrap().stdout).unwrap().trim().to_string();
        snapshot.scope = Scope::Branch;
        snapshot.base = Some(Base { requested: "refs/heads/main".into(), commit: head.clone(), merge_base: Some(head.clone()), source: BaseSource::Default });
        snapshot.files.truncate(2);
        let (_, files) = dispatch::request_files(&ctx(snapshot), &dispatch::ReviewScope::All).await.unwrap();
        assert!(files.iter().all(|f| matches!(&f.comparison, comments::AnchorComparison::Branch { merge_base, label } if *merge_base == head && label == "main")));
        assert_eq!(files[0].additions, vec![(1, 2)]);
    }

    #[test]
    fn a_colliding_nonce_is_passed_over_and_the_bound_is_measured_on_the_encoded_line() {
        let (dir, state, top, host) = sending_fixture();
        let (_rt, handle) = start_from(sending_config(dir.path(), state.path(), host.clone()));
        ready_to_send(&handle);
        // A retained record already carries the nonce the seam would make first.
        let mut seeded = comments::Store::open(Some(state.path().to_path_buf()), &top, 1).0;
        let mut earlier = seeded.comments()[0].clone();
        earlier.id = comments::new_id();
        earlier.text = "earlier".into();
        earlier.state = comments::CommentState::Sent(comments::Stamp { at: 1, nonce: "n00001".into(), item: 1, to: target::Destination::clipboard() });
        seeded.transact(comments::Operation::Add(earlier), 1).unwrap();
        handle.commands.send(Command::Refresh).unwrap();
        wait_for(&handle, "seeded", |s| s.comments.len() == 3);
        handle.commands.send(feedback(dispatch::Accepted::default())).unwrap();
        let s = wait_for(&handle, "sent", |s| s.send_seq == 1);
        assert!(s.send_error.is_none(), "{:?}", s.send_error);
        let nonce = s.comments.iter().find(|c| c.text == "first").unwrap().stamp().unwrap().nonce.clone();
        assert_eq!(nonce, "n00002", "the retained nonce was passed over");
        assert_eq!(s.send_outcome.as_ref().unwrap().items, 2, "the sent record was not claimed again");
        // The bound counts the bytes the socket would carry, prefixes and escaping included.
        let text = "x".repeat(dispatch::REQUEST_BOUND - 10);
        assert!(dispatch::encoded_len("w4:p2", &text) > dispatch::REQUEST_BOUND);
        assert!(dispatch::encoded_len("w4:p2", "> short") < 200);
        let quoted = "\"".repeat(dispatch::REQUEST_BOUND / 4);
        assert!(dispatch::encoded_len("w4:p2", &quoted) > dispatch::REQUEST_BOUND / 2, "escaping is counted");
    }

    #[test]
    fn a_clipboard_send_is_sent_when_the_file_was_written_and_unconfirmed_when_only_the_sequence_went_out() {
        let dir = fixture();
        let state = tempfile::tempdir().unwrap();
        let mut config = test_config(dir.path(), Arc::new(AtomicBool::new(true)));
        config.state_dir = Some(state.path().to_path_buf());
        config.nonce = Arc::new(|counter| format!("n{counter:05}"));
        let (_rt, handle) = start_from(config);
        wait_for(&handle, "rows", |s| !s.files.is_empty());
        handle.commands.send(Command::SetTarget(Target::Clipboard)).unwrap();
        wait_for(&handle, "clipboard", |s| s.target_seq == 1);
        handle.commands.send(pending(2, "first")).unwrap();
        wait_for(&handle, "pending", |s| s.comments.len() == 1);
        handle.commands.send(feedback(dispatch::Accepted::default())).unwrap();
        let s = wait_for(&handle, "copied", |s| s.send_seq == 1);
        assert!(s.send_error.is_none());
        let out = s.send_outcome.clone().unwrap();
        assert!(!out.unconfirmed && out.copy.as_ref().unwrap().osc.as_deref().unwrap().starts_with("\x1b]52;c;"));
        assert_eq!(s.copy_seq, 1);
        assert!(std::fs::read_to_string(state.path().join("clipboard.md")).unwrap().starts_with("> Inline review — 1 item."));
        assert!(matches!(&s.comments[0].state, comments::CommentState::Sent(st) if st.to == target::Destination::clipboard()));
        // Without a state directory: the sequence alone, Unconfirmed.
        let mut config = config_no_state(dir.path());
        // The maker's second nonce is the first one again: the request must pass it over.
        config.nonce = Arc::new(|counter| if counter == 2 { "n00001".to_string() } else { format!("n{counter:05}") });
        let (_rt2, bare) = start_from(config);
        wait_for(&bare, "rows, diff ready", |s| !s.files.is_empty() && matches!(s.diff, DiffState::Ready(_)));
        bare.commands.send(Command::SetTarget(Target::Clipboard)).unwrap();
        bare.commands.send(pending(2, "x")).unwrap();
        wait_for(&bare, "pending", |s| s.comments.len() == 1);
        bare.commands.send(feedback(dispatch::Accepted::default())).unwrap();
        let s = wait_for(&bare, "unconfirmed copy", |s| s.send_seq == 1);
        assert!(s.send_outcome.as_ref().unwrap().unconfirmed);
        assert!(matches!(&s.comments[0].state, comments::CommentState::Unconfirmed { stamp, .. } if stamp.nonce == "n00001"));
        assert!(s.send_outcome.as_ref().unwrap().copy.as_ref().unwrap().osc.is_some());
        // A request with no file to record in still takes a nonce the array does not hold: the maker's
        // "n00001" (counter 2) is passed over for counter 3, so the next feedback is stamped under counter 4.
        bare.commands.send(Command::Send(dispatch::SendRequest { kind: dispatch::SendKind::Review { scope: dispatch::ReviewScope::All }, accepted: Default::default() })).unwrap();
        let s = wait_for(&bare, "request copied", |s| s.send_seq == 2);
        assert!(s.send_error.is_none(), "{:?}", s.send_error);
        bare.commands.send(pending(1, "y")).unwrap();
        wait_for(&bare, "pending again", |s| s.comments.iter().any(|c| c.is_pending()));
        bare.commands.send(feedback(dispatch::Accepted::default())).unwrap();
        let s = wait_for(&bare, "sent again", |s| s.send_seq == 3);
        assert!(s.comments.iter().any(|c| matches!(&c.state, comments::CommentState::Unconfirmed { stamp, .. } if stamp.nonce == "n00004")), "the request took n00003, not the stamped n00001: {:?}", s.comments.iter().map(|c| &c.state).collect::<Vec<_>>());
        // Over the OSC limit with a writable directory: the file alone, no sequence, still Sent.
        let wide = "字".repeat(comments::MAX_CHARS);
        for _ in 0..30 {
            handle.commands.send(pending(2, &wide)).unwrap();
        }
        wait_for(&handle, "thirty more", |s| s.comments.iter().filter(|c| c.is_pending()).count() == 30);
        handle.commands.send(feedback(dispatch::Accepted::default())).unwrap();
        let s = wait_for(&handle, "file only", |s| s.send_seq == 2);
        let out = s.send_outcome.clone().unwrap();
        assert!(out.copy.as_ref().unwrap().osc.is_none() && !out.unconfirmed);
        assert!(out.copy.as_ref().unwrap().notice.contains("too long for the terminal's clipboard"));
        assert!(s.comments.iter().all(|c| matches!(c.state, comments::CommentState::Sent(_))));
        // Over the limit with no directory either: nothing can receive it; nothing is stamped, the
        // earlier unconfirmed record keeps its stamp, the box says why.
        for _ in 0..30 {
            bare.commands.send(pending(2, &wide)).unwrap();
        }
        let before = wait_for(&bare, "thirty pending", |s| s.comments.iter().filter(|c| c.is_pending()).count() == 30);
        let earlier = Some(before.comments[0].state.clone());
        bare.commands.send(feedback(dispatch::Accepted::default())).unwrap();
        let s = wait_for(&bare, "nothing could receive", |s| s.send_seq == 2);
        assert!(s.send_error.as_deref().unwrap().starts_with("nothing could receive the copy: "), "{:?}", s.send_error);
        assert_eq!(s.comments.iter().filter(|c| c.is_pending()).count(), 30);
        assert!(matches!(&s.comments[0].state, comments::CommentState::Unconfirmed { stamp, .. } if Some(stamp.nonce.as_str()) == earlier.as_ref().and_then(|e| e.stamp_nonce())));
    }

    #[test]
    fn copy_stamps_nothing_and_a_review_request_is_recorded_with_ranges_read_before_the_check() {
        let (dir, state, top, host) = sending_fixture();
        let (_rt, handle) = start_from(sending_config(dir.path(), state.path(), host.clone()));
        ready_to_send(&handle);
        handle.commands.send(Command::Copy(dispatch::CopyRequest { what: dispatch::CopyWhat::Review })).unwrap();
        // One predicate: the copy's answer and the ready diff the request needs may arrive together.
        let s = wait_for(&handle, "copied, diff ready", |s| s.copy_seq == 1 && matches!(s.diff, DiffState::Ready(_)));
        assert!(s.copy.as_ref().unwrap().notice.starts_with("copied 2 comments · also in "));
        assert!(s.comments.iter().all(|c| c.is_pending()), "`c` claims nothing");
        // An all-changes request: one entry per row, ranges from the diffs, the selected row's included.
        handle.commands.send(Command::Send(dispatch::SendRequest { kind: dispatch::SendKind::Review { scope: dispatch::ReviewScope::All }, accepted: Default::default() })).unwrap();
        let s = wait_for(&handle, "requested", |s| s.send_seq == 1);
        assert!(s.send_error.is_none(), "{:?}", s.send_error);
        let (requests, _) = comments::load_requests(state.path(), &top);
        assert_eq!(requests.len(), 1);
        // `-U3` context makes a.txt's one hunk span both lines; b.txt is one line.
        let files: Vec<_> = requests[0].files.iter().map(|f| (f.key.path.as_str(), f.additions.clone())).collect();
        assert_eq!(files, [("a.txt", vec![(1, 2)]), ("b.txt", vec![(1, 1)])]);
        // A staged rename with further unstaged edits: both halves are read with their own source
        // (the staged half's `-M old new`, the unstaged half's none), so neither records empty ranges.
        // The file is given enough common content first, or git sees a deletion and an addition.
        let body: String = (1..=20).map(|i| format!("line {i}\n")).collect();
        std::fs::write(dir.path().join("b.txt"), &body).unwrap();
        git(dir.path(), &["add", "b.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "a longer b"]);
        git(dir.path(), &["mv", "b.txt", "c.txt"]);
        // The staged half carries an edit too, or it is a pure rename with no hunk to range.
        std::fs::write(dir.path().join("c.txt"), format!("{body}more\n")).unwrap();
        git(dir.path(), &["add", "c.txt"]);
        std::fs::write(dir.path().join("c.txt"), format!("{body}more\nand more\n")).unwrap();
        let status = Proc::new("git").arg("-C").arg(dir.path()).args(["status", "--porcelain=v1"]).output().unwrap();
        assert!(String::from_utf8_lossy(&status.stdout).contains("RM b.txt -> c.txt"), "the fixture must be a staged rename with edits on both sides");
        handle.commands.send(Command::Refresh).unwrap();
        wait_for(&handle, "both halves listed", |s| s.files.iter().filter(|f| f.path == "c.txt").count() == 2);
        handle.commands.send(Command::Send(dispatch::SendRequest { kind: dispatch::SendKind::Review { scope: dispatch::ReviewScope::All }, accepted: Default::default() })).unwrap();
        let s = wait_for(&handle, "requested again", |s| s.send_seq == 2);
        assert!(s.send_error.is_none(), "{:?}", s.send_error);
        let (requests, _) = comments::load_requests(state.path(), &top);
        let halves: Vec<_> = requests[1].files.iter().filter(|f| f.key.path == "c.txt").map(|f| (f.key.staged, f.additions.clone())).collect();
        assert_eq!(halves.len(), 2);
        assert!(halves.iter().all(|(_, ranges)| !ranges.is_empty()), "{halves:?}");
        assert_eq!(requests[0].target, target::Destination::Pane { pane: "w4:p2".into(), agent: "codex".into(), session: Some(host::SessionRef { kind: "id".into(), value: "s1".into() }) });
        let prompts = host.prompts.lock().unwrap().clone();
        assert!(prompts[0].1.starts_with(&format!("> Delegate a code review of these 2 changes:\n> unstaged diff (`git -C '{top}' diff`):\n> ─ a.txt ({top}/a.txt)\n> ─ b.txt ({top}/b.txt)\n>\n")));
        assert!(prompts[0].1.contains(&format!("\"nonce\":\"{}\"", requests[0].nonce)));
        // The check follows the reads: a pane that is blocked by the time they are done refuses, records nothing.
        host.set_pane(agent_pane("w4:p2", "codex", "blocked", Some("s1"), &top));
        handle.commands.send(Command::Send(dispatch::SendRequest { kind: dispatch::SendKind::Review { scope: dispatch::ReviewScope::All }, accepted: Default::default() })).unwrap();
        let s = wait_for(&handle, "refused", |s| s.send_seq == 3);
        assert!(s.send_error.as_deref().unwrap().starts_with("codex is waiting for an approval"));
        assert_eq!(comments::load_requests(state.path(), &top).0.len(), 2);
        // `c` in the Request box records the request with a clipboard destination.
        handle.commands.send(Command::Copy(dispatch::CopyRequest { what: dispatch::CopyWhat::Request { scope: dispatch::ReviewScope::All } })).unwrap();
        wait_for(&handle, "request copied", |s| s.copy_seq == 2);
        let (requests, _) = comments::load_requests(state.path(), &top);
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[2].target, target::Destination::clipboard());
        assert!(std::fs::read_to_string(state.path().join("clipboard.md")).unwrap().starts_with("> Delegate a code review"));
    }
```

`config_no_state(dir)` is `test_config(dir, Arc::new(AtomicBool::new(true)))` with `state_dir` left `None`; write it as a two-line helper beside `test_config`. `stamp_nonce()` in the clipboard test and `is_pending_state()` in the settlement test are three-line test helpers on `CommentState` (the nonce of an `Unconfirmed` or `Sent` state as `Option<&str>`; `matches!(self, Pending)`). The refusal-before-stamp path of the bound is `a_claim_takes_pending...`'s `Err("too large")` case in `comments.rs`; `encoded_len` is proven here on synthetic texts because fifty comments within the caps cannot reach 512 KiB.

Two more session tests, for the late answer and the oversized review:

```rust
    #[test]
    fn a_late_host_answer_settles_the_unconfirmed_records_by_nonce() {
        let (dir, state, _top, host) = sending_fixture();
        let mut config = sending_config(dir.path(), state.path(), host.clone());
        config.host_wait = Duration::from_millis(300);
        *host.prompt_delay.lock().unwrap() = Some(Duration::from_millis(900));
        let (_rt, handle) = start_from(config);
        ready_to_send(&handle);
        handle.commands.send(feedback(dispatch::Accepted::default())).unwrap();
        let s = wait_for(&handle, "unconfirmed at the wait", |s| s.send_seq == 1);
        assert!(s.send_outcome.as_ref().unwrap().unconfirmed);
        assert!(s.comments.iter().all(|c| matches!(c.state, comments::CommentState::Unconfirmed { .. })));
        // The host's yes arrives 600 ms later and is final.
        wait_for(&handle, "settled late", |s| s.comments.iter().all(|c| matches!(&c.state, comments::CommentState::Sent(st) if st.nonce == "n00001")));
        // A late definite failure returns a record to what it was.
        handle.commands.send(pending(2, "third")).unwrap();
        wait_for(&handle, "third", |s| s.comments.len() == 3);
        host.prompt_results.lock().unwrap().push(Err(host::HostFailure::Api { code: "agent_not_found".into(), message: "gone".into() }));
        handle.commands.send(feedback(dispatch::Accepted::default())).unwrap();
        wait_for(&handle, "unconfirmed again", |s| s.send_seq == 2 && s.send_outcome.as_ref().is_some_and(|o| o.unconfirmed));
        wait_for(&handle, "returned to pending", |s| s.comments.iter().any(|c| c.text == "third" && c.is_pending()));
    }

    #[test]
    fn a_call_that_outlives_the_wait_keeps_the_send_lock_until_it_answers() {
        let (dir, state, _top, host) = sending_fixture();
        let mut config = sending_config(dir.path(), state.path(), host.clone());
        config.host_wait = Duration::from_millis(300);
        *host.prompt_delay.lock().unwrap() = Some(Duration::from_millis(900));
        let (_rt, a) = start_from(config);
        ready_to_send(&a);
        let dir2 = fixture();
        let mut config = sending_config(dir2.path(), state.path(), host.clone());
        config.nonce = Arc::new(|counter| format!("m{counter:05}"));
        let (_rt2, b) = start_from(config);
        wait_for(&b, "rows", |s| !s.files.is_empty());
        b.commands.send(Command::SetTarget(pane_target("w4:p2", "codex", Some("s1")))).unwrap();
        wait_for(&b, "live", |s| s.target_state == TargetState::Live("idle".into()));
        b.commands.send(pending(2, "from b")).unwrap();
        wait_for(&b, "pending", |s| s.comments.len() == 1);
        a.commands.send(feedback(dispatch::Accepted::default())).unwrap();
        wait_for(&a, "a unconfirmed at the wait", |s| s.send_seq == 1 && s.send_outcome.as_ref().is_some_and(|o| o.unconfirmed));
        // a's call is still running; b's send must wait for it, not paste into the same line.
        b.commands.send(feedback(dispatch::Accepted::default())).unwrap();
        wait_for(&b, "b sent", |s| s.send_seq == 1 && s.send_error.is_none());
        let starts = host.prompt_started.lock().unwrap().clone();
        assert_eq!(starts.len(), 2);
        assert!(starts[1].duration_since(starts[0]) >= Duration::from_millis(900 + 500), "{:?}", starts[1].duration_since(starts[0]));
        wait_for(&a, "a settled late", |s| s.comments.iter().all(|c| matches!(c.state, comments::CommentState::Sent(_))));
    }

    #[test]
    fn an_oversized_review_is_refused_before_any_stamp_and_without_a_call() {
        let (dir, state, _top, host) = sending_fixture();
        let (_rt, handle) = start_from(sending_config(dir.path(), state.path(), host.clone()));
        ready_to_send(&handle);
        // Fifty comments of 4,000 CJK characters are 600,000 bytes of text, over the 512 KiB bound.
        let wide = "字".repeat(comments::MAX_CHARS);
        for _ in 0..48 {
            handle.commands.send(pending(2, &wide)).unwrap();
        }
        wait_for(&handle, "fifty", |s| s.comments.len() == 50);
        handle.commands.send(feedback(dispatch::Accepted::default())).unwrap();
        let s = wait_for(&handle, "refused", |s| s.send_seq == 1);
        assert!(s.send_error.as_deref().unwrap().starts_with("review too large to send at once ("), "{:?}", s.send_error);
        assert!(s.comments.iter().all(|c| c.is_pending()), "nothing was stamped");
        assert_eq!(host.prompts.lock().unwrap().len(), 0, "the host was not called");
        // An oversized request leaves requests.json untouched: the bound runs inside the reservation. The
        // request text names each file twice, so 350 untracked files three directories deep, on a path of
        // some 750 characters, put it past 512 KiB. One name is capped at 255 bytes everywhere and a path
        // at 1,024 on the Darwin targets (4,096 on Linux); this stays under both.
        let before = std::fs::read_to_string(state.path().join("requests.json")).ok();
        let deep = (0..3).fold(dir.path().to_path_buf(), |p, i| p.join(format!("{i}{}", "d".repeat(229))));
        std::fs::create_dir_all(&deep).unwrap();
        for i in 0..350 {
            std::fs::write(deep.join(format!("{i:03}.txt")), "x\n").unwrap();
        }
        handle.commands.send(Command::Refresh).unwrap();
        wait_for(&handle, "the rows", |s| s.files.len() >= 350);
        handle.commands.send(Command::Send(dispatch::SendRequest { kind: dispatch::SendKind::Review { scope: dispatch::ReviewScope::All }, accepted: Default::default() })).unwrap();
        let s = wait_for(&handle, "request refused", |s| s.send_seq == 2);
        assert!(s.send_error.as_deref().unwrap().starts_with("review too large"), "{:?}", s.send_error);
        assert_eq!(std::fs::read_to_string(state.path().join("requests.json")).ok(), before, "a refused request was recorded");
    }
```

And `two_viewers_sending_to_one_pane_take_the_send_lock_in_turn`'s last assertion measures the gap the margin creates, not the total: the scripted host records when each prompt call began (`Scripted.prompt_started: Mutex<Vec<Instant>>`, pushed before the delay) and the test asserts `started[1] - started[0] >= delay + ENTER_MARGIN` (700 + 500 ms). Two calls serialised by the lock alone would begin 700 ms apart (the first's delay); only the margin makes it 1,200.

- [ ] **Step 6: Run them**

Run: `cargo test --locked --lib engine::session::tests -- --test-threads=1`
Expected: all pass. Falsifications to see once each: remove the `before` restoration in `settle` and watch `host_answers_settle_the_claim_each_their_way` fail on the retry assertion; remove the `guard` from `claim` and watch `a_claim_takes_pending...`'s changed-target case fail; comment out `ENTER_MARGIN`'s sleep and watch `two_viewers_sending...` fail on the gap assertion.

- [ ] **Step 7: Gates and commit**

```bash
git add src/engine/dispatch.rs src/engine/comments.rs src/engine/types.rs src/engine/session.rs src/engine/mod.rs src/actions/reuse.rs tests/actions_tier_a.rs
git commit -m "feat(engine): send and copy a review, claimed before the call and settled by nonce"
```

### Task 6: The target in the TUI: the chip and the pane picker

Implements spec 10.2 "The picker" and "The chip", 10.6's picker and chip sentences, and 10.9 tests 8 (the chip, the picker) and 9 (`A`). Read 10.2 before starting, and `src/tui/picker.rs` beside it: the pane picker is built the way the base picker is, as its own type, because `Picker`'s rows, observe and submit are the base picker's.

**Files:**
- Create: `src/tui/panes.rs`
- Modify: `src/tui/mod.rs`, `src/tui/keys.rs` (`A`, `KeyAction::PickPane`), `src/tui/state.rs` (`panes`, `panes_token`, `socket_path`, observe), `src/tui/input.rs` (`A`, the picker's keys, mouse rows), `src/tui/view.rs` (the chip, `Action::PickPane`, `Action::PickPaneRow`, the overlay, `body_is_drawn`), `src/tui/shell.rs` (`socket_path` from the environment)

**Interfaces:**
- Consumes: Task 2's `Snapshot.{target, target_state, target_seq, target_error, panes, panes_seq, panes_error}`, `Command::{LoadPanes, SetTarget}`, `PaneRow`, `Target`, `TargetState`.
- Produces:

```rust
// src/tui/panes.rs
pub const WIDTH: u16 = 72;
pub const NO_AGENT: &str = "No agent pane in this session. Start claude, codex or kimi in a pane, then press A.";
pub const CLIPBOARD_ROW: &str = "✂ clipboard · copy the review instead of sending it";

/// Where the picker returns when a pick lands; Tasks 7 and 8 add their variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReturnTo { Nothing }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice { Pane(PaneRow), Clipboard }
impl Choice { pub fn target(&self, socket_path: &str) -> Target; }

#[derive(Debug)]
pub struct PanePicker { pub input: String, pub cursor: usize, pub offset: usize, pub token: u64,
                        pub pending: Option<u64>, pub done: bool, pub return_to: ReturnTo, seen_panes_seq: u64 }
impl PanePicker {
    pub fn open(token: u64, return_to: ReturnTo) -> Self;
    /// This worktree's panes, the others, the clipboard; filtered by the input.
    pub fn choices(&self, snapshot: &Snapshot) -> Vec<Choice>;
    pub fn panel(&self, snapshot: &Snapshot, width: u16, height: u16) -> Panel;
    pub fn visible(&self, height: u16) -> usize;
    pub fn move_by(&mut self, delta: isize, choices: &[Choice], visible: usize) -> bool;
    pub fn retarget(&mut self, snapshot: &Snapshot);
    /// Closes on the answered pick; the error, if any, is the viewer's notice (observe takes it).
    pub fn observe(&mut self, snapshot: &Snapshot);
    /// The rendered line (within the panel's rows) where `choices[i]` is drawn, wrapped notes counted at their rendered height; `None` when it is not drawn.
    pub fn panel_line(&self, snapshot: &Snapshot, width: u16, height: u16, choice: usize) -> Option<usize>;
}

/// The chip of 10.2: text and tone from the target and its state.
pub fn chip(snapshot: &Snapshot) -> (String, Option<Semantic>, bool /* dim */);

// src/tui/keys.rs
KeyAction::PickPane   // "A", label "send to", vimeflow None
// src/tui/view.rs
Action::PickPane, Action::PickPaneRow(usize)
// src/tui/state.rs: ViewState gains
pub panes: Option<PanePicker>,
pub panes_token: u64,
pub socket_path: Option<String>,   // HERDR_SOCKET_PATH, for the Target a pick builds
deferred: VecDeque<String>,        // urgent warnings an answer displaced; the front speaks when nothing urgent stands
seen_target_seq: u64,
```

- [ ] **Step 1: Tests first, the chip and the picker's rows**

Create `src/tui/panes.rs` with tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::host::{PaneRecord, SessionRef};
    use crate::engine::{PaneRow, Snapshot, Target, TargetState};

    fn pane(id: &str, agent: &str, status: &str, cwd: &str, title: &str, this: bool) -> PaneRow {
        PaneRow {
            record: PaneRecord {
                pane_id: id.into(),
                agent: Some(agent.into()),
                agent_status: Some(status.into()),
                agent_session: Some(SessionRef { kind: "id".into(), value: "s".into() }),
                cwd: Some(cwd.into()),
                title: Some(title.into()),
                ..PaneRecord::default()
            },
            this_worktree: this,
        }
    }

    fn snapshot_with(panes: Vec<PaneRow>, seq: u64) -> Snapshot {
        let mut s = Snapshot::empty("/r");
        s.panes = Some(std::sync::Arc::new(panes));
        s.panes_seq = seq;
        s
    }

    #[test]
    fn the_chip_reads_each_state_and_host_absence_outranks_target_absence() {
        let mut s = Snapshot::empty("/r");
        s.target_state = TargetState::NoHost;
        assert_eq!(chip(&s), ("→ no host".to_string(), None, true));
        s.target_state = TargetState::Unverified;
        assert_eq!(chip(&s), ("→ no agent".to_string(), None, true));
        s.target = Some(Target::Pane { pane: "w4:p2".into(), socket: "/s".into(), agent: "codex".into(), session: None, title: String::new() });
        assert_eq!(chip(&s), ("→ codex w4:p2 · ?".to_string(), None, true));
        s.target_state = TargetState::Live("idle".into());
        assert_eq!(chip(&s), ("→ codex w4:p2".to_string(), Some(Semantic::Accent), false));
        s.target_state = TargetState::Live("done".into());
        assert_eq!(chip(&s).0, "→ codex w4:p2");
        s.target_state = TargetState::Live("working".into());
        assert_eq!(chip(&s), ("→ codex w4:p2 · working".to_string(), Some(Semantic::Warn), false));
        s.target_state = TargetState::Live("blocked".into());
        assert_eq!(chip(&s), ("→ codex w4:p2 · blocked".to_string(), Some(Semantic::Warn), false));
        s.target_state = TargetState::Live("unknown".into());
        assert_eq!(chip(&s), ("→ codex w4:p2 · unknown".to_string(), None, true));
        s.target_state = TargetState::Restarted("idle".into());
        assert_eq!(chip(&s), ("→ w4:p2 · restarted".to_string(), Some(Semantic::Warn), false));
        s.target_state = TargetState::Left;
        assert_eq!(chip(&s), ("→ w4:p2 · left".to_string(), Some(Semantic::Bad), false));
        s.target_state = TargetState::Gone;
        assert_eq!(chip(&s), ("→ w4:p2 · gone".to_string(), Some(Semantic::Bad), false));
        s.target_state = TargetState::NoHost;
        assert_eq!(chip(&s), ("→ no host".to_string(), None, true));
        s.target = Some(Target::Clipboard);
        s.target_state = TargetState::Clipboard;
        assert_eq!(chip(&s), ("→ clipboard".to_string(), None, false));
        // Untrusted text: a title or kind with control characters is sanitised before the chip.
        s.target = Some(Target::Pane { pane: "w4:p2".into(), socket: "/s".into(), agent: "co\u{1b}dex".into(), session: None, title: String::new() });
        s.target_state = TargetState::Live("idle".into());
        assert!(!chip(&s).0.contains('\u{1b}'));
    }

    #[test]
    fn choices_come_in_three_groups_and_the_filter_matches_every_column() {
        let panes = vec![
            pane("w1:p3", "claude", "idle", "/home/u/repo/src", "fix the cart", true),
            pane("w1:p2", "codex", "working", "/home/u/other", "lint", false),
            pane("w2:p1", "kimi", "idle", "/home/u/repo", "", true),
        ];
        let picker = PanePicker::open(1, ReturnTo::Nothing);
        let s = snapshot_with(panes.clone(), 1);
        let choices = picker.choices(&s);
        assert_eq!(choices.len(), 4);
        assert!(matches!(&choices[0], Choice::Pane(r) if r.record.pane_id == "w1:p3"));
        assert!(matches!(&choices[1], Choice::Pane(r) if r.record.pane_id == "w2:p1"));
        assert!(matches!(&choices[2], Choice::Pane(r) if r.record.pane_id == "w1:p2"));
        assert_eq!(choices[3], Choice::Clipboard);
        for (input, expected) in [("codex", "w1:p2"), ("p1", "w2:p1"), ("working", "w1:p2"), ("other", "w1:p2"), ("cart", "w1:p3")] {
            let mut picker = PanePicker::open(1, ReturnTo::Nothing);
            picker.input = input.into();
            let choices = picker.choices(&s);
            assert_eq!(choices.len(), 2, "{input}: {choices:?}");
            assert!(matches!(&choices[0], Choice::Pane(r) if r.record.pane_id == expected), "{input}");
            assert_eq!(choices[1], Choice::Clipboard, "the clipboard row is never filtered out");
        }
        // The panel: two headings, the rows, the clipboard; the cursor lands on an entry row.
        let panel = picker.panel(&s, WIDTH, 20);
        let plain: Vec<String> = panel.rows.iter().map(|r| match r {
            Row::Entry { label, value, .. } => format!("E {label} | {value}"),
            Row::Text(t) => format!("T {t}"),
            Row::Note(t) => format!("N {t}"),
            Row::Warn(t) => format!("W {t}"),
            Row::Rule => "R".into(),
        }).collect();
        assert!(plain[0].starts_with("T > _"));
        assert_eq!(plain[1], "T this worktree");
        assert!(plain[2].starts_with("E claude  w1:p3  idle  ~/repo/src") && plain[2].ends_with("| fix the cart"), "{}", plain[2]);
        assert!(plain[3].starts_with("E kimi  w2:p1  idle  ~/repo"));
        assert_eq!(plain[4], "T other panes");
        assert!(plain[5].starts_with("E codex  w1:p2  working  ~/other"));
        assert_eq!(plain[6], format!("E {CLIPBOARD_ROW} | "));
        assert_eq!(panel.cursor, Some(2));
        assert_eq!(picker.panel_line(&s, WIDTH, 20, 3), Some(6));
        assert_eq!(panel.title, "Send to");
        assert_eq!(panel.footer, "Enter pick · Esc cancel · type to filter");
    }

    #[test]
    fn an_empty_host_offers_the_clipboard_alone_and_a_failure_names_its_reason() {
        let picker = PanePicker::open(1, ReturnTo::Nothing);
        let s = snapshot_with(Vec::new(), 1);
        let panel = picker.panel(&s, WIDTH, 20);
        assert!(panel.rows.iter().any(|r| matches!(r, Row::Note(t) if t == NO_AGENT)));
        assert_eq!(picker.choices(&s), vec![Choice::Clipboard]);
        assert_eq!(panel.cursor, Some(2));
        // The no-agent note wraps: two lines at the panel's width, three at 44 columns. The clipboard
        // row's line follows the rendered lines (the input line plus the note's), not its row index.
        assert_eq!(picker.panel_line(&s, WIDTH, 20, 0), Some(3));
        assert_eq!(picker.panel_line(&s, 44, 20, 0), Some(4));
        assert_eq!(picker.panel_line(&s, 44, 20, 1), None, "a choice that does not exist has no line");
        let mut s = s;
        s.panes_error = Some("could not list panes: deadline".into());
        let panel = picker.panel(&s, WIDTH, 20);
        assert!(panel.rows.iter().any(|r| matches!(r, Row::Warn(t) if t == "could not list panes: deadline")));
        assert!(!panel.rows.iter().any(|r| matches!(r, Row::Note(t) if t == NO_AGENT)));
        // Loading: the token has not been answered yet.
        let mut s = Snapshot::empty("/r");
        s.panes_seq = 0;
        let panel = picker.panel(&s, WIDTH, 20);
        assert_eq!(panel.footer, "loading panes…");
        assert_eq!(picker.choices(&s), vec![Choice::Clipboard]);
    }

    #[test]
    fn a_choice_becomes_the_target_it_names() {
        let row = pane("w1:p3", "claude", "idle", "/r", "fix", true);
        let target = Choice::Pane(row.clone()).target("/run/h.sock");
        assert_eq!(target, Target::Pane { pane: "w1:p3".into(), socket: "/run/h.sock".into(), agent: "claude".into(), session: row.record.agent_session.clone(), title: "fix".into() });
        assert_eq!(Choice::Clipboard.target("/run/h.sock"), Target::Clipboard);
    }

    #[test]
    fn observe_closes_on_the_answered_pick_and_follows_the_rows() {
        let mut picker = PanePicker::open(3, ReturnTo::Nothing);
        let mut s = snapshot_with(vec![pane("w1:p3", "claude", "idle", "/r", "", true)], 2);
        picker.observe(&s);
        assert_eq!(picker.cursor, 0);
        s.panes_seq = 3;
        picker.observe(&s);
        picker.pending = Some(s.target_seq);
        s.target_seq += 1;
        picker.observe(&s);
        assert!(picker.done);
    }
}
```

- [ ] **Step 2: Run to watch them fail**

Add `pub mod panes;` to `src/tui/mod.rs` first. Run: `cargo test --locked --lib tui::panes`
Expected: compile errors.

- [ ] **Step 3: The module**

```rust
//! The pane picker of spec 10.2, built like the base picker: an input line that filters, three groups.
use crate::engine::{PaneRow, Snapshot, Target, TargetState};
use crate::tui::dialog::{self, Panel, Row};
use crate::tui::format::truncate;
use crate::tui::sanitize::sanitize;
use crate::tui::style::Semantic;

pub const WIDTH: u16 = 72;
pub const NO_AGENT: &str =
    "No agent pane in this session. Start claude, codex or kimi in a pane, then press A.";
pub const CLIPBOARD_ROW: &str = "✂ clipboard · copy the review instead of sending it";

/// Where the picker returns when a pick lands; Tasks 7 and 8 add their variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReturnTo {
    Nothing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    Pane(PaneRow),
    Clipboard,
}

impl Choice {
    pub fn target(&self, socket_path: &str) -> Target {
        match self {
            Self::Clipboard => Target::Clipboard,
            Self::Pane(row) => Target::Pane {
                pane: row.record.pane_id.clone(),
                socket: socket_path.to_string(),
                agent: row.record.agent.clone().unwrap_or_default(),
                session: row.record.agent_session.clone(),
                title: row
                    .record
                    .label
                    .clone()
                    .or_else(|| row.record.title.clone())
                    .unwrap_or_default(),
            },
        }
    }
}

/// `~`-relative when under HOME, as the picker shows directories.
fn relative_to_home(dir: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && dir.starts_with(&home) => format!("~{}", &dir[home.len()..]),
        _ => dir.to_string(),
    }
}

fn columns(row: &PaneRow) -> (String, String, String, String, String) {
    let record = &row.record;
    let dir = record
        .foreground_cwd
        .clone()
        .or_else(|| record.cwd.clone())
        .map(|d| relative_to_home(&d))
        .unwrap_or_default();
    (
        sanitize(record.agent.as_deref().unwrap_or("?")),
        sanitize(&record.pane_id),
        sanitize(record.agent_status.as_deref().unwrap_or("unknown")),
        sanitize(&dir),
        sanitize(record.label.as_deref().or(record.title.as_deref()).unwrap_or("")),
    )
}

#[derive(Debug)]
pub struct PanePicker {
    pub input: String,
    pub cursor: usize,
    pub offset: usize,
    pub token: u64,
    /// The `target_seq` at the time of the pick; the picker closes when it moves past it.
    pub pending: Option<u64>,
    pub done: bool,
    pub return_to: ReturnTo,
    seen_panes_seq: u64,
}

impl PanePicker {
    pub fn open(token: u64, return_to: ReturnTo) -> Self {
        Self {
            input: String::new(),
            cursor: 0,
            offset: 0,
            token,
            pending: None,
            done: false,
            return_to,
            seen_panes_seq: 0,
        }
    }

    fn loaded(&self, snapshot: &Snapshot) -> bool {
        snapshot.panes_seq == self.token
    }

    fn matches(&self, row: &PaneRow) -> bool {
        if self.input.is_empty() {
            return true;
        }
        let needle = self.input.to_lowercase();
        let (agent, id, status, dir, title) = columns(row);
        [agent, id, status, dir, title]
            .iter()
            .any(|column| column.to_lowercase().contains(&needle))
    }

    /// This worktree's panes, then the others, then the clipboard, which no filter removes.
    pub fn choices(&self, snapshot: &Snapshot) -> Vec<Choice> {
        let mut choices = Vec::new();
        if self.loaded(snapshot) {
            if let Some(panes) = &snapshot.panes {
                choices.extend(panes.iter().filter(|r| r.this_worktree && self.matches(r)).cloned().map(Choice::Pane));
                choices.extend(panes.iter().filter(|r| !r.this_worktree && self.matches(r)).cloned().map(Choice::Pane));
            }
        }
        choices.push(Choice::Clipboard);
        choices
    }

    pub fn visible(&self, height: u16) -> usize {
        // The frame, the input line, two headings and the footer leave this many rows for entries.
        usize::from(height.saturating_sub(8)).max(1)
    }

    fn window(&self, visible: usize) -> usize {
        self.offset.min(self.cursor).max(self.cursor.saturating_sub(visible - 1))
    }

    /// The panel's rows: the input line, the error or the no-agent note, the groups with their
    /// headings, the clipboard. Returns the rows and, for each drawn choice, its row index.
    fn rows_and_positions(&self, snapshot: &Snapshot, width: u16, height: u16) -> (Vec<Row>, Vec<usize>) {
        let line = usize::from(width).saturating_sub(4).max(1);
        let mut rows = vec![Row::Text(truncate(&sanitize(&format!("> {}_", self.input)), line))];
        let choices = self.choices(snapshot);
        if let Some(error) = &snapshot.panes_error {
            rows.push(Row::Warn(sanitize(error)));
        } else if self.loaded(snapshot) && choices.len() == 1 {
            rows.push(Row::Note(NO_AGENT.to_string()));
        }
        let visible = self.visible(height);
        let first = self.window(visible);
        let mut positions = vec![usize::MAX; choices.len()];
        let mut last_group: Option<bool> = None;
        for (index, choice) in choices.iter().enumerate().skip(first).take(visible) {
            if let Choice::Pane(row) = choice {
                if last_group != Some(row.this_worktree) {
                    last_group = Some(row.this_worktree);
                    rows.push(Row::Text(if row.this_worktree { "this worktree" } else { "other panes" }.to_string()));
                }
            }
            positions[index] = rows.len();
            rows.push(match choice {
                Choice::Clipboard => Row::Entry { label: CLIPBOARD_ROW.to_string(), value: String::new(), enabled: true },
                Choice::Pane(row) => {
                    let (agent, id, status, dir, title) = columns(row);
                    Row::Entry {
                        label: truncate(&format!("{agent}  {id}  {status}  {dir}"), line.saturating_sub(20)),
                        value: truncate(&title, 18),
                        enabled: true,
                    }
                }
            });
        }
        (rows, positions)
    }

    /// The terminal line (within the panel's rows, frame excluded) where `choices[choice]` is drawn: the
    /// rendered height of every row before it, because a `Note` or `Warn` wraps to several lines at the
    /// panel's width while an `Entry` is one.
    pub fn panel_line(&self, snapshot: &Snapshot, width: u16, height: u16, choice: usize) -> Option<usize> {
        let (rows, positions) = self.rows_and_positions(snapshot, width, height);
        let row = positions.get(choice).copied().filter(|p| *p != usize::MAX)?;
        let before = Panel { title: String::new(), rows: rows[..row].to_vec(), footer: String::new(), cursor: None, offset: 0 };
        Some(dialog::line_count(&before, width))
    }

    pub fn panel(&self, snapshot: &Snapshot, width: u16, height: u16) -> Panel {
        let (rows, positions) = self.rows_and_positions(snapshot, width, height);
        let footer = if self.pending.is_some() {
            "picking…".to_string()
        } else if !self.loaded(snapshot) {
            "loading panes…".to_string()
        } else {
            "Enter pick · Esc cancel · type to filter".to_string()
        };
        Panel {
            title: "Send to".into(),
            rows,
            footer,
            cursor: positions.get(self.cursor).copied().filter(|p| *p != usize::MAX),
            offset: 0,
        }
    }

    pub fn move_by(&mut self, delta: isize, choices: &[Choice], visible: usize) -> bool {
        let len = choices.len();
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

    /// After an edit: the first match, the clipboard when nothing matches.
    pub fn retarget(&mut self, snapshot: &Snapshot) {
        let choices = self.choices(snapshot);
        self.cursor = self.cursor.min(choices.len().saturating_sub(1));
        if !self.input.is_empty() {
            self.cursor = 0;
        }
        self.offset = 0;
    }

    /// A new row list re-places the cursor; the answered pick closes the picker.
    pub fn observe(&mut self, snapshot: &Snapshot) {
        if self.loaded(snapshot) && self.seen_panes_seq != snapshot.panes_seq {
            self.seen_panes_seq = snapshot.panes_seq;
            self.retarget(snapshot);
        }
        if let Some(sent) = self.pending {
            if snapshot.target_seq > sent {
                self.pending = None;
                self.done = true;
            }
        }
    }
}

/// The chip of spec 10.2: `(text, tone, dim)`. Host absence outranks target absence.
pub fn chip(snapshot: &Snapshot) -> (String, Option<Semantic>, bool) {
    let state = &snapshot.target_state;
    match (&snapshot.target, state) {
        (_, TargetState::NoHost) => ("→ no host".to_string(), None, true),
        (None, _) => ("→ no agent".to_string(), None, true),
        (Some(Target::Clipboard), _) => ("→ clipboard".to_string(), None, false),
        (Some(Target::Pane { pane, agent, .. }), state) => {
            let pane = truncate(&sanitize(pane), 12);
            let agent = truncate(&sanitize(agent), 12);
            match state {
                TargetState::Live(status) if status == "idle" || status == "done" => {
                    (format!("→ {agent} {pane}"), Some(Semantic::Accent), false)
                }
                TargetState::Live(status) if status == "unknown" => {
                    (format!("→ {agent} {pane} · unknown"), None, true)
                }
                TargetState::Live(status) => {
                    (format!("→ {agent} {pane} · {}", truncate(&sanitize(status), 8)), Some(Semantic::Warn), false)
                }
                TargetState::Unverified => (format!("→ {agent} {pane} · ?"), None, true),
                TargetState::Restarted(_) => (format!("→ {pane} · restarted"), Some(Semantic::Warn), false),
                TargetState::Left => (format!("→ {pane} · left"), Some(Semantic::Bad), false),
                TargetState::Gone => (format!("→ {pane} · gone"), Some(Semantic::Bad), false),
                TargetState::NoHost | TargetState::Clipboard => unreachable!("matched above"),
            }
        }
    }
}
```

Add `pub mod panes;` to `src/tui/mod.rs`. The `relative_to_home` read of `HOME` is a display concern, not state; it is the same `~` the base picker's `age` text leaves alone.

- [ ] **Step 4: Run the module tests**

Run: `cargo test --locked --lib tui::panes`
Expected: five pass. The `choices_come_in_three_groups` test's `plain[2]` assertion depends on `HOME=/home/u`; set it in the test with `std::env::set_var("HOME", "/home/u")` under the `--test-threads=1` rule, or assert on `contains("repo/src")` instead. Prefer the latter: change both `~/…` assertions to `contains`.

- [ ] **Step 5: Keys, state, input, view, shell**

1. `keys.rs`: a binding after `MarkReviewed`:

```rust
    Binding {
        key: "A",
        label: "send to (pane)",
        action: KeyAction::PickPane,
        vimeflow: None,
    },
```

`KeyAction::PickPane` added; in `keys.rs`'s `navigation_aliases_require_no_modifiers_and_do_not_extend_the_sheet`, the `KEYS.len()` pin becomes 28 and `help_panel(false).rows.len()` 28 with it (one entry row per binding, still no notes); the two `help_offset` arithmetic tests in `input.rs` still hold (they use `KEYS.len()`). `RESERVED` is untouched until Task 8.

2. `state.rs`: `ViewState` gains `pub panes: Option<crate::tui::panes::PanePicker>`, `pub panes_token: u64`, `pub socket_path: Option<String>`, `seen_target_seq: u64`, `deferred: VecDeque<String>`; `new` sets `None, 0, None, 0, VecDeque::new()`. In `observe`, before the mark block:

```rust
        if let Some(picker) = &mut self.panes {
            picker.observe(snapshot);
            if picker.done {
                self.panes = None;
            }
        }
        if snapshot.target_seq != self.seen_target_seq {
            self.seen_target_seq = snapshot.target_seq;
            if let Some(error) = &snapshot.target_error {
                self.warn(crate::tui::sanitize::sanitize(error));
            }
        }
```

A notice set here follows invariant 3 as the mark answer does: it is the answer to the key just pressed and shows at once; the displaced-warning bookkeeping of the mark block applies, so factor that `if let Some(displaced)` block into a method and call it from all three places (mark, action, target), and give the `Other` kind a way back, which 0.0.4 had no need of (its only urgent `Other` warnings were answers themselves) and which Tasks 7 and 8 need, since comment, send and copy answers arrive on their own and two of them can arrive before either is read:

```rust
    /// The answer to a key shows at once; what it displaces comes back once the answer is read: a
    /// classification warning regenerates from the snapshot, a base error returns to its slot, and
    /// any other unread warning waits in `deferred`, in order, so no answer is lost behind another.
    fn displace_urgent(&mut self) {
        if let Some(displaced) = self.notice.as_ref().filter(|n| n.urgent) {
            match self.notice_kind {
                NoticeKind::Rewrite => self.seen_rewrite = None,
                NoticeKind::BaseError => self.pending_base_error = Some(displaced.text.clone()),
                NoticeKind::Other => self.deferred.push_back(displaced.text.clone()),
            }
        }
    }
```

with `deferred: VecDeque<String>` on `ViewState` and, at the end of `observe` after the base error's replay, `if !answered_now && self.notice.is_none() { if let Some(text) = self.deferred.pop_front() { self.warn(text); } }`: a deferred warning speaks only into an empty slot, never over a notice that has not been read, urgent or not (an answer shown through `notify`, a send error outside a box, must survive the unrelated snapshots that follow it); `answered_now` is the disjunction of the answered flags of this `observe` (mark, action, target and, as the later tasks add them, comment, send, copy). `input::apply_action` already re-runs `observe` when a key clears a notice, which is when the next deferred warning speaks. A state test here, `two_answers_before_a_key_keep_the_first_warning`, observes a snapshot with a target error (`target_seq` 1, `target_error` set) and then one with another target error (`target_seq` 2) before any key, asserts the second is shown, observes an unrelated snapshot (nothing answered) and asserts the second still stands, and after a key clears it (`handle_key` with a navigation key) asserts the first is shown; Task 8's `a_comment_answer_and_a_send_answer_in_a_row_both_speak` does the same with a comment error followed by a send error, an unrelated snapshot in between.

3. `input.rs`: in `handle_key`, before the `state.picker.is_some()` check, `if state.panes.is_some() { return panes_key(state, snapshot, key); }`:

```rust
fn panes_key(state: &mut ViewState, snapshot: &Snapshot, key: KeyEvent) -> Outcome {
    let overlay = state.body_height.saturating_add(u16::from(view::notice(state, snapshot).is_some()));
    let Some(picker) = state.panes.as_mut() else { return Outcome::Inert };
    let choices = picker.choices(snapshot);
    let visible = picker.visible(overlay);
    match (key.code, key.modifiers) {
        (KeyCode::Esc, KeyModifiers::NONE) => {
            state.panes = None;
            Outcome::Redraw
        }
        (KeyCode::Enter, KeyModifiers::NONE) => pick_pane(state, snapshot, picker_cursor(state)),
        (KeyCode::Down, KeyModifiers::NONE) | (KeyCode::Char('n'), KeyModifiers::CONTROL) => redraw_if(picker.move_by(1, &choices, visible)),
        (KeyCode::Up, KeyModifiers::NONE) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => redraw_if(picker.move_by(-1, &choices, visible)),
        (KeyCode::Backspace, KeyModifiers::NONE) => {
            if picker.input.pop().is_some() {
                picker.retarget(snapshot);
                Outcome::Redraw
            } else {
                Outcome::Inert
            }
        }
        (KeyCode::Char(ch), KeyModifiers::NONE | KeyModifiers::SHIFT) if !ch.is_control() => {
            picker.input.push(ch);
            picker.retarget(snapshot);
            Outcome::Redraw
        }
        _ => Outcome::Inert,
    }
}

fn redraw_if(moved: bool) -> Outcome {
    if moved { Outcome::Redraw } else { Outcome::Inert }
}

fn picker_cursor(state: &ViewState) -> usize {
    state.panes.as_ref().map(|p| p.cursor).unwrap_or(0)
}

/// `Enter` or a click: the choice becomes `SetTarget`; the picker waits for the answer.
fn pick_pane(state: &mut ViewState, snapshot: &Snapshot, index: usize) -> Outcome {
    let socket = state.socket_path.clone().unwrap_or_default();
    let Some(picker) = state.panes.as_mut() else { return Outcome::Inert };
    if picker.pending.is_some() {
        return Outcome::Inert;
    }
    let choices = picker.choices(snapshot);
    let Some(choice) = choices.get(index) else { return Outcome::Inert };
    picker.cursor = index;
    picker.pending = Some(snapshot.target_seq);
    Outcome::Engine(Command::SetTarget(choice.target(&socket)))
}
```

and in `act`: `PickPane => { state.panes_token += 1; state.panes = Some(PanePicker::open(state.panes_token, ReturnTo::Nothing)); return Outcome::Engine(Command::LoadPanes(state.panes_token)); }`. In `handle_mouse`: the pane picker's scroll and click mirror the base picker's (`Action::PickPaneRow(i)` → `pick_pane`), and while it is open nothing under it is hit.

4. `view.rs`: `Action` gains `PickPane` (→ `KeyAction::PickPane`) and `PickPaneRow(usize)` (→ `None`). `ToolbarItem` gains a tone: `type ToolbarItem = (Vec<(String, Option<Action>)>, u8, Option<Semantic>, bool)` — the last two the chip's `(tone, dim)`, `(None, false)` for every other item; in `toolbar`, a clickable span takes the item's tone instead of `Semantic::Accent` when one is given, and a dim item draws `Role::Label` with no reverse. The chip is pushed after the stats:

```rust
    let (chip_text, tone, dim) = crate::tui::panes::chip(snapshot);
    items.push((vec![(chip_text, Some(Action::PickPane))], 2, tone, dim));
```

and the drop orders shift up by one from the scope chip on (scope 3, view 4, STAGED 5, stats 6, files 7, busy 8, actions 9), so the target chip drops last but one. `Action::PickPane` is always enabled. The overlay: after the base picker's block, the same for `state.panes` with `panes::WIDTH`, hits `Action::PickPaneRow(first + i)` for each choice `first + i` whose `picker.panel_line(snapshot, panel_width, panel_height, first + i)` is `Some(line)`, at `y = 2 + line` (the rendered line, so a wrapped note or warning above moves the hits with the rows it pushes down; `the_pane_picker_overlay_lists_groups_and_hits_its_rows` also renders the empty-host picker at 44 columns and asserts the clipboard row's one hit is at `y = 6`, the line that shows it, below the three-line note). `body_is_drawn` gains `&& state.panes.is_none()`.

5. `shell.rs`: `state.socket_path = std::env::var("HERDR_SOCKET_PATH").ok();` in `run_terminal` after `state.popup`.

- [ ] **Step 6: Input and view tests**

The tests of this and the next two tasks press `Enter`, `Esc`, `Backspace` and the arrows; `input.rs`'s `key` helper only knows characters and `ctrl+`. Extend it first:

```rust
    fn key(text: &str) -> KeyEvent {
        let code = match text {
            "Enter" => KeyCode::Enter,
            "Esc" => KeyCode::Esc,
            "Backspace" => KeyCode::Backspace,
            "Down" => KeyCode::Down,
            "Up" => KeyCode::Up,
            _ => {
                return match text.strip_prefix("ctrl+") {
                    Some(c) => KeyEvent::new(KeyCode::Char(c.chars().next().unwrap()), KeyModifiers::CONTROL),
                    None => KeyEvent::new(KeyCode::Char(text.chars().next().unwrap()), KeyModifiers::NONE),
                }
            }
        };
        KeyEvent::new(code, KeyModifiers::NONE)
    }
```

Every existing test keeps passing: no existing call passes one of the five words. In `input.rs`'s tests (using its `setup` helper):

```rust
    #[test]
    fn a_opens_the_pane_picker_and_enter_picks_a_target() {
        let (mut snap, mut st) = setup(&[(10, " --+ ")]);
        st.socket_path = Some("/run/h.sock".into());
        assert_eq!(handle_key(&mut st, &snap, key("A"), 120), Outcome::Engine(Command::LoadPanes(1)));
        assert!(st.panes.is_some());
        // Loading: Enter on the clipboard row is the only pick possible, and it is a pick.
        snap.panes = Some(std::sync::Arc::new(vec![crate::engine::PaneRow {
            record: crate::engine::host::PaneRecord { pane_id: "w1:p2".into(), agent: Some("codex".into()), ..Default::default() },
            this_worktree: true,
        }]));
        snap.panes_seq = 1;
        st.observe(&snap);
        let outcome = handle_key(&mut st, &snap, key("Enter"), 120);
        assert!(matches!(outcome, Outcome::Engine(Command::SetTarget(crate::engine::Target::Pane { pane, socket, .. })) if pane == "w1:p2" && socket == "/run/h.sock"));
        // A second Enter while the pick is pending does nothing; the answer closes the picker.
        assert_eq!(handle_key(&mut st, &snap, key("Enter"), 120), Outcome::Inert);
        snap.target_seq = 1;
        st.observe(&snap);
        assert!(st.panes.is_none());
        // Esc keeps the target as it is.
        handle_key(&mut st, &snap, key("A"), 120);
        assert_eq!(handle_key(&mut st, &snap, key("Esc"), 120), Outcome::Redraw);
        assert!(st.panes.is_none());
        // A refused pick (not remembered) is the viewer's notice, once.
        snap.target_seq = 2;
        snap.target_error = Some("target not remembered: read-only".into());
        st.observe(&snap);
        assert_eq!(st.notice.as_ref().map(|n| (n.text.as_str(), n.urgent)), Some(("target not remembered: read-only", true)));
    }

    #[test]
    fn two_answers_before_a_key_keep_the_first_warning() {
        let (mut snap, mut st) = setup(&[(10, " --+ ")]);
        snap.target_seq = 1;
        snap.target_error = Some("target not remembered: read-only".into());
        st.observe(&snap);
        // A second answer before the first is read: it shows at once, the first waits.
        snap.target_seq = 2;
        snap.target_error = Some("target not remembered: disk full".into());
        st.observe(&snap);
        assert_eq!(st.notice.as_ref().map(|n| n.text.as_str()), Some("target not remembered: disk full"));
        // An unrelated snapshot (a refresh that answered nothing) leaves the unread answer where it is.
        snap.refreshing = !snap.refreshing;
        st.observe(&snap);
        assert_eq!(st.notice.as_ref().map(|n| n.text.as_str()), Some("target not remembered: disk full"));
        // Read: the first speaks; read again: nothing more is owed.
        handle_key(&mut st, &snap, key("j"), 120);
        assert_eq!(st.notice.as_ref().map(|n| n.text.as_str()), Some("target not remembered: read-only"), "an answer never loses the warning it displaced");
        handle_key(&mut st, &snap, key("j"), 120);
        assert!(st.notice.is_none());
    }
```

In `view.rs`'s tests: `the_target_chip_is_drawn_and_drops_last_but_one` renders at 160 and 70 columns with `target_state = Live("idle")` and asserts `→ codex w4:p2` is in the first line at 160 and still at 70 while `files` and the stats are gone, and that its span is reverse accent like every other clickable chip; and `the_pane_picker_overlay_lists_groups_and_hits_its_rows` renders with the picker open and asserts the headings, the clipboard row and one `Action::PickPaneRow(0)` hit on the first entry's line. The existing `toolbar_chips_are_padded_reversed_accent_bold_and_plain_text_is_not` renders a snapshot with no target, where the chip reads `→ no agent`, is clickable and dim: it is the one clickable span that is not reverse accent, so the test changes in two named ways and no other: the chip count becomes 11 (the seven Phase 1 chips, the three action chips, the target chip), and the style assertion runs on every clickable span except the one whose action is `Action::PickPane`, which is asserted separately to be padded, hit over all its cells, and styled `Style::role(Role::Label)` with `reverse: false`. The ten original chips keep the reverse-accent guarantee untouched.

- [ ] **Step 7: Gates and commit**

```bash
git add src/tui/panes.rs src/tui/mod.rs src/tui/keys.rs src/tui/state.rs src/tui/input.rs src/tui/view.rs src/tui/shell.rs
git commit -m "feat(tui): the target chip and the pane picker behind A"
```

### Task 7: Comments in the TUI: cards, the orphan section, the editor, the selection

Implements spec 10.3 "Cards", "The editor", "Keys", "The target comes first", "Marks and counts" (the panel), the orphan paragraph, and 10.6 "The view" (rows, reconcile, the editor splice); 10.9 tests 8 (cards, editor, panel) and 9 (the comment keys), and Review Focus item 2. Read 10.3 before starting.

**Files:**
- Create: `src/tui/cards.rs`, `src/tui/review.rs`
- Modify: `src/tui/mod.rs`, `src/tui/rows.rs` (`Row::Card`, `Row::Orphans`, `build` with comments), `src/tui/state.rs` (`editor`, `visual`, `orphan`, the rebuild rule, observe for `comment_seq`, the picker's `ReturnTo::Editor`), `src/tui/input.rs` (`i I v u U x X`, the editor's keys, the orphan cursor), `src/tui/keys.rs` (the seven bindings), `src/tui/view.rs` (card lines, the editor splice, `✎`, the header count, card hits), `src/tui/panes.rs` (`ReturnTo::Editor`)

**Interfaces:**
- Consumes: Task 3's `Comment`, `Anchor`, `Span`, `Category`, `CommentState`, `check_text`, `NOTICE_LIMIT`, `Command::{AddComment, EditComment, DeleteComment}`; Task 6's `PanePicker`, `ReturnTo`; 0.0.4's `nav::{find, Target, Side}`, `rows::Rows`, `dialog`.
- Produces:

```rust
// src/tui/cards.rs
/// One terminal row of a card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardLine { Top { title: String, tone: Option<Semantic>, dim: bool }, Text(String), Bottom }
pub fn tone(category: Category) -> Option<Semantic>;          // Question Accent, Bug Warn, others None
pub fn state_word(state: &CommentState) -> &'static str;        // pending, sending, sent?, sent
pub fn place(anchor: &Anchor) -> String;                        // ":16", ":3-9", " (file)"
pub fn title(comment: &Comment, orphan: bool) -> String;        // "Bug · pending"; orphan: "src/cart.py:16 · Bug · pending"
/// Word-wrapped by terminal cells, never empty; a word wider than `width` is cut.
pub fn wrap(text: &str, width: usize) -> Vec<String>;
pub fn lines(comment: &Comment, width: usize, orphan: bool) -> Vec<CardLine>;
/// The one width cards are wrapped and drawn at: the body past the gutter (12 cells unified, 6 split), at least MIN_WIDTH.
pub fn card_width(body_width: usize, mode: ViewMode) -> usize;
/// The three frame drawers `view.rs` uses for cards and the editor alike.
pub fn frame_top(title: Vec<Span>, width: usize) -> Line;      // ╭─ title ───╮
pub fn frame_text(text: &str, width: usize) -> Line;           // │ text      │
pub fn frame_bottom(footer: &str, width: usize) -> Line;       // ╰─ footer ──╯
pub const MIN_WIDTH: usize = 12;

// src/tui/rows.rs
Row::Card { id: String, target: Option<usize>, line: CardLine }   // `target`: the anchor's target, None for an orphan
Row::Orphans { count: usize }
pub struct Rows { /* existing */ pub orphan_tops: Vec<(usize, String)> }   // row index of each orphan card's top line, and its id
/// Which comments a frame shows in place: the diff's key under the current comparison kind.
pub fn in_place<'a>(comments: &'a [Comment], diff: &LoadedDiff, branch: bool) -> Vec<&'a Comment>;
/// Pending or unconfirmed comments under the current comparison kind whose row is gone.
pub fn orphans<'a>(comments: &'a [Comment], snapshot: &Snapshot) -> Vec<&'a Comment>;
pub fn build(diff: &LoadedDiff, mode: ViewMode, comments: &[&Comment], orphans: &[&Comment], width: usize, editor: Option<&EditorPlace>) -> Rows;
/// The body with no diff loaded: the orphan section alone (an empty list with comments left behind).
pub fn orphans_only(orphans: &[&Comment], width: usize, editor: Option<&EditorPlace>) -> Rows;
Row::Editor { line: usize }                       // one per editor line; the text comes from ViewState.editor at draw time
pub struct EditorPlace { pub after: EditorAnchor, pub lines: usize }
pub enum EditorAnchor { Anchor(Anchor), Orphan(String), End }
/// The row a card for `anchor` sits after, in `rows` without cards: the line's row, the range's last, the header.
pub fn attach_row(diff: &LoadedDiff, row_of_target: &[usize], anchor: &Anchor) -> Option<usize>;
/// The anchor's own target (its side and line; a range's last line exactly, never an earlier survivor; none for a file): what a card click lands on and what `u`/`x` match.
pub fn anchor_target(diff: &LoadedDiff, anchor: &Anchor) -> Option<usize>;

// src/tui/review.rs
pub struct Editor { pub anchor: Anchor, pub category: Category, pub text: String, pub editing: Option<Comment> /* the record `u` opened */, pub at_limit: bool, pub pending: Option<u64> /* the token a save was sent under; the draft waits for the answer that carries it */ }
impl Editor {
    pub fn new(anchor: Anchor) -> Self;
    pub fn edit(comment: &Comment) -> Self;
    pub fn insert(&mut self, ch: char);  pub fn newline(&mut self);  pub fn backspace(&mut self);
    pub fn title(&self) -> Vec<Span>;          // comment on R16 · Question Change Bug Suggestion   ctrl+h/l
    pub fn lines(&self, width: usize) -> Vec<CardLine>;
    /// `Enter`: None when the text is whitespace only; otherwise sets `pending = Some(token)` and carries `token` in the command.
    pub fn submit(&mut self, token: u64) -> Option<Command>;
    pub fn place_label(&self) -> String;       // R16, L3-9, file
}
pub struct Visual { pub start: usize, pub diff: Arc<LoadedDiff> }   // the target the selection began on, in this diff; the cursor is its end
pub fn selection(diff: &LoadedDiff, visual: &Visual, cursor: usize) -> Option<(Side, u32, u32)>;
pub fn selection_text(diff: &LoadedDiff, side: Side, start: u32, end: u32) -> String;
pub const NO_LINE: &str = "No diff line selected for comment.";
pub const NO_COMMENT: &str = "No comment selected.";
pub const CHOOSE_PANE: &str = "choose the pane this review goes to";

// src/tui/panes.rs
ReturnTo::Editor(Anchor)

// src/tui/state.rs: ViewState gains
pub editor: Option<Editor>,
pub visual: Option<Visual>,
/// The cursor is on orphan card n (the diff cursor is then not drawn).
pub orphan: Option<usize>,
seen_comment_seq: u64,
// built_from grows to (Arc<LoadedDiff>, ViewMode, Arc<Vec<Comment>>, u64 /* files+scope hash */, u16 /* width */)

// src/tui/keys.rs
KeyAction::{Comment, CommentFile, Visual, EditComment, EditFileComment, DeleteComment, DeleteFileComment}
```

- [ ] **Step 1: Cards and rows, tests first**

In `src/tui/cards.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_wraps_wide_characters_by_cells() {
        let lines = wrap("字字字字字字", 7);             // each 2 cells: three per 7-cell line
        assert_eq!(lines, ["字字字", "字字字"]);
        let lines = wrap("one two three four", 9);
        assert_eq!(lines, ["one two ", "three ", "four"]);
        assert_eq!(wrap("", 9), [""]);
        assert_eq!(wrap("abcdefghijkl", 5), ["abcde", "fghij", "kl"]);
        assert_eq!(wrap("字字字字", 3), ["字", "字", "字", "字"], "a cut never lands inside a character");
        assert_eq!(wrap("a\nb", 9), ["a", "b"], "a newline is a hard break");
        assert_eq!(wrap("  indented\n    deeper", 20), ["  indented", "    deeper"], "indentation is content");
        assert_eq!(wrap("one two", 7), ["one two"]);
        assert_eq!(wrap("one two three", 8), ["one two ", "three"], "the space that broke the line stays with it");
        let text = "  a  b   c";
        assert_eq!(wrap(text, 4).concat(), text, "every character, spaces included, appears exactly once");
        let text = "字".repeat(4_000);
        assert_eq!(wrap(&text, 80).len(), 100);
    }

    #[test]
    fn titles_name_the_category_state_and_for_an_orphan_the_place() {
        let mut c = test_comment();
        assert_eq!(title(&c, false), "Bug · pending");
        c.state = crate::engine::comments::CommentState::Unconfirmed { stamp: test_stamp(), before: Vec::new() };
        assert_eq!(title(&c, false), "Bug · sent?");
        c.state = crate::engine::comments::CommentState::Sent(test_stamp());
        assert_eq!(title(&c, true), "src/cart.py:16 · Bug · sent");
        c.anchor.span = crate::engine::comments::Span::Range { end: 20 };
        assert_eq!(place(&c.anchor), ":16-20");
        c.anchor.span = crate::engine::comments::Span::File;
        assert_eq!(place(&c.anchor), " (file)");
        assert_eq!(tone(Category::Question), Some(Semantic::Accent));
        assert_eq!(tone(Category::Bug), Some(Semantic::Warn));
        assert_eq!(tone(Category::Change), None);
    }

    #[test]
    fn the_frame_fits_its_width_exactly() {
        let top = frame_top(vec![Span::body("Bug · pending")], 30);
        let text: String = top.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(crate::tui::format::width(&text), 30);
        assert!(text.starts_with("╭─ Bug · pending ─") && text.ends_with("╮"));
        let body: String = frame_text("hello", 30).iter().map(|s| s.text.as_str()).collect();
        assert_eq!(crate::tui::format::width(&body), 30);
        assert!(body.starts_with("│ hello") && body.ends_with("│"));
        let bottom: String = frame_bottom("", 30).iter().map(|s| s.text.as_str()).collect();
        assert_eq!(bottom, format!("╰{}╯", "─".repeat(28)));
        // A control character in the text never reaches a cell.
        let body: String = frame_text("a\u{1b}[31mb", 30).iter().map(|s| s.text.as_str()).collect();
        assert!(!body.contains('\u{1b}'));
    }
}
```

(`test_comment`/`test_stamp` are small constructors in the test module: a `Bug` on `src/cart.py:16`, additions, worktree.) In `src/tui/rows.rs`'s tests, with the existing `diff` fixture helpers of that module (or `state::tests::snapshot`):

```rust
    #[test]
    fn cards_sit_under_their_lines_ranges_and_header_and_orphans_close_the_body() {
        let snap = crate::tui::state::tests::snapshot("f", "raw", &[(10, " --+ "), (40, " + ")]);
        let diff = match &snap.diff { DiffState::Ready(d) => d.clone(), _ => unreachable!() };
        let line = comment("l", &diff.key, Side::Additions, 11, Span::Line);
        let range = comment("r", &diff.key, Side::Deletions, 11, Span::Range { end: 12 });
        let file = comment("f", &diff.key, Side::Additions, 0, Span::File);
        let orphan = comment("o", &FileKey { path: "gone.rs".into(), staged: false, untracked: false }, Side::Additions, 3, Span::Line);
        let rows = build(&diff, ViewMode::Unified, &[&line, &range, &file], &[&orphan], 60, None);
        let kinds: Vec<String> = rows.rows.iter().map(|r| match r {
            Row::FileHeader { .. } => "H".into(),
            Row::HunkHeader { .. } => "@".into(),
            Row::Unified { old_no, new_no, .. } => format!("{}/{}", old_no.map_or("-".into(), |n| n.to_string()), new_no.map_or("-".into(), |n| n.to_string())),
            Row::Card { id, line: CardLine::Top { .. }, .. } => format!("[{id}"),
            Row::Card { line: CardLine::Text(_), .. } => "|".into(),
            Row::Card { line: CardLine::Bottom, .. } => "]".into(),
            Row::Orphans { count } => format!("orphans {count}"),
            _ => "?".into(),
        }).collect();
        // The file card follows the header; the range card the last deleted line (12); the line card line 11.
        assert_eq!(kinds[..3], ["H", "[f", "|"]);
        assert!(kinds.windows(4).any(|w| w == ["10/10", "11/-", "12/-", "[r"]), "{kinds:?}");
        let line_pos = kinds.iter().position(|k| k == "[l").unwrap();
        assert_eq!(kinds[line_pos - 1], "-/11");
        let tail = &kinds[kinds.len() - 4..];
        assert_eq!(tail, ["orphans 1", "[o", "|", "]"]);
        assert_eq!(rows.orphan_tops, vec![(kinds.len() - 3, "o".to_string())]);
        // Every target still maps to its own row, cards notwithstanding.
        for (t, &row) in rows.row_of_target.iter().enumerate() {
            assert!(matches!(&rows.rows[row], Row::Unified { target, .. } if *target == t), "target {t} at row {row}");
        }
        // Card rows carry the anchor's own target so a click lands on the line, on the anchor's side.
        let card = rows.rows.iter().find(|r| matches!(r, Row::Card { id, .. } if id == "l")).unwrap();
        assert!(matches!(card, Row::Card { target: Some(t), .. } if diff.targets[*t].side == Side::Additions && diff.targets[*t].line_number == 11));
        let range_card = rows.rows.iter().find(|r| matches!(r, Row::Card { id, .. } if id == "r")).unwrap();
        assert!(matches!(range_card, Row::Card { target: Some(t), .. } if diff.targets[*t].side == Side::Deletions && diff.targets[*t].line_number == 12));
        assert!(rows.rows.iter().any(|r| matches!(r, Row::Card { id, target: None, .. } if id == "o")));
    }

    #[test]
    fn only_this_rows_comments_under_this_comparison_kind_are_in_place() {
        let snap = crate::tui::state::tests::snapshot("f", "raw", &[(10, " + ")]);
        let diff = match &snap.diff { DiffState::Ready(d) => d.clone(), _ => unreachable!() };
        let mine = comment("a", &diff.key, Side::Additions, 10, Span::Line);
        let other_half = comment("b", &FileKey { staged: true, ..diff.key.clone() }, Side::Additions, 10, Span::Line);
        let mut branch = mine.clone();
        branch.id = "c".into();
        branch.anchor.comparison = AnchorComparison::Branch { merge_base: "0".repeat(40), label: "main".into() };
        let all = vec![mine.clone(), other_half, branch.clone()];
        let shown = in_place(&all, &diff, false);
        assert_eq!(shown.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["a"]);
        let two = vec![mine, branch];
        assert_eq!(in_place(&two, &diff, true).iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["c"]);
    }

    #[test]
    fn orphans_are_unsent_comments_whose_row_left_the_list() {
        let mut snap = crate::tui::state::tests::snapshot("f", "raw", &[(10, " + ")]);
        // `snapshot` lists no files; the present row must be listed for its comment not to be an orphan.
        snap.files = vec![crate::git::ChangedFile { path: "f".into(), status: crate::git::ChangedFileStatus::Modified, staged: false, insertions: None, deletions: None }];
        let key = FileKey { path: "f".into(), staged: false, untracked: false };
        let present = comment("p", &key, Side::Additions, 10, Span::Line);
        let gone = comment("g", &FileKey { path: "gone".into(), staged: false, untracked: false }, Side::Additions, 1, Span::Line);
        let mut sent_gone = gone.clone();
        sent_gone.id = "s".into();
        sent_gone.state = CommentState::Sent(stamp());
        let mut other_kind = gone.clone();
        other_kind.id = "k".into();
        other_kind.anchor.comparison = AnchorComparison::Branch { merge_base: "0".repeat(40), label: "main".into() };
        snap.comments = std::sync::Arc::new(vec![present, gone, sent_gone, other_kind]);
        let ids: Vec<_> = orphans(&snap.comments, &snap).iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["g"], "present rows, sent comments and the other comparison kind are not orphans");
    }
```

- [ ] **Step 2: Run to watch them fail**

Add `pub mod cards;` and `pub mod review;` to `src/tui/mod.rs` first. Run: `cargo test --locked --lib tui::cards && cargo test --locked --lib tui::rows`
Expected: compile errors.

- [ ] **Step 3: `cards.rs`**

```rust
//! Cards (spec 10.3): a comment drawn as a rounded frame under its line; the editor shares the frame.
use crate::engine::comments::{Anchor, Category, Comment, CommentState, Span as AnchorSpan};
use crate::tui::format::{truncate, width};
use crate::tui::sanitize::sanitize;
use crate::tui::style::{Line, Role, Semantic, Span, Style};

pub const MIN_WIDTH: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardLine {
    Top { title: String, tone: Option<Semantic>, dim: bool },
    Text(String),
    Bottom,
}

pub fn tone(category: Category) -> Option<Semantic> {
    match category {
        Category::Question => Some(Semantic::Accent),
        Category::Bug => Some(Semantic::Warn),
        Category::Change | Category::Suggestion => None,
    }
}

/// The width a card is wrapped *and* drawn at: the body's columns past the line-number gutter.
/// `rows::build` and `view::body_line` both take it from here, so no text is wrapped for one width
/// and framed at another.
pub fn card_width(body_width: usize, mode: crate::engine::nav::ViewMode) -> usize {
    let gutter = match mode {
        crate::engine::nav::ViewMode::Unified => 12,
        crate::engine::nav::ViewMode::Split => 6,
    };
    body_width.saturating_sub(gutter).max(MIN_WIDTH)
}

pub fn state_word(state: &CommentState) -> &'static str {
    match state {
        CommentState::Pending => "pending",
        CommentState::Sending { .. } => "sending",
        CommentState::Unconfirmed { .. } => "sent?",
        CommentState::Sent(_) => "sent",
    }
}

pub fn place(anchor: &Anchor) -> String {
    match anchor.span {
        AnchorSpan::File => " (file)".to_string(),
        AnchorSpan::Line => format!(":{}", anchor.line),
        AnchorSpan::Range { end } => format!(":{}-{end}", anchor.line),
    }
}

pub fn title(comment: &Comment, orphan: bool) -> String {
    let core = format!("{} · {}", comment.category.short(), state_word(&comment.state));
    if orphan {
        format!("{}{} · {core}", sanitize(&comment.anchor.key.path), place(&comment.anchor))
    } else {
        core
    }
}

/// Word-wrapped by terminal cells; a newline is a hard break; a line breaks after a space when the
/// next word would not fit, a word wider than the width is cut by whole characters, and every
/// character of the text, spaces included, appears exactly once (`truncate` appends `…` and is
/// not used here): the cards show the indentation the prompt sends.
pub fn wrap(text: &str, width_cells: usize) -> Vec<String> {
    let width_cells = width_cells.max(1);
    let mut out = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        // Each piece is a run of spaces or a run of non-spaces; spaces are content too.
        let mut pieces: Vec<String> = Vec::new();
        for ch in paragraph.chars() {
            match pieces.last_mut() {
                Some(last) if (last.ends_with(' ')) == (ch == ' ') => last.push(ch),
                _ => pieces.push(ch.to_string()),
            }
        }
        for piece in pieces {
            if width(&format!("{line}{piece}")) <= width_cells {
                line.push_str(&piece);
                continue;
            }
            // The piece does not fit after what the line holds: the line ends here (the spaces that
            // fitted stay at its end), and the piece starts the next one, cut by whole characters
            // only when it is wider than a line on its own.
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            for ch in piece.chars() {
                if width(&format!("{line}{ch}")) > width_cells && !line.is_empty() {
                    out.push(std::mem::take(&mut line));
                }
                line.push(ch);
            }
        }
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// The card's rows at `width` cells: the frame takes four, the text the rest.
pub fn lines(comment: &Comment, width_cells: usize, orphan: bool) -> Vec<CardLine> {
    let inner = width_cells.max(MIN_WIDTH) - 4;
    let dim = matches!(comment.state, CommentState::Sent(_));
    let mut lines = vec![CardLine::Top { title: title(comment, orphan), tone: tone(comment.category), dim }];
    // Wrapped first, sanitised line by line: `sanitize` would turn the newlines into control pictures.
    lines.extend(wrap(&comment.text, inner).into_iter().map(|l| CardLine::Text(sanitize(&l))));
    lines.push(CardLine::Bottom);
    lines
}

fn rule(cells: usize) -> Span {
    Span::new("─".repeat(cells), Style::role(Role::Rule))
}

/// `╭─ <title> ───╮`, exactly `width_cells` wide; the title is cut before the frame is.
pub fn frame_top(title: Vec<Span>, width_cells: usize) -> Line {
    let width_cells = width_cells.max(MIN_WIDTH);
    let mut line = vec![Span::new("╭─ ", Style::role(Role::Rule))];
    let room = width_cells - 5; // "╭─ " and " ╮" around the title
    let mut used = 0;
    for span in title {
        if used >= room {
            break;
        }
        let text = truncate(&sanitize(&span.text), room - used);
        used += width(&text);
        line.push(Span::new(text, span.style));
    }
    line.push(Span::body(" "));
    line.push(rule(room - used));
    line.push(Span::new("╮", Style::role(Role::Rule)));
    line
}

/// `│ <text> │`, exactly `width_cells` wide.
pub fn frame_text(text: &str, width_cells: usize) -> Line {
    let width_cells = width_cells.max(MIN_WIDTH);
    let inner = width_cells - 4;
    let text = truncate(&sanitize(text), inner);
    let pad = inner - width(&text);
    vec![
        Span::new("│ ", Style::role(Role::Rule)),
        Span::body(format!("{text}{}", " ".repeat(pad))),
        Span::new(" │", Style::role(Role::Rule)),
    ]
}

/// `╰─ <footer> ──╯`; an empty footer draws a plain rule.
pub fn frame_bottom(footer: &str, width_cells: usize) -> Line {
    let width_cells = width_cells.max(MIN_WIDTH);
    if footer.is_empty() {
        return vec![
            Span::new("╰", Style::role(Role::Rule)),
            rule(width_cells - 2),
            Span::new("╯", Style::role(Role::Rule)),
        ];
    }
    let room = width_cells - 5;
    let footer = truncate(&sanitize(footer), room);
    vec![
        Span::new("╰─ ", Style::role(Role::Rule)),
        Span::label(footer.clone()),
        Span::body(" "),
        rule(room - width(&footer)),
        Span::new("╯", Style::role(Role::Rule)),
    ]
}
```

- [ ] **Step 4: `rows.rs`**

`Row` gains the two variants above; `Rows` gains `pub orphan_tops: Vec<(usize, String)>`. The three functions:

```rust
/// The comments a frame shows in place: this row's, made under this comparison kind.
pub fn in_place<'a>(comments: &'a [Comment], diff: &LoadedDiff, branch: bool) -> Vec<&'a Comment> {
    comments
        .iter()
        .filter(|c| c.anchor.key == diff.key)
        .filter(|c| matches!(c.anchor.comparison, AnchorComparison::Branch { .. }) == branch)
        .collect()
}

/// Pending or unconfirmed comments under the current comparison kind whose row is no longer listed.
pub fn orphans<'a>(comments: &'a [Comment], snapshot: &Snapshot) -> Vec<&'a Comment> {
    let branch = snapshot.scope == Scope::Branch;
    let listed: std::collections::HashSet<FileKey> = snapshot.files.iter().map(FileKey::of).collect();
    comments
        .iter()
        .filter(|c| matches!(c.state, CommentState::Pending | CommentState::Unconfirmed { .. }))
        .filter(|c| matches!(c.anchor.comparison, AnchorComparison::Branch { .. }) == branch)
        .filter(|c| !listed.contains(&c.anchor.key))
        .collect()
}

/// The target an anchor names: its line on its side, a range's last line exactly (a range whose
/// last line is gone is drawn nowhere, as 10.3 says of a line that is gone; it never slides),
/// none for a file.
pub fn anchor_target(diff: &LoadedDiff, anchor: &Anchor) -> Option<usize> {
    match anchor.span {
        AnchorSpan::File => None,
        AnchorSpan::Line => nav::find(&diff.targets, anchor.side, anchor.line),
        AnchorSpan::Range { end } => nav::find(&diff.targets, anchor.side, end),
    }
}

/// The row a card sits after: the line's row, the range's last line's row (either side), the header.
pub fn attach_row(diff: &LoadedDiff, row_of_target: &[usize], anchor: &Anchor) -> Option<usize> {
    match anchor.span {
        AnchorSpan::File => Some(0),
        _ => anchor_target(diff, anchor).and_then(|t| row_of_target.get(t).copied()),
    }
}
```

`build` becomes `build(diff, mode, comments: &[&Comment], orphans: &[&Comment], card_width, editor: Option<&EditorPlace>)`, `card_width` being `cards::card_width(body_width, mode)` computed by the caller and `editor` the open editor's place and height: it builds the rows and `row_of_target` as today into `plain`, then splices the cards, and last the editor's `Row::Editor { line }` rows after the row its `EditorAnchor` names:

```rust
    let card_width = card_width.max(MIN_WIDTH);
    let mut after: Vec<Vec<Row>> = vec![Vec::new(); plain.len()];
    for comment in comments {
        if let Some(row) = attach_row(diff, &row_of_target, &comment.anchor) {
            // The anchor's own target, not the row's first: in split mode a row shows both sides.
            let target = anchor_target(diff, &comment.anchor);
            after[row].extend(cards::lines(comment, card_width, false).into_iter().map(|line| Row::Card { id: comment.id.clone(), target, line }));
        }
    }
    let mut rows = Vec::with_capacity(plain.len());
    let mut remap = vec![0usize; plain.len()];
    for (i, row) in plain.into_iter().enumerate() {
        remap[i] = rows.len();
        rows.push(row);
        rows.append(&mut after[i]);
    }
    for row in &mut row_of_target {
        *row = remap[*row];
    }
    let mut orphan_tops = Vec::new();
    if !orphans.is_empty() {
        rows.push(Row::Orphans { count: orphans.len() });
        for comment in orphans {
            orphan_tops.push((rows.len(), comment.id.clone()));
            rows.extend(cards::lines(comment, card_width, true).into_iter().map(|line| Row::Card { id: comment.id.clone(), target: None, line }));
        }
    }
```

where `anchor_target(diff, anchor)` is the target of the anchor's own line on its own side (`nav::find(&diff.targets, anchor.side, anchor.line)`, the range's last line for a `Range`, `None` for a `File`), so a click on a card lands on the line the comment is about, on its side, and never on the other side's cell of the same split row. `orphans_only(orphans, width, editor)` is the same tail on an empty `rows` with no `row_of_target`, the editor (if any) spliced after the last row. A comment whose anchor line is not in the diff (`attach_row` is `None`) is not drawn on this screen and not an orphan either: its row is listed, its line is not; it is counted and sent as 10.3 says, and `✎` still marks the file.

`view::body_line`'s match is exhaustive, so this step also adds its two arms, or the crate does not compile: `Row::Orphans { count }` draws `Span::label(format!("✎ on changes no longer shown ({count})"))`, and `Row::Card { line, .. }` draws, after the gutter's spaces, through `cards::frame_top` / `frame_text` / `frame_bottom` at `cards::card_width(columns, mode)`, the same number the rows were wrapped for, the title span styled `Style { semantic: tone, role: if dim { Role::Label } else { Role::Emphasis }, ..Style::role(Role::Body) }`. Step 7 adds the cursor highlight and the hits on top of this. A test in `view.rs`, `a_card_is_wrapped_and_framed_at_one_width`, renders a 200-character comment at 80 and 120 columns in both modes and asserts that the text rows' contents, joined, equal the comment's text: nothing hidden by a frame narrower than the wrap.

- [ ] **Step 5: Run the rows and cards tests**

Run: `cargo test --locked --lib tui::cards && cargo test --locked --lib tui::rows`
Expected: six pass (the three of each module). Every existing `rows::build(diff, mode)` call in tests and in `state.rs` gains `, &[], &[], 80, None`.

- [ ] **Step 6: The editor and the selection, `review.rs`**

```rust
//! The editor card and the visual selection of spec 10.3.
use crate::engine::comments::{self, Anchor, Category, Comment, Span as AnchorSpan};
use crate::engine::nav::Side;
use crate::engine::{Command, LoadedDiff};
use crate::tui::cards::CardLine;
use crate::tui::format::width;
use crate::tui::style::{Role, Semantic, Span, Style};

pub const NO_LINE: &str = "No diff line selected for comment.";
pub const NO_COMMENT: &str = "No comment selected.";
pub const CHOOSE_PANE: &str = "choose the pane this review goes to";
pub const EDITOR_FOOTER: &str = "enter save · ctrl+j newline · esc cancel";

pub struct Editor {
    pub anchor: Anchor,
    pub category: Category,
    pub text: String,
    /// The record `u`/`U` opened, kept as seen for the edit's transaction.
    pub editing: Option<Comment>,
    pub at_limit: bool,
    /// The token a save was sent under (`ViewState.comment_token`, one per save). Keys wait and the draft
    /// stays until the answer carrying it: accepted closes the editor, a refusal (the cap, a record that
    /// changed) hands the text back. Another save's answer, or a notice from the refresh, is not this one's.
    pub pending: Option<u64>,
}

impl Editor {
    pub fn new(anchor: Anchor) -> Self {
        Self { anchor, category: Category::Change, text: String::new(), editing: None, at_limit: false, pending: None }
    }

    pub fn edit(comment: &Comment) -> Self {
        Self {
            anchor: comment.anchor.clone(),
            category: comment.category,
            text: comment.text.clone(),
            editing: Some(comment.clone()),
            at_limit: false,
            pending: None,
        }
    }

    fn try_push(&mut self, ch: char) {
        let mut candidate = self.text.clone();
        candidate.push(ch);
        if !comments::within_caps(&candidate) {
            self.at_limit = true;
            return;
        }
        self.text = candidate;
        self.at_limit = false;
    }

    pub fn insert(&mut self, ch: char) {
        if !ch.is_control() {
            self.try_push(ch);
        }
    }

    pub fn newline(&mut self) {
        self.try_push('\n');
    }

    pub fn backspace(&mut self) {
        self.text.pop();
        self.at_limit = false;
    }

    pub fn place_label(&self) -> String {
        match self.anchor.span {
            AnchorSpan::File => "file".to_string(),
            AnchorSpan::Line => format!("{}{}", side_letter(self.anchor.side), self.anchor.line),
            AnchorSpan::Range { end } => format!("{}{}-{end}", side_letter(self.anchor.side), self.anchor.line),
        }
    }

    /// `comment on R16 · Question Change Bug Suggestion` with the chosen word in reverse video, or the limit.
    pub fn title(&self) -> Vec<Span> {
        if self.at_limit {
            return vec![Span::new(comments::NOTICE_LIMIT, Style::semantic(Role::Emphasis, Semantic::Warn))];
        }
        let mut spans = vec![Span::body(format!("comment on {} · ", self.place_label()))];
        for (i, category) in Category::ALL.iter().enumerate() {
            if i > 0 {
                spans.push(Span::body(" "));
            }
            let mut style = Style::role(Role::Body);
            style.reverse = *category == self.category;
            spans.push(Span::new(category.short(), style));
        }
        spans
    }

    pub fn lines(&self, width_cells: usize) -> Vec<CardLine> {
        let inner = width_cells.max(crate::tui::cards::MIN_WIDTH) - 4;
        let mut lines = vec![CardLine::Top { title: String::new(), tone: None, dim: false }];
        let mut text = crate::tui::cards::wrap(&self.text, inner);
        // The caret sits after the last character; a full last line pushes it to a new one.
        match text.last_mut() {
            Some(last) if width(last) < inner => last.push('_'),
            _ => text.push("_".to_string()),
        }
        lines.extend(text.into_iter().map(CardLine::Text));
        lines.push(CardLine::Bottom);
        lines
    }

    /// `Enter`: a whitespace-only text is inert. `token` is the number the answer will carry back.
    pub fn submit(&mut self, token: u64) -> Option<Command> {
        if self.text.trim().is_empty() {
            return None;
        }
        self.pending = Some(token);
        Some(match &self.editing {
            // The record as the editor opened it travels with the edit (spec 10.3's conflict rule).
            Some(seen) => Command::EditComment { token, seen: seen.clone(), category: self.category, text: self.text.clone() },
            None => Command::AddComment { token, anchor: self.anchor.clone(), category: self.category, text: self.text.clone() },
        })
    }
}

fn side_letter(side: Side) -> char {
    match side {
        Side::Additions => 'R',
        Side::Deletions => 'L',
    }
}

/// The target the selection began on; the cursor is its other end. The selection belongs to one
/// loaded diff: `reconcile` drops it when another `Arc` is on screen, so an index never names a line
/// of another file or comparison.
pub struct Visual {
    pub start: usize,
    pub diff: std::sync::Arc<LoadedDiff>,
}

/// `(side, first line, last line)` of the selection on the start's side; `None` when the cursor
/// crossed to the other side (a selection is one side's lines).
pub fn selection(diff: &LoadedDiff, visual: &Visual, cursor: usize) -> Option<(Side, u32, u32)> {
    let start = diff.targets.get(visual.start)?;
    let end = diff.targets.get(cursor)?;
    if start.side != end.side {
        return None;
    }
    let (a, b) = (start.line_number.min(end.line_number), start.line_number.max(end.line_number));
    Some((start.side, a, b))
}

/// The selected lines' text, for `y`: the side's content, line by line.
pub fn selection_text(diff: &LoadedDiff, side: Side, start: u32, end: u32) -> String {
    use crate::git::DiffLineType;
    let mut lines = Vec::new();
    for hunk in &diff.file_diff.hunks {
        for line in &hunk.lines {
            // By reference: the frozen `DiffLineType` is `Clone`, not `Copy`, and `line` is borrowed.
            let (number, mine) = match (side, &line.line_type) {
                (Side::Additions, DiffLineType::Removed) | (Side::Deletions, DiffLineType::Added) => continue,
                (Side::Additions, _) => (line.new_line_number, true),
                (Side::Deletions, _) => (line.old_line_number, true),
            };
            if let (Some(n), true) = (number, mine) {
                if n >= start && n <= end {
                    lines.push(line.content.clone());
                }
            }
        }
    }
    lines.join("\n")
}
```

Tests in `review.rs`: `the_editor_title_names_the_place_and_the_chosen_category` (R16 / L3-9 / file; the reverse span moves with `ctrl+l`), `the_editor_stops_at_the_caps_and_says_so` (4,000 chars accepted, the 4,001st refused with `at_limit`, a backspace clears it; 100 lines then `newline` refused), `submit_is_inert_on_whitespace_and_edits_keep_the_id`, `a_selection_spans_one_side_and_yields_its_lines`.

- [ ] **Step 7: Keys, state, input, view**

1. `keys.rs`, after `PickPane`:

```rust
    Binding { key: "i", label: "comment on line / selection", action: KeyAction::Comment, vimeflow: Some("diff-comment") },
    Binding { key: "I", label: "comment on file", action: KeyAction::CommentFile, vimeflow: Some("diff-comment-file") },
    Binding { key: "v", label: "select lines", action: KeyAction::Visual, vimeflow: Some("diff-visual") },
    Binding { key: "u", label: "edit comment", action: KeyAction::EditComment, vimeflow: Some("diff-comment-edit") },
    Binding { key: "U", label: "edit file comment", action: KeyAction::EditFileComment, vimeflow: None },
    Binding { key: "x", label: "delete comment", action: KeyAction::DeleteComment, vimeflow: Some("diff-comment-delete") },
    Binding { key: "X", label: "delete file comment", action: KeyAction::DeleteFileComment, vimeflow: None },
```

`RESERVED` shrinks to `["y", "Y", "@", "c", "/"]` (Task 8 finishes it); `KEYS.len()` becomes 35, and `help_panel(false).rows.len()` 35 in the same `keys.rs` test.

2. `state.rs`: the new fields (`editor`, `visual`, `orphan`, `seen_comment_seq`, and `comment_token: u64`, the counter whose next value numbers a save); `reconcile` rebuilds when any of the six parts of `built_from` changed (the diff's `Arc`, the mode, the comments' `Arc`, the files hash, the width, the editor's place and line count), where the files hash is `(snapshot.scope, snapshot.files.iter().map(FileKey::of))` hashed with `DefaultHasher`, and the width is the body width (`self.width` minus the panel); `rows::build(diff, self.mode, &rows::in_place(&snapshot.comments, diff, branch), &rows::orphans(&snapshot.comments, snapshot), cards::card_width(body_width, self.mode), self.editor_place().as_ref())`, with `editor_place()` deriving `EditorPlace` from `self.editor` (its anchor, or the orphan id it edits, and `editor.lines(card_width).len()`). `reconcile` also drops a visual selection whose diff is no longer the one on screen (below). When the diff is not `Ready` but orphans exist, or the editor is open, `reconcile` builds `rows::orphans_only(&orphans, cards::card_width(body_width, self.mode), self.editor_place().as_ref())` (the same card width `build` receives, so a long orphan wraps at the width it is drawn at and `frame_text` truncates nothing) instead of clearing `self.rows`, with `cursor = None` (the editor is spliced at `EditorAnchor::End`: its anchor is in no loaded diff, and a draft must never be off screen while its keys are modal, as when the last changed file turns clean under an open editor), and clears it only when there are no orphans and no editor; the five-part `built_from` key is kept with a `None` diff in that case. `a_draft_stays_on_screen_when_the_diff_goes_away` (Step 7's tests) pins it. `observe` gains:

```rust
        if snapshot.comment_seq != self.seen_comment_seq {
            self.seen_comment_seq = snapshot.comment_seq;
            if let Some(error) = &snapshot.comment_error {
                self.displace_urgent();
                self.warn(crate::tui::sanitize::sanitize(error));
            }
            // The editor's save answered, by token: accepted (journaled included) closes it; a refusal hands
            // the draft back, with the notice above saying why, for another try or Esc. An answer to an
            // earlier save, or a refresh's notice (no token), leaves a waiting editor waiting.
            if self.editor.as_ref().is_some_and(|e| e.pending.is_some() && e.pending == snapshot.comment_token) {
                if snapshot.comment_refused {
                    if let Some(editor) = self.editor.as_mut() {
                        editor.pending = None;
                    }
                } else {
                    self.editor = None;
                }
            }
        }
        // The picker opened by a comment key returns to the editor once a target is set.
        if let Some(picker) = &self.panes {
            if picker.done {
                if let crate::tui::panes::ReturnTo::Editor(anchor) = &picker.return_to {
                    if snapshot.target.is_some() {
                        self.editor = Some(crate::tui::review::Editor::new(anchor.clone()));
                    }
                }
            }
        }
```

(placed before the `self.panes = None` of Task 6's block, so the return destination is read first.) The orphan cursor: `self.orphan = None` whenever the rows are rebuilt without an orphan section or with fewer orphans than the index.

3. `input.rs`: routing order in `handle_key`: help, confirm, editor (`editor_key(state, snapshot, key)`), pane picker, base picker, then actions. `editor_key`:

```rust
fn editor_key(state: &mut ViewState, _snapshot: &Snapshot, key: KeyEvent) -> Outcome {
    let Some(editor) = state.editor.as_mut() else { return Outcome::Inert };
    // A save on its way: the draft is kept until the answer (`observe`), and Esc alone still discards it.
    if editor.pending.is_some() && key.code != KeyCode::Esc {
        return Outcome::Inert;
    }
    match (key.code, key.modifiers) {
        (KeyCode::Esc, KeyModifiers::NONE) => {
            state.editor = None;
            Outcome::Redraw
        }
        (KeyCode::Enter, KeyModifiers::NONE) => {
            // One number per save (`editor` borrows only `state.editor`, so the counter is free to touch).
            state.comment_token += 1;
            match editor.submit(state.comment_token) {
                Some(command) => Outcome::Engine(command),
                None => Outcome::Inert,
            }
        }
        (KeyCode::Char('j'), KeyModifiers::CONTROL) => { editor.newline(); Outcome::Redraw }
        (KeyCode::Char('h'), KeyModifiers::CONTROL) => { editor.category = editor.category.previous(); Outcome::Redraw }
        (KeyCode::Char('l'), KeyModifiers::CONTROL) => { editor.category = editor.category.next(); Outcome::Redraw }
        (KeyCode::Backspace, KeyModifiers::NONE) => { editor.backspace(); Outcome::Redraw }
        (KeyCode::Char(ch), KeyModifiers::NONE | KeyModifiers::SHIFT) => { editor.insert(ch); Outcome::Redraw }
        _ => Outcome::Inert,
    }
}
```

The comment keys in `act`:

```rust
        Comment | CommentFile | Visual | EditComment | EditFileComment | DeleteComment | DeleteFileComment => {
            return comment_key(state, snapshot, action);
        }
```

```rust
fn comment_key(state: &mut ViewState, snapshot: &Snapshot, action: KeyAction) -> Outcome {
    use KeyAction::*;
    // An orphan card under the cursor: u and x act on it, the rest is inert there.
    if let Some(index) = state.orphan {
        let id = state.rows.as_ref().and_then(|r| r.orphan_tops.get(index)).map(|(_, id)| id.clone());
        let comment = id.and_then(|id| snapshot.comments.iter().find(|c| c.id == id).cloned());
        return match (action, comment) {
            (EditComment, Some(c)) => { state.editor = Some(review::Editor::edit(&c)); Outcome::Redraw }
            (DeleteComment, Some(c)) => Outcome::Engine(Command::DeleteComment { seen: c }),
            _ => Outcome::Inert,
        };
    }
    let DiffState::Ready(diff) = &snapshot.diff else {
        state.notify(review::NO_LINE);
        return Outcome::Redraw;
    };
    let branch = snapshot.scope == crate::engine::Scope::Branch;
    let comparison = match (&diff.comparison, &snapshot.base) {
        (Comparison::Branch { merge_base }, Some(base)) => AnchorComparison::Branch { merge_base: merge_base.clone(), label: base.label().to_string() },
        _ => AnchorComparison::Worktree,
    };
    let cursor_target = state.cursor.and_then(|c| diff.targets.get(c).map(|t| (c, t.side, t.line_number)));
    let in_place = rows::in_place(&snapshot.comments, diff, branch);
    // `u` and `x` take the most recent editable card (pending or unconfirmed) whose anchor is the
    // cursor's target: the line on the cursor's side, so in split mode the other side's card is untouched.
    let on_cursor_row = || -> Option<Comment> {
        let cursor = state.cursor?;
        in_place
            .iter()
            .filter(|c| c.is_editable())
            .filter(|c| rows::anchor_target(diff, &c.anchor) == Some(cursor))
            // The claim's order (Task 5): the id breaks a same-second tie, so a replayed older comment never wins.
            .max_by_key(|c| (c.created_at, c.id.clone()))
            .map(|c| (*c).clone())
    };
    let file_comment = || -> Option<Comment> {
        in_place.iter().filter(|c| c.is_editable() && matches!(c.anchor.span, AnchorSpan::File)).max_by_key(|c| (c.created_at, c.id.clone())).map(|c| (*c).clone())
    };
    let at_cap = snapshot.comments.iter().filter(|c| c.is_unsent()).count() >= comments::CAP;
    match action {
        Visual => {
            let Some((cursor, ..)) = cursor_target else { state.notify(review::NO_LINE); return Outcome::Redraw };
            state.visual = Some(review::Visual { start: cursor, diff: diff.clone() });
            Outcome::Redraw
        }
        Comment | CommentFile => {
            if at_cap {
                // The 51st is refused before the editor opens (spec 10.3); the engine re-checks under the lock.
                state.notify(comments::NOTICE_CAP);
                return Outcome::Redraw;
            }
            let anchor = if action == CommentFile {
                Anchor { key: diff.key.clone(), side: Side::Additions, line: 0, span: AnchorSpan::File, comparison }
            } else {
                let Some((cursor, side, line)) = cursor_target else { state.notify(review::NO_LINE); return Outcome::Redraw };
                // A selection is one side's lines; the visual mode's movement keeps it so (`step_on_side`),
                // and `reconcile` has dropped any selection made on another diff.
                match state.visual.take().filter(|v| std::sync::Arc::ptr_eq(&v.diff, diff)).and_then(|v| review::selection(diff, &v, cursor)) {
                    Some((side, start, end)) if start != end => Anchor { key: diff.key.clone(), side, line: start, span: AnchorSpan::Range { end }, comparison },
                    _ => Anchor { key: diff.key.clone(), side, line, span: AnchorSpan::Line, comparison },
                }
            };
            if snapshot.target.is_none() {
                // The target comes first: the picker opens and keeps the anchor (spec 10.3).
                state.notify(review::CHOOSE_PANE);
                state.panes_token += 1;
                state.panes = Some(crate::tui::panes::PanePicker::open(state.panes_token, crate::tui::panes::ReturnTo::Editor(anchor)));
                return Outcome::Engine(Command::LoadPanes(state.panes_token));
            }
            state.editor = Some(review::Editor::new(anchor));
            Outcome::Redraw
        }
        EditComment | DeleteComment => {
            let Some(comment) = on_cursor_row() else {
                state.notify(if cursor_target.is_none() { review::NO_LINE } else { review::NO_COMMENT });
                return Outcome::Redraw;
            };
            if action == EditComment {
                state.editor = Some(review::Editor::edit(&comment));
                Outcome::Redraw
            } else {
                Outcome::Engine(Command::DeleteComment { seen: comment })
            }
        }
        EditFileComment | DeleteFileComment => {
            let Some(comment) = file_comment() else { state.notify(review::NO_COMMENT); return Outcome::Redraw };
            if action == EditFileComment {
                state.editor = Some(review::Editor::edit(&comment));
                Outcome::Redraw
            } else {
                Outcome::Engine(Command::DeleteComment { seen: comment })
            }
        }
        _ => Outcome::Inert,
    }
}
```

`Esc` in the body with a visual selection ends it (add to `handle_key` before the popup-Esc rule: `if state.visual.is_some() && Esc { state.visual = None; return Redraw }`). While a selection stands, `LineDown`/`LineUp` do not use `nav::move_line`, which crosses sides in unified mode: they call `review::step_on_side(diff, cursor, side, delta)`, the next or previous target on the start's side by line number, so `selection` always has one side to report; `h`/`l` in split mode collapse the selection to the other side's row: in `move_cursor`'s `SideDeletions | SideAdditions` arm, when `state.visual.is_some()`, set `visual.start` to the new cursor after moving. `step_on_side` is a small function in `review.rs`:

```rust
/// The next (`delta > 0`) or previous target on `side` by line number; the cursor when there is none.
pub fn step_on_side(diff: &LoadedDiff, cursor: usize, side: Side, delta: isize) -> usize {
    let Some(current) = diff.targets.get(cursor) else { return cursor };
    let mut candidates: Vec<(u32, usize)> = diff
        .targets
        .iter()
        .enumerate()
        .filter(|(_, t)| t.side == side)
        .map(|(i, t)| (t.line_number, i))
        .collect();
    candidates.sort();
    let at = candidates.iter().position(|(_, i)| *i == cursor).or_else(|| {
        candidates.iter().position(|(line, _)| *line >= current.line_number)
    });
    match at {
        Some(at) => candidates
            .get((at as isize + delta).clamp(0, candidates.len() as isize - 1) as usize)
            .map(|(_, i)| *i)
            .unwrap_or(cursor),
        None => cursor,
    }
}
```

The orphan cursor: in `move_cursor`'s `LineDown`, when the cursor is the last target of the unified order (or `targets.len() - 1` in split) and `rows.orphan_tops` is non-empty, set `state.orphan = Some(0)` and keep the cursor; with **no targets at all** (an empty list, or a diff that is not `Ready`, with orphans drawn by `orphans_only`), `LineDown` from nothing sets `orphan = Some(0)` too, which is the entry path into an orphan-only view; `LineDown` on orphan `n` moves to `n + 1` while one exists; `LineUp` on orphan 0 clears `orphan` (and leaves the cursor where it was, or nowhere); `LineUp` on `n` moves to `n - 1`; every other movement key clears `orphan`. `keep_cursor_visible` scrolls to `orphan_tops[n].0` when `orphan` is set. `move_cursor`'s early return on `(DiffState::Ready, Some(cursor))` must therefore give way to the orphan rules before it returns `Inert`.

4. `view.rs`: `body_line`'s two card arms are Step 4's; here they gain the cursor: an orphan card under the orphan cursor is drawn in reverse video as a cursor row is. A visual selection is drawn: every row whose target lies on the selection's side between its first and last line takes the cursor's reverse video on its text span (`render` computes the selected target set from `review::selection` once per frame and passes `selected: bool` to `body_line` beside `cursor`), so what `i` or `y` will act on is visible; a view test, `a_visual_selection_is_drawn_in_reverse_over_its_lines`, selects two lines with `v`, `j` and asserts the reverse flag on exactly those rows' text spans and on no other. `Action` gains `EditorCategory(Category)` (`key_action()` → `None`), handled in `handle_mouse`'s click arm by setting the open editor's category. The editor is modal for the mouse as the y/n box is: while `state.editor.is_some()`, `handle_mouse` handles `EditorCategory` clicks and the wheel (scrolling the body) and nothing else, so a click on the toolbar, the files panel or a diff row cannot open a picker, a box or another action under an open editor; a test, `clicks_under_an_open_editor_are_inert_except_the_category_words`, presses a toolbar hit and a file hit while editing and asserts `Outcome::Inert`, then clicks a category word and asserts the category changed. When the diff is not `Ready` but `rows::orphans` is non-empty or the editor is open, `render` draws `state.rows` (built by `orphans_only`) instead of `state_message`'s centred text, so an empty list with comments left behind shows the orphan section, not `working tree clean`, and a draft is never hidden behind that message. The editor's rows are real rows, so one coordinate system serves scrolling, hits and drawing: `Row::Editor { line: usize }`, one per line of `editor.lines(card_width)`, spliced by `rows::build` (and `orphans_only`) after the row the editor sits under, which `build` receives as `editor: Option<EditorPlace>` with `EditorPlace { after: EditorAnchor, lines: usize }` and `enum EditorAnchor { Anchor(Anchor), Orphan(String) /* the card's id */, End }`: the anchor's row through `attach_row` for an anchor in the loaded diff — only when `anchor.key == diff.key` and the anchor's comparison kind is the diff's, the rule `in_place` applies to cards, so a draft never sits under another file's line of the same number when a refresh replaces the diff (`a_draft_keeps_its_file_when_another_diff_takes_the_screen`) — the last `Row::Card` row carrying the id for an orphan being edited (the editor replaces the card visually), the last body row when the anchor's line is not in the diff or the diff is another file's. The rows carry no text; `view::body_line` draws `Row::Editor { line }` from `state.editor.lines(card_width)[line]`, `frame_top(editor.title(), …)` for the first, `frame_bottom(if editor.pending.is_some() { "saving…" } else { EDITOR_FOOTER }, …)` for the last, so typing never rebuilds the rows: `reconcile`'s key gains `(editor anchor, line count)` and rebuilds only when the editor opens, closes, moves or grows a line, which is rare and cheap. The title line pushes one hit per category word, `Action::EditorCategory(Category)`, and a click sets `editor.category` (spec 10.3's clickable words); the text rows push no hit. `view::body_is_drawn` stays true while the editor is open (it is a card, not a modal).

With the editor's rows in `rows`, keeping it on screen is the existing machinery: `keep_cursor_visible` scrolls to the span from the editor's first row to its last (`ensure_visible` over `first..=last`), after `reconcile` and after every editor key; when the editor is taller than the body, the offset puts its last row (the caret's) on the bottom row, and the anchor above scrolls out as any row does. Tests in `state.rs`: `an_editor_opened_on_the_bottom_row_scrolls_into_view` (a 40-target diff in a 10-row body, the editor opened on the last target, its bottom row within the viewport), `an_editor_taller_than_the_body_keeps_its_caret_visible` (120 lines typed through `ctrl+j`, the last rendered row carries `_`), and `a_partially_visible_editor_whose_anchor_scrolled_away_still_draws` (the offset moved past the anchor row by `ctrl+d`: the editor's remaining rows are drawn at the top of the body, nothing is skipped). `files_lines`: the `✎` cell before the marker when any comment of the file exists under the current comparison kind (`snapshot.comments.iter().any(|c| c.anchor.key.path == file.path && kind matches)`), the name fitted to `FILES_WIDTH - 3`; the header `CHANGED {n} · ✎ {pending}` when `pending > 0`, counting `comments.iter().filter(|c| c.is_pending()).count()` across both scopes. Card rows push `Action::CursorToRow(row)` hits as other rows do; `CursorToRow` on a card row with `target: Some(t)` sets the cursor to `t`, the anchor's own target (in `handle_mouse`'s click arm: look the row up in `state.rows`), so the cursor lands on the comment's side.

- [ ] **Step 8: Input and view tests**

The existing `setup` fixture gives a diff of `a.rs` with no `files` row and no line numbers on its lines, which the comment keys, the panel marks and `selection_text` all need. Add one fixture for the review tests, beside `setup`, and use it in this task's and Task 8's tests:

```rust
    /// `setup` plus what the review loop reads: the diff's row in `files`, numbered lines, a clipboard target.
    fn review_setup(hunks: &[(u32, &str)]) -> (crate::engine::Snapshot, ViewState) {
        let (mut snap, mut st) = setup(hunks);
        snap.files = vec![crate::git::ChangedFile {
            path: "a.rs".into(),
            status: crate::git::ChangedFileStatus::Modified,
            staged: false,
            insertions: None,
            deletions: None,
        }];
        if let DiffState::Ready(diff) = &mut snap.diff {
            let mut numbered = (**diff).clone();
            for hunk in &mut numbered.file_diff.hunks {
                let (mut old, mut new) = (hunk.old_start, hunk.new_start);
                for line in &mut hunk.lines {
                    match line.line_type {
                        crate::git::DiffLineType::Added => { line.new_line_number = Some(new); new += 1; }
                        crate::git::DiffLineType::Removed => { line.old_line_number = Some(old); old += 1; }
                        _ => { line.old_line_number = Some(old); line.new_line_number = Some(new); old += 1; new += 1; }
                    }
                }
            }
            snap.diff = DiffState::Ready(std::sync::Arc::new(numbered));
        }
        snap.target = Some(crate::engine::Target::Clipboard);
        snap.target_state = crate::engine::TargetState::Clipboard;
        st.observe(&snap);
        st.reconcile(&snap);
        (snap, st)
    }

    fn comment_at(anchor: &Anchor, text: &str, created_at: u64) -> comments::Comment {
        comments::Comment { id: format!("c{created_at}"), anchor: anchor.clone(), category: comments::Category::Bug, text: text.into(), created_at, state: comments::CommentState::Pending }
    }

    fn anchor_on(snap: &crate::engine::Snapshot, line: u32) -> Anchor {
        let DiffState::Ready(diff) = &snap.diff else { panic!("ready") };
        Anchor { key: diff.key.clone(), side: Side::Additions, line, span: comments::Span::Line, comparison: comments::AnchorComparison::Worktree }
    }
```

The tests that follow start from `review_setup` where they need a listed file, line numbers or a target, and from `setup` where they test the no-target path; the file is `a.rs` throughout. In `input.rs`'s tests:

```rust
    #[test]
    fn i_without_a_target_opens_the_picker_and_keeps_the_anchor() {
        let (mut snap, mut st) = review_setup(&[(10, " --+ ")]);
        snap.target = None;
        snap.target_state = crate::engine::TargetState::Unverified;
        assert_eq!(handle_key(&mut st, &snap, key("i"), 120), Outcome::Engine(Command::LoadPanes(1)));
        assert!(st.editor.is_none());
        assert!(matches!(st.panes.as_ref().map(|p| &p.return_to), Some(crate::tui::panes::ReturnTo::Editor(_))));
        assert_eq!(st.notice.as_ref().map(|n| n.text.as_str()), Some(review::CHOOSE_PANE));
        // Esc drops the anchor with the picker.
        handle_key(&mut st, &snap, key("Esc"), 120);
        assert!(st.panes.is_none() && st.editor.is_none());
        // A pick reopens the editor on it.
        handle_key(&mut st, &snap, key("i"), 120);
        snap.panes = Some(std::sync::Arc::new(Vec::new()));
        snap.panes_seq = 2;
        st.observe(&snap);
        assert!(matches!(handle_key(&mut st, &snap, key("Enter"), 120), Outcome::Engine(Command::SetTarget(crate::engine::Target::Clipboard))));
        snap.target = Some(crate::engine::Target::Clipboard);
        snap.target_state = crate::engine::TargetState::Clipboard;
        snap.target_seq = 1;
        st.observe(&snap);
        assert!(st.panes.is_none());
        let editor = st.editor.as_ref().expect("the editor opened on the kept anchor");
        assert_eq!(editor.place_label(), "L11");   // the fixture's first changed row is the deletion of old line 11
    }

    #[test]
    fn the_editor_saves_edits_and_deletes_the_most_recent_card_of_the_line() {
        let (mut snap, mut st) = review_setup(&[(10, " --+ ")]);
        handle_key(&mut st, &snap, key("i"), 120);
        for ch in "needs a test".chars() {
            handle_key(&mut st, &snap, key(&ch.to_string()), 120);
        }
        handle_key(&mut st, &snap, key("ctrl+l"), 120);
        let outcome = handle_key(&mut st, &snap, key("Enter"), 120);
        let Outcome::Engine(Command::AddComment { token, anchor, category, text }) = outcome else { panic!("{outcome:?}") };
        assert_eq!((anchor.side, anchor.line, category, text.as_str()), (Side::Deletions, 11, comments::Category::Bug, "needs a test"));
        // The draft waits for the answer carrying its token; an accepted save closes the editor.
        assert_eq!(st.editor.as_ref().and_then(|e| e.pending), Some(token));
        assert_eq!(handle_key(&mut st, &snap, key("x"), 120), Outcome::Inert, "keys wait for the answer");
        snap.comment_seq += 1;
        snap.comment_token = None;   // a refresh's notice: not this save's answer
        st.observe(&snap);
        assert!(st.editor.is_some(), "another answer leaves the editor waiting");
        snap.comment_seq += 1;
        snap.comment_token = Some(token);
        st.observe(&snap);
        assert!(st.editor.is_none());
        // Two cards on the line: u and x take the most recent.
        let older = comment_at(&anchor, "older", 1);
        let newer = comment_at(&anchor, "newer", 2);
        snap.comments = std::sync::Arc::new(vec![older, newer.clone()]);
        st.observe(&snap);
        st.reconcile(&snap);
        handle_key(&mut st, &snap, key("u"), 120);
        assert_eq!(st.editor.as_ref().map(|e| e.text.as_str()), Some("newer"));
        for _ in 0..5 { handle_key(&mut st, &snap, key("Backspace"), 120); }
        for ch in "edited".chars() { handle_key(&mut st, &snap, key(&ch.to_string()), 120); }
        let outcome = handle_key(&mut st, &snap, key("Enter"), 120);
        let Outcome::Engine(Command::EditComment { token, seen, text, .. }) = outcome else { panic!("{outcome:?}") };
        assert!(seen == newer && text == "edited");
        snap.comment_seq += 1;
        snap.comment_token = Some(token);
        st.observe(&snap);
        assert!(st.editor.is_none());
        assert!(matches!(handle_key(&mut st, &snap, key("x"), 120), Outcome::Engine(Command::DeleteComment { seen }) if seen.id == newer.id));
        // Two cards made in the same second, the older one appended later by a journal replay: the id
        // (Task 3's `new_id` increases with time) breaks the tie, so `x` deletes the newer, not the first found.
        let mut first = comment_at(&anchor, "made first", 7);
        first.id = "00000000000000010000000000000000".into();
        let mut second = comment_at(&anchor, "made second", 7);
        second.id = "00000000000000020000000000000000".into();
        snap.comments = std::sync::Arc::new(vec![second.clone(), first.clone()]);
        st.observe(&snap);
        st.reconcile(&snap);
        assert!(matches!(handle_key(&mut st, &snap, key("x"), 120), Outcome::Engine(Command::DeleteComment { seen }) if seen.id == second.id), "the same-second tie goes to the later id");
        handle_key(&mut st, &snap, key("u"), 120);
        assert_eq!(st.editor.as_ref().map(|e| e.text.as_str()), Some("made second"));
        handle_key(&mut st, &snap, key("Esc"), 120);
        // Whitespace is inert; Esc discards; the limit title.
        handle_key(&mut st, &snap, key("i"), 120);
        handle_key(&mut st, &snap, key(" "), 120);
        assert_eq!(handle_key(&mut st, &snap, key("Enter"), 120), Outcome::Inert);
        assert_eq!(handle_key(&mut st, &snap, key("Esc"), 120), Outcome::Redraw);
        assert!(st.editor.is_none());
    }

    #[test]
    fn a_refused_save_hands_the_draft_back_and_a_journaled_one_counts_as_saved() {
        let (mut snap, mut st) = review_setup(&[(10, " --+ ")]);
        handle_key(&mut st, &snap, key("i"), 120);
        for ch in "kept".chars() {
            handle_key(&mut st, &snap, key(&ch.to_string()), 120);
        }
        let Outcome::Engine(Command::AddComment { token, .. }) = handle_key(&mut st, &snap, key("Enter"), 120) else { panic!() };
        // Refused (another viewer reached the cap first): the text is still there, the notice says why.
        snap.comment_seq += 1;
        snap.comment_error = Some(comments::NOTICE_CAP.to_string());
        snap.comment_refused = true;
        snap.comment_token = Some(token);
        st.observe(&snap);
        let editor = st.editor.as_ref().expect("the draft survives a refusal");
        assert_eq!((editor.text.as_str(), editor.pending), ("kept", None));
        assert_eq!(st.notice.as_ref().map(|n| n.text.as_str()), Some(comments::NOTICE_CAP));
        // Typing resumes; Esc still discards.
        assert_eq!(handle_key(&mut st, &snap, key("!"), 120), Outcome::Redraw);
        assert_eq!(st.editor.as_ref().unwrap().text, "kept!");
        // Journaled (the store could not be written) is an accepted save: the comment is on screen.
        let Outcome::Engine(Command::AddComment { token, .. }) = handle_key(&mut st, &snap, key("Enter"), 120) else { panic!() };
        snap.comment_seq += 1;
        snap.comment_error = Some(format!("{}disk full", comments::NOTICE_NOT_REMEMBERED));
        snap.comment_refused = false;
        snap.comment_token = Some(token);
        st.observe(&snap);
        assert!(st.editor.is_none(), "a journaled save closes the editor");
        // Two saves in flight: Esc on the first, a second editor opened and saved; the first answer
        // (a success) is not the second's, which then fails and keeps its draft.
        handle_key(&mut st, &snap, key("i"), 120);
        handle_key(&mut st, &snap, key("a"), 120);
        let Outcome::Engine(Command::AddComment { token: first, .. }) = handle_key(&mut st, &snap, key("Enter"), 120) else { panic!() };
        assert_eq!(handle_key(&mut st, &snap, key("Esc"), 120), Outcome::Redraw);
        handle_key(&mut st, &snap, key("i"), 120);
        handle_key(&mut st, &snap, key("b"), 120);
        let Outcome::Engine(Command::AddComment { token: second, .. }) = handle_key(&mut st, &snap, key("Enter"), 120) else { panic!() };
        assert!(second > first);
        snap.comment_seq += 1;
        snap.comment_error = None;
        snap.comment_refused = false;
        snap.comment_token = Some(first);
        st.observe(&snap);
        assert_eq!(st.editor.as_ref().map(|e| (e.text.as_str(), e.pending)), Some(("b", Some(second))), "the first save's answer is not the second's");
        snap.comment_seq += 1;
        snap.comment_error = Some(comments::NOTICE_CAP.to_string());
        snap.comment_refused = true;
        snap.comment_token = Some(second);
        st.observe(&snap);
        assert_eq!(st.editor.as_ref().map(|e| (e.text.as_str(), e.pending)), Some(("b", None)), "the second's refusal hands its draft back");
        handle_key(&mut st, &snap, key("Esc"), 120);
        // An edit whose record changed under it is refused the same way, with the record's words.
        let DiffState::Ready(diff) = &snap.diff else { panic!("ready") };
        // The cursor's row is the deletion of old line 11, as in the test above.
        let anchor = Anchor { key: diff.key.clone(), side: Side::Deletions, line: 11, span: comments::Span::Line, comparison: comments::AnchorComparison::Worktree };
        let card = comment_at(&anchor, "theirs", 1);
        snap.comments = std::sync::Arc::new(vec![card.clone()]);
        st.observe(&snap);
        st.reconcile(&snap);
        handle_key(&mut st, &snap, key("u"), 120);
        for ch in " mine".chars() {
            handle_key(&mut st, &snap, key(&ch.to_string()), 120);
        }
        let Outcome::Engine(Command::EditComment { token, .. }) = handle_key(&mut st, &snap, key("Enter"), 120) else { panic!() };
        snap.comment_seq += 1;
        snap.comment_error = Some("No comment selected.".to_string());
        snap.comment_refused = true;
        snap.comment_token = Some(token);
        st.observe(&snap);
        assert_eq!(st.editor.as_ref().map(|e| e.text.as_str()), Some("theirs mine"));
    }

    #[test]
    fn a_draft_stays_on_screen_when_the_diff_goes_away() {
        let (mut snap, mut st) = review_setup(&[(10, " + ")]);
        handle_key(&mut st, &snap, key("i"), 120);
        for ch in "half".chars() {
            handle_key(&mut st, &snap, key(&ch.to_string()), 120);
        }
        // The last changed file turns clean under the editor: the draft is drawn, the keys still reach it.
        snap.files.clear();
        snap.diff = DiffState::Idle;
        st.observe(&snap);
        st.reconcile(&snap);
        assert!(st.rows.as_ref().is_some_and(|r| r.rows.iter().any(|row| matches!(row, Row::Editor { .. }))), "the editor's rows survive the diff");
        let rendered = crate::tui::view::render(&snap, &st, 120, 24);
        assert!(rendered.plain().iter().any(|line| line.contains("half_")), "the draft is drawn, caret included");
        assert_eq!(handle_key(&mut st, &snap, key("!"), 120), Outcome::Redraw);
        assert!(matches!(handle_key(&mut st, &snap, key("Enter"), 120), Outcome::Engine(Command::AddComment { text, .. }) if text == "half!"));
    }

    #[test]
    fn a_draft_keeps_its_file_when_another_diff_takes_the_screen() {
        let (mut snap, mut st) = review_setup(&[(10, " + ")]);
        handle_key(&mut st, &snap, key("i"), 120);
        handle_key(&mut st, &snap, key("k"), 120);
        // A refresh replaces a.rs with b.rs, which has the same changed line: the draft is drawn at the
        // end, not under b.rs's line, and saves against a.rs.
        let other = crate::tui::state::tests::snapshot("b.rs", "r2", &[(10, " + ")]);
        snap.diff = other.diff.clone();
        snap.files = other.files.clone();
        snap.selected = other.selected.clone();
        st.observe(&snap);
        st.reconcile(&snap);
        let rows = st.rows.as_ref().unwrap();
        let editor_rows: Vec<usize> = rows.rows.iter().enumerate().filter(|(_, r)| matches!(r, Row::Editor { .. })).map(|(i, _)| i).collect();
        assert!(!editor_rows.is_empty());
        assert_eq!(*editor_rows.last().unwrap(), rows.rows.len() - 1, "the draft sits at the end, under no line of b.rs");
        let outcome = handle_key(&mut st, &snap, key("Enter"), 120);
        assert!(matches!(outcome, Outcome::Engine(Command::AddComment { anchor, .. }) if anchor.key.path == "a.rs"), "{outcome:?}");
    }

    #[test]
    fn in_split_mode_u_and_x_take_the_card_of_the_cursors_side_only() {
        let (mut snap, mut st) = review_setup(&[(10, " -+ ")]);
        st.requested_mode = ViewMode::Split;
        st.resize(120, 20);
        let DiffState::Ready(diff) = &snap.diff else { panic!() };
        let left = comment_at(&Anchor { key: diff.key.clone(), side: Side::Deletions, line: 11, span: comments::Span::Line, comparison: comments::AnchorComparison::Worktree }, "left", 1);
        let right = comment_at(&Anchor { key: diff.key.clone(), side: Side::Additions, line: 11, span: comments::Span::Line, comparison: comments::AnchorComparison::Worktree }, "right", 2);
        snap.comments = std::sync::Arc::new(vec![left.clone(), right.clone()]);
        st.observe(&snap);
        st.reconcile(&snap);
        // The two cards share one split row; the cursor is on the deletion.
        handle_key(&mut st, &snap, key("h"), 120);
        assert!(matches!(handle_key(&mut st, &snap, key("x"), 120), Outcome::Engine(Command::DeleteComment { seen }) if seen.id == left.id));
        handle_key(&mut st, &snap, key("l"), 120);
        assert!(matches!(handle_key(&mut st, &snap, key("x"), 120), Outcome::Engine(Command::DeleteComment { seen }) if seen.id == right.id));
        // A click on the right card lands on the addition, not the deletion in the same row.
        let rendered = crate::tui::view::render(&snap, &st, 120, 24);
        let rows = st.rows.as_ref().unwrap();
        let (row, target) = rows.rows.iter().enumerate().find_map(|(i, r)| match r { Row::Card { id, target: Some(t), .. } if *id == right.id => Some((i, *t)), _ => None }).unwrap();
        assert_eq!(diff.targets[target].side, Side::Additions);
        let y = (row - st.offset) as u16 + 1;
        let hit = rendered.hit(60, y).expect("the card row is a hit");
        assert!(matches!(hit, crate::tui::view::Action::CursorToRow(r) if *r == row));
    }

    #[test]
    fn file_comments_use_capital_keys_and_the_notices_say_what_is_missing() {
        let (mut snap, mut st) = review_setup(&[(10, " --+ ")]);
        handle_key(&mut st, &snap, key("I"), 120);
        assert_eq!(st.editor.as_ref().map(|e| e.place_label()), Some("file".into()));
        handle_key(&mut st, &snap, key("Esc"), 120);
        handle_key(&mut st, &snap, key("U"), 120);
        assert_eq!(st.notice.as_ref().map(|n| n.text.as_str()), Some(review::NO_COMMENT));
        handle_key(&mut st, &snap, key("x"), 120);
        assert_eq!(st.notice.as_ref().map(|n| n.text.as_str()), Some(review::NO_COMMENT));
        snap.diff = DiffState::Loading;
        handle_key(&mut st, &snap, key("i"), 120);
        assert_eq!(st.notice.as_ref().map(|n| n.text.as_str()), Some(review::NO_LINE));
    }

    #[test]
    fn v_selects_a_range_on_one_side_and_i_comments_on_it() {
        let (snap, mut st) = review_setup(&[(10, " --++ ")]);
        st.requested_mode = ViewMode::Split;
        st.resize(120, 20);
        st.reconcile(&snap);
        handle_key(&mut st, &snap, key("v"), 120);
        handle_key(&mut st, &snap, key("j"), 120);
        handle_key(&mut st, &snap, key("i"), 120);
        let editor = st.editor.as_ref().unwrap();
        assert!(matches!(editor.anchor.span, comments::Span::Range { .. }), "{:?}", editor.anchor);
        assert!(st.visual.is_none(), "i ends the selection");
        handle_key(&mut st, &snap, key("Esc"), 120);
        handle_key(&mut st, &snap, key("v"), 120);
        assert_eq!(handle_key(&mut st, &snap, key("Esc"), 120), Outcome::Redraw);
        assert!(st.visual.is_none());
        assert_eq!(handle_key(&mut st, &snap, key("y"), 120), Outcome::Inert, "y is inert without a selection");
        // A selection does not survive another diff on screen.
        handle_key(&mut st, &snap, key("v"), 120);
        let other = crate::tui::state::tests::snapshot("b.rs", "r2", &[(5, " + ")]);
        st.observe(&other);
        st.reconcile(&other);
        assert!(st.visual.is_none(), "a selection made on a.rs cannot name lines of b.rs");
    }

    #[test]
    fn j_past_the_last_line_lands_on_an_orphan_card_where_u_and_x_act() {
        let (mut snap, mut st) = review_setup(&[(10, " + ")]);
        let gone = comment_at(&Anchor { key: FileKey { path: "gone".into(), staged: false, untracked: false }, side: Side::Additions, line: 1, span: comments::Span::Line, comparison: comments::AnchorComparison::Worktree }, "lost", 1);
        snap.comments = std::sync::Arc::new(vec![gone.clone()]);
        st.observe(&snap);
        st.reconcile(&snap);
        let last = st.cursor.unwrap();
        handle_key(&mut st, &snap, key("G"), 120);
        handle_key(&mut st, &snap, key("j"), 120);
        assert_eq!(st.orphan, Some(0));
        assert_eq!(handle_key(&mut st, &snap, key("i"), 120), Outcome::Inert);
        handle_key(&mut st, &snap, key("u"), 120);
        assert_eq!(st.editor.as_ref().map(|e| e.text.as_str()), Some("lost"));
        handle_key(&mut st, &snap, key("Esc"), 120);
        assert!(matches!(handle_key(&mut st, &snap, key("x"), 120), Outcome::Engine(Command::DeleteComment { seen }) if seen.id == gone.id));
        handle_key(&mut st, &snap, key("k"), 120);
        assert_eq!(st.orphan, None);
        let _ = last;
    }

    #[test]
    fn an_empty_list_with_orphans_is_reachable_and_editable() {
        let (mut snap, mut st) = review_setup(&[]);
        snap.files.clear();
        snap.diff = DiffState::Idle;
        let gone = comment_at(&Anchor { key: FileKey { path: "gone".into(), staged: false, untracked: false }, side: Side::Additions, line: 1, span: comments::Span::Line, comparison: comments::AnchorComparison::Worktree }, "left behind", 1);
        let mut unsure = gone.clone();
        unsure.id = "u".into();
        unsure.state = comments::CommentState::Unconfirmed {
            stamp: comments::Stamp { at: 1, nonce: "abc123".into(), item: 1, to: crate::engine::target::Destination::clipboard() },
            before: Vec::new(),
        };
        snap.comments = std::sync::Arc::new(vec![gone.clone(), unsure.clone()]);
        st.observe(&snap);
        st.reconcile(&snap);
        assert!(st.rows.as_ref().is_some_and(|r| r.orphan_tops.len() == 2), "the orphan section is built without a diff");
        assert_eq!(st.cursor, None);
        // A long orphan is wrapped at the width it is drawn at, in both modes: every wrapped piece is on
        // screen whole, and the pieces concatenate back to the text (`cards::wrap` is lossless).
        let long = "x".repeat(300);
        let mut wide = gone.clone();
        wide.id = "wide".into();
        wide.text = long.clone();
        snap.comments = std::sync::Arc::new(vec![wide]);
        for mode in [ViewMode::Unified, ViewMode::Split] {
            st.requested_mode = mode;
            st.resize(120, 20);
            st.observe(&snap);
            st.reconcile(&snap);
            let rendered = crate::tui::view::render(&snap, &st, 120, 24).plain();
            let pieces: Vec<String> = st.rows.as_ref().unwrap().rows.iter().filter_map(|r| match r { Row::Card { line: crate::tui::cards::CardLine::Text(t), .. } => Some(t.clone()), _ => None }).collect();
            assert_eq!(pieces.concat(), long, "{mode:?}");
            for piece in &pieces {
                assert!(rendered.iter().any(|l| l.contains(piece.as_str())), "{mode:?}: a wrapped piece was cut: {piece}");
            }
        }
        snap.comments = std::sync::Arc::new(vec![gone.clone(), unsure.clone()]);
        st.requested_mode = ViewMode::Unified;
        st.resize(120, 20);
        st.observe(&snap);
        st.reconcile(&snap);
        handle_key(&mut st, &snap, key("j"), 120);
        assert_eq!(st.orphan, Some(0), "j from nothing enters the orphan section");
        handle_key(&mut st, &snap, key("j"), 120);
        assert_eq!(st.orphan, Some(1));
        // The unconfirmed orphan is editable and deletable too.
        handle_key(&mut st, &snap, key("u"), 120);
        assert_eq!(st.editor.as_ref().and_then(|e| e.editing.as_ref()).map(|c| c.id.as_str()), Some("u"));
        handle_key(&mut st, &snap, key("Esc"), 120);
        assert!(matches!(handle_key(&mut st, &snap, key("x"), 120), Outcome::Engine(Command::DeleteComment { seen }) if seen.id == "u"));
        let rendered = crate::tui::view::render(&snap, &st, 120, 24);
        let plain = rendered.plain().join("\n");
        assert!(plain.contains("✎ on changes no longer shown (2)") && !plain.contains("working tree clean"), "{plain}");
    }
```

(`comment_at(anchor, text, created_at)` is a test constructor.) In `view.rs`'s tests: `cards_are_drawn_under_their_lines_with_their_titles` (a pending Bug and a sent Question: `╭─ Bug · pending`, `╭─ Question · sent` on the lines after the anchor rows, the text inside `│`), `the_panel_marks_files_with_comments_and_counts_pending` (`✎` in the cell before `S`, `CHANGED 2 · ✎ 1`; a sent comment keeps the `✎` and not the count), `the_editor_is_spliced_after_its_anchor_row` (the `comment on R12 ·` title line directly after row 12's line and `enter save · ctrl+j newline · esc cancel` two rows later; rows below shift by the editor's height), `an_orphan_section_closes_the_body` (`✎ on changes no longer shown (1)` then a card titled with the path). In `state.rs`'s tests: `reconcile_rebuilds_on_a_new_comments_arc_and_on_width_and_not_otherwise`.

- [ ] **Step 9: Gates and commit**

```bash
git add src/tui/cards.rs src/tui/review.rs src/tui/mod.rs src/tui/rows.rs src/tui/state.rs src/tui/input.rs src/tui/keys.rs src/tui/view.rs src/tui/panes.rs
git commit -m "feat(tui): comments as cards, an inline editor, a visual selection"
```

### Task 8: Finish, Request and copy in the TUI

Implements spec 10.4 "Finish" (the box and its table), "Request review" (the box), the answers (`send_seq`, `copy_seq`), the `send anyway` relabel, the copy keys, 10.3's `Y finish (2)` hint, 10.6's key-sheet and `RESERVED` sentences; 10.9 tests 8 (the boxes) and 9 (`Y @ c y n`), and Review Focus item 4. Read 10.4 before starting, with `src/tui/confirm.rs` beside it: the two boxes are panels of the same dialog kind, their keys handled in `input.rs` as the y/n box's are.

**Files:**
- Modify: `src/tui/review.rs` (`ReviewBox`), `src/tui/panes.rs` (`ReturnTo::{Finish, Request}`), `src/tui/keys.rs` (`Y`, `@`; `RESERVED = ["/"]`), `src/tui/state.rs` (`review_box`, `seen_send_seq`, `seen_copy_seq`, `pending_copy`), `src/tui/input.rs` (the box's keys, `Y`, `@`, `y` on a selection), `src/tui/view.rs` (the overlay, the footer hint, `body_is_drawn`), `src/tui/shell.rs` (the OSC 52 write), `src/engine/dispatch.rs` and `src/engine/types.rs` (`Refusal` beside `send_error`)

**Interfaces:**
- Consumes: Task 5's `Command::{Send, Copy}`, `SendRequest`, `Accepted`, `SendKind`, `ReviewScope`, `CopyRequest`, `CopyWhat`, `CopyOut`, `SendOutcome`, `Snapshot.{send_seq, send_error, send_outcome, send_waiting, copy_seq, copy}`; Tasks 6 and 7.
- Produces:

```rust
// Snapshot.send_refusal (Task 5's) is what the box reads beside send_error

// src/tui/review.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum BoxKind { Finish, Request }
pub struct ReviewBox { pub kind: BoxKind, pub scope: ReviewScope, pub refusal: Option<(String, Refusal)>,
                       /// Every condition the reviewer accepted with "send anyway" since the box opened: the
                       /// next Y carries all of them, so a restarted *and* working agent asks twice, not forever.
                       pub accepted: Accepted, pub pending: Option<u64>, pub drawn: bool }
impl ReviewBox {
    pub fn finish() -> Self;
    pub fn request(snapshot: &Snapshot) -> Self;                  // the default scope of 10.4
    pub fn panel(&self, snapshot: &Snapshot, width: u16) -> Panel;
    /// `(y_label, pick, copy, scope_line)`: what the box offers now; `y_label` None when Y is not offered.
    pub fn offers(&self, snapshot: &Snapshot) -> Offers;
    /// The request Y sends, with what a "send anyway" accepts.
    pub fn submit(&self, snapshot: &Snapshot) -> Option<Command>;
    pub fn copy(&self) -> Command;
    pub fn scope_available(&self, snapshot: &Snapshot, scope: &ReviewScope) -> bool;
}
pub struct Offers { pub y: Option<&'static str>, pub pick: bool, pub copy: bool, pub scope_line: bool, pub nothing: bool }
pub fn counts(snapshot: &Snapshot) -> (usize /* pending */, usize /* unconfirmed */, usize /* files */);
pub fn outcome_notice(outcome: &SendOutcome, snapshot: &Snapshot) -> (String, bool /* urgent */);

// src/tui/panes.rs
ReturnTo::Finish, ReturnTo::Request(ReviewScope)   // the scope the box had when A was pressed, restored on return
// src/tui/keys.rs
KeyAction::Finish ("Y", "finish: send the review"), KeyAction::RequestReview ("@", "request a review")
// src/tui/state.rs: ViewState gains
pub review_box: Option<ReviewBox>,
seen_send_seq: u64, seen_copy_seq: u64,
/// The OSC 52 text the shell writes before the next frame, taken once.
pub pending_copy: Option<String>,
/// A command `observe` decided on (the picker it opened asks for its rows); the shell sends it after `observe`, once.
pub pending_command: Option<Command>,
```

- [ ] **Step 1: The typed refusal on the snapshot**

Nothing to add: `Snapshot.send_refusal` is published by Task 5 (its `every_gate_refuses_or_passes_as_the_table_says` asserts `Busy` after the working refusal, `Restarted(Some(s2))` after the restart one, `Other` after `gone`). This step only confirms the field is what the box reads.

- [ ] **Step 2: The boxes, tests first**

In `review.rs`'s tests:

```rust
    fn with_comments(target: Option<Target>, state: TargetState, pending: usize, unconfirmed: usize) -> Snapshot {
        let mut s = crate::tui::state::tests::snapshot("src/cart.py", "raw", &[(10, " + ")]);
        // `snapshot` lists no files; the box's scope rules read `files` and `selected`.
        s.files = vec![crate::git::ChangedFile { path: "src/cart.py".into(), status: crate::git::ChangedFileStatus::Modified, staged: false, insertions: None, deletions: None }];
        s.selected = Some(FileKey { path: "src/cart.py".into(), staged: false, untracked: false });
        s.target = target;
        s.target_state = state;
        let mut comments = Vec::new();
        for i in 0..pending {
            comments.push(comment_at(&anchor("src/cart.py", 10), &format!("p{i}"), i as u64));
        }
        for i in 0..unconfirmed {
            let mut c = comment_at(&anchor("other.rs", 1), &format!("u{i}"), 100 + i as u64);
            c.state = CommentState::Unconfirmed { before: Vec::new(), stamp: stamp("aaaaaa") };
            comments.push(c);
        }
        s.comments = std::sync::Arc::new(comments);
        s
    }

    fn pane() -> Target {
        Target::Pane { pane: "w4:p2".into(), socket: "/s".into(), agent: "codex".into(), session: None, title: "demo".into() }
    }

    fn texts(panel: &Panel) -> Vec<String> {
        panel.rows.iter().filter_map(|r| match r { Row::Text(t) | Row::Note(t) | Row::Warn(t) => Some(t.clone()), Row::Entry { label, value, .. } => Some(format!("{label} {value}")), Row::Rule => None }).collect()
    }

    #[test]
    fn the_finish_box_says_what_the_table_says_for_each_state() {
        let cases: Vec<(TargetState, &str, Option<&str>, bool, bool)> = vec![
            (TargetState::Live("idle".into()), "Send 2 comments across 1 file to codex · w4:p2?", Some("send"), true, true),
            (TargetState::Live("done".into()), "Send 2 comments across 1 file to codex · w4:p2?", Some("send"), true, true),
            (TargetState::Live("working".into()), "codex is working in w4:p2; the review would queue behind its current turn.", Some("send anyway"), true, true),
            (TargetState::Live("unknown".into()), "codex's state in w4:p2 is unknown to the host.", Some("send anyway"), true, true),
            (TargetState::Live("sleeping".into()), "codex reports sleeping in w4:p2, a state this viewer does not know.", Some("send anyway"), true, true),
            (TargetState::Unverified, "w4:p2 has not been checked; it will be before sending.", Some("send"), true, true),
            (TargetState::Live("blocked".into()), "codex is waiting for an approval in w4:p2. Answer it there, or press A to pick another pane.", None, true, true),
            (TargetState::Restarted("blocked".into()), "codex is waiting for an approval in w4:p2. Answer it there, or press A to pick another pane.", None, true, true),
            (TargetState::Restarted("idle".into()), "codex in w4:p2 was restarted since you picked it and has not seen earlier messages.", Some("send anyway"), true, true),
            (TargetState::NoHost, "No host: this viewer runs outside herdr.", None, false, true),
        ];
        for (state, first, y, pick, copy) in cases {
            let s = with_comments(Some(pane()), state.clone(), 2, 0);
            let b = ReviewBox::finish();
            let panel = b.panel(&s, 60);
            assert_eq!(texts(&panel)[0], first, "{state:?}");
            let offers = b.offers(&s);
            assert_eq!((offers.y, offers.pick, offers.copy), (y, pick, copy), "{state:?}");
            assert_eq!(panel.title, "Finish");
        }
        let s = with_comments(Some(Target::Clipboard), TargetState::Clipboard, 2, 1);
        let panel = ReviewBox::finish().panel(&s, 60);
        assert_eq!(texts(&panel)[0], "Copy 2 pending + 1 unconfirmed across 2 files to the clipboard?");
        assert_eq!(ReviewBox::finish().offers(&s).y, Some("copy"));
        assert!(!ReviewBox::finish().offers(&s).copy, "copy is the Y of a clipboard target");
        // The counts name unconfirmed comments separately, across both scopes.
        let s = with_comments(Some(pane()), TargetState::Live("idle".into()), 1, 2);
        assert_eq!(texts(&ReviewBox::finish().panel(&s, 60))[0], "Send 1 pending + 2 unconfirmed across 2 files to codex · w4:p2?");
        assert_eq!(counts(&s), (1, 2, 2));
    }

    #[test]
    fn y_carries_what_the_box_showed_and_a_refusal_relabels_it() {
        let s = with_comments(Some(pane()), TargetState::Live("working".into()), 1, 0);
        let b = ReviewBox::finish();
        let Some(Command::Send(request)) = b.submit(&s) else { panic!() };
        assert_eq!(request.accepted, Accepted { busy: true, restarted: None });
        let s = with_comments(Some(pane()), TargetState::Live("idle".into()), 1, 0);
        let Some(Command::Send(request)) = ReviewBox::finish().submit(&s) else { panic!() };
        assert_eq!(request.accepted, Accepted::default());
        // A refusal the check found: the box shows it and the next Y accepts exactly that.
        let mut refused = ReviewBox::finish();
        let session = SessionRef { kind: "id".into(), value: "s2".into() };
        refused.refuse("codex in w4:p2 was restarted since you picked it and has not seen earlier messages.".into(), Refusal::Restarted(Some(session.clone())));
        assert_eq!(texts(&refused.panel(&s, 60))[0], "codex in w4:p2 was restarted since you picked it and has not seen earlier messages.");
        assert_eq!(refused.offers(&s).y, Some("send anyway"));
        let Some(Command::Send(request)) = refused.submit(&s) else { panic!() };
        assert_eq!(request.accepted, Accepted { busy: false, restarted: Some(Some(session.clone())) });
        // A second refusal for the other gate adds to what was accepted; nothing is forgotten.
        refused.refuse("codex is working in w4:p2; the review would queue behind its current turn.".into(), Refusal::Busy);
        let Some(Command::Send(request)) = refused.submit(&s) else { panic!() };
        assert_eq!(request.accepted, Accepted { busy: true, restarted: Some(Some(session)) }, "a restarted and working agent asks twice, not forever");
        // A restart into an agent with no session is accepted as that absence, not as "no restart".
        let mut sessionless = ReviewBox::finish();
        sessionless.refuse("codex in w4:p2 was restarted since you picked it and has not seen earlier messages.".into(), Refusal::Restarted(None));
        let Some(Command::Send(request)) = sessionless.submit(&s) else { panic!() };
        assert_eq!(request.accepted.restarted, Some(None));
        let mut other = ReviewBox::finish();
        other.refuse("could not verify w4:p2: deadline".into(), Refusal::Other);
        assert_eq!(other.offers(&s).y, Some("send"), "an unacceptable refusal keeps a plain retry");
        // A refusal never offers more than the state the chip now shows: blocked or no host after a refusal.
        let blocked = with_comments(Some(pane()), TargetState::Live("blocked".into()), 1, 0);
        assert_eq!(refused.offers(&blocked).y, None, "send anyway is withdrawn while the agent is blocked");
        assert_eq!(texts(&refused.panel(&blocked, 60))[0], "codex is working in w4:p2; the review would queue behind its current turn.", "the refusal is still what the box says");
        let no_host = with_comments(Some(pane()), TargetState::NoHost, 1, 0);
        let offers = other.offers(&no_host);
        assert!(offers.y.is_none() && !offers.pick && offers.copy);
        // Blocked offers no Y at all.
        let s = with_comments(Some(pane()), TargetState::Live("blocked".into()), 1, 0);
        assert!(ReviewBox::finish().submit(&s).is_none());
    }

    #[test]
    fn the_request_box_scopes_as_the_spec_says() {
        let mut s = with_comments(Some(pane()), TargetState::Live("idle".into()), 0, 0);
        // Two rows, a diff loaded: all changes first, both available.
        s.files.push(crate::git::ChangedFile { path: "b.rs".into(), status: crate::git::ChangedFileStatus::Modified, staged: false, insertions: None, deletions: None });
        let b = ReviewBox::request(&s);
        assert_eq!(b.scope, ReviewScope::All);
        assert_eq!(texts(&b.panel(&s, 60))[0], "Scope  f this file   a all changes (2)");
        assert_eq!(texts(&b.panel(&s, 60))[1], "Delegate a review of all changes to codex · w4:p2?");
        assert_eq!(b.offers(&s).y, Some("delegate"));
        assert!(b.scope_available(&s, &ReviewScope::File(FileKey { path: "src/cart.py".into(), staged: false, untracked: false })));
        // One row whose diff is loaded: the scopes coincide, no scope line.
        s.files.truncate(1);
        let b = ReviewBox::request(&s);
        assert!(!b.offers(&s).scope_line);
        // One row whose diff is not loaded: the line is drawn with f unavailable.
        s.diff = DiffState::Loading;
        let b = ReviewBox::request(&s);
        assert!(b.offers(&s).scope_line);
        assert!(!b.scope_available(&s, &ReviewScope::File(FileKey { path: "src/cart.py".into(), staged: false, untracked: false })));
        assert_eq!(b.offers(&s).y, Some("delegate"), "all changes is available with any row");
        // An empty list: nothing to review, n alone.
        s.files.clear();
        let b = ReviewBox::request(&s);
        let offers = b.offers(&s);
        assert!(offers.nothing && offers.y.is_none() && !offers.pick && !offers.copy);
        let panel = b.panel(&s, 60);
        assert_eq!(texts(&panel)[0], "nothing to review");
        assert_eq!(panel.footer, "n cancel");
        assert!(b.submit(&s).is_none());
    }

    #[test]
    fn outcome_notices_name_the_destination_and_the_unconfirmed_case_is_urgent() {
        let s = with_comments(Some(pane()), TargetState::Live("idle".into()), 0, 0);
        let sent = SendOutcome { kind: SendKind::Feedback, items: 2, to: Destination::Pane { pane: "w4:p2".into(), agent: "codex".into(), session: None }, unconfirmed: false, copy: None };
        assert_eq!(outcome_notice(&sent, &s), ("sent 2 items to codex · w4:p2".to_string(), false));
        let one = SendOutcome { items: 1, ..sent.clone() };
        assert_eq!(outcome_notice(&one, &s).0, "sent 1 item to codex · w4:p2");
        let unsure = SendOutcome { unconfirmed: true, ..sent.clone() };
        assert_eq!(outcome_notice(&unsure, &s), ("sent? the host did not confirm · 2 comments marked sent? until the next finish".to_string(), true));
        let requested = SendOutcome { kind: SendKind::Review { scope: ReviewScope::All }, ..sent.clone() };
        assert_eq!(outcome_notice(&requested, &s).0, "review requested from codex · w4:p2");
        let copied = SendOutcome { to: Destination::clipboard(), copy: Some(CopyOut { osc: None, notice: "copied 2 comments · also in ~/x/clipboard.md".into(), urgent: false }), ..sent };
        assert_eq!(outcome_notice(&copied, &s).0, "copied 2 comments · also in ~/x/clipboard.md");
    }
```

- [ ] **Step 3: `ReviewBox`**

Append to `review.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxKind {
    Finish,
    Request,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offers {
    pub y: Option<&'static str>,
    pub pick: bool,
    pub copy: bool,
    pub scope_line: bool,
    pub nothing: bool,
}

pub struct ReviewBox {
    pub kind: BoxKind,
    pub scope: ReviewScope,
    /// The engine's refusal, shown in place of the first line; what Y may accept rides beside it.
    pub refusal: Option<(String, Refusal)>,
    /// Every condition accepted with "send anyway" since the box opened, for the same target.
    pub accepted: Accepted,
    /// The `send_seq` at the time of the press; the answer moves past it.
    pub pending: Option<u64>,
    pub drawn: bool,
}

/// Pending comments, unconfirmed ones, and the files they are on, across both scopes.
pub fn counts(snapshot: &Snapshot) -> (usize, usize, usize) {
    let pending = snapshot.comments.iter().filter(|c| c.is_pending()).count();
    let unconfirmed = snapshot.comments.iter().filter(|c| matches!(c.state, CommentState::Unconfirmed { .. })).count();
    let files: std::collections::HashSet<&str> = snapshot
        .comments
        .iter()
        .filter(|c| matches!(c.state, CommentState::Pending | CommentState::Unconfirmed { .. }))
        .map(|c| c.anchor.key.path.as_str())
        .collect();
    (pending, unconfirmed, files.len())
}

fn count_phrase(pending: usize, unconfirmed: usize) -> String {
    match unconfirmed {
        0 => format!("{pending} comment{}", if pending == 1 { "" } else { "s" }),
        _ => format!("{pending} pending + {unconfirmed} unconfirmed"),
    }
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

fn pane_label(target: &Target) -> (String, String) {
    match target {
        Target::Pane { pane, agent, .. } => (sanitize(agent), sanitize(pane)),
        Target::Clipboard => ("clipboard".into(), String::new()),
    }
}

impl ReviewBox {
    pub fn finish() -> Self {
        Self { kind: BoxKind::Finish, scope: ReviewScope::All, refusal: None, accepted: Accepted::default(), pending: None, drawn: false }
    }

    /// The scope starts on all changes (spec 10.4).
    pub fn request(_snapshot: &Snapshot) -> Self {
        Self { kind: BoxKind::Request, scope: ReviewScope::All, refusal: None, accepted: Accepted::default(), pending: None, drawn: false }
    }

    /// A refusal the engine typed: shown in the box, and folded into what the next Y accepts.
    pub fn refuse(&mut self, message: String, refusal: Refusal) {
        match &refusal {
            Refusal::Busy => self.accepted.busy = true,
            // `Some(None)` accepts a restart into an agent that reports no session.
            Refusal::Restarted(session) => self.accepted.restarted = Some(session.clone()),
            Refusal::Other => {}
        }
        self.refusal = Some((message, refusal));
        self.pending = None;
    }

    pub fn scope_available(&self, snapshot: &Snapshot, scope: &ReviewScope) -> bool {
        match scope {
            ReviewScope::All => !snapshot.files.is_empty(),
            ReviewScope::File(key) => matches!(&snapshot.diff, DiffState::Ready(d) if &d.key == key),
        }
    }

    fn selected_key(snapshot: &Snapshot) -> Option<FileKey> {
        snapshot.selected.clone()
    }

    /// The state line of 10.4's table and what the keys row offers; the refusal replaces the state line.
    fn state_line(&self, snapshot: &Snapshot) -> (String, Offers) {
        let (pending, unconfirmed, files) = counts(snapshot);
        let what = count_phrase(pending, unconfirmed);
        let Some(target) = &snapshot.target else {
            return ("no target: press A".into(), Offers { y: None, pick: true, copy: true, scope_line: false, nothing: false });
        };
        let (agent, pane) = pane_label(target);
        let all = Offers { y: Some("send"), pick: true, copy: true, scope_line: false, nothing: false };
        let anyway = Offers { y: Some("send anyway"), ..all };
        let no_y = Offers { y: None, ..all };
        // The table decides what the current state allows; a refusal replaces the line and may relabel
        // Y, but never offers more than the state does: a target that went blocked or lost its host
        // after a refusal offers what blocked or no-host offers.
        let (line, table) = self.table_line(snapshot, &agent, &pane, &what, files, all, anyway, no_y);
        if let Some((message, refusal)) = &self.refusal {
            // A plain send under an accepted refusal reads "send anyway"; nothing else is relabelled.
            let y = match (table.y, refusal) {
                (Some("send") | Some("delegate"), Refusal::Busy | Refusal::Restarted(_)) => Some("send anyway"),
                (y, _) => y,
            };
            return (sanitize(message), Offers { y, ..table });
        }
        (line, table)
    }

    #[allow(clippy::too_many_arguments)]
    fn table_line(&self, snapshot: &Snapshot, agent: &str, pane: &str, what: &str, files: usize, all: Offers, anyway: Offers, no_y: Offers) -> (String, Offers) {
        match (&snapshot.target_state, self.kind) {
            (TargetState::Clipboard, BoxKind::Finish) => (
                format!("Copy {what} across {} to the clipboard?", plural(files, "file")),
                Offers { y: Some("copy"), pick: true, copy: false, scope_line: false, nothing: false },
            ),
            (TargetState::Clipboard, BoxKind::Request) => (
                "Copy the review request to the clipboard?".into(),
                Offers { y: Some("copy"), pick: true, copy: false, scope_line: false, nothing: false },
            ),
            (TargetState::NoHost, _) => ("No host: this viewer runs outside herdr.".into(), Offers { y: None, pick: false, copy: true, scope_line: false, nothing: false }),
            (TargetState::Live(s) | TargetState::Restarted(s), _) if s == "blocked" => (
                format!("{agent} is waiting for an approval in {pane}. Answer it there, or press A to pick another pane."),
                no_y,
            ),
            (TargetState::Restarted(_), _) => (format!("{agent} in {pane} was restarted since you picked it and has not seen earlier messages."), anyway),
            (TargetState::Live(s), _) if s == "working" => (format!("{agent} is working in {pane}; the review would queue behind its current turn."), anyway),
            (TargetState::Live(s), _) if s == "unknown" => (format!("{agent}'s state in {pane} is unknown to the host."), anyway),
            // A status outside the documented five is as good as unknown (the engine's gate agrees).
            (TargetState::Live(s), _) if s != "idle" && s != "done" => (format!("{agent} reports {} in {pane}, a state this viewer does not know.", sanitize(s)), anyway),
            (TargetState::Unverified, _) => (format!("{pane} has not been checked; it will be before sending."), all),
            (TargetState::Live(_), BoxKind::Finish) => (format!("Send {what} across {} to {agent} · {pane}?", plural(files, "file")), all),
            (TargetState::Live(_), BoxKind::Request) => (
                format!("Delegate a review of {} to {agent} · {pane}?", match &self.scope { ReviewScope::All => "all changes".to_string(), ReviewScope::File(k) => sanitize(&k.path) }),
                Offers { y: Some("delegate"), ..all },
            ),
            // Left and Gone never reach the box: the key opens the picker instead.
            (TargetState::Left | TargetState::Gone, _) => (format!("{agent} · {pane} is gone · pick a pane"), no_y),
        }
    }

    pub fn offers(&self, snapshot: &Snapshot) -> Offers {
        let (_, mut offers) = self.state_line(snapshot);
        if self.kind == BoxKind::Request {
            let one_loaded_row = snapshot.files.len() == 1 && self.scope_available(snapshot, &ReviewScope::All) && Self::selected_key(snapshot).is_some_and(|k| self.scope_available(snapshot, &ReviewScope::File(k)));
            offers.scope_line = !snapshot.files.is_empty() && !one_loaded_row;
            offers.nothing = snapshot.files.is_empty();
            if offers.nothing {
                // Nothing to review: `n` alone (10.4), neither a pane to pick for it nor a copy of it.
                offers = Offers { y: None, pick: false, copy: false, scope_line: false, nothing: true };
            } else if !self.scope_available(snapshot, &self.scope) {
                offers.y = None;
            }
            if offers.y == Some("send") {
                offers.y = Some("delegate");
            }
        }
        offers
    }

    pub fn panel(&self, snapshot: &Snapshot, width: u16) -> Panel {
        let (line, offers) = (self.state_line(snapshot).0, self.offers(snapshot));
        let mut rows = Vec::new();
        if offers.nothing {
            rows.push(Row::Text("nothing to review".into()));
        } else {
            if offers.scope_line {
                let n = snapshot.files.len();
                // The chosen scope is drawn in reverse video by view.rs from this marker pair.
                rows.push(Row::Text(format!("Scope  f this file   a all changes ({n})")));
            }
            rows.push(Row::Note(line));
        }
        let mut keys = Vec::new();
        if let Some(y) = offers.y { keys.push(format!("Y {y}")); }
        if offers.pick { keys.push("A pick another pane".into()); }
        if offers.copy { keys.push("c copy".into()); }
        keys.push("n cancel".into());
        if self.pending.is_some() {
            keys = vec![if snapshot.send_waiting { super::super::engine::dispatch::NOTICE_WAITING.to_string() } else { "sending…".to_string() }];
        }
        Panel {
            title: match self.kind { BoxKind::Finish => "Finish", BoxKind::Request => "Request review" }.into(),
            rows,
            footer: keys.join(" · "),
            cursor: None,
            offset: 0,
        }
    }

    /// What Y sends: the kind, and every condition accepted so far plus the one the box shows now.
    pub fn submit(&self, snapshot: &Snapshot) -> Option<Command> {
        let offers = self.offers(snapshot);
        offers.y?;
        let mut accepted = self.accepted.clone();
        if offers.y == Some("send anyway") && self.refusal.is_none() {
            // The chip's own state is what the box showed: a busy agent, or a restart.
            match &snapshot.target_state {
                TargetState::Live(_) => accepted.busy = true,
                TargetState::Restarted(_) => {
                    // The chip does not carry the new session, so this names the old one; the engine
                    // refuses with the session it found and `refuse` folds that in for the next Y.
                    accepted.restarted = Some(snapshot.target.as_ref().and_then(|t| match t { Target::Pane { session, .. } => session.clone(), _ => None }));
                }
                _ => {}
            }
        }
        let kind = match self.kind {
            BoxKind::Finish => SendKind::Feedback,
            BoxKind::Request => SendKind::Review { scope: self.scope.clone() },
        };
        Some(Command::Send(SendRequest { kind, accepted }))
    }

    pub fn copy(&self) -> Command {
        Command::Copy(CopyRequest {
            what: match self.kind {
                BoxKind::Finish => CopyWhat::Review,
                BoxKind::Request => CopyWhat::Request { scope: self.scope.clone() },
            },
        })
    }
}

/// The notice for a successful send (spec 10.4): urgent when the host did not confirm.
pub fn outcome_notice(outcome: &SendOutcome, _snapshot: &Snapshot) -> (String, bool) {
    if let Some(copy) = &outcome.copy {
        return (copy.notice.clone(), copy.urgent || outcome.unconfirmed);
    }
    let to = match &outcome.to {
        Destination::Pane { pane, agent, .. } => format!("{} · {}", sanitize(agent), sanitize(pane)),
        Destination::Clipboard { .. } => "the clipboard".to_string(),
    };
    if outcome.unconfirmed {
        return (
            format!(
                "sent? the host did not confirm · {} marked sent? until the next finish",
                plural(outcome.items as usize, "comment")
            ),
            true,
        );
    }
    match outcome.kind {
        SendKind::Feedback => (format!("sent {} to {to}", plural(outcome.items as usize, "item")), false),
        SendKind::Review { .. } => (format!("review requested from {to}"), false),
    }
}
```

A note on the `Restarted` acceptance in `submit`: the chip's `Restarted` state does not carry the new session, and the target record carries the old one, so a `Y` on a box that *opened* on `Restarted` sends an acceptance the engine will refuse (its `restarted` names the old session) and re-show with the typed refusal carrying the new session; `refuse` folds that session in and the second `Y` passes. The engine runs the continuity gate before the status gate, so a restarted *and* working agent is refused for the restart first, then for the status; `accepted` accumulates both, and the third `Y` passes: the sequence is finite because every refusal adds a condition and none is dropped. This is one or two extra presses in a rare case and keeps the rule that a send is never forced past something the reviewer has not seen; `y_carries_what_the_box_showed_and_a_refusal_relabels_it` pins both. If the orchestrator prefers to avoid the first extra press, the alternative is a `Snapshot.restarted_session` field set by the check; record the choice in the execution notes.

- [ ] **Step 4: Keys, state, input, view, shell**

1. `keys.rs`: `Binding { key: "Y", label: "finish: send the review", action: KeyAction::Finish, vimeflow: Some("diff-review-finish") }` and `Binding { key: "@", label: "request a review", action: KeyAction::RequestReview, vimeflow: Some("diff-review-request") }`; `RESERVED` becomes `&["/"]`; `KEYS.len()` becomes 37. The key sheet (`help_panel`) gains three `Row::Note`s after the entries: `in the Finish and Request boxes: Y confirm · A pick another pane · c copy · n cancel · f / a scope`, `in the editor: Enter save · ctrl+j newline · ctrl+h/l category · Esc cancel`, `y copies a selection made with v`; so `help_panel(false).rows.len()` becomes 40 in `keys.rs`'s test, and the two scroll assertions in `input.rs`'s wheel test (`KEYS.len() - 6`, `KEYS.len() - 9`) become `dialog::line_count(&keys::help_panel(false), 40) - 6` and `… - 9`: the notes wrap at forty columns, and `scroll_help` already bounds the offset by that rendered count, not by the row count. The test that asserts every `KEYS` entry does something gains the new keys (they do: a notice or a box); the reserved test keeps `/` inert.

2. `state.rs`: `review_box: Option<ReviewBox>`, `seen_send_seq`, `seen_copy_seq`, `pending_copy: Option<String>`. In `observe`:

```rust
        let send_answered = snapshot.send_seq != self.seen_send_seq;
        if send_answered {
            self.seen_send_seq = snapshot.send_seq;
            let answered_box = self.review_box.as_ref().is_some_and(|b| b.pending.is_some_and(|p| snapshot.send_seq > p));
            match (&snapshot.send_error, &snapshot.send_outcome) {
                (Some(error), _) if answered_box => {
                    // A refusal keeps the box open with the reason in place of its first line, and folds
                    // what it names into the next Y, so no two conditions can trade refusals for ever.
                    if let Some(b) = self.review_box.as_mut() {
                        b.refuse(error.clone(), snapshot.send_refusal.clone().unwrap_or(crate::engine::dispatch::Refusal::Other));
                    }
                }
                (Some(error), _) => {
                    self.displace_urgent();
                    self.notify(crate::tui::sanitize::sanitize(error));
                }
                (None, Some(outcome)) => {
                    self.review_box = None;
                    let (text, urgent) = crate::tui::review::outcome_notice(outcome, snapshot);
                    self.displace_urgent();
                    if urgent { self.warn(text) } else { self.notify(text) }
                }
                (None, None) => {}
            }
        }
        if snapshot.copy_seq != self.seen_copy_seq {
            self.seen_copy_seq = snapshot.copy_seq;
            if let Some(copy) = &snapshot.copy {
                self.pending_copy = copy.osc.clone();
                // A copy published with a send's answer spoke through it (the outcome's notice, or the
                // error that names the copy); one made by `c` or `y` speaks here. The engine publishes a
                // send's copy in the snapshot that answers the send, and each snapshot is observed once.
                if !send_answered {
                    self.displace_urgent();
                    if copy.urgent { self.warn(copy.notice.clone()) } else { self.notify(copy.notice.clone()) }
                }
            }
        }
        // A target that leaves or vanishes under an open box: the box becomes the picker (10.4's table),
        // which asks for its rows through `pending_command`. A box with a send pending waits for the
        // answer first; the refusal it brings clears `pending`, and the next observe gets here.
        if let (Some(b), Some(Target::Pane { agent, pane, .. }), TargetState::Left | TargetState::Gone) = (&self.review_box, &snapshot.target, &snapshot.target_state) {
            if b.pending.is_none() {
                let return_to = match b.kind {
                    crate::tui::review::BoxKind::Finish => crate::tui::panes::ReturnTo::Finish,
                    crate::tui::review::BoxKind::Request => crate::tui::panes::ReturnTo::Request(b.scope.clone()),
                };
                self.review_box = None;
                self.notify(format!("{} · {} is gone · pick a pane", crate::tui::sanitize::sanitize(agent), crate::tui::sanitize::sanitize(pane)));
                self.panes_token += 1;
                self.panes = Some(crate::tui::panes::PanePicker::open(self.panes_token, return_to));
                self.pending_command = Some(Command::LoadPanes(self.panes_token));
            }
        }
        // The picker opened by Y or @ returns to its box.
        if let Some(picker) = &self.panes {
            if picker.done && snapshot.target.is_some() {
                // By reference: the scope is cloned out of the picker, which `self.panes` still owns here.
                match &picker.return_to {
                    crate::tui::panes::ReturnTo::Finish => self.review_box = Some(crate::tui::review::ReviewBox::finish()),
                    crate::tui::panes::ReturnTo::Request(scope) => {
                        // The scope chosen before A is the scope after the pick; `offers` withdraws Y if it is gone.
                        let mut b = crate::tui::review::ReviewBox::request(snapshot);
                        b.scope = scope.clone();
                        self.review_box = Some(b);
                    }
                    _ => {}
                }
            }
        }
```

A box whose target state turns `Left`/`Gone` while open is closed by `observe`, which opens the picker with the gone notice instead (`ReturnTo` the box's kind, a Request box's with its scope). `observe` returns nothing, so the rows are asked for through one slot: `ViewState.pending_command: Option<Command>`, set to `Command::LoadPanes(self.panes_token)` here and drained by the shell right after `observe` (`if let Some(command) = state.pending_command.take() { let _ = handle.commands.send(command); }`, beside the `pending_copy` write of Step 4), so the picker a snapshot opened loads like one a key opened. The code is in `observe` above; `a_box_whose_target_goes_away_becomes_the_picker_and_asks_for_rows` in Step 5 pins it.

3. `input.rs`: routing: help, confirm, `review_box` (`box_key`), editor, pane picker, base picker, body. `box_key`:

```rust
fn box_key(state: &mut ViewState, snapshot: &Snapshot, key: KeyEvent) -> Outcome {
    let Some(b) = state.review_box.as_mut() else { return Outcome::Inert };
    if b.pending.is_some() {
        return Outcome::Inert;
    }
    match (key.code, key.modifiers) {
        (KeyCode::Char('n'), KeyModifiers::NONE) | (KeyCode::Esc, KeyModifiers::NONE) => {
            state.review_box = None;
            Outcome::Redraw
        }
        (KeyCode::Char('Y'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
            if !b.drawn {
                return Outcome::Inert;
            }
            match b.submit(snapshot) {
                Some(command) => {
                    b.pending = Some(snapshot.send_seq);
                    Outcome::Engine(command)
                }
                None => Outcome::Inert,
            }
        }
        (KeyCode::Char('A'), KeyModifiers::SHIFT | KeyModifiers::NONE) if b.offers(snapshot).pick => {
            let return_to = match b.kind { review::BoxKind::Finish => panes::ReturnTo::Finish, review::BoxKind::Request => panes::ReturnTo::Request(b.scope.clone()) };
            state.review_box = None;
            state.panes_token += 1;
            state.panes = Some(panes::PanePicker::open(state.panes_token, return_to));
            Outcome::Engine(Command::LoadPanes(state.panes_token))
        }
        (KeyCode::Char('c'), KeyModifiers::NONE) if b.offers(snapshot).copy => {
            let command = b.copy();
            state.review_box = None;
            Outcome::Engine(command)
        }
        (KeyCode::Char('f'), KeyModifiers::NONE) if b.kind == review::BoxKind::Request => {
            if let Some(key) = snapshot.selected.clone() {
                let scope = dispatch::ReviewScope::File(key);
                if b.scope_available(snapshot, &scope) { b.scope = scope; return Outcome::Redraw; }
            }
            Outcome::Inert
        }
        (KeyCode::Char('a'), KeyModifiers::NONE) if b.kind == review::BoxKind::Request => {
            if b.scope_available(snapshot, &dispatch::ReviewScope::All) { b.scope = dispatch::ReviewScope::All; Outcome::Redraw } else { Outcome::Inert }
        }
        _ => Outcome::Inert,
    }
}
```

`Finish` and `RequestReview` in `act`:

```rust
        Finish | RequestReview => {
            let kind = if action == Finish { review::BoxKind::Finish } else { review::BoxKind::Request };
            if action == Finish && review::counts(snapshot) == (0, 0, 0) {
                state.notify(dispatch::NOTICE_NO_PENDING);
                return Outcome::Redraw;
            }
            // Nothing to review: the box says so with `n` alone (10.4), whatever the target; no picker.
            if action == RequestReview && snapshot.files.is_empty() {
                state.review_box = Some(review::ReviewBox::request(snapshot));
                return Outcome::Redraw;
            }
            match (&snapshot.target, &snapshot.target_state) {
                (None, _) | (Some(_), TargetState::Left | TargetState::Gone) => {
                    if let Some(Target::Pane { agent, pane, .. }) = &snapshot.target {
                        state.notify(format!("{} · {} is gone · pick a pane", sanitize(agent), sanitize(pane)));
                    }
                    let return_to = if action == Finish { panes::ReturnTo::Finish } else { panes::ReturnTo::Request(dispatch::ReviewScope::All) };
                    state.panes_token += 1;
                    state.panes = Some(panes::PanePicker::open(state.panes_token, return_to));
                    return Outcome::Engine(Command::LoadPanes(state.panes_token));
                }
                _ => {}
            }
            state.review_box = Some(match kind { review::BoxKind::Finish => review::ReviewBox::finish(), review::BoxKind::Request => review::ReviewBox::request(snapshot) });
        }
```

`y` on a selection, in `handle_key` before `keys::lookup` is consulted for the body: `if key is 'y' with no modifiers && state.visual.is_some()` → `selection(diff, visual, cursor)` → `Command::Copy(CopyRequest { what: CopyWhat::Selection(selection_text(..)) })`, clearing `visual`; without a selection `y` is inert (not in `KEYS`).

4. `view.rs`: the box overlay after the confirm box's, `panel_width = columns.min(72)`, height from `dialog::line_count + 4` as the y/n box does; and `handle_mouse` returns `Inert` for every click and wheel event while `state.review_box.is_some()`, exactly as it does for `state.confirm` (a test, `the_boxes_are_modal_for_the_mouse_too`, scrolls and clicks a file row with the Finish box open and asserts `Inert` and an unchanged offset); `b.drawn` set by the shell when the box fits, as `confirm.drawn` is (`ReviewBox::fits(width, height)` mirrors `Confirm::fits`). The scope line's chosen word in reverse video: `view.rs` post-processes the rendered `Scope  f this file   a all changes (n)` line, setting `reverse` on the span of the chosen scope's words. The footer hint: `hints` gains `format!("Y finish ({pending})")` while `pending > 0` and `@ request` always (dropped before `? help` as the others are). `body_is_drawn` gains `&& state.review_box.is_none()`.

5. `shell.rs`: after `state.observe(&next)` in the snapshot loop and before drawing, `if let Some(command) = state.pending_command.take() { let _ = handle.commands.send(command); }` and `if let Some(osc) = state.pending_copy.take() { write_copy(&mut io::stdout(), &osc)?; }` with:

```rust
/// The OSC 52 write of spec 10.4: raw bytes between frames; the terminal does not answer.
fn write_copy(out: &mut impl Write, osc: &str) -> io::Result<()> {
    out.write_all(osc.as_bytes())?;
    out.flush()
}
```

and a unit test that a `Vec<u8>` receives exactly the sequence.

- [ ] **Step 5: Input and view tests**

```rust
    #[test]
    fn y_opens_the_finish_box_and_sends_what_it_showed() {
        let (mut snap, mut st) = review_setup(&[(10, " + ")]);
        snap.target = Some(pane_target());
        snap.target_state = crate::engine::TargetState::Live("idle".into());
        handle_key(&mut st, &snap, key("Y"), 120);
        assert_eq!(st.notice.as_ref().map(|n| n.text.as_str()), Some(dispatch::NOTICE_NO_PENDING));
        snap.comments = std::sync::Arc::new(vec![comment_at(&anchor_on(&snap, 10), "p", 1)]);
        handle_key(&mut st, &snap, key("Y"), 120);
        assert!(st.review_box.is_some());
        assert_eq!(handle_key(&mut st, &snap, key("Y"), 120), Outcome::Inert, "not drawn yet");
        st.review_box.as_mut().unwrap().drawn = true;
        let outcome = handle_key(&mut st, &snap, key("Y"), 120);
        assert!(matches!(outcome, Outcome::Engine(Command::Send(dispatch::SendRequest { kind: dispatch::SendKind::Feedback, accepted })) if accepted == dispatch::Accepted::default()));
        assert_eq!(handle_key(&mut st, &snap, key("Y"), 120), Outcome::Inert, "a second Y while pending");
        // A refusal keeps the box open, relabelled; the next Y accepts it.
        snap.send_seq = 1;
        snap.send_error = Some("codex is working in w4:p2; the review would queue behind its current turn.".into());
        snap.send_refusal = Some(dispatch::Refusal::Busy);
        st.observe(&snap);
        let b = st.review_box.as_ref().unwrap();
        assert_eq!(b.offers(&snap).y, Some("send anyway"));
        let outcome = handle_key(&mut st, &snap, key("Y"), 120);
        assert!(matches!(outcome, Outcome::Engine(Command::Send(r)) if r.accepted.busy));
        // Success closes it with the notice.
        snap.send_seq = 2;
        snap.send_error = None;
        snap.send_refusal = None;
        snap.send_outcome = Some(dispatch::SendOutcome { kind: dispatch::SendKind::Feedback, items: 1, to: crate::engine::Destination::Pane { pane: "w4:p2".into(), agent: "codex".into(), session: None }, unconfirmed: false, copy: None });
        st.observe(&snap);
        assert!(st.review_box.is_none());
        assert_eq!(st.notice.as_ref().map(|n| n.text.as_str()), Some("sent 1 item to codex · w4:p2"));
    }

    #[test]
    fn a_pick_made_for_the_finish_box_returns_to_it_without_sending() {
        let (mut snap, mut st) = review_setup(&[(10, " + ")]);
        snap.target = None;
        snap.target_state = crate::engine::TargetState::Unverified;
        snap.comments = std::sync::Arc::new(vec![comment_at(&anchor_on(&snap, 10), "p", 1)]);
        assert_eq!(handle_key(&mut st, &snap, key("Y"), 120), Outcome::Engine(Command::LoadPanes(1)));
        assert!(st.review_box.is_none() && st.panes.is_some());
        // Y again while the rows load: inert in the picker (typed as a filter character, harmless).
        handle_key(&mut st, &snap, key("Y"), 120);
        snap.panes = Some(std::sync::Arc::new(Vec::new()));
        snap.panes_seq = 1;
        st.observe(&snap);
        st.panes.as_mut().unwrap().input.clear();
        st.panes.as_mut().unwrap().retarget(&snap);
        assert!(matches!(handle_key(&mut st, &snap, key("Enter"), 120), Outcome::Engine(Command::SetTarget(crate::engine::Target::Clipboard))));
        snap.target = Some(crate::engine::Target::Clipboard);
        snap.target_state = crate::engine::TargetState::Clipboard;
        snap.target_seq = 1;
        st.observe(&snap);
        assert!(st.panes.is_none());
        let b = st.review_box.as_ref().expect("the box reopened");
        assert_eq!((b.kind, b.pending), (review::BoxKind::Finish, None), "nothing was sent by the pick");
        assert_eq!(b.offers(&snap).y, Some("copy"));
    }

    #[test]
    fn a_comment_answer_and_a_send_answer_in_a_row_both_speak() {
        let (mut snap, mut st) = review_setup(&[(10, " + ")]);
        // Two answers before a key: the second shows at once, the first waits, then speaks.
        snap.comment_seq = 1;
        snap.comment_error = Some(comments::NOTICE_CAP.to_string());
        snap.comment_refused = true;
        st.observe(&snap);
        assert_eq!(st.notice.as_ref().map(|n| n.text.as_str()), Some(comments::NOTICE_CAP));
        snap.send_seq = 1;
        snap.send_error = Some("could not verify w4:p2: deadline".to_string());
        st.observe(&snap);
        assert_eq!(st.notice.as_ref().map(|n| n.text.as_str()), Some("could not verify w4:p2: deadline"));
        // A send error outside a box is a plain notice; an unrelated snapshot must not replace it with
        // the deferred warning before it is read.
        snap.refreshing = !snap.refreshing;
        st.observe(&snap);
        assert_eq!(st.notice.as_ref().map(|n| n.text.as_str()), Some("could not verify w4:p2: deadline"));
        // Read (a key clears it): the displaced warning is back; read again: nothing more.
        handle_key(&mut st, &snap, key("j"), 120);
        assert_eq!(st.notice.as_ref().map(|n| n.text.as_str()), Some(comments::NOTICE_CAP), "the first answer was not lost");
        handle_key(&mut st, &snap, key("j"), 120);
        assert!(st.notice.is_none());
    }

    #[test]
    fn a_box_whose_target_goes_away_becomes_the_picker_and_asks_for_rows() {
        let (mut snap, mut st) = review_setup(&[(10, " + ")]);
        snap.target = Some(pane_target());
        snap.target_state = crate::engine::TargetState::Live("idle".into());
        snap.comments = std::sync::Arc::new(vec![comment_at(&anchor_on(&snap, 10), "p", 1)]);
        assert_eq!(handle_key(&mut st, &snap, key("Y"), 120), Outcome::Redraw);
        assert!(st.review_box.is_some());
        // The agent leaves the pane while the box is open: the box becomes the picker, which asks for rows.
        snap.target_state = crate::engine::TargetState::Gone;
        st.observe(&snap);
        assert!(st.review_box.is_none());
        assert!(matches!(st.panes.as_ref().map(|p| &p.return_to), Some(crate::tui::panes::ReturnTo::Finish)));
        assert_eq!(st.pending_command.take(), Some(Command::LoadPanes(1)));
        assert_eq!(st.pending_command.take(), None, "sent once");
        assert_eq!(st.notice.as_ref().map(|n| n.text.as_str()), Some("codex · w4:p2 is gone · pick a pane"));
        // Observed again with the picker open, nothing happens twice.
        st.observe(&snap);
        assert_eq!(st.pending_command, None);
        assert_eq!(st.panes_token, 1);
    }

    #[test]
    fn the_request_box_scopes_with_f_and_a_and_c_copies_without_claiming() {
        let (mut snap, mut st) = review_setup(&[(10, " + ")]);
        snap.target = Some(pane_target());
        snap.target_state = crate::engine::TargetState::Live("idle".into());
        snap.files.push(crate::git::ChangedFile { path: "b.rs".into(), status: crate::git::ChangedFileStatus::Modified, staged: false, insertions: None, deletions: None });
        snap.selected = Some(FileKey { path: "a.rs".into(), staged: false, untracked: false });
        handle_key(&mut st, &snap, key("@"), 120);
        let b = st.review_box.as_ref().unwrap();
        assert_eq!(b.scope, dispatch::ReviewScope::All);
        handle_key(&mut st, &snap, key("f"), 120);
        assert!(matches!(&st.review_box.as_ref().unwrap().scope, dispatch::ReviewScope::File(k) if k.path == "a.rs"));
        handle_key(&mut st, &snap, key("a"), 120);
        assert_eq!(st.review_box.as_ref().unwrap().scope, dispatch::ReviewScope::All);
        // A pick made from a file-scoped box returns to a file-scoped box.
        handle_key(&mut st, &snap, key("f"), 120);
        assert_eq!(handle_key(&mut st, &snap, key("A"), 120), Outcome::Engine(Command::LoadPanes(1)));
        assert!(matches!(st.panes.as_ref().map(|p| &p.return_to), Some(crate::tui::panes::ReturnTo::Request(dispatch::ReviewScope::File(k))) if k.path == "a.rs"));
        snap.panes = Some(std::sync::Arc::new(Vec::new()));
        snap.panes_seq = 1;
        st.observe(&snap);
        st.panes.as_mut().unwrap().input.clear();
        st.panes.as_mut().unwrap().retarget(&snap);
        assert!(matches!(handle_key(&mut st, &snap, key("Enter"), 120), Outcome::Engine(Command::SetTarget(crate::engine::Target::Clipboard))));
        snap.target = Some(crate::engine::Target::Clipboard);
        snap.target_state = crate::engine::TargetState::Clipboard;
        snap.target_seq = 1;
        st.observe(&snap);
        assert!(matches!(&st.review_box.as_ref().expect("the box reopened").scope, dispatch::ReviewScope::File(k) if k.path == "a.rs"), "the pick kept the scope");
        snap.target = Some(pane_target());
        snap.target_state = crate::engine::TargetState::Live("idle".into());
        handle_key(&mut st, &snap, key("a"), 120);
        let outcome = handle_key(&mut st, &snap, key("c"), 120);
        assert!(matches!(outcome, Outcome::Engine(Command::Copy(dispatch::CopyRequest { what: dispatch::CopyWhat::Request { scope: dispatch::ReviewScope::All } }))));
        assert!(st.review_box.is_none());
        // Gone target: @ opens the picker with the notice (the second picker of this test).
        snap.target_state = crate::engine::TargetState::Gone;
        assert_eq!(handle_key(&mut st, &snap, key("@"), 120), Outcome::Engine(Command::LoadPanes(2)));
        assert_eq!(st.notice.as_ref().map(|n| n.text.as_str()), Some("codex · w4:p2 is gone · pick a pane"));
        handle_key(&mut st, &snap, key("Esc"), 120);
        // A clean repository with no target: @ shows `nothing to review`, n alone; no picker, no host call.
        snap.files.clear();
        snap.target = None;
        snap.target_state = crate::engine::TargetState::Unverified;
        assert_eq!(handle_key(&mut st, &snap, key("@"), 120), Outcome::Redraw);
        assert!(st.panes.is_none());
        let b = st.review_box.as_ref().expect("the box opened");
        let panel = b.panel(&snap, 60);
        assert_eq!((panel.rows.len(), panel.footer.as_str()), (1, "n cancel"));
        assert_eq!(handle_key(&mut st, &snap, key("A"), 120), Outcome::Inert);
        assert_eq!(handle_key(&mut st, &snap, key("c"), 120), Outcome::Inert);
        assert_eq!(handle_key(&mut st, &snap, key("n"), 120), Outcome::Redraw);
        assert!(st.review_box.is_none());
    }

    #[test]
    fn y_copies_a_selection_and_n_means_no_in_a_box_and_next_file_outside() {
        let (mut snap, mut st) = review_setup(&[(10, " ++ ")]);
        assert_eq!(handle_key(&mut st, &snap, key("n"), 120), Outcome::Engine(Command::SelectNext));
        handle_key(&mut st, &snap, key("v"), 120);
        handle_key(&mut st, &snap, key("j"), 120);
        let outcome = handle_key(&mut st, &snap, key("y"), 120);
        assert!(matches!(outcome, Outcome::Engine(Command::Copy(dispatch::CopyRequest { what: dispatch::CopyWhat::Selection(text) })) if text == "line +\nline +"), "the fixture's added lines read `line +`");
        assert!(st.visual.is_none());
        snap.comments = std::sync::Arc::new(vec![comment_at(&anchor_on(&snap, 10), "p", 1)]);
        handle_key(&mut st, &snap, key("Y"), 120);
        assert_eq!(handle_key(&mut st, &snap, key("n"), 120), Outcome::Redraw);
        assert!(st.review_box.is_none());
        // A copy answer speaks once, with the engine's notice.
        snap.copy_seq = 1;
        snap.copy = Some(std::sync::Arc::new(dispatch::CopyOut { osc: Some("\x1b]52;c;AA==\x07".into()), notice: "copied selection · also in ~/x/clipboard.md".into(), urgent: false }));
        st.observe(&snap);
        assert_eq!(st.pending_copy.as_deref(), Some("\x1b]52;c;AA==\x07"));
        assert_eq!(st.notice.as_ref().map(|n| n.text.as_str()), Some("copied selection · also in ~/x/clipboard.md"));
    }
```

(`pane_target()` returns a `Target::Pane` for `codex` in `w4:p2`; `review_setup`, `comment_at` and `anchor_on` are Task 7's fixtures; the `n` test relies on `SelectNext` being what `n` sends today.) In `view.rs`'s tests: `the_finish_box_is_drawn_for_each_row_of_the_table` (render with the box open for `Live(idle)`, `Live(blocked)`, `NoHost`, `Clipboard`, asserting the first line and the footer keys), `the_request_box_draws_its_scope_line_in_three_shapes`, `the_footer_offers_y_finish_with_the_pending_count` (`Y finish (2)` present with two pending, absent with none), `the_key_sheet_lists_the_new_rows`.

- [ ] **Step 6: Gates and commit**

```bash
git add src/tui/review.rs src/tui/panes.rs src/tui/keys.rs src/tui/state.rs src/tui/input.rs src/tui/view.rs src/tui/shell.rs src/engine/dispatch.rs src/engine/types.rs src/engine/session.rs
git commit -m "feat(tui): the finish and request boxes, the copy keys, the answers on the snapshot"
```

### Task 9: The recording test, Tier B, documentation and version 0.0.5

Implements spec 10.6 "The read-only guarantee" and "Standalone", 10.7's version paragraph, 10.9 tests 10 and 11 and criteria 12-17. Read 10.6, 10.7 and 10.9 before starting.

**Files:**
- Create: `tests/review_loop_guarantee.rs`
- Modify: `tests/e2e_real_herdr.rs`, `README.md`, `README.zh-CN.md`, `README.ja.md`, `AGENTS.md`, `PORT-SURFACE.md`, `docs/superpowers/specs/2026-09-18-hunks-roadmap-p1-viewer-design.md` (one note under the 1.2 table), `docs/superpowers/specs/2026-09-30-agent-review-loop-design.md` (the state-file list of 10.6 and 10.9, see Step 1), `docs/acceptance-p1.md`, `Cargo.toml`, `Cargo.lock`, `herdr-plugin.toml`

**Interfaces:**
- Consumes: everything above.
- Produces: the release.

- [ ] **Step 1: The recording test**

Create `tests/review_loop_guarantee.rs`: its own integration-test process, because it changes `PATH`, the working directory and the git environment policy, which `tests/env_policy.rs`'s rule reserves to one test per process (`cargo test` runs the tests of one file in one process; two such tests in `readonly_guarantee.rs` would race under the default thread count). Copy `real_git`, `git`, `tree_hash`, `ALLOWED` (ten entries: the review loop adds no subcommand) and the subcommand parser from `tests/readonly_guarantee.rs`, as `tests/hunk_actions.rs` did, then:

```rust
mod support;

#[test]
fn the_review_loop_talks_to_the_host_alone_and_writes_only_its_files() {
    // The one test of this file: it owns the process (PATH, cwd, init_process_env).
    use herdr_hunks::engine::comments::{Anchor, AnchorComparison, Category, Span};
    use herdr_hunks::engine::dispatch::{Accepted, CopyRequest, CopyWhat, ReviewScope, SendKind, SendRequest};
    use herdr_hunks::engine::host::{HerdrHost, SessionRef};
    use herdr_hunks::engine::nav::Side;
    use herdr_hunks::engine::{Target, TargetState};

    let original_cwd = std::env::current_dir().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    std::env::set_current_dir(scratch.path()).unwrap();
    let real = real_git();
    let repo = tempfile::tempdir().unwrap();
    let p = repo.path();
    git(&real, p, &["init", "-q", "-b", "main"]);
    git(&real, p, &["config", "user.email", "t@example.com"]);
    git(&real, p, &["config", "user.name", "t"]);
    std::fs::write(p.join("a.txt"), "one\n").unwrap();
    std::fs::write(p.join("b.txt"), "b\n").unwrap();
    git(&real, p, &["add", "-A"]);
    git(&real, p, &["commit", "-q", "-m", "init"]);
    std::fs::write(p.join("a.txt"), "ONE\n").unwrap();     // an unstaged row
    std::fs::write(p.join("b.txt"), "B\n").unwrap();
    git(&real, p, &["add", "b.txt"]);                     // a staged row
    std::fs::write(p.join("new.txt"), "n\n").unwrap();     // an untracked row
    let toplevel = p.canonicalize().unwrap().to_string_lossy().into_owned();

    // The recording wrapper, as in the first test (PATH is process-wide; the suite runs one thread).
    let bin = tempfile::tempdir().unwrap();
    let log = bin.path().join("git.log");
    let wrapper = bin.path().join("git");
    std::fs::write(&wrapper, format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexec '{}' \"$@\"\n", log.display(), real.display())).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::env::set_var("PATH", format!("{}:{}", bin.path().display(), std::env::var("PATH").unwrap()));
    init_process_env();
    let index_before = std::fs::read(p.join(".git/index")).unwrap();
    let refs_before = git(&real, p, &["for-each-ref"]);
    let tree_before = tree_hash(p);

    let fake_dir = tempfile::tempdir().unwrap();
    let fake = support::FakeHerdr::start(fake_dir.path());
    fake.set_panes(serde_json::json!([
        { "pane_id": "w1:p1", "cwd": toplevel },
        { "pane_id": "w1:p2", "agent": "codex", "agent_status": "idle", "cwd": toplevel,
          "agent_session": { "kind": "id", "value": "s-1" }, "terminal_title_stripped": "codex" }
    ]));
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let state = tempfile::tempdir().unwrap();
    let mut config = SessionConfig::production(p.to_path_buf());
    config.poll_interval = Duration::from_millis(100);
    config.state_dir = Some(state.path().to_path_buf());
    config.host = Some(Arc::new(HerdrHost::new(fake.socket_path.clone())));
    config.socket_path = Some(fake.socket_path.to_string_lossy().into_owned());
    config.nonce = Arc::new(|counter| format!("t{counter:05}"));
    let handle = spawn(rt.handle(), config);
    let recv = |what: &str, pred: &dyn Fn(&Snapshot) -> bool| -> Arc<Snapshot> {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut last = None;
        while Instant::now() < deadline {
            if let Ok(s) = handle.snapshots.recv_timeout(Duration::from_millis(200)) {
                if pred(&s) { return s; }
                last = Some(s);
            }
        }
        panic!("timed out waiting for {what}; last = {last:?}");
    };
    recv("rows", &|s| s.files.len() == 3);
    // The script of spec 10.6: pick, comment, copy, finish, request all changes (the copy before the
    // send, because a copy takes the pending comments and the send leaves none).
    handle.commands.send(Command::LoadPanes(1)).unwrap();
    let s = recv("panes", &|s| s.panes_seq == 1);
    assert_eq!(s.panes.as_ref().unwrap().len(), 1, "the shell pane is not a row");
    handle.commands.send(Command::SetTarget(Target::Pane {
        pane: "w1:p2".into(), socket: fake.socket_path.to_string_lossy().into_owned(), agent: "codex".into(),
        session: Some(SessionRef { kind: "id".into(), value: "s-1".into() }), title: "codex".into(),
    })).unwrap();
    recv("live", &|s| s.target_state == TargetState::Live("idle".into()));
    handle.commands.send(Command::AddComment {
        token: 1,
        anchor: Anchor { key: FileKey { path: "a.txt".into(), staged: false, untracked: false }, side: Side::Additions, line: 1, span: Span::Line, comparison: AnchorComparison::Worktree },
        category: Category::Bug,
        text: "shouting".into(),
    }).unwrap();
    recv("comment", &|s| s.comment_seq == 1 && s.comments.len() == 1);
    // `c` first, while the comment is pending: a copy claims nothing, and the file is the evidence.
    handle.commands.send(Command::Copy(CopyRequest { what: CopyWhat::Review })).unwrap();
    let s = recv("copied", &|s| s.copy_seq == 1);
    assert!(s.copy.as_ref().unwrap().notice.starts_with("copied 1 comments"), "{:?}", s.copy);
    assert!(std::fs::read_to_string(state.path().join("clipboard.md")).unwrap().starts_with("> Inline review — 1 item."));
    assert!(s.comments[0].is_pending(), "a copy stamps nothing");
    handle.commands.send(Command::Send(SendRequest { kind: SendKind::Feedback, accepted: Accepted::default() })).unwrap();
    let s = recv("sent, diff ready", &|s| s.send_seq == 1 && matches!(s.diff, DiffState::Ready(_)));
    assert!(s.send_error.is_none(), "{:?}", s.send_error);
    handle.commands.send(Command::Send(SendRequest { kind: SendKind::Review { scope: ReviewScope::All }, accepted: Accepted::default() })).unwrap();
    let s = recv("requested", &|s| s.send_seq == 2);
    assert!(s.send_error.is_none(), "{:?}", s.send_error);
    handle.commands.send(Command::Shutdown).unwrap();
    rt.shutdown_timeout(Duration::from_secs(5));

    // The host saw the three methods and nothing else; one prompt per confirmed send.
    let all_calls = fake.all_calls();
    assert!(!all_calls.is_empty());
    assert!(all_calls.iter().all(|c| matches!(c["method"].as_str(), Some("pane.get" | "pane.list" | "agent.prompt"))), "{all_calls:?}");
    let prompts = fake.calls_named("agent.prompt");
    assert_eq!(prompts.len(), 2);
    assert!(prompts[0]["params"]["text"].as_str().unwrap().starts_with("> Inline review — 1 item."));
    assert!(prompts[0]["params"]["text"].as_str().unwrap().contains(&format!("{toplevel}/a.txt:1 (additions) [unstaged]")));
    assert!(prompts[1]["params"]["text"].as_str().unwrap().starts_with("> Delegate a code review of these 3 changes:"));
    assert!(prompts.iter().all(|p| p["params"].get("wait").is_none()));
    fake.stop();

    // Every git call is one of the ten reads (`--version` among them); the request's diff loads too.
    let recorded = std::fs::read_to_string(&log).unwrap();
    for line in recorded.lines() {
        let sub = subcommand_of(line);
        assert!(ALLOWED.contains(&sub), "`{line}` runs {sub}, which is not allow-listed");
    }
    // The state directory holds what the script wrote and nothing else.
    let mut written: Vec<String> = std::fs::read_dir(state.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    written.sort();
    assert_eq!(written, ["clipboard.md", "comments.json", "requests.json", "send.lock", "split-panes.lock", "targets.json"]);
    assert_eq!(std::fs::read(p.join(".git/index")).unwrap(), index_before, ".git/index changed");
    assert_eq!(git(&real, p, &["for-each-ref"]), refs_before, "refs changed");
    assert_eq!(tree_hash(p), tree_before, "worktree changed");
    assert!(std::fs::read_dir(scratch.path()).unwrap().next().is_none(), "the viewer wrote to cwd");
    std::env::set_current_dir(original_cwd).unwrap();
}
```

`FakeHerdr::all_calls()` is a one-line addition to `tests/support/mod.rs` returning every recorded request. `subcommand_of(line)` is `readonly_guarantee.rs`'s subcommand parser copied as a function: it skips `-C <directory>` pairs and nothing else, so `--version`, which `SessionConfig::production` runs at start, is read as the subcommand it is. The file list is the one this script produces: no base is picked and no mark made, so `bases.json` and `marks.json` are absent. Spec 10.6 and 10.9 (test 10) list eight files for this script; amend both sentences in this task's commit to "the files the script writes: `targets.json`, `comments.json`, `requests.json`, `clipboard.md`, `send.lock` and `split-panes.lock`, and `bases.json`/`marks.json` only when a base or a mark was made" — the guarantee is "nothing else", which the assertion keeps exact.

- [ ] **Step 2: Run it**

Run: `CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" HOME="$(mktemp -d)" cargo test --locked --test review_loop_guarantee`
Expected: it passes in a few seconds (one `ENTER_MARGIN` per send); `--test readonly_guarantee` is unchanged and still green. Falsify once: make `dispatch::send_inner` skip `store.claim` and watch the assertion on the comment's state in the engine test of Task 5 fail — this test's own falsification is the file list: add a stray `std::fs::write(dir.join("x"), b"")` in `save_target` and watch `written` disagree.

- [ ] **Step 3: Tier B**

In `tests/e2e_real_herdr.rs`, after the hunk-action block (`staged hunk 1/1 of a.txt`), the review loop's slice through a real host with no agent pane:

```rust
        // Part 1 of the review loop (spec 10): no agent pane in this session, so the picker offers
        // the clipboard; a comment and a clipboard send, which writes clipboard.md under the isolated state.
        iso.herdr(&["pane", "send-text", viewer_id, "A"]);
        iso.herdr(&["pane", "wait-output", viewer_id, "--match", "No agent pane in this session", "--source", "visible", "--timeout", "15000"]);
        iso.send_enter(viewer_id);
        iso.herdr(&["pane", "wait-output", viewer_id, "--match", "→ clipboard", "--source", "visible", "--timeout", "15000"]);
        iso.herdr(&["pane", "send-text", viewer_id, "i"]);
        iso.herdr(&["pane", "wait-output", viewer_id, "--match", "comment on R", "--source", "visible", "--timeout", "15000"]);
        iso.herdr(&["pane", "send-text", viewer_id, "tier b"]);
        iso.send_enter(viewer_id);
        iso.herdr(&["pane", "wait-output", viewer_id, "--match", "Change · pending", "--source", "visible", "--timeout", "15000"]);
        iso.herdr(&["pane", "send-text", viewer_id, "Y"]);
        iso.herdr(&["pane", "wait-output", viewer_id, "--match", "to the clipboard?", "--source", "visible", "--timeout", "15000"]);
        iso.herdr(&["pane", "send-text", viewer_id, "Y"]);
        let clipboard = Path::new("plugins").join(plugin_id).join("clipboard.md");
        wait_for("clipboard.md holds the review", || {
            host_paths(&iso.state, &clipboard)
                .iter()
                .any(|p| std::fs::read_to_string(p).map(|t| t.starts_with("> Inline review — 1 item.")).unwrap_or(false))
        });
```

`send_enter` is a helper on the isolated-host struct that sends the Enter key through the host's key command (`herdr pane send-keys <id> enter` on herdr; check `herdr pane` for the exact name on each host and keep the fallback `send-text` with `"\r"` if the host accepts it). The plugin's state directory is not set by the test: the host runs the viewer with its own `HERDR_PLUGIN_STATE_DIR` under the isolated session's state root (`iso.state`), so the test finds the file exactly as it finds `split-panes.json` today: `host_paths(&iso.state, &Path::new("plugins").join(plugin_id).join("clipboard.md"))`, one directory level under `iso.state`, as that helper searches. The assertion is on the file, not the OSC sequence: whether the host forwards OSC 52 is the open question 10.4 names, and this is the first place it is tried by hand (acceptance row 10 records the answer).

Run as `docs/acceptance-p1.md` says, against both hosts. Expected: green on herdr 0.8.0 and vimeflow-terminal 0.8.0.

- [ ] **Step 4: READMEs**

`README.md`, after "Hunk actions":

```markdown
## Review loop

Press `A` to choose the agent pane a review goes to: the picker lists this
worktree's agent panes first, every other agent pane after, and a clipboard
row for a review you would rather paste yourself. The chip after the stats
shows the pane and what the host says about it (`→ codex w4:p2 · working`),
checked at every refresh; a pane that closed reads `gone`, an agent that was
restarted since you picked it `restarted`. The choice is remembered per
worktree. Without a host the chip reads `→ no host` and the clipboard still
works.

`i` writes a comment on the line under the cursor, `I` on the file, `v` then
`j`/`k` and `i` on a range; `ctrl+h`/`ctrl+l` pick the category (Question,
Change, Bug, Suggestion), `ctrl+j` adds a line, Enter saves, Esc discards.
A comment is a card under its line; `u`/`x` edit or delete the card on the
cursor line, `U`/`X` the file's. Comments are shared through `comments.json`
in the state directory, so two viewers on one worktree see the same cards,
and a comment whose line is gone moves to a section at the end of the body
where it can still be edited or dropped. At most 50 unsent comments per
worktree, 4,000 characters and 100 lines each.

`Y` sends every pending comment to the chosen pane as one message, Vimeflow's
inline-review prompt with a `[#n]` per item and a nonce the agent echoes; the
box refuses while the agent is at an approval, asks before sending to one
that is working, unknown or restarted, and `c` copies the text to the
clipboard and to `clipboard.md` instead. `@` asks the agent to delegate a
review of this file or of all changes. Replies are not read yet: that is the
next release.

The viewer talks to the host through three methods only, `pane.get`,
`pane.list` and `agent.prompt`, and runs no new git subcommand.
```

The keys table gains the rows for `A`, `i`, `I`, `v`, `u`, `U`, `x`, `X`, `Y`, `@` with the one-line descriptions above, the Esc row gains "the editor, the pane picker and the boxes", and the sentence under the table becomes `Phase 3 leaves \`/\` unbound.` The roadmap paragraph becomes `P1: read-only viewing. P2: stage/unstage/discard actions and defect fixes (0.0.4). P3: comments and agent dispatch (0.0.5). P4: replies and threads. P5: delegated review.` The first line's "Read-only git hunk viewer" sentence in `herdr-plugin.toml` is the description Task 9 rewrites (Step 6).

`README.zh-CN.md`, after "差异块操作":

```markdown
## 评审回路

按 `A` 选择评审要发送到的 agent 窗格：选择器先列出本工作树的 agent 窗格，再列出
其他 agent 窗格，最后是"剪贴板"一行，供你自行粘贴。统计信息后的标签显示该窗格及
宿主对它的描述（`→ codex w4:p2 · working`），每次刷新都会重新核对；窗格已关闭显示
`gone`，agent 在你选择之后重启过显示 `restarted`。选择按工作树记忆。没有宿主时标签
显示 `→ no host`，剪贴板仍可用。

`i` 对光标所在行写评论，`I` 对文件，`v` 加 `j`/`k` 再按 `i` 对一段范围；
`ctrl+h`/`ctrl+l` 选择类别（Question、Change、Bug、Suggestion），`ctrl+j` 换行，
Enter 保存，Esc 放弃。评论是其所在行下方的卡片；`u`/`x` 编辑或删除光标行的卡片，
`U`/`X` 作用于文件评论。评论通过状态目录中的 `comments.json` 共享，同一工作树的两个
查看器看到相同的卡片；所在行已消失的评论会移到正文末尾的区段，仍可编辑或删除。
每个工作树最多 50 条未发送评论，每条最多 4,000 字符、100 行。

`Y` 把所有待发送评论作为一条消息发给所选窗格，即 Vimeflow 的内联评审提示词，每项带
`[#n]` 和一个供 agent 回显的 nonce；agent 正在等待批准时对话框拒绝发送，agent 处于
working、unknown 或 restarted 状态时先询问；`c` 则把文本复制到剪贴板和 `clipboard.md`。
`@` 请求 agent 对当前文件或全部改动委派一次评审。回复的读取尚未实现：那是下一个版本。

查看器只通过三个方法与宿主通信：`pane.get`、`pane.list` 和 `agent.prompt`，并且不
运行任何新的 git 子命令。
```

`README.ja.md`, after "ハンク操作":

```markdown
## レビューループ

`A` でレビューの送り先となるエージェントペインを選びます。ピッカーはこのワークツリーの
エージェントペインを先に、他のエージェントペインをその後に、最後に自分で貼り付けるための
「クリップボード」行を並べます。統計の後のチップはそのペインとホストの報告
（`→ codex w4:p2 · working`）を示し、リフレッシュごとに確認し直します。閉じられたペインは
`gone`、選択後に再起動したエージェントは `restarted` と読めます。選択はワークツリーごとに
記憶されます。ホストがなければチップは `→ no host` と読み、クリップボードは使えます。

`i` はカーソル行、`I` はファイル、`v` と `j`/`k` の後の `i` は範囲にコメントを書きます。
`ctrl+h`/`ctrl+l` でカテゴリ（Question、Change、Bug、Suggestion）、`ctrl+j` で改行、
Enter で保存、Esc で破棄。コメントはその行の下のカードで、`u`/`x` はカーソル行のカードを
編集・削除し、`U`/`X` はファイルのコメントを扱います。コメントは状態ディレクトリの
`comments.json` で共有され、同じワークツリーの二つのビューアは同じカードを見ます。行が
消えたコメントは本文末尾のセクションに移り、そこでも編集・削除できます。ワークツリーごとに
未送信 50 件、各 4,000 文字・100 行まで。

`Y` は保留中のコメントをすべて一つのメッセージとして選んだペインに送ります。Vimeflow の
インラインレビュープロンプトで、項目ごとの `[#n]` とエージェントが返す nonce を持ちます。
エージェントが承認待ちならダイアログは拒み、working・unknown・restarted なら先に確認します。
`c` は代わりにテキストをクリップボードと `clipboard.md` にコピーします。`@` はこのファイル
または全変更のレビューをエージェントに委ねるよう頼みます。返信の読み取りはまだです。それは
次のリリースです。

ビューアがホストと話す方法は `pane.get`、`pane.list`、`agent.prompt` の三つだけで、新しい
git サブコマンドは実行しません。
```

Mirror the keys-table rows and the roadmap line in both.

- [ ] **Step 5: AGENTS.md, PORT-SURFACE.md, the roadmap note, acceptance**

`AGENTS.md`: the first rule becomes `- Phases 1-3 ship a viewer whose only git mutations are the confirmed hunk actions of spec 9 and whose only host traffic is the three methods of spec 10 (\`pane.get\`, \`pane.list\`, \`agent.prompt\`). Reading agent replies is spec 11; keep it out until then.`; the reserved-key line becomes `- \`/\` remains unbound. \`b\` and \`B\` are the scope keys; \`s\`, \`d\` and \`D\` are the Phase 2 write actions, bound in \`worktree\` scope only and, in \`branch\` scope, showing \`switch to worktree scope (b) to stage or discard\`; \`A i I u U x X v y Y @ c\` are the review loop's (spec 10).`; the architecture's "State on disk" paragraph lists `targets.json`, `comments.json`, `requests.json`, `clipboard.md` and `send.lock` beside the three, naming `send.lock` as the second lock (`dispatch::SendLock`); "Where decisions are recorded" reads `1-6 the roadmap and Phase 1, 7 branch scope, 8 review marks, 9 hunk actions, 10 the review loop`; "The viewer" gains one line under the guarantees: `**The host is three methods behind a trait.** \`engine::host::HostClient\`; tests inject \`host::Scripted\`, the read-only test a fake socket.`

`PORT-SURFACE.md`: a short "Carried prompt" paragraph after the patches: `src/engine/prompts/delegated-review.md` is the pin's `src/features/diff/prompts/delegated-review.prompt.md`, byte for byte, compared by `scripts/port-check.sh`; it is not part of `src/git/`.

The P1 spec's 1.2 table gets one line under it: `Order amended 2026-09-30: Phase 2 (section 9, 0.0.4) shipped before Phase 3 (section 10, 0.0.5); the loop's second half is section 11.`

`docs/acceptance-p1.md` gains row 10 with `pending` in the status column and `Status: PENDING` on the first line until the orchestrator runs it:

```markdown
| 10. Review loop, part 1 | In a herdr session with a codex pane beside the viewer: `A` and pick it; `i` on a changed line, a Bug, Enter; `Y`, `Y`; read the codex pane. Then, for each gate, write a fresh comment first (`i`, text, Enter), because a send leaves nothing pending: close the codex pane, `Y` (the picker opens; `A` is not needed), pick the pane codex is restarted in next; start codex again in that pane, `i`, text, Enter, `Y` (the box asks), `Y` again; put codex at an approval, `i`, text, Enter, `Y` (refused), answer the approval, `Y`. Open a second viewer on the worktree. `@`, `a`, `Y`. In a plain terminal: `herdr-hunks .`, `A`, Enter (the clipboard row), `i`, text, Enter, `Y`, `Y`. | The pane is listed first and the chip reads `→ codex w4:p2`; the editor opens without asking for a target; one pasted message arrives in the codex pane as the 10.5 prompt with Enter pressed, codex answers it, the viewer shows `sent 1 item to codex · w4:p2` and the card reads `Bug · sent`; the chip reads `· gone` within one refresh and `Y` opens the picker; `· restarted` and the box asks before sending; `· blocked` and the box refuses until the approval is answered; the second viewer shows the same cards and counts; the request names every changed file with its `git -C` command and codex's reply ends with a `VIMEFLOW_REVIEW` block whose nonce is in `requests.json`; outside a host the chip reads `→ no host`, the picker offers the clipboard, and `Y` fills the clipboard and `clipboard.md`. | pending | | | |
```

- [ ] **Step 6: Version 0.0.5**

`Cargo.toml` `version = "0.0.5"` with `description = "Git hunk viewer with staging, discarding, review comments and a send to the agent pane of your choice."`; `cargo build --release` rewrites `Cargo.lock`'s own entry; `herdr-plugin.toml` `version = "0.0.5"` and the same description. `tests/version_sync.rs` keeps the three together.

- [ ] **Step 7: Gates and commit**

The full gate line, plus Tier B against both hosts, plus `cargo build --release` for the acceptance run.

```bash
git add tests/review_loop_guarantee.rs tests/support/mod.rs tests/e2e_real_herdr.rs README.md README.zh-CN.md README.ja.md AGENTS.md PORT-SURFACE.md docs/superpowers/specs/2026-09-18-hunks-roadmap-p1-viewer-design.md docs/superpowers/specs/2026-09-30-agent-review-loop-design.md docs/acceptance-p1.md Cargo.toml Cargo.lock herdr-plugin.toml
git commit -m "feat: the review loop, part 1, documented and versioned 0.0.5"
```

The acceptance row's evidence and `Status: PASS` are a separate commit after the hand run (`docs(acceptance): row 10 for 0.0.5`), as row 9 was.

---

## Self-review against the spec

- **10.1** is prose; nothing to build.
- **10.2 "The record"**: Task 2 (`Target`, `TargetState`, `compare`, the session rule, the two-missing-references rule in `every_state_of_the_table_is_reached`). **"How it is chosen"**: Task 2 (`resolve_target`, `a_remembered_target_beats_the_opener_and_the_opener_is_not_written`, the opener written on the first comment in Task 3's `Done::Comment`). **"The picker"**: Task 6 (`PanePicker`, the groups, the filter, the no-agent line, the clipboard row never filtered, `A`), Task 2 (`rows`, own pane and shells excluded, `a_sibling_directory_with_a_shared_prefix_is_another_pane`). **"The chip"**: Task 6 (`chip`, every state, the drop order). **"Re-verification"**: Task 2 (the check in `run_job`, `NoHost` on the three conditions, keep-previous on others, the selection generation in `a_check_answered_after_a_repick_is_dropped`). **"What is remembered"**: Task 2 (`load_targets`, `save_target`, `records_are_checked_by_shape_on_load`, socket compared never opened).
- **10.3 "The record"**: Task 3 (`Comment`, `CommentState` with `to` and `before`, `new_id`). **"The anchor"**: Task 3 (`Anchor`, `Span`, `AnchorComparison` with the label), Task 7 (`attach_row`, `in_place`, drawn only under its comparison kind and half). **"Cards"**: Task 7 (`cards::lines`, tones, `· sent` dim, the orphan section, cards as rows, hits). **"The editor"**: Task 7 (`Editor`, the title, `ctrl+h/l/j`, Enter/Esc, the limit title, the splice). **"Keys"**: Task 7 (`comment_key`, `v`, `u`/`U`/`x`/`X`, the notices). **"The target comes first"**: Task 7 (`i_without_a_target_opens_the_picker_and_keeps_the_anchor`). **"Marks and counts"**: Task 7 (`✎`, `CHANGED 3 · ✎ 2`), Task 8 (`Y finish (2)`), Task 3 (the cap over unsent, checked by adds alone). **"Remembered"**: Task 3 (transactions, shape checks, the journal and its conflict rule, the expiry on load and on refresh, the mtime reread, no state directory).
- **10.4 "Finish"**: Task 8 (`ReviewBox::finish`, the table in `the_finish_box_says_what_the_table_says_for_each_state`, `Esc` is `n`, `A` over the box, the counts). **"The send"**: Task 5 (the six steps, `gates`, `Accepted`, the generation check twice, `fresh_nonce`, `claim`, the bound, the call without `wait`, `settle` three ways with `before`, `send_seq`/`send_error`/`send_outcome`), Task 8 (`send anyway` relabel with the typed `Refusal`). The send lock: Task 5 (`SendLock`, `two_viewers_sending_to_one_pane_take_the_send_lock_in_turn`, `NOTICE_WAITING`). The unconfirmed card and the next Finish: Tasks 3 and 5. **"Copy"**: Task 5 (`copy_out`, `osc52`, `clipboard.md`, the three settlements of a clipboard send, `c` claims nothing), Task 8 (`c` in both boxes, `y` on a selection, the OSC write in the shell). **"Request review"**: Task 5 (`request_files` reads first, ranges, `requests.json`, the selected row loaded like the others, `c` records with a clipboard target), Task 8 (the box, `f`/`a`, the three shapes of the scope line, `nothing to review`).
- **10.5**: Task 4 (`review`, `request`, `strip_controls`, `quote_toplevel`, `comparison_label`, the merge-base footer only with a branch item, the pinned file and `port-check.sh`, `nonce`).
- **10.6 "Commands and snapshot"**: Tasks 2, 3, 5 (every field in `fingerprint`). **"The host client"**: Task 1 (the deadline, the phases, `HostClient`, `HerdrHost`, `from_env`, `ENGINE_WAIT`). **"Where the work runs"**: Task 2 (the check in the refresh job), Task 5 (`Send`/`Copy` as their own tasks, one send at a time, `spawn_blocking` for the store and the host). **"The store module"**: Tasks 3 and 4 (`comments`, `target`, `prompt`; no git in them; the request's loads through the existing loaders). **"The view"**: Task 7 (`Row::Card`, the orphan section, `row_of_target`, the rebuild rule on five parts), Task 8 (the boxes as `dialog::Panel`s, `RESERVED = ["/"]`, the key sheet rows). **"The read-only guarantee"**: Task 9 (the recording test with the fake socket, in its own process beside `readonly_guarantee.rs`). **"Standalone"**: Task 2 (`NoHost`), Task 6 (`→ no host`), Task 9 (Tier B's clipboard send).
- **10.7**: the table's rows map to: `NoHost` conditions (Task 1 `classify`, Task 2 `compare`), `pane.get` cannot run (Task 2), gone/left/restarted/blocked (Tasks 2, 5, 8), `pane.list` failure (Task 2, Task 6's `Row::Warn`), `targets.json` unreadable or unwritable (Task 2), `comments.json`/`requests.json` malformed (Task 3), a failed transaction and the journal (Task 3, Task 5's refusal to claim), the host's refusals and the timeout (Task 5), the viewer dying mid-send (Task 3's expiry), the second `Y` (Task 5), the 51st unsent (Task 3), the caps (Tasks 3 and 7), the 512 KiB bound (Task 5), an unloadable file in a request (Task 5), OSC 52 dropped (Task 5's file-first settlement), the moved base (Task 3's anchors keep their merge-base; Task 4 prints it), a row that left the list (Task 7's orphans), the send-time check that cannot run and the changed target (Task 5), the send lock (Task 5), the failed retry (Task 5), a hidden condition behind an accepted one (Task 5), a journaled edit meeting a changed record (Task 3), the small popup (`view.rs`'s existing size guard, Task 7 adds nothing of this section below 40×10). Cost and configuration: README (Task 9). Version: Task 9.
- **10.8**: boundaries honoured by construction: no key of section 9 is touched; `to` and `requests.json` carry what part 2 needs (Tasks 3, 5); nothing here reads a pane or a transcript.
- **10.9 tests 1-11**: 1 Task 2; 2 Task 2 and Task 6; 3 Task 3; 4 Task 5; 5 Task 5; 6 Task 4; 7 Task 1; 8 Tasks 6, 7, 8; 9 Tasks 6, 7, 8; 10 Task 9; 11 Task 9. Criteria 12-17: acceptance row 10 (Task 9).

Placeholder scan: none. Type consistency: `HostFailure`, `PaneRecord`, `SessionRef`, `Target`, `TargetState`, `Destination`, `Comment`, `Anchor`, `Span`, `AnchorComparison`, `Category`, `CommentState`, `Stamp`, `Operation`, `Store`, `Claimed`, `Settlement`, `RequestRecord`, `RequestFile`, `StoreOp`, `SendRequest`, `Accepted`, `SendKind`, `ReviewScope`, `CopyRequest`, `CopyWhat`, `CopyOut`, `SendOutcome`, `Finished`, `Refusal`, `SendLock`, `PanePicker`, `Choice`, `ReturnTo`, `CardLine`, `EditorPlace`, `EditorAnchor`, `Editor`, `Visual`, `ReviewBox`, `Offers` are each defined once and used by those names throughout; `call_host` is defined in Task 2 and moved to `dispatch` in Task 5, which the Task 5 text says; `NOTICE_NOTHING` is `comments.rs`'s and re-exported by `dispatch`. Three deliberate simplifications are recorded above: the Finish box names the pane rather than its directory (the target record carries none), a `Y` on a box opened on `Restarted` costs one extra press (Task 8), and the chip's clickable style takes a tone override rather than a new span kind (Task 6).

## Review notes

Appended after each review round.

**Round 1 (codex, plan-complete, 2026-10-04).** Twenty-two findings, all applied. Four
were compile-order or interface mismatches (an `is_under` closure with mismatched
lifetimes, `rows::build` taking `&[Comment]` for a `Vec<&Comment>`, `Row` variants added
before `body_line`'s exhaustive match, a notice constant used before its module existed);
the wire shape of a comment gained `#[serde(flatten)]` on `state` so the fields the test
asserts are where the spec puts them. Five changed contracts: `claim` and `settle` run in
memory without a state directory, as 10.3 requires; the session rule now reads a known
session that the host stops reporting as `Restarted`, and a record without one adopts the
session it first sees (`adopted_session`, written with the record's next write); the nonce
is chosen inside the claim transaction and inside `record_request_fresh`, under the state
lock, instead of before it; `clipboard.md` is replaced under the same lock through a
counter-named temporary file; and the `Accepted` of a box accumulates across refusals, so
a restarted and working agent asks twice and never for ever. Two race windows closed: the
first refresh's resolution and a send's session adoption apply only under the selection
generation they started for. Three recovery paths were added: an add the cap refuses on
replay stays visible and journaled, unconfirmed records are editable and deletable
(`is_editable`), and an orphan-only view has an entry path (`j` from nothing) with the
editor spliced after the orphan's card. The card wrapper is lossless and sanitises per
line; visual-mode movement stays on one side (`step_on_side`); the editor's category words
are hits; the 51st comment is refused before the editor; review-request ranges are read
from the parsed file before the display cap. The store has one owner at a time through a
queue (`store_opened` distinguishes "borrowed" from "absent"), every store and file
operation of a send runs on the blocking pool, and copies read the published comments.
Test fixtures were corrected: the key helper learned `Enter`, `Esc`, `Backspace` and the
arrows; claim and settlement tests use timestamps near their store's `now`; the
control-character test expects printable text to stay; the nonce counter flows back from
the task so successive sends are `n00001`, `n00002`, …; the recording test copies before it
sends; `cargo test` runs one filter per command and keeps `CARGO_HOME`/`RUSTUP_HOME`. One
operational note: the planner's `codex-review.sh` passes the prompt as a single argument,
which a 400 KB plan exceeds (`Argument list too long`); the review ran with the same
prompt file on stdin (`codex exec … - < plan-complete-prompt.md`).

**Round 2 (codex, plan-complete, 2026-10-04).** Seventeen findings, all applied. Three
checkpoint repairs: `Refusal` and the typed gate error move into Task 5 so its code
compiles at its own gate (Task 8 only publishes the field); `write_target` is defined in
Task 3, where the first comment needs it; each new module is registered before its first
red run. The claim and the settlement work on a copy and publish only what the file holds;
both take a `guard` that the send task uses to re-check the selection generation under the
state lock, after every wait, so a queued send, which now keeps the `Context` of the moment
it was confirmed, never adopts a replacement target. `record_request_fresh` rereads both
files under the lock instead of taking a captured array. Target writes are guarded by the
selection generation under the lock (`save_target_if`), with a regression for a delayed
adoption followed by a re-pick. An edit or delete carries the record as the viewer showed
it (`EditComment { seen, .. }`, `DeleteComment { seen }`), so an edit saved elsewhere while
the editor was open is refused rather than overwritten. A host answer that arrives after
the engine's wait settles the records by nonce (`call_host_late`, `Done::LateAnswer`,
`settle` accepting `Unconfirmed`), with `host_wait` a seam so the test runs in a second;
`ENGINE_WAIT` stays the production value. The review bound is tested with fifty 4,000-CJK
comments (600,000 bytes) and request texts are measured with their final nonce. Test
fixtures: `review_setup` lists the diff's file, numbers its lines and sets a target; the
claim tests stamp the other viewer's record a second ago, add a record before exercising
the bound, and inject write failures; the target test combines its waits into one
predicate and polls counters with `wait_until`; the lock test measures the gap between the
two call starts. Cards are wrapped and framed at one width (`cards::card_width`), the
editor scrolls itself into view and keeps its caret visible, a visual selection is bound to
its `Arc<LoadedDiff>` and dropped with it, and the acceptance row writes a fresh comment
before each gate it exercises.

**Round 3 (codex, plan-complete, 2026-10-04).** Seventeen findings, all applied. Three
were data-loss paths my earlier rounds had opened: `read_and_apply` now sets `others`, so
a claim or settlement writes every worktree's records back (tested across two worktrees);
`Unconfirmed` keeps the `before` stamps through the timeout, so a late definite failure
restores the earlier correlation instead of `Pending`; and a prompt call that outlives the
engine's wait carries the send lock with it, released after the host answers plus the
Enter margin (tested with two viewers and a delayed late answer). `Accepted.restarted`
became `Option<Option<SessionRef>>`, so a restart into an agent that reports no session is
acceptable as that absence. Nonces are checked against every worktree's records in both
files, and claims are numbered by creation time. The store opens on the blocking pool
(`Done::StoreOpened`), the editor's rows are real rows (`Row::Editor`, rebuilt only when
the editor opens, closes, moves or grows), the mouse is modal while editing, and Tier B
finds `clipboard.md` through `host_paths`. Compilation and fixture repairs: the nineteen
`SessionConfig` literals of the session tests take struct-update defaults; the test
helper is `test_config` so locals named `config` do not shadow it; a shadowed `rows`
local, a duplicated target extraction and an obsolete `record_request_fresh` call were
removed; `record_request_fresh`, `remove_request`, `write_clipboard` and the
delayed-adoption test moved to Task 3, where their dependencies are; the settlement test
no longer re-claims a sent record; the request test expects the `-U3` range `(1, 2)`; the
editor tests expect the fixture's first changed row, `L11`; the recording test reuses the
first test's subcommand parser; waits that could consume each other's snapshot were
combined; and the clipboard send is tested over the OSC limit with and without a writable
directory.

**Round 4 (codex, plan-complete, 2026-10-04).** Thirteen findings, all applied. Two rules
settled: an edit of an unconfirmed comment makes it pending again, so a late success for the
old nonce cannot mark text the host never saw as sent (the new text goes out under a new
nonce), and cards carry the anchor's own target, with `u`/`x` matching by that target, so in
split mode a row that shows both sides never routes a click or a key to the other side's
comment (`anchor_target`; a split-mode test). A comment submitted while the store is still
opening is queued, not refused (`store_opening`; a test holds the state lock across the
start). `Snapshot.send_refusal` is Task 5's, where its test reads it; the sessionless-restart
test asserts `s2` before accepting the absence and then asserts the absence was adopted;
the rows test binds its array before borrowing from it; Task 8's fixture lists its file; Tier
B finds `clipboard.md` the way it finds `split-panes.json`; review requests look worktree
renames up by side; the clipboard regression keeps the snapshot its wait returned; the
adoption-write regression makes its race by holding the state lock; one line rule
(`line_count`, `within_caps`) serves the editor, the store and the tests; and the wrapper
keeps every character, spaces and indentation included.

**Round 5 (codex, plan-complete, 2026-10-04).** Fourteen findings, all applied. Four were
HIGH: the new test helpers collided with the session module's existing `start_with` and
`wait_until` (now `start_from`, `wait_cond`) and the review `Target` with `nav::Target` in
`types.rs` (written by path there); Task 3's edited-unconfirmed test called Task 5's `settle`
(the settle half moved to Task 5); `HerdrHost::agent_prompt` took any JSON without an
`error` as success (it now requires the documented result envelope and treats anything
else as `After`, with a raw-reply control on the fake socket to prove it); and the request
file's read errors were swallowed into an empty map (a missing file is empty, an unreadable
one aborts the write). Then: a failed expiry write on load keeps the loaded comments on
screen; the in-memory store refuses stale edits and deletions as the persisted one does; a
range card anchors to its last line exactly and never slides; a visual selection is drawn in
reverse over its lines; the Finish and Request boxes are modal for the mouse; the
adoption-write regression forces its race through a `target_write_gate` seam and two
counters; the competing-send test starts the second viewer with an hour's poll so its stale
snapshot reaches the claim; the client gains a write-side deadline test (a server that never
reads a four-megabyte request) and the fake's silent mode holds the stream open past the
deadline; the recording test moves to its own integration file (`review_loop_guarantee.rs`),
since it changes process-wide state; and `request_files` is unit-tested on a `Failed`
selected row, an unreadable row and a branch-scope snapshot.

**Round 6 (codex, plan-complete, 2026-10-05).** Eleven findings, all applied. Three were
HIGH: `Command::SetTarget` still named the review `Target` unqualified where `types.rs`
imports `nav::Target` (now by path, like the snapshot field); a failed write in
`read_and_apply` replaced the comments with the file's and so hid an earlier journaled add
behind a later failure (the journal is reapplied after the reload); and the failed-expiry
fixture seeded a claim already expired at the time of its own transaction (seeded at 950,
reopened at 1,100). Then: `compare` rejects a `pane.get` reply that names another pane (no
state, so the send refuses "could not verify … the host answered about <pane>" and adopts
nothing; the fake gained `answer_pane_get_with`); `HerdrHost::agent_prompt` requires
`result.type == "agent_prompted"`, with `pong` as the control; `nonces_in_use` and
`record_request_fresh` return read errors instead of an empty set, tested with a directory in
the file's place; the request text and its bound are built inside the reservation (`build`
returns the record and the text, or a refusal that writes nothing), and a session test puts a
hundred deep untracked files past the bound and asserts `requests.json` unchanged; the Finish
and Request boxes compute their offers from the current state table first and a refusal only
replaces the line and relabels a plain send, so a blocked or hostless target after a refusal
withdraws Y (transition tests added); the pane picker's hits come from the rendered layout
(`panel_line` counts wrapped notes through `dialog::line_count`, `None` for an undrawn
choice) and the empty-host picker is tested at 44 columns; the rename regression commits
twenty common lines first and asserts `RM b.txt -> c.txt` before requesting ranges; and the
remaining synchronous filesystem work (`nonces_in_use` in `copy`, the send lock's first
`try_take`) moved onto the blocking pool.

**Round 7 (codex, plan-complete, 2026-10-05).** Twelve findings, all applied. Six were HIGH:
`RequestLine` and `Claimed` lacked the derives Task 5's code and tests use (`Clone`, `Debug`);
a `c` copy of the review propagated a read error from `nonces_in_use` where 10.7 promises that
`c` still copies (it now falls back to the nonces in memory, tested with an unreadable
`comments.json`); a re-pick that landed between the transaction and the host call was never
met (the generation is checked once more after the transaction, a failed claim is settled
back and a request's record removed, through a `send_gate` seam that holds the send there
while the test re-picks); a clipboard copy whose settlement failed lost its OSC sequence
(`Finished.copy` carries it, the session publishes it with the error, and the TUI treats any
copy published with a send's answer as spoken for); the rename regression had a pure rename
on its staged side (it now stages an edit with the rename and asserts `RM b.txt -> c.txt`);
and the post-check test waited for the send and the blocked chip in two steps (one predicate).
Then: a session is learned only from a reply `compare` accepted as this pane alive, with a
session test that watches every published snapshot while the host answers about another pane;
a status outside the documented five asks like `unknown` does in the gate and the box
(`sleeping` tested in both); a save keeps its draft until the answer (`Editor.pending`,
`Snapshot.comment_refused`, `NOTICE_NOT_REMEMBERED` to tell a journaled operation from a
refused one), with tests for a cap refusal, a journaled save and a changed record; the claim
samples its time at the claim (`Context.clock`), not at the command; `from_env`'s test moves to
`tests/host_env.rs`, alone in its process; and an empty Request box offers `n` alone, footer
and keys both asserted.

**Round 8 (codex, plan-complete, 2026-10-05).** Ten findings, all applied. Four were HIGH:
two leftover compile defects (`ctx.now` in `copy` after the clock change; a malformed
`Unconfirmed` initializer in an input test); a pending save was matched by `comment_seq`
alone, so another save's answer or a refresh's notice could close the wrong editor (an add or
edit now carries a `token` the answer echoes as `Snapshot.comment_token`, and the editor
waits for its own; a test submits, presses Esc, submits a second editor and answers the first);
the request file was decoded as one map of arrays, so one malformed worktree entry made the
next write drop every other worktree's records (entries are raw values now, decoded one
worktree at a time, written back as read; tested with a string beside a valid neighbour); and
the opener's `pane.get` reply was preselected whatever pane it named (it must name the
opener; tested with `answer_pane_get_with`). Then: a failing store applies the rules to the
array in memory before journaling, so a cap or a changed record is refused rather than
"remembered"; the clipboard request arm takes the same post-reservation generation check as
the pane path, with its own send-gate test; the key sheet's row counts are pinned task by task
and the wheel test's scroll arithmetic moves to `dialog::line_count` once the notes wrap;
`new_id` puts a strictly increasing time in its first sixteen hex characters so same-second
comments claim in creation order (tested); a pick from a file-scoped Request box returns to a
file-scoped box (`ReturnTo::Request(ReviewScope)`); and `tests/host_env.rs` is in Task 1's
commit.

**Round 9 (codex, plan-complete, 2026-10-05).** Seven findings, all applied. Three were HIGH:
two tests reached forward across tasks (Task 2's opener test now publishes through its own
`LoadPanes`; the same-second claim order moved into Task 5's claim test, Task 3 keeping the id
shape and sort); the picker-return handler moved a `ReviewScope` out of a borrowed picker (it
matches by reference); and the failure path of `read_and_apply` re-applied the journal through
the cap, so a journaled add vanished behind a failed edit on a full file (`show_journaled_adds`
overlays it on both paths; tested with fifty of another viewer's and a failing edit). Then: the
picker that `observe` opens for a target gone under an open box asks for its rows through
`ViewState.pending_command`, drained by the shell after `observe` (code and test); a draft is
drawn whatever the diff does (`orphans_only` splices the open editor, `render` draws the rows,
and a test turns the last file clean under a half-typed comment); `@` on a clean repository
without a target shows `nothing to review` with `n` alone instead of opening the picker; and
`check_request` applies the comment record's path and comparison rules plus one-based ordered
ranges, while a request file that is no object reads as empty with a reported problem (seven
malformed records and a malformed file tested).

**Round 10 (codex, plan-complete, 2026-10-05).** Five findings, all applied. Two were HIGH:
a retry kept only the one stamp it replaced, so a third attempt's failure followed by the
second's late failure returned a record to `Pending` while the first send may have arrived
(`before` is now the chain of uncertain stamps, newest first; a definite failure restores its
head with the rest, a late success for any stamp in it settles the record as sent under that
stamp; the sequence is tested); and the existing toolbar chip test could not pass with a dim
clickable target chip (the test is named with its two changes: eleven chips, and the
reverse-accent assertion excluding the `PickPane` span, which is asserted dim on its own).
Then: an answer that displaces an unread `Other` warning now queues it (`deferred`, spoken when
the answer is read; tests in Tasks 6 and 7 with two answers before a key); `u` and `x` break a
same-second tie by id, as the claim orders; and the oversized-request fixture uses 350 files
on a 750-character path, under Darwin's 1,024-byte limit.

**Round 11 (codex, plan-complete, 2026-10-05).** Six findings, all applied. Two were HIGH:
two tests added in round 10 reached forward again (the retry-chain test moved to Task 5's
claim tests, the two-answers test to Task 8), and `settle` ignored a late word about an attempt
behind a newer one (a late success for a stamp in the chain now settles the record as sent
under it whether the newer attempt is sending or unconfirmed, a late failure drops the attempt
from the chain so a later rollback cannot restore it; both tested with the retry still
sending). Then: one `nonces_of` serves the file-backed and the in-memory nonce checks, so a
chain's stamps count everywhere (tested without a state directory); a deferred warning speaks
only into an empty notice slot, never over an unread plain notice (both tests observe an
unrelated snapshot in between); `orphans_only` wraps at `cards::card_width`, the width the cards
are drawn at (a 300-character orphan tested whole in both modes); and a write that drops
unusable records logs it first, in `comments.json`'s rewrite and in `requests.json`'s, with a
test that plants an unknown-state record after the store opened.

**Round 12 (codex, plan-complete, 2026-10-05).** Six findings, all applied. Two were HIGH:
`selection_text` moved the frozen `DiffLineType` out of a borrowed line (it matches by
reference), and the retry-chain test's first add on a store without a state directory expected
`Ok` where Task 3 answers `comments not remembered: no state directory` (asserted, with the
record kept). Then: every target write, a pick's or a comment's, answers `Done::Target {
generation, written }`, the handler drops an answer for an older selection and, on failure,
publishes the notice of 10.7 and sets `target_write_pending` again so the next comment retries
(two tests: an opener write failing and recovering, and two picks' writes released in order
through the shared gate); the editor's anchor attaches only to its own file's diff under the
same comparison kind, else to the end (a refresh that replaces a.rs with b.rs under an open
draft is tested, and the save still names a.rs); and a request or its copy without a state
directory takes its nonce through the same collision loop as everything else (`fresh_nonce`
over `nonces_of`, tested with a maker that repeats a stamped nonce).
