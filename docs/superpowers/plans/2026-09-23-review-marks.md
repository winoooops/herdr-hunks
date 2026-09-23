# herdr-hunks review marks and quick bases Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the branch scope a review mark: one key records the commit the reviewer has read up to, the files panel then dots every row a later commit touched, and the picker offers that mark plus three bases that are typed by hand today.

**Architecture:** The markable commit id travels with the content it belongs to — the diff task brackets its own call with `rev-parse` and reports the id it read at, `LoadedDiff` carries it, the snapshot's `head` is that id, and the view records it only for frames that drew the body. The mark itself is a small persisted record (`marks.json`) that changes no comparison: it drives a set of unread paths computed between two commits, and it is offered by the picker as an ordinary base. Nothing in the frozen tree changes.

**Tech Stack:** Rust 1.88, edition 2021; the 0.0.2 dependencies unchanged (`tokio` 1, `serde`/`serde_json` 1, `toml` 0.8, `libc` 0.2, `ratatui` 0.30 behind the `tui` feature, `crossterm` 0.29; dev `tempfile` 3). Runtime: `git` 2.31 or newer.

**Spec:** `docs/superpowers/specs/2026-09-23-review-marks-design.md` (section 8), on top of `docs/superpowers/specs/2026-09-22-branch-scope-design.md` (section 7, shipped as 0.0.2) and `docs/superpowers/specs/2026-09-18-hunks-roadmap-p1-viewer-design.md` (sections 1-6). Each task names the subsections it implements; read them before starting it.

## Global Constraints

