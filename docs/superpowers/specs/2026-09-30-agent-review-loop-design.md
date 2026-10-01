# Agent review loop, part 1: the target, comments and sending

Addendum to the Phase 1 design
(`2026-09-18-hunks-roadmap-p1-viewer-design.md`), its branch-scope section 7
(`2026-09-22-branch-scope-design.md`, shipped as 0.0.2), its review-marks section
8 (`2026-09-23-review-marks-design.md`, shipped as 0.0.3) and the hunk-actions
section 9 (0.0.4, in progress). It is section 10, and every term (`Scope`,
`Base`, `Comparison`, the picker, the chip, the files panel, the state directory,
the Y/N box, the read-only guarantee G7) means what those documents say it
means. The behaviour source is vimeflow `main` at `91e45b1c`, cited as
`vimeflow:<path>:<line>`, and a run of its packaged build with Codex CLI 0.158 on
2026-09-30; the interaction was settled first as a storyboard (v2) with the
owner.

## 10. Agent review loop, part 1

### 10.1 Why

Vimeflow's "review changes hunk by hunk" is a loop: the reviewer writes comments
on lines, sends them to the agent working in a terminal pane as one prompt, and
the agent's answer comes back onto the same lines. Phase 1 ported the viewer;
this section ports the first half of the loop, up to the moment the prompt has
been sent: choosing the pane, writing comments, sending them, and sending a
review request. Reading the answer back, threads, resolve and findings are the
next section; nothing here is designed in a way that would have to be redone for
them.

One thing is not a port. Vimeflow sends to the active terminal pane and to
nothing else: its resolver accepts several candidates
(`vimeflow:src/features/diff/services/activePanePicker.ts:41-66`) but its caller
passes only the pane whose diff is on screen
(`vimeflow:src/features/workspace/WorkspaceView.tsx:3072-3112`), and a review
there belongs to that pane. The viewer here is a pane of its own, opened from
another pane, beside any number of agent panes. So the pane that receives a
review is a choice the reviewer makes, and it is made before the first comment,
because a review written to no one is the failure this section exists to
prevent.

### 10.2 The target

A target is the pane a review goes to, chosen once per worktree and changeable
at any time. It is not a base and not a mark: it changes no comparison, and a
target that is gone, blocked or wrong can never stop the rows, the diffs or the
comments from loading. Most of 10.7 follows from that independence, as 8.6
followed from the mark's.

**What the viewer knows about panes.** herdr 0.8.0 lists every pane of the
session through `pane.list`, and the record of a pane that runs a coding agent
carries `agent` (`claude`, `codex`, `kimi` and the other kinds the host
detects), `agent_status` (`idle`, `working`, `blocked`, `done`, `unknown`) and
`agent_session` (the agent's own session id, when its hook reported one); a
plain shell has no `agent` key and reports `unknown` too, so `agent` decides
presence and `agent_status` never does. Both hosts answer the same (the fork is
wire-compatible, protocol 19). The viewer has never spoken to the socket until
now: `src/herdr/client.rs` served the actions. From this section the engine
calls it as well, through `spawn_blocking`, one request per connection as
today; 10.6 says how.

**The record.** `Target::Pane { pane, socket, agent, session, title }`: the
host's pane id, the socket path it belongs to (pane ids are per server run and
per socket, 10.7), the agent kind and session id as they were when the target
was chosen, and the title shown when it was chosen. `session` is
`Option<SessionRef { kind, value }>`, the host's `agent_session` minus its
`source`: `kind` is `id` or `path` (the host reports a path for a few agent
kinds instead of an id) and `value` the string that came with it. An agent the
host detected without a hook-reported session is as choosable as any other,
and its record simply has none. `Target::Clipboard` is
the other variant: a review written to be copied, not sent. The snapshot
carries `target: Option<Target>` and `target_state`:

| `target_state` | Meaning |
| --- | --- |
| `Unverified` | a pane target no verification has answered for yet: the remembered one before the first refresh reaches the host, or a fresh pick before the next |
| `Live(status)` | the pane exists, runs the same agent kind, and the session matches (below); `status` is the host's |
| `Restarted(status)` | same pane, same kind, another session: the agent was started again since the pick |
| `Left` | the pane exists but runs no agent, or another kind |
| `Gone` | the host has no such pane |
| `NoHost` | there is no socket: the viewer runs standalone, or the host went away |
| `Clipboard` | nothing to verify |

Session matching: a record with a session reference matches only the same
`kind` and `value`, so a host that now reports a different one, or none, gives
`Restarted`; a record without one matches by pane and kind alone, and when the
host later reports a session for that pane the engine adopts it, rewriting the
record with it on its next write (a pick or a comment), so continuity is
tracked from then on. Two missing references never prove continuity; they only
fail to disprove it.

**How it is chosen.** In this order, the first that applies:

1. the target remembered for this worktree in `targets.json` (below), verified
   against the host at the first refresh and re-verified at every refresh after;
2. the opener pane, `HERDR_HUNKS_OPENER_PANE`, when `pane.get` says it runs an
   agent -- the common case of pressing the keybinding inside the agent's own
   pane. An opener preselection is not written: it becomes the remembered target
   when the first comment is made under it, so a viewer opened from a shell to
   look at a diff writes nothing;
3. nothing. The chip reads `→ no agent`, and the first `i`, `I` or `v`…`i`
   opens the picker before the editor (10.3).

**The picker.** `A` opens "Send to", built like 7.4's base picker: one input
line that filters the rows, `Enter` picks, `Esc` keeps what is set (and, when
the picker was opened by a comment key, cancels that comment). Its rows come
from `Command::LoadPanes(token)`, answered on the snapshot the way `LoadRefs`
is, with the same token rule against late replies. Three groups, in this order:
panes whose `cwd` or `foreground_cwd` is the worktree's toplevel or lies under
it; every other agent pane; and `✂ clipboard · copy the review instead of
sending it`. The viewer's own pane is never a row (`HERDR_PANE_ID`, when the
viewer is a split; a popup has no id and is not listed by the host). A row shows
the agent kind, the pane id, the status word, the directory relative to `$HOME`
and the title (`label`, else `terminal_title_stripped`); the filter matches any
of them. Panes with no `agent` are not rows: the host cannot deliver a prompt to
a shell as a prompt, and the point of the picker is a pane that answers. When
the list has no agent pane at all, the picker says so, `No agent pane in this
session. Start claude, codex or kimi in a pane, then press A.`, and offers the
clipboard row alone. `Enter` on a row sends `Command::SetTarget(Target)`; the
engine writes it (below), publishes it, and the chip changes with the next
snapshot.

**The chip.** One toolbar chip after the stats, drawn from `target` and
`target_state`: `→ codex w4:p2` (accent) for `Live` with `idle` or `done`;
`→ codex w4:p2 · working` and `· blocked` (warn) and `· unknown` (dim) for the
other statuses; `→ codex w4:p2 · ?` (dim) for `Unverified`;
`→ w4:p2 · restarted` (warn) for `Restarted`; `→ w4:p2 · left` and
`→ w4:p2 · gone` (bad) for `Left` and `Gone`; `→ no host` (dim) for `NoHost`;
`→ clipboard` for the clipboard; `→ no agent` (dim) when there is no target
and a host, and `→ no host` when there is neither: host absence outranks
target absence, because the picker then has nothing but the clipboard to
offer.
Clicking it opens the picker, as clicking the scope chip toggles the scope. It
drops from the toolbar last but one, before the stats, so it stays visible at
the widths where the file pill still is.

