# Branch scope: reviewing what the agent already committed

Addendum to the Phase 1 design
(`2026-09-18-hunks-roadmap-p1-viewer-design.md`). It is that document's
section 7: the numbering continues from section 6, and every term (scope of
the read-only guarantee G7, the frozen tree and its registered divergences,
`Snapshot`, `FileKey`, the toolbar chips, the popup) means what the Phase 1
design says it means. Section 1.4 of that design listed base-branch comparison
as outside the roadmap; this addendum withdraws that line for base-branch
comparison. Commit log and branch operations stay outside.

## 7. Branch scope

### 7.1 Why

Phase 1 lists what `git status` lists: the working tree and the index against
`HEAD`. An agent that commits as it goes empties that list. On a linked
worktree where the agent has committed two files and then edited one of them
again, the viewer shows one row; after the next commit it shows "working tree
clean", although nothing has been reviewed. The changes are on the branch, not
in the working tree.

Branch scope shows a second thing: the working tree against the point where
the branch left its base. That is every change the branch carries, committed
or not, in one diff per file, which is what a reviewer wants to read before
the branch is merged. `herdr-reviewr` calls the same view its `branch` scope;
lazygit reaches a similar view through its diffing mode (7.4 borrows its
prompt). The default base is `main`. The user can compare against anything
else that names a commit.

Two scopes exist:

| Scope | Rows | One row's diff |
| --- | --- | --- |
| `worktree` (Phase 1, the default) | `git status`: staged, unstaged and untracked rows; a partly staged path is two rows | working tree or index against `HEAD`, as in 3.2 |
| `branch` | every path that differs between the working tree and the merge-base of `HEAD` and the base, plus untracked paths; one row per path | working tree against that merge-base |

The staged/unstaged distinction does not exist in branch scope: both halves
of a partly staged file are in the working tree, and both are on the branch.

### 7.2 Semantics

