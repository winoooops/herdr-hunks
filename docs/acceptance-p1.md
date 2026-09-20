Status: PENDING

# Phase 1 release acceptance

This is the release gate for the five success criteria in
[spec section 1.5](superpowers/specs/2026-09-18-hunks-roadmap-p1-viewer-design.md#15-phase-1-success-criteria).
The orchestrator and user perform and record these checks. Automated test results
alone do not complete manual acceptance. Phase 1 is not release-ready while any
result is PENDING.

| criterion | how to check | expected | result | date | herdr version | host |
| --- | --- | --- | --- | --- | --- | --- |
| 1. File list and hunk parity | Run `cargo test git_diff_response`. Cover staged, unstaged, partially staged (`MM`, `AM`), untracked including a new nested directory, deleted, and renamed files. Compare the list with `git status --porcelain=v1 --untracked-files=all`, tracked hunks with `git diff` / `git diff --cached`, and untracked hunks with `git diff --no-index -- /dev/null <path>`. Account for the documented K5 limitation. | All fixtures pass; list and hunks agree with git within the documented limitation. | pass | 2026-09-20 | n/a (no host involved; git 2.55.0) | Linux 7.1.3 x86_64 (Nobara 44) |
| 2. Linked worktree context | From an agent pane inside a linked worktree, invoke `open`; inspect the toolbar file list and diff. | The viewer shows that worktree's changes, not the main checkout's changes. | PENDING | PENDING | PENDING | PENDING |
| 3. Live refresh and degraded mode | Have an agent edit the same file twice and observe without pressing refresh. Repeat on an isolated Linux test host with `fs.inotify.max_user_watches` exhausted before opening; restore the test host's resources afterward. | Both edits appear without input; the notification failure shows a degraded-mode notice and polling still converges on the second edit. | PENDING | PENDING | PENDING | PENDING |
| 4. Same build in both hosts | Build once; link that same build into upstream Herdr 0.8.0 and the fork using its `vimeflow` binary. Invoke `open` from each host's separate plugin registry. Record both host versions. | The same binary opens and works in both hosts. | PENDING | PENDING | PENDING | PENDING |
| 5. Read-only guarantee | Run `cargo test --test readonly_guarantee`. | Only allow-listed git reads occur, required environment policy is present, and the index, refs, and worktree remain unchanged. | pass | 2026-09-20 | n/a (no host involved; git 2.55.0) | Linux 7.1.3 x86_64 (Nobara 44) |

Only the orchestrator/user may replace result cells with `pass`, fill in the date,
version, and host evidence, and change the first line to `Status: PASS` after all
five rows pass. The release workflow's guard refuses to build or publish without
that PASS line. Record both upstream and fork observations for criteria 2–4;
the inotify exhaustion check specifically requires a Linux test host.

## Evidence recorded so far

Rows 1 and 5 are automated and were run by the orchestrator on 2026-09-20 (commit `39aaf02`).
Rows 2-4 stay PENDING: the observations below cover part of each, and the rest needs the
plugin linked into a live host, which is the user's decision.

- **Row 2 (linked worktree), partial.** The release binary was run directly inside a linked
  worktree of a demo repository in a real herdr 0.8.0 pane: it listed that worktree's two
  changes and none of the main checkout's five. Still to do: invoke `open` through the plugin
  action from an agent pane inside a linked worktree.
- **Row 3 (live refresh), partial.** In a real pane, a line appended to the selected file from
  outside appeared without a keypress and the stats moved from +2 to +3. Degraded mode is
  covered by the engine tests (`degraded_mode_converges_on_a_repeated_edit_and_recovers_on_refresh`).
  Still to do: the inotify-exhaustion run on a test host (needs root for the sysctl).
- **Row 4 (same build in both hosts), partial.** The ignored Tier B test passed against
  upstream `herdr 0.8.0` and against the fork's `vimeflow 0.8.0` with the same release binary:
  plugin link, `open-split`, one viewer at the repository, reuse with focus, one record. Each
  run used its own named session with isolated HOME and XDG directories. Still to do: invoke
  `open` (the overlay) by hand in each live host.

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
