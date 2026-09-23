# Review marks and quick bases

Addendum to the Phase 1 design
(`2026-09-18-hunks-roadmap-p1-viewer-design.md`) and to its branch-scope
section 7 (`2026-09-22-branch-scope-design.md`, shipped as 0.0.2). It is
section 8: the numbering continues from section 7, and every term (`Scope`,
`Base`, `Comparison`, the picker, the chip, the state directory, the read-only
guarantee G7) means what those two documents say it means. Nothing here adds a
scope; this section is about choosing the base of the branch scope that exists.

## 8. Review marks and quick bases

### 8.1 Why

Branch scope answers "what does this branch carry". The question its owner
asks more often is "what has happened since I last looked". Between two
readings the agent adds commits; the row list grows, and the reviewer has to
remember which rows they have already read. Nothing in the viewer knows where
they stopped.

Setting the base to the commit that was last reviewed answers it exactly: the
rows become the commits made since, plus the working tree, and an empty list
means there is nothing new. That base is one keystroke from being free,
because the viewer already remembers a picked base per worktree (7.3) and the
refresh already knows where `HEAD` is. `M` stores that commit as the pick and
records it as a review mark, so the next look shows only what arrived after
it. The promise holds while the marked commit is still an ancestor of `HEAD`,
which is the ordinary case of an agent adding commits; 8.2 says what a rewrite
does and how the viewer reports it.

The same section removes the typing from the three bases that are otherwise
entered by hand on every repository: the branch's upstream, the last commit,
and the last three. They become rows in the picker of 7.4, computed from
`rev-parse`, so the allow-list of 3.5 and 7.6 does not grow.

### 8.2 The mark

A mark is a commit and a time: the object id the viewer had published as the
head of the worktree when `M` was pressed, and the time the engine recorded
it. It is per worktree, it is at most one, and it is never resolved as a
preference -- resolution (7.3) is untouched, still reading one string per
worktree from `bases.json`.

**Which commit is marked.** The snapshot gains `head: Option<String>`, the
full object id of `HEAD`. Every refresh resolves it with
`git rev-parse --verify --quiet HEAD^{commit}` (allow-listed since Phase 1),
not only a refresh that carries `with_head`: an ordinary commit advances
`refs/heads/<branch>` and leaves `<git_dir>/HEAD` holding the same
`ref: refs/heads/<branch>`, so the watcher's head detection -- which compares
that file's contents (`watcher.rs`, "HEAD only moves on commit/checkout") --
does not fire, and a `with_head`-only rule would leave the markable commit
permanently behind on the branch an agent is committing to. (The same fact
corrects one sentence of 7.5: after a commit it is `git-status-changed`, from
the index write, that refreshes the branch rows; `git-head-changed` follows a
checkout. The shipped behaviour is what 7.5 describes; only its explanation of
which event fires was wrong.)

**When the published head advances.** Two rules bind the markable id to what
the viewer has actually drawn.

*Sampled first, then used, then confirmed.* The head `rev-parse` runs at the
start of the refresh job, before the status read of 3.3 and before the row
commands of 7.2. Its output is then **used in place of the symbol `HEAD`** by
every command of that refresh that needs it: 7.2's pin becomes
`git merge-base <head> <base commit>`, and the ancestry check below becomes
`git merge-base --is-ancestor <mark.commit> <head>`. This is what binds the
id to the rows, and it binds them by construction rather than by timing: the
merge-base the rows and every row diff are computed against is derived from
the sampled id, so the comparison a frame shows is always the comparison of
the id that frame publishes -- even if `HEAD` moves away and back while the
job runs, which two samples taken around it could not detect.

The same `rev-parse` runs once more when the job's commands are done, and the
job offers its id only when the two agree. That second sample no longer
carries the guarantee; it is the cheap detector for the half that cannot be
pinned -- the status read, which asks git about the worktree and the index
against whatever `HEAD` is live. A refresh that spans a commit, a `checkout`
or a `reset` therefore offers nothing, the previous id stands, and the next
refresh settles it. Two `rev-parse` runs per refresh is the price; 8.5 counts
it.