**Re-verification.** Every refresh of 3.3 asks `pane.get` for the target pane,
in the same job as the status read, and the published `target_state` is that
answer. `NoHost` is published when the socket path is unset, when the socket
file is missing (the host removes it as it shuts down), or when the connection
is refused. Any other failure to run the check (a timeout, a malformed reply,
another socket error) keeps the previous state and sets nothing new, as a
`rev-parse` that cannot run keeps `head_seen` in 8.2; a remembered target that
has never been answered for stays `Unverified` until a check runs. A check
belongs to the selection it was started for: every `SetTarget` advances a
selection generation, the job carries it, and an answer from an earlier
generation is discarded before any comparison, so re-picking the same pane
after its agent restarted cannot be marked `Restarted` by a check that was
already in flight; the new selection is published as `Unverified` until its
own check answers. The comparison
is by pane id and socket first, then `agent`, then the session rule above: a
different session on the same pane is `Restarted`, not `Left`, because the pane
is still the one the reviewer chose, and 10.4 lets the send go ahead after a
confirmation.

**What is remembered.** `targets.json` beside `bases.json` and `marks.json`,
keyed by the canonical toplevel like both, one record per worktree, written
under the same lock through a temporary file and a rename, only under an
absolute state directory (4.7). A record is `{"pane": "w4:p2", "socket":
"/…/herdr.sock", "agent": "codex", "session": {"kind": "id", "value": "01a0…"},
"title": "…", "at": 1759…}`, with `"session": null` for an agent that reported
none, or `{"clipboard": true, "at": …}`. On load every field is checked by
shape before it is used: the pane id must match the host's grammar
(`w<digits>:p<alphanumerics>`), the socket must be an absolute path, `agent`
must be a lowercase word, `session` null or an object whose `kind` is `id` or
`path` and whose `value` is a printable string without control characters, an
absolute path when the kind is `path`; a record that fails is dropped with a
problem line, as 8.2 drops a malformed mark. A pane id is a string the host
receives as a request parameter, never as an argv element, and the socket path
is compared, never opened from the record: the viewer only ever connects to
`HERDR_SOCKET_PATH`, and a record whose socket differs is `Gone` for this host.
Without a state directory the target holds for the session, as a pick does in
7.3.

### 10.3 Comments

A comment is the reviewer's own text on a place in the diff. In this section
every comment is the reviewer's; the next section adds the agent's turns to the
same store, which is why a comment already carries the fields a thread needs.

**The record.** `Comment { id, anchor, category, text, created_at, state }`.
`id` is a random 32-hex string; once sent it is also the thread id, Vimeflow's
rule (`vimeflow:src/features/diff/hooks/useFeedbackBatch.ts:1165`). `category`
is `Question`, `Change`, `Bug` or `Suggestion`, `Change` being the default; it
decides what the agent is told (10.5). `text` is one or more lines, control
characters other than newline removed, at most 4,000 characters and 100 lines.
`state` is `Pending`, `Sending { at, nonce, item, to }`, `Unconfirmed { at,
nonce, item, to }` or `Sent { at, nonce, item, to }`: claimed by a send still
in flight, claimed by a send whose outcome is unknown, or sent, under which
nonce, as which `[#n]`, and to whom (10.4 says when each is written). `to` is
the destination as the send verified it, `{ pane, agent, session }` or
`clipboard`, not a reference to the worktree's target: the target can be
replaced after the send, and the agent that has this comment, and whose
transcript part 2 reads for the reply, is the one that had it then.

**The anchor** names a row and a line the way 7.2 and `engine::nav` name them:
`Anchor { key, side, line, span, comparison }`, with `key` the row's `FileKey`
of 7.2 (`path`, `staged`, `untracked`: a path deleted on the branch and
recreated untracked is two rows and two anchors), `side` `Additions` (new-file
numbering) or `Deletions` (old-file numbering), `span` `Line`, `Range { end }`
(same side, `line` through `end`) or `File` (then `side` is `Additions` and
`line` is 0), and `comparison` the `Comparison` the diff was loaded under when
the comment was made: `Worktree` or `Branch { merge_base, label }`, the label
being the base's label at that moment (`main`, `origin/main`, `reviewed`), kept
because 10.5 prints it and it cannot be recovered from the id later. The anchor
never moves: a comment is drawn at whatever row now carries its number under
its comparison, or nowhere when that row is gone, and it is counted and sent
either way. Vimeflow does the same, and the agent's reply is matched by item
number, not by position. A comment is drawn only while the viewer shows its
comparison kind and, in worktree scope, its half; the panel count counts it
regardless.

**Cards.** A pending comment is a card under its line: a one-line rounded
frame, the title `Bug · pending` on its top edge, the text wrapped inside at
the card's width, which is the body width minus the gutter. A range comment's
card sits under the range's last line; a file comment's card sits under the
file header row. Cards are rows of the body: they scroll with it, `j`/`k` step
over them (the cursor stays on diff targets), and the hit regions of 4.4 cover
them. Card titles use the accent colour for `Question`, the warn colour for
`Bug`, body for `Change` and `Suggestion`. A sent comment keeps its card,
titled `Bug · sent` in the dim colour: it shows what the agent already has,
and it is the card part 2's thread grows under; it is counted nowhere.

**The editor** is a card too, drawn in the place the comment's card will take:
the title `comment on R16 · Question Change Bug Suggestion` with the chosen
category in reverse video and `ctrl+h/l` at the right, one text row that grows
with the text, and `enter save · ctrl+j newline · esc cancel` on the bottom
edge. It is modal like the picker: printable keys insert, `ctrl+h`/`ctrl+l`
cycle the category, `ctrl+j` inserts a newline, `Enter` saves (whitespace-only
text is inert), `Esc` discards the text and closes, `ctrl+c` still quits the
viewer. A mouse click on a category word selects it. There are no persisted
drafts: the editor's text lives in `ViewState` until saved or discarded.

**Keys.**

| Key | Does |
| --- | --- |
| `i` | opens the editor on the cursor line; with a visual selection, on the range |
| `I` | opens the editor on the file |
| `v` | starts a visual selection at the cursor; `j`/`k` extend it on the cursor's side; in split mode `h`/`l` collapse it to the other side's row, as in Vimeflow; `Esc` ends it; `y` copies the selected lines' text (10.4's copy) and ends it |
| `u` | edits the pending card of the cursor line (the most recent one when there are several): the editor opens with its text and category, and `Enter` keeps its id and anchor |
| `U` | edits the most recent pending file comment of the selected file |
| `x` | deletes the pending card of the cursor line, without confirmation |
| `X` | deletes the most recent pending file comment of the selected file, without confirmation |

`i`, `I` and `u` with no diff row under the cursor show the notice `No diff
line selected for comment.`; `u`, `U`, `x` and `X` with no pending card there
show `No comment selected.`. `X` is not in the reserved set of 4.3 and Vimeflow
does not bind it; it pairs with `I` and `U` as the file-level form of `x`. Sent
comments are never edited or deleted: part 2 gives them threads.

