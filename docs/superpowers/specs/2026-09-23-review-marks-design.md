# Review marks and quick bases

Addendum to the Phase 1 design
(`2026-09-18-hunks-roadmap-p1-viewer-design.md`) and to its branch-scope
section 7 (`2026-09-22-branch-scope-design.md`, shipped as 0.0.2). It is
section 8: the numbering continues from section 7, and every term (`Scope`,
`Base`, `Comparison`, the picker, the chip, the files panel, the state
directory, the read-only guarantee G7) means what those two documents say it
means. Nothing here adds a scope or changes what a scope compares; this
section marks what has already been read inside the branch scope that exists,
and removes the typing from three bases that are entered by hand today.

## 8. Review marks and quick bases

### 8.1 Why

Branch scope answers "what does this branch carry". The question its owner
asks more often is "what has arrived since I last looked". Between two
readings the agent adds commits; the row list grows, and nothing on the screen
separates the rows that are new from the rows that were read an hour ago.

A review mark is one commit, remembered per worktree: the commit the reviewer
had read up to when they pressed `M`. The viewer then marks, in the files
panel, every row that a commit has touched since -- the branch list stays
whole, so the reviewer keeps the context of everything the branch carries, and
the dots say where to look. Pressing `M` again clears every one of them,
because a mark is a commit and the dots count commits: uncommitted work the
agent is still doing does not raise a dot (8.3 says why), and is read where it
always was, in the row's own diff.

Narrowing to only the new rows stays available and stays optional: the picker
of 7.4 offers the mark as a base (`reviewed (a1b2c3d · 12 min ago)`), and
choosing it compares against the mark instead of against `main`, which empties
the list whenever there is nothing new. Annotation is the default because it
keeps the whole branch on screen; narrowing is one `B` away when the branch is
large enough that the whole of it is noise.

The same section removes the typing from the three bases that are otherwise
entered by hand on every repository: the branch's upstream, the last commit,
and the last three. They become rows in the picker of 7.4, computed from
`rev-parse`, so the allow-list of 3.5 and 7.6 does not grow.

### 8.2 The mark

A mark is a commit and a time: the object id the viewer had drawn as the head
of the worktree when `M` was pressed, and the time the engine recorded it. It
is per worktree, it is at most one, and it is not a base: it changes no
comparison, so a mark that is rewritten, pruned or nonsense can never stop the
rows from loading. That independence is the difference between this section
and the base of 7.3, and most of 8.6's failure table follows from it.

`M` is bound to `Command::MarkReviewed(commit)`. It is not in the reserved set
of 4.3 (`s d D i I u U x v y Y @ c /`). Reading it as "mark" next to `m` for
the mouse toggle follows the viewer's existing pairs that share a letter
without sharing a meaning (`h`/`l` move between the diff's sides, `H`/`L`
scroll).

**Which commit is marked.** The mark must name a commit whose changes the
reviewer has actually seen, or the rows that arrived with it would never be
flagged and would be read as old. Three rules together decide it.

*The head is sampled first and used, not re-resolved.* The snapshot gains
`head: Option<String>`, the full object id of `HEAD`. Every refresh resolves
it with `git rev-parse --verify --quiet HEAD^{commit}` (allow-listed since
Phase 1), not only a refresh that carries `with_head`: an ordinary commit
advances `refs/heads/<branch>` and leaves `<git_dir>/HEAD` holding the same
`ref: refs/heads/<branch>`, so the watcher's head detection -- which compares
that file's contents (`watcher.rs`, "HEAD only moves on commit/checkout") --
does not fire, and a `with_head`-only rule would leave the markable commit
permanently behind on the branch an agent is committing to. (The same fact
corrects one sentence of 7.5: after a commit it is `git-status-changed`, from
the index write, that refreshes the branch rows; `git-head-changed` follows a
checkout. The shipped behaviour is what 7.5 describes; only its explanation of
which event fires was wrong.) The sampled id is then used in place of the
symbol `HEAD` by every command of that refresh that needs it -- 7.2's pin
becomes `git merge-base <head> <base commit>`, and 8.3's commands take it too
-- so the rows a frame shows are the rows of the id that frame carries. The
same `rev-parse` runs once more when the job's commands are done, and the job
offers its id only when the two agree, which is the cheap detector for the
half that cannot be pinned: the status read, which asks git about the worktree
and the index against whatever `HEAD` is live.

*The markable id travels with the content, not beside it.* A snapshot's
`head` is the id **its own content corresponds to**, and nothing else:

- with a `Ready` diff, the id the diff task reports (below);
- with an empty row list, the id the refresh sampled, whose two samples agreed;
- while the diff is `Loading` or `Failed`, `None` -- this snapshot shows no
  content to acknowledge.

The diff task brackets its work: `git rev-parse --verify --quiet HEAD^{commit}`
before its diff call and again after, and it reports an id only when the two
agree, `None` otherwise. That makes the report mean "the content in this
result was read while `HEAD` stood at this commit", which is what a mark
needs, and it holds whether the call was branch scope's own command (8.3, one
command) or worktree scope's frozen helper (3.2, which runs a patch command
and then several `cat-file` and `show` reads before returning): a longer
interval only makes the two samples disagree more often, which costs an
acknowledgement, never truth. `LoadedDiff` carries the reported id, so it
cannot be separated from the hunks it belongs to.

That is the whole mechanism, and it replaces the one this section first
proposed -- a pending candidate promoted by a matching diff generation. A
candidate is a claim about which refresh a diff belongs to, and every ordering
it assumed between a diff task, another process's `reset` and the next refresh
had to be defended separately. An id carried by the content defends itself:
there is no refresh to associate, nothing to invalidate when `HEAD` moves, and
a diff requested before a commit and completed after it reports where it
actually read.

*What the terminal drew is what can be marked.* Publication is still not the
terminal: the shell drains every queued snapshot and draws only the last, so a
snapshot can be skipped. `ViewState` therefore gains
`drawn_head: Option<String>`, assigned after each frame from the `head` of the
snapshot that frame used, and only when the frame showed the review body --
not the "terminal too small" notice of 5.1, the key sheet of 4.3 or the picker
of 7.4, each of which covers the diff completely. A modal frame leaves it
alone, and a snapshot whose `head` is `None` clears it: whatever was drawn
before, the viewer is no longer showing content it can vouch for. `M` marks
`drawn_head`, and with nothing drawn -- no commit yet on an unborn branch, not
a repository, or no frame yet whose diff was loaded -- it shows the notice
`nothing to mark: no commit yet` and sends nothing, as `b` does without a base
(7.3).

The promise is therefore narrow and checkable: **`M` never marks a commit
newer than the last frame this viewer drew with its diff loaded.** If a commit
lands between that frame and the press, the mark is the older id and that
commit's files are flagged as new at the next refresh: the error is always
toward showing an already-read change again, never toward hiding an unread
one.

**The boundary.** One thing cannot be pinned: the working tree is not a
commit, and `git diff` reads it live; worktree scope's rows come from the
frozen path of 3.2, whose `git diff --cached` names live `HEAD` and takes no
sampled commit, so the pinning above is branch scope's alone. A process that
rewinds the tree and restores it while a refresh runs -- `reset --hard` to an
ancestor and back inside one job -- can make a frame draw other content, or an
empty list, under an id whose comparison is otherwise correct, with both
samples agreeing, and nothing detects it. The viewer holds no lock on the
repository and cannot close that window. What it does instead is bounded and
stated here: in branch scope every frame's comparison is derived from the id
that frame carries, a frame whose diff never loaded acknowledges nothing, a
diff during which `HEAD` moved reports no id at all, and the next refresh
redraws whatever the tree settled on. A rewind that does not come back is a
rewrite, which 8.3's warning reports; a rewind that comes back inside one
refresh is the one case where this section's promise rests on no other process
rewinding history under the reader, and one more `M` corrects the mark.

History need not move at all for the tree to mask a commit: an editor can put
a file back to its pre-commit content, and the row's diff -- the working tree
against the base -- then shows nothing of what that commit did, while `M`
still acknowledges it. That is not information the viewer hid. The diff a
frame draws is always the current truth about the tree, and a masked change is
a real absence of difference, not a change waiting to be read; the dot of 8.3
is computed between commits, so that file carries one anyway and says which
commit touched it. What `M` acknowledges is "the state I was shown at this
commit", and that is exactly what was shown.

**What the engine does.** `MarkReviewed(commit)` is short, because no
comparison changes:

1. Validate the id with `git rev-parse --verify --quiet <commit>^{commit}`. It
   can fail if the commit was rewritten between the frame and the press; then
   nothing is written and the answer carries
   `mark_error = "not a commit: <7 hex>"`, leaving the previous mark alone.
