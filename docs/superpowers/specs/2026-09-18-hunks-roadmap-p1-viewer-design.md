# herdr-hunks: roadmap and Phase 1 (read-only hunk viewer)

**Date:** 2026-09-18 · **Status:** draft
**Scope:** roadmap for all phases; detailed design for Phase 1 only.
**Behaviour source:** vimeflow `main` at `91e45b1c`, cited below as `vimeflow:<path>:<line>`.

## 1. Scope, goals, and roadmap

### 1.1 Background

vimeflow's "Review Changes Hunk by Hunk" feature is being migrated to herdr as a
standalone plugin. It is the third extraction, after `herdr-agent-title-sync` and
`herdr-agent-watcher`. The plugin must run unchanged on upstream herdr >= 0.8.0
and on the vimeflow-terminal fork. A diff of the plugin host between herdr 0.8.0
and the fork found it unchanged (socket protocol 19).

### 1.2 Roadmap

Each phase is its own spec -> plan -> implementation cycle and ships as a release.
The spec for phase N+1 starts after phase N ships.

| Phase | Delivers | Mutates git | Talks to agents |
| ----- | -------- | ----------- | --------------- |
| P1 | Read-only viewer: changed-files list, diff rendering, hunk/line/file navigation, navigation toolbar, live refresh | no | no |
| P2 | Hunk actions: stage / unstage / discard hunk, discard file, Y/N confirmations, the frozen-tree fixes K1-K5 listed in 2.4 | yes | no |
| P3 | Comments and dispatch: line / range / file comments, four categories, feedback batch, dispatch through `agent.prompt`, copy fallback, persistence | yes | send only |
| P4 | Replies and threads: `VIMEFLOW_REPLY` capture, nonce-scoped recovery, thread cards, resolve / reply | yes | yes |
| P5 | Delegated review: request review (file / all changes), `VIMEFLOW_REVIEW` findings placement, review-level notes | yes | yes |

### 1.3 Phase 1 goals

- **G1 Open from herdr.** One action opens the viewer from any pane, scoped to
  that pane's git worktree. The repo is resolved from the opener's
  `foreground_cwd` when herdr reports one, else from its `cwd`.
- **G2 Changed-files list.** Same content as vimeflow's list: one row per
  `(path, staged)` half, so a partially staged file (`MM`, `AM`, or a staged
  rename with worktree edits) shows two rows; statuses modified / added / deleted /
  renamed / untracked; insertion and deletion counts. The list inherits the frozen
  parser's one known gap, K5 in 2.4.
- **G3 Diff view.** Hunks come from git's own unified diff for the selected row.
  Unified and split modes. A configured external diff tool must not replace that
  output (D4 in 2.4).
- **G4 Navigation.** vimeflow's read-only key subset: line `j`/`k`, half page
  `Ctrl+d`/`Ctrl+u`, hunk `[`/`]`, file `n`/`p`, side `h`/`l` (split mode), view
  toggle `t`, files list `e` / pin `E`, refresh `r`. The TUI adds a few keys
  vimeflow has no need for, such as `?` help and `q` quit (4.3).
- **G5 Toolbar.** One row mirroring vimeflow's diff toolbar: file stepper (name,
  i/n), hunk stepper (i/n), view mode, staged/unstaged badge, +/- totals. Every
  item is reachable by key and clickable with the mouse.
- **G6 Live refresh.** Working-tree and index changes appear without a keypress.
  The primary signal is vimeflow's ported watcher (filesystem events, 300 ms
  debounce). Its 10 s poll fallback hashes only `git status` output
  (`vimeflow:crates/backend/src/git/watcher.rs:487,844`), so a repeated edit to an
  already-modified file is invisible to it. The engine therefore adds its own
  content poll for the selected row (D2 in 2.4), so the diff still converges when
  filesystem events are missed.
- **G7 Strictly read-only.** The P1 binary never runs a mutating git subcommand
  and never changes the worktree, the index, or any branch, tag or stash ref. This
  holds on every supported git (2.31 or newer, checked at startup), and a test
  enforces it. Read-oriented commands have a side effect of their own:
  `git status` and `git diff` refresh the index opportunistically, which D3 in 2.4
  switches off for every git child. D3 also asks git not to lazy-fetch missing
  objects in a partial clone; git honours that only from 2.45, so it is best-effort
  hygiene and not part of the guarantee. Side effects that the user's own git
  configuration attaches to read commands (a textconv cache under
  `refs/notes/textconv/`, external diff or textconv programs, fsmonitor) are that
  configuration at work and are outside G7.
- **G8 Standalone.** `herdr-hunks [PATH]` also runs outside herdr, so it can be
  tested and used without the host.

### 1.4 Non-goals for Phase 1

- Stage / unstage / discard (P2); comments and dispatch (P3); replies (P4);
  delegated review (P5).
- Visual selection and yank (`v`/`y`) and in-diff search (`/`): scheduled with P3,
  where range selection is first required.
- Syntax highlighting and word-level intra-line diff: unscheduled enhancements.
- Base-branch comparison, commit log, branch operations: outside this roadmap.
- Windows. A daemon or any background process.

### 1.5 Phase 1 success criteria

1. On a repo holding staged, unstaged, partially staged (`MM`, `AM`), untracked (including a
   file inside a new untracked directory), deleted and renamed files, the list
   agrees with `git status --porcelain=v1 --untracked-files=all`, tracked hunks
   agree with `git diff` and `git diff --cached`, and untracked hunks agree with
   `git diff --no-index -- /dev/null <path>` (proved by the ported tests plus a
   nested-untracked fixture).
2. Opened from an agent pane inside a linked worktree, it shows that worktree.
3. While an agent edits files, the view updates without user input, including a
   second edit to an already-modified file with filesystem notifications
   unavailable.
4. The same build works in upstream herdr 0.8.0 and in the fork.
5. The read-only guarantee test passes.

### 1.6 Prior art

The `herdr-plugin` GitHub topic held 1,256 repositories on 2026-09-19. The closest
ones, described from their READMEs:

| Plugin | Covers |
| --- | --- |
| `persiyanov/herdr-reviewr` (Rust) | diff review pane, four diff scopes, line and range comments sent to the agent, read-only, runs standalone |
| `ChmaraX/herdr-gitview` (Rust) | staged / unstaged sections, stage / discard / commit per file or directory, notes sent to an agent, mouse |
| `jhochenbaum/herdr-hunk-diff` (TypeScript) | opens the Hunk TUI (hunk.dev) on the right worktree and sends its inline comments to the agent |
| `smarzban/herdr-file-viewer`, `alexarthurs/herdr-sidebar` | git-aware file and diff viewing, source control |

About ten more repositories carry `hunk` in their name, four of them exactly
`herdr-hunk`; they wrap the Hunk tool. Phase 1 and most of Phase 3 therefore
overlap existing plugins. What none of those READMEs offers, and what this roadmap
ports from vimeflow, is per-hunk stage / unstage / discard (P2), agent replies
threaded back onto the hunk with outcomes and restart recovery (P4), delegated
review findings placed on hunks (P5), and an engine the fork can embed. Read-only
first is kept deliberately: Phase 1 is the foundation for those phases, not the
differentiator.

## 2. Architecture and port method

### 2.1 Crate layout

One crate, `herdr-hunks`, with a library and a binary. The library holds
everything that is not terminal I/O, so the fork can embed it later the way it
embeds `herdr-agent-watcher`.

```
herdr-hunks/
├── herdr-plugin.toml
├── Cargo.toml            # [lib] herdr_hunks; [[bin]] herdr-hunks, required-features = ["tui"]
├── PORT-SURFACE.md       # what the frozen tree imports, and every registered divergence
├── port/patches/         # registered patches applied on top of the pinned sources (2.2)
├── scripts/
│   ├── fetch-or-build.sh # [[build]] step: verified release asset, else cargo build
│   └── port-check.sh     # diffs src/git/ against vimeflow at the pinned commit
└── src/
    ├── lib.rs
    ├── main.rs           # subcommands: tui [PATH] (default), open, open-split, update
    ├── git/              # FROZEN. vimeflow crates/backend/src/git/ (mod.rs, watcher.rs,
    │                     #   test_helpers.rs) at 91e45b1c, plus port/patches/
    ├── filesystem/scope.rs   # shim for the frozen tree's imports (policy differs: D1)
    ├── runtime/              # shim: EventSink, serialize_event, FakeEventSink (byte-identical)
    ├── engine/               # UI-agnostic view-model (Section 3)
    ├── herdr/                # socket client + typed calls, copied from herdr-agent-watcher
    └── tui/                  # pure view + ratatui shell (Section 4)
```

**Features.** The frozen `git` module uses `tokio`, `notify`, `ignore` and `libc`
unconditionally, so those are unconditional dependencies and `pub mod git;` needs
no gate. The one feature is `tui` (default): it gates `ratatui`, `crossterm`,
`unicode-width`, the `src/tui/` module and the binary. An embedder that wants only
the engine depends on the crate with `default-features = false`. CI runs both
`cargo check` and `cargo check --no-default-features`.

**Targets.** Unix only. `lib.rs` carries
`#[cfg(not(unix))] compile_error!(...)`, and an embedder declares the dependency
under `[target.'cfg(unix)'.dependencies]`, which is how the fork already declares
`herdr-agent-watcher`.

`ts-rs` is a dev-dependency only, kept so the frozen files'
`#[cfg_attr(test, derive(ts_rs::TS))]` lines compile unchanged; the generated
`bindings/` directory is gitignored.

### 2.2 Port method

The method is the one `herdr-agent-watcher` used (its `DESIGN.md` and
`PORT-SURFACE.md`), with one addition.

1. **Copy mechanically and freeze.** `src/git/` is
   `vimeflow:crates/backend/src/git/` at `91e45b1c` with the patch files in
   `port/patches/` applied in order, and nothing else. Nobody edits it by hand: a
   change is either a new pin or a new patch file. Phase 1 has two patches, D4 and
   D5 (2.4); each carries its reason in its header and is small enough to offer
   back to vimeflow, where the same defect exists.
2. **Satisfy its imports with shims.** The frozen tree imports exactly
   `crate::filesystem::scope::{ensure_within_home, expand_home, home_canonical, reject_parent_refs}`
   (`vimeflow:crates/backend/src/git/mod.rs:13-15`) and
   `crate::runtime::{serialize_event, EventSink}` plus `FakeEventSink` in tests
   (`vimeflow:crates/backend/src/git/watcher.rs:27,1497`). Those two modules are
   the whole port surface.
3. **Register every divergence** in `PORT-SURFACE.md` with its reason.
4. **One host seam.** The watcher emits through `Arc<dyn EventSink>`
   (`emit_json(event, payload)`). The plugin implements that trait with a
   channel-backed sink that feeds the engine's session loop (3.3).
5. **Addition: parity is checked by a script, not by convention.**
   `scripts/port-check.sh <vimeflow-checkout>` takes a pristine copy of the pinned
   sources, applies `port/patches/*.patch` in order, and requires the result to
   equal `src/git/` byte for byte. CI runs it when a vimeflow checkout is
   available. `herdr-agent-watcher` has no such check and its frozen
   tree has drifted in five unregistered files.

The mutating functions in the frozen tree (`stage_file_inner`,
`unstage_file_inner`, `discard_file_inner`) are copied but not called in Phase 1.
`lib.rs` declares the module as `#[allow(dead_code)] pub mod git;` so no frozen
file needs an attribute added. G7 is enforced by the test in Section 5, not by
deleting code.

### 2.3 Process model

One process per open viewer pane. There is no daemon, no `[[startup]]` hook and no
`[[events]]` hook in Phase 1.

Inside the process the main thread owns the terminal and runs the draw loop. A
two-worker tokio runtime runs git subprocesses and the watcher. Results reach the
draw loop through channels that it polls with `try_recv`, so a slow git call never
blocks input or drawing. This is the structure `herdr-agent-watcher`'s sidebar
uses.

### 2.4 Registered divergences and known defects

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

