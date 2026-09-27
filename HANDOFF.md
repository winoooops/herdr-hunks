# Handoff

Where this project stands, what must not be broken, and how work gets done here.
`AGENTS.md` is the rules; this file is the context behind them. Read both before
starting anything non-trivial.

Last updated 2026-09-26, after v0.0.3.

## Where it stands

A read-only git hunk viewer that runs as a herdr plugin pane (`winoooops.hunks`),
extracted from vimeflow's hunk-review feature. Phase 1 of a five-phase roadmap
(§1.2 of the P1 spec); every phase is its own spec → plan → release cycle, and the
next spec starts only after the previous phase ships.

| Version | Ships | Spec |
| --- | --- | --- |
| v0.0.1 | The Phase 1 viewer: changed files, diffs, hunk/line/file navigation, toolbar, live refresh | `specs/2026-09-18-hunks-roadmap-p1-viewer-design.md` §1-6 |
| v0.0.2 | Branch scope: `b` toggles worktree ↔ branch, `B` picks a base, `bases.json` | `specs/2026-09-22-branch-scope-design.md` §7 |
| v0.0.3 | Review marks: `M` marks a commit reviewed, `●` on rows a later commit touched, `marks.json`, quick picker bases | `specs/2026-09-23-review-marks-design.md` §8 |

`main` is released as v0.0.3. 399 tests pass (`cargo test --locked`), one further
test is `#[ignore]`d because it needs a real host (see "Tier B" below). The version
stays on `0.0.x` until the Phase 2 write actions ship — the owner has said so
explicitly; do not propose `0.1.0` or semver reasoning about it.

Installed hosts, as of this writing: upstream herdr 0.8.0 runs
`github:winoooops/herdr-hunks@v0.0.3`; the vimeflow fork host runs
`local:/home/will/projects/herdr-hunks` (also 0.0.3) because
`vimeflow plugin install` rejects the GitHub source with `invalid_plugin_source`
— a fork-side installer problem, not ours.

## Three invariants, and why they exist

These are the things a well-meaning change can quietly destroy. Each cost several
review rounds to get right; the reasoning is here so it does not have to be
rediscovered.

**1. The read-only guarantee (G7).** The viewer may run exactly ten git
subcommands: `--version rev-parse status diff ls-files show cat-file symbolic-ref
merge-base for-each-ref`. Every revision is one argv element, never interpolated
into a string, and **anything read back from a file is validated by shape before it
can become an argument** — a hand-edited `marks.json` holding `--output=tracked.txt`
would otherwise become a `git diff` option that writes a file. That is what
`base::is_object_id` is for. `tests/readonly_guarantee.rs` proves it with a
recording git wrapper that asserts the subcommand of *every* invocation, plus
before/after comparison of the `.git/index` bytes, `for-each-ref` output and a
content hash of the worktree, and an exact list of the files the viewer may write
to the state directory; when you add a git call, make sure the test actually
*reaches* it (0.0.3
shipped a hole here: the test marked in worktree scope, where classification never
runs, so the two commands the feature added never entered the log).

**2. A commit is markable only when everything loaded on screen was read at it.**
Marking a commit the reader has not seen is the one failure that matters: files
scroll past with no dot and are never reviewed. Three mechanisms enforce it, and
all three are load-bearing:

- the diff task brackets its read with `rev-parse` before and after, so the id
  travels with the content on `LoadedDiff.read_at`;
- `State.rows_at` records the commit the row list was loaded at, taken from the
  refresh's **bracketed** sample (`confirmed`), not its opening one; `markable`
  publishes an id only when the rows and the diff name the same commit;
- `ViewState.drawn_head` restricts marking to frames that actually drew the body
  (`view::body_is_drawn`).

Where the surfaces disagree, `M` refuses for one refresh. That is the conservative
direction: a mark that is too *old* only ever shows dots again, while one that is
too new hides changes. Two of the three HIGH findings in 0.0.3's review were
violations of this rule, and the first fix for one of them reopened the same hole
through another entrance — so when you touch head sampling, re-read this section.