The base is a ref or revision `B` that names a commit. Branch scope compares
against `M = merge-base(HEAD, B)`. The engine computes `M` once per refresh
with `git merge-base HEAD <C>` (`<C>` is the base's object id, 7.3) and gives
every row and diff command of that refresh the same `M`. `git diff
--merge-base` would recompute `M` in every command against the live `HEAD`,
so a commit or checkout in the middle of a refresh could give the rows and a
diff different baselines; a pinned `M` cannot. `merge-base` is read-only and
joins G7's allow-list (7.6); the diffs themselves stay `diff`.

Rows. The engine runs, in the same refresh as the Phase 1 status:

```
git -C <toplevel> merge-base HEAD <C>
git -C <toplevel> diff <M> --name-status -M -z --
git -C <toplevel> diff <M> --numstat -M -z --
```

`<C>` is `Base.commit` and `<M>` the merge-base the first command printed,
both resolved in this same refresh, so the rows, the diffs and the displayed
base agree even while the ref moves; each is one argv element and is never
interpolated into a shell. The trailing `--` makes git read the id as a
revision even when a file of the same name exists (`git diff main` fails with
`ambiguous argument` next to a file called `main`). Both diff commands pass
`-M` so a rename's counts describe the rename, not a deletion and an
addition. Records of
the first command are `<status>\0<path>\0` and, for `R`/`C`, `<status>\0<old>\0<new>\0`.
They map onto the frozen `ChangedFile` as follows: `A` -> `Added`, `D` ->
`Deleted`, `M` and `T` -> `Modified`, `R` -> `Renamed` with `path = new`, `C`
-> `Added` with `path = new`, `U` -> `Modified`; `staged` is `false` for every
row. The frozen `ChangedFile` has no field for a rename's source and
`Renamed` is a unit variant, so the engine keeps the sources beside the rows:
`Snapshot.rename_sources: Arc<BTreeMap<String, String>>` maps a renamed row's
`path` to its `<old>`; it is empty in worktree scope, where the frozen code
finds the source itself. The numstat records supply `insertions` and `deletions`, keyed by the
new path for renames, exactly as the frozen `parse_numstat` keys them.
Untracked rows come from the Phase 1 `git status` result and are appended
unchanged. A path can be in both lists: the branch deleted `f` and the user
recreated `f` without adding it, so the merge-base list says `D f` (git
compares only tracked paths, so the untracked content is invisible to it) and
the status says `?? f`. Both rows are shown: `f` deleted on the branch, and
`f` untracked, each with the diff its command produces. They are two facts,
and the state is transient (adding `f` turns them into one `M f` row at the
next refresh). To keep the two rows distinct, `FileKey` gains a third field,
`untracked: bool`, derived from `status == Untracked`; in worktree scope the
pair `(path, staged)` was already unique per row, so nothing there changes.
Rows are sorted by path, untracked rows included, so the list is stable across
refreshes.

A row's diff. For a tracked row:

```
git -C <toplevel> diff <M> --no-color --no-ext-diff --src-prefix=a/ --dst-prefix=b/ [-M] -- [<old>] <path>
```

`<M>` is the merge-base pinned by this refresh. `-M` and `<old>` (looked up
in `rename_sources`) are present when the row is a rename; both endpoints
must be
in the pathspec for the unified header to carry `rename from`, the same
reason the frozen code re-runs its diff with two paths (2.4, K5's neighbour).
A pathspec matches its descendants too, so when the branch replaced the file
`tools` with the directory `tools/run`, `git diff <M> -- tools` prints both
patches, and the frozen `parse_git_diff` would read the second file's headers
as content of the first. The engine therefore cuts the output into its
`diff --git` sections and keeps every section whose header names the row:
`b/<path>` (`a/<path>` for a deletion; `a/<old>` and `b/<path>` for a
rename). The prefixes are forced with `--src-prefix=a/ --dst-prefix=b/`, so
`diff.noprefix` and `diff.mnemonicPrefix` in the user's config cannot change
them, and a header path that git quoted (spaces, non-ASCII, quotes) is
decoded with the frozen `decode_git_patch_path` (D6, 7.7) before comparison.
A type change (a file replaced by a symlink, or the reverse) is printed as
two sections for one path, a deletion and a creation; both are kept, each is
parsed on its own by the frozen `parse_git_diff`, and their hunks are
concatenated in order into one `LoadedDiff` through the Phase 1 size cap
(3.2). Sections naming other paths are dropped. An untracked row uses the Phase 1 path unchanged:
`get_git_diff_inner(cwd, path, false, Some(true))`, which diffs against
`/dev/null` with `--no-index`. (The frozen worktree-scope diff has the same
exposure for a staged row whose path became a directory; it is registered as
K7 in 7.7 and left to Phase 2 with the other frozen-tree defects.)

`FileKey` is `(path, staged, untracked)` from now on; branch rows have
`staged = false`. The snapshot carries the scope and the base:

```rust
pub enum Scope { Worktree, Branch }

pub struct Base {
    pub requested: String,   // exactly what git is given: "refs/heads/main" for a picked or resolved
                             // branch, the typed text for free text and config ("HEAD~3", "origin/main")
    pub commit: String,      // full object id of `requested` as of the last refresh (7.3)
    pub merge_base: String,  // merge-base(HEAD, commit) the current rows and diffs were computed against
    pub source: BaseSource,  // Picked | Config | Default
}

impl Base {
    /// What the chip and the picker show: `requested` without a leading
    /// `refs/heads/`, `refs/remotes/` or `refs/tags/`.
    pub fn label(&self) -> &str { ... }
}

pub enum BaseSource { Picked, Config, Default }

#[derive(PartialEq)]
pub enum Comparison { Worktree, Branch { merge_base: String } }

// Snapshot gains:
pub scope: Scope,
pub base: Option<Base>,          // None when nothing resolved (7.3)
pub base_error: Option<String>,  // the last failed resolution or pick, for the dialog and the notice
pub rename_sources: Arc<BTreeMap<String, String>>,

// LoadedDiff gains:
pub comparison: Comparison,      // what the hunks were computed against
```

A worktree row and a branch row of the same path share a `FileKey`, and 3.3
keeps a Ready diff whose key survives a status result. That rule now compares
comparisons as well: a status result whose `Comparison` differs from the
Ready diff's puts `diff = Loading` even though the key survived, and the diff
generation of 3.3 is bumped on every comparison change (scope switch, base
change, merge-base moved), so a diff result computed under the previous
comparison is discarded like any stale generation. A diff request records the
comparison it was issued under and the result carries it back.

`Command` gains `SetScope(Scope)`, `SetBase(Option<String>)` (`Some`:
validate, load the branch rows under it, then persist and publish; `None`:
forget the pick, re-resolve, load, publish) and `LoadRefs` (7.4). A snapshot's `scope`, `base`, `files` and
`rename_sources` always describe one comparison: the engine publishes a new
scope or base only together with the first row list loaded under it. Until
that list arrives the previous snapshot stays, with `refreshing = true`; if
loading fails, the previous scope, base and rows stay, `status_error` (or
`base_error` for a failed pick) carries the reason, and the notice of 7.8
shows it. The last good rows are therefore never labelled with a comparison
they were not computed under. Selection follows 4.8's rules with one
addition: on a scope switch the engine keeps the selected `path` when the new
scope lists it (preferring the unstaged row when switching to `worktree`
scope and both rows exist) and otherwise selects the first row. The view
treats a scope switch like a new `FileKey` (cursor to the first changed row,
offsets reset) unless the path survived, in which case the `(side,
line_number)` reconciliation of 4.8 applies.

Corner cases that are behaviours, not errors:

- `B` is the branch checked out (the user works on `main` and the base is
  `main`): `M = HEAD`, and branch scope shows the working tree against `HEAD`
  in a single row per path. The chip still reads `vs main`.
- `B` is behind `HEAD` on the same line (the base is `origin/main` and the
  branch is `main` with unpushed commits): branch scope shows exactly the
  unpushed work plus the working tree. This is the intended way to review an
  agent that commits straight to `main`.
- Detached `HEAD`: works; `HEAD` is a commit like any other.
- Unrelated histories (no merge-base): `git merge-base` fails; the
  failure is a status error in branch scope (7.8) and the last good rows stay.

### 7.3 Resolving, validating and remembering the base

The engine resolves the base at session start and whenever the scope, the
config or the pick changes, in this order, taking the first that names a
commit:

1. The pick remembered for this worktree (below).
2. `[base] ref` from `config.toml` (4.7 gains the key).
3. `refs/heads/main`, when `git rev-parse --verify --quiet refs/heads/main^{commit}`
   succeeds.
4. The remote default branch: the target of
   `git symbolic-ref --quiet refs/remotes/origin/HEAD`, used as printed
   (`refs/remotes/origin/<name>`).
5. `refs/heads/master`, by the same check as 3.

Steps 3-5 and the picker (7.4) always give git a fully qualified ref, because
a short name is ambiguous when a branch and a tag share it, and git then
prefers the tag (`gitrevisions`). Only free text (a typed revision, or
`[base] ref` in the config) is passed as written; the README tells users to
write `refs/heads/x` when their repository has a tag of the same name.

Steps 1 and 2 are validated the same way as 3; a value that fails validation
is skipped with a `base_error` naming it, and resolution continues. When
nothing resolves, `base = None`: branch scope is unavailable, `b` shows the
notice `no base branch: set [base] ref or press B` and the scope stays
`worktree`; `B` still opens the picker (7.4), and a successful pick makes
branch scope available. The resolver's git calls are `rev-parse` and
`symbolic-ref`, both on the allow-list.

Validation of any base, whichever step supplied it, is
`git -C <toplevel> rev-parse --verify --quiet <B>^{commit}`, with `<B>` as one
argv element. A `B` that starts with `-` is rejected before git sees it, so a
typed value can never become an option. The object id it prints is
`Base.commit`; the chip and the picker show `Base::label()`, the acceptance
note and the status line show `commit` shortened to seven characters.

Two different things are resolved at two different rates. The preference,
which `requested` names, is resolved by the steps above at session start and
when the scope, the config, the pick or the checked-out branch changes. The
object id behind it, `Base.commit`, is refreshed by the same `rev-parse` on
every refresh in branch scope, and `Base.merge_base` by `git merge-base HEAD
<commit>` right after it, before the row commands run; the row and diff
commands of 7.2 take that merge-base, so a ref that moved between two
refreshes changes the rows, the diffs and the displayed ids together. When
the refresh's `rev-parse` or `merge-base` fails (the ref was deleted, or the
histories are unrelated), the refresh reports the error as in 7.8 and keeps
the last rows.

Remembering a pick. A pick made with `B` is stored as its qualified ref (or
the typed text) in `<state dir>/bases.json`
(the state directory of 4.7, absolute or nothing, as for `split-panes.json`),
as a map from the worktree's canonical path (`git rev-parse --show-toplevel`
of the viewed directory) to the `requested` string. The read-modify-write is
done under the same non-blocking, bounded-retry file lock that guards
`split-panes.json` (`reuse::with_lock`), because the map is shared by every
worktree of every repository that uses this state directory, and two viewers
picking at once must not lose each other's entry. The file is then replaced
atomically (temp file and rename, as `reuse::save` does). It is written only on
a pick; resolution never writes. When the lock is busy for its whole retry
window, or the state directory is unusable, the pick is kept in memory as a
session override, `session_pick: Option<Option<String>>` (`Some(Some(ref))`
for a pick, `Some(None)` for a reset), which resolution consults before step
1 for the rest of the session, so `r` and scope switches cannot restore the
stale entry in `bases.json`; the picker's footer says the pick was not
remembered. A later pick that does persist clears the override. Another
viewer's saved update is seen only by a viewer without an override, at its
next resolution. Picking the picker's first row, the reset row of 7.4, removes the
entry, so the worktree returns to steps 2-5. Without a usable
state directory the same session override applies. Two viewers on the same worktree see each other's pick at their next
resolution, which happens on their next refresh of the base (a scope switch
or `r`), not on every poll.

