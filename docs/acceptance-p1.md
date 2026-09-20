Status: PENDING

# Phase 1 release acceptance

This is the release gate for the five success criteria in
[spec section 1.5](superpowers/specs/2026-09-18-hunks-roadmap-p1-viewer-design.md#15-phase-1-success-criteria).
The orchestrator and user perform and record these checks. Automated test results
alone do not complete manual acceptance. Phase 1 is not release-ready while any
result is PENDING.

| criterion | how to check | expected | result | date | herdr version | host |
| --- | --- | --- | --- | --- | --- | --- |
| 1. File list and hunk parity | Run `cargo test git_diff_response`. Cover staged, unstaged, partially staged (`MM`, `AM`), untracked including a new nested directory, deleted, and renamed files. Compare the list with `git status --porcelain=v1 --untracked-files=all`, tracked hunks with `git diff` / `git diff --cached`, and untracked hunks with `git diff --no-index -- /dev/null <path>`. Account for the documented K5 limitation. | All fixtures pass; list and hunks agree with git within the documented limitation. | PENDING | PENDING | PENDING | PENDING |
| 2. Linked worktree context | From an agent pane inside a linked worktree, invoke `open`; inspect the toolbar file list and diff. | The viewer shows that worktree's changes, not the main checkout's changes. | PENDING | PENDING | PENDING | PENDING |
| 3. Live refresh and degraded mode | Have an agent edit the same file twice and observe without pressing refresh. Repeat on an isolated Linux test host with `fs.inotify.max_user_watches` exhausted before opening; restore the test host's resources afterward. | Both edits appear without input; the notification failure shows a degraded-mode notice and polling still converges on the second edit. | PENDING | PENDING | PENDING | PENDING |
| 4. Same build in both hosts | Build once; link that same build into upstream Herdr 0.8.0 and the fork using its `vimeflow` binary. Invoke `open` from each host's separate plugin registry. Record both host versions. | The same binary opens and works in both hosts. | PENDING | PENDING | PENDING | PENDING |
| 5. Read-only guarantee | Run `cargo test --test readonly_guarantee`. | Only allow-listed git reads occur, required environment policy is present, and the index, refs, and worktree remain unchanged. | PENDING | PENDING | PENDING | PENDING |

Only the orchestrator/user may replace result cells with `pass`, fill in the date,
version, and host evidence, and change the first line to `Status: PASS` after all
five rows pass. The release workflow's guard refuses to build or publish without
that PASS line. Record both upstream and fork observations for criteria 2–4;
the inotify exhaustion check specifically requires a Linux test host.

## Automated Tier B check

This ignored test starts and stops its own named server under a short `/tmp`
directory, with separate HOME, XDG_CONFIG_HOME, and XDG_STATE_HOME. Every child
starts with a cleared environment; host commands select the isolated session via
`--session`. It links the plugin, verifies the viewer cwd, invokes `open-split`
twice from the opener, and checks pane reuse and the saved reuse record. It never
sets the test process's HERDR_SOCKET_PATH.

Build first: `plugin link` skips build steps and uses `target/release/herdr-hunks`.
Set HERDR_BIN_PATH to the absolute path of the host binary. Set
HERDR_E2E_REAL_SESSION to the real user's session.json before overriding HOME;
this keeps the before/after mtime assertion meaningful. If omitted, the test
derives the path from the inherited socket or configuration directory.

```sh
cargo build --release
target/release/herdr-hunks --version
HERDR_BIN_PATH=/absolute/path/to/host \
HERDR_E2E_REAL_SESSION=/absolute/path/to/real/session.json \
cargo test --test e2e_real_herdr -- --ignored
```

All cargo tests need a writable HOME outside a git checkout. When overriding it,
preserve CARGO_HOME and RUSTUP_HOME for the installed Rust toolchain. If the
sandbox prevents starting the isolated host, record the failure and rerun outside
that sandbox; never redirect the test to a live user session.