The pinning covers the row job and every row diff, which take the merge-base
it produced, but `HEAD` can still move between the job and the moment a diff
reads the working tree, and the two ways it can move are both already safe. A move forward -- a new commit -- releases
an id older than the content that was drawn, which is the direction that shows
an already-read change again. A move that is not a fast-forward -- `reset
--hard` to an ancestor, a rebase, an amend -- leaves the marked commit off the
branch, which the ancestry check below detects at the next refresh and
announces with its own notice, so the reviewer is told to mark again rather
than silently losing a change. A third sample inside the diff task would buy
only a smaller window of the same two outcomes, at one `rev-parse` per diff.

**The boundary.** One thing cannot be pinned: the working tree is not a
commit, and `git diff <M>` reads it live. A process that rewinds the tree and
restores it while a refresh runs -- `reset --hard` to an ancestor and back
inside one job -- can make a frame draw that other content under an id whose
comparison is otherwise correct, with both samples agreeing and the mark still
an ancestor, so nothing detects it. The viewer holds no lock on the repository
and cannot close that window. What it does instead is bounded and stated here:
every frame's comparison is derived from the id that frame carries, a frame
without a loaded diff acknowledges nothing, and the next refresh redraws
whatever the tree settled on. A rewind that does not come back is a rewrite,
which the ancestry warning below reports; a rewind that comes back inside one
refresh is the one case where 8.1's promise rests on no other process
rewinding history under the reader.

*Published last.* The sampled id is held by the session and becomes
`Snapshot.head` only when a row's diff from that refresh has been loaded
successfully and published. A diff that ends in `DiffState::Failed` -- a
timeout, a git error -- leaves the previous id in place, because its hunks
were never drawn, and so does a refresh whose row list is empty: there is
nothing to read, so there is nothing to acknowledge. The consequence of the
second rule is small and self-correcting -- on a branch that carries nothing
against its base, `M` repeats the mark already in force, and the first refresh
with a row again advances the id, so one further press settles it. Until then
the snapshot carries the previous id. `refreshing` cannot carry this rule: it is published only by the
command paths (`r`, `b`, `B`), so a watcher- or poll-driven refresh -- exactly
the one that follows an agent's commit -- runs with `refreshing` false
throughout. Without the rule, such a refresh would publish the new rows and
the new id while 3.3 still shows the previous `Ready` diff, and `M` would mark
a commit whose hunks have never been drawn.

*Drawn, not merely published.* Publication is still not the terminal: the
shell drains every queued snapshot and draws only the last one, so a snapshot
that completed a refresh can be skipped when a newer one arrives inside the
same 100 ms poll, and `head` -- a state field, not an event -- would survive
into the frame that is drawn. The view therefore keeps the id it has drawn:
`ViewState` gains `drawn_head: Option<String>`, assigned after a frame that
actually showed the review body -- the snapshot's diff is `Ready`, *and* the
frame was the ordinary body, not the "terminal too small" notice of 5.1, the
key sheet of 4.3 or the picker of 7.4, each of which covers the diff
completely. An empty list draws no diff and assigns nothing, matching the
publication rule above. A frame showing `Loading` or `Failed`, and any
modal frame, leaves it alone. So a refresh that succeeds behind an open key
sheet is not acknowledged, and `M` after closing it still marks the last
commit whose hunks were on the terminal. `M` marks `drawn_head`, never
`Snapshot.head` directly.

The clearing is not gated: when the head `rev-parse` runs and reports no
commit (an unborn branch, or the directory stopped being a repository),
`Snapshot.head` becomes `None` in the next publication whatever else that
refresh did, and a snapshot with `head = None` clears `drawn_head` on the next
frame. Both are invalidation, not advance: the only thing they can cause is
`M` refusing with the notice below, which is why they need no gate. Without
it, switching to an unborn branch in branch scope -- where `merge-base` then
fails and no refresh completes -- would leave `M` marking a commit of the
branch that was left.

The promise is therefore narrow and checkable: **`M` never marks a commit
newer than the last frame this viewer drew with its diff loaded.** What a
reader did with the hunks that were drawn is theirs; the viewer only
guarantees it never advances the mark past what it has put on the terminal.