The base moves. `B` is a ref, and refs move: someone merges into `main`, or
`git fetch` advances `origin/main`. The watcher does not report ref changes
other than `HEAD`, and the engine does not add one. Branch scope relies on the
Phase 1 content poll (2.4 D2): every refresh re-resolves `Base.commit` and
re-runs the merge-base commands against it, so a moved base is reflected
within one poll interval, the same bound the Phase 1 design gives an edit the
watcher missed.

### 7.4 Keys, toolbar and the picker

herdr's key grammar is `prefix+<one chord>`; a three-key sequence is not
expressible in `config.toml`. The gesture the owner asked for, `prefix` `f`
`b`, is therefore two layers: `prefix+f` is the host's binding for
`open-split` (it opens the viewer, or focuses the one already open for this
opener pane, 2.5), and `b` is a viewer key. The same holds for the popup:
`prefix` `d` `b`.

Two keys are added to 4.3. Neither is in the reserved set of 4.3
(`s d D i I u U x v y Y @ c /`).

| Key | Action | vimeflow binding |
| --- | --- | --- |
| `b` | switch scope: `worktree` <-> `branch` | none (added) |
| `B` | compare against: open the base picker | none (added) |

`b` with no base resolved shows the notice of 7.3 and stays in `worktree`.
The key sheet gains both rows (23 rows; it scrolls, 4.3).