**The target comes first.** `i`, `I` and `v`…`i` with `target = None` do not
open the editor: they open the picker of 10.2 with the notice `choose the pane
this review goes to`, and `ViewState` keeps the anchor the key was pressed on.
When `SetTarget` is answered, the editor opens on that anchor; `Esc` in the
picker drops it. With a target in any other state, `Gone` and `NoHost`
included, the editor opens: a target that is unreachable is a sending problem
(10.4), not a writing problem. `u`, `U`, `x` and `X` never ask for a target.

**Marks and counts.** A file with any comment under the current comparison
shows `✎` in the panel in the cell before the staged or unread marker, the name
losing one cell; the panel header reads `CHANGED 3 · ✎ 2`, counting pending
comments in the worktree across both scopes, and the footer hint reads
`Y finish (2)` while the count is positive. The cap is 50 unsent comments per
worktree -- pending, sending or unconfirmed, so a batch another viewer has
claimed still counts and a failed send can always return to `Pending` --
Vimeflow's number: the 51st shows `50 comments unsent; send or delete some
first.` and opens no editor. The cap is checked by an add alone, never by a
settlement.

**Remembered.** `comments.json` beside `targets.json`, keyed by the canonical
toplevel, one array of records per worktree. Every add, edit, delete and send
is one transaction under the lock: read the file as it is now, apply the
change by comment id to what was read (an add appends, an edit and a delete
find their id or do nothing, a send stamps the ids it sent), an add checks
the cap of 50 against what was read, write through a temporary file and a
rename, and publish the merged array. Two viewers on one worktree therefore
never lose each other's comments, and a viewer also rereads the file in each
refresh when its mtime moved, as `marks.json` is reread in 8.2. A transaction
that cannot read or write the file keeps its operation in a journal of the
viewer's own, applied by id to what the viewer shows, and every later
transaction replays the journal on top of what it read before applying its
own operation, so a write that succeeds carries the journal without undoing
anything another viewer wrote in between; a send is never claimed while the
journal is not empty (10.4). Every record is checked by shape on load: `path`
relative, not empty, no `..` component; for a `Line` or `Range` span, `line`
and `end` positive and `end` not before `line`; for a `File` span, `line` 0
and no `end`; `category` one of the four; `text` within both caps with no
control characters other than newline; a `merge_base` a 40- or 64-hex id
(`base::is_object_id`) and its `label` a printable string without control
characters; a nonce six to sixteen alphanumerics; a `to` that is `clipboard`
or whose `pane`, `agent` and `session` pass the checks 10.2 applies to those
three fields of a target record (it carries no socket). A `Sending` record
whose claim is older than 60 seconds is rewritten as `Unconfirmed`, by nonce
and only while it is still `Sending`, by whichever viewer notices first: on
load, and in every refresh of 3.3, which checks the ages of the records it
holds whether or not the file's mtime moved. The viewer that claimed it is
gone with its answer, or its host call has long since timed out (10.6). A
record that fails is dropped with a problem line in `config-problems.log`, and
the next write rewrites the file without it. Without a state directory there
is no shared store and no journal: the session's array is the store, every
transaction of this section runs on it in memory, claims and settlements
included, both kinds of send work, and the first add shows `comments not
remembered: no state directory`, once, which is also the warning that a quit
forgets what was sent. The journal is for a state directory that exists and
fails.

### 10.4 Finishing, sending and copying

**Finish.** `Y` with no pending or unconfirmed comment in the worktree shows
the notice `nothing to send: no pending comments` and opens nothing; with at
least one it opens the Finish box, a modal of 4.6's dialog kind under the
toolbar, like 7.4's picker. Its text comes from the snapshot's `target_state`;
the check that matters runs again inside the engine when the send is confirmed.

| `target_state` | The box says | Keys |
| --- | --- | --- |
| `Live(idle)`, `Live(done)` | `Send 2 comments across 1 file to codex · w4:p2?` and `codex is idle in ~/…/demo-repo` | `Y` send · `A` pick another pane · `c` copy · `n` cancel |
| `Live(working)` | `codex is working in w4:p2; the review would queue behind its current turn.` | `Y` send anyway · `A` · `c` · `n` |
| `Live(unknown)` | `codex's state in w4:p2 is unknown to the host.` | `Y` send anyway · `A` · `c` · `n` |
| `Unverified` | `w4:p2 has not been checked; it will be before sending.` | `Y` send · `A` · `c` · `n` |
| `Live(blocked)`, `Restarted(blocked)` | `codex is waiting for an approval in w4:p2. Answer it there, or press A to pick another pane.` | `A` · `c` · `n` |
| `Restarted(_)`, any other status | `codex in w4:p2 was restarted since you picked it and has not seen earlier messages.` | `Y` send anyway · `A` · `c` · `n` |
| `Left`, `Gone` | no box: the picker opens with the notice `codex · w4:p2 is gone · pick a pane` | the picker's keys |
| `NoHost` | `No host: this viewer runs outside herdr.` | `c` copy · `n` cancel |
| `Clipboard` | `Copy 2 comments across 1 file to the clipboard?` | `Y` copy · `A` · `n` |

`Esc` is `n`. `A` opens the picker over the box; a pick returns to the box
with the new target, and `Y` or `@` with no target at all opens the picker
first, as `i` does in 10.3, and returns to the box once a pane is picked.
Every other key is inert while the box is open, and the box, the picker, the
editor and the key sheet are modal one at a time. The counts are pending
comments, plus unconfirmed ones (below) named separately, `2 pending + 1
unconfirmed`, and the files they are on, across both scopes; the comments
drawn under another comparison are sent too, each labelled with its own
(10.5).

**The send.** `Y` sends `Command::Send(SendRequest { kind: Feedback, force })`,
`force` being true from a "send anyway" row. The engine, in one task:

1. verifies the target with `pane.get`, fresh, under the selection generation
   of 10.2, and applies two gates that do not know about each other. The
   continuity gate: `Left`, `Gone` and `NoHost` refuse; `Restarted` refuses
   unless `force`; `Live` passes; `Unverified` is simply what the check now
   answers. The status gate, on the status the check just returned whatever
   the continuity: `blocked` refuses, always, because the host would type the
   payload into the dialog (`herdr:src/app/api/agents.rs:62-111` checks
   nothing of the kind); `working` and `unknown` refuse unless `force`; `idle`
   and `done` pass. A check that cannot run -- a timeout, a malformed reply,
   a socket error other than the three that make `NoHost` -- refuses with
   `could not verify w4:p2: <reason>`: the chip keeps its previous state, as
   10.2 says, and a previous state never authorizes a send. A `SetTarget`
   answered since the command was issued refuses too, with `the target
   changed; press Y again`. When `force` carries a send past `Restarted`, the
   check's session becomes the target record's, so the next Finish does not
   ask again about a restart the reviewer has already accepted;
2. makes a nonce: six characters of `[a-z0-9]` from a SHA-256 of the time, the
   pid and a counter (the crate carries `sha2`, not a random number crate),
   Vimeflow's length and alphabet, and a tag rather than a secret: it tells one
   dispatch's reply from another's on the same pane. Six characters are not
   unique by construction, and settlement goes by nonce, so step 3 checks
   under the lock that no retained record of `comments.json` or
   `requests.json` carries it and advances the counter until none does; the
   maker is a `SessionConfig` seam, so a test can make it collide;