`M` sends `Command::MarkReviewed(commit)` carrying `drawn_head`, so the marked
commit is the one the reviewer was looking at, not whatever `HEAD` becomes
while the command waits behind a refresh in the queue of 7.2. If a commit
lands between that frame and the press, the mark is the older id and that
commit is listed as new at the next refresh: the error is always toward
showing an already-read change again, never toward hiding an unread one.

`M` is not in the reserved set of 4.3 (`s d D i I u U x v y Y @ c /`). Reading
it as "mark" next to `m` for the mouse toggle follows the viewer's existing
pairs that share a letter without sharing a meaning (`h`/`l` move between the
diff's sides, `H`/`L` scroll). With nothing to mark -- no commit yet on an unborn branch,
not a repository, or no frame drawn yet with a loaded diff -- `M` shows the
notice `nothing to mark: no commit yet` and sends nothing, as `b` does without
a base (7.3).

**What the engine does.** `MarkReviewed(commit)` is answered in the order a
`SetBase(Some(..))` is answered, and under 7.2's publication rule (the base is
published only with the first row list loaded under it):

1. Validate the id with `git rev-parse --verify --quiet <commit>^{commit}`. It
   can fail if the commit was rewritten between the publication and the press;
   then nothing is written, the previous mark and base are left alone, and the
   answer carries `mark_error = "not a commit: <7 hex>"`.
2. Load the branch rows against it, as for any pick.
3. On success write two files under the state directory of 4.7, both inside
   one `reuse::with_lock` critical section, each replaced atomically:
   - `bases.json` gains this worktree's entry with the object id as its
     string, exactly as a pick made in the picker. The file's shape does not
     change, so a 0.0.2 viewer reading it sees a pinned base and behaves
     correctly, only without the label of 8.4.
   - `marks.json`, new, maps the same canonical toplevel to
     `{"commit": "<full object id, as git printed it>", "at": <seconds since
     the Unix epoch>}`. The id is stored verbatim; nothing assumes 40
     characters, so a SHA-256 repository stores its 64.
4. Publish the base, the rows and the mark together, in branch scope.

**How a failed mark is shown.** A mark answers on its own channel:
`Snapshot` gains `mark_seq: u64` and `mark_error: Option<String>`, advanced
exactly once per answered `MarkReviewed`, next to the `pick_seq`/`pick_error`
pair that 7.4 gave `SetBase`. They are separate because the two can be in
flight at once -- pressing `M`, then opening the picker and submitting a pick
before the mark is answered -- and a picker that watched one counter for both
would consume the mark's answer as its own, closing on it or showing its
error. The picker watches `pick_seq` only, and `ViewState::observe` turns a
new `mark_seq` carrying `mark_error` into the body notice, sanitized like
every other. That notice must be seen even in degraded mode, where 4.2 keeps a
permanent watcher line above ordinary notices and would hide it until the
condition cleared, which it need not ever do. `ViewState.notice` therefore
carries a rank: `Notice { text: String, urgent: bool }`, where an urgent
notice is one that must be read to be acted on -- a mark error, a pick or mark
that could not be remembered, and the rewrite warning below, which answers no
keypress but is the only thing that tells a reviewer their mark has stopped
meaning what 8.1 promises -- and is shown above the status and watcher lines
of 4.2. Ordinary notices keep the priority they have today, below both. An urgent notice is cleared by the next handled *body* key, like any other
notice: the keys a modal consumes -- everything the picker takes while it is
open, and the key sheet's own keys -- leave it alone, so an answer that
arrived behind a modal is read when the modal closes, and the ambient line
returns on the next body key after that. Because the counter advances on every answer, pressing `M` twice
against the same broken commit shows the notice twice, unlike `base_error`,
which is shown once per distinct value. The notice outlives the picker (it is
cleared by the next handled key, 4.3), so a mark that failed while the picker
was open is read after `Esc`.

`at` is read from the system clock in step 3: it is when the mark was
written, which is what the age of 8.4 measures. It is not the time of the
press, and no bound is claimed between the two -- the command waits behind
whatever refreshes are queued, and each of its git calls carries the frozen
30 s timeout. Defining `at` at the write keeps the clock out of the pure input
layer and makes the stored value mean exactly one thing.

