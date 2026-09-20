# Port surface

## Pin

vimeflow `91e45b1c` (`91e45b1c8f381385093813d0b4eb1d9daeb2d563`).
The following files are copied from `crates/backend/src/git/` at the pin,
then changed only by the registered patches below:

- `src/git/mod.rs`
- `src/git/watcher.rs`
- `src/git/test_helpers.rs`

`scripts/port-check.sh <vimeflow-checkout>` verifies the pinned sources plus
`port/patches/*.patch` in order against `src/git/`. Two patches are registered:
`0001-no-ext-diff.patch` (D4) and `0002-drain-sync-output.patch` (D5).

## Port surface

The frozen tree imports exactly these two shim modules:

```rust
crate::filesystem::scope::{ensure_within_home, expand_home, home_canonical, reject_parent_refs}
crate::runtime::{serialize_event, EventSink}
```

Tests additionally import `crate::runtime::FakeEventSink`.
The mutating git functions are copied but are not called in Phase 1.

## Shims

`src/runtime/event_sink.rs` and `src/runtime/test_event_sink.rs` are byte-identical
copies of vimeflow's `crates/backend/src/runtime/` files at the pin.
`src/runtime/mod.rs` supplies the re-exports used by the frozen tree.
The unused `e2e-test` compatibility feature preserves the copied conditional
compilation attributes; `tui` is the default application feature.

`src/filesystem/scope.rs` copies `expand_home`, `home_canonical` and
`reject_parent_refs` unchanged. `ensure_within_home` differs according to D1:

**D1 (Phase 1): cwd scope policy.** vimeflow's `validate_cwd` rejects any cwd
outside `$HOME` (`vimeflow:crates/backend/src/git/mod.rs:81-89`), because there the
cwd arrives over IPC from a renderer. In the plugin the path comes from the user's
own pane or command line, and repositories outside `$HOME` (`/srv`, `/mnt`, a
container's `/workspace`) are ordinary. The frozen `git/mod.rs` is not edited.
The shim `filesystem/scope.rs` keeps the four function signatures, keeps
`expand_home` and `reject_parent_refs` byte-identical, and changes
`ensure_within_home` to accept any canonical absolute path. The shim's doc comment
and `PORT-SURFACE.md` both state this. Canonicalization and the `..` rejection
still run.

## Registered divergences

D1 is implemented by the filesystem shim. D2 and D3 are registered Phase 1
policies; their engine and process startup implementations are still pending.

**D1 (Phase 1): cwd scope policy.** vimeflow's `validate_cwd` rejects any cwd
outside `$HOME` (`vimeflow:crates/backend/src/git/mod.rs:81-89`), because there the
cwd arrives over IPC from a renderer. In the plugin the path comes from the user's
own pane or command line, and repositories outside `$HOME` (`/srv`, `/mnt`, a
container's `/workspace`) are ordinary. The frozen `git/mod.rs` is not edited.
The shim `filesystem/scope.rs` keeps the four function signatures, keeps
`expand_home` and `reject_parent_refs` byte-identical, and changes
`ensure_within_home` to accept any canonical absolute path. The shim's doc comment
and `PORT-SURFACE.md` both state this. Canonicalization and the `..` rejection
still run.

The frozen tests `test_git_branch_rejects_out_of_scope_cwd` and
`test_git_worktree_name_rejects_out_of_scope_cwd` still pass with `/etc`
because it is not a git repository, rather than because of the scope check.
On a machine where `/etc` is a repository (for example, with etckeeper),
these tests fail because of that machine configuration.

**D2 (Phase 1): engine content poll.** The frozen watcher's poll fallback detects
change by hashing `git status` output only
(`vimeflow:crates/backend/src/git/watcher.rs:487,844`). A repeated edit to an
already-modified file changes neither the status letters nor that hash, so when a
filesystem event is missed the diff and the +/- counts stay stale. The watcher is
not edited. The engine adds its own poll: every 5 s, if no watcher event has
triggered a refresh in the last 5 s, it re-runs status and the selected row's
diff, and replaces its state only when the status response or `raw_diff` differs.
When the watcher failed to start, the tick is unconditional and also re-fetches
branch and worktree name (degraded mode, 3.3).

**D3 (Phase 1): git child environment.** The frozen code spawns git itself, so
policy can only be applied through the process environment. `main` sets three
variables before any thread starts, and every git child inherits them, including
the watcher's synchronous calls. The frozen tree is not edited.

- `GIT_OPTIONAL_LOCKS=0`. vimeflow never sets it, so its background `git status`
  and `git diff` calls (`vimeflow:crates/backend/src/git/mod.rs:1146`,
  `watcher.rs:488`) may take `index.lock` and rewrite the index to refresh its
  stat cache. For a viewer that polls while an agent runs git in the same
  worktree, that risks `index.lock` collisions and breaks G7.
- `GIT_LITERAL_PATHSPECS=1`. The frozen diff call passes the selected path after
  `--` with no literal-pathspec mode (`mod.rs:1513-1518`), so a file named
  `a*.txt` would also match `ab.txt`, and the single-file parser would merge both
  files' hunks under one `FileKey`. The frozen code uses no pathspec magic
  anywhere, so literal mode is correct for every call.
- `GIT_NO_LAZY_FETCH=1`. In a partial clone, `git diff`, `git show` and
  `git cat-file` would otherwise fetch missing objects from the promisor remote.
  git 2.45 and newer honour the variable; older versions ignore it. This item is
  therefore best-effort hygiene, and G7 does not depend on it. A hard floor of 2.45
  was rejected: it would lock out every system that still ships an older git, for
  the sake of a partial-clone edge case.

**D4 (Phase 1, patch): no external diff.** The two patch-producing diff calls
(`vimeflow:crates/backend/src/git/mod.rs:1343,1500`) pass `--no-color` but not
`--no-ext-diff`, so `diff.external`, `GIT_EXTERNAL_DIFF` or a per-path diff driver
replaces the unified diff. With a tool such as difftastic configured globally,
every file would parse to zero hunks. No environment variable turns external
diffs off, so this cannot be a D3 item. `port/patches/0001-no-ext-diff.patch` adds
`--no-ext-diff` to those two calls. The numstat and name-status calls never run
an external diff and are untouched. Textconv filters are left on: their output is
still a unified diff.

**D5 (Phase 1, patch): drain child output.** The watcher's
`run_sync_with_timeout` polls `try_wait` and reads the child's pipes only after it
exits (`vimeflow:crates/backend/src/git/watcher.rs:195-214`). A `git status` whose
output exceeds the pipe buffer (64 KiB on Linux; a few thousand untracked paths)
blocks on write until the 10 s timeout kills it, so in such a repository the
status-hash fallback never works and a git process is always hung.
`port/patches/0002-drain-sync-output.patch` reads stdout and stderr on helper
threads while waiting, and adds its test inside `watcher.rs`, because the function
is private.

## Known defects

**K1-K5: known defects in the frozen tree.** K1-K4 sit in the mutating paths and
are unreachable in Phase 1. K5 is in a read path and is visible in Phase 1. All
five are fixed in Phase 2, through the patch mechanism of 2.2 or by sibling
reimplementation where a patch would be large.

- **K1.** Stage, unstage and discard run with `current_dir(<pane cwd>)`
  (`mod.rs:369,393,437-462`), while status and diff return toplevel-relative paths
  and run with `-C <toplevel>`. From a subdirectory the path no longer matches.
- **K2.** Whole-file operations discard git's exit status:
  `run_git_with_timeout` returns `Ok` for any exit code (`mod.rs:41`) and the
  callers end in `.map(|_| ())?` (`mod.rs:370,395,438-463`).
- **K3.** Per-hunk discard ignores its scope (`mod.rs:426-428,466-469`). From the
  staged view it reverse-applies the HEAD-to-index patch to the worktree only, so
  the index keeps the change.
- **K4.** Unstaging one hunk of a staged rename also reverses the rename, because
  the reused patch header carries `rename from` / `rename to`. This was observed
  with a plain-git probe during the scan and must be re-verified in the Phase 2
  spec.
- **K5.** `parse_git_status` splits `MM`, `AM` and rename-plus-edit into two rows,
  but `MD` and `AD` fall to its default arm (`mod.rs:883-892`) and produce one
  unstaged `Modified` row. After staging a change and then deleting the working
  file, the staged half is hidden and the deletion is labelled modified. vimeflow
  accepted this limit (its VIM-327 spec puts extending the parser out of scope).
  Phase 1 inherits it because the parser is frozen.

## Copied from herdr-agent-watcher

| file | source | adaptation |
| --- | --- | --- |