3. claims the comments in one transaction of 10.3: under the lock it reads the
   store as it is now, takes every record that is `Pending` or `Unconfirmed`,
   never one another viewer holds as `Sending` -- and when that leaves
   nothing, because another viewer finished first, it stops here and answers
   `nothing to send: another viewer sent these comments` -- in creation
   order, numbers
   them `[#1..n]`, builds the payload of 10.5 from their text as read, encodes
   the whole `agent.prompt` request, and only then writes them back as
   `Sending { at, nonce, item, to }`, `to` being the pane, agent and session
   step 1 just verified. A viewer whose journal of 10.3 is not empty claims
   nothing and refuses with `comments not saved: <reason>; fix it before
   sending`, because a claim no other viewer can read would let two viewers
   send one comment. A request longer than 512 KiB is refused
   inside the same lock, before any stamp, with `review too large to send at
   once (<n> KiB of 512): delete or shorten comments`; the caps of 10.3 keep
   an ordinary review far below it, and this bound is the one that counts,
   measured on the bytes the socket would carry, prefixes and escaping
   included. What is sent is what was stamped, and nothing else: an edit
   another viewer made a moment before is sent as edited, one made a moment
   after lands on a record that is no longer pending and is refused there. Two
   viewers that finish at once each claim what the other has not, and neither
   overwrites the other's stamps. The claim precedes the host call, as
   Vimeflow registers its correlation before it writes to the pty
   (`vimeflow:src/features/diff/Panel.tsx:938-953`), so a reply can never
   outrun it;
4. calls `agent.prompt { target: <pane id>, text }` without `wait`. The host
   wraps the text in a bracketed paste when the pane has it switched on and
   presses Enter 300 ms later; the request is one JSON line, under the host's
   1 MiB line by step 3's bound. The prompt names a pane, not a session, and
   the host offers no way to say which session the text is for, so between
   step 1's answer and this write -- two round trips on a local socket -- the
   agent can still restart or reach a dialog, and the text then lands in a
   session `to` does not name or in the dialog. The window is accepted and
   named rather than papered over: `to` records what was verified, part 2
   also has the pane id to look at when the recorded session holds no reply,
   and closing the window needs an expected-session parameter on the host's
   side, which this section asks for and does not depend on;
5. settles the claim by what the socket said, in a second transaction that
   touches only records still carrying this claim's nonce, so a claim made in
   between by another viewer is never settled by this one. A definite answer
   -- an error before the request was written in full (connection refused, a
   write error), or the host's own error reply (`agent_not_ready`,
   `agent_not_found`, `agent_prompt_failed`) -- returns the records to
   `Pending` and answers with the message. A success reply stamps them `Sent`.
   An uncertain outcome -- the request was written and no reply came within
   the client's deadline (10.6) -- stamps them `Unconfirmed`: the agent may
   well have the text, and a lost correlation is the costlier mistake;
6. answers on the snapshot: `send_seq` advances once per request, `send_error`
   carries the refusal or the host's message, and a success publishes the
   stamped comments.

An unconfirmed record is drawn with the title `Bug · sent?`, is counted
neither as pending nor as sent, and is claimed again by the next Finish along
with the pending ones (the counts name it), under the new nonce: the agent may
receive a comment twice, which it can see from the text, and the next section
matches its reply by the latest nonce. A `Sending` record is drawn `· sending`
and claimed by no one; 10.3's load rule turns one that outlived its viewer
into `Unconfirmed`.

The box reads the answer by `send_seq` as the picker reads `pick_seq` in 7.4:
success closes it and shows the notice `sent 2 items to codex · w4:p2`; a
refusal keeps it open with the reason in place of its first line, and when the
reason is one that `force` overrides -- working, unknown, restarted -- its `Y`
row now reads `send anyway` and the next `Y` carries `force`; `A` or `c` is one
key away in every case. Focus stays in the viewer: a popup viewer would close
if focus left it, and the agent's pane is one keybinding away in the host.

**Copy.** `c` in either box, `Y` on a clipboard target, and `y` on a visual
selection write text to the terminal as an OSC 52 sequence (`ESC ] 52 ; c ;
<base64> BEL`), the only clipboard a terminal program has, and also to
`clipboard.md` in the state directory, overwritten each time, because whether
the host forwards OSC 52 from a plugin pane to the outer terminal is unknown
until the plan's first task tries it, and a terminal that drops the sequence
still leaves the file. The notice names both: `copied 2 comments · also in
~/.local/state/herdr-hunks/clipboard.md`. `c` beside a pane target keeps the
comments pending, as in Vimeflow; `Y` on the clipboard target is a send whose
host is the clipboard: the comments are claimed as in step 3, with `to =
clipboard`, and settled by what the engine can know, which is the file: a
terminal never acknowledges an OSC 52 write, and the shell's write to stdout
is not reported back (a stdout that fails ends the viewer, 4.6). When
`clipboard.md` was written the records are stamped `Sent` with the nonce the
text carries, the count clears, and a reply pasted back later can still be
matched by part 2; when the file could not be written, or there is no state
directory, and the text is within the OSC 52 limit, the sequence goes out
alone and they are stamped `Unconfirmed`, `sent?` until the next Finish copies
them again. OSC 52 payloads above 100,000 bytes are not written (terminals cut
them), the file alone is, and the notice says so. When the file could not be
written and the text is over that limit, nothing can receive it: nothing is
stamped, the comments stay pending, and the box says `nothing could receive
the copy: <reason>`.

**Request review.** `@` opens the Request box: `Scope  f this file   a all
changes (4)` on its first line, the scope in reverse video; `Delegate a review
of <scope> to codex · w4:p2?` with the target's state line as in the Finish
box; `Y` delegate · `A` · `c` copy · `n` cancel. The scope starts on all
changes; `f` is unavailable with no diff loaded and `a` with an empty list, and
while the chosen scope is unavailable `Y` is not offered. With an empty list
the box reads `nothing to review` with `n` alone; with exactly one row whose
diff is loaded the two scopes coincide and the scope line is not drawn, and
with one row whose diff is not loaded it is drawn with `f` unavailable. The
request goes
through the same six steps with `kind: Review { scope }`, under the same
512 KiB bound, except that no comment is claimed: step 3 instead records the
request in `requests.json` beside `comments.json`, under the lock, as `{
nonce, at, target, files: [{ key, comparison, additions: [[start, end]…],
deletions: [[start, end]…] }] }`, `key` being the row's `FileKey` of 7.2 as
in 10.3's anchors (both halves of a partially staged file are two entries),
and the ranges the hunks of each file in scope as they are now, which is the
snapshot the next section anchors findings against, as Vimeflow's
`pendingReviewRequests` does. `f` takes the selected row's ranges from its
loaded diff, which its availability rule guarantees. `a` needs no loaded diff:
for every row whose diff is not `Ready` in the snapshot, the selected one
included, the task loads it the way 3.3 and 7.2 load a selected row -- the
frozen `get_git_diff_inner` in worktree scope, `branch::diff` and
`branch::untracked_diff` in branch scope, one row after another, each under
the frozen 30 s timeout -- and keeps only the hunk headers. Those are the
commands the allow-list of 7.6 already admits, so it does not grow; a row
whose diff fails is recorded with no ranges and listed in the prompt all the
same, and a request is never refused for one unreadable row. The reads come
first, before step 1: the check of the target is then made with the payload
ready and the dispatch a moment away, so an agent that was answered or
restarted during a long read refuses the send, and a `SetTarget` during it
refuses with `the target changed; press @ again`; and the record is written
after the reads, so a request is never recorded with ranges it does not have.
`target` in the record is the destination step 1 verified, as `to` is in
10.3. Refusals, `A` and `c` behave as in the Finish box; success closes it
with `review requested from codex · w4:p2`.