2. Write `marks.json` under the state directory of 4.7, inside the
   `reuse::with_lock` critical section the picks already use and replaced
   atomically: a map from the worktree's canonical toplevel (`git rev-parse
   --show-toplevel`, as 7.3 keys `bases.json`) to
   `{"commit": "<full object id, as git printed it>", "at": <seconds since the
   Unix epoch>}`. The id is stored verbatim; nothing assumes 40 characters, so
   a SHA-256 repository stores its 64. `bases.json` is not touched: the mark
   is not a base.
3. Publish the mark with the unread set of 8.3 recomputed against it, and show
   the notice `marked <7 hex> as reviewed` -- the only feedback in worktree
   scope, where no row changes.

`at` is read from the system clock in step 2: it is when the mark was written,
which is what the age of 8.5 measures. It is not the time of the press, and no
bound is claimed between the two -- the command waits behind whatever
refreshes are queued, and each of its git calls carries the frozen 30 s
timeout. Defining `at` at the write keeps the clock out of the pure input
layer.

**Where the mark lives while the viewer runs.** `Snapshot` gains
`head_seen: Option<String>` beside the gated `head`: the id the last refresh
observed, published as observed, with no gate. It is what the unread set and
the ancestry classification of 8.3 are computed against, and what tells the
view that a head change happened at all -- the gated `head` cannot, since a
refresh whose diff keeps failing leaves it unchanged -- and, being part of the
snapshot, it also keeps 3.3 from suppressing the publication of a head change
that altered nothing else. When the head `rev-parse` runs and reports no
commit, `head_seen` becomes `None`, a snapshot with no content to acknowledge
carries `head = None` by the rule above, and the frame that draws it clears
`drawn_head`; the only effect is `M` refusing.

`Snapshot` also gains `mark: Option<Mark>`:

```rust
pub struct Mark {
    /// A full object id: hexadecimal only, of the length this repository's hash gives.
    pub commit: String,
    /// Seconds since the Unix epoch, recorded when the mark was written.
    pub at: u64,
    pub state: MarkState,
    /// The `head_seen` `state` was successfully classified against; `None` while unclassified.
    pub classified_at: Option<String>,
}

