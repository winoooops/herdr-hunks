# Hunk actions: staging, discarding and the frozen-tree fixes

Addendum to the Phase 1 design
(`2026-09-18-hunks-roadmap-p1-viewer-design.md`), to branch scope (section 7,
`2026-09-22-branch-scope-design.md`) and to review marks (section 8,
`2026-09-23-review-marks-design.md`). It is section 9: the numbering continues,
and every term (`Scope`, `FileKey`, `LoadedDiff`, `Comparison`, the files
panel, the toolbar slot, the dialog layer, the state directory, the read-only
guarantee G7, the defects K1-K7) means what those documents say it means. This
is Phase 2 of the roadmap of 1.2, shipped as 0.0.4.

## 9. Hunk actions

### 9.1 Why

Phase 1 reads. A reviewer who has read a hunk and wants it in the next commit,
or wants it gone, leaves the viewer for a shell and types a path. Vimeflow's
reviewer does not: `s`, `d` and `D` stage, discard and discard-file from the
keyboard behind a y/n box (`vimeflow:src/features/diff/Panel.tsx`, the keyboard
confirm). This section brings those three keys over, and with them the
frozen-tree fixes the roadmap of 1.2 tied to Phase 2, because the first
mutation the viewer runs is the first time K1-K4 can be reached.

What 3.5 promised changes shape rather than weakening: the viewer still runs
only allow-listed git subcommands, and the one mutating subcommand it gains,
`apply`, runs only from a confirmed key, on a patch sliced from the diff that
was on screen. Nothing else mutates, and 9.6 keeps proving it.

The actions have no meaning in branch scope, where a row is a committed change
against a merge-base (7.7): they bind in `worktree` scope only.

### 9.2 The actions

Three keys, one unit of work. `s` stages the hunk under the cursor when the
row is unstaged or untracked and unstages it when the row is staged; `d`
discards it; `D` discards every change the row shows. Each is applied as a
unified-diff patch piped to `git apply`, the only mutating subcommand this
section adds. The patch is cut from the bytes of the diff the frame drew: what
the reviewer read is what git is given, and the guards under *Stale content*
below refuse a patch whose source has changed since.

**The diff behind the row.** A row is `(path, staged, untracked)` (7.2). In
worktree scope its diff is the index against `HEAD` for a staged row, the
working tree against the index for an unstaged one, and the file against
`/dev/null` for an untracked one (3.2). From this section on the engine builds
that diff's command itself, as 7.2 does for branch scope, instead of calling
the frozen `get_git_diff_inner`:

```
git -C <toplevel> diff [--cached] --no-color --no-ext-diff --no-textconv -U3 --src-prefix=a/ --dst-prefix=b/ [-M] -- [<old>] <path>
git -C <toplevel> diff --no-index --no-color --no-ext-diff --no-textconv -U3 --src-prefix=a/ --dst-prefix=b/ -- /dev/null <path>
```

The first is a tracked row, `--cached` when it is staged; the second an
untracked row, whose exit status `1` means "differs" as the frozen helper
already treats it. `-M` and `<old>` are present when the row is a rename,
found as branch scope finds its renames: one `diff --name-status -M -z --`
per side and per refresh, with and without `--cached`, fills
`rename_sources` in worktree scope too, keyed by `(path, staged)` there,
because the unstaged side can carry a rename as well (a moved file whose
destination was added with `git add -N`). The output is cut into
sections with 7.2's cutter and only the sections naming the row's path, or
its rename source, are kept, before anything is parsed: that is the K7 fix,
and a descendant's patch can neither be shown as the file's nor applied as it.
The kept bytes are `LoadedDiff.patch: Vec<u8>`, the source of every action;
`raw_diff` stays the lossy string the view draws and the fingerprint compares,
and the frozen `parse_git_diff` (D6) parses it as before. Four flags are new
and fixed: `--no-textconv`, because a converted text is not something git can
apply back (D4 is amended in 9.5: a binary file with a textconv driver now
shows as binary and a text file with one shows its raw text, in both
scopes); `-U3`, because a hunk sliced out of a diff without context
carries line numbers that assume the hunks before it and lands in the wrong
place, so `diff.context` is ignored; and the two prefixes, so `diff.noprefix`
and `diff.mnemonicPrefix` cannot change what the slicer or `git apply` parses.
Branch scope's command of 7.2 gains the same `--no-textconv -U3` for
consistency, and nothing else about it changes. The frozen
`get_git_diff_inner` is no longer called; it stays in the tree, tested by
`git_diff_response_tests`, and 9.8 uses it as the oracle the engine's diffs
are compared against.

**The row decides the direction.** Every action is one or two `apply` forms
over a patch taken from that diff:

| Row | `s` | `d` (hunk) and `D` (file) |
| --- | --- | --- |
| unstaged | `apply --cached` stages the hunk: the index moves toward the working tree | `apply -R` reverses it in the working tree; staged content is untouched |
| staged | `apply --cached -R` moves the hunk out of the index; the working tree keeps it, so it reappears on the unstaged row | `apply --index -R` reverses it in the index and the working tree together, or in neither: git refuses the form while the file has unstaged changes (`does not match index`), so the unstaged half is never touched; the notice then says to unstage the hunk first and discard it from the unstaged row |
| untracked | `apply --cached` of the creation patch adds the file to the index | `apply -R` of the creation patch deletes the file: the one unrecoverable action here, and the box says so (9.3) |

On an untracked row all three act on the whole file through its section,
whether or not it has a hunk: an empty file's creation diff is a header
alone, and `apply --cached` of it adds the empty blob all the same.

Every `apply` runs as `git -c apply.ignoreWhitespace=no -C <toplevel> apply
--whitespace=nowarn [--cached | --index] [-R]` with the patch on stdin; the
`-c` pins
the context check that `apply.ignoreWhitespace = change` in a user's config
would otherwise loosen. `<toplevel>` is `RepoState::Repo.toplevel`, the
directory the frozen status and the diffs run in, so the paths in the patch
are the toplevel-relative paths the rows carry: K1 by construction. The exit
status is read, and a non-zero one is the answer, stderr included: K2 by
construction. A staged row's discard runs with `--index`, so the index and
the working tree change together or not at all: K3. `git apply` checks the
whole patch before it writes, so a patch that does not apply changes nothing;
a failure while writing (a full disk, a permission) is reported with git's
own text and the next refresh shows what state the file is in. The forms
compose: `--index -R` of a staged deletion recreates the index entry and the
file, `-R` of an unstaged deletion recreates the file from the index,
`--index -R` of a staged creation removes the index entry and the file. Each
form in the table was run against git 2.55 before this section was written.

**Discarding a file discards what the row shows.** `D` is `d` over the row's
whole patch: on the unstaged row of an `MM` path the staged half is untouched,
and on its staged row git refuses the discard while the file carries unstaged
changes, so the unstaged half is never touched either; the way through is to
unstage first and discard from the unstaged row. Vimeflow's
`DiscardScope::Both` (reset, then
checkout) would also throw away the half the row did not show; it is not
used. A rename row's whole patch carries `rename from`/`rename to`, and
reversing it undoes the rename together with the edits, which is what the row
shows.

**Slicing a hunk.** A hunk patch is its section's file header followed by that
one hunk, as `extractHunkPatch` builds it
(`vimeflow:src/features/diff/services/gitPatch.ts:65-86`), cut from
`LoadedDiff.patch` by lines, with three differences.

1. Sections. The kept text can hold more than one section: a type change
   prints a deletion and a creation for one path. Hunk `i` on screen is hunk
   `i` of the kept text, counted across sections in order, and its patch takes
   the header of the section it is in.
2. Headers. A hunk patch drops the section's `old mode` and `new mode` lines:
   confirming one hunk confirms that hunk, not the file's mode, which only the
   whole-file patch of `D` carries. In a rename or copy section the header is
   also rewritten to name the new path alone: `diff --git`, `---` and `+++`
   are rebuilt from the section's own `+++` line, so git's quoting of the path
   is kept as git wrote it; `similarity index`, `rename from`, `rename to`,
   `copy from` and `copy to` are dropped; `index`, `new file mode` and
   `deleted file mode` lines stay. A hunk of a staged rename then reverses
   that hunk in the index entry of the new name and leaves the rename
   standing: K4, re-verified as the brief asked (the row reads `RM`
   afterwards, the rename survives with its other hunk, and the reversed hunk
   reappears on the unstaged row). Other header lines are kept verbatim.
3. Coordinates. The `@@` line of a hunk cut out of a multi-hunk diff names a
   new-side start that assumes the hunks before it were applied, and
   `git apply` begins matching there, so a context that repeats at that
   offset would be edited instead of the one the reviewer read. The sliced
   header is therefore recounted as `git add -p` recounts it: both sides
   start at the pre-image line, `-start` for a forward form and `+start` for
   a reverse one, with the line counts unchanged.

**What cannot be acted on.** On a tracked row `s` and `d` need a hunk under
the cursor: with no cursor, a diff that is `Loading` or `Failed`, or a row
with zero hunks (a binary file, a mode-only change), the notice reads `no
hunk under the cursor` and nothing is sent; `D`, and every key on an
untracked row, needs only a `Ready` diff. A diff the size cap of 3.2 cut short
(`truncated_lines > 0`) takes no action at all, `D` and an untracked `s`
included, because part of it was never on screen: `diff cut by the size cap;
use a shell`. Every whole-file action, `D` and the keys of an untracked row,
is refused on a row whose patch carries `Binary files` or `GIT binary patch`
with `binary file: not applied here`, because the displayed diff carries no
payload git could apply. A row whose section is a submodule
pointer (`Subproject commit`) is refused with `submodule: not applied here`,
because `git apply` would ignore the pointer in the working tree and only
unstage it in the index. In branch scope every key shows `switch to worktree
scope (b) to stage or discard`.