### 10.5 What is sent

Both prompts are Vimeflow's, so that an agent, a reply parser and a reviewer
who knows one tool know the other. Every line is a Markdown quote, which is
why a reply parser accepts `>` in front of its markers. Before anything of the
reviewer's or the repository's enters the text, control characters
(`0x00`–`0x1F` and `DEL`) are removed from paths, comment lines and labels,
newline excepted where it separates the lines of a comment: a file name or a
comment could otherwise carry a bracketed-paste terminator or a carriage
return into the agent's input
(`vimeflow:src/features/diff/services/feedbackDispatch.ts:14-28`). The host
adds nothing and strips nothing (10.4).

**The review.** Items in creation order, numbered as the claim numbered them:

```
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
> status is one of: "reply" (answers a question), "clarify" (you need the user to answer — the thread awaits them), "resolved" (you made the change), "deferred" (punted for later; cite the issue # in text), "rejected" (declined).
> <<<VIMEFLOW_REPLY
> {"v":1,"nonce":"oqzpww","replies":[{"id":1,"status":"reply","text":"..."}]}
> VIMEFLOW_REPLY>>>
```

The item line is `[#n · <label>] <absolute path><place> (<side>)
[<comparison>]`: the label is `Question`, `Change request`, `Bug` or
`Suggestion`; the place is `:<line>`, `:<start>-<end>` or ` (file)`; the side
is `additions` or `deletions`; the comparison is `unstaged`, `staged` or `vs
<base label> @ <7 hex of the merge-base>`, the label being the one the anchor
stored. The path is absolute, `<toplevel>/<path>`, as Vimeflow sends it
(`vimeflow:src/features/diff/Panel.tsx:880-888`), and every git command the
prompt quotes carries `-C '<toplevel>'`, because an absolute path alone selects
no repository: the agent may run in another worktree or another repository
altogether (10.2 lets it), and `git -C` is what makes the command show the same
diff from anywhere. The toplevel is single-quoted with any quote inside it
written as `'\''`. One `> ─ ` line per line of the comment, then the
instruction of its category: `Answer inline in your reply. Do not edit files.`,
`Make this change.`, `Fix this.`, `Apply this if you agree.` The merge-base
line of the footer is printed only when at least one item is a branch-scope
item; the rest of the footer is verbatim Vimeflow. The agent's reply block is
read by part 2; this section only promises that the nonce in it is the one the
claim recorded.

**The review request.** The scope's files in two or three groups, then the
finding contract:

```
> Delegate a code review of these 3 changes:
> unstaged diff (`git -C '/home/will/demo' diff`):
> ─ src/cart.py (/home/will/demo/src/cart.py)
> ─ notes.txt (/home/will/demo/notes.txt) (untracked — not in git diff; read the file, all lines are additions)
> staged diff (`git -C '/home/will/demo' diff --cached`):
> ─ src/util.py (/home/will/demo/src/util.py)
>
> Anchor each finding with diff-side line numbers: "additions" uses new-file lines, "deletions" uses old-file lines.
> …
> <<<VIMEFLOW_REVIEW
> {"v":1,"nonce":"wvpx71","reviewer":"<your name>","findings":[{"path":"<file>","scope":"line","side":"additions","line":1,"category":"bug","text":"..."}]}
> VIMEFLOW_REVIEW>>>
```

In branch scope the groups are one, `branch diff (`git -C '<toplevel>' diff
<merge-base id>`):`, untracked rows marked as above; in worktree scope they are
Vimeflow's two with the same `-C`, the one departure from Vimeflow's text, for
the reason given above. The lines from `Anchor each finding` to the closing
marker are the contents of
`vimeflow:src/features/diff/prompts/delegated-review.prompt.md` with
`{{NONCE}}` replaced, carried in the crate as
`src/engine/prompts/delegated-review.md` and compared byte for byte with the
pin by `scripts/port-check.sh`, the gate that has the reference checkout, so
the contract and its parser cannot drift apart.

**Why the names stay.** `VIMEFLOW_REPLY` and `VIMEFLOW_REVIEW` are not
renamed. The parsers that read them exist twice already, in vimeflow's backend
and in herdr-agent-watcher's port of it, and part 2 will take one of the two
unchanged (6.3 reserved exactly this); the agent does not care what the
markers are called, and a reviewer who moves between the desktop app and the
plugin meets one contract.

### 10.6 Engine, host and view