**3. Notices: nothing overwrites a notice that has not been read, and nothing
deferred is forgotten.** `ViewState::observe` has one exception: a mark answer *is*
shown at once, because it answers the key the user just pressed — and displacing a
warning that way puts it back, by that warning's own mechanism (`notice_kind` says
which: a classification warning regenerates itself from snapshot state, a base
error returns to `pending_base_error`). A deferred base error is held in
`ViewState`, never re-read from the snapshot, because a later refresh publishes
none. `apply_action` re-runs `observe` when a body key clears a notice, which is
the only moment a deferred notice can surface in a settled repository. Four review
rounds were variants of this one interaction; the root cause was not modelling
*which* notice is on screen.

## Shape of the code

`src/git/` is a **frozen verbatim port** of vimeflow `crates/backend/src/git/` at
pin `91e45b1c` plus registered patches — never hand-edit it; change the pin or add
a numbered patch in `port/patches/` registered in `PORT-SURFACE.md`, and keep
`scripts/port-check.sh` passing. Everything else is ours:

```
src/engine/   session.rs  the loop: commands, refresh jobs, diff tasks, publication
              types.rs    Snapshot, LoadedDiff, Command, Mark, MarkState, QuickBase
              base.rs     git plumbing, bases.json + marks.json, is_object_id
              marks.rs    the unread set and the ancestry classification (0.0.3)
              branch.rs   branch-scope rows and diffs (0.0.2)
src/tui/      view.rs     pure render: a Snapshot + ViewState in, lines out
              state.rs    ViewState, Notice, drawn_head, observe
              input.rs    keys → Outcome, notice clearing
              picker.rs   the base picker, quick rows, the live reviewed row
              shell.rs    the terminal loop and initial state
src/actions/  the plugin actions herdr invokes (open, open-split, update)
tests/        readonly_guarantee.rs  G7, with a recording git wrapper
              e2e_real_herdr.rs     Tier B: an isolated real host, #[ignore]d
              version_sync.rs, env_policy.rs, actions_tier_a.rs
```

## How work gets done here

Each release followed the same loop, and it is worth keeping:

1. **Spec** a section of the roadmap with `/lifeline:planner` (codex reviews each
   section as it is written, then the whole spec). The spec is committed and gets
   an HTML-comment footer marking it codex-reviewed.
2. **Plan** from the spec in the same skill: bite-sized tasks with the real code in
   them, then codex rounds until the findings stop being structural.
3. **Execute** task by task with a coder agent in a herdr pane (`herdr pane split`
   + `herdr agent start`, never headless). The orchestrator audits each task, runs
   every gate outside the agent's sandbox, and makes **all** commits.
4. **Review** at the end: codex on the committed branch diff, round after round
   until it returns `No findings`; then a kimi milestone review that judges the
   whole branch with fresh eyes.
5. **Accept** by hand: `docs/acceptance-p1.md` has one row per criterion, and the
   release workflow refuses to publish unless its first line reads `Status: PASS`.
   Row 8 (review marks) is the model: run the real TUI and record what you saw.
6. **Ship**: PR → merge → annotated tag `vX.Y.Z` → the release workflow builds four
   targets and publishes → reinstall both hosts.

**Every fix gets falsified before it counts.** Put the old code back and confirm the
new test actually fails. In 0.0.3 this caught four cases where a fix or a test did
not do what it claimed — including one where `rustfmt` had reflowed the text a
patch was matching, so the "reproduction" silently tested the fixed code.

## Operational gotchas

Things that cost real time. None are obvious from the code.

- **`cargo test` needs a writable HOME outside any git repository**; the frozen test
  helpers create fixtures under `$HOME`. Preserve `CARGO_HOME` and `RUSTUP_HOME`
  when overriding it.
- **`scripts/port-check.sh` takes the vimeflow checkout as its argument**, and that
  checkout is read-only. `port-check-selftest.sh` takes it too.
- **codex needs the proxy.** Direct egress to its API is blocked here; clearing
  `http_proxy`/`https_proxy` makes it retry forever with `Reconnecting… waiting for
  network`. Feed long prompts via `codex exec -` on stdin (the argv limit is ~128 KB)
  and launch it detached through a small script so `pkill -f` patterns cannot match
  your own shell.