**K1-K6: known defects.** K1-K5 are in the frozen tree. K1-K4 sit in the mutating
paths and are unreachable in Phase 1. K5 is in a read path and is visible in
Phase 1. All five are fixed in Phase 2, through the patch mechanism of 2.2 or by sibling
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
- **K6.** Superseded engine diff requests are not cancelled: the frozen
  `run_git_with_timeout` waits in `spawn_blocking`, so aborting the Tokio task
  would not kill its git child. Holding n/p with slow diffs can therefore pile
  up git processes. Phase 1 accepts this limitation. Phase 2 adds a concurrency
  cap or a cancellable runner.

### 2.5 herdr integration

**Manifest.** `id = "winoooops.hunks"`, `name = "hunks"`,
`min_herdr_version = "0.8.0"`,
`platforms = ["macos", "linux"]`, a `[[build]]` step that runs
`scripts/fetch-or-build.sh`, one `[[panes]]` entry `viewer` with
`placement = "overlay"`, and three `[[actions]]`: `open`, `open-split`, `update`.
The id is owner-namespaced, which is the convention among herdr plugins and keeps
it clear of the two unrelated plugins that already use the bare id `herdr-hunk`
(1.6). The repository, crate and binary keep the name `herdr-hunks`. Code never
hardcodes the id: requests take `plugin_id` from `$HERDR_PLUGIN_ID`, and the
config and state directories come from `$HERDR_PLUGIN_CONFIG_DIR` and
`$HERDR_PLUGIN_STATE_DIR`.

**The host binary is never named literally.** Every call goes through
`$HERDR_BIN_PATH` or the socket at `$HERDR_SOCKET_PATH`, because the fork's
executable is `vimeflow`.

**Open flow.**

```
key  ->  [[keys.command]] type="plugin_action" command="winoooops.hunks.open"
     ->  herdr runs the action argv (no TTY, cwd = plugin root) with HERDR_PLUGIN_CONTEXT_JSON
     ->  `herdr-hunks open`:
           opener  = context.focused_pane_id
           pane    = pane.get {pane_id: opener}
           repo_cwd = pane.foreground_cwd, else pane.cwd, else context.focused_pane_cwd
           open:        plugin.pane.open {plugin_id, entrypoint:"viewer", placement:"overlay",
                                          cwd: repo_cwd, env:{HERDR_HUNKS_OPENER_PANE: opener}, focus:true}
           open-split:  plugin.pane.open {plugin_id, entrypoint:"viewer", placement:"split",
                                          target_pane_id: opener, direction:"right",
                                          cwd: repo_cwd, env:{HERDR_HUNKS_OPENER_PANE: opener}, focus:true}
     ->  herdr spawns the [[panes]] argv in a PTY:
           ["/bin/sh","-lc","exec \"$HERDR_PLUGIN_ROOT/target/release/herdr-hunks\" tui"]
     ->  `herdr-hunks tui` uses its process cwd as the repository path
```

The pane command is an absolute path behind `sh -lc` for two reasons: herdr
resolves a relative program against the pane's cwd, which `cwd:` has just changed,
and a login shell gives `git` the same `PATH` the user's panes have.
herdr rejects `target_pane_id` and `direction` for `overlay` with
`invalid_params` and uses the focused pane instead, so only the `open-split`
request carries them.

**Placements.** `open` uses `overlay`: herdr splits the focused pane, zooms the
new pane, and restores focus and zoom when the viewer exits. `open-split` uses
`split` to the right of the opener, for watching the diff while the agent works.
herdr opens splits at 50/50 and has no ratio parameter.

**Instances.** herdr neither dedupes nor lists plugin panes. `open-split` resolves
the worktree toplevel of `repo_cwd` (`git rev-parse --show-toplevel`) and records
`{viewer_pane_id, repo_cwd, toplevel}` in
`$HERDR_PLUGIN_STATE_DIR/split-panes.json`. The state directory is shared by every
herdr session and pane ids such as `w1:p1` are session-local, so the key is the
pair (`$HERDR_SOCKET_PATH`, opener pane id). On the next invoke it reuses the
recorded viewer only when all of these hold: the recorded `toplevel` equals the
one just resolved; `pane.get` on the recorded viewer id succeeds; that pane's
label is the viewer's manifest title and its `cwd` is the recorded `repo_cwd`; and
`plugin.pane.focus` succeeds. Otherwise it opens a new viewer and overwrites the
record; an old viewer keeps running until the user quits it. `overlay` needs no
record.

**Keybinding.** Phase 1 documents the `[[keys.command]]` snippet in the README with
`prefix+d` as the suggested key (unbound in both upstream herdr and the fork). Porting
`herdr-agent-watcher`'s keybinding installer is deferred; it is about 1,200 lines
and nothing in Phase 1 depends on it.

**Socket methods used in Phase 1:** `pane.get`, `plugin.pane.open`,
`plugin.pane.focus` (`pane.get` serves both the opener lookup and the reuse check). The client is `herdr-agent-watcher`'s: one JSON request per
connection, newline-delimited, 3 s read timeout.

## 3. Engine API and data flow

### 3.1 Boundary

`engine` is the library's public face. It has no `ratatui` or `crossterm` types,
wraps the frozen `git` module, and is the only caller of it. The TUI is one
consumer of the engine; a fork-native view could be another. Later phases add to
the engine (mutations in P2, comments in P3) without changing this boundary.

### 3.2 Types

```rust
/// vimeflow's file identity: a partially staged path is two rows.
pub struct FileKey { pub path: String, pub staged: bool }

pub struct Snapshot {
    pub revision: u64,                 // bumps on every published change
    pub repo: RepoState,               // Repo { toplevel, branch, worktree } | NotARepo { cwd }
                                       //   | Unusable { reason }: bad path, or git older than 2.31
    pub files: Vec<ChangedFile>,       // frozen type, in git_status order
    pub selected: Option<FileKey>,
    pub diff: DiffState,               // Idle | Loading | Ready(Arc<LoadedDiff>) | Failed(String)
    pub status_error: Option<String>,  // last status failure; `files` keeps the last good list
    pub watcher_error: Option<String>, // set while live refresh runs degraded, see 3.3
    pub refreshing: bool,              // a user-requested refresh is in flight, see 3.3
}

pub struct LoadedDiff {
    pub key: FileKey,
    pub file_diff: FileDiff,           // frozen type: hunks parsed from git's unified diff
    pub raw_diff: String,              // kept for change detection now, patch slicing in P2
    pub targets: Vec<Target>,          // navigation rows, see 3.4
    pub truncated_lines: usize,        // diff lines dropped by the size cap, see below
}

pub enum Command { Select(FileKey), SelectNext, SelectPrev, Refresh, Shutdown }
```

