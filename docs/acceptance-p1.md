Status: PASS

# Phase 1 release acceptance

This is the release gate for the five success criteria in
[spec section 1.5](superpowers/specs/2026-09-18-hunks-roadmap-p1-viewer-design.md#15-phase-1-success-criteria)
and the branch-scope criteria in
[spec section 7.9](superpowers/specs/2026-09-22-branch-scope-design.md#79-tests-and-success-criteria).
The orchestrator and user perform and record these checks. Automated test results
alone do not complete manual acceptance. Phase 1 is not release-ready while any
result is PENDING.

| criterion | how to check | expected | result | date | herdr version | host |
| --- | --- | --- | --- | --- | --- | --- |
| 1. File list and hunk parity | Run `cargo test git_diff_response`. Cover staged, unstaged, partially staged (`MM`, `AM`), untracked including a new nested directory, deleted, and renamed files. Compare the list with `git status --porcelain=v1 --untracked-files=all`, tracked hunks with `git diff` / `git diff --cached`, and untracked hunks with `git diff --no-index -- /dev/null <path>`. Account for the documented K5 limitation. | All fixtures pass; list and hunks agree with git within the documented limitation. | pass | 2026-09-20 | n/a (no host involved; git 2.55.0) | Linux 7.1.3 x86_64 (Nobara 44) |
| 2. Linked worktree context | From an agent pane inside a linked worktree, invoke `open`; inspect the toolbar file list and diff. | The viewer shows that worktree's changes, not the main checkout's changes. | pass | 2026-09-21 | herdr 0.8.0 | Linux 7.1.3 x86_64 (Nobara 44) |
| 3. Live refresh and degraded mode | Have an agent edit the same file twice and observe without pressing refresh. Repeat on an isolated Linux test host with `fs.inotify.max_user_watches` exhausted before opening; restore the test host's resources afterward. | Both edits appear without input; the notification failure shows a degraded-mode notice and polling still converges on the second edit. | pass | 2026-09-21 | herdr 0.8.0 | Linux 7.1.3 x86_64 (Nobara 44) |
| 4. Same build in both hosts | Build once; link that same build into upstream Herdr 0.8.0 and the fork using its `vimeflow` binary. Invoke `open` from each host's separate plugin registry. Record both host versions. | The same binary opens and works in both hosts. | pass | 2026-09-21 | herdr 0.8.0 + vimeflow 0.8.0 | Linux 7.1.3 x86_64 (Nobara 44) |
| 5. Read-only guarantee | Run `cargo test --test readonly_guarantee`. | Only allow-listed git reads occur, required environment policy is present, and the index, refs, and worktree remain unchanged. | pass | 2026-09-20 | n/a (no host involved; git 2.55.0) | Linux 7.1.3 x86_64 (Nobara 44) |
| 6. Branch scope parity | On a branch with committed and uncommitted changes (the fixture of `branch_scope_lists_every_change_the_branch_carries_once`, or a real worktree), press `b`. Compare the row list with `git diff $(git merge-base HEAD main) --name-status -M --` plus the `??` rows of `git status --porcelain --untracked-files=all`; compare one tracked row's hunks with `git diff <M> -- <path>`, the rename with `git diff <M> -M -- <old> <new>`, and an untracked row with `git diff --no-index -- /dev/null <path>`. | Every changed path is listed once (twice only for a path deleted on the branch and recreated untracked) and each diff matches hunk for hunk. | pass | 2026-09-22 | herdr 0.8.0 + vimeflow 0.8.0 (tier B on both); by-hand check on a build of `df5ae8c` | Linux 7.1.3 x86_64 (Nobara 44) |
| 7. Default base and remembered pick | On a repository with `main` and no configuration, press `b`: the chip reads `vs main`. Press `B`, pick another ref, close the viewer, reopen it on the same worktree and press `b`. | The base is `main` without configuration; the reopened viewer compares against the picked ref. | pass | 2026-09-22 | n/a (viewer run directly; git 2.55.0) | Linux 7.1.3 x86_64 (Nobara 44) |

Only the orchestrator/user may replace result cells with `pass`, fill in the date,
version, and host evidence, and change the first line to `Status: PASS` after all
seven rows pass. The release workflow's guard refuses to build or publish without
that PASS line. Record both upstream and fork observations for criteria 2–4;
the inotify exhaustion check specifically requires a Linux test host.

## Evidence

Rows 1–5 passed for the original Phase 1 release. The owner confirmed the dialog and the
toolbar buttons added after the field test (commit `063982b`) on 2026-09-21, completing that
acceptance. Rows 6 and 7 (branch scope, 0.1.0) passed on 2026-09-22: the tier B test ran green
against both hosts, and a by-hand session on a demo repository (`main`, a feature branch with a
committed edit, rename, addition and deletion, plus an uncommitted edit, an untracked file and a
deleted-then-recreated path) listed the six rows `git diff <M> --name-status -M --` and the `??`
rows give, with `app.rs` showing the committed and uncommitted change in one hunk equal to
`git diff <M> -- app.rs`, the untracked row equal to `git diff --no-index -- /dev/null u.txt`,
and the rename row carrying its old path and, after editing the renamed file, the hunk
`@@ -1,3 +1,4 @@` / `+plus a line` with `+1 −0` equal to `git diff <M> -M -- old.txt new.txt`;
the base was `main` with no configuration, a pick of
`refs/tags/v0.1` was written to `bases.json` and survived quitting and reopening the viewer.

- **Row 1.** `cargo test --locked git_diff_response` (16 tests) on 2026-09-20. By hand on
  2026-09-21 the owner staged part of a file with `git add -p`: two rows, one marked `S`, each
  showing its half.
- **Row 2.** 2026-09-21, live herdr 0.8.0. The `open` action ran with an opener pane inside a
  linked worktree of a demo repository: the viewer started in that worktree and listed its 2
  changes, none of the main checkout's 5. The same result through `plugin pane open`.
- **Row 3.** 2026-09-21, live herdr 0.8.0, a real plugin pane. Live: two edits to the selected
  file appeared 0.41 s after each write with no keypress (stats `+2 -2` to `+3 -3`). Degraded:
  the viewer ran inside an unprivileged user namespace with `user.max_inotify_instances=0`
  (`unshare -Ur`), which makes the watcher fail exactly as an exhausted limit does without
  touching the host. The notice read `live refresh degraded: Failed to create watcher: Too many
  open files (os error 24) · polling every 5 s · r retries`, and two edits converged through
  polling in 1.8 s and 5.0 s; `r` retried and kept the notice. The host limit stayed at 384.
- **Row 4.** One release build, linked into both hosts. Upstream herdr 0.8.0: the owner opened
  it by action and by `prefix+d` in a live session (2026-09-21). Fork `vimeflow 0.8.0`: linked
  into the fork's live session, the viewer pane rendered and the `open` action exited 0.
  After `063982b` the popup `open` was re-run in an isolated session of each host: exit 0, the
  viewer started in the repository with `HERDR_HUNKS_PLACEMENT=popup`, sized 80% of the host,
  and no pane was listed. The ignored Tier B test (`open-split`) passes on both hosts.
- **Row 5.** `cargo test --locked --test readonly_guarantee` on 2026-09-20. By hand on
  2026-09-21: the 14 reserved keys left the screen byte-identical, and `git status`, the index
  bytes, the refs, the stash list and a content hash of the worktree were identical before and
  after a session of navigation, view toggles and refreshes.

The owner's field test covered 15 checks (10 by the owner, 5 by the orchestrator on request);
all 15 passed.

## Automated Tier B check

This ignored test starts and stops its own named server under a short `/tmp`
directory, with separate HOME, XDG_CONFIG_HOME, and XDG_STATE_HOME. Every child
starts with a cleared environment; host commands select the isolated session via
`--session`. It links the plugin, verifies the viewer cwd, presses `b` and selects
a committed row, invokes `open-split` twice from the opener, and checks pane reuse
and the saved reuse record. It never sets the test process's HERDR_SOCKET_PATH.

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