Toolbar. One chip is added after the view chip: `worktree` in worktree scope,
`vs main` in branch scope, where the name is `Base::label()` truncated to 16
cells with the ellipsis of 4.2. Clicking it acts like `b`. It is drawn
disabled (dim, no hit region, 4.4) when `base` is `None`, and its hover hint is
`switch scope · b`. In branch scope the `STAGED`/`UNSTAGED` label is not
drawn, because the rows have no such half; the file stepper, hunk stepper,
stats, `files` and refresh chips are unchanged. The chip sits left of the view
chip and outlives it when the toolbar is narrow (4.2's drop order, where the
highest number drops first: it takes drop order 2, the view chip moves to 3).

The picker (`B`) is modelled on lazygit's diffing prompt (`W` / `ctrl+e`,
"Enter ref to diff"): one input line that both filters a list and accepts
free text. It is a dialog drawn with the Phase 1 dialog primitive (4.1), 60
columns wide at most, over the body like the key sheet:

```
 Compare against                                   (title)
 > ma_                                             (input line; `_` is the caret)
   default (main)                                  (reset row: always first, never filtered)
   main                    current                 (rows: label, then markers)
   origin/main
   maint-2.1
 Enter pick · Esc cancel · type to filter          (footer)
```

- Rows come from `Command::LoadRefs`, which the engine answers by running
  `git for-each-ref --sort=-creatordate
  --format=%(refname)%00%(symref) refs/heads refs/remotes refs/tags` and
  publishing `refs: Option<Arc<Vec<String>>>` on the snapshot: the qualified
  names, entries whose `%(symref)` is non-empty (such as
  `refs/remotes/origin/HEAD`) removed, most recently created first.
  `creatordate` is the committer date of a commit and the tagger date of an
  annotated tag, so tags sort by when they were made rather than dropping to
  the end. Each row displays the short label (`refs/heads/x` -> `x`,
  `refs/remotes/o/x` -> `o/x`, `refs/tags/v` -> `v`) and submits the
  qualified name. The command is not capped at the git side; the engine
  removes the symbolic entries first, keeps the first 200 of what remains and
  sets `refs_overflow = true` when more remained, which the footer reports
  (7.8) with the true count of shown rows. `for-each-ref` is read-only and is added to G7's allow-list (7.6).
  The list is loaded when the picker opens and discarded when it closes; it
  is not refreshed while open.