**Size cap.** `LoadedDiff` keeps at most 200,000 diff lines. It keeps whole hunks
while the running line count fits, and if the first hunk alone is larger it keeps
that hunk's first 200,000 lines. `truncated_lines` records what was dropped.
`targets` are built from the kept hunks, so navigation, the hunk stepper and the
rendered rows always agree; `raw_diff` stays complete for change detection.

The UI sends `Command`s over a non-blocking channel and receives `Arc<Snapshot>`
values over another. It renders from the latest snapshot and never waits on the
engine.

### 3.3 Session loop

The loop runs on the tokio runtime (2.3) and has three inputs: commands from the
UI, watcher events, and the D2 tick.

- **Version check.** Before any repository read the engine runs `git --version`.
  The floor is git 2.31: the frozen watcher resolves the git directory with
  `rev-parse --path-format=absolute` (`vimeflow:crates/backend/src/git/watcher.rs:408-414`),
  which older versions lack, so it could never start there. (`GIT_OPTIONAL_LOCKS`
  alone would need only 2.15.) An older git is a fatal start error: the engine
  publishes `RepoState::Unusable` and runs no repository command. A `PATH` argument
  that is missing or not a directory ends the same way.
- **Start.** Resolve the path, then start the frozen watcher with
  `start_git_watcher_backend(cwd, sink, state)` and run the first refresh
  concurrently; the first refresh never waits for the watcher. The sink
  is the channel-backed `EventSink`; the events it receives are
  `git-status-changed` and `git-head-changed`, each with a `cwds` payload
  (`vimeflow:crates/backend/src/git/watcher.rs:367-377,1470-1484`).
- **Degraded mode.** Watcher creation can fail before its poll thread starts
  (`vimeflow:crates/backend/src/git/watcher.rs:722-766`; an exhausted inotify watch
  limit is the usual cause). The engine then sets `watcher_error`, keeps running,
  and relies on the D2 tick alone: the tick runs every 5 s unconditionally and also
  re-fetches branch and worktree name, because no `git-head-changed` will arrive.
  A `Refresh` command retries the watcher start, and success clears
  `watcher_error`. This is the state success criterion 3 calls "filesystem
  notifications unavailable".
- **Refresh.** `git_status_inner(cwd)`, then for the selected row
  `get_git_diff_inner(cwd, path, staged, untracked)`. Branch and worktree name
  come from `git_branch_inner` and `git_worktree_name_inner`, fetched at start and
  on `git-head-changed`.
- **Not a repository.** `git_status_inner` returns an empty `repo_root` and no
  files for a non-repo cwd (`vimeflow:crates/backend/src/git/mod.rs:1126-1132`).
  The engine maps that to `RepoState::NotARepo`. The frozen watcher's pre-repo mode
  fires when `.git/` appears, and the next refresh upgrades the state.
- **Loading versus background refresh.** `DiffState::Loading` is used only when
  there is no diff to show for the selected `FileKey`: the first load and a
  selection change. A refresh of the same `FileKey` (watcher event, D2 tick or
  `r`) keeps `Ready(old)` in place and swaps it only when the new result differs,
  so an unchanged repository produces no transition, no redraw, and no loss of
  the diff the TUI reconciles against (4.8). Only a refresh the user asked for with
  `r` sets `refreshing` while it runs, as feedback.
- **Coalescing.** At most one refresh is in flight. A trigger that arrives during
  a refresh sets a dirty flag, and exactly one follow-up refresh runs.
- **Stale results.** Every refresh carries a generation number. A diff result is
  dropped when its `FileKey` is no longer selected or its generation is older than
  the last published one.
- **Selection after a status change.** Keep the same `FileKey` if it is still
  listed. Otherwise keep the same index, clamped to the new length. The first load
  selects the first row. `SelectNext` and `SelectPrev` wrap, as vimeflow's `n`/`p`
  do (`vimeflow:src/features/diff/Panel.tsx:1796-1814`).
- **Publishing.** A snapshot is published whenever any field other than
  `revision` differs from the last published snapshot: `repo` (a branch switch in
  a clean repository changes nothing else), `files`, `selected`, `diff` (including
  the `Loading`, `Failed` and recovery transitions, and a changed `raw_diff`),
  `status_error`, `watcher_error` and `refreshing`. `revision` increments only then, so a quiet
  repository causes no redraws.

### 3.4 Navigation model

`engine::nav` is pure. It ports four functions from
`vimeflow:src/features/diff/hooks/useReviewTargetNavigation.ts` as functions over
`&[Target]` and a cursor index:

| vimeflow | lines | engine::nav |
| --- | --- | --- |
| `reviewTargetsForDiff` | 74-176 | `targets_for_diff(&FileDiff) -> Vec<Target>` |
| `targetIndexForHunk` | 633-644 | `target_index_for_hunk` (first changed row of the hunk, else its first row) |
| `moveTargetLine` | 663-730 | `move_line(targets, cursor, delta, mode)` (clamps at both ends; in split mode a replacement pair is one row and the side is kept) |
| `moveTargetSide` | 733-762 | `move_side(targets, cursor, side)` (split mode only, same row) |

```rust
pub struct Target {
    pub line_number: u32,
    pub side: Side,            // Additions = new-file numbering, Deletions = old-file numbering
    pub hunk_index: usize,
    pub split_row_index: usize,
    pub changed: bool,
}
```

The ported builder emits a changed block's rows pair-interleaved (deletion 0,
addition 0, deletion 1, ...), which is display order for split mode. Unified mode
displays a block's deletions first and then its additions, so for unified mode
`engine::nav` also provides that block-ordered sequence, and `move_line` walks it.
`(FileKey, side, line_number)` is the anchor P3 comments will attach to.

Half-page scrolling stays in the TUI, because it depends on the viewport. After
scrolling, the TUI asks `engine::nav` for the target nearest a given display row,
which is vimeflow's "snap to viewport centre" behaviour
(`useReviewTargetNavigation.ts:646-659`).