**Stale content.** Three guards, each stricter than the last. The command
carries the `Arc<LoadedDiff>` the key was pressed against (9.4), and the
engine acts only while that `Arc` is the diff it last published and no form
has run on it; otherwise it answers `the diff changed; look again`, and the
newer content is on screen by then. Second, the diff task records the
pre-image of what it read: the index entry's blob id from
`git ls-files -s -- <path>`, or its absence for an untracked row, and the
working-tree path's kind from `lstat` with what that kind holds: a regular
file's bytes hash, a symlink's target bytes, or the bare kind for a
directory, anything else, an absence or a path that could not be read.
Neither read moves `HEAD`, so the bracket of 8.2 says nothing about them; the task
brackets the diff with them instead, reading both before and after the diff
and keeping the pre-image only when the two readings agree. A diff whose
readings disagree is published without a pre-image, which refuses every
action on it with the same notice until the next reload. The hash is one the
engine computes; it detects edits, not forgeries, so a cryptographic one is
not required. Immediately before each form, and again before each retry of
9.4, the engine reads the same two things and refuses with the same notice
when the one that form applies to differs: the blob, or its absence, for a
`--cached` form; the file, or its absence, for a working-tree one; both for
an `--index` form, whose working-tree file must be present whenever the
index still holds an entry for it, which the engine checks itself because
git would otherwise recreate the missing file from the index and so erase an
unstaged deletion (an `MD` path), while a clean staged deletion has no entry
and `--index -R` restores it; a present file must equal its index entry,
which git checks itself. A working-tree form runs only on a
regular file, a symlink or an absence; a directory or anything else is
refused with `not a regular file: not applied here`, and a path that could
not be read leaves the diff without a pre-image. The bytes a form is
applied to are then the bytes the
patch was cut from, so the recounted coordinates match in place, and git's
search for context, which could otherwise settle on an identical block
elsewhere in the file, never runs. One window stays: an edit landing between
the engine's check and git's own read of the file. Git's context check is
the third guard and covers it as far as any patch tool can: a patch whose
context does not match is refused with git's message and nothing changes.

**After the action**, the refresh that carried it (9.4) reloads the rows and
the selected diff. When the acted row is gone, because its last hunk was
staged, unstaged or discarded, the selection moves to the other row of the
same path when one exists (the staged row after a stage, the unstaged row
after an unstage), and otherwise 3.3's index rule applies. The cursor follows
4.8: the same line when it survives, the nearest when it does not.

### 9.3 Keys, the box, the toolbar and the footer

Three bindings join the table of 4.3, each with Vimeflow's own key and
meaning:

| Key | Action | vimeflow binding |
| --- | --- | --- |
| `s` | stage / unstage the hunk under the cursor | `diff-hunk-stage` |
| `d` | discard the hunk under the cursor | `diff-hunk-discard` |
| `D` | discard the file: every change this row shows | `diff-file-discard` |

The reserved set of 4.3 shrinks to `i I u U x X v y Y @ c /`, which the
review loop that follows (section 10) takes together with `A`; `y` and `n`
stay unbound in the main view (`n` keeps "next file") and are the box's keys
only. The key sheet
grows by the same three rows, and the lock of 5.2 item 7 moves with it.

**The box.** Each key opens a confirmation in the dialog layer of 4.3, the
layer the key sheet and the picker use: a panel of at most 60 columns centred
on the body, with a title, a body of at most two lines, a warning row when
the action is unrecoverable, and the footer `y yes · n no`. The path is
shortened from the left with `…` until the body fits two lines at the
panel's inner width, and the warning wraps there to at most two; with the
four lines the dialog's frame takes (two borders, the footer and its rule)
the box is at most eight lines tall, which the 40x10 minimum of 4.2 holds,
so all of it is always drawn: nothing scrolls, and `y` never confirms a
question that was only half shown. It opens only from the main view, so it never
shares the screen with the key sheet or the picker. While it is open `y`
confirms, `n` cancels, and every other key and every mouse event is inert:
the box has to be answered.
`Ctrl+C` still quits, as 4.3 says it does from any state. `y` confirms only
on a frame that drew the whole box, recorded the way `drawn_head` is (8.2):
below the 40x10 minimum nothing but the size notice is drawn, and there `y`
is inert while `n` still cancels. The text names the action, the hunk's
position from the hunk stepper and the row's path (sanitized, 4.5):

| Row, key | Title | Body | Warning row |
| --- | --- | --- | --- |
| unstaged, `s` | `Stage hunk?` | `Stage hunk 2/3 of src/values.ts?` | none |
| staged, `s` | `Unstage hunk?` | `Move hunk 2/3 of src/values.ts out of the index?` | none |
| tracked, `d` | `Discard hunk?` | `Discard hunk 2/3 of src/values.ts?` | `This cannot be undone.` |
| tracked, `D` | `Discard file?` | `Discard every change this row shows for src/values.ts?` | `This cannot be undone.` |
| untracked, `s` | `Stage file?` | `Add src/values.ts to the index?` | none |
| untracked, `d` or `D` | `Delete untracked file?` | `Delete src/values.ts?` | `It is not in git and cannot be recovered.` |