**Commands and snapshot.** `Command` gains `LoadPanes(u64)`,
`SetTarget(Target)`, `AddComment { anchor, category, text }`,
`EditComment { id, category, text }`, `DeleteComment { id }`,
`Send(SendRequest)` and `Copy(CopyRequest)`. The snapshot gains `target`,
`target_state`, `target_seq` and `target_error` (one answer per `SetTarget`: a
record that could not be written is kept for the session and the error says
so, as 7.3's pick); `panes: Option<Arc<Vec<PaneRow>>>` and `panes_seq`, the
picker's rows under the opening's token as `refs` and `refs_seq` are in 7.4;
`comments: Arc<Vec<Comment>>`, `comment_seq` and `comment_error` (one answer
per add, edit or delete); `send_seq` and `send_error`; `copy_seq` and `copy:
Option<Arc<CopyOut>>`, the text the shell writes as OSC 52 once per
`copy_seq`, with the notice to show. Every new field joins `fingerprint`, or a
change to it alone would never be published (AGENTS.md).

**The host client.** `engine::host` declares `trait HostClient { pane_get,
pane_list, agent_prompt }`, injected through `SessionConfig` as the watcher
and the git check are, so engine tests run against a fake that records every
request and answers from a script, and the production value wraps
`herdr::client::HerdrClient` -- the actions' client, unchanged in protocol:
one request per connection, a three-second read timeout -- in
`spawn_blocking`. Two changes in `src/herdr/client.rs`: `HerdrClientError`
says whether the failure came before or after the request line was written in
full, which 10.4's settlement reads, and one request gets one deadline, five
seconds from its start, instead of a timeout per read. The connection is made
non-blocking and polled against the deadline; every read and write timeout
is set to the time the deadline has left before the call; the deadline is
checked between calls; and a line that is not complete when it passes fails
the request. A socket timeout bounds one call, not the exchange, and a host
that answered a byte every two seconds would otherwise hold a request open
for as long as it liked. The engine, for its part, waits at most ten seconds
on the blocking call before it treats the outcome as uncertain, and an answer
that arrives later is settled by nonce like any other, so 10.3's 60-second
claim expiry is six times the longest a send can run: a viewer whose send is
still running can never be mistaken for one that is gone. Nothing else about
the client moves, and the actions keep the same client, with a deadline where
they had a read timeout. The
socket path is `HERDR_SOCKET_PATH`; unset, the client is `None` and every
pane target is `NoHost`, while `Target::Clipboard` is `Clipboard` with or
without a host. The three methods are the whole host surface of this section:
the engine never sends keys, never reads a pane, never focuses anything.

**Where the work runs.** The target check of 10.2 rides the refresh job of
3.3: after the status read, the job calls `pane_get` for the target under the
selection generation it was given, and `Done::Status` carries the answer
beside the rows, so the chip and the list change in one frame. `LoadPanes` is
its own task answering `Done::Panes { token, rows }`, as `LoadRefs` answers
`Done::Refs`. `Send` and `Copy` are their own tasks too, one at a time: a
second `Send` while one is in flight is refused with `a send is in progress`
rather than queued, because a claim must settle before another claim can read
the store. The comment transactions run in `spawn_blocking` under 7.3's lock,
as `save_pick` and `save_mark` do today; 3.3's refresh never waits on them.

**The store module.** `engine::comments` owns `comments.json` and
`requests.json`: the records, the shape checks, the transaction of 10.3 (read,
apply by id, check the cap and the request bound, write, return the merged
array), the `Sending`-to-`Unconfirmed` rule, and the mtime watch;
`engine::target` owns `targets.json`, the resolution order and the
verification compare; `engine::prompt` builds the two texts of 10.5 and
carries the delegated-review file. The store and the prompt builder never
call git; the one task that does is an all-changes review request, which
loads the other rows' diffs through the loaders 3.3 and 7.2 already use
(10.4). The toplevel, the comparison and the rows come from the snapshot the
command was issued against.

**The view.** `tui::rows` gains `Row::Card { comment, lines }`, built after
the row the anchor names, after the file header for a file comment, after the
range's last line for a range comment; `rows::build` interleaves them from
the snapshot's comments filtered by the current comparison, and
`row_of_target` is unchanged because cards are never targets. The rebuild
rule of 4.8 widens with them: `ViewState::reconcile` rebuilds when the diff's
`Arc`, the mode, the comments' `Arc` or the body width changed, the last
because a card's lines are wrapped at build time, and `Arc::ptr_eq` on the
comments keeps the unchanged case free, as it is for the diff. `view.rs` draws
a card as a rounded frame in the body's columns past the gutter, the title on
the top edge, and a hit over its rows that moves the cursor to the anchor's
line. The editor and the visual selection are `ViewState` modes beside the
picker and the key sheet; the Finish and Request boxes are `dialog::Panel`
values with entry rows for their keys, drawn where the picker is drawn.
`tui::picker` takes its rows from `panes` as well as from `refs`, the quick
rows of 8.4 replaced by the three groups of 10.2; `keys.rs` gains the bindings
of 10.3 and 10.4, the key sheet lists them, and `RESERVED` shrinks to `/`:
search is the one Vimeflow key still unbound. The toolbar gains the target
chip (10.2) and the files panel the marks of 10.3.

**Keys that change meaning.** `y` inside a Y/N box of section 9 means yes;
outside one it copies a visual selection and is inert without one, as in
Vimeflow; `n` in a box means no and in the diff moves to the next file; `c` is
bound only inside the two boxes. The routing is by mode, as 4.3 routes the key
sheet: a box, the picker, the editor and the key sheet each own their keys
while open.

**The read-only guarantee.** The git allow-list of 7.6 does not change: the
only git this section runs is the diff loads of an all-changes request, through
the loaders 3.3 and 7.2 already use, and the engine's writes stay in the state
directory. `tests/readonly_guarantee.rs` gains the fake host of `tests/support`
so the engine's socket traffic is seen: the test runs a session that picks a
target, writes a comment, finishes, copies with `c` and requests a review of all
changes, and asserts that the only methods the host saw are `pane.list`,
`pane.get` and `agent.prompt`, that `agent.prompt` was called exactly once per
confirmed send with the text the test expects, that every git command was one
of the ten, that the state directory afterwards holds `bases.json`,
`marks.json`, `targets.json`, `comments.json`, `requests.json`, `clipboard.md`
and the lock file and nothing else, and, as before, that the index, the refs
and the worktree are byte for byte what they were. The fake answers `pane.get`
from a table the test edits mid-run, which is how the `Gone`, `Left`,
`Restarted` and `blocked` paths of 10.4 are exercised without a host.

**Standalone.** `herdr-hunks [PATH]` outside a host has no socket: the chip
reads `→ no host`, the picker offers the clipboard alone, comments and copy
work in full, and nothing else differs. The fork's bundling design
(2026-09-27) rules out fork-side changes to the plugin, and nothing here needs
one: both hosts answer the three methods alike.

### 10.7 Failure modes, cost and configuration

Additions to the tables of 5.1, 7.8, 8.6 and 9's. No row of this table touches
the rows, the diffs, the base or the mark: a target is not a base, and a
comment is a record beside the diff, never in it.

| Condition | Behaviour |
| --- | --- |
| `HERDR_SOCKET_PATH` unset, the socket file missing, the connection refused | pane targets are `NoHost`; the chip reads `→ no host`; the Finish box offers `c` and `n`; comments and the clipboard target are unaffected |
| `pane.get` times out, or answers something the client cannot read | the previous `target_state` stands, nothing new is published, the next refresh retries; `Unverified` stays `Unverified` |
| the target pane closes | `Gone` at the next refresh; `Y` and `@` open the picker with `codex · w4:p2 is gone · pick a pane` |
| the agent in the target pane exits, or another kind takes the pane | `Left`; the same |
| the agent is started again in the pane | `Restarted`; the Finish box asks before sending |
| the agent is at an approval or a question | `blocked`; the Finish box refuses and names the pane |
| `pane.list` fails while the picker is open | the picker reads `could not list panes: <reason>` above the clipboard row, which stays; `Esc` keeps the current target |
| `targets.json` is unreadable or malformed, or a record fails its shape | treated as absent with a problem line; the next pick rewrites it |
| `targets.json` cannot be written | the target holds for the session; `target not remembered: <reason>` |
| `comments.json` or `requests.json` is malformed, or a record fails its shape | the record is dropped with a problem line; the next transaction rewrites the file without it |
| a comment transaction cannot read or write the store | the operation goes to the viewer's journal (10.3) and the notice `comments not remembered: <reason>` shows once; every later transaction replays the journal on what it read, so a write that succeeds carries it without undoing another viewer's work; while the journal is not empty `Y` claims nothing, `comments not saved: <reason>; fix it before sending`, and `c` still copies |
| the send-time check of the target cannot run | the send is refused with `could not verify w4:p2: <reason>`; the chip keeps its previous state, which authorizes nothing |
| the target is replaced while a send or a request is being prepared | refused with `the target changed; press Y again` (or `@`); nothing is claimed or recorded |
| the host refuses a prompt (`agent_not_ready`, `agent_not_found`, `agent_prompt_failed`), or the request could not be written in full | the claim returns to `Pending`; the box shows the host's words; the chip re-verifies at the next refresh |
| the host accepted the request and did not answer in time | `Unconfirmed`; the box closes with the urgent notice `sent? the host did not confirm · 2 comments marked sent? until the next finish` |
| the viewer dies between the claim and the answer | `Sending` becomes `Unconfirmed` after 60 seconds, found by any viewer |
| a second `Y` while a send is in flight | `a send is in progress` |
| the 51st unsent comment, counting pending, sending and unconfirmed ones across every viewer | `50 comments unsent; send or delete some first.`; no editor; a failed send returns its records to `Pending` whatever the count |
| a comment reaches 4,000 characters or 100 lines | the editor takes no more input and its title reads `comment limit: 4,000 characters, 100 lines` |
| the encoded request would exceed 512 KiB | refused before any claim, with the size (10.4) |
| a file of an all-changes request has no loadable diff, the selected one included | listed in the prompt without ranges; the request goes out |
| OSC 52 is dropped by the terminal or the host | `clipboard.md` has the text and the notice names it; a clipboard send with no file written is `Unconfirmed` |
| the base moves (a merge into `main`, a fetch) under branch-scope comments | the anchors keep the merge-base they were made against and are drawn at their numbers; the prompt names that merge-base, so the agent sees what the reviewer saw |
| a comment's row leaves the list (the file was committed, the hunk discarded) | the card is not drawn; the comment is counted and sent; `✎` leaves the panel with the row |
| a popup viewer below 40×10 | 4.2's size notice; nothing of this section is drawn |

Cost. One `pane.get` per refresh while a pane target exists, over the socket,
in the job that already runs the status read, so a refresh is one round trip
longer, and one more per send; one `pane.list` per picker opening; one
`agent.prompt` per send; one rewrite of `comments.json` per add, edit, delete,
claim and settlement; one `git diff` per row of an all-changes request whose
diff is not already loaded, headers only, each under the frozen timeout. The
poll interval and the git allow-list are unchanged.

Configuration is unchanged: no new keys. A target and a review are state, not
preference, and the state directory rule of 4.7 covers `targets.json`,
`comments.json`, `requests.json` and `clipboard.md` as it covers `bases.json`
-- written only under an absolute state directory, never relative to the
repository.

The version becomes 0.0.5. The roadmap table of 1.2 is amended to show the
order decided on 2026-09-30, section 9 then this section, and AGENTS.md's
first rule, which keeps comments and agent dispatch out of Phase 1, is
rewritten when this section lands.

### 10.8 Boundaries

**With section 9 (0.0.4).** This section lands on it: the body rows it draws
cards between are the rows section 9's `s` and `d` act on, and the two never
meet on a row, because an action changes the diff and a card is drawn at a
number. The Y/N box's `y` and `n` are the convention the Finish and Request
boxes follow, but those boxes are their own panels. In branch scope section
9's keys show their notice (7.7) and everything here works; in worktree scope
both do.

**With part 2 (section 11).** What this section leaves for it, and promises it:
every sent comment carries the nonce and item number its prompt carried and the
pane, agent and session the send verified, so a reply block can be matched by
`(nonce, id)` and the transcript it is read from is that agent's, whatever the
worktree's target is by then, the pane id standing by for the one window 10.4
names; `requests.json` holds the same destination and the hunk ranges a review
was requested against, so findings can be anchored as Vimeflow anchors them;
`Unconfirmed` comments are settled by the reply that names their nonce. Part 2
adds the agent's turns to the store with the sent comment's id as their thread,
the thread cards of the storyboard, `i` to reply inside one and `x` to resolve
it, reviewer findings as cards, and the reading itself, from the agent's
transcript and never from its screen, through one of the two existing parsers.

**Not in this section.** Persisted drafts (the editor's text is lost on `Esc`
and on quit); `/` search; moving an anchor when the file changes; editing or
deleting a sent comment; a key that jumps to the agent's pane; host
notifications; any read of a pane's screen; a target per pane rather than per
worktree.

### 10.9 Tests and success criteria

Test layers, added to 5.2, 7.9, 8.7 and section 9's. The engine tests run
against the fake `HostClient` of 10.6, which records every request and answers
`pane.get` and `pane.list` from a table the test edits mid-run:

1. Engine, the target: `SetTarget` writes `targets.json` under the lock,
   publishes `Unverified`, and the first refresh after it publishes `Live` with
   the table's status; every state of 10.2 is reached by editing the table
   between refreshes -- the same pane with another `agent_session` is
   `Restarted`, with no `agent` or another kind `Left`, a `pane_not_found`
   error `Gone`, and a record without a session reference is matched by pane
   and kind alone; a `pane.get` timeout or a malformed reply keeps the
   previous state and publishes nothing; a socket path that is unset, a socket
   file that is missing and a connection that is refused are each `NoHost`
   while a clipboard target stays `Clipboard`; a check answered after a
   `SetTarget` for the same pane is dropped, so the re-pick is `Unverified`
   until its own check; the resolution order: a remembered record beats the
   opener pane, an opener pane that runs an agent is preselected and not
   written until the first comment, an opener pane that runs a shell is
   nothing; a record whose pane id, socket, agent or session fails its shape
   is dropped with a problem line; a record whose socket differs from
   `HERDR_SOCKET_PATH` is `Gone`; with no state directory the target holds for
   the session and `target_error` says so.
2. Engine, the picker's rows: `LoadPanes` lists the table's panes in the
   three groups of 10.2, this worktree's first by `cwd` or `foreground_cwd`,
   without the viewer's own pane and without panes that report no `agent`,
   the clipboard row last; a host with no agent pane answers the clipboard row
   alone with the no-agent line; a `pane.list` failure answers its reason and
   the clipboard row; a reply under an earlier token is dropped whole.
3. Engine, the store: `AddComment` appends under the lock and publishes the
   merged array, and an add by a second viewer between two of this viewer's
   writes survives; `EditComment` and `DeleteComment` find their id or do
   nothing, and refuse a record that is no longer `Pending`; the cap counts
   unsent records as the file holds them, so two viewers at 49 cannot both
   add, an add is refused while another viewer's 50 are `Sending`, and a
   settlement that returns those 50 to `Pending` beside new ones is not;
   every shape rule of 10.3 is exercised by one malformed record (`..` in the
   path, `end` before `line`, a `File` span with a line, a fifth category, a
   control character in the text, a `merge_base` that is not an object id, a
   two-character nonce), each dropped with a problem line and absent from the
   file after the next transaction; a `Sending` record older than 60 seconds
   becomes `Unconfirmed` on load and in a refresh whose mtime did not move,
   and a younger one does not; a file whose mtime moved is reread in the next
   refresh; with no state directory the comments hold for the session, the
   notice shows once, and a claim and its settlement run in memory so both
   kinds of send work; a transaction against a store that cannot be written
   journals its operation and the viewer shows the result; when the store
   can be written again after another viewer added and deleted comments in
   between, the next transaction's write holds the journal's operations and
   the other viewer's both; `Send` refuses while the journal is not empty.
4. Engine, sending: with the fake accepting, `Send` claims the `Pending` and
   `Unconfirmed` records in creation order, never one another viewer holds as
   `Sending`, stamps them `Sending` before the host is called (the fake reads
   the file when `agent.prompt` arrives), calls `agent.prompt` once with the
   pane id and the text of 10.5 and without `wait`, and stamps them `Sent`
   under the nonce the text carries, with `to` the pane, agent and session
   the check returned; a comment another viewer edited after the claim is
   refused there and sent as claimed; each gate of 10.4: `Left`, `Gone` and
   `NoHost` refuse, `blocked` refuses with and without `force`, `working`,
   `unknown` and `Restarted` refuse without `force` and send with it, a
   forced send past `Restarted` rewrites the target record's session, the
   status is the one read at the send, not the chip's, a check that cannot
   run refuses with `could not verify`, and a `SetTarget` answered between
   the command and the check refuses with `the target changed`;
   `agent_not_ready`, `agent_not_found`, `agent_prompt_failed` and a refused
   connection return the claim to `Pending` with the host's words in
   `send_error`; a request written in full with no reply within the deadline
   stamps `Unconfirmed`; the settlement touches only records carrying this
   claim's nonce; a second `Send` while one is in flight answers `a send is in
   progress`; a claim that finds every eligible record taken by another
   viewer sends nothing and answers `nothing to send: another viewer sent
   these comments`; a request over 512 KiB, measured on the encoded line, is
   refused before any stamp and names the size; the nonce is six `[a-z0-9]`,
   and one the seam makes equal to a retained record's is passed over for the
   next; a clipboard target is `Sent` when `clipboard.md` was written,
   `Unconfirmed` when the file could not be written and the text is within
   the OSC 52 limit, and nothing is stamped when the file could not be
   written and the text is over it, with `nothing could receive the copy`;
   `c` stamps nothing; a text above 100,000 bytes with a written file yields
   a `CopyOut` with the file and the notice and without the sequence.
5. Engine, the request: `Send` with `kind: Review` claims no comment and
   writes `requests.json` with the nonce, the target and one entry per row in
   scope with the hunk ranges of its loaded diff, both halves of a partially
   staged file as two entries, an untracked row as one range over the file,
   a row whose diff fails recorded with no ranges and named in the prompt all
   the same, a selected row whose diff is `Failed` loaded by the task like
   the others; the reads precede the check and the write, so a table edit to
   `blocked` while they run refuses the request and nothing is recorded;
   `target` is the destination the check returned; `f` with no loaded diff
   and `a` with an empty list are refused; in branch scope the ranges are
   against the merge-base of the snapshot the command was issued against;
   the git commands the task ran are allow-listed ones and nothing else.
6. Engine, the prompts: the review text of 10.5 for one `Line`, one `Range`,
   one `File`, one `Deletions` and one branch-scope item, against a fixture
   byte for byte: the absolute path, the place, the side, `[vs main @
   <7 hex>]`, one `> ─ ` line per comment line, the four instructions, the
   footer with and without the merge-base line, and `-C` with a toplevel that
   contains a quote; the request text in worktree scope with its three groups
   and in branch scope with its one; control characters in a path, a label
   and a comment are removed and a newline inside a comment is kept; the
   `VIMEFLOW_REPLY` nonce in the text is the one stamped on the records;
   `src/engine/prompts/delegated-review.md` equals the pinned Vimeflow file
   byte for byte, checked by `scripts/port-check.sh` against the reference
   checkout, and the request text equals it with `{{NONCE}}` replaced.
7. Host client: `HerdrClientError` says whether the failure came before or
   after the request line was written in full -- a missing socket file and a
   refused connection before; a server that reads the line and never answers
   after; a server that stops reading before -- on a real Unix socket in
   `tests/support`, with the three methods' parameters as the host documents
   them; a server that answers one byte every two seconds fails at the
   deadline and not later, as does one that accepts and never reads; the
   engine's wait on the call yields `Unconfirmed` at ten seconds, and an
   answer after it settles the records by nonce.
8. View: the chip for every `target_state` and status, its colour and its
   place in the toolbar's drop order; cards under a line, a range's last line
   and a file header, wrapped at the card width, the four title colours, and
   the titles `· pending`, `· sending`, `· sent?` and `· sent`; a comment
   whose row is gone draws nothing and is counted; cards drawn only under
   their comparison kind and, in worktree scope, their half; the editor with
   each category selected, a multi-line text, a narrow pane, and the limit
   title at either cap; `✎` in the panel and `CHANGED 3 · ✎ 2`; `Y finish
   (2)`; the Finish box for each row of 10.4's table, the `send anyway`
   relabel after a refusal, the Request box with the scope line present,
   present with `f` unavailable, absent and `nothing to review`; the picker's
   three groups and its failure line; the key sheet's new rows; `→ no host`
   in a standalone viewer with and without a remembered pane target, and
   `→ clipboard` there with a clipboard one; a 40×10 popup shows the size
   notice.
9. Input: every binding of 10.3 and 10.4; `i` with no target opens the picker
   and keeps the anchor, `Esc` there drops it, `Enter` on a row opens the
   editor on it; `v` with `j`/`k`, `h`/`l` in split mode, `Esc` and `y`; `u`
   and `x` take the most recent pending card of the line and `U` and `X` the
   most recent pending file comment; `y` is inert without a selection; `n`
   cancels a box and moves to the next file outside one; `c` is bound only
   inside the boxes; `/` is the whole reserved set and is inert; a box's
   other keys are inert while it is open; the picker and the key sheet leave
   an urgent notice pending; `ctrl+c` quits from the editor; the table length
   of `keys.rs` is pinned again.
10. Read-only: the G7 test of 7.6 and 8.7 gains the fake host and the script
    of 10.6 (pick, comment, finish, copy, request all changes) and asserts
    that the host saw only `pane.list`, `pane.get` and `agent.prompt`,
    `agent.prompt` once per confirmed send with the expected text; that every
    git command the session ran, the request's diff loads included, is one of
    the ten; that the state directory holds exactly `bases.json`,
    `marks.json`, `targets.json`, `comments.json`, `requests.json`,
    `clipboard.md` and the lock file; and that the repository is byte for
    byte what it was.
11. Tier B (ignored, real host): after the existing `b` and `M`, the test
    presses `A` in a session whose only other pane is a shell, asserts the
    picker's no-agent line and the clipboard row through the real host's
    `pane.list`, picks the clipboard, writes a comment with `i`, presses `Y`,
    and asserts `clipboard.md` holds the prompt of 10.5, in both hosts. A
    live agent is left to the acceptance rows: CI has none.

Success criteria, in addition to 1.5, 7.9, 8.7 and section 9's:

12. In a herdr session with a codex pane beside the viewer, `A` lists that
    pane first; `i` on a changed line asks for the target only while none is
    set; after `Y` the codex pane receives one pasted message whose text is
    the prompt of 10.5, Enter is pressed, codex answers it, the viewer shows
    `sent 1 item to codex · w4:p2` and the card reads `· sent`.
13. Closing the codex pane turns the chip `· gone` within one refresh and `Y`
    opens the picker; starting codex again in the same pane turns it
    `· restarted` and `Y` asks before sending; a codex waiting at an approval
    is `· blocked`, and `Y` refuses until it is answered there.
14. A comment made in one viewer appears in a second viewer on the same
    worktree at its next refresh, with the same count in both; closing and
    reopening the viewer keeps the target and the comments.
15. `@` on all changes puts a review request in the codex pane that names
    every changed file with its `-C` command, and codex's reply ends with a
    `VIMEFLOW_REVIEW` block whose nonce is the one in `requests.json`.
16. Outside a host (`herdr-hunks .` in a plain terminal) the chip reads
    `→ no host`, the picker offers the clipboard, and `Y` puts the review on
    the clipboard and in `clipboard.md`.
17. `docs/acceptance-p1.md` gains the row after section 9's with the same
    evidence columns; the release guard is unchanged.