### 3.5 Read-only guarantee and git hygiene

In Phase 1 the engine calls only `git_status_inner`, `get_git_diff_inner`,
`git_branch_inner`, `git_worktree_name_inner` and the watcher's start and stop
functions.

Those read paths still have side effects of their own, which D3 (2.4) addresses
through three environment variables (the lazy-fetch one is best effort on git
older than 2.45). The frozen code spawns git itself, so
the variables can only be applied to the whole process.
`engine::init_process_env()` sets them; `main` calls it before starting any
thread, and an embedder must do the same before it starts the engine.

### 3.6 Errors

- A status failure sets `status_error` and keeps the last good `files`.
- A diff failure sets `DiffState::Failed` for that row only; other rows still load.
- A watcher start failure sets `watcher_error` and switches to degraded mode (3.3);
  it is never fatal.
- Git calls keep the frozen 30 s timeout with SIGKILL
  (`vimeflow:crates/backend/src/git/mod.rs:19-78`).
- Every string that came from git (paths, error text, diff lines) is untrusted
  display text. The engine passes it through unchanged; the TUI sanitizes it
  before drawing (Section 4).

## 4. TUI

### 4.1 Structure

The TUI follows `herdr-agent-watcher`'s sidebar: a pure view and a thin shell.

- `tui::rows` turns a `LoadedDiff` and a view mode into a `Vec<Row>` once per
  diff or mode change. Row kinds: `FileHeader`, `HunkHeader`, `Gap(n)`,
  `Unified { old_no, new_no, sign, text }`, `Split { left, right }`.
- `tui::view::render(&ViewState, width, height) -> Rendered` is pure. It emits
  only the rows inside the viewport, plus the hit regions for the mouse. A diff of
  many thousand lines costs one screen of work per frame.
- `tui::shell` owns the terminal: `TerminalGuard` (raw mode, alternate screen,
  mouse capture, restored on drop and on panic), the poll loop, and drawing
  `Rendered` through ratatui. These are copied from `herdr-agent-watcher`
  (`src/sidebar/tui.rs`, `style.rs`, `layout.rs`, `dialog.rs`) and registered in
  `PORT-SURFACE.md` as copies.
- Key and mouse handlers mutate only `ViewState` (4.8) and return an outcome:
  `Quit`, `Redraw`, `Inert`, or `Engine(engine::Command)`. Cursor movement,
  scrolling, the view toggle, the files panel, the mouse flag and the key sheet
  are all `ViewState` changes. Handlers never touch the snapshot or the terminal:
  the run loop forwards `Engine` commands, applies the requested mouse-capture
  state to the terminal, and redraws. `Inert` events never redraw.

### 4.2 Layout

```
┌────────────────────────────────────────────────────────────────────────────┐
│ ‹ values.ts 4/5 ›   {} 2/3 ↑ ↓   unified   UNSTAGED   +4 −3   files   ⟳    │ toolbar
├─────────────────┬──────────────────────────────────────────────────────────┤
│ CHANGED 5       │ src/values.ts                                   −3 +4    │ file header
│  M auth.ts    S │    1    1   export const value1 = 1                      │
│  M auth.ts      │    3      - export const value3 = 3                      │
│  D old.ts     S │         3 + export const value3 = 300 // changed near…   │
│ ▸M values.ts    │ ··· 18 unmodified lines ···                              │
│  A new-file.ts  │ @@ -27,7 +27,8 @@ export const value26 = 26              │
│                 │   27   27   export const value27 = 27                    │
│ +8 −6 · 5 files │ ▌ 30      - export const value30 = 30                    │ cursor row
├─────────────────┴──────────────────────────────────────────────────────────┤
│ j/k line  [ ] hunk  n/p file  t view  e files  r refresh  ? help  q quit   │ footer
└────────────────────────────────────────────────────────────────────────────┘
```

- **Toolbar (one row).** Left to right: file stepper `‹ name i/n ›`, hunk stepper
  `{} i/n ↑ ↓`, view-mode chip, `STAGED`/`UNSTAGED` badge, the row's `+a −d`,
  files toggle, refresh. The order matches vimeflow's toolbar
  (`vimeflow:src/features/diff/components/toolbar/DiffChipToolbar.tsx`); the slot
  between the two steppers is reserved for P2's stage / unstage / discard group.
  When the row is too narrow, items drop from the right in that order and the
  steppers go last.
- **Files panel.** One row per `ChangedFile`: status letter (`M A D R ?`),
  basename, a trailing `S` for staged rows, and the directory dimmed when two rows
  share a basename. Footer: totals and file count. `e` toggles it, `E` pins it.
  With `files = "auto"` it is pinned at widths of 100 columns or more and hidden
  below that.
- **Diff body.** Unified mode shows old number, new number, sign and text. Split
  mode shows two columns, aligned by `split_row_index`, with blank filler on the
  shorter side of a replacement. A `Gap(n)` row between hunks reads
  `··· n unmodified lines ···`, computed from the hunk ranges; Phase 1 does not
  expand context. Hunk headers show git's `@@` line dimmed.
- **Cursor.** The current target's row is drawn in reverse video, which survives
  any terminal theme. The hunk stepper tracks the cursor's `hunk_index`.
- **Long lines.** No wrapping. `H` and `L` scroll the body horizontally by 8
  columns. Width uses `unicode-width`; a tab advances to the next multiple of 8.
- **Footer.** Key hints for the focused area. Hints drop from the right when
  narrow.

Split mode needs at least 100 columns. Requesting split below that width shows a
one-line notice and stays unified. `view = "auto"` chooses split at 120 columns or more.
The requested mode survives resizes: the effective mode is unified below 100
columns and the requested mode otherwise. `t` toggles the requested mode and
refuses a new split request below 100 columns.
Below 40x10 the TUI draws a single "terminal too small" line.

### 4.3 Keys