- **`herdr pane read` prints the pane's text, not a JSON envelope.** A test helper
  that parses every host reply as JSON will panic on it; `e2e_real_herdr.rs` has a
  separate `herdr_text` for this.
- **Tests that mutate a fixture the engine is polling must tolerate `index.lock`.**
  Both test `git` helpers retry on exactly that message for up to 10 s and fail with
  git's own text otherwise. CI caught this as a one-in-two flake.
- **Asynchronous tests must wait for the state they assert, not a proxy for it.**
  After a commit, wait for that commit's own diff, not for `head.is_some()`; when
  two conditions must both hold, put them in one predicate — otherwise the wait can
  return on the first and the assertion races the second.
- **`gh pr merge` fails with `'main' is already used by worktree`** when run from a
  linked worktree. The merge itself still happened; check the PR state before
  retrying anything.
- **Release notes come from the tag's annotation** since v0.0.3 (`%(contents:body)`,
  with the commit body as fallback, and `fetch-tags: true` in the publish job). Tag
  with `git tag -a -F <file>` and the notes appear on the release; a lightweight tag
  falls back to the commit body.
- **Agent dialogs are the user's to answer.** kimi asks "Trust this folder?" on a new
  directory and then per-command approval; codex may show an update prompt. Relay
  them, never answer them. kimi's `-y` starts it in its ask-when-needed mode, but
  then herdr's classifier reports `agent_status: unknown` and `herdr agent prompt`
  refuses — drive the pane with `pane send-text` + `enter` instead.

## What is next

**Phase 2: hunk actions** — stage / unstage / discard hunk, discard file, Y/N
confirmations, and the frozen-tree fixes K1-K5 (§2.4 of the P1 spec). The write
actions bind in `worktree` scope only; in `branch` scope they show
`switch to worktree scope (b) to stage or discard`. Reserved keys are already held
open: `s d D i I u U x v y Y @ c /`. The panel columns `M` draws into are the ones
Phase 2's staged rows use, and `M` is outside Phase 2's reserved set, so 0.0.3 does
not constrain it.

Known defects carried forward: **K1-K5** live in the frozen tree (K1-K4 in the
mutating paths, unreachable until Phase 2 touches them; K5 is a read-path defect in
`parse_git_status`, visible today). **K6** is ours: superseded engine diff requests
are not cancelled. **K7** was registered by 0.0.2: a staged row whose path was a file
and is now a directory gets both patches from one `git diff --cached` and parses the
second as content of the first — rare, worktree scope only. All are fixed in Phase 2
with K1-K6; see §2.4 of the P1 spec and §7.7 of the branch-scope spec.

Deliberate limitations recorded rather than fixed — do not "discover" these as bugs:

- `save_mark`/`save_pick` are synchronous inside the async refresh and their lock
  retries with `std::thread::sleep` for up to 2 s. Pre-existing since 0.0.2; moving
  both to `spawn_blocking` is its own change with its own contention test.
- `head_seen` advances on a refresh whose row load failed, while the rows, mark and
  dots stay at the last good state. Every consumer gates on a matching id.
- `A → B → A` inside one diff read leaves `read_at` at the older id. The consequence
  runs the safe way; closing it costs a reflog read per diff.
- One frame can list a new commit's rows with dots computed against the older head.
  `M` refuses in that window and the next refresh recomputes. This is the spec's own
  declared boundary: the status read asks git about whatever HEAD is live.

## Where the records are

- **Specs and plans**: `docs/superpowers/specs/` and `docs/superpowers/plans/`. The
  plans carry notes appended after each review round, which is where the reasoning
  behind late changes lives.
- **Acceptance**: `docs/acceptance-p1.md`, eight rows with evidence, and the
  `Status:` line the release workflow enforces.
- **Review records**: `.lifeline-planner/archive/` (gitignored, local to this
  machine) holds every codex review round, the kimi milestone reviews, the per-task
  execution reports and both `deferrals.md` files for 0.0.2 and 0.0.3 — 43 files.
  The deferrals are the record of what was deliberately *not* fixed and why.