pub enum MarkState {
    /// An ancestor of `head_seen`: 8.3's unread set is the one the diff computed.
    Current,
    /// Still a commit, no longer on this branch: every row is flagged.
    Rewritten,
    /// The unread command failed; the string is git's reason. Every row is flagged.
    Unreadable(String),
}
```

A stored `commit` is checked before it reaches any command, and by shape, not
by asking git: it must be non-empty and hexadecimal, of the length the
repository's hash function gives (40 or 64). A record that fails is treated as
absent, with a problem line in `config-problems.log` as for a malformed file.
The check is what keeps a hand-edited or corrupted `marks.json` from turning
into an argument: `--output=tracked.txt` is a valid JSON string and a valid
`git diff` option that writes a file, and the trailing `--` of 8.3's command
protects paths, not the arguments before it. `check_text` of 7.3 would reject
that particular value for its leading dash, but the shape check rejects every
value that is not what `rev-parse` printed, which is the invariant this file
is supposed to hold.

The record is read from `marks.json` in the same step that reads
`bases.json` -- whenever the preference is resolved (session start, `r`, a
pick, a mark, a scope change, a branch change) -- and carried forward on the
refreshes in between, like the rest of the snapshot. A successful `M`
publishes what it wrote without re-reading. When the write fails, the intended
mark is kept in memory for the rest of the session and wins over the file, the
same rule and the same lifetime as the session override of 7.3 for picks;
without it, a failed replacement could leave an older record on disk that the
next resolution would read back and label as the current mark. A session mark
is a mark like any other -- it flags rows, it is offered by the picker, it
shows its age -- and only the next session is without it. The notice is
`mark not remembered: <reason>`.

**How a failed mark is shown.** A mark answers on its own channel: `Snapshot`
gains `mark_seq: u64` and `mark_error: Option<String>`, advanced exactly once
per answered `MarkReviewed`, next to the `pick_seq`/`pick_error` pair that 7.4
gave `SetBase`. They are separate because the two can be in flight at once --
pressing `M`, then opening the picker and submitting a pick before the mark is
answered -- and a picker that watched one counter for both would consume the
mark's answer as its own. The picker watches `pick_seq` only, and
`ViewState::observe` turns a new `mark_seq` carrying `mark_error` into the
body notice, sanitized like every other. Because the counter advances on every
answer, pressing `M` twice against the same broken commit shows the notice
twice, unlike `base_error`, which is shown once per distinct value.

That notice must be seen even in degraded mode, where 4.2 keeps a permanent
watcher line above ordinary notices and would hide it until a condition
cleared that need never clear. `ViewState.notice` therefore carries a rank:
`Notice { text: String, urgent: bool }`, where an urgent notice is one that
must be read to be acted on -- a mark error, a mark that could not be
remembered, and 8.3's rewrite warning, which answers no keypress but is the
only thing that tells a reviewer their dots have stopped meaning what 8.1
promises -- and is shown above the status and watcher lines of 4.2. Ordinary
notices, `marked <7 hex> as reviewed` among them, keep the priority they have
today, below both. An urgent notice is cleared by the next handled *body* key:
the keys a modal consumes -- everything the picker takes while it is open, and
the key sheet's own -- leave it alone, so an answer that arrived behind a
modal is read when the modal closes.

### 8.3 Unread rows

With a mark, every refresh in branch scope computes which of its rows a commit
has touched since it. The engine publishes

```rust
// Snapshot gains:
pub unread: Arc<BTreeSet<String>>,   // paths a commit changed after the mark
```

next to `rename_sources`, from one command per refresh:

```
git -C <toplevel> diff <mark.commit> <head_seen> --name-only --no-renames -z --
```

Two commits, no working tree. That is the whole semantics of a dot: *this
file's committed content is not what it was at your mark*. It is a net
difference, as every git diff is -- a file changed after the mark and then
restored to its marked content carries no dot, which is the honest answer to
"is there something here I have not read", not a gap in the accounting. Uncommitted work does not raise one,
which is what makes `M` mean something -- the comparison of a mark against the
working tree would leave a dot on every file the agent is still editing, and
no number of presses could clear it, because uncommitted content is in no
commit and a mark is a commit. Those edits are not hidden: they are in the
row's own diff, which is still the working tree against the base's merge-base
(7.2), and in worktree scope they are the rows. The dot answers the one
question the row list cannot: which of these files did the agent *commit*
something to since I last looked. Untracked rows are in no commit and
therefore never carry one, for the same reason.

Rename detection is off, explicitly: `--no-renames`, because git turns it on
by default (`diff.renames`) and a detected rename is reported by its
destination alone, which would leave the deletion of the source -- a row of
its own in the base comparison -- unmarked and read as old. With
`--no-renames` both endpoints are ordinary paths and both are in the set.

A row is marked when *its path or its rename source* is in the set (8.5).
7.2's rows name a rename by its destination and keep the source in
`rename_sources`, so the branch list can show `R new.txt` for a row whose only
change after the mark was the deletion of `old.txt`; matching the destination
alone would leave that row without a dot.

**When the mark is not an ancestor.** The mark classifies the pair
`(mark.commit, head_seen)` with
`git merge-base --is-ancestor <mark.commit> <head_seen>`, computed when a mark
is first seen and again when either half changes. The command is skipped only
when the marked commit *is* `head_seen`, where the pair is trivially true; a
successful `M` does not get that exemption by being a mark, because it submits
`drawn_head` (8.2), which can lag -- after an amend whose diff then failed,
the stale id still resolves, so validation succeeds and the mark would claim
to be current while the branch has moved past it. Only two exit statuses are
answers: `0` is true and `1` is false. Anything else -- `128` for a missing
object, a signal, a failure to spawn -- is not cached: `classified_at` keeps
the pair it last answered for, which is no longer the current one, and the
next refresh tries again, so a classification that timed
out once cannot leave a rewritten mark labelled `Current` for as long as the
pair happens not to change. Until an answer arrives the previous state stands
and no warning is shown; the frozen runner returns `Ok(Output)` for every exit
status, so the three cases are told apart by the code, not by `Result`.

When the answer is false -- `commit --amend`, a rebase, a reset away from the
marked commit -- the mark no longer divides this branch's history, so the
unread set is not computed and **every row is flagged**, which is the truthful
reading: nothing on this branch can be said to have been read at that commit.
The state is `MarkState::Rewritten`, the urgent notice `the marked commit is
no longer on this branch; press M again` appears once per classified pair: the
view warns only when `classified_at == head_seen`, so a state retained through
a failed classification at a new head says nothing until an answer confirms
it, and it remembers the `(mark.commit, head_seen)` it last warned about, so
the warning follows every head the refresh observed rather than every head it
published -- and the picker's row reads `reviewed (a1b2c3d · rewritten)`. One
`M` repairs it completely, because the mark is not the base: the rows, the
diffs and the picker never stopped working.

A failure of the `--name-only` command itself -- the marked object pruned, a
timeout, an unreadable object store -- is `MarkState::Unreadable(reason)`: it
also flags every row, and its urgent notice is `the mark cannot be read:
<reason>`, git's own message, which is what tells a pruned object from a
timeout. It takes precedence over the ancestry answer, whose `--is-ancestor`
would have exited `128` on the same object and left the pair unclassified. A
mark already classified `Rewritten` keeps that state if its object is pruned
later while the pair does not change, because neither command runs again for
an unchanged pair; both states flag every row and warn, and only the wording
differs until the next head change reclassifies it. the
picker's row then reads `reviewed (a1b2c3d · unreadable)`. The view
distinguishes the three states by `Mark::state`, never by the size of the
unread set, which is all-paths in the ordinary case where every row was
touched after the mark.

### 8.4 Quick rows in the picker

The picker of 7.4 lists, in this order: the reset row `default (<label>)`; the
typed row `use "<input>"` when the input is not exactly a listed label; the
quick rows of this section; then the ref rows, the current base first.

| Row | Submits | Offered when |
| --- | --- | --- |
| `reviewed (<7 hex> · <age or rewritten>)` | the mark's commit | `Snapshot.mark` is set (8.2); the row is built by the view from that field, not by the engine |
| `upstream (<short name>)` | the full name git printed | `git rev-parse --symbolic-full-name @{upstream}` succeeds |
| `last commit (<7 hex>)` | `HEAD~1` | `git rev-parse --verify --quiet HEAD~1^{commit}` succeeds |
| `last 3 commits (<7 hex>)` | `HEAD~3` | the same for `HEAD~3^{commit}` |

Choosing `reviewed` is the narrowing of 8.1, and it is an ordinary pick, so
7.2's ordinary rule applies: the rows are the working tree against
`merge-base(HEAD, mark)`. While the mark is `Current` -- an ancestor of the
head, which is what it is for as long as nobody rewrites history -- that
merge-base *is* the mark, so the rows are what the working tree now differs
from the mark by: the commits made since, the uncommitted edits, and the
untracked files, and an empty list means the tree matches what was read. That
is the guarantee, and it is limited to `Current` marks on purpose. A
`Rewritten` mark is still offered and still picks -- comparing against a
commit that has left the branch is a legitimate thing to ask for -- but its
baseline is then the common ancestor of the two histories, so the list can
hold rows the mark's tree already contained, and an amend that changed nothing
but the message can leave it non-empty. The rewrite warning of 8.3 is what
tells the reviewer they are in that case. One `M` ends the warning, because it
marks a commit that is the head; it does not change the base, so a comparison
already picked against the old mark stays until the reviewer picks again --
`reviewed` then submits the new commit, and the chip stops reading
`vs reviewed` for the old one (8.5). It is a comparison, not a filter of the dotted rows: a file the agent
edited without committing has no dot but is listed, and a file reverted to its
marked content has neither. It is an ordinary pick -- remembered in `bases.json`, shown as
`vs reviewed` by the chip of 8.5, cleared by the reset row -- and the mark
itself is untouched by it, so the markers keep working and `default (main)`
returns to the whole branch.

It is also the one way to make a mark's fate matter to the rows, and the
independence 8.2 claims is qualified exactly here: a pick is a base, and a
base whose commit is later pruned fails 7.3's verification on every refresh,
which 7.8 answers by keeping the last rows with a status error. `M` cannot
repair that, because it writes no base; `r` and the reset row can, as they do
for any other broken pick. The claim in 8.2 is about a mark that has not been
picked as the base, which is the default and the case the markers are for.

The two `HEAD~N` rows submit their revision as text, so they keep meaning what
their label says as the agent commits: `last commit` is always the newest one.
The mark and the upstream submit a fixed name -- an object id, and the name
git printed (`refs/remotes/origin/main`, shortened for display by 7.3's rule)
-- so they stay where they are.

Computation. Only the last three rows need git. `Command::LoadRefs(token)`
already spawns one task that lists the refs; it now also runs those three
`rev-parse` calls in that task and publishes
`quick: Option<Arc<Vec<QuickBase>>>` next to `refs`, under the same `refs_seq`
token of 7.4, so the reply from an earlier opening of the picker is dropped as
a whole.

```rust
pub struct QuickBase {
    /// `upstream`, `last commit`, `last 3 commits`.
    pub label: String,
    /// The row's second column: a ref name or a short id.
    pub detail: String,
    /// The text `SetBase` is given.
    pub submits: String,
}
```

The `reviewed` row is not in that list. The picker builds it from
`Snapshot.mark` every time the panel is drawn, rather than from a string the
engine froze when `LoadRefs` was answered, so it always shows the mark the
snapshot carries: a `rewritten` detail reaches an open picker by itself,
because the ancestry flag changes the snapshot and every snapshot redraws. The
age is computed in the same place but has no event of its own; a picker left
untouched on an unchanging repository keeps the age of its last frame until
the next key or snapshot redraws it, and the viewer adds no timer for it. The
row is listed first among the quick rows.

`@{upstream}` is one argv element, never composed into a shell, and cannot
become an option: it does not start with `-`, so `check_text` of 7.3 accepts
it. A repository with no upstream, no parent commit, or fewer than three
commits simply has fewer rows; the three commands are independent and a
failure of one is not an error.

Filtering follows 7.4: the input is matched case-insensitively as a substring
of what the row displays. For the three engine-computed rows that is their
label and their detail together, so `orig` keeps `upstream (origin/main)`;
both parts are fixed for as long as the picker is open. The `reviewed` row is
matched on its label alone, because its detail is live: were the age part of
the match, a mark crossing a minute boundary or turning `rewritten` would add
or remove the row under the cursor. The cursor is reconciled by identity when
a change of `Snapshot.mark` (its presence or its commit) or of
`base.requested` rearranges the rows -- the base reorders them because 7.4
lists the current base first: the picker looks for the row that submits what
the cursor submitted, moves it there, and clamps to the last row when that row
is gone. A new `refs_seq` keeps 7.4's own rule instead, re-placing the cursor
on the first match of the input, because the arrival of the list is exactly
when a typed `main` becomes the listed `refs/heads/main`: identity matching
would fail on the changed `submits`, and clamping could land on an unrelated
ref. Quick rows are never disabled, and the reset row is never filtered out.

### 8.5 The panel marker, the key, the chip and the empty list

**The marker.** In branch scope the files panel draws `●` in the two columns
it reserves at the end of a row for the `S` of a staged row (4.4). Those
columns are free there, because every branch row has `staged = false` (7.2),
so nothing moves and no name loses a cell. A row is marked when its path, or its entry in `rename_sources` (7.2), is in
`Snapshot.unread`:

```
CHANGED 6
 M app.rs        ●
 D b.txt         ●
 A d.txt