| Key | Action | vimeflow binding |
| --- | --- | --- |
| `j` / `k` | next / previous row | `diff-line-next` / `-previous` |
| `Ctrl+d` / `Ctrl+u` | half page, then snap the cursor to the row nearest the viewport centre | `diff-scroll-page-down` / `-up` |
| `[` / `]` | previous / next hunk, landing on its first changed row | `diff-hunk-previous` / `-next` |
| `n` / `p` | next / previous file, wrapping | `diff-file-next` / `-previous` |
| `h` / `l` | deletions / additions side, split mode only | `diff-side-deletions` / `-additions` |
| `t` | unified / split | `diff-view-toggle` |
| `e` / `E` | toggle / pin files panel | `diff-files-toggle` / `-pin` |
| `r` | refresh now | `diff-refresh` |
| `g` / `G` | first / last row | none (added) |
| `H` / `L` | scroll left / right | none (added) |
| `m` | toggle mouse capture | none (added) |
| `?` | key sheet | none (added) |
| `q` | quit | none (added) |

Ctrl+C quits from any state because a raw-mode terminal does not raise SIGINT, and the arrow keys, PageUp/PageDown and Home/End alias j/k/h/l, Ctrl+D/Ctrl+U and g/G.

Bindings come from `vimeflow:src/features/keymap/catalog.ts:44-186`. Phase 1
leaves these keys unbound because later phases use them with vimeflow's
meanings: `s d D` (P2), `i I u U x v y Y @ c /` (P3-P5). herdr's prefix key
(`ctrl+b` by default) never reaches the TUI.

Routing has two layers, as in `herdr-agent-watcher`: an open dialog (the key
sheet) takes every key and `Esc` closes it; otherwise keys go to the main view.
The key sheet is generated from the same table the router uses, and a test fails
if the two differ.

### 4.4 Mouse

Mouse capture is on by default, because G5 requires a clickable toolbar. herdr
forwards SGR mouse events with pane-local coordinates to applications that
enable reporting. Actions: click a toolbar item (same effect as its key), click a
file row to select it, click a diff row to move the cursor, wheel scrolls three
rows. Only a modifier-free left press and the wheel act.

Capture disables the terminal's own text selection, and Phase 1 has no yank. `m`
turns capture off and on at runtime, and `[input] mouse = false` sets the
default. The toggle reuses `herdr-agent-watcher`'s conservative capture lifecycle
(`TerminalGuard::set_mouse`).

### 4.5 Colour and untrusted text

Colours are the terminal's named ANSI colours only: green for additions, red for
deletions, cyan for hunk headers, dim for context numbers and gaps, bold for the
file header, reverse for the cursor. There are no background tints and no RGB, so
light and dark themes both work without detection.

Every string from git is untrusted. Before it reaches a cell the TUI replaces
each C0 control character other than tab with its Unicode control picture
(`U+2400 + code`), and DEL and C1 characters with `U+FFFD`. A diff that contains
escape sequences therefore cannot drive the terminal.
The TUI also replaces bidirectional formatting controls U+202A–U+202E and
U+2066–U+2069 with `U+FFFD` (width 1) to prevent misleading source display,
while preserving U+200E, U+200F and U+061C for legitimate right-to-left prose.

### 4.6 Empty and error states

One centred line each: not a git repository; working tree clean; loading; binary
file or no textual changes (zero hunks); unmerged path (the frozen parser yields
zero hunks for combined diffs, `vimeflow:crates/backend/src/git/mod.rs:929-937`);
status or diff failure with git's message. A failure never clears the last good
file list. The frozen layer's 2 MiB cap applies to the full old and new texts
only, which Phase 1 does not use; hunks come from `raw_diff` and render at any
file size, so there is no large-file state.

A `watcher_error` does not replace the body. It shows as a one-line notice above
the footer: `live refresh degraded: <reason> · polling every 5 s · r retries`.

### 4.7 Configuration

`config.toml` lives in `$HERDR_PLUGIN_CONFIG_DIR`, or in
`${XDG_CONFIG_HOME:-~/.config}/herdr-hunks/` when run standalone. It is parsed key
by key, so one bad value costs one key.

```toml
[view]
mode = "auto"      # "auto" | "unified" | "split"
files = "auto"     # "auto" | "pinned" | "hidden"

[input]
mouse = true
```

### 4.8 View state and reconciliation

`ViewState` is everything the TUI owns that is not in the snapshot: the cursor (an
index into the current target sequence, or none), the vertical and horizontal
offsets, the view mode, the files-panel state, the requested mouse-capture state,
and the open dialog. The cursor's identity is `(side, line_number)`, not its
index, so it can be found again after the rows change.

| Event | Cursor | Offsets |
| --- | --- | --- |
| a different `FileKey` becomes selected | `target_index_for_hunk(0)`, or none when the diff has no targets | both reset to 0 |
| same `FileKey`, new `LoadedDiff` (refresh) | the target with the same `(side, line_number)`; else the nearest `line_number` on that side; else the old index clamped | vertical offset re-anchored so the cursor keeps its viewport row where possible, then clamped; horizontal retained, then clamped to the current rows |
| view mode toggled | same `(side, line_number)` in the other sequence | rows rebuilt, then `ensure_visible` |
| terminal resized | unchanged | both clamped, then `ensure_visible` |
| diff has zero targets | none | both 0; `j k [ ] h l g G` are inert and the hunk stepper reads `0/0` |

The hunk stepper always shows the cursor's `hunk_index`, and the files panel
highlight always follows `snapshot.selected`. The scroll arithmetic
(`clamp_scroll`, `ensure_visible`, `reanchor`) follows `herdr-agent-watcher`'s
`layout.rs`, with one registered adaptation: its content offsets are `u16` and
saturate at row 65,535, which a sidebar never reaches and a diff does. Here
content offsets and row counts are `usize`; only viewport sizes stay `u16`. The
view slices rows itself, so ratatui's `u16` scroll limit never applies.

## 5. Failure modes and testing

### 5.1 Failure modes