The refusals of 9.2 are decided before the box opens: a key that cannot act
shows its notice and no box.

**Confirming.** `y` closes the box and sends `Command::Act` (9.4) carrying the
`Arc<LoadedDiff>` the box was opened on, the hunk index and the kind. From
then until the engine has answered and, when a form ran (`action_applied`,
9.4), until the diff on screen is no longer the `Arc` that was sent, `s`,
`d` and `D` show `an action is still running` and open nothing; the
toolbar's busy mark `…` shows while the carrying refresh runs, because the
engine sets `refreshing` for it. The answer rides on the snapshot as
`action_seq`, `action_error` and `action_applied` (9.4), and `observe` turns
it into a notice: ordinary on success (`staged hunk 2/3
of src/values.ts`, `unstaged hunk …`, `discarded hunk …`, `discarded
src/values.ts`, `staged src/values.ts`, `deleted src/values.ts`) and urgent
on refusal (`stage failed: <git's reason>`, `discard failed: …`; when git's
reason is `does not match index`, the notice ends with `; unstage it first
(s)`). The answer to a
confirmed action is shown at once, exactly as the answer to `M` is in 8.5:
invariant 3's one exception becomes "the answer to the key the user just
pressed", and what it displaces comes back by the mechanism `notice_kind`
names.

**The toolbar.** The slot 4.2 reserved between the two steppers takes one
group of three chips: ` stage ` (` unstage ` on a staged row, ` stage ` on an
untracked one), ` discard ` and ` discard file `. Each chip routes through the
same `KeyAction` as its key, so a click opens the same box, and hovering it
shows the key sheet's text (`discard hunk · d`) in the footer as 4.2 does for
every chip. The group is drawn in worktree scope only. Its drop order is 8,
after every Phase 1 item: when the row is too narrow it is the first thing
dropped, because its keys remain. The chips are enabled while the diff is
`Ready` and dim otherwise, with no hit region when dim (4.4); a refusal 9.2
decides from the content, such as a binary row, is still shown as the key's
notice on click.

**The footer** of 4.2 gains `s stage  d discard  D file` before `? help`, in
worktree scope only, dropping from the right with the rest when narrow.

**Branch scope.** `s`, `d`, `D` and the chips are absent from the toolbar,
and the keys show the notice of 7.7, `switch to worktree scope (b) to stage
or discard`, and send nothing.

### 9.4 The engine

**The command.** `Command::Act(Action)`, where

```rust
pub enum ActionKind { Stage, Discard, DiscardFile }   // what the key means; the row gives the direction (9.2)

pub struct Action {
    pub kind: ActionKind,
    pub diff: Arc<LoadedDiff>,   // the diff the box was opened on
    pub hunk: Option<usize>,     // the hunk under the cursor; None for DiscardFile and for every untracked row
}
```

`Stage` on a staged row is the unstage of 9.2. The engine resolves kind and
row into the apply forms of 9.2's table, slices the patch from `diff.patch`,
and runs the forms in order; the first refusal ends the sequence and is the
answer.

**Carried by one refresh.** An `Act` is queued as a `Change`, like a scope
switch, a pick or a mark (3.3, 7.2, 8.2): the next refresh pops it, runs the
apply forms before its status read, then loads the rows as always, so the
rows published with the answer are the ones git shows after the mutation;
the selected row's diff follows in a publication of its own, as after every
refresh (3.3), and the key guard of 9.3 covers the gap. `refreshing` is set
while it is carried, which is what
the toolbar's `…` of 9.3 shows. At most one refresh is in flight, so no
status read overlaps a mutation; a diff task can, and the paragraph on
supersession covers it.

**Answers.** `Snapshot` gains `action_seq: u64`, `action_error:
Option<String>` and `action_applied: bool`, all in `fingerprint`;
`action_seq` advances exactly once per `Act` received, with the error and
whether any form ran beside it, as `mark_seq` does for `M`. Before any form
runs the engine decides the refusals itself, trusting nothing the TUI
checked: `the diff changed; look again` when `Arc::ptr_eq(action.diff,
published diff)` fails or a form already ran on that `Arc` (checked when the
command arrives and again when the refresh that carries it pops it from the
queue, because a diff result can land in between), and when the pre-image of
9.2 differs; the eligibility rules of 9.2 re-applied to the `Arc`
(`Comparison::Worktree`, a hunk index inside the kept hunks, the cap, binary,
submodule, the kind against the row), answered with the notice text the TUI
would have shown; `an action is still running` when another `Act` is queued
or in flight (the TUI already refuses this case, 9.3; the engine answers it
so every `Act` has exactly one answer); and `not a git repository` when the
snapshot has no toplevel. Once a form runs, its result is the answer, with
`action_applied` true: `None` or git's reason. A status read that fails
afterwards does not change it, and is shown as 3.6 shows a status error.