- The first row is the reset row, `default (<label>)`, naming what steps 2-5
  of 7.3 would resolve to, or `default (none)` when they resolve to nothing.
  It is never filtered out. Picking it sends `SetBase(None)`: the remembered
  pick for this worktree is removed and the base re-resolved; when that yields
  nothing, the scope returns to `worktree` with the notice of 7.3.
- When the input is not empty and is not exactly a listed label, a typed row
  `use "<input>"` appears right after the reset row, so free text is always a
  row of its own (lazygit shows the typed value the same way). The cursor
  follows the input: it moves to the first match when there is one, else to
  the typed row; `Down`/`Up` move it from there, the reset row included.
- The list below the reset row is filtered by the input as a case-insensitive
  substring match on the label; an empty input shows every row. The row of the
  current base, when it matches, comes right after the reset row and is marked
  `picked` or `config` by its source; the checked-out branch is marked
  `current`; every tag row is marked `tag`, so a branch and a tag that share a
  name (`release` and `release` · `tag`) stay distinguishable while their
  labels are the same. Rows are never disabled.
- Keys inside the picker: printable characters and `Backspace` edit the input;
  `Down`/`Up` and `Ctrl+n`/`Ctrl+p` move the cursor (`j`/`k` are text here);
  `Enter` picks the row under the cursor, whichever kind of row it is; `Esc`
  closes without a change. A mouse click on a row picks it;
  the wheel scrolls the list. `Ctrl+C` quits the viewer, as it does from every
  state (4.3). Every other key is inert.
- A pick of a listed or typed row is `Command::SetBase(Some(text))`. The
  engine validates as in 7.3, then loads the branch rows under the new base;
  on success it remembers the pick and publishes the base together with those
  rows, in branch scope (picking a base means wanting to see it). On failure
  it publishes `base_error` and nothing else changes; the picker stays open
  showing the error under the input line, for example `not a commit:
  maint-2.1x`.

The picker is the third modal after the key sheet and the "terminal too
small" notice: while it is open, body keys and toolbar hits are inert, as for
the key sheet (4.3).

### 7.5 Live refresh in branch scope