| Condition | Behaviour |
| --- | --- |
| stdout is not a terminal | exit 2 with a one-line message; nothing is drawn |
| stdin is not a terminal | exit 2 with `herdr-hunks: stdin is not a terminal`; nothing is drawn |
| `PATH` argument missing or not a directory | the TUI starts and shows the error state, so an overlay pane does not flash and vanish; `q` quits |
| not a git repository | "not a git repository" state; the frozen watcher's pre-repo mode upgrades it when `.git/` appears |
| `git` missing from `PATH` | status error carrying the spawn message; `r` retries |
| git older than 2.31 | fatal start error shown as an error state; no repository command runs (3.3) |
| git call exceeds the frozen 30 s timeout | `status_error` or `DiffState::Failed`; the last good file list stays |
| worktree removed while the viewer is open | refresh errors are shown; no crash; `r` retries |
| the watcher cannot start (for example the inotify watch limit is exhausted) | degraded mode (3.3): `watcher_error` notice, 5 s polling that includes branch and worktree name, `r` retries the watcher |
| slow status on a large repository | input and drawing continue; the toolbar shows a loading mark while `diff` is `Loading` or `refreshing` is set; background refreshes stay silent; triggers coalesce (3.3) |
| very large diff | the engine's size cap (3.2) keeps 200,000 lines; the body ends with a "N more lines not shown" row, and every navigation target lies inside what is shown |
| terminal hangs up (pane closed) | the poll loop sees `POLLHUP` and exits cleanly (`herdr-agent-watcher`'s `poll_terminal`) |
| panic | a panic hook restores the terminal through `TerminalGuard` before printing |
| herdr socket unavailable to `open` / `open-split` | the action exits 1 with a message that lands in `herdr plugin log list`; the standalone TUI is unaffected |
| `split-panes.json` unreadable | treated as empty and overwritten |
| bad `config.toml` value | that key falls back to its default; problems are written to `config-problems.log` in the state directory and the TUI shows a one-line notice |

### 5.2 Test layers

1. **Frozen-tree tests, unchanged.** The inline test modules of `git/mod.rs` and
   `git/watcher.rs` come with the frozen files and run as they are, using the
   copied `git/test_helpers.rs`.
2. **vimeflow integration tests, adapted.** `vimeflow:crates/backend/tests/git_diff_response.rs`
   drives the diff path through `vimeflow_lib::runtime::BackendState`, which is not
   ported. The frozen tree's public wrappers (`git_status`, `get_git_diff`,
   `git_branch`, `git_worktree_name`) are `#[cfg(test)]`-gated
   (`vimeflow:crates/backend/src/git/mod.rs:1106-1108,1435-1437,1858-1860,1867-1869`)
   and the `_inner` functions are `pub(crate)`, so nothing outside the crate can
   call them. The cases therefore move into the crate as a `#[cfg(test)]` module,
   `src/git_diff_response_tests.rs`, with one registered change: each request calls
   `crate::git::get_git_diff` directly. Fixtures and assertions stay.
   `git_staging.rs` exercises the mutating paths and is ported in Phase 2.
3. **Shim tests.** D1: `ensure_within_home` accepts a canonical path outside
   `$HOME`; `reject_parent_refs` still rejects `..`; `expand_home` still expands
   `~`.
4. **Engine tests** against real fixture repositories in temp directories:
   first load selects the first row; selection survives a status change by key and
   then by index; a trigger during a refresh produces exactly one follow-up; a diff
   result for a deselected row is dropped; a directory that becomes a repository
   upgrades from `NotARepo`; publishing is skipped when nothing changed, and a
   branch switch in a clean repository does publish. **D2 and degraded mode:** the
   session is built with the watcher start forced to fail and a 50 ms poll
   interval. `watcher_error` must be set; a modified file is edited a second time
   and the snapshot's `raw_diff` must converge; a branch switch must reach
   `repo.branch` through the tick; a `Refresh` with the failure lifted must clear
   `watcher_error`. **Literal pathspecs:** with `a*.txt` and `ab.txt` both
   modified, selecting `a*.txt` yields only its own hunks. **D4:** with
   `diff.external=/bin/echo` in the fixture's config, a modified file and an
   untracked file still parse to their real hunks. **Background refresh:** a D2
   tick on an unchanged repository publishes nothing, and `diff` never leaves
   `Ready` during a same-key refresh. **D5** is tested inside the patched
   `watcher.rs`: a child that writes 1 MiB to stdout returns its full output well
   inside the timeout. The fixture set
   includes `MM`, `AM`, a staged rename with a worktree edit, a staged deletion,
   and an untracked file inside a new untracked directory (criterion 1).
5. **Navigation tests.** Table tests for `targets_for_diff`,
   `target_index_for_hunk`, `move_line` in both modes (including a replacement
   block with more deletions than additions, and clamping at both ends) and
   `move_side`. The target-shape fixture of the first case in
   `vimeflow:src/features/diff/hooks/useReviewTargetNavigation.test.ts` is carried
   over; that file's other assertions concern comment selection and precedence,
   which belong to P3.
6. **View tests.** Plain-text projections of `Rendered` (no snapshot crate, as in
   `herdr-agent-watcher`): unified and split bodies, gap rows, the toolbar at 120,
   80 and 50 columns, files panel pinned and hidden, every empty and error state,
   split refused below 100 columns, and sanitization of an ESC byte in a diff line
   and in a file name. **Reconciliation (4.8):** a file change resets cursor and
   offsets; a refresh keeps the cursor on its `(side, line_number)` when that line
   survives and moves it to the nearest line when it does not; a mode toggle keeps
   the same line; a resize clamps; a diff with zero targets makes the movement
   keys inert. **Large diffs:** in a diff longer than 65,535 rows, `G` reaches the
   last row and `ensure_visible` holds; in a diff past the size cap, the last
   target is the last kept line and the final row reports the dropped count.
7. **Key-sheet lock.** The key sheet equals the routed key table, and no key
   reserved in 4.3 is bound.
8. **Read-only guarantee (G7).** A recording `git` wrapper is placed first in
   `PATH`; it logs argv and environment and then runs the real git. A scripted
   engine session starts, selects every row, refreshes, takes a D2 tick and handles a
   branch switch. The test asserts three things. Every recorded subcommand is in
   the allow-list `rev-parse`, `status`, `diff`, `ls-files`, `show`, `cat-file`,
   `symbolic-ref`, plus the startup `--version` check, which is the set the frozen
   read paths spawn
   (`vimeflow:crates/backend/src/git/mod.rs:1120-2049`, `watcher.rs:387-491`); any
   other subcommand fails the test, and extending the list is a reviewed change.
   Every recorded environment carries the three D3 variables
   (`GIT_OPTIONAL_LOCKS=0`, `GIT_LITERAL_PATHSPECS=1`, `GIT_NO_LAZY_FETCH=1`). The
   bytes of `.git/index`, the output of `git for-each-ref` and a recursive hash of
   the worktree are identical before and after.