**Supersession.** Any action that ran a form, whether it succeeded or not,
increments `diff_generation` and clears `diff_in_flight`, the way a changed
`Comparison` does: a diff read before the mutation is never published after
it. The `Arc` the forms ran on is remembered as acted on, so no second `Act`
can name it, until a diff published later replaces it; 3.3 keeps it on
screen as `Ready(old)` meanwhile, which is why the TUI holds its keys (9.3).
The refresh that carried the action requests the selected row's diff again
on publication. 3.3 keeps `Ready(old)` in place and would keep the old `Arc`
when the reload reads the same bytes, which after a refused form it does;
the first diff after an attempted action therefore always becomes a fresh
`Arc`, so a failed action never leaves the keys of 9.3 held, and from this
section on the unchanged shortcut compares the pre-images too, because a
working-tree edit can change a staged row's pre-image without changing its
diff. The selection rule of 9.2 runs in that refresh's `Done::Status`
handler, before the index rule of 3.3. The watcher sees the index and the
working tree change and triggers a refresh of its own, which coalesces with
the one in flight (3.3).

**The runner.** `engine::actions::apply(toplevel, flags, patch: &[u8])`
spawns `git -c apply.ignoreWhitespace=no -C <toplevel> apply
--whitespace=nowarn <flags>` with `tokio::process::Command`:
`GIT_TERMINAL_PROMPT=0`, the D3 variables inherited from the process, stdin
piped and closed after the patch is
written, stdout and stderr captured, `kill_on_drop`, and the frozen 30 s
timeout with a kill on expiry (`git apply timed out after 30s`). A non-zero
exit answers with the first non-empty line of stderr, or `git apply failed
with status <n>` when there is none. One retry rule: when stderr names
`index.lock`, the form is retried every 100 ms for up to 2 s, the bound
`save_mark` uses for its own lock, because an agent committing in the same
worktree holds that lock for a moment; the pre-image of 9.2 is checked again
before each retry, because whoever held the lock may have changed the file. `GIT_OPTIONAL_LOCKS=0` (D3) does not
apply to `apply --cached`: the index write needs the lock, and the retry is
what absorbs the collision.

**The diffs.** A new module `engine::worktree` builds the two commands of 9.2
and is the sibling of `engine::branch`: the tracked form with `--cached` for
a staged row and `-M <old>` for a staged rename, and the no-index form for an
untracked row, which `branch::untracked_diff` now calls too. Both keep the
output bytes, cut the sections with the cutter of 7.2 (moved where both
modules reach it), parse the kept text with the frozen `parse_git_diff`, and
return `LoadedDiff.patch` beside `raw_diff` and `file_diff`, together with
the pre-image of 9.2: before and after the diff the task runs
`git -C <toplevel> ls-files -s -- <path>`, a read already on the allow-list,
and hashes the working-tree file's bytes, and it keeps the pair only when
both readings agree. `load_rows` in
worktree scope adds `git -C <toplevel> diff --name-status -M -z --` with and
without `--cached` to its refresh and fills `rename_sources` from their `R`
records with 7.2's `parse_name_status`, keyed by `(path, staged)`, so
`request_diff` passes `old` to the diff task in both scopes the same way. The frozen `get_git_diff_inner` is no longer called from
the engine (9.5).

**K6.** Diff tasks run git one at a time: each task waits on a one-permit
gate held for the task's git call, and a task that acquires it after its
generation was superseded returns without spawning (counted in
`diffs_discarded`). Holding `n` across slow diffs then costs at most one git
process at a time instead of one per keypress. The bracket of 8.2 opens once
the gate is held, so the opening sample and the diff it vouches for are never
separated by a wait; the `diff_delay` seam sits inside the bracket as before.
The `diff_gate` test seam stays as a second gate the tests open permit by
permit.

### 9.5 The frozen tree

What this section changes in `src/git/` is one patch; everything else is a
sibling in the engine. The registry in `PORT-SURFACE.md` gains the rows
below, and `scripts/port-check.sh` keeps passing.

| Defect | Where it was | How 0.0.4 fixes it |
| --- | --- | --- |
| K1, paths relative to the pane's cwd | the frozen mutators (`mod.rs:369,393,437-462`) | not reached: every form of 9.2 runs with `-C <toplevel>` on toplevel-relative paths |
| K2, exit status discarded | `run_git_with_timeout`'s callers (`mod.rs:370,395,438-463`) | not reached: the runner of 9.4 reads the status and answers with stderr |
| K3, per-hunk discard ignores its scope | `mod.rs:426-428,466-469` | not reached: a staged row's discard is the `--index -R` form of 9.2, index and working tree together |
| K4, unstaging one hunk of a rename reverses the rename | the reused patch header | the slicer of 9.2 rewrites a rename section's header; re-verified against git 2.55 |
| K5, `MD` and `AD` fall to the default arm | `parse_git_status` (`mod.rs:883-892`) | **patch `0004-status-two-halves.patch`**: the default arm splits `XY` into up to two rows, a staged one for `X` and an unstaged one for `Y`, with `M` and `T` as `Modified`, `A` as `Added` and `D` as `Deleted`; a letter outside those keeps the old single unstaged `Modified` row, and the conflict arms before it are untouched. `MD`, `AD`, `T `, ` T`, `TM` and `MT` are the codes this reaches. Its test lives in `mod.rs`'s test module, as D5's does in `watcher.rs` |
| K6, superseded diffs pile up git processes | the engine | the gate of 9.4 |
| K7, a descendant's patch parsed as the file's | `get_git_diff_inner`'s pathspec | not reached: the engine cuts sections before parsing (9.2); the frozen function keeps the defect and is no longer called |