- The vimeflow pin stays `91e45b1c` and `src/git/` is frozen. This section adds no patch: `port/patches/` keeps its three files, and `scripts/port-check.sh "$VIMEFLOW"` plus `sh scripts/port-check-selftest.sh "$VIMEFLOW"` must pass after every task. `$VIMEFLOW` is a read-only checkout of vimeflow at that pin (on the author's machine `~/projects/vimeflow`).
- Read-only guarantee G7 (spec 3.5, 7.6): the engine spawns only `git --version rev-parse status diff ls-files show cat-file symbolic-ref merge-base for-each-ref`. This section adds no subcommand. Every revision or object id is one argv element, never interpolated into a shell, and every revision argument is followed by `--` before any path.
- A commit id that came from persistence is checked by shape before it reaches a command: non-empty, hexadecimal only, and 40 or 64 characters. `check_text` (7.3) still guards typed text.
- `marks.json` is written only under an absolute state directory (`src/paths.rs` returns `None` otherwise), under `reuse::with_lock`, by temp file and rename — the rule `bases.json` and `split-panes.json` already follow. Nothing is ever written relative to the repository.
- Reserved keys stay unbound: `s d D i I u U x v y Y @ c /`. `M` is the only key added.
- Colours are the terminal's named ANSI colours only. Every string that came from git passes through `tui::sanitize` before it reaches a cell.
- Behaviour with no mark is exactly 0.0.2's: every existing test keeps passing without being weakened.
- Commits are conventional with a lowercase subject; inline comments are one short line and never reference a task or PR. The orchestrator makes every commit; the implementer leaves the tree uncommitted.
- `cargo test` needs a writable `HOME` outside any git repository (the frozen test helpers create fixtures under `$HOME`); the coder's instructions give the exact invocation.
- The version becomes `0.0.3` in Task 5 only; no other task touches `Cargo.toml`, `Cargo.lock` or `herdr-plugin.toml`.
- Run before every commit: `cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked && cargo check --locked --no-default-features && scripts/port-check.sh "$VIMEFLOW" && sh scripts/port-check-selftest.sh "$VIMEFLOW"`.

## File Structure

```
src/engine/types.rs        Mark, MarkState, LoadedDiff.read_at, Snapshot.{head, head_seen, mark,
                           unread, mark_seq, mark_error}, Command::MarkReviewed (Tasks 1-3)
src/engine/base.rs         read_head, mark records: load_marks, save_mark, is_object_id (Task 2)
src/engine/marks.rs        NEW: the unread set and the ancestry classification (Task 3)
src/engine/session.rs      head sampling and confirmation, the diff's bracket, MarkReviewed,
                           the mark and unread on every publication (Tasks 1-3)
src/engine/mod.rs          module list (Tasks 2, 3)
src/tui/state.rs           drawn_head, Notice { text, urgent }, observe for mark answers (Tasks 1, 4)
src/tui/keys.rs            `M` (Task 4)
src/tui/view.rs            the panel marker, the notice ranking, the empty-list wording (Task 4)
src/tui/input.rs           `M` sends MarkReviewed with drawn_head (Task 4)
src/tui/shell.rs           drawn_head assignment after each frame (Task 1)
src/tui/picker.rs          the reviewed row and the three quick rows (Task 5)
tests/readonly_guarantee.rs  a mark and a picker opening; marks.json in the state directory (Task 5)
tests/e2e_real_herdr.rs    presses `M` and waits for the marker to clear (Task 5)
README.md, .zh-CN, .ja     "Review marks" section (Task 5)
docs/acceptance-p1.md      row 8 (Task 5)
Cargo.toml, Cargo.lock, herdr-plugin.toml  0.0.3 (Task 5)
```

---

### Task 1: The markable id travels with the content

Implements spec 8.2, the three paragraphs "Which commit is marked", "The markable id travels with the content, not beside it" and "What the terminal drew is what can be marked", plus the boundary. Read them, and 7.2's publication rule, before starting.

**Files:**
- Modify: `src/engine/types.rs` (`LoadedDiff.read_at`, `Snapshot.{head, head_seen}`)
- Modify: `src/engine/base.rs` (`read_head`)
- Modify: `src/engine/session.rs` (`Job`, `run_job`, `load_rows`, `request_diff`, `Done::Diff`, both publication arms, `fingerprint`)
- Modify: `src/tui/state.rs` (`drawn_head`), `src/tui/shell.rs` (its assignment)

**Interfaces:**
- Consumes: 0.0.2's `Comparison`, `Base`, `BaseJob`, `Loaded`, `comparison_of`, `base::git`.
- Produces:

```rust
// src/engine/base.rs
/// `Ok(Some(id))` for a commit, `Ok(None)` when git ran and found none (unborn branch,
/// not a repository), `Err` when git could not be run.
pub(crate) async fn read_head(toplevel: &str) -> Result<Option<String>, String>;

// src/engine/session.rs
// EngineHandle gains, beside `refreshes` and `diffs_discarded`:
pub head_samples: Arc<AtomicUsize>,   // opening samples taken by diff tasks

// src/engine/types.rs
// LoadedDiff gains: pub read_at: Option<String>
// build(key, comparison, read_at, response) / build_with_cap(key, comparison, read_at, response, cap)
// Snapshot gains:
pub head: Option<String>,        // the id this snapshot's content corresponds to
pub head_seen: Option<String>,   // the id the last refresh observed, ungated

// src/tui/state.rs
// ViewState gains: pub drawn_head: Option<String>
```

- [ ] **Step 1: Write the failing engine tests**

Append to the `tests` module of `src/engine/session.rs`. `branch_fixture`, `start_with`, `wait_for`, `ready` and `git` already exist there from 0.0.2.

```rust
    #[test]
    fn a_snapshot_carries_the_head_its_content_was_read_at() {
        let dir = branch_fixture();
        let (_rt, h) = start_with(dir.path(), Scope::Branch, None, None);
        let s = wait_for(&h, "first branch diff", |s| ready(s).is_some());
        let head = git_out(dir.path(), &["rev-parse", "HEAD"]).trim().to_string();
        assert_eq!(s.head_seen.as_deref(), Some(head.as_str()));
        assert_eq!(s.head.as_deref(), Some(head.as_str()));
        assert_eq!(ready(&s).unwrap().read_at.as_deref(), Some(head.as_str()));

        // A snapshot whose diff is not loaded carries no markable id.
        h.commands.send(Command::SelectNext).unwrap();
        let loading = wait_for(&h, "loading", |s| matches!(s.diff, DiffState::Loading));
        assert_eq!(loading.head, None);
        assert_eq!(loading.head_seen.as_deref(), Some(head.as_str()));
        wait_for(&h, "loaded again", |s| ready(s).is_some());
    }

    #[test]
    fn an_empty_list_is_markable_and_an_unborn_branch_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        git(p, &["config", "user.email", "t@example.com"]);
        git(p, &["config", "user.name", "t"]);
        std::fs::write(p.join("a.txt"), "a\n").unwrap();
        git(p, &["add", "-A"]);
        git(p, &["commit", "-q", "-m", "init"]);
        let (_rt, h) = start_with(p, Scope::Worktree, None, None);
        let head = git_out(p, &["rev-parse", "HEAD"]).trim().to_string();
        let s = wait_for(&h, "clean tree", |s| {
            matches!(s.repo, RepoState::Repo { .. }) && s.files.is_empty()
        });
        assert_eq!(s.head.as_deref(), Some(head.as_str()), "an empty list is markable");

        // An unborn branch clears both ids.
        git(p, &["checkout", "-q", "--orphan", "fresh"]);
        git(p, &["rm", "-q", "--cached", "a.txt"]);
        std::fs::remove_file(p.join("a.txt")).unwrap();
        h.commands.send(Command::Refresh).unwrap();
        let s = wait_for(&h, "unborn", |s| s.head_seen.is_none());
        assert_eq!(s.head, None);
    }

    #[test]
    fn an_unborn_branch_is_unmarkable_with_a_diff_still_on_screen() {
        let dir = branch_fixture();
        let (_rt, h) = start_with(dir.path(), Scope::Branch, None, None);
        let s = wait_for(&h, "first branch diff", |s| ready(s).is_some());
        assert!(s.head.is_some());

        // Branch scope's row load fails without a commit, so the rows and the diff stay on
        // screen -- and the id that diff was read at belongs to the branch that was left.
        git(dir.path(), &["checkout", "-q", "--orphan", "fresh"]);
        h.commands.send(Command::Refresh).unwrap();
        let s = wait_for(&h, "unborn", |s| s.head_seen.is_none());
        assert!(ready(&s).is_some(), "the previous diff is still drawn");
        assert_eq!(s.head, None, "and it no longer vouches for an id");
    }

    #[test]
    fn a_diff_that_spans_a_head_move_reports_no_id() {
        let dir = branch_fixture();
        // No permits at all: the very first diff takes its opening sample and then blocks,
        // which is the only moment in the test where a diff is waiting, so no later refresh
        // can race the setup.
        let gate = Arc::new(Semaphore::new(0));
        let (_rt, h) = start_with(dir.path(), Scope::Branch, None, Some(gate.clone()));
        let deadline = Instant::now() + Duration::from_secs(10);
        while h.head_samples.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < deadline, "no diff task reached its first sample");
            std::thread::sleep(Duration::from_millis(20));
        }
        std::fs::write(dir.path().join("late.txt"), "late\n").unwrap();
        git(dir.path(), &["add", "late.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "late"]);
        gate.add_permits(4);
        let seen = wait_for(&h, "a diff with no id", |s| {
            matches!(&s.diff, DiffState::Ready(d) if d.read_at.is_none())
        });
        assert_eq!(seen.head, None, "a snapshot whose diff spanned a move is unmarkable");
        // Once the tree settles, a later diff brackets cleanly and the id comes back.
        gate.add_permits(8);
        let settled = wait_for(&h, "a clean bracket", |s| {
            matches!(&s.diff, DiffState::Ready(d) if d.read_at.is_some())
        });
        assert!(settled.head.is_some());
    }
```

Add one helper next to `git_out` if it is not already present (0.0.2 added it):

```rust
    fn git_out(dir: &std::path::Path, args: &[&str]) -> String {
        let out = Proc::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
```


Worktree scope has no diff to retain, so clearing `head` there is trivial; the second test is
the case 8.6 is really about. In branch scope the row load fails without a commit, every row and
the `Ready` diff survive that refresh, and only the explicit clear keeps `M` from marking the
commit of the branch that was left.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --locked --lib engine::session` (with the writable HOME)
Expected: compile errors — `head`, `head_seen` and `read_at` do not exist.

- [ ] **Step 3: Add `read_head` to `src/engine/base.rs`**

```rust
/// `Ok(Some(id))` for a commit, `Ok(None)` when git ran and found none, `Err` when it could not run.
pub(crate) async fn read_head(toplevel: &str) -> Result<Option<String>, String> {
    let output = git(toplevel, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"]).await?;
    if !output.status.success() {
        return Ok(None);
    }
    let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!id.is_empty()).then_some(id))
}
```

- [ ] **Step 4: Carry the id on the diff and the snapshot**

`src/engine/types.rs`: `LoadedDiff` gains `pub read_at: Option<String>,` after `comparison`; both constructors take it:

```rust
    pub fn build(
        key: FileKey,
        comparison: Comparison,
        read_at: Option<String>,
        response: GetGitDiffResponse,
    ) -> Self {
        Self::build_with_cap(key, comparison, read_at, response, MAX_DIFF_LINES)
    }

    pub fn build_with_cap(
        key: FileKey,
        comparison: Comparison,
        read_at: Option<String>,
        response: GetGitDiffResponse,
        cap: usize,
    ) -> Self {
```

and sets `read_at,` in the returned struct. `Snapshot` gains, after `repo`:

```rust
    /// The commit this snapshot's content corresponds to: the id its diff reported, the
    /// refresh's confirmed id when the row list is empty, `None` while nothing is loaded.
    pub head: Option<String>,
    /// The id the last refresh observed, published as observed and never gated.
    pub head_seen: Option<String>,
```

both `None` in `Snapshot::empty`, and both appended to `fingerprint`'s format string and argument list (after `s.refreshing`), so a head change is never deduplicated away.

Every existing `LoadedDiff::build(...)` / `build_with_cap(...)` call site gains the new argument: `src/engine/session.rs` (one), `src/tui/state.rs` tests, `src/tui/rows.rs` tests, `src/engine/types.rs` tests. In the test helpers pass `None`; in `session.rs` pass what the result carried (Step 6).

- [ ] **Step 5: Sample the head in the refresh job and use it**

`src/engine/session.rs`. `Job` gains nothing; `run_job` brackets the work:

```rust
async fn run_job(job: Job) -> Done {
    let toplevel_for_head = job.cwd.clone();
    let sampled = base::read_head(&toplevel_for_head).await;
    let response = git::git_status_inner(job.cwd.clone()).await;
    let head = if job.with_head {
        let (branch, worktree) = tokio::join!(
            git::git_branch_inner(job.cwd.clone()),
            git::git_worktree_name_inner(job.cwd.clone())
        );
        Some((branch.ok(), worktree.ok().flatten()))
    } else {
        None
    };
    // `seen` is taken by a local in `load_rows` (the row dedup of 7.2); this one is the head.
    let sampled_head = match &sampled {
        Ok(id) => id.clone(),
        Err(_) => None,
    };
    let loaded = match &response {
        Ok(status) if !status.repo_root.is_empty() => {
            Some(load_rows(&job, status, head.as_ref(), sampled_head.as_deref()).await)
        }
        _ => None,
    };
    // The second sample confirms nothing moved while the row commands ran; only a
    // confirmed id may make an empty list markable.
    let confirmed = match (&sampled, base::read_head(&toplevel_for_head).await) {
        (Ok(first), Ok(second)) if *first == second => first.clone(),
        _ => None,
    };
    Done::Status {
        response,
        head,
        head_seen: match &sampled {
            Ok(id) => id.clone(),
            Err(_) => None,
        },
        head_sampled: sampled.is_ok(),
        confirmed,
        loaded,
        change: job.change,
    }
}
```

`Done::Status` gains the three fields (`head_seen: Option<String>`, `head_sampled: bool`, `confirmed: Option<String>`). `load_rows` takes `head_id: Option<&str>` — not `seen`, which `load_rows` already uses for the
row dedup of 7.2 — and uses it instead of the symbol `HEAD` in the merge-base call:

```rust
    if job.scope == Scope::Branch {
        if let Some(b) = base.as_mut() {
            if matches!(job.base, BaseJob::Keep(_)) {
                b.commit = base::verify(toplevel, &b.requested).await?;
            }
            let head = head_id.ok_or_else(|| "no commit yet".to_string())?;
            b.merge_base = Some(base::merge_base_of(toplevel, head, &b.commit).await?);
        }
    }
```

and `base.rs`'s `merge_base` becomes `merge_base_of(toplevel, head, commit)`, taking the id explicitly rather than naming `HEAD`:

```rust
/// `git merge-base <head> <commit>`; fails for unrelated histories.
pub(crate) async fn merge_base_of(
    toplevel: &str,
    head: &str,
    commit: &str,
) -> Result<String, String> {
    let output = git(toplevel, &["merge-base", head, commit]).await?;
    if !output.status.success() {
        let short = &commit[..commit.len().min(7)];
        let err = stderr_of(&output);
        return Err(if err.is_empty() {
            format!("no merge-base with {short}")
        } else {
            format!("no merge-base with {short}: {err}")
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
```

Update `base.rs`'s own test that called `merge_base(&top, &id)` to `merge_base_of(&top, "HEAD", &id)`.

- [ ] **Step 6: Bracket the diff task and publish the id**

In `request_diff`, wrap the diff call:

```rust
        tokio::spawn(async move {
            // The bracket opens before the delay and the gate, so a HEAD move during either
            // also costs the acknowledgement; only a still HEAD across the whole read counts.
            let before = base::read_head(&toplevel).await;
            // Test hook, like `refreshes` and `diffs_discarded`: a test that wants to move
            // HEAD *between* the samples waits for this before doing so.
            head_samples.fetch_add(1, Ordering::SeqCst);
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            if let Some(gate) = gate {
                gate.acquire().await.expect("diff gate closed").forget();
            }
            let result = match &comparison {
                Comparison::Branch { merge_base } if !key.untracked => {
                    branch::diff(&toplevel, merge_base, &key.path, old.as_deref()).await
                }
                Comparison::Branch { .. } => branch::untracked_diff(cwd, key.path.clone()).await,
                Comparison::Worktree => {
                    git::get_git_diff_inner(cwd, key.path.clone(), key.staged, Some(key.untracked))
                        .await
                }
            };
            // The id is reported only when HEAD stood still across the read.
            let read_at = match (before, base::read_head(&toplevel).await) {
                (Ok(Some(a)), Ok(Some(b))) if a == b => Some(a),
                _ => None,
            };
            let _ = results.send(Done::Diff {
                generation,
                key,
                comparison,
                read_at,
                result,
            });
        });
```

`Done::Diff` gains `read_at: Option<String>`. In its arm, the id reaches both the diff and the snapshot:

```rust
                    Done::Diff { generation, key, comparison, read_at, result } => {
                        if generation != state.diff_generation || comparison != comparison_of(&state.snapshot) {
                            // The counter an existing regression waits on; keep it.
                            diffs_discarded.fetch_add(1, Ordering::SeqCst);
                            continue;
                        }
                        state.diff_in_flight = None;
                        if Some(&key) == next.selected.as_ref() {
                            match result {
                                Ok(response) => {
                                    let unchanged = matches!(&next.diff, DiffState::Ready(d) if d.key == key && d.comparison == comparison && d.raw_diff == response.raw_diff && d.read_at == read_at);
                                    if !unchanged {
                                        next.diff = DiffState::Ready(Arc::new(LoadedDiff::build(key, comparison, read_at.clone(), response)));
                                    }
                                    next.head = read_at;
                                }
                                Err(e) => {
                                    next.diff = DiffState::Failed(e);
                                    next.head = None;
                                }
                            }
```

A snapshot's `head` is a function of that snapshot, never a value left over from an
earlier one, so it is assigned at every site that publishes a diff state. Add one helper
next to `comparison_of`:

```rust
/// The commit this snapshot's content corresponds to: what its diff reported, the refresh's
/// confirmed id when there is no row to load, and nothing while no content is loaded.
fn head_of(snapshot: &Snapshot, confirmed: Option<String>) -> Option<String> {
    match &snapshot.diff {
        DiffState::Ready(d) => d.read_at.clone(),
        DiffState::Idle if snapshot.files.is_empty() => confirmed,
        _ => None,
    }
}
```

In the `Done::Status` arm, `next.diff` is decided *after* `state.requested_scope = next.scope;`
— the comparison check and the `if succeeded && !same { next.diff = ... }` block follow it — so
`head_seen` is set where the other snapshot fields are, and the head assignment goes after that
block, immediately before the `pick_seq` bookkeeping:

```rust
                        // with the other fields, before `state.requested_scope = next.scope;`
                        if head_sampled {
                            next.head_seen = head_seen;
                        }
```

```rust
                        // after `if succeeded && !same { next.diff = ... }`, so it reads the
                        // diff state this publication actually carries
                        let previous_head = next.head.clone();
                        next.head = markable(&next, previous_head, head_sampled, confirmed.filter(|_| succeeded));
```

with the decision beside `head_of`, so the three cases are one readable rule rather than a
condition inside the publication:

```rust
/// 8.6: the id this publication may be marked at.
fn markable(
    next: &Snapshot,
    previous_head: Option<String>,
    head_sampled: bool,
    confirmed: Option<String>,
) -> Option<String> {
    if !head_sampled {
        // The sample could not run at all. 8.6 keeps both previous values rather than making
        // a settled worktree unmarkable because one `rev-parse` could not be spawned.
        return head_of(next, previous_head);
    }
    if next.head_seen.is_none() {
        // The sample ran and found no commit: whatever diff is still on screen belongs to the
        // branch that was left, and `M` must refuse.
        return None;
    }
    head_of(next, confirmed)
}
```

A refresh that keeps a `Ready` diff therefore keeps that diff's id -- the background
refreshes of 3.3 do not flap it to `None` -- while one that moved the diff to `Loading`
publishes `None`. In the selection arm (`Command::Select`/`SelectNext`/`SelectPrev`), which
publishes `DiffState::Loading` before requesting the diff, add `next.head = None;` next to
`next.diff = DiffState::Loading;`: the body is about to show another file's hunks, and
nothing on screen vouches for the id until they load. That is what Step 1's second
assertion checks.

- [ ] **Step 7: Record the drawn id in the view**

`src/tui/state.rs`: `ViewState` gains `pub drawn_head: Option<String>,` initialised `None` in `new`.

The decision belongs next to `render`, which owns the thresholds that replace the body, so
the shell cannot drift from it. In `src/tui/view.rs`:

```rust
/// Whether `render` at this size draws the review body: not the "terminal too small" notice
/// of 5.1, and not under a modal that covers it. The shell asks this before believing a
/// frame's id; the thresholds live here, with the check `render` itself makes.
pub fn body_is_drawn(state: &ViewState, snapshot: &Snapshot, columns: u16, height: u16) -> bool {
    columns >= 40
        && height >= 10
        && !state.help_open
        && state.picker.is_none()
        && (matches!(&snapshot.diff, DiffState::Ready(_)) || snapshot.files.is_empty())
}
```

`render`'s own first lines become `if !(columns >= 40 && height >= 10) { .. }` unchanged in
behaviour; leave them as they are and keep the two numbers in one place by having
`body_is_drawn` be the only other reader of them.

`src/tui/shell.rs`, inside the `terminal.draw(|frame| { .. })` closure, after
`rendered = view::render(..)`, record the decision for the size that was actually drawn:

```rust
                // Only a frame that showed the body vouches for an id.
                drew_body = view::body_is_drawn(&state, &snapshot, width, area.height);
```

with `let mut drew_body = false;` declared before the closure, and immediately after the
`terminal.draw(...)?;` call:

```rust
            state.record_drawn(&snapshot, drew_body);
```

with the rule itself on `ViewState`, where it can be tested without a terminal:

```rust
    /// After a frame: a body frame vouches for its snapshot's id, a modal or unloaded one
    /// leaves the last one alone, and a snapshot with no id clears it.
    pub fn record_drawn(&mut self, snapshot: &Snapshot, drew_body: bool) {
        self.drawn_head = if drew_body {
            snapshot.head.clone()
        } else {
            self.drawn_head.take().filter(|_| snapshot.head.is_some())
        };
    }
```

`DiffState` is already imported in `shell.rs` through `crate::engine`; add it to the import
list if it is not.

- [ ] **Step 8: Write the view test and run everything**

`src/tui/view.rs` tests — the production predicate, not a copy of the shell's assignment:

```rust
    #[test]
    fn only_a_drawn_body_vouches_for_an_id() {
        let mut snap = snapshot("a.rs", "r1", &[(1, "+")]);
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        st.resize(120, 20);
        st.reconcile(&snap);
        assert!(body_is_drawn(&st, &snap, 120, 24));
        assert!(!body_is_drawn(&st, &snap, 39, 24), "too narrow: the size notice is drawn");
        assert!(!body_is_drawn(&st, &snap, 120, 9), "too short: the size notice is drawn");
        st.help_open = true;
        assert!(!body_is_drawn(&st, &snap, 120, 24), "the key sheet covers the diff");
        st.help_open = false;
        st.picker = Some(crate::tui::picker::Picker::open(0));
        assert!(!body_is_drawn(&st, &snap, 120, 24), "the picker covers the diff");
        st.picker = None;
        // `snapshot(..)` leaves `files` empty, which is itself a drawn body; give it a row
        // before asserting that an unloaded diff is not one.
        snap.files = vec![crate::git::ChangedFile {
            path: "a.rs".into(),
            status: crate::git::ChangedFileStatus::Modified,
            staged: false,
            insertions: Some(1),
            deletions: Some(0),
        }];
        snap.diff = crate::engine::DiffState::Loading;
        assert!(!body_is_drawn(&st, &snap, 120, 24), "a row whose diff is not loaded");
        snap.files.clear();
        snap.diff = crate::engine::DiffState::Idle;
        assert!(body_is_drawn(&st, &snap, 120, 24), "an empty list is a drawn body");
    }
```

And `src/engine/session.rs` tests the publication rule itself, which no repository fixture can
reach: `head_sampled` is false only when `rev-parse` could not be spawned at all.

```rust
    #[test]
    fn a_sample_that_could_not_run_keeps_the_previous_id() {
        let mut snap = Snapshot::empty();
        let previous = "a".repeat(40);
        let confirmed = "b".repeat(40);
        // A clean worktree: an empty list with no diff to read, so the id can only come from
        // the sample.
        assert_eq!(
            markable(&snap, Some(previous.clone()), true, Some(confirmed.clone())),
            None,
            "sampled, and there is no commit"
        );
        snap.head_seen = Some(confirmed.clone());
        assert_eq!(
            markable(&snap, Some(previous.clone()), true, Some(confirmed.clone())),
            Some(confirmed.clone()),
            "a confirmed sample is what the empty list is marked at"
        );
        assert_eq!(
            markable(&snap, Some(previous.clone()), true, None),
            None,
            "sampled, but the two samples disagreed"
        );
        assert_eq!(
            markable(&snap, Some(previous.clone()), false, None),
            Some(previous.clone()),
            "the sample could not run: 8.6 keeps the previous id"
        );
        // A loaded diff vouches for itself in every case; a loading one for none.
        snap.diff = DiffState::Loading;
        assert_eq!(markable(&snap, Some(previous.clone()), false, None), None);
    }
```

Run, with the writable HOME:

```
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo check --locked --no-default-features
scripts/port-check.sh "$VIMEFLOW"
sh scripts/port-check-selftest.sh "$VIMEFLOW"
```

Expected: all pass, the three new engine tests and the state test included. Run `cargo test --locked --lib engine::session -- --test-threads=1` three times and report any flake rather than widening a wait.

Add its own test in `src/tui/state.rs`:

```rust
    #[test]
    fn a_frame_records_the_id_only_when_it_drew_the_body() {
        let mut snap = snapshot("a.rs", "r1", &[(1, "+")]);
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        snap.head = Some("a".repeat(40));
        st.record_drawn(&snap, true);
        assert_eq!(st.drawn_head.as_deref(), Some("a".repeat(40).as_str()));

        // A modal frame leaves the last id alone, even as the snapshot's own moves on.
        snap.head = Some("b".repeat(40));
        st.record_drawn(&snap, false);
        assert_eq!(st.drawn_head.as_deref(), Some("a".repeat(40).as_str()));

        // A snapshot with no id clears it, drawn or not.
        snap.head = None;
        st.record_drawn(&snap, false);
        assert_eq!(st.drawn_head, None);
        snap.head = Some("c".repeat(40));
        st.record_drawn(&snap, true);
        snap.head = None;
        st.record_drawn(&snap, true);
        assert_eq!(st.drawn_head, None);
    }
```

Applied after the pre-PR review's third round (codex, 2026-09-23): the markable id needs *both*
loaded surfaces to name the same commit, not only the diff. `State.rows_at` records the commit
the published row list was loaded at, `markable` ends with `id.filter(|id| rows_at == Some(id))`,
and the `Done::Diff` arm applies the same filter to `read_at`. Without it a diff that brackets
cleanly after a commit the status refresh has not published yet makes that commit markable while
its new rows are still absent: `M` would then acknowledge files that were never on screen, and
they would arrive carrying no dot. The disagreement lasts one refresh cycle, during which `M`
refuses -- the conservative direction, because a mark that is too old only shows dots again.

Also applied there: `request_status` must not let a queued `Change::Mark` swallow the
`resolve_pending` flag an explicit `r` set. A mark now runs `BaseJob::Resolve` when that flag is
pending and `BaseJob::Keep` otherwise, so `Mark → Refresh → Mark` still reloads the preferences.

The fourth round found the other half of both: `rows_at` was taken from the refresh's *opening*
sample, so rows loaded across a HEAD move could name a commit whose files they never listed --
restoring that commit afterwards then let a clean diff pass the check and `M` acknowledge unseen
files. It now comes from `confirmed`, the bracketed sample, so a refresh that spanned a move
vouches for nothing. And a resolution is only *consumed* when it is delivered: `State`'s
`in_flight_resolve` puts `resolve_pending` back whenever the refresh carrying it published no
rows, which covers a mark whose id will not verify and, with it, the older case of an explicit
`r` lost to a failed status.

- [ ] **Step 9: Commit (orchestrator)**

```bash
git add -A
git commit -m "feat(engine): carry the head a diff was read at"
```

---

### Task 2: The mark — `marks.json`, `MarkReviewed`, and its answer channel

Implements spec 8.2, the paragraphs "What the engine does", "Where the mark lives while the viewer runs" and "How a failed mark is shown", and the persistence rows of 8.6. Read them and 7.3's `bases.json` rules before starting.

**Files:**
- Modify: `src/engine/base.rs` (`is_object_id`, `load_marks`, `save_mark`)
- Modify: `src/engine/types.rs` (`Mark`, `MarkState`, `Snapshot.{mark, mark_seq, mark_error}`, `Command::MarkReviewed`)
- Modify: `src/engine/session.rs` (`ResolveInputs` session mark, the `MarkReviewed` arm, `Loaded.mark`)

**Interfaces:**
- Consumes: Task 1's `read_head`; 0.0.2's `reuse::with_lock`, `base::{load_picks, note_problem, verify}`, `Change`, `BaseJob`, `Loaded`.
- Produces:

```rust
// src/engine/base.rs
pub fn is_object_id(text: &str) -> bool;                       // hex only, 40 or 64 chars
pub fn load_marks(state_dir: &Path) -> (BTreeMap<String, MarkRecord>, Option<String>);
pub fn save_mark(state_dir: &Path, toplevel: &str, record: &MarkRecord) -> std::io::Result<()>;
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MarkRecord { pub commit: String, pub at: u64 }

// src/engine/types.rs
pub struct Mark { pub commit: String, pub at: u64, pub state: MarkState, pub classified_at: Option<String> }
pub enum MarkState { Current, Rewritten, Unreadable(String) }   // Clone, Debug, PartialEq, Eq
// Snapshot gains: mark: Option<Mark>, mark_seq: u64, mark_error: Option<String>
// Command gains: MarkReviewed(String)
```

- [ ] **Step 1: Write the failing persistence tests**

Append to the `tests` module of `src/engine/base.rs`:

```rust
    #[test]
    fn object_ids_are_recognised_by_shape_alone() {
        assert!(is_object_id(&"a".repeat(40)));
        assert!(is_object_id(&"0".repeat(64)));
        assert!(!is_object_id(&"a".repeat(39)));
        assert!(!is_object_id(&"a".repeat(41)));
        assert!(!is_object_id(""));
        assert!(!is_object_id("--output=tracked.txt"));
        assert!(!is_object_id("refs/heads/main"));
        assert!(!is_object_id(&format!("{}z", "a".repeat(39))));
    }

    #[test]
    fn marks_round_trip_and_a_bad_record_is_dropped() {
        let state = tempfile::tempdir().unwrap();
        assert_eq!(load_marks(state.path()).0.len(), 0);
        let record = MarkRecord {
            commit: "a".repeat(40),
            at: 1_700_000_000,
        };
        save_mark(state.path(), "/r/one", &record).unwrap();
        let (marks, problem) = load_marks(state.path());
        assert_eq!(marks.get("/r/one"), Some(&record));
        assert!(problem.is_none());

        // A record whose commit is not an object id is dropped with a problem line.
        std::fs::write(
            state.path().join("marks.json"),
            r#"{"/r/one":{"commit":"--output=x","at":1}}"#,
        )
        .unwrap();
        let (marks, problem) = load_marks(state.path());
        assert!(marks.is_empty());
        assert!(problem.unwrap().starts_with("marks.json: "));

        // A bad record beside a good one still reports: the file needs repairing even
        // though this worktree's own record survives.
        std::fs::write(
            state.path().join("marks.json"),
            format!(
                r#"{{"/r/one":{{"commit":"{}","at":1}},"/r/two":{{"commit":"nope","at":2}}}}"#,
                "a".repeat(40)
            ),
        )
        .unwrap();
        let (marks, problem) = load_marks(state.path());
        assert_eq!(marks.len(), 1, "the valid record survives");
        assert!(marks.contains_key("/r/one"));
        assert!(problem.is_some(), "the dropped record is still reported");

        // Malformed JSON degrades the same way, and the next save rewrites it.
        std::fs::write(state.path().join("marks.json"), "{ not json").unwrap();
        assert!(load_marks(state.path()).1.is_some());
        save_mark(state.path(), "/r/two", &record).unwrap();
        assert_eq!(load_marks(state.path()).0.len(), 1);
        let mut names: Vec<_> = std::fs::read_dir(state.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["marks.json", "split-panes.lock"], "no temp file is left behind");

        // A relative state directory is refused, as for picks.
        assert!(save_mark(std::path::Path::new("relative"), "/r/one", &record).is_err());
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --locked --lib engine::base`
Expected: compile errors — `is_object_id`, `load_marks`, `save_mark`, `MarkRecord` are undefined.

- [ ] **Step 3: Implement the persistence**

In `src/engine/base.rs`, next to the picks:

```rust
const MARKS_FILE: &str = "marks.json";

/// A commit id as `rev-parse` prints one: hexadecimal, 40 or 64 characters. Everything read
/// back from a file is checked by shape before it can become an argument.
pub fn is_object_id(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|b| b.is_ascii_hexdigit())
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MarkRecord {
    pub commit: String,
    /// Seconds since the Unix epoch, recorded when the mark was written.
    pub at: u64,
}

/// The remembered marks, keyed by canonical toplevel; a record whose commit is not an
/// object id is dropped, as is an unreadable or malformed file, with the reason.
pub fn load_marks(state_dir: &Path) -> (BTreeMap<String, MarkRecord>, Option<String>) {
    let text = match std::fs::read_to_string(state_dir.join(MARKS_FILE)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (BTreeMap::new(), None),
        Err(e) => return (BTreeMap::new(), Some(format!("{MARKS_FILE}: {e}"))),
        Ok(text) => text,
    };
    let marks: BTreeMap<String, MarkRecord> = match serde_json::from_str(&text) {
        Ok(marks) => marks,
        Err(e) => return (BTreeMap::new(), Some(format!("{MARKS_FILE}: {e}"))),
    };
    let total = marks.len();
    let kept: BTreeMap<String, MarkRecord> = marks
        .into_iter()
        .filter(|(_, m)| is_object_id(&m.commit))
        .collect();
    // A record we refuse to hand to git is worth a line even when another worktree's
    // record survives beside it.
    let dropped = total - kept.len();
    let problem = (dropped > 0).then(|| format!("{MARKS_FILE}: {dropped} unusable record(s)"));
    (kept, problem)
}

/// Read-modify-write under the same lock as the picks, then an atomic replace.
pub fn save_mark(state_dir: &Path, toplevel: &str, record: &MarkRecord) -> std::io::Result<()> {
    if !state_dir.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "state directory must be absolute",
        ));
    }
    reuse::with_lock(state_dir, || {
        let (mut marks, _) = load_marks(state_dir);
        marks.insert(toplevel.to_string(), record.clone());
        let tmp = state_dir.join(format!("{MARKS_FILE}.{}.tmp", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec_pretty(&marks).unwrap_or_default())?;
        std::fs::rename(tmp, state_dir.join(MARKS_FILE))
    })?
}
```

The `problem` rule deserves its one-line comment in the code: every record the filter refuses is worth a line in `config-problems.log`, because `marks.json` holds one record per worktree and a corrupt entry beside a healthy one is still a file someone has to repair. A file that simply has no entry for *this* worktree is not a problem — the count is of records dropped, not of records missing.

- [ ] **Step 4: Run the persistence tests**

Run: `cargo test --locked --lib engine::base`
Expected: PASS.

- [ ] **Step 5: Write the failing engine tests for `MarkReviewed`**

Append to `src/engine/session.rs`'s tests:

```rust
    #[test]
    fn marking_writes_the_record_and_answers_on_its_own_channel() {
        let dir = branch_fixture();
        let state = tempfile::tempdir().unwrap();
        let (_rt, h) = start_with(dir.path(), Scope::Branch, Some(state.path().to_path_buf()), None);
        let s = wait_for(&h, "first", |s| ready(s).is_some());
        let head = s.head.clone().expect("a markable id");

        h.commands.send(Command::MarkReviewed(head.clone())).unwrap();
        let s = wait_for(&h, "answered", |s| s.mark_seq == 1);
        assert!(s.mark_error.is_none());
        assert_eq!(s.pick_seq, 0, "a mark is not a pick");
        let mark = s.mark.clone().expect("the mark is published");
        assert_eq!(mark.commit, head);
        assert!(mark.at > 1_600_000_000, "a real timestamp");
        assert_eq!(mark.state, crate::engine::MarkState::Current);
        assert_eq!(s.base.as_ref().map(|b| b.requested.as_str()), Some("refs/heads/main"),
            "the base is untouched");

        let toplevel = dir.path().canonicalize().unwrap().to_string_lossy().into_owned();
        let (marks, _) = crate::engine::base::load_marks(state.path());
        assert_eq!(marks.get(&toplevel).map(|m| m.commit.clone()), Some(head.clone()));
        assert!(!state.path().join("bases.json").exists(), "no base was written");

        // A second mark replaces the record.
        std::fs::write(dir.path().join("more.txt"), "more\n").unwrap();
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "more"]);
        let s = wait_for(&h, "the new head is markable", |s| {
            s.head.as_deref().is_some_and(|h| h != head)
        });
        let head2 = s.head.clone().unwrap();
        h.commands.send(Command::MarkReviewed(head2.clone())).unwrap();
        let s = wait_for(&h, "second answer", |s| s.mark_seq == 2);
        assert_eq!(s.mark.as_ref().map(|m| m.commit.clone()), Some(head2.clone()));
        let (marks, _) = crate::engine::base::load_marks(state.path());
        assert_eq!(marks.get(&toplevel).map(|m| m.commit.clone()), Some(head2));
    }

    #[test]
    fn a_mark_that_cannot_be_validated_or_written_says_so() {
        let dir = branch_fixture();
        let (_rt, h) = start_with(dir.path(), Scope::Branch, None, None);
        wait_for(&h, "first", |s| ready(s).is_some());

        // An id that is not a commit: answered, nothing written, previous mark untouched.
        h.commands.send(Command::MarkReviewed("b".repeat(40))).unwrap();
        let s = wait_for(&h, "rejected", |s| s.mark_seq == 1);
        assert_eq!(s.mark_error.as_deref(), Some("not a commit: bbbbbbb"));
        assert!(s.mark.is_none());

        // With no state directory the mark holds for the session and says so.
        let head = s.head.clone().expect("a markable id");
        h.commands.send(Command::MarkReviewed(head.clone())).unwrap();
        let s = wait_for(&h, "session mark", |s| s.mark_seq == 2);
        assert_eq!(s.mark.as_ref().map(|m| m.commit.clone()), Some(head.clone()));
        assert!(s.mark_error.as_deref().unwrap().starts_with("mark not remembered: "),
            "{:?}", s.mark_error);
        assert_eq!(s.pick_seq, 0, "a mark never answers on the pick channel");
        // It survives an ordinary refresh, like the session pick of 7.3.
        h.commands.send(Command::Refresh).unwrap();
        let s = wait_for(&h, "after refresh", |s| s.revision > 3 && !s.refreshing);
        assert_eq!(s.mark.as_ref().map(|m| m.commit.clone()), Some(head));
    }
```

- [ ] **Step 6: Run them to verify they fail**

Run: `cargo test --locked --lib engine::session`
Expected: compile errors — `Command::MarkReviewed`, `Snapshot.mark`, `mark_seq`, `mark_error` do not exist.

- [ ] **Step 7: Add the types**

`src/engine/types.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkState {
    /// An ancestor of `head_seen`: the unread set is the one the diff computed.
    Current,
    /// Still a commit, no longer on this branch: every row is flagged.
    Rewritten,
    /// The unread command failed; the string is git's reason. Every row is flagged.
    Unreadable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    /// A full object id: hexadecimal only, of the length this repository's hash gives.
    pub commit: String,
    /// Seconds since the Unix epoch, recorded when the mark was written.
    pub at: u64,
    pub state: MarkState,
    /// The `head_seen` `state` was successfully classified against.
    pub classified_at: Option<String>,
}
```

`Snapshot` gains `pub mark: Option<Mark>,` next to `base`, and `pub mark_seq: u64,` with `pub mark_error: Option<String>,` next to `pick_seq`/`pick_error`; all three are `None`/`0` in `empty` and all three join `fingerprint`. `Command` gains:

```rust
    /// Record this commit as reviewed for this worktree. It changes no comparison.
    MarkReviewed(String),
```

- [ ] **Step 8: Answer `MarkReviewed` in the session**

`src/engine/session.rs`. `Change` gains a variant and `Loaded` a field:

```rust
enum Change {
    Scope(Scope),
    Base(Option<String>),
    Mark(String),
}
```

```rust
struct Loaded {
    // ... existing fields ...
    /// `Some` when this refresh answered a mark: the record it published and whether it was written.
    marked: Option<(Mark, Result<(), String>)>,
}
```

`State` gains `session_mark: Option<MarkRecord>` next to `inputs`, and `mark_seq: u64` next to `pick_seq`.

In the command loop, next to `SetBase`:

```rust
                    Command::MarkReviewed(commit) => {
                        state.changes.push_back(Change::Mark(commit));   // see the repeat guard below
                        let mut next = state.snapshot.clone();
                        next.refreshing = true;
                        publish(&mut state, next, &snapshots);
                        state.request_status(&cwd, false, &results_tx, &refreshes);
                    }
```

`request_status` maps it to a job that keeps the base exactly as it is:

```rust
        let base = match (&change, std::mem::take(&mut self.resolve_pending)) {
            (Some(Change::Base(pick)), _) => BaseJob::Pick(pick.clone()),
            (Some(Change::Scope(_)), _) | (None, true) => BaseJob::Resolve,
            (Some(Change::Mark(_)), _) => BaseJob::Keep(self.snapshot.base.clone()),
            (None, false) => BaseJob::Keep(self.snapshot.base.clone()),
        };
```

and the scope it requests for a mark is `self.requested_scope` (a mark changes no scope), so extend that match too:

```rust
        let scope = match &change {
            Some(Change::Scope(scope)) => *scope,
            Some(Change::Base(_)) => Scope::Branch,
            Some(Change::Mark(_)) | None => self.requested_scope,
        };
```

In `load_rows`, after the rows are built, answer a mark:

```rust
    let marked = match &job.change {
        Some(Change::Mark(commit)) => {
            let id = base::verify(toplevel, commit).await?;
            let at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let record = base::MarkRecord { commit: id, at };
            let written = match &job.inputs.state_dir {
                Some(dir) => base::save_mark(dir, toplevel, &record).map_err(|e| e.to_string()),
                None => Err("no state directory".to_string()),
            };
            // Task 3 replaces this construction with `marks::classify`, which is what
            // decides the state: `drawn_head` can lag `head_seen`, so a fresh mark is not
            // automatically an ancestor of the head this refresh observed.
            Some((
                Mark {
                    commit: record.commit.clone(),
                    at: record.at,
                    state: MarkState::Current,
                    classified_at: None,
                },
                written,
            ))
        }
        _ => None,
    };
```

`base::verify` returns `not a commit: <text>` for an id that does not resolve; the `?` sends it to the arm below, which routes it to `mark_error`. Shorten the id in that message to seven characters where the command carried a full one, by mapping the error:

```rust
            let id = base::verify(toplevel, commit)
                .await
                .map_err(|_| format!("not a commit: {}", &commit[..commit.len().min(7)]))?;
```

`Loaded` carries `marked`, and the `Done::Status` arm publishes it:

```rust
                                        if let Some((mark, written)) = loaded.marked {
                                            state.mark_seq += 1;
                                            next.mark_seq = state.mark_seq;
                                            next.mark_error = None;
                                            next.mark = Some(mark.clone());
                                            match written {
                                                Ok(()) => state.session_mark = None,
                                                Err(e) => {
                                                    state.session_mark = Some(base::MarkRecord {
                                                        commit: mark.commit.clone(),
                                                        at: mark.at,
                                                    });
                                                    next.base_error =
                                                        Some(format!("mark not remembered: {e}"));
                                                }
                                            }
                                        }
```

A failed write is reported on the same channel as the mark itself, not through
`base_error`: the picker can be open, and `observe` suppresses a `base_error` while it is
(7.4). So the success branch above becomes

```rust
                                        if let Some((mark, written)) = loaded.marked {
                                            mark_answered = true;
                                            state.mark_seq += 1;
                                            next.mark_seq = state.mark_seq;
                                            next.mark = Some(mark.clone());
                                            match written {
                                                Ok(()) => {
                                                    state.session_mark = None;
                                                    next.mark_error = None;
                                                }
                                                Err(e) => {
                                                    state.session_mark = Some(base::MarkRecord {
                                                        commit: mark.commit.clone(),
                                                        at: mark.at,
                                                    });
                                                    next.mark_error =
                                                        Some(format!("mark not remembered: {e}"));
                                                }
                                            }
                                        }
```

-- `mark` is `Some` and `mark_error` is `Some` together exactly when the mark was taken but
not persisted, which is what 8.2 describes and what Task 4's `observe` reads.

A failed load whose change was a mark answers on the mark channel, and so does every other
way a consumed mark can end without an answer -- a status command that failed, or a
directory that is not a repository, both of which leave `loaded` at `None` after the queue
has already taken the change:

```rust
                                    Some(Err(e)) => match &change {
                                        Some(Change::Base(_)) => change_error = Some(e),
                                        Some(Change::Mark(_)) => {
                                            mark_answer = Some(e);
                                        }
                                        Some(Change::Scope(_)) => next.base_error = Some(e),
                                        None => next.status_error = Some(e),
                                    },
```

with `let mut mark_answer: Option<String> = None;` declared beside `change_error`, the
`Err(e)` arm of `response` setting `mark_answer = Some(e.clone())` when the change was a
mark, the `loaded: None` fallback setting
`mark_answer = Some("not a git repository".to_string())` for one, and a single block after
the match that answers whatever is left:

```rust
                        if matches!(change, Some(Change::Mark(_))) && !mark_answered {
                            state.mark_seq += 1;
                            next.mark_seq = state.mark_seq;
                            next.mark_error = mark_answer.or_else(|| Some("no answer".to_string()));
                        }
```

with `let mut mark_answered = false;` beside `mark_answer`, set to `true` by the success block
above — comparing the two counters cannot work, because the success block has already made them
equal. Exactly one `mark_seq` advance then follows every `MarkReviewed` the queue consumed.

- [ ] **Step 9: Load the record on every resolution**

In `load_rows`, where the preference is resolved, read the mark beside the picks so a mark written by another viewer arrives:

```rust
    let mark_record = match (&state_mark_override, &job.inputs.state_dir) {
        (Some(record), _) => Some(record.clone()),
        (None, Some(dir)) => {
            let (marks, problem) = base::load_marks(dir);
            if let Some(problem) = problem {
                base::note_problem(dir, &problem);
            }
            marks.get(toplevel).cloned()
        }
        (None, None) => None,
    };
```

where `state_mark_override` is `job.inputs.session_mark`, a new field on `ResolveInputs` carrying `Option<MarkRecord>` that `request_status` fills from `state.session_mark`. When `marked` is `None` and a record exists, `Loaded` publishes it as a `Mark` with `state: MarkState::Current` and `classified_at: None` — Task 3 replaces that placeholder with the real classification, and until then the field simply says "not classified yet".

- [ ] **Step 10: Run everything**

Run the full gate of the Global Constraints, plus `cargo test --locked --lib engine::session -- --test-threads=1` three times.
Expected: all pass, the four new tests included.

Applied after the pre-PR review (codex, 2026-09-23): `M` is a single key and auto-repeats, so the
arm drops a press that names the *latest pending* mark -- the last `Change::Mark` in `changes`,
or, when none is queued, the one the refresh in flight is carrying (`State.in_flight_mark`,
cleared on every `Done::Status`). This is the same coalescing the engine already does for
refreshes and diffs. Comparing against the in-flight mark alone is wrong and was caught by the
review's second round: `A → B → A` would drop the last press and persist `B`, so the rule reads
the queue first. A deliberate retry still lands, because the answer that prompts it clears the
in-flight mark first, and a press dropped here never reaches the queue and so owes no answer --
every mark that does reach it still advances `mark_seq` exactly once, which is what the
failure-path test checks.

- [ ] **Step 11: Commit (orchestrator)**

```bash
git add -A
git commit -m "feat(engine): record and publish a review mark"
```

---

### Task 3: Unread rows and the ancestry classification

Implements spec 8.3 in full, and the mark rows of 8.6. Read 8.3, then 7.2's row rules, before starting.

**Files:**
- Create: `src/engine/marks.rs`
- Modify: `src/engine/mod.rs` (`pub mod marks;`)
- Modify: `src/engine/types.rs` (`Snapshot.unread`)
- Modify: `src/engine/session.rs` (`load_rows` computes both, `Loaded.unread`, the publication)

**Interfaces:**
- Consumes: Task 1's `base::git` and `head_seen`; Task 2's `Mark`, `MarkState`, `MarkRecord`.
- Produces:

```rust
// src/engine/marks.rs
/// Paths whose committed content differs between `mark` and `head`.
pub(crate) async fn unread(toplevel: &str, mark: &str, head: &str) -> Result<BTreeSet<String>, String>;
/// `Ok(true)`/`Ok(false)` for exit 0/1; `Err` for any other status, a signal, or a spawn failure.
pub(crate) async fn is_ancestor(toplevel: &str, mark: &str, head: &str) -> Result<bool, String>;
/// The state and set for one refresh, given what the previous snapshot carried.
pub(crate) async fn classify(
    toplevel: &str,
    record: &base::MarkRecord,
    head: &str,
    previous: Option<&Mark>,
    previous_unread: Option<(&str, &BTreeSet<String>)>,   // the head that set was read at, and the set
) -> (Mark, BTreeSet<String>);

// src/engine/types.rs
// Snapshot gains: pub unread: Arc<BTreeSet<String>>
```

- [ ] **Step 1: Write the failing tests**

Create `src/engine/marks.rs` with only this test module:

```rust
//! The unread set and the ancestry classification of a review mark (spec 8.3).

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as Proc;

    fn run(dir: &std::path::Path, args: &[&str]) {
        assert!(
            Proc::new("git").arg("-C").arg(dir).args(args).status().unwrap().success(),
            "git {args:?}"
        );
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
    }

    fn head_of(dir: &std::path::Path, rev: &str) -> String {
        let out = Proc::new("git").arg("-C").arg(dir).args(["rev-parse", rev]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// main, then four commits: a.txt, b.txt, c.txt (which also rewrites a.txt), d.txt.
    fn fixture() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        run(p, &["init", "-q", "-b", "main"]);
        run(p, &["config", "user.email", "t@example.com"]);
        run(p, &["config", "user.name", "t"]);
        for (n, files) in [
            (1, vec![("a.txt", "one\n")]),
            (2, vec![("b.txt", "two\n")]),
            (3, vec![("c.txt", "three\n"), ("a.txt", "one changed\n")]),
            (4, vec![("d.txt", "four\n")]),
        ] {
            for (name, body) in files {
                std::fs::write(p.join(name), body).unwrap();
            }
            run(p, &["add", "-A"]);
            run(p, &["commit", "-q", "-m", &format!("c{n}")]);
        }
        let toplevel = p.canonicalize().unwrap().to_string_lossy().into_owned();
        (dir, toplevel)
    }

    #[test]
    fn unread_is_the_net_difference_between_two_commits() {
        let (dir, top) = fixture();
        let second = head_of(dir.path(), "HEAD~2");
        let head = head_of(dir.path(), "HEAD");
        let set = rt().block_on(unread(&top, &second, &head)).unwrap();
        let mut names: Vec<&str> = set.iter().map(String::as_str).collect();
        names.sort();
        assert_eq!(names, ["a.txt", "c.txt", "d.txt"], "b.txt was already read");

        // A path changed and then restored carries no dot: this is a net difference.
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        run(dir.path(), &["add", "-A"]);
        run(dir.path(), &["commit", "-q", "-m", "restore a"]);
        let head = head_of(dir.path(), "HEAD");
        let set = rt().block_on(unread(&top, &second, &head)).unwrap();
        assert!(!set.contains("a.txt"), "{set:?}");
        assert!(set.contains("c.txt") && set.contains("d.txt"));

        // The set is empty against itself, which is what M leaves behind.
        assert!(rt().block_on(unread(&top, &head, &head)).unwrap().is_empty());
    }

    #[test]
    fn rename_detection_is_off_so_both_endpoints_appear() {
        let (dir, top) = fixture();
        let before = head_of(dir.path(), "HEAD");
        run(dir.path(), &["mv", "a.txt", "renamed.txt"]);
        run(dir.path(), &["commit", "-q", "-m", "rename"]);
        let head = head_of(dir.path(), "HEAD");
        let set = rt().block_on(unread(&top, &before, &head)).unwrap();
        let mut names: Vec<&str> = set.iter().map(String::as_str).collect();
        names.sort();
        assert_eq!(names, ["a.txt", "renamed.txt"], "both endpoints, not just the destination");
    }

    #[test]
    fn ancestry_answers_only_on_exit_zero_and_one() {
        let (dir, top) = fixture();
        let head = head_of(dir.path(), "HEAD");
        let second = head_of(dir.path(), "HEAD~2");
        assert_eq!(rt().block_on(is_ancestor(&top, &second, &head)), Ok(true));
        run(dir.path(), &["switch", "-q", "-c", "side", "HEAD~1"]);
        std::fs::write(dir.path().join("side.txt"), "side\n").unwrap();
        run(dir.path(), &["add", "-A"]);
        run(dir.path(), &["commit", "-q", "-m", "side"]);
        let side = head_of(dir.path(), "HEAD");
        assert_eq!(rt().block_on(is_ancestor(&top, &head, &side)), Ok(false));
        // A commit that does not exist is an error, not a false answer.
        assert!(rt().block_on(is_ancestor(&top, &"b".repeat(40), &side)).is_err());
    }

    #[test]
    fn an_answered_pair_is_not_asked_again() {
        let (dir, top) = fixture();
        let head = head_of(dir.path(), "HEAD");
        let second = head_of(dir.path(), "HEAD~2");
        let record = base::MarkRecord { commit: second, at: 1 };
        let (mark, set) = rt().block_on(classify(&top, &record, &head, None, None));
        assert!(set.contains("c.txt"));
        // Point the repository at a git that would fail if it were called: the cached pair
        // must be returned without running anything.
        let cached = rt().block_on(classify(
            "/nonexistent-toplevel",
            &record,
            &head,
            Some(&mark),
            Some((head.as_str(), &set)),
        ));
        assert_eq!(cached.0, mark);
        assert_eq!(cached.1, set);

        // Without a previous branch-scope set there is no cache: a scope switch and back
        // must recompute rather than keep worktree scope's empty set.
        let (again, set_again) = rt().block_on(classify(&top, &record, &head, Some(&mark), None));
        assert_eq!(again.state, MarkState::Current);
        assert_eq!(set_again, set);

        // A newer record for the same commit keeps the classification and takes its time.
        let newer = base::MarkRecord { commit: record.commit.clone(), at: record.at + 60 };
        let (fresh, _) = rt().block_on(classify(
            "/nonexistent-toplevel",
            &newer,
            &head,
            Some(&mark),
            Some((head.as_str(), &set)),
        ));
        assert_eq!(fresh.at, newer.at);

        // A set read at another head is not reused, however well the classification matches:
        // the dots must describe the head on screen.
        let earlier = head_of(dir.path(), "HEAD~1");
        let (other, other_set) = rt().block_on(classify(&top, &record, &earlier, None, None));
        assert_ne!(other_set, set, "the two heads have different unread sets");
        let (back, back_set) = rt().block_on(classify(
            &top,
            &record,
            &head,
            Some(&other),
            Some((earlier.as_str(), &other_set)),
        ));
        assert_eq!(back_set, set, "the set for this head is recomputed, not carried over");
        assert_eq!(back.classified_at.as_deref(), Some(head.as_str()));
    }

    #[test]
    fn an_unanswered_ancestry_keeps_the_previous_classification_and_retries() {
        let (dir, top) = fixture();
        let head = head_of(dir.path(), "HEAD");
        let second = head_of(dir.path(), "HEAD~2");
        // Failure injection with nothing but git: a tree id is a real object that
        // `--is-ancestor` refuses to answer for (`not a valid commit name`, exit 128) while
        // `diff` reads it as a tree-ish quite happily. Ancestry fails, the read succeeds --
        // the one combination 8.7(4) is about.
        let tree = head_of(dir.path(), "HEAD~2^{tree}");
        assert!(rt().block_on(is_ancestor(&top, &tree, &head)).is_err());
        assert!(rt().block_on(unread(&top, &tree, &head)).unwrap().contains("c.txt"));

        let record = base::MarkRecord { commit: tree.clone(), at: 1 };
        let previous = Mark {
            commit: tree.clone(),
            at: 1,
            state: MarkState::Current,
            classified_at: Some(second.clone()),
        };
        let (mark, set) = rt().block_on(classify(&top, &record, &head, Some(&previous), None));
        assert_eq!(mark.state, MarkState::Current, "the previous classification survives");
        assert_eq!(
            mark.classified_at.as_deref(),
            Some(second.as_str()),
            "the pair is left undated, so nothing warns about an answer nobody gave"
        );
        assert!(set.contains("c.txt"), "the rows come from the read that did answer");

        // And the next refresh asks again: a pair this head never classified is not served
        // from the cache, even though the set beside it was read here.
        let (again, set_again) = rt().block_on(classify(
            &top,
            &record,
            &head,
            Some(&mark),
            Some((head.as_str(), &set)),
        ));
        assert_eq!(again.classified_at.as_deref(), Some(second.as_str()));
        assert_eq!(set_again, set);

        // Nor is that set reused back at the head the classification does name: it was read
        // here, not there.
        let (at_old, set_at_old) = rt().block_on(classify(
            &top,
            &record,
            &second,
            Some(&mark),
            Some((head.as_str(), &set)),
        ));
        assert_eq!(
            set_at_old,
            rt().block_on(unread(&top, &tree, &second)).unwrap(),
            "the older head's own rows, not the newer head's"
        );
        assert!(matches!(at_old.state, MarkState::Current));
    }

    #[test]
    fn classify_reports_current_rewritten_and_unreadable() {
        let (dir, top) = fixture();
        let head = head_of(dir.path(), "HEAD");
        let second = head_of(dir.path(), "HEAD~2");
        let record = base::MarkRecord { commit: second.clone(), at: 1 };

        let (mark, set) = rt().block_on(classify(&top, &record, &head, None, None));
        assert_eq!(mark.state, MarkState::Current);
        assert_eq!(mark.classified_at.as_deref(), Some(head.as_str()));
        assert!(set.contains("c.txt") && !set.contains("b.txt"));

        // A commit on another branch is Rewritten, and then every row is the caller's job:
        // the set classify returns is empty, and the caller flags everything.
        run(dir.path(), &["switch", "-q", "-c", "side", "HEAD~1"]);
        std::fs::write(dir.path().join("side.txt"), "side\n").unwrap();
        run(dir.path(), &["add", "-A"]);
        run(dir.path(), &["commit", "-q", "-m", "side"]);
        let side = head_of(dir.path(), "HEAD");
        let record = base::MarkRecord { commit: head.clone(), at: 1 };
        let (mark, set) = rt().block_on(classify(&top, &record, &side, None, None));
        assert_eq!(mark.state, MarkState::Rewritten);
        assert!(set.is_empty());

        // An id that no longer exists: `--is-ancestor` gives no answer and the read fails,
        // which is conclusive about this pair, so the state becomes Unreadable and is dated.
        let gone = base::MarkRecord { commit: "b".repeat(40), at: 1 };
        let previous = Mark {
            commit: gone.commit.clone(),
            at: 1,
            state: MarkState::Current,
            classified_at: Some("old".into()),
        };
        let (mark, set) = rt().block_on(classify(&top, &gone, &side, Some(&previous), None));
        assert!(matches!(mark.state, MarkState::Unreadable(_)));
        assert_eq!(mark.classified_at.as_deref(), Some(side.as_str()), "a dated answer, so it warns");
        assert!(set.is_empty());

        // A mark equal to the head needs no command and is Current.
        let same = base::MarkRecord { commit: side.clone(), at: 1 };
        let (mark, set) = rt().block_on(classify(&top, &same, &side, None, None));
        assert_eq!(mark.state, MarkState::Current);
        assert!(set.is_empty());
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Add `pub mod marks;` to `src/engine/mod.rs` together with this test module, so the run compiles the module rather than matching nothing.

Run: `cargo test --locked --lib engine::marks`
Expected: compile errors — `unread`, `is_ancestor`, `classify` are undefined.

- [ ] **Step 3: Implement the module**

Above the tests in `src/engine/marks.rs`:

```rust
use std::collections::BTreeSet;

use crate::engine::{base, Mark, MarkState};

/// Paths whose committed content differs between `mark` and `head`. Rename detection is off:
/// git reports a detected rename by its destination alone, which would leave the deletion of
/// the source -- a row of its own -- unflagged.
pub(crate) async fn unread(
    toplevel: &str,
    mark: &str,
    head: &str,
) -> Result<BTreeSet<String>, String> {
    let output = base::git(
        toplevel,
        &["diff", mark, head, "--name-only", "--no-renames", "-z", "--"],
    )
    .await?;
    if !output.status.success() {
        return Err(format!(
            "git diff --name-only failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output
        .stdout
        .split(|&b| b == 0)
        .filter(|f| !f.is_empty())
        .map(|f| String::from_utf8_lossy(f).into_owned())
        .collect())
}

/// `Ok(true)` on exit 0, `Ok(false)` on exit 1, `Err` on anything else: `128` for a missing
/// object, a signal, or a failure to spawn. The frozen runner returns `Ok` for every status.
pub(crate) async fn is_ancestor(toplevel: &str, mark: &str, head: &str) -> Result<bool, String> {
    let output = base::git(toplevel, &["merge-base", "--is-ancestor", mark, head]).await?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        other => Err(match other {
            Some(code) => format!("merge-base --is-ancestor exited {code}"),
            None => "merge-base --is-ancestor was killed".to_string(),
        }),
    }
}

/// One refresh's answer for a mark: its state and the paths to flag. An empty set with a state
/// other than `Current` means "flag every row"; the caller reads `state`, never the set's size.
pub(crate) async fn classify(
    toplevel: &str,
    record: &base::MarkRecord,
    head: &str,
    previous: Option<&Mark>,
    previous_unread: Option<(&str, &BTreeSet<String>)>,
) -> (Mark, BTreeSet<String>) {
    // An answered pair is not asked again: both commits are fixed, so neither the state nor
    // the set can change while they do. This is what keeps a settled repository at zero git
    // processes per poll for this section. The set carries the head it was read at, because
    // the two dates can differ: the retry path below returns a set read at this head while
    // leaving the classification dated at an earlier one. Reuse needs both to be this head --
    // the state was decided here, and the set was read here.
    if let (Some(p), Some((read_at, set))) = (previous, previous_unread) {
        if p.commit == record.commit
            && read_at == head
            && p.classified_at.as_deref() == Some(head)
        {
            // The timestamp comes from the record just read: another viewer may have marked
            // the same commit again, and the age shown must follow it.
            let mut mark = p.clone();
            mark.at = record.at;
            return (mark, set.clone());
        }
    }
    let mut mark = Mark {
        commit: record.commit.clone(),
        at: record.at,
        state: previous
            .filter(|p| p.commit == record.commit)
            .map(|p| p.state.clone())
            .unwrap_or(MarkState::Current),
        classified_at: previous
            .filter(|p| p.commit == record.commit)
            .and_then(|p| p.classified_at.clone()),
    };
    if record.commit == head {
        mark.state = MarkState::Current;
        mark.classified_at = Some(head.to_string());
        return (mark, BTreeSet::new());
    }
    match is_ancestor(toplevel, &record.commit, head).await {
        Ok(true) => match unread(toplevel, &record.commit, head).await {
            Ok(set) => {
                mark.state = MarkState::Current;
                mark.classified_at = Some(head.to_string());
                (mark, set)
            }
            Err(reason) => {
                mark.state = MarkState::Unreadable(reason);
                mark.classified_at = Some(head.to_string());
                (mark, BTreeSet::new())
            }
        },
        Ok(false) => {
            mark.state = MarkState::Rewritten;
            mark.classified_at = Some(head.to_string());
            (mark, BTreeSet::new())
        }
        // No answer from `--is-ancestor`: the pair stays unclassified and the next refresh
        // tries again -- unless the read below fails too, which is a conclusive answer about
        // this pair (a pruned object) and is dated like any other.
        Err(_) => match unread(toplevel, &record.commit, head).await {
            Ok(set) => (mark, set),
            Err(reason) => {
                mark.state = MarkState::Unreadable(reason);
                mark.classified_at = Some(head.to_string());
                (mark, BTreeSet::new())
            }
        },
    }
}
```

- [ ] **Step 4: Run the module tests**

Run: `cargo test --locked --lib engine::marks`
Expected: PASS (five tests).

- [ ] **Step 5: Publish the set from the session**

`src/engine/types.rs`: `Snapshot` gains

```rust
    /// Branch scope only: row paths a commit changed after the mark. Empty in worktree
    /// scope, with no mark, and whenever `Mark::state` is not `Current` -- a state other
    /// than `Current` means every row is flagged, which the view reads from the state.
    pub unread: Arc<BTreeSet<String>>,
```

(`Arc::new(BTreeSet::new())` in `empty`, and in `fingerprint`). `Loaded` gains
`unread: BTreeSet<String>` and `unread_at: Option<String>` — the head that set was read at,
`head_id.map(str::to_string)` in the branch arm below and `None` in the other two — and
`load_rows` fills them with the mark:

```rust
    let (mark, unread) = match (&mark_record, head_id) {
        (Some(record), Some(head)) if scope == Scope::Branch => {
            let (mark, set) = marks::classify(
                toplevel,
                record,
                head,
                job.previous_mark.as_ref(),
                job.previous_unread
                    .as_ref()
                    .map(|(at, set)| (at.as_str(), set)),
            )
            .await;
            (Some(mark), set)
        }
        (Some(record), _) => (
            Some(Mark {
                commit: record.commit.clone(),
                at: record.at,
                state: job
                    .previous_mark
                    .as_ref()
                    .filter(|p| p.commit == record.commit)
                    .map(|p| p.state.clone())
                    .unwrap_or(MarkState::Current),
                classified_at: job
                    .previous_mark
                    .as_ref()
                    .filter(|p| p.commit == record.commit)
                    .and_then(|p| p.classified_at.clone()),
            }),
            BTreeSet::new(),
        ),
        (None, _) => (None, BTreeSet::new()),
    };
```

with `Job` gaining `previous_mark: Option<Mark>` and
`previous_unread: Option<(String, BTreeSet<String>)>` — the head the published set was read at,
and the set — filled in `request_status`:

```rust
            previous_mark: self.snapshot.mark.clone(),
            previous_unread: self
                .unread_at
                .clone()
                .map(|at| (at, (*self.snapshot.unread).clone())),
```

`Loaded` gains `unread_at: Option<String>`, which `load_rows` fills with the head it classified
at (`head_id.map(str::to_string)` in the branch arm, `None` in the other two), and `State` gains
`unread_at: Option<String>`, assigned beside `next.unread` on every publication. Routing the
provenance through one field rather than reading `snapshot.scope` covers all three ways a
published set can fail to describe the current head: worktree scope publishes an empty set, a
refresh that could not sample the head publishes one too, and the retry path publishes a set
read at a head the classification does not name. Any of them leaves `unread_at` unequal to the
head (or `None`), and the next refresh recomputes instead of reusing dots that were never about
this commit.

A mark this refresh has just written is classified the same way, not assumed current: `M`
submits `drawn_head`, which can be older than the `head_seen` this refresh observed, so an
amend between the frame and the press leaves a fresh mark that is already off the branch.
Replace Task 2's construction in the `marked` block with

```rust
            let (mark, set) = match head_id {
                Some(head) if scope == Scope::Branch => {
                    marks::classify(toplevel, &record, head, None, None).await
                }
                _ => (
                    Mark {
                        commit: record.commit.clone(),
                        at: record.at,
                        state: MarkState::Current,
                        classified_at: None,
                    },
                    BTreeSet::new(),
                ),
            };
            Some((mark, set, written, head_id.map(str::to_string)))
```

so `Loaded.marked` becomes
`Option<(Mark, BTreeSet<String>, Result<(), String>, Option<String>)>` and the publication
carries the pair, its answer and its provenance together. Publish the refresh's own pair first:

```rust
                                        next.mark = loaded.mark;
                                        next.unread = Arc::new(loaded.unread);
                                        state.unread_at = loaded.unread_at;
```

right after `next.rename_sources` is assigned, and keep Task 2's `marked` block *after* it,
now setting both:

```rust
                                        if let Some((mark, set, written, read_at)) = loaded.marked {
                                            next.mark = Some(mark.clone());
                                            next.unread = Arc::new(set);
                                            state.unread_at = read_at;
                                            // ... the mark_seq and session_mark handling of Task 2
                                        }
```

A fresh mark never leaves the previous mark's set behind it, and `read_at` — the `head_id` its
`classify` call used, `None` outside branch scope — keeps the provenance of the published set
true for the next refresh's cache.

- [ ] **Step 6: Write the failing session test for the published set**

```rust
    #[test]
    fn the_unread_set_follows_the_mark_and_empties_on_a_new_one() {
        let dir = branch_fixture();
        let state = tempfile::tempdir().unwrap();
        let (_rt, h) = start_with(dir.path(), Scope::Branch, Some(state.path().to_path_buf()), None);
        let s = wait_for(&h, "first", |s| ready(s).is_some());
        assert!(s.unread.is_empty(), "no mark, no dots");
        let head = s.head.clone().unwrap();

        h.commands.send(Command::MarkReviewed(head)).unwrap();
        let s = wait_for(&h, "marked", |s| s.mark_seq == 1);
        assert!(s.unread.is_empty(), "marking the head leaves nothing unread");

        // Only `fresh.txt` is committed: `branch_fixture` leaves an edited `a.txt` and an
        // untracked `u.txt`, and the assertions below are about exactly those staying out.
        std::fs::write(dir.path().join("fresh.txt"), "fresh\n").unwrap();
        git(dir.path(), &["add", "fresh.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "fresh"]);
        // The dots can arrive while the previous diff is still on screen, whose id is the
        // older commit; wait for the new commit's own diff before marking again.
        let fresh_head = git_out(dir.path(), &["rev-parse", "HEAD"]).trim().to_string();
        let s = wait_for(&h, "the new commit is unread and drawn", |s| {
            s.unread.contains("fresh.txt") && s.head.as_deref() == Some(fresh_head.as_str())
        });
        assert!(!s.unread.contains("a.txt"), "{:?}", s.unread);
        assert!(!s.unread.contains("u.txt"), "untracked rows are never in the set");
        assert_eq!(s.mark.as_ref().map(|m| m.state.clone()), Some(crate::engine::MarkState::Current));

        // Uncommitted work raises nothing, and a second mark clears the set on a dirty tree.
        std::fs::write(dir.path().join("a.txt"), "one\nTWO\nthree\nfour\n").unwrap();
        let s = wait_for(&h, "the edit is listed", |s| {
            s.files.iter().any(|f| f.path == "a.txt")
        });
        assert!(!s.unread.contains("a.txt"), "an uncommitted edit is not a dot");
        let head = wait_for(&h, "markable again", |s| {
            s.head.as_deref() == Some(fresh_head.as_str())
        })
        .head
        .clone()
        .unwrap();
        h.commands.send(Command::MarkReviewed(head)).unwrap();
        let s = wait_for(&h, "marked again", |s| s.mark_seq == 2);
        assert!(s.unread.is_empty(), "M clears every dot even with a dirty worktree");
    }

    #[test]
    fn a_scope_switch_and_back_keeps_the_dots() {
        let dir = branch_fixture();
        let state = tempfile::tempdir().unwrap();
        let (_rt, h) = start_with(dir.path(), Scope::Branch, Some(state.path().to_path_buf()), None);
        let s = wait_for(&h, "first", |s| ready(s).is_some());
        h.commands.send(Command::MarkReviewed(s.head.clone().unwrap())).unwrap();
        wait_for(&h, "marked", |s| s.mark_seq == 1);
        std::fs::write(dir.path().join("fresh.txt"), "fresh\n").unwrap();
        git(dir.path(), &["add", "fresh.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "fresh"]);
        wait_for(&h, "a dot", |s| s.unread.contains("fresh.txt"));

        h.commands.send(Command::SetScope(Scope::Worktree)).unwrap();
        let s = wait_for(&h, "worktree", |s| s.scope == Scope::Worktree);
        assert!(s.unread.is_empty(), "worktree scope publishes no set");
        h.commands.send(Command::SetScope(Scope::Branch)).unwrap();
        let s = wait_for(&h, "branch again", |s| s.scope == Scope::Branch);
        assert!(s.unread.contains("fresh.txt"), "the dots come back: {:?}", s.unread);
    }

    #[test]
    fn an_amended_mark_is_rewritten_and_one_mark_repairs_it() {
        let dir = branch_fixture();
        let state = tempfile::tempdir().unwrap();
        let (_rt, h) = start_with(dir.path(), Scope::Branch, Some(state.path().to_path_buf()), None);
        let s = wait_for(&h, "first", |s| ready(s).is_some());
        h.commands.send(Command::MarkReviewed(s.head.clone().unwrap())).unwrap();
        wait_for(&h, "marked", |s| s.mark_seq == 1);

        // A changed message guarantees a different object id; `--no-edit` inside one second
        // can reproduce the same commit.
        git(dir.path(), &["commit", "-q", "--amend", "-m", "amended"]);
        let s = wait_for(&h, "rewritten", |s| {
            matches!(s.mark.as_ref().map(|m| m.state.clone()), Some(crate::engine::MarkState::Rewritten))
        });
        assert!(s.unread.is_empty(), "the view flags every row from the state, not the set");
        assert!(s.files.len() >= 4, "the rows and the base keep working");

        // The dots and the `Rewritten` state arrive while the pre-amend diff is still on
        // screen, whose id is the commit that was just replaced; marking that one repairs
        // nothing. Wait for the amended commit's own diff.
        let amended = git_out(dir.path(), &["rev-parse", "HEAD"]).trim().to_string();
        let head = wait_for(&h, "markable at the amended commit", |s| {
            s.head.as_deref() == Some(amended.as_str())
        })
        .head
        .clone()
        .unwrap();
        h.commands.send(Command::MarkReviewed(head)).unwrap();
        let s = wait_for(&h, "repaired", |s| s.mark_seq == 2);
        assert_eq!(s.mark.as_ref().map(|m| m.state.clone()), Some(crate::engine::MarkState::Current));
    }
```

- [ ] **Step 7: Run everything**

Run the full gate, plus `cargo test --locked --lib engine -- --test-threads=1` three times.
Expected: all pass. The amend test depends on the watcher noticing `<git_dir>/HEAD`-adjacent writes; if it is slow, wait on the snapshot predicate (as written) rather than sleeping.

- [ ] **Step 8: Commit (orchestrator)**

```bash
git add -A
git commit -m "feat(engine): unread rows and the mark's classification"
```

---

### Task 4: The panel marker, the `M` key, urgent notices and the empty-list wording

Implements spec 8.5 in full and the display half of 8.2 ("How a failed mark is shown"). Read 8.5, 8.2's notice paragraph and 4.2's notice priority before starting.

**Files:**
- Modify: `src/tui/keys.rs` (`KeyAction::MarkReviewed`, the `M` binding, the count 24)
- Modify: `src/tui/state.rs` (`Notice`, `observe` for `mark_seq`, `seen_rewrite`)
- Modify: `src/tui/view.rs` (the marker column, `notice`'s ranking, `state_message`)
- Modify: `src/tui/input.rs` (`M` sends `MarkReviewed(drawn_head)`)

**Interfaces:**
- Consumes: Task 1's `ViewState.drawn_head`; Task 2's `Snapshot.{mark, mark_seq, mark_error}`, `Command::MarkReviewed`; Task 3's `Snapshot.unread` and `MarkState`.
- Produces:

```rust
// src/tui/state.rs
pub struct Notice { pub text: String, pub urgent: bool }
// ViewState.notice becomes Option<Notice>; helpers:
impl ViewState { pub fn notify(&mut self, text: impl Into<String>); pub fn warn(&mut self, text: impl Into<String>); }
// keys.rs: KeyAction::MarkReviewed (`M`, label `mark reviewed`)
// view.rs: fn unread_marker(snapshot: &Snapshot, file: &ChangedFile) -> bool
```

- [ ] **Step 1: Write the failing tests**

`src/tui/keys.rs`: change both `23` assertions to `24`, and add next to the `B` assertion:

```rust
        assert_eq!(
            lookup(&KeyEvent::new(KeyCode::Char('M'), KeyModifiers::SHIFT)),
            Some(KeyAction::MarkReviewed)
        );
```

`src/tui/view.rs` tests:

```rust
    #[test]
    fn the_panel_marks_unread_rows_in_branch_scope_only() {
        use crate::engine::{Base, BaseSource, Mark, MarkState, Scope};
        use crate::git::{ChangedFile, ChangedFileStatus};
        let mut snap = snapshot("a.rs", "r1", &[(1, "+")]);
        snap.files = vec![
            ChangedFile { path: "a.rs".into(), status: ChangedFileStatus::Modified, staged: false, insertions: Some(1), deletions: Some(0) },
            ChangedFile { path: "b.rs".into(), status: ChangedFileStatus::Added, staged: false, insertions: Some(1), deletions: Some(0) },
            ChangedFile { path: "u.rs".into(), status: ChangedFileStatus::Untracked, staged: false, insertions: None, deletions: None },
        ];
        snap.scope = Scope::Branch;
        snap.base = Some(Base { requested: "refs/heads/main".into(), commit: "0".repeat(40), merge_base: Some("1".repeat(40)), source: BaseSource::Default });
        snap.mark = Some(Mark { commit: "2".repeat(40), at: 1, state: MarkState::Current, classified_at: Some("3".repeat(40)) });
        snap.unread = std::sync::Arc::new(["a.rs".to_string()].into_iter().collect());
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Pinned, true);
        st.resize(120, 20);
        st.reconcile(&snap);
        // Only the panel's own columns: line 0 is the toolbar, which also names the file.
        let panel = |text: &[String], name: &str| -> String {
            text.iter()
                .skip(1)
                .map(|l| l.chars().take(usize::from(FILES_WIDTH)).collect::<String>())
                .find(|l| l.contains(name))
                .unwrap_or_default()
        };
        let text = render(&snap, &st, 120, 24).plain();
        assert!(panel(&text, "a.rs").contains('●'), "{}", panel(&text, "a.rs"));
        assert!(!panel(&text, "b.rs").contains('●'), "{}", panel(&text, "b.rs"));
        assert!(!panel(&text, "u.rs").contains('●'), "untracked rows never carry one");

        // A rewritten mark flags every row, from the state and not from the set.
        snap.mark.as_mut().unwrap().state = MarkState::Rewritten;
        snap.unread = std::sync::Arc::new(Default::default());
        let text = render(&snap, &st, 120, 24).plain();
        for name in ["a.rs", "b.rs"] {
            assert!(panel(&text, name).contains('●'), "{}", panel(&text, name));
        }

        // Worktree scope never marks, whatever the set says.
        snap.scope = Scope::Worktree;
        snap.mark.as_mut().unwrap().state = MarkState::Current;
        snap.unread = std::sync::Arc::new(["a.rs".to_string()].into_iter().collect());
        let text = render(&snap, &st, 120, 24).plain();
        assert!(!text.iter().skip(1).any(|l| l.contains('●')));
    }

    #[test]
    fn an_urgent_notice_outranks_the_watcher_line_and_an_ordinary_one_does_not() {
        let mut snap = snapshot("a.rs", "r1", &[(1, "+")]);
        snap.watcher_error = Some("inotify limit".into());
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        st.resize(120, 20);
        st.reconcile(&snap);
        st.notify("marked 1234567 as reviewed");
        assert!(notice(&st, &snap).unwrap().contains("live refresh degraded"));
        st.warn("the mark cannot be read: gone");
        assert_eq!(notice(&st, &snap).as_deref(), Some("the mark cannot be read: gone"));
    }

    #[test]
    fn an_empty_branch_list_names_the_base() {
        use crate::engine::{Base, BaseSource, Scope};
        let mut snap = snapshot("a.rs", "r1", &[(1, "+")]);
        snap.files.clear();
        snap.diff = crate::engine::DiffState::Idle;
        snap.selected = None;
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        st.resize(120, 20);
        st.reconcile(&snap);
        assert!(render(&snap, &st, 120, 24).plain().iter().any(|l| l.contains("working tree clean")));
        snap.scope = Scope::Branch;
        snap.base = Some(Base { requested: "refs/heads/main".into(), commit: "0".repeat(40), merge_base: Some("1".repeat(40)), source: BaseSource::Default });
        let text = render(&snap, &st, 120, 24).plain();
        assert!(text.iter().any(|l| l.contains("nothing on this branch since main")), "{text:?}");
    }
```

`src/tui/input.rs` tests:

```rust
    #[test]
    fn capital_m_marks_the_drawn_head_and_refuses_without_one() {
        let (snap, mut st) = setup(&[(1, "+")]);
        assert_eq!(handle_key(&mut st, &snap, key("M"), 120), Outcome::Redraw);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.clone()).as_deref(),
            Some("nothing to mark: no commit yet")
        );
        st.drawn_head = Some("c".repeat(40));
        assert_eq!(
            handle_key(&mut st, &snap, key("M"), 120),
            Outcome::Engine(Command::MarkReviewed("c".repeat(40)))
        );
    }

    #[test]
    fn a_mark_answer_becomes_a_notice_even_behind_a_modal() {
        let (mut snap, mut st) = setup(&[(1, "+")]);
        st.help_open = true;
        snap.mark_seq = 1;
        snap.mark_error = Some("not a commit: abcdef1".into());
        st.observe(&snap);
        assert_eq!(
            st.notice.as_ref().map(|n| (n.text.clone(), n.urgent)),
            Some(("not a commit: abcdef1".to_string(), true))
        );
        // A key the modal consumes leaves it, and so does closing the modal; the first body
        // key afterwards clears it.
        handle_key(&mut st, &snap, key("j"), 120);
        assert!(st.notice.is_some(), "the key sheet's own key does not clear it");
        handle_key(&mut st, &snap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), 120);
        assert!(st.notice.is_some(), "closing the sheet does not clear an urgent notice");
        handle_key(&mut st, &snap, key("t"), 120);
        assert!(st.notice.is_none());

        // The same holds behind the picker, and for a mark that was taken but not written.
        handle_key(&mut st, &snap, key("B"), 120);
        snap.mark_seq = 3;
        snap.mark_error = Some("mark not remembered: no state directory".into());
        snap.mark = Some(crate::engine::Mark {
            commit: "d".repeat(40),
            at: 1,
            state: crate::engine::MarkState::Current,
            classified_at: None,
        });
        st.observe(&snap);
        handle_key(&mut st, &snap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), 120);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.clone()).as_deref(),
            Some("mark not remembered: no state directory")
        );
        // The same error answered twice shows twice.
        snap.mark_seq = 2;
        st.observe(&snap);
        assert!(st.notice.is_some());
    }
```

`src/tui/state.rs` tests:

```rust
    #[test]
    fn the_rewrite_warning_waits_for_a_classified_pair_and_repeats_per_pair() {
        use crate::engine::{Mark, MarkState};
        let mut snap = snapshot("a.rs", "r1", &[(1, "+")]);
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        snap.head_seen = Some("h1".repeat(20));
        snap.mark = Some(Mark {
            commit: "m".repeat(40),
            at: 1,
            state: MarkState::Rewritten,
            classified_at: None,
        });
        st.observe(&snap);
        assert!(st.notice.is_none(), "an unclassified pair says nothing");

        snap.mark.as_mut().unwrap().classified_at = snap.head_seen.clone();
        st.observe(&snap);
        assert!(st.notice.as_ref().unwrap().urgent);
        assert!(st.notice.as_ref().unwrap().text.contains("no longer on this branch"));

        st.notice = None;
        st.observe(&snap);
        assert!(st.notice.is_none(), "once per classified pair");

        snap.head_seen = Some("h2".repeat(20));
        snap.mark.as_mut().unwrap().classified_at = snap.head_seen.clone();
        st.observe(&snap);
        assert!(st.notice.is_some(), "a new pair warns again");
    }

    #[test]
    fn a_mark_answer_outranks_a_rewrite_warning_in_the_same_snapshot() {
        use crate::engine::{Mark, MarkState};
        let mut snap = snapshot("a.rs", "r1", &[(1, "+")]);
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        snap.head_seen = Some("h1".repeat(20));
        // Marking a stale drawn commit while storage is unwritable: the answer and the
        // classification arrive together.
        snap.mark_seq = 1;
        snap.mark_error = Some("mark not remembered: no state directory".into());
        snap.mark = Some(Mark {
            commit: "m".repeat(40),
            at: 1,
            state: MarkState::Rewritten,
            classified_at: snap.head_seen.clone(),
        });
        st.observe(&snap);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.clone()).as_deref(),
            Some("mark not remembered: no state directory"),
            "the answer the user is owed is not overwritten"
        );

        // However many snapshots arrive before the next draw, the answer is still the one on
        // screen: the warning is pending, not queued behind a single observation.
        st.observe(&snap);
        st.observe(&snap);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.clone()).as_deref(),
            Some("mark not remembered: no state directory"),
        );

        // The warning is not lost either: once a body key has cleared the answer, the next
        // snapshot of the same state says it.
        st.notice = None;
        st.observe(&snap);
        assert!(st.notice.as_ref().unwrap().text.contains("no longer on this branch"));
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --locked --lib tui`
Expected: compile errors — `KeyAction::MarkReviewed`, `notify`, `warn`, `Notice` do not exist.

- [ ] **Step 3: The key**

`src/tui/keys.rs`: `KeyAction` gains `MarkReviewed,` after `PickBase`, and the table gains, after the `B` binding:

```rust
    Binding {
        key: "M",
        label: "mark reviewed",
        action: KeyAction::MarkReviewed,
        vimeflow: None,
    },
```

- [ ] **Step 4: Ranked notices**

`src/tui/state.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    /// Shown above the status and watcher lines: an answer or a warning that must be read.
    pub urgent: bool,
}
```

`ViewState.notice` becomes `Option<Notice>`, with two helpers:

```rust
    pub fn notify(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice { text: text.into(), urgent: false });
    }

    pub fn warn(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice { text: text.into(), urgent: true });
    }
```

Every existing assignment becomes a call: `state.notice = Some("split view needs 100 columns".into())` becomes `state.notify("split view needs 100 columns")` in `input.rs` and `shell.rs`; `NO_BASE_NOTICE` and the config notice stay ordinary (`notify`); `pick not remembered` / `base_error` in `observe` becomes `warn`. Tests that read `st.notice.as_deref()` become `st.notice.as_ref().map(|n| n.text.as_str())`.

`observe` gains the mark channel and the rewrite warning:

```rust
    pub fn observe(&mut self, snapshot: &Snapshot) {
        if let Some(picker) = &mut self.picker {
            picker.observe(snapshot);
            if picker.done {
                self.picker = None;
            }
        }
        let mut answered_mark = false;
        if snapshot.mark_seq != self.seen_mark_seq {
            self.seen_mark_seq = snapshot.mark_seq;
            answered_mark = true;
            match (&snapshot.mark_error, &snapshot.mark) {
                // A mark that was taken but not written reports both: the mark is in force
                // for this session, and the reason it will not outlive it.
                (Some(error), _) => self.warn(crate::tui::sanitize::sanitize(error)),
                (None, Some(mark)) => {
                    let short = &mark.commit[..mark.commit.len().min(7)];
                    self.notify(format!("marked {short} as reviewed"));
                }
                (None, None) => {}
            }
        }
        // The warning speaks only for a pair the engine actually classified, which
        // `classify` dates for every conclusive answer -- a rewrite and a failed read alike.
        let rewritten = snapshot.mark.as_ref().and_then(|m| {
            let at = m.classified_at.clone()?;
            (Some(&at) == snapshot.head_seen.as_ref()
                && m.state != crate::engine::MarkState::Current)
                .then(|| (m.commit.clone(), at, m.state.clone()))
        });
        // The warning never displaces an urgent notice the user has not acknowledged, and a
        // mark answered in this same snapshot outranks it too. Its pair stays unrecorded, so
        // it speaks at the first snapshot after a body key clears the notice -- 8.2's rule
        // that an urgent answer stands until then, applied to the one thing that could
        // quietly overwrite it between two draws.
        let urgent_stands = self.notice.as_ref().is_some_and(|n| n.urgent);
        if rewritten != self.seen_rewrite && !answered_mark && !urgent_stands {
            self.seen_rewrite = rewritten.clone();
            match rewritten.map(|(_, _, state)| state) {
                Some(crate::engine::MarkState::Rewritten) => {
                    self.warn("the marked commit is no longer on this branch; press M again")
                }
                Some(crate::engine::MarkState::Unreadable(reason)) => {
                    self.warn(format!("the mark cannot be read: {}", crate::tui::sanitize::sanitize(&reason)))
                }
                _ => {}
            }
        }
        if snapshot.base_error != self.seen_base_error {
            self.seen_base_error = snapshot.base_error.clone();
            if let (Some(error), None) = (&snapshot.base_error, &self.picker) {
                self.warn(crate::tui::sanitize::sanitize(error));
            }
        }
    }
```

with the private fields `seen_mark_seq: u64` and
`seen_rewrite: Option<(String, String, crate::engine::MarkState)>` initialised in `new`.

`src/tui/view.rs`'s `notice` puts an urgent one first:

```rust
pub fn notice(state: &ViewState, snapshot: &Snapshot) -> Option<String> {
    if let Some(notice) = state.notice.as_ref().filter(|n| n.urgent) {
        return Some(notice.text.clone());
    }
    if let (Some(error), false) = (&snapshot.status_error, snapshot.files.is_empty()) {
        return Some(format!(
            "status failed: {} · showing the last good list · r retries",
            sanitize(error)
        ));
    }
    if let Some(reason) = &snapshot.watcher_error {
        return Some(format!(
            "live refresh degraded: {} · polling every 5 s · r retries",
            sanitize(reason)
        ));
    }
    state.notice.as_ref().map(|n| n.text.clone())
}
```

- [ ] **Step 5: The marker and the empty-list wording**

`src/tui/view.rs`, next to `files_lines`:

```rust
/// A row carries a marker when a commit touched it after the mark, or whenever the mark
/// cannot divide this branch's history, in which case nothing can be called read.
fn unread_marker(snapshot: &Snapshot, file: &ChangedFile) -> bool {
    use crate::engine::{MarkState, Scope};
    if snapshot.scope != Scope::Branch || matches!(file.status, ChangedFileStatus::Untracked) {
        return false;
    }
    match snapshot.mark.as_ref().map(|m| &m.state) {
        None => false,
        Some(MarkState::Current) => {
            snapshot.unread.contains(&file.path)
                || snapshot
                    .rename_sources
                    .get(&file.path)
                    .is_some_and(|old| snapshot.unread.contains(old))
        }
        Some(_) => true,
    }
}
```

and the staged column becomes the marker column:

```rust
        line.push(if unread_marker(snapshot, file) {
            Span::new(
                "● ".to_string(),
                Style {
                    semantic: Some(Semantic::Accent),
                    ..Style::role(Role::Emphasis)
                },
            )
        } else {
            Span::body(if file.staged { "S " } else { "  " })
        });
```

The chip says `vs reviewed` while the base in force *is* the mark, which is a comparison
made at render time and not a label the pick remembered. In `toolbar_items`, where 7.4
builds the scope chip:

```rust
    let scope_chip = match (snapshot.scope, &snapshot.base) {
        (Scope::Branch, Some(base)) => {
            let names_mark = snapshot
                .mark
                .as_ref()
                .is_some_and(|m| m.commit == base.requested);
            let label = if names_mark {
                "reviewed".to_string()
            } else {
                truncate(&sanitize(base.label()), 16)
            };
            format!("vs {label}")
        }
        _ => "worktree".to_string(),
    };
```

and the hover hint of 7.4 substitutes the same label, so it reads
`switch scope · b · reviewed @ a1b2c3d`. The hint already formats
`<label> @ <7 hex of base.commit>`; give it the same `names_mark` label rather than
`Base::label()`.

Its test:

```rust
    #[test]
    fn the_chip_says_reviewed_only_while_the_base_is_the_mark() {
        use crate::engine::{Base, BaseSource, Mark, MarkState, Scope};
        let mut snap = snapshot("a.rs", "r1", &[(1, "+")]);
        let commit = "a".repeat(40);
        snap.scope = Scope::Branch;
        snap.base = Some(Base { requested: commit.clone(), commit: commit.clone(), merge_base: Some(commit.clone()), source: BaseSource::Picked });
        snap.mark = Some(Mark { commit: commit.clone(), at: 1, state: MarkState::Current, classified_at: None });
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        st.resize(120, 20);
        st.reconcile(&snap);
        assert!(render(&snap, &st, 120, 24).plain()[0].contains("vs reviewed"));
        // A later mark leaves the base where it was: the alias stops, the id is shown.
        snap.mark.as_mut().unwrap().commit = "b".repeat(40);
        let top = render(&snap, &st, 120, 24).plain()[0].clone();
        assert!(top.contains("vs aaaaaaa"), "{top}");
    }
```

`state_message`'s empty case names the base in branch scope:

```rust
    if snapshot.files.is_empty() {
        if let Some(error) = &snapshot.status_error {
            return Some(sanitize(error));
        }
        return Some(match (snapshot.scope, &snapshot.base) {
            (Scope::Branch, Some(base)) => format!(
                "nothing on this branch since {}",
                truncate(&sanitize(base.label()), 16)
            ),
            _ => "working tree clean".into(),
        });
    }
```

- [ ] **Step 6: The `M` key in input**

`src/tui/input.rs`, in `act`:

```rust
        MarkReviewed => {
            return match state.drawn_head.clone() {
                None => {
                    state.notify("nothing to mark: no commit yet");
                    Outcome::Redraw
                }
                Some(commit) => Outcome::Engine(Command::MarkReviewed(commit)),
            };
        }
```

`apply_action` clears the notice before `act` runs, which is what makes the refusal notice
stick until the next handled body key. Modal paths mostly avoid `apply_action`, but one does
not: `handle_key`'s `help_open` branch sets `state.notice = None` when the sheet closes.
Change that line to keep an urgent one:

```rust
            state.help_open = false;
            state.help_offset = 0;
            // An answer that arrived behind the sheet is read after it closes.
            state.notice = state.notice.take().filter(|n| n.urgent);
            return Outcome::Redraw;
```

and in `apply_action`, clear only what a body key should clear -- an urgent notice is cleared
by the *next* body key rather than the one that was already in flight when it arrived:

```rust
fn apply_action(state: &mut ViewState, snapshot: &Snapshot, action: KeyAction, width: u16) -> Outcome {
    let cleared_notice = state.notice.take().is_some();
    let outcome = act(state, snapshot, action, width);
    if cleared_notice && outcome == Outcome::Inert {
        Outcome::Redraw
    } else {
        outcome
    }
}
```

is already that rule and needs no change; the only edit is the `help_open` branch above.

- [ ] **Step 7: Run everything**

Run the full gate of the Global Constraints.
Expected: all pass, including `every_key_on_the_sheet_does_something_and_reserved_keys_do_nothing` (which now exercises `M`'s refusal notice as a `Redraw`) and the four new tests.

- [ ] **Step 8: Commit (orchestrator)**

```bash
git add -A
git commit -m "feat(tui): mark key, panel markers and ranked notices"
```

---

### Task 5: Quick picker rows, the read-only extension, documentation and version 0.0.3

Implements spec 8.4 in full, the allow-list sentence of 8.6, and 8.7 items 5, 8, 9 with criteria 10-12. Read 8.4, 8.6 and 8.7 before starting.

**Files:**
- Modify: `src/engine/base.rs` (`quick_bases`), `src/engine/types.rs` (`QuickBase`, `Snapshot.quick`), `src/engine/session.rs` (the `LoadRefs` task)
- Modify: `src/tui/picker.rs` (the quick rows, the live `reviewed` row, filtering, cursor reconciliation)
- Modify: `tests/readonly_guarantee.rs`, `tests/e2e_real_herdr.rs`
- Modify: `README.md`, `README.zh-CN.md`, `README.ja.md`, `AGENTS.md`, `docs/acceptance-p1.md`
- Modify: `Cargo.toml`, `Cargo.lock`, `herdr-plugin.toml`

**Interfaces:**
- Consumes: Tasks 2 and 3's `Snapshot.mark` and `MarkState`; 0.0.2's `Picker`, `PickerRow`, `refs_seq`.
- Produces:

```rust
// src/engine/types.rs
pub struct QuickBase { pub label: String, pub detail: String, pub submits: String }
// Snapshot gains: pub quick: Option<Arc<Vec<QuickBase>>>
// src/engine/base.rs
pub(crate) async fn quick_bases(toplevel: &str) -> Vec<QuickBase>;
// src/tui/picker.rs
// PickerRow gains: Quick { label: String, detail: String, submits: String }
```

- [ ] **Step 1: Write the failing engine test**

Append to `src/engine/base.rs`'s tests:

```rust
    #[test]
    fn quick_bases_offer_only_what_resolves() {
        let (dir, top) = repo("main");
        // One commit: no parent, no upstream.
        let quick = rt().block_on(quick_bases(&top));
        assert!(quick.is_empty(), "{quick:?}");

        for n in 2..=4 {
            std::fs::write(dir.path().join("a.txt"), format!("v{n}\n")).unwrap();
            run(dir.path(), &["add", "-A"]);
            run(dir.path(), &["commit", "-q", "-m", &format!("c{n}")]);
        }
        // `--set-upstream-to` needs origin to be a configured remote with a fetch mapping;
        // creating the remote-tracking ref alone is not enough.
        run(dir.path(), &["remote", "add", "origin", "."]);
        run(dir.path(), &["config", "remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*"]);
        run(dir.path(), &["update-ref", "refs/remotes/origin/main", "HEAD~1"]);
        run(dir.path(), &["branch", "--set-upstream-to=origin/main", "main"]);

        let quick = rt().block_on(quick_bases(&top));
        let labels: Vec<&str> = quick.iter().map(|q| q.label.as_str()).collect();
        assert_eq!(labels, ["upstream", "last commit", "last 3 commits"]);
        assert_eq!(quick[0].submits, "refs/remotes/origin/main");
        assert_eq!(quick[0].detail, "origin/main");
        assert_eq!(quick[1].submits, "HEAD~1");
        assert_eq!(quick[2].submits, "HEAD~3");
        assert_eq!(quick[1].detail.len(), 7, "a short id: {:?}", quick[1].detail);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --locked --lib engine::base`
Expected: compile error — `quick_bases` is undefined.

- [ ] **Step 3: Implement `quick_bases`**

In `src/engine/base.rs`:

```rust
/// The picker's computed rows, in display order. Each is independent: a repository with no
/// upstream, no parent or fewer than three commits simply offers fewer.
pub(crate) async fn quick_bases(toplevel: &str) -> Vec<QuickBase> {
    let mut rows = Vec::new();
    if let Ok(output) = git(
        toplevel,
        &["rev-parse", "--symbolic-full-name", "@{upstream}"],
    )
    .await
    {
        if output.status.success() {
            let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !name.is_empty() {
                rows.push(QuickBase {
                    label: "upstream".into(),
                    detail: ref_label(&name).to_string(),
                    submits: name,
                });
            }
        }
    }
    for (label, revision) in [("last commit", "HEAD~1"), ("last 3 commits", "HEAD~3")] {
        if let Ok(id) = verify(toplevel, revision).await {
            rows.push(QuickBase {
                label: label.into(),
                detail: id[..id.len().min(7)].to_string(),
                submits: revision.into(),
            });
        }
    }
    rows
}
```

`QuickBase` goes in `src/engine/types.rs`:

```rust
/// A picker row the engine computed: what it shows and what `SetBase` is given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickBase {
    /// `upstream`, `last commit`, `last 3 commits`.
    pub label: String,
    /// The row's second column: a ref name or a short id.
    pub detail: String,
    pub submits: String,
}
```

`Snapshot` gains `pub quick: Option<Arc<Vec<QuickBase>>>,` next to `refs` (`None` in `empty`, in `fingerprint` by `Arc::as_ptr` like `refs`). The `LoadRefs` task computes both under the same token:

```rust
                        tokio::spawn(async move {
                            let (refs, quick) = match toplevel {
                                Some(toplevel) => (
                                    base::list_refs(&toplevel).await,
                                    base::quick_bases(&toplevel).await,
                                ),
                                None => (Ok((Vec::new(), false)), Vec::new()),
                            };
                            let _ = results.send(Done::Refs { token, result: refs, quick });
                        });
```

`Done::Refs` gains `quick: Vec<QuickBase>`, and its arm publishes `next.quick = Some(Arc::new(quick));` beside `next.refs`, under the same `token <= next.refs_seq` guard.

- [ ] **Step 4: Write the failing picker tests**

`src/tui/picker.rs` tests — the existing `snap` helper gains the new fields:

```rust
    #[test]
    fn quick_rows_sit_between_the_typed_row_and_the_refs() {
        use crate::engine::{Mark, MarkState, QuickBase};
        let mut s = snap(&REFS, None);
        s.quick = Some(std::sync::Arc::new(vec![
            QuickBase { label: "upstream".into(), detail: "origin/main".into(), submits: "refs/remotes/origin/main".into() },
            QuickBase { label: "last commit".into(), detail: "9ffc8fd".into(), submits: "HEAD~1".into() },
        ]));
        s.mark = Some(Mark { commit: "a".repeat(40), at: now() - 7_200, state: MarkState::Current, classified_at: None });
        let p = Picker::open(1); // the token `snap` publishes as refs_seq
        let shown = labels(&p.rows(&s));
        assert_eq!(&shown[0], "default (main)");
        assert_eq!(&shown[1], "reviewed (aaaaaaa · 2 h ago)");
        assert_eq!(&shown[2], "upstream (origin/main)");
        assert_eq!(&shown[3], "last commit (9ffc8fd)");
        assert!(shown[4..].iter().any(|l| l.starts_with("main")));
        assert_eq!(p.rows(&s)[1].submit().as_deref(), Some("a".repeat(40).as_str()));

        // The reviewed row is live: its detail follows the snapshot, not a frozen string.
        s.mark.as_mut().unwrap().state = MarkState::Rewritten;
        assert_eq!(&labels(&p.rows(&s))[1], "reviewed (aaaaaaa · rewritten)");
        s.mark.as_mut().unwrap().state = MarkState::Unreadable("gone".into());
        assert_eq!(&labels(&p.rows(&s))[1], "reviewed (aaaaaaa · unreadable)");
        s.mark.as_mut().unwrap().at = now();
        s.mark.as_mut().unwrap().state = MarkState::Current;
        assert_eq!(&labels(&p.rows(&s))[1], "reviewed (aaaaaaa · just now)");
    }

    #[test]
    fn filtering_matches_a_quick_rows_detail_but_only_the_reviewed_label() {
        use crate::engine::{Mark, MarkState, QuickBase};
        let mut s = snap(&REFS, None);
        s.quick = Some(std::sync::Arc::new(vec![QuickBase {
            label: "upstream".into(), detail: "origin/main".into(), submits: "refs/remotes/origin/main".into(),
        }]));
        s.mark = Some(Mark { commit: "a".repeat(40), at: now(), state: MarkState::Current, classified_at: None });
        let mut p = Picker::open(1);
        p.input = "orig".into();
        assert!(labels(&p.rows(&s)).iter().any(|l| l.starts_with("upstream")));
        p.input = "rev".into();
        assert!(labels(&p.rows(&s)).iter().any(|l| l.starts_with("reviewed")));
        p.input = "just now".into();
        assert!(
            !labels(&p.rows(&s)).iter().any(|l| l.starts_with("reviewed")),
            "the age is not matched, or a passing minute would move the row"
        );
    }

    #[test]
    fn the_cursor_keeps_its_row_when_the_mark_arrives() {
        use crate::engine::{Mark, MarkState, QuickBase};
        let mut s = snap(&REFS, None);
        s.quick = Some(std::sync::Arc::new(vec![QuickBase {
            label: "upstream".into(), detail: "origin/main".into(), submits: "refs/remotes/origin/main".into(),
        }]));
        let mut p = Picker::open(1);
        p.observe(&s); // consume this opening's refs_seq, so the move below is not undone
        p.move_by(1, p.rows(&s).len(), 10); // onto `upstream`
        let before = p.rows(&s)[p.cursor].submit();
        assert_eq!(before.as_deref(), Some("refs/remotes/origin/main"));
        s.mark = Some(Mark { commit: "a".repeat(40), at: now(), state: MarkState::Current, classified_at: None });
        p.observe(&s);
        assert_eq!(p.rows(&s)[p.cursor].submit(), before, "the cursor followed its row");
    }
```

with one helper in that test module:

```rust
    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
```

- [ ] **Step 5: Implement the picker rows**

`PickerRow` gains a variant:

```rust
    /// A computed row: the mark, or one of the engine's three.
    Quick {
        label: String,
        detail: String,
        submits: String,
    },
```

whose `submit()` returns `Some(submits.clone())`. `rows()` inserts them after the typed row and before the refs:

```rust
        let needle = self.input.to_lowercase();
        if let Some(mark) = &snapshot.mark {
            let short = &mark.commit[..mark.commit.len().min(7)];
            let detail = match &mark.state {
                crate::engine::MarkState::Current => age(mark.at),
                crate::engine::MarkState::Rewritten => "rewritten".to_string(),
                crate::engine::MarkState::Unreadable(_) => "unreadable".to_string(),
            };
            // Matched on the label alone: the detail is live, and a passing minute must not
            // add or remove a row under the cursor.
            if "reviewed".contains(&needle) {
                rows.push(PickerRow::Quick {
                    label: "reviewed".into(),
                    detail: format!("{short} · {detail}"),
                    submits: mark.commit.clone(),
                });
            }
        }
        // The same token as the refs: a list from an earlier opening is never shown.
        for quick in self.quick(snapshot) {
            let haystack = format!("{} {}", quick.label, quick.detail).to_lowercase();
            if haystack.contains(&needle) {
                rows.push(PickerRow::Quick {
                    label: quick.label.clone(),
                    detail: quick.detail.clone(),
                    submits: quick.submits.clone(),
                });
            }
        }
```

with the token guard beside `refs`:

The guard is the one `refs()` already uses — `snapshot.refs_seq == self.token`, the token this
opening sent with its `LoadRefs` — not a comparison against an earlier value, and `observe`
keeps its existing `snapshot.refs_seq == self.token && self.seen_refs_seq != self.token`
condition so a reply belonging to another opening never repositions the cursor:

```rust
    /// This opening's quick rows; empty until its own `LoadRefs` is answered.
    pub fn quick<'a>(&self, snapshot: &'a Snapshot) -> &'a [QuickBase] {
        if snapshot.refs_seq == self.token {
            snapshot.quick.as_deref().map(Vec::as_slice).unwrap_or(&[])
        } else {
            &[]
        }
    }
```

Two rules of 0.0.2's `rows()` and `follow_input` extend to the new rows, each one line:

- the typed row is suppressed when the input is exactly the label of a row that is actually
  offered, so `listed` becomes
  `refs.iter().any(|r| ref_label(r) == self.input)
  || (self.input == "reviewed" && snapshot.mark.is_some())
  || self.quick(snapshot).iter().any(|q| q.label == self.input)` — without the
  `mark.is_some()` term, typing `reviewed` in a repository with no mark would suppress the
  typed row while adding no row to pick, leaving the reset row under the cursor;
- `follow_input` puts the cursor on the first row that submits a revision *in display
  order*, which now means `matches!(r, PickerRow::Quick { .. } | PickerRow::Ref { .. })`
  before falling back to the typed row. Without it, typing `reviewed` or `last commit` and
  pressing Enter would submit those words as free text rather than the row's revision.

Their test:

```rust
    #[test]
    fn typing_a_quick_rows_label_submits_that_base() {
        use crate::engine::{Mark, MarkState, QuickBase};
        let mut s = snap(&REFS, None);
        s.quick = Some(std::sync::Arc::new(vec![QuickBase {
            label: "last commit".into(), detail: "9ffc8fd".into(), submits: "HEAD~1".into(),
        }]));
        s.mark = Some(Mark { commit: "a".repeat(40), at: now(), state: MarkState::Current, classified_at: None });
        let mut p = Picker::open(1);
        p.input = "last commit".into();
        p.retarget(&s);
        assert_eq!(p.rows(&s)[p.cursor].submit().as_deref(), Some("HEAD~1"));
        assert!(
            !labels(&p.rows(&s)).iter().any(|l| l.starts_with("use ")),
            "an exact quick label leaves no typed row to pick by mistake"
        );
        p.input = "reviewed".into();
        p.retarget(&s);
        assert_eq!(p.rows(&s)[p.cursor].submit().as_deref(), Some("a".repeat(40).as_str()));

        // With no mark there is no reviewed row, so the typed row must still be offered.
        s.mark = None;
        p.retarget(&s);
        assert!(
            labels(&p.rows(&s)).iter().any(|l| l == "use \"reviewed\""),
            "{:?}",
            labels(&p.rows(&s))
        );
    }
```

and the age helper:

```rust
/// Coarse, computed at every render: `just now`, `N min ago`, `N h ago`, `N d ago`.
/// A mark dated in the future reads `just now`; a mark is never rejected for its time.
pub fn age(at: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let seconds = now.saturating_sub(at);
    match seconds {
        0..=59 => "just now".to_string(),
        60..=3_599 => format!("{} min ago", seconds / 60),
        3_600..=86_399 => format!("{} h ago", seconds / 3_600),
        _ => format!("{} d ago", seconds / 86_400),
    }
}
```

The panel draws a `Quick` row as `label (detail)` in its label column with no marker, exactly as the reset and typed rows are drawn. `observe` reconciles the cursor by identity when the mark's presence or commit changes, or when `base.requested` changes, and keeps 7.4's first-match rule for a new `refs_seq`:

```rust
    pub fn observe(&mut self, snapshot: &Snapshot) {
        let mark_key = snapshot.mark.as_ref().map(|m| m.commit.clone());
        let base_key = snapshot.base.as_ref().map(|b| b.requested.clone());
        if mark_key != self.seen_mark || base_key != self.seen_base {
            let wanted = self.rows_at_cursor_submit.clone();
            self.seen_mark = mark_key;
            self.seen_base = base_key;
            let rows = self.rows(snapshot);
            self.cursor = rows
                .iter()
                .position(|r| r.submit() == wanted)
                .unwrap_or_else(|| rows.len().saturating_sub(1));
        }
        if snapshot.refs_seq == self.token && self.seen_refs_seq != self.token {
            self.seen_refs_seq = snapshot.refs_seq;
            self.follow_input(snapshot);
        }
        if let Some(sent) = self.pending {
            if snapshot.pick_seq > sent {
                self.pending = None;
                match &snapshot.pick_error {
                    Some(error) => self.error = Some(error.clone()),
                    None => self.done = true,
                }
            }
        }
    }
```

with `seen_mark: Option<String>`, `seen_base: Option<String>` and `rows_at_cursor_submit: Option<String>` on `Picker`; the last is refreshed whenever the cursor moves (`move_by`, `follow_input`, a click) by reading `rows(snapshot)[cursor].submit()`. Give `move_by` the snapshot so it can, or record it in `panel()` — either is fine, but the value must be the *submitted text*, not the index.

- [ ] **Step 6: Extend the read-only test**

`tests/readonly_guarantee.rs`: after the branch-scope drive of 0.0.2 and before the harness's HEAD switch, mark and reopen the picker:

```rust
    // Mark the head, then open the picker once: both must stay inside the allow-list.
    let head = {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut id = None;
        while Instant::now() < deadline && id.is_none() {
            if let Ok(s) = handle.snapshots.recv_timeout(Duration::from_millis(200)) {
                id = s.head.clone();
            }
        }
        id.expect("a markable head")
    };
    handle.commands.send(Command::MarkReviewed(head)).unwrap();
    // A token beyond the one 0.0.2's drive already used, so this is a fresh answer.
    handle.commands.send(Command::LoadRefs(2)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let (mut marked, mut refs) = (false, false);
    while Instant::now() < deadline && !(marked && refs) {
        let Ok(s) = handle.snapshots.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        marked |= s.mark_seq == 1 && s.mark_error.is_none();
        refs |= s.refs_seq == 2 && s.quick.is_some();
    }
    assert!(marked, "the mark was never answered");
    assert!(refs, "the quick rows never arrived for this opening");
```

and the state-directory assertion accepts the new file:

```rust
    assert_eq!(
        written,
        ["bases.json", "marks.json", "split-panes.lock"],
        "the viewer wrote something else"
    );
```

Run: `cargo test --locked --test readonly_guarantee -- --nocapture`
Expected: PASS, with the allow-list unchanged at ten subcommands.

- [ ] **Step 7: Extend the tier B test**

`tests/e2e_real_herdr.rs`, after the existing `b` and `n` steps:

```rust
        // The body must be loaded before marking: the file name appears in the toolbar while
        // the diff is still `Loading`, and a mark taken then acknowledges nothing.
        iso.herdr(&[
            "pane", "wait-output", viewer_id, "--match", "@@", "--source", "visible",
            "--timeout", "15000",
        ]);
        iso.herdr(&["pane", "send-text", viewer_id, "M"]);
        iso.herdr(&[
            "pane", "wait-output", viewer_id, "--match", "as reviewed", "--source", "visible",
            "--timeout", "15000",
        ]);
        // The dot is drawn in the files panel, which the viewer leaves hidden below 100
        // columns -- and a split pane in the isolated host is often narrower than that. `E`
        // pins it; press until the panel's own header is on screen, so a hidden panel fails
        // as "no panel" rather than as "no marker".
        wait_for("the files panel is shown", || {
            let screen =
                iso.herdr_text(&["pane", "read", viewer_id, "--source", "visible"]);
            if screen.contains("CHANGED ") {
                return true;
            }
            iso.herdr(&["pane", "send-text", viewer_id, "E"]);
            false
        });

        // A commit after the mark must raise a marker, and the next mark must clear it. It
        // touches the selected file so the new diff is the one already on screen.
        std::fs::write(repo.join("c.txt"), "c\nAFTER-THE-MARK\n").unwrap();
        git(&["add", "c.txt"]);
        git(&["commit", "-q", "-m", "later"]);
        iso.herdr(&[
            "pane", "wait-output", viewer_id, "--match", "●", "--source", "visible",
            "--timeout", "15000",
        ]);
        // The dot can appear while the previous commit's diff is still drawn, whose id is the
        // one `M` would take. This line exists only in the new commit, so seeing it proves
        // the body on screen was read at the head that commit created.
        iso.herdr(&[
            "pane", "wait-output", viewer_id, "--match", "AFTER-THE-MARK", "--source",
            "visible", "--timeout", "15000",
        ]);
        iso.herdr(&["pane", "send-text", viewer_id, "M"]);
        wait_for("the marker clears", || {
            let screen =
                iso.herdr_text(&["pane", "read", viewer_id, "--source", "visible"]);
            !screen.contains('●')
        });

`pane read` prints the pane's own text rather than a JSON envelope, so it cannot go through the
`iso.herdr` helper, which parses stdout as JSON and panics otherwise. Give `Isolated` a second
helper beside it:

```rust
    /// `pane read` prints the pane's own text, not a JSON envelope.
    fn herdr_text(&self, args: &[&str]) -> String {
        let out = self
            .host_command()
            .args(args)
            .output()
            .expect("run isolated host");
        assert!(
            out.status.success(),
            "host {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
```
```

Waiting for the notice alone would pass with marker clearing broken, which is what 8.7 item 9
asks about; the sequence above sees a marker appear and go. The test stays `#[ignore]`; the
orchestrator runs it against both hosts.

- [ ] **Step 8: Documentation, in three languages**

`README.md`: the key table gains `M` — `Mark the current commit as reviewed`; the "Keys" list of `Esc` is unchanged. Add a section `## Review marks` after `## Branch scope`:

```markdown
## Review marks

In branch scope, `M` records the commit you have read up to for this worktree.
Every row a later commit touches then carries `●` in the files panel, and `M`
again clears them. The dots count commits, not edits: uncommitted work raises
none, so `M` always clears every dot, and the edits themselves are in the row's
diff as before. Untracked rows never carry one.

The mark is kept in `marks.json` beside `bases.json` in the state directory, one
commit per worktree, so it survives closing the viewer and is shared with a
second viewer on the same worktree. It changes no comparison: a mark that is
rewritten or pruned flags every row and says so, and the rows, diffs and base
keep working.

`B` offers the mark as a base — `reviewed (a1b2c3d · 12 min ago)` — and
choosing it narrows the list to what the working tree differs from the mark by.
The same picker now lists `upstream`, `last commit` and `last 3 commits` when
they resolve, so the common bases need no typing.
```

`README.zh-CN.md` and `README.ja.md` mirror it section for section (translate; keep `M`, `●`, `marks.json`, `bases.json`, `reviewed`, `upstream`, `last commit`, `last 3 commits` verbatim). The `--version` line in all three becomes `herdr-hunks 0.0.3`.

`AGENTS.md`: one sentence — `M` marks a commit as reviewed and writes only `marks.json`; the allow-list is unchanged at ten subcommands.

- [ ] **Step 9: Acceptance row 8 and the version**

`docs/acceptance-p1.md`: change the first line to `Status: PENDING` and add:

```markdown
| 8. Review marks | On a branch where an agent has committed, press `b`, read the rows and press `M`: every `●` clears. Have the agent commit again and compare the marked rows with `git diff <marked commit> HEAD --name-only --no-renames`. Edit a file without committing, press `M` again. Close and reopen the viewer. | The dots clear on `M`; after the new commit exactly that command's paths carry one; an uncommitted edit carries none and `M` still clears everything; the mark survives reopening. | pending | | | |
```

and extend the prose under the table to say rows 1-7 are the earlier releases' and row 8 is 0.0.3's. Set `version = "0.0.3"` in `Cargo.toml` and `herdr-plugin.toml`, run `cargo check` once without `--locked` so `Cargo.lock` records it, and confirm `./target/release/herdr-hunks --version` prints `herdr-hunks 0.0.3` after `cargo build --release --locked`.

- [ ] **Step 10: Run everything**

Run the full gate of the Global Constraints plus `cargo build --release --locked`.
Expected: all pass.

- [ ] **Step 11: Commit (orchestrator)**

```bash
git add -A
git commit -m "feat: quick picker bases, docs and version 0.0.3"
```

---

## Self-review against the spec

- 8.1: the dots are the default and narrowing is optional — Task 3 (the set), Task 4 (the marker), Task 5 (the `reviewed` row).
- 8.2: head sampled first and used in the commands, the id carried by `LoadedDiff`, `drawn_head` only for body frames, the boundary (Task 1); `MarkReviewed`'s four steps, `marks.json`, the session mark, `mark_seq`/`mark_error`, `at` at the write, the shape check on stored ids (Task 2); `head_seen` (Task 1) and the mark on the snapshot (Tasks 2, 3); the urgent notice rank (Task 4).
- 8.3: the `--name-only --no-renames` command between two commits, untracked rows excluded, the rename-source rule, the `(mark, head_seen)` classification with retries and the three states (Task 3); the row marker's use of `rename_sources` (Task 4).
- 8.4: the four quick rows, the live `reviewed` row, label-only matching for it, the cursor rules, the `refs_seq` token (Task 5).
- 8.5: the marker column, `M`, the key sheet's 24 rows, the chip's `vs reviewed` alias and its hover hint, the empty-list wording (all Task 4; the alias is a comparison made at render time, so it also stops by itself when a later `M` moves the mark away from the base in force).
- 8.6: every failure row has a home — nothing drawn (Task 4's refusal), an id that will not validate (Task 2), a malformed or unwritable `marks.json` (Task 2), `Rewritten` and `Unreadable` (Task 3), an unanswerable classification (Task 3's retry), the head `rev-parse` failing or finding nothing (Task 1), a diff that fails or spans a move (Task 1), a quick row that does not resolve (Task 5); the cost paragraph is the commands Tasks 1, 3 and 5 add; no new config key; version 0.0.3 (Task 5).
- 8.7: tests 1-2 in Task 2, 3 in Task 1, 4 in Task 3, 5 in Task 5, 6-7 in Task 4, 8-9 in Task 5; criteria 10-12 in Task 5's acceptance row.

Type consistency: `read_head`, `merge_base_of`, `is_object_id`, `MarkRecord`, `load_marks`, `save_mark`, `quick_bases`, `marks::{unread, is_ancestor, classify}`, `Mark { commit, at, state, classified_at }`, `MarkState::{Current, Rewritten, Unreadable}`, `QuickBase { label, detail, submits }`, `Snapshot.{head, head_seen, mark, unread, quick, mark_seq, mark_error}`, `LoadedDiff.read_at`, `Command::MarkReviewed`, `KeyAction::MarkReviewed`, `Notice { text, urgent }`, `ViewState.{drawn_head, notify, warn}`, `PickerRow::Quick`, `picker::age` are the names used throughout.

One deliberate deviation from the spec's wording, recorded here rather than left implicit: 8.3 says a `Rewritten` or `Unreadable` mark makes the engine flag every row. The engine publishes an *empty* `unread` set in those states and the view flags every row from `Mark::state` (Task 3's `classify`, Task 4's `unread_marker`). The observable behaviour is the spec's; putting the rule in one place keeps the set's meaning single ("paths the diff named") and matches 8.3's own instruction that the view distinguishes the states by `Mark::state`, never by the size of the set.

<!-- codex-reviewed: 2026-09-23T17:58:54Z -->