9. **herdr integration, tier A.** A fake herdr socket that enforces object
   `params` and records requests (from `herdr-agent-watcher`'s `tests/support`):
   `open` sends the overlay shape with no `target_pane_id`; `open-split` sends the
   split shape; reuse happens only when the recorded toplevel matches, the socket
   path matches, and `pane.get` reports the viewer's title and recorded cwd; a
   record made under another socket path, a relabelled pane or a failed focus each
   open a new viewer; the host is reached only through `$HERDR_BIN_PATH` or
   the socket. Request parameters are pinned against a fixture of
   `herdr api schema --json` captured from herdr 0.8.0.
10. **herdr integration, tier B (`#[ignore]`).** A real `herdr --session <name>
    server` with isolated `HOME` and XDG directories: link the plugin, invoke
    `open-split`, assert a plugin pane exists with the expected cwd, and assert the
    user's real session files are untouched.
11. **Port parity.** `scripts/port-check.sh` runs in CI (2.2).

### 5.3 Manual acceptance

Success criteria 2-4 are checked by hand before release: open from an agent pane
inside a linked worktree; watch an agent edit and see the view follow; link the
same build into upstream herdr 0.8.0 and into the fork (their plugin registries
are separate) and open it in each.

## 6. Distribution, conventions, and seams for later phases

### 6.1 Distribution

Distribution copies `herdr-agent-watcher`'s, with the repository and binary names
changed.

- **Manifest.** `herdr-plugin.toml` as described in 2.5. `min_herdr_version` stays
  at `"0.8.0"`; a higher value would lock out the fork until it merges that
  upstream release.
- **Build step.** `[[build]]` runs `sh scripts/fetch-or-build.sh`. The script
  reads the version from `Cargo.toml`, maps `uname` to one of four targets,
  downloads `herdr-hunks-<version>-<target>` and `SHA256SUMS` from the GitHub
  release, verifies the hash, and on any failure runs
  `exec cargo build --release`, which needs a Rust toolchain. Wherever cargo is
  available, a missing release asset means a slower install and not a failed one.
  `herdr plugin link` skips `[[build]]`, so local development builds by hand.
- **Release workflow.** Runs on `v*` tags. A guard job fails when the tag differs
  from the `Cargo.toml` version. The matrix is `aarch64-apple-darwin`,
  `x86_64-apple-darwin`, `x86_64-unknown-linux-musl` and
  `aarch64-unknown-linux-musl`. `SHA256SUMS` keeps the two-space format the script
  parses.
- **`update` action.** herdr has no plugin update command. `update` refuses a
  linked or unknown install source. Otherwise it finds the newest `v*` tag with
  `git ls-remote --tags` (so no HTTP client is linked) and re-runs
  `plugin install winoooops/herdr-hunks --ref <that tag> --yes` through
  `$HERDR_BIN_PATH`. Installing a tag, never the branch head, keeps the source in
  step with the release asset `fetch-or-build.sh` downloads. There is no daemon to
  restart, and nothing contacts the network unless the user invokes the action.
- **Versioning.** `Cargo.toml`, `herdr-plugin.toml` and `Cargo.lock` move together;
  a test fails when the manifest and crate versions differ.
- **Runtime requirement.** `git` 2.31 or newer on `PATH`, checked at startup
  (3.3); the frozen watcher needs `rev-parse --path-format=absolute`.
  `GIT_NO_LAZY_FETCH` takes effect from git 2.45; on older versions a partial clone
  may still fetch on read.

### 6.2 Repository conventions

The repository mirrors `herdr-agent-watcher`: `README.md` with `zh-CN` and `ja`
translations in the same section order; `PORT-SURFACE.md`; `AGENTS.md` and
`CLAUDE.md`; specs and plans under `docs/superpowers/`; conventional commits with a
lowercase subject; licence Apache-2.0, as `herdr-agent-watcher`. CI runs `cargo test`,
`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` (inherited clippy
warnings are allowed at the frozen git module declaration),
`cargo check --no-default-features`, `scripts/port-check.sh`, and its self-test.
Port checks emit a skip notice when the pinned reference checkout is unavailable.
The repository carries the GitHub topic `herdr-plugin`,
which is how herdr's marketplace index finds plugins.

### 6.3 Seams Phase 1 leaves for later phases

Phase 1 builds none of the following. It only avoids closing them off.

| Later need | What Phase 1 already provides |
| --- | --- |
| P2 hunk patch slicing (port of `extractHunkPatch`, `vimeflow:src/features/diff/services/gitPatch.ts:65-86`) | `LoadedDiff.raw_diff` is kept, and hunks keep git's own boundaries |
| P2 actions and confirmations | the toolbar slot between the steppers, the unbound keys `s d D`, the dialog layer, an extensible `engine::Command` |
| P2 frozen-tree fixes K1-K5 | the patch mechanism of 2.2 (already exercised by D4 and D5), the divergence registry and `port-check.sh` |
| P3 comment anchors | `(FileKey, side, line_number)` from `engine::nav`, the same coordinates vimeflow's prompt format uses |
| P3 dispatch target | `HERDR_HUNKS_OPENER_PANE` is already passed to the viewer; `herdr/` already wraps the socket |
| P3 persistence | `$HERDR_PLUGIN_STATE_DIR` is already the only place Phase 1 writes (`split-panes.json`, `config-problems.log`) |
| P4 reply detection | nothing in Phase 1 constrains the choice between the plugin's own nonce-scoped transcript scan and events published by `herdr-agent-watcher` |
| P3-P5 sentinel names | undecided; `VIMEFLOW_REPLY` / `VIMEFLOW_REVIEW` allow reuse of the frozen parsers unchanged |

### 6.4 Unscheduled follow-ups

Keybinding installer (port of `herdr-agent-watcher`'s); syntax highlighting;
word-level intra-line diff; context expansion in gap rows.

<!-- codex-reviewed: 2026-09-20T02:13:33Z -->