**Where the mark lives while the viewer runs.** `Snapshot` gains
`mark: Option<Mark>`:

```rust
pub struct Mark {
    pub commit: String,
    /// Seconds since the Unix epoch, recorded when the mark was written.
    pub at: u64,
    /// False once the marked commit is no longer an ancestor of `HEAD`.
    pub ancestor: bool,
}
```

The record is read from `marks.json` in the same step that reads
`bases.json` -- whenever the preference is resolved (session start, `r`, a
pick, a mark, a scope change, a branch change) -- and carried forward on the
refreshes in between, like the rest of the snapshot. A successful `M`
publishes what it wrote without re-reading.

When a write fails, the intended mark is kept in memory for the rest of the
session and wins over the file, the same rule and the same lifetime as the
session override of 7.3 for picks. Without it, a failed replacement could
leave an older record on disk that the next resolution would read back and
label as the current mark. The session mark is a mark like any other: it is
displayed with its label and age, and only the next session is without it. The
notices are independent, because the two writes can fail independently:

- `bases.json` unwritable: `pick not remembered: <reason>`, as for any pick;
  the base itself still holds for the session.
- `marks.json` unwritable: `mark not remembered: <reason>`.

**The mark is displayed, never resolved.** `BaseSource` gains no variant and
`Base` no field: the view derives "this base is the mark" by comparing
`base.requested` with `mark.commit`. A mark therefore survives every change of
base; the only thing that replaces it is another `M`. Picking the reset row of
7.4 clears the *pick* and leaves the mark, so `reviewed (...)` stays in the
picker and the reviewer can return to it after looking at the whole branch.

Ages are coarse, computed from `at` against the system clock when a frame is
drawn: `just now` under a minute, `N min ago` under an hour, `N h ago` under a
day, `N d ago` beyond. A mark dated in the future (a clock moved backwards)
reads `just now`; a mark is never rejected for its time.

**A rewritten mark.** A mark can stay valid and stop meaning what 8.1
promises: after `commit --amend` or a rebase, the marked commit still exists
but is no longer on the branch, and 7.2 compares against the merge-base of
`HEAD` and that commit -- their common ancestor -- so changes that were
already read reappear. Ancestry is therefore recomputed whenever the *sampled* head id changes and a
mark exists -- the id the refresh observed, not the one the gate of 8.2 has
published, so a selected diff that keeps failing cannot leave the flag stale
and the warning unspoken -- and not only when the preference is resolved:
one `git merge-base --is-ancestor <mark.commit> HEAD` (the subcommand is
allow-listed by 7.6), which is at most one process per commit, none per poll.
Its exit status is the answer, and only two values are answers: `0` is true,
`1` is false, and anything else -- `128` for a missing object, a signal, a
failure to spawn -- leaves the flag as it was. The frozen runner returns
`Ok(Output)` for every exit status, so the three cases are told apart by the
code, not by `Result`. When the answer is false the viewer says so
rather than pretending: the urgent notice `the marked commit is no longer on
this branch; press M again` appears once per head change, and the picker's row
reads `reviewed (abc1234 · rewritten)`. The comparison is left alone -- it is
still a correct diff against a real commit -- and one `M` repairs it. When the
command itself fails to run, the flag keeps its previous value and no notice
is shown: failing to classify a mark is not a reason to distrust the diff.

One rewrite is not repairable by `M`: amending a repository's root commit
leaves the marked root and the new root with no common ancestor at all, so
`merge-base` fails and 7.8's rule keeps the previous rows with a status error.
The head gate above then holds `head` at the marked commit, because no refresh
completes, and `M` would resubmit the very id that cannot be compared. The way
out is the base, not the mark: `B` and the reset row (or any working base)
make the rows load again, after which `M` marks the new head. 8.5 lists the
case so it is not mistaken for a defect in marking.