The frozen mutators `stage_file_inner`, `unstage_file_inner` and
`discard_file_inner` stay as they are, copied and uncalled, and the sentence
in `PORT-SURFACE.md` that says so stays true. `get_git_diff_inner` joins
them: the engine's list of called frozen functions loses it and otherwise
stands (D6's five `pub(crate)` functions are all still used;
`parse_name_status` is the engine's own). `git_diff_response_tests` keeps
testing it, because it is the oracle of 9.8.

Two divergences are registered. **D7 (hunk actions): engine-built diffs.**
Every diff the viewer shows is built by the engine with the four fixed flags
of 9.2, in both scopes; the reasons (bytes an action can apply back, no
textconv, three lines of context, fixed prefixes) are recorded there. **D4 is
amended** by it: textconv filters are no longer left on; a binary file with
a textconv driver shows as binary and is refused by 9.2's binary rule, and a
text file with one shows its raw text. D4's
patch itself is unchanged, because the frozen calls it touches still exist.

The guarantee text of 3.5 and 7.6 is restated in 9.6. The version becomes
0.0.4 in `Cargo.toml`, `Cargo.lock` and `herdr-plugin.toml`, whose
`description` and the README's first line stop saying read-only; a README
section "Hunk actions" documents the three keys, the box, the direction table
of 9.2 in prose, and the one mutating subcommand, in the three languages of
6.2.

### 9.6 The guarantee: read-only except on confirmation

G7 is restated rather than dropped. The viewer runs only these git
subcommands: the ten of 7.6 (`--version rev-parse status diff ls-files show
cat-file symbolic-ref merge-base for-each-ref`), all reads, and `apply`.
`apply` runs only inside the refresh that carries a confirmed `Act` (9.4):
never from a poll, a watcher event, a scope switch, a pick, a mark or a plain
`r`. Everything git is given is one argv element or the bytes on stdin, and
the only text that names a revision still comes through `base::check_text`,
`base::verify` or `base::is_object_id` (7.3, 8.2); a patch is bytes git read
out of the repository moments before, cut by lines and rewritten only in the
header of 9.2.

Two tests carry it. `tests/readonly_guarantee.rs` keeps its scripted session,
which never confirms anything, and keeps its allow-list at ten: a session
without an `Act` must never record `apply`. It additionally asserts that
every patch-producing `diff` the session records in either scope, one
without `--name-status`, `--numstat` or `--name-only`, carries
`--no-textconv -U3 --src-prefix=a/ --dst-prefix=b/`, and that the worktree
refresh records `diff --name-status -M -z --` with and without `--cached`,
so D7's commands are covered by the recording and not by convention. The state directory's file list is
unchanged: this section writes no new file.

A new `tests/hunk_actions.rs` uses the same recording wrapper and the same
before/after measurements (the index bytes, `for-each-ref`, the worktree
hash) on a fixture that holds every row kind of 9.2's table, and proves
exactly what each action changes: for every confirmed `Act`, the recorded
`apply` invocations are the forms of 9.2 with their flags and nothing else
mutating ran; the refs are identical before and after every action; and the
index and the worktree differ afterwards in exactly the way the table says,
checked through the real git's `status --porcelain=v1` and `diff` output.
Every refusal of 9.2 and 9.4 that is decided before git records no `apply`.
9.8 lists the cases.

### 9.7 Failure modes, cost and configuration

Additions to the tables of 5.1, 7.8 and 8.6. Every refusal decided before git
leaves the index, the working tree and the rows untouched, and runs no
`apply`:

| Condition | Behaviour |
| --- | --- |
| `s`, `d` or `D` in branch scope | `switch to worktree scope (b) to stage or discard`; nothing is sent |
| `s` or `d` on a tracked row with no hunk under the cursor | `no hunk under the cursor`; no box |
| the row's diff is cut by the size cap, binary, or a submodule pointer | the notice of 9.2; no box |
| a key while an `Act` is unanswered, or answered as applied while the diff it acted on is still on screen | `an action is still running`; no box |
| the diff was replaced between the frame and `y`, or between `y` and the refresh that carries it; a form already ran on it; or the pre-image read before the form differs from the one read with the diff | `the diff changed; look again`; no form runs; the newer diff is on screen or arrives with the next refresh |
| an `Act` fails 9.2's eligibility in the engine (a branch-scope `Arc`, a hunk index out of range) | the notice the TUI would have shown; no form runs |
| the terminal shrinks below 40x10 while the box is open | the box is not drawn; `y` is inert and `n` cancels; it is drawn again when the terminal grows |
| the context no longer matches when git checks the patch (an edit landed between the diff read and the apply) | git refuses; the urgent `<verb> failed: <git's first stderr line>`; nothing changed; the next refresh shows the current content |
| the file of a staged row has unstaged changes when `d` or `D` is confirmed | git refuses the `--index` form before writing: `discard failed: <path>: does not match index; unstage it first (s)`; nothing changes |
| the file of a staged row is missing from the working tree (`MD`) when `d` or `D` is confirmed | the engine refuses the `--index` form with the same notice before git, which would otherwise recreate the file; the file stays absent |
| the working-tree path is a directory or another non-file when a working-tree form would run | `not a regular file: not applied here`; no form runs |
| the diff's two pre-image readings disagreed (an edit landed during the read) | the diff is shown without a pre-image; every key on it answers `the diff changed; look again` until the next reload |
| `index.lock` is held by another process | the form is retried every 100 ms for up to 2 s, then answers with git's message |
| `git apply` exceeds 30 s | killed; `git apply timed out after 30s`; nothing changed when git was still checking, and the next refresh shows the file's state otherwise |
| a write fails midway (a full disk, a permission) | git's message; the next refresh shows the file's state |
| no toplevel (not a repository, unusable path) | the action is answered `not a git repository` and no form runs |
| the status read fails after the forms ran | the forms' result is the answer; the status error is shown as 3.6 shows it and `r` reloads the rows |
| git missing from `PATH` | the spawn error is the answer, as 5.1 |
| `HEAD` moves while an action is carried | nothing special: an action touches the index and the working tree, and the rules of 8.2 decide what is markable as before |

Files under `core.autocrlf` or clean/smudge filters are converted by git's own
`apply`, the same conversion `git add -p` relies on; the viewer adds nothing
of its own.

Cost. Per refresh in worktree scope, two `diff --name-status -M -z --` more
than 3.3 ran, one per side, and per selected row one `diff --name-status`
fewer, because the frozen diff probed renames per diff and the engine now
knows them per refresh. Per confirmed action, one or two `apply` and the
refresh that carries them, which the watcher's own trigger coalesces into
(3.3). The gate of 9.4 adds no command and removes concurrent ones. Branch
scope runs what 7.5 and 8.6 say it runs.

Configuration. No new keys. The viewer's diffs and applies override five git
settings on purpose, for the reasons 9.2 gives: `diff.context`,
`diff.noprefix`, `diff.mnemonicPrefix`, textconv drivers (D4 as amended) and
`apply.ignoreWhitespace`; `apply.whitespace` is overridden by
`--whitespace=nowarn`, and `diff.external` stays off (D4). `diff.algorithm`
is honoured and shapes the hunks the reviewer reads and applies alike.
Rename detection is always on, whatever `diff.renames` says: the refresh's
`--name-status -M` finds a rename on either side and the row's diff passes
`-M` with both endpoints, as the frozen status and diff already did.

The version becomes 0.0.4.

Boundary with section 10, the review loop that follows. Its keys stay
reserved (9.3, `X` and `A` included). Its boxes follow this section's `y`/`n`
convention, so the box of 9.3 is an ordinary `dialog::Panel` with its key
handling in one place that a later box reuses, not something wired to
staging alone. The body rows stay what 4.8 builds, and nothing in this
section assumes that every body row is a diff target, because section 10
adds rows of another kind between them. The toolbar group of 9.3 is the
first item dropped, so a later group can take the space. The table above has
the shape of 7.8 and 8.6, which section 10's own table extends.

### 9.8 Tests and success criteria

Test layers, added to 5.2, 7.9 and 8.7:

1. Slicer, table tests on byte fixtures: one hunk out of three carries its
   section's header and only that hunk; a rename section's header is
   rewritten from its `+++` line, a quoted path included, and the rename
   lines are gone; a type change's two sections count hunks in order and each
   hunk takes its own header; a hunk patch drops `old mode`/`new mode` while
   the whole-file patch keeps them; coordinates are recounted for a forward
   and for a reverse form; an empty-file section has no hunk and is still a
   whole-file patch; `Binary files`, `GIT binary patch` and `Subproject
   commit` are detected; the cutter keeps only the row's sections on the K7
   fixture (`D tools` beside `A tools/run`).
2. Runner: a patch that applies, one git refuses (the answer is stderr's
   first line), a missing git (the spawn error), a fake git that sleeps past
   the timeout (killed within the bound, no process left), `index.lock` held
   and released within 2 s (succeeds) and held longer (git's text), a 1 MiB
   patch on stdin (applies; the D5 lesson in the other direction).
3. Engine, diffs (D7): on the status fixture of 5.2 item 4 every worktree
   row's `file_diff.hunks` equals the frozen `get_git_diff_inner`'s for the
   same key, the oracle; `patch` is byte-identical to the test's own
   `git diff` with the same flags; a Latin-1 file's `patch` keeps its bytes
   while `raw_diff` carries U+FFFD; `rename_sources` holds the staged rename;
   the K7 fixture yields one section; a binary file with a textconv driver
   yields zero hunks and a text file with one yields its raw hunks;
   `diff.noprefix=true` and `diff.context=0` in the fixture's config change
   nothing in `patch`; the pre-image blob and hash are the ones
   `git ls-files -s` and the file give at that moment, a symlink's is its
   target, a directory's is its kind, and an edit landed between the two
   readings leaves the diff without one; a moved file whose destination was
   added with `git add -N` is an unstaged rename row with `-M` and its
   source.
4. Engine, actions, through `Command::Act` against real fixtures: every row
   of 9.2's table (stage, unstage and discard a hunk; `D` on an unstaged row;
   `d` and `D` on a staged row of a clean file, and on one with unstaged
   changes, the latter refused by git with nothing changed, and on an `MD`
   path, refused by the engine with the file still absent; a working-tree
   form on the K7 directory refused as not a regular file; stage and delete
   an untracked file, an empty one included, and a binary one refused;
   `D` on a staged creation; `d` on a working-tree deletion; `s` and `D` on a
   staged deletion; one hunk of a staged rename, K4; a type change), each
   checked against the real git's status and diffs afterwards; `action_seq`
   advances exactly once per `Act`, including every answer decided without
   git; an edit landed between the diff read and the key that moves the
   block while an identical block stays is refused by the pre-image check
   before git and the other block is untouched, for a `--cached` and for a
   working-tree form; a branch-scope `Arc` and an out-of-range hunk index are
   refused by the engine with no TUI involved; a second `Act` on an acted-on
   `Arc` is refused until the new diff is published, and `action_applied` is
   true only when a form ran; a refused form on unchanged content still
   publishes a fresh `Arc` that frees the keys; a diff result read before the
   action is discarded and the first diff published after the answer is one
   read after it; the selection rule when the acted row is gone; `refreshing` while
   carried; a mark queued behind an action and an action behind a mark both
   answer in order.
5. Engine, K6: with a slow diff (`diff_delay`), five selections in quick
   succession reach git at most twice, the task in flight and the last
   requested; the ones in between take no opening sample and count as
   discarded; the bracket still invalidates a diff during which `HEAD` moved.
6. Frozen status, patch 0004: `MD`, `AD`, `T `, ` T`, `TM` and `MT` records
   each produce the rows of 9.5, inside `mod.rs`'s test module; the status
   fixture of `git_diff_response_tests` gains `MD` and `AD`.
7. View: the toolbar group present at 120 columns and the first item dropped
   at 80, ` unstage ` on a staged row, absent in branch scope, dim while
   `Loading` with no hit; the footer hints in worktree scope only; each box of
   9.3 by title and body, the warning row, the path shortened from the left
   at 40 columns, eight lines at 40x10 with the warning wrapped to two,
   `body_is_drawn` false while it is
   open; the three key-sheet rows and the reserved set of 9.3 (the lock of
   5.2 item 7).
8. Input: each key opens its box only from the main view with the right diff
   state, and every refusal of 9.2 shows its notice without one; `y` sends
   `Act` with the drawn `Arc`, the hunk index and the kind, `n` sends
   nothing, every other key and mouse event is inert while open, `Ctrl+C`
   quits; `y` is inert on a frame that did not draw the box and `n` still
   cancels; a key while unanswered shows `an action is still running`, and
   so does one after an applied answer until the diff on screen changes,
   while a refused answer frees the keys at once; the branch-scope notice;
   `observe` turns each answer into its notice with the verb of 9.3, urgent
   on error, and an answer over an unread warning or base error puts it back
   as 8.5's cases do.
9. Read-only, 9.6: `readonly_guarantee.rs` with its allow-list at ten and the
   D7 flag assertions; `hunk_actions.rs` recording every `apply` form, the
   unchanged refs, the exact index and worktree differences, and no `apply`
   on any refusal.
10. Tier B (ignored, real host): in worktree scope on a fixture with one
    unstaged hunk, the test presses `s` then `y` and waits for the row to
    move to the staged side, in both hosts.
11. Port parity: `port-check.sh` with four patches; the selftest unchanged.

Success criteria, in addition to 1.5, 7.9 and 8.7:

12. On an `MM` file with two hunks on each side, `s` on one unstaged hunk
    moves exactly that hunk to the staged row, `s` on the staged row moves it
    back, `d` removes it from the working tree and leaves the other three,
    `d` on a staged hunk is refused with the unstage hint while the file has
    unstaged changes and discards exactly that hunk once it has none, each
    step compared with `git diff` and `git diff --cached`.
13. `D` on an untracked row deletes the file after its box and `n` leaves
    everything; in branch scope the keys show the notice and change nothing;
    a hunk whose context the agent edited between the frame and `y` is
    refused and nothing changes.
14. On the real TUI, from a pane in a subdirectory (K1), a staged rename with
    edits (K4), an `MD` path (K5) and a `tools` replaced by `tools/run` in
    the index (K7) behave as this section says.
15. `docs/acceptance-p1.md` gains row 9 with the same evidence columns; the
    release guard is unchanged.

<!-- codex-reviewed: 2026-10-01T10:59:03Z -->