▸R new.txt
 ? u.txt         ●
```

The marker is drawn with the accent of 4.4 and no background, so it reads on
any terminal theme and stays legible under the selected row's reverse video.
With no mark, in worktree scope, on an untracked row, or on a row no commit
touched after the mark, those columns stay blank -- there is no "read" glyph,
because absence is the quiet state and the panel is 18 columns wide.

**The key.** One key is added to the table of 4.3, which 7.4 last extended:

| Key | Action | vimeflow binding |
| --- | --- | --- |
| `M` | mark the current commit as reviewed | none (added) |

The key sheet gains a row: 24 rows. `M` does not change the scope: in branch
scope its effect is the markers clearing, in worktree scope it is the notice
of 8.2, and a reviewer who wants the narrowed list asks for it with `B`.

**The chip.** Unchanged by this section, except that the chip reads
`vs reviewed` exactly while `base.requested` is the commit `Snapshot.mark`
names -- the alias is a comparison, not a label the pick remembers. Picking
the `reviewed` row makes it true; a later `M` marks a new commit and makes it
false again, and the chip then reads `vs a1b2c3d` for the base that is
genuinely in force, which is the older commit the pick froze. The same holds
after reopening the viewer, where the pick comes from `bases.json` and the
mark from `marks.json` and nothing else ties them together. Its hover hint
keeps the shape of 7.4 with the same substitution,
`switch scope · b · reviewed @ a1b2c3d`.

**The empty list.** `state_message` of 4.2 shows `working tree clean` whenever
the row list is empty. That is true in worktree scope and wrong in branch
scope, where an empty list means the branch carries nothing against its base.
In branch scope with a base the message becomes `nothing on this branch since
<label>`, the label truncated as the chip truncates it (16 cells, 4.2's
ellipsis) -- and with the mark as the base, that is the sentence a caught-up
reviewer reads.

### 8.6 Failure modes, cost and configuration

Additions to the tables of 5.1 and 7.8. Every row of this table leaves the
rows, the diffs and the base untouched, because a mark is not a base:

| Condition | Behaviour |
| --- | --- |
| `M` with nothing drawn yet, no commit on the branch, or outside a repository | the notice `nothing to mark: no commit yet`; no command is sent and nothing is written |
| the marked id no longer resolves between the frame and the press | `mark_error = not a commit: <7 hex>`; nothing is written; the previous mark stays |
| `marks.json` is unreadable or malformed | treated as absent; a problem line goes to `config-problems.log`; the next `M` rewrites it |
| `marks.json` cannot be written | the mark holds for this session (8.2's session mark) and flags rows normally; the urgent notice `mark not remembered: <reason>` |
| the marked commit is no longer an ancestor of `head_seen` (amend, rebase, reset) | `MarkState::Rewritten`; every row is flagged; the urgent notice `the marked commit is no longer on this branch; press M again` once per classified pair; the picker row reads `rewritten`; one `M` repairs it |
| the `--name-only` command fails (pruned object, timeout, unreadable store) | `MarkState::Unreadable(reason)`; every row is flagged; the urgent notice `the mark cannot be read: <reason>`; the picker row reads `unreadable`; one `M` repairs it |
| `merge-base --is-ancestor` gives no answer | the pair stays unclassified, the previous state stands with no warning, and the next refresh retries |
| the mark has been picked as the base and its object is pruned | not a mark failure but a base failure: 7.3's verification fails and 7.8 keeps the last rows with a status error; `r` or the reset row recovers, `M` does not |

| the head `rev-parse` cannot be run (spawn failure, timeout) | `head` and `head_seen` keep their previous values; `M` marks the older id, the safe direction of 8.2 |
| the head `rev-parse` runs and finds no commit (unborn branch, not a repository) | both ids become `None` in the next publication, taking any pending candidate with them, and `drawn_head` clears; `M` then refuses instead of marking a commit of the branch that was left |
| the selected row's diff fails, or `HEAD` moves while it runs | that snapshot carries `head = None`, the frame that draws it clears `drawn_head`, and `M` refuses until a diff is drawn again |
| `@{upstream}`, `HEAD~1` or `HEAD~3` do not resolve | that quick row is not offered; the others are |

Cost. Two `rev-parse` runs per refresh for the head id -- one before the row
commands, which the commands then use, and one after, which must agree for an
empty list to be markable -- and two in each diff task, bracketing its call;
in both scopes, on top of what 3.3
and 7.5 already run; one `diff --name-only` between two commits per refresh in
branch scope while a mark exists and is an ancestor; one `merge-base --is-ancestor` per changed
`(mark.commit, head_seen)` pair; three `rev-parse` runs each time the picker
opens. No further quick-row command runs while the picker is open -- the
refreshes of 3.3 and 7.5 continue as always -- and the poll interval is
unchanged. The allow-list of 7.6 stays at ten subcommands: every command of
this section is `rev-parse`, `diff` or `merge-base`.

Configuration is unchanged: no new keys. A mark is state, not preference, and
the state directory rule of 4.7 covers `marks.json` as it covers `bases.json`
and `split-panes.json` -- written only under an absolute state directory,
never relative to the repository.

The version becomes 0.0.3. Phase 2 is unaffected: `M` is outside its reserved
set (4.3), its write actions remain worktree-scope only (7.7), and the two
panel columns this section draws into are the ones Phase 2's staged rows use
only in worktree scope, where no marker is drawn.

### 8.7 Tests and success criteria

Test layers, added to 5.2 and 7.9:

1. Engine, marking: a stored `commit` that is not a hexadecimal object id --
   `--output=x` among them -- makes the record absent with a problem line, and
   no command ever receives it; `MarkReviewed` validates the id, writes `marks.json` under
   the lock and publishes the mark with a recomputed unread set, without
   touching `bases.json` or the base; a second `M` replaces the record; an id
   that no longer resolves answers `mark_error`, leaves `pick_seq` untouched
   and writes nothing; with no state directory the mark holds as the session
   mark, flags rows, and the notice says so; a mark equal to `head_seen` skips
   the ancestry command while a stale `drawn_head` mark does not.
2. Engine, the unread set: on a fixture whose branch carries four commits, a
   mark at the second flags exactly the paths whose committed content differs
   between the second and the fourth, and nothing else; a path changed by the
   third and restored by the fourth carries no dot; a path changed both before and after the mark is flagged;
   uncommitted edits to a file the mark already covers raise no flag, and a
   second `M` on a dirty worktree leaves the set empty; untracked rows are
   never in it; a file modified before the mark and renamed after it yields
   both the old and the new name, which rename detection -- on by default
   without `--no-renames` -- would have reduced to one; a row whose only
   post-mark change is the deletion of its rename source is flagged through
   `rename_sources`; the set is empty in worktree scope and with no mark.
3. Engine, the markable id: a snapshot with a `Ready` diff carries the id that
   diff reported, one with an empty list carries the refresh's confirmed id,
   and one that is `Loading` or `Failed` carries `None`; a diff during which
   `HEAD` moves -- a commit landing between its two samples, and a `reset
   --hard` to the parent -- reports nothing and leaves that snapshot
   unmarkable, whatever any other refresh has seen; a diff requested before a
   commit and completed after it reports where it read, so no generation
   bookkeeping is needed; a diff issued before the rows were published never
   promotes a candidate, even when it completes afterwards; the refresh's row
   commands carry the sampled id, so the rows a frame shows are the rows of
   the id it publishes even when `HEAD` leaves and returns during the job; a
   refresh during which `HEAD` moves offers no id and the next one settles it;
   with a watcher that never emits, an ordinary commit still advances `head`
   within one poll (the case a `with_head`-only rule would miss); switching to
   an unborn branch publishes `None`, while a failed spawn keeps the previous
   `head_seen`.
4. Engine, ancestry: a plain commit keeps the state `Current`; `commit --amend`
   makes the next refresh publish `MarkState::Rewritten`, flag every row and warn
   once per classified pair, and one `M` restores both; a mark loaded from
   another viewer at an unchanged head is classified on arrival; an
   `--is-ancestor` exit of `1` is a negative answer while `128` and a spawn
   failure leave the pair unclassified and are retried by the next refresh,
   which a rewrite immediately after a timed-out classification proves; a
   `Rewritten` state retained across a head change warns only once the new
   pair is classified; a
   pruned marked object is
   `Unreadable`, flags every row, warns with git's reason rather than the
   rewrite wording, and leaves the rows, diffs and base working; the same mark
   picked as the base instead fails 7.3's verification and is recovered by `r`
   or the reset row, not by `M`.
5. Engine, quick bases: the three git-computed rows' presence and `submits` on
   repositories with and without an upstream and with one, two and four
   commits; they ride the `refs_seq` token of 7.4, so a reply from an earlier
   opening is dropped whole.
6. View: the panel marker in branch scope for exactly the unread rows, absent
   in worktree scope and without a mark, and legible under the selected row's
   reverse video; an urgent notice above a watcher error and an ordinary one
   below it, with the rewrite warning urgent and `marked <7 hex> as reviewed`
   ordinary; ages at each boundary (`just now`, `59 min ago`, `2 h ago`,
   `3 d ago`, a future `at` reading `just now`); `nothing on this branch since
   <label>` in branch scope while worktree scope still reads `working tree
   clean`; the picker's row order, the `reviewed` row's live age and
   `rewritten` detail across two renders of one open picker, its membership
   not changing with them, and the cursor rules of 8.4.
7. Input: `M` sends `MarkReviewed` with `drawn_head`, which a frame whose diff
   is `Loading` or `Failed` clears rather than keeps, so a commit whose diff
   was drawn only in a snapshot the shell skipped can never be marked, and
   neither can one left behind in a stale field while the body shows another
   commit's diff; a frame drawn with the key sheet or the picker
   open leaves it untouched too; `head = None` clears it; with nothing drawn
   it shows the notice and sends nothing; `M` does not change the scope; a
   mark answered while a pick is in flight neither closes the picker nor
   becomes its error; picker and key-sheet keys do not clear a pending urgent
   notice, and the first body key afterwards does.
8. Read-only: the G7 test of 7.6 gains a mark and a picker opening; the
   allow-list stays at ten, and the state directory afterwards holds only
   `bases.json`, `marks.json` and the lock file.
9. Tier B (ignored, real host): after the existing `b`, the test presses `M`
   and waits for the panel marker to clear, in both hosts.

Success criteria, in addition to 1.5 and 7.9:

10. On a branch where an agent has committed, reading the rows and pressing
    `M` clears every marker while the list keeps its rows, and it clears them
    on a dirty worktree too; when the agent commits again, exactly the paths
    of the new commit carry a marker, and uncommitted edits in between carry
    none.
11. The mark survives closing and reopening the viewer on the same worktree,
    and a second viewer on the same worktree shows the same markers at its
    next resolution; with a `Current` mark, picking `reviewed` in the picker
    lists what the working tree differs from the mark by -- the new commits'
    files, plus uncommitted and untracked ones -- while a `Rewritten` mark
    compares against the common ancestor instead, and `default (...)` returns
    to the whole branch with the markers intact.
12. `docs/acceptance-p1.md` gains row 8 with the same evidence columns; the
    release guard is unchanged.

<!-- codex-reviewed: 2026-09-23T15:29:21Z -->