A mark whose commit no longer exists at all (an amend plus a pruned object)
needs the same care as the root-commit case, because the marked id is also the
base: in branch scope every refresh re-verifies it (7.3), the verification
fails, and 7.8 keeps the previous rows with a status error, so the head never
advances and `M` would resubmit the missing id. Here `r` is the way out: it
re-resolves the preference, which skips the invalid pick with `base_error`
naming it and falls through to step 2 of 7.3, after which the rows load, the
head advances and `M` marks again; `B` and the reset row do the same. The
picker's `reviewed` row fails validation with `not a commit: <7 hex>` if it is
chosen while the object is gone.

### 8.3 Quick rows in the picker

The picker of 7.4 lists, in this order: the reset row `default (<label>)`; the
typed row `use "<input>"` when the input is not exactly a listed label; the
quick rows of this section; then the ref rows, the current base first.

| Row | Submits | Offered when |
| --- | --- | --- |
| `reviewed (<7 hex> · <age or rewritten>)` | the mark's commit | `Snapshot.mark` is set (8.2); the row is built by the view from that field, not by the engine |
| `upstream (<short name>)` | the full name git printed | `git rev-parse --symbolic-full-name @{upstream}` succeeds |
| `last commit (<7 hex>)` | `HEAD~1` | `git rev-parse --verify --quiet HEAD~1^{commit}` succeeds |
| `last 3 commits (<7 hex>)` | `HEAD~3` | the same for `HEAD~3^{commit}` |