Nothing changes in the trigger paths of 3.3. A commit moves `<git_dir>/HEAD`
and the watcher reports `git-head-changed`, which refreshes with the head;
edits report `git-status-changed`. In branch scope a refresh runs the Phase 1
status commands and the two merge-base commands of 7.2 together, in one
spawned task, so `refreshing` covers both, and the row list is published once.
The preference behind the base (7.3's steps) is re-resolved only when the head
refresh reports a changed branch name, on `r`, and on scope, config or pick
changes; `Base.commit` is refreshed on every refresh, so a poll in branch
scope costs one `rev-parse` more than in Phase 1 and never lists refs.

Cost. Branch scope adds three git processes per refresh (`merge-base`,
name-status and numstat) and, for a rename row, one two-path diff, on top of
Phase 1's three.
The coalescing rules of 3.3 apply unchanged: a burst of watcher events is
still one refresh. The size cap of 3.2 applies per row; the row list itself
is not capped, as in Phase 1.

### 7.6 Read-only guarantee

Branch scope adds two subcommands to the allow-list of 3.5, both read-only:
`merge-base` (the pinned baseline of 7.2) and `for-each-ref` (the picker's
candidates). Resolution uses `rev-parse` and `symbolic-ref`, already listed.
The list becomes `--version rev-parse status diff ls-files show cat-file
symbolic-ref merge-base for-each-ref`. The G7 test (5.2 item 8) is extended: it loads every row in
branch scope, opens the picker's ref list, picks a different base and switches
back, and asserts the same three things as before (every subcommand on the
list, the D3 environment on every child, and the index, refs and worktree
unchanged), plus that everything the viewer wrote lies under the state
directory it was given and that, once the viewer has exited, that directory
holds only `bases.json` and the lock file of 7.3 (the temporary file of the
atomic replacement has been renamed away).

The state directory gains a second file the viewer writes, `bases.json`,
under the same rule as `split-panes.json` and `config-problems.log`: it is
written only under an absolute state directory, never relative to the
repository (4.7).

### 7.7 Boundaries with the frozen tree and with Phase 2

Frozen tree. The merge-base commands are built by the engine, not by the
frozen `get_git_diff_inner`, which hard-codes its two bases (working tree or
index against `HEAD`). The engine needs four functions of the frozen module
that are private today: `run_git_with_timeout` (the 30 s timeout and the
D3-compatible spawn), `parse_git_diff`, `parse_numstat`,
`decode_git_patch_path` and `validate_file_path`. Divergence D6 is a
registered patch that changes their
visibility to `pub(crate)` and nothing else; it is listed in
`PORT-SURFACE.md` and checked by `port-check` like D4 and D5. No frozen
behaviour changes, and the frozen tests are unaffected. One frozen defect is
newly registered: K7, a staged row whose path was a file and is now a
directory (`D tools` next to `A tools/run` in the index) gets both patches
from `git diff --cached -- tools` and parses the second as content of the
first; rare, worktree scope only, fixed with K1-K6 in Phase 2 (branch scope
isolates the section in the engine, 7.2).

Phase 2. The write actions of Phase 2 (stage, unstage, discard a hunk or a
file; keys `s d D` and their whole-file forms) operate on the working tree
and the index against `HEAD`. They have no meaning for a change that is
already committed, and "discarding" a committed change would rewrite history,
which is a different feature. Phase 2 therefore binds its keys in `worktree`
scope only. In `branch` scope those keys show one notice, `switch to worktree
scope (b) to stage or discard`, and change nothing. The scope keys `b` and `B`
are outside Phase 2's reserved set, so the two features never share a key.
Phase 3's comments and dispatch apply in both scopes; a comment anchored in
branch scope carries `Base.merge_base` next to the path and line, so the
agent knows which baseline the reviewer read the hunk against. The working
tree side is whatever it is when the agent looks, as for a worktree-scope
comment.

Phase 1 behaviour is untouched: with `[view] scope` unset the viewer opens in
`worktree` scope and everything in sections 1-6 holds as written. The version
becomes 0.0.2 (the owner keeps 0.0.x until the write actions of Phase 2 ship; a new
viewing dimension, no incompatible change).

### 7.8 Failure modes and configuration

Additions to the table of 5.1:

| Condition | Behaviour |
| --- | --- |
| no base resolves (no pick, no config, no `main`, no `origin/HEAD`, no `master`) | `b` shows `no base branch: set [base] ref or press B`; scope stays `worktree`; `B` works |
| the remembered pick or `[base] ref` no longer names a commit | that step is skipped with `base_error` naming it; the next step resolves; the notice shows the error once |
| the base disappears while in branch scope (branch deleted, ref pruned) | the next refresh's `rev-parse` fails; shown as a status error with the last good rows kept (3.6); `b` returns to `worktree`; `r` retries |
| unrelated histories (no merge-base) | same as the row above, with git's message |
| a typed base fails validation | the picker stays open and shows `not a commit: <text>`; nothing changes |
| `for-each-ref` fails or the repository has no refs | the picker opens with an empty list and the footer `no refs listed; type a revision`; free text still works |
| `bases.json` is unreadable or malformed | treated as empty; a problem line goes to `config-problems.log`; the next pick rewrites it |
| more than 200 selectable refs (`refs_overflow`) | the list shows the 200 most recently created; the footer says `200 shown, more exist · type to filter`; free text reaches any ref |

Configuration (4.7 gains two keys):

```toml
[view]
scope = "worktree"   # "worktree" | "branch": the scope at start; "branch" falls back to "worktree" with the notice when no base resolves

[base]
ref = "main"         # any revision; step 2 of 7.3
```

Both are parsed key by key like the rest of 4.7; a bad value costs that key.
A README section "Branch scope" documents the two scopes, the two keys, the
picker, the resolution order and the two config keys, in the three languages
of 6.2.

### 7.9 Tests and success criteria

Test layers, added to 5.2:

1. Engine, on a fixture with `main`, a feature branch with one committed
   change, one committed-then-edited file, one committed rename and one
   untracked file: worktree scope lists the edit and the untracked file;
   branch scope lists all four rows once each, the rename's `rename_sources`
   entry and `file_diff.old_path` naming the old path, and the untracked row
   present; each branch row's diff is the working tree against the merge-base
   (the committed-then-edited file shows both changes in one diff); after
   `git add -A && git commit` in the fixture, worktree scope is empty and
   branch scope lists the same four paths, the former untracked row now an
   `Added` row (a different key, since `untracked` flipped). A fifth case
   deletes a committed file on the branch and recreates it untracked: branch
   scope shows the `Deleted` and the `Untracked` row with distinct keys.
2. Engine, resolution: each step of 7.3 in isolation and in precedence order,
   using an injected config and a temporary state directory; a value that
   fails validation is skipped and reported; `SetBase` validates, loads, then
   persists and switches scope, and a commit with unrelated history (valid,
   no merge-base) is reported and not persisted; picking the default clears
   the persisted entry; a `-`-led value is rejected without spawning git; a
   diff result from the previous comparison arriving after a scope switch is
   discarded, and a surviving key is reloaded under the new comparison.
3. Engine, refs: `LoadRefs` lists local and remote branches and tags most
   recent first, without `origin/HEAD`, capped at 200.
4. View: the scope chip in both scopes and disabled; `STAGED`/`UNSTAGED`
   hidden in branch scope; the picker's rendering, filtering, markers,
   caret, error line and the empty-list footer; the key sheet's two new rows.
5. Input: `b` toggles or shows the notice; `B` opens the picker; picker keys
   as listed in 7.4, including that `j` types a letter; the Phase 2 reserved
   keys are inert in both scopes.
6. Read-only: the G7 extension of 7.6.
7. Tier B (ignored, real host): the existing `open-split` test also presses
   `b` on a fixture with a committed change and asserts the branch row appears
   through a real herdr, in both hosts.

Success criteria, in addition to 1.5:

6. On a branch with committed and uncommitted changes, branch scope lists
   every changed path once (twice only for a path that is both deleted on the
   branch and recreated untracked) and each diff equals, hunk for hunk: `git
   diff <M> -- <path>` for a tracked row, `git diff <M> -M -- <old> <new>`
   for a rename, and `git diff --no-index -- /dev/null <path>` for an
   untracked row, where `M` is `git merge-base HEAD main` (compared through
   the engine's parsed form in test 1 and by hand in the acceptance row).
7. The base is `main` with no configuration on a repository that has one, and
   a pick made with `B` survives closing and reopening the viewer on the same
   worktree.
8. `docs/acceptance-p1.md` gains rows 6 and 7 with the same evidence columns;
   the release guard is unchanged.

<!-- codex-reviewed: 2026-09-22T15:27:00Z -->