The two `HEAD~N` rows submit their revision as text, so they keep meaning what
their label says as the agent commits: `last commit` is always the newest one.
The mark and the upstream submit a fixed name -- an object id, and the name
git printed (`refs/remotes/origin/main`, shortened for display by 7.3's rule)
-- so they stay where they are. A row whose revision is what the current base
already is carries no marker; the chip names the base.

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
failure of one is not an error. The cost is three `rev-parse` runs when the
picker opens, none while it is open, and none on a refresh. The allow-list of
7.6 is unchanged, at ten subcommands.

Filtering follows 7.4 unchanged: the input is matched case-insensitively as a
substring of what the row displays. For the three engine-computed rows that is
their label and their detail together, so `orig` keeps
`upstream (origin/main)`; both parts are fixed for as long as the picker is
open. The `reviewed` row is matched on its label alone, because its detail is
live (8.2): were the age part of the match, a mark crossing a minute boundary
or turning `rewritten` would add or remove the row under the cursor. The cursor is reconciled by identity when a change of `Snapshot.mark` (its
presence or its commit) or of `base.requested` rearranges the rows -- the base
reorders them because 7.4 lists the current base first: the picker looks for
the row that submits what the cursor submitted, moves it there, and clamps to
the last row when that row is gone. A new `refs_seq` keeps 7.4's own rule
instead, re-placing the cursor on the first match of the input, because the
arrival of the list is exactly when a typed `main` becomes the listed
`refs/heads/main`: identity matching would fail on the changed `submits`, and
clamping could land on an unrelated ref. Without it, the `reviewed` row appearing while a mark is
answered -- the sequence 8.2 explicitly supports, `M` and then `B` -- would
shift every row down by one under a fixed index, and `Enter` would submit a
base the reviewer never selected. Quick rows are never disabled, and the reset row is
never filtered out.

### 8.4 Keys, the chip and the empty list

One key is added to the table of 4.3, which 7.4 last extended:

| Key | Action | vimeflow binding |
| --- | --- | --- |
| `M` | mark the current commit as reviewed and compare against it | none (added) |

The key sheet gains a row: 24 rows.

The chip. In branch scope it reads `vs reviewed` when `base.requested` is the
mark's commit, and otherwise what 7.4 says. Its hover hint keeps the shape of
7.4, with the same substitution: `switch scope · b · reviewed @ <7 hex>`.

The empty list. `state_message` of 4.2 shows `working tree clean` whenever the
row list is empty. That is true in worktree scope and wrong in branch scope,
where an empty list means the branch carries nothing against its base -- with
a mark, the "caught up" state, and the one the reviewer sees most. In branch
scope with a base the message becomes `nothing on this branch since <label>`,
the label truncated as the chip truncates it (16 cells, 4.2's ellipsis).
Worktree scope is unchanged.

`M` pressed in worktree scope marks and switches to branch scope, as a pick
from the picker does (7.4): pressing it means wanting to see what comes next.

### 8.5 Failure modes, cost and configuration

Additions to the tables of 5.1 and 7.8:

| Condition | Behaviour |
| --- | --- |
| `M` with no commit yet, or outside a repository | the notice `nothing to mark: no commit yet`; no command is sent and nothing is written |
| the marked id no longer resolves between the publication and the press | `mark_error = not a commit: <7 hex>`; nothing is written; the previous mark and base stay |
| `marks.json` is unreadable or malformed | treated as absent; a problem line goes to `config-problems.log`; the next `M` rewrites it |
| `marks.json` cannot be written | the base is set and the mark holds for this session (the session mark of 8.2); the notice adds `mark not remembered: <reason>` |
| the marked commit is no longer an ancestor of `HEAD` | `Mark::ancestor` is false; `the marked commit is no longer on this branch; press M again` once per head change; the picker row reads `rewritten`; the comparison is unchanged |
| the marked commit no longer exists | in branch scope the base fails to verify on every refresh, so 7.8 keeps the previous rows with a status error until `r` (or `B`) re-resolves; resolution then skips the invalid pick with `base_error` and continues at step 2; the picker's `reviewed` row fails validation if chosen |
| `@{upstream}`, `HEAD~1` or `HEAD~3` do not resolve | that quick row is not offered; the others are |
| `merge-base --is-ancestor` fails to run | `ancestor` keeps its previous value and no notice is shown |
| a rewrite leaves no common ancestor at all (an amended root commit) | 7.8's rule: the rows and the head both stay, with git's message as a status error; `B` and a working base restore the rows, and `M` marks again afterwards |
| the selected row's diff fails during a refresh | `head` does not advance; `M` keeps marking the last commit whose hunks were drawn |
| the head `rev-parse` cannot be run (spawn failure, timeout) | `head` keeps its previous value; `M` marks that older id, the safe direction of 8.2 |
| the head `rev-parse` runs and finds no commit (unborn branch, not a repository) | `head` becomes `None` in the next publication, gate or no gate, and clears `drawn_head`; `M` then shows `nothing to mark: no commit yet` instead of marking a commit of the branch that was left |
| `M` fails to validate or its rows fail to load | the notice of 8.2, once per press; the base, the mark and the rows are unchanged |

Cost. Two `rev-parse` runs per refresh for the head id -- one before the row
commands, one after, which must agree -- in both scopes, on top of what 3.3
and 7.5 already run; one `merge-base --is-ancestor` per head change
while a mark exists; three `rev-parse` runs each time the picker opens. No
further quick-row command runs while the picker is open -- the refreshes of
3.3 and 7.5 continue as always, the head and ancestry commands included -- and
the poll interval is unchanged. The
allow-list of 7.6 stays at ten subcommands: every command of this section is
`rev-parse` or `merge-base`.

Configuration is unchanged: no new keys. A mark is state, not preference, and
the state directory rule of 4.7 covers `marks.json` as it covers `bases.json`
and `split-panes.json` -- written only under an absolute state directory,
never relative to the repository.

The version becomes 0.0.3. Phase 2 is unaffected: `M` is outside its reserved
set (4.3), and its write actions remain worktree-scope only (7.7).

### 8.6 Tests and success criteria

Test layers, added to 5.2 and 7.9:

1. Engine, marking: `MarkReviewed` validates the id, loads the branch rows
   against it, writes both files in one locked section and publishes base,
   rows and mark together in branch scope; a second `M` replaces both records;
   picking the reset row clears the pick and leaves the mark; an id that no
   longer resolves answers `mark_error`, leaves `pick_seq` untouched and
   writes nothing; with no state directory the mark holds as the session mark
   and the notice says so.
2. Engine, the published head: an empty row list never advances it, and
   neither does a failed diff; the refresh's row commands carry the sampled id
   (`merge-base <head> <base>`), so the rows a frame shows are the comparison
   of the id it publishes even when `HEAD` leaves and returns during the job;
   a refresh during which `HEAD` moves -- a commit landing between the two
   samples, and a `reset --hard` to an ancestor -- offers no id, and the next
   refresh publishes the settled one; on a fixture
   whose diff is held by the
   `diff_gate` of 5.2, a commit that lands while a diff is blocked does not
   change `head` in any snapshot published before that diff is accepted, and
   does change it in the one that carries the diff -- the watcher-driven path,
   where `refreshing` is false throughout; an empty row list advances it
   without a diff; with a watcher that never emits, an ordinary commit still
   advances `head` within one poll (the case the `with_head`-only rule would
   miss); a refresh whose diff fails does not advance it; switching to an
   unborn branch publishes `head = None` while a failed spawn keeps the
   previous id.
3. Engine, ancestry: a plain commit keeps `ancestor` true; `commit --amend`
   makes the next refresh publish `ancestor = false` once per head change; a
   an `--is-ancestor` exit of `1` is a negative answer while `128` and a
   spawn failure leave the flag and show no notice; an amend whose selected
   diff then fails repeatedly still publishes `ancestor = false` and its
   warning, because ancestry follows the sampled head, not the published one; an amended
   root commit leaves the rows and the head untouched with a status error, and
   picking the reset row restores them so the next `M` succeeds; a pruned
   marked object is recovered by `r` alone, which re-resolves and skips it.
4. Engine, quick bases: the three git-computed rows' presence and `submits` on
   repositories with and without an upstream and with one, two and four
   commits; they ride
   the `refs_seq` token of 7.4, so a reply from an earlier opening is dropped
   whole (that rule is already covered; this adds `quick` to its assertions).
5. View: an urgent notice is drawn above a watcher error and an ordinary one
   below it, the rewrite warning is urgent, and the next handled body key
   clears both kinds; the chip reads
   `vs reviewed` and its hover hint `switch scope · b ·
   reviewed @ <7 hex>`; ages at each boundary (`just now`, `59 min ago`,
   `2 h ago`, `3 d ago`, a future `at` reading `just now`); `nothing on this
   branch since <label>` in branch scope while worktree scope still reads
   `working tree clean`; the picker's row order with quick rows, their
   filtering by label and detail; the `reviewed` row's age and `rewritten`
   detail follow `Snapshot.mark` across two renders of one open picker, its
   membership does not change with them (label-only matching); a `reviewed`
   row appearing or disappearing between two renders keeps the cursor on the
   row it was on, a change of base that reorders the ref rows does the same, a
   cursor whose row is gone clamps to the last one, and refs arriving after
   `main` was typed leave the cursor on the listed `main`, not on the last
   filtered row.
6. Input: `M` sends `MarkReviewed` with `drawn_head`, which a frame whose diff
   is `Loading` or `Failed` leaves untouched, so a `Ready` snapshot the shell
   skipped can never be marked; a frame drawn with the key sheet or the picker
   open leaves it untouched too; `head = None` clears it; with nothing drawn
   it shows the notice and sends nothing; `M` is inert while the picker
   or the key sheet is open, like every body key (7.4); an answer carrying
   `mark_error` becomes the body notice whether or not the picker is open, and
   two answers with the same error produce it twice; a mark answered while a
   pick is in flight neither closes the picker nor becomes its error; picker
   and key-sheet keys do not clear a pending urgent notice, and the first body
   key afterwards does.
7. Read-only: the G7 test of 7.6 gains a mark and a picker opening; the
   allow-list stays at ten, and the state directory afterwards holds only
   `bases.json`, `marks.json` and the lock file.
8. Tier B (ignored, real host): after the existing `b`, the test presses `M`
   and waits for `vs reviewed` on the toolbar, in both hosts.

Success criteria, in addition to 1.5 and 7.9:

9. With the viewer open on a branch where an agent has just committed once:
   reading that commit and pressing `M` empties the list, which reads
   `nothing on this branch since reviewed`; when the agent then makes a second
   commit, the list holds exactly that commit's changes plus the working tree,
   and a second `M` empties it again.
10. The mark survives closing and reopening the viewer on the same worktree;
    picking `default (...)` leaves it offered in the picker, and choosing it
    again compares against it.
11. `docs/acceptance-p1.md` gains row 8 with the same evidence columns; the
    release guard is unchanged.
