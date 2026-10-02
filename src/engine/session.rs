//! Non-blocking repository sessions and immutable snapshots.
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::Semaphore;

use super::base::{self, MarkRecord, ResolveInputs};
use super::{
    actions, branch, gitver, marks, worktree, Action, Base, BaseSource, Command, Comparison,
    DiffState, FileKey, LoadedDiff, Mark, MarkState, PreImage, QuickBase, RepoState, Scope,
    Snapshot, NO_BASE_NOTICE,
};
use crate::git::{self, ChangedFile, GetGitDiffResponse, GitStatusResponse};
use crate::runtime::EventSink;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub type BoxFut<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>;

pub trait WatcherControl: Send + Sync + 'static {
    fn start(&self, cwd: String, sink: Arc<dyn EventSink>) -> BoxFut<Result<(), String>>;
    fn stop(&self, cwd: String) -> BoxFut<Result<(), String>>;
}

pub struct SessionConfig {
    /// `[view] scope`: the scope loaded first; branch falls back to worktree when no base resolves.
    pub scope: Scope,
    /// `[base] ref`, step 2 of the resolution order.
    pub base_ref: Option<String>,
    /// Where `bases.json` lives; `None` keeps picks for the session only.
    pub state_dir: Option<PathBuf>,
    pub path: PathBuf,
    pub poll_interval: Duration,
    pub watcher: Arc<dyn WatcherControl>,
    /// Injected so tests never change the process PATH.
    pub git_check: Arc<dyn Fn() -> Result<gitver::GitVersion, gitver::GitCheckError> + Send + Sync>,
    pub diff_delay: Option<Duration>,
    /// Test hook: each diff consumes one permit before running git.
    pub diff_gate: Option<Arc<Semaphore>>,
    /// Test seam: a refresh sleeps this long before its status read; `None` in production.
    pub status_delay: Option<Duration>,
}

pub struct EngineHandle {
    pub commands: UnboundedSender<Command>,
    pub snapshots: mpsc::Receiver<Arc<Snapshot>>,
    /// Status refreshes started by this session.
    pub refreshes: Arc<AtomicUsize>,
    /// Test hook: ref replies handled, including discarded replies.
    pub refs_answered: Arc<AtomicUsize>,
    /// Test hook: diff results discarded for a stale generation or comparison.
    pub diffs_discarded: Arc<AtomicUsize>,
    /// Test hook: opening head samples taken by diff tasks.
    pub head_samples: Arc<AtomicUsize>,
    /// Test hook: pre-image readings taken by diff tasks, two per worktree-scope diff.
    pub pre_images: Arc<AtomicUsize>,
}

struct FrozenWatcher {
    state: git::watcher::GitWatcherState,
}

impl WatcherControl for FrozenWatcher {
    fn start(&self, cwd: String, sink: Arc<dyn EventSink>) -> BoxFut<Result<(), String>> {
        Box::pin(git::watcher::start_git_watcher_backend(
            cwd,
            sink,
            self.state.clone(),
        ))
    }

    fn stop(&self, cwd: String) -> BoxFut<Result<(), String>> {
        Box::pin(git::watcher::stop_git_watcher_backend(
            cwd,
            self.state.clone(),
        ))
    }
}

impl SessionConfig {
    pub fn production(path: PathBuf) -> Self {
        Self {
            scope: Scope::Worktree,
            base_ref: None,
            state_dir: None,
            path,
            poll_interval: Duration::from_secs(5),
            watcher: Arc::new(FrozenWatcher {
                state: git::watcher::GitWatcherState::new(),
            }),
            git_check: Arc::new(gitver::check),
            diff_delay: None,
            diff_gate: None,
            status_delay: None,
        }
    }
}

enum Trigger {
    Status,
    Head,
}

struct ChannelSink {
    tx: UnboundedSender<Trigger>,
}

impl EventSink for ChannelSink {
    fn emit_json(&self, event: &str, _payload: serde_json::Value) -> Result<(), String> {
        let trigger = match event {
            "git-status-changed" => Trigger::Status,
            "git-head-changed" => Trigger::Head,
            _ => return Ok(()),
        };
        self.tx.send(trigger).map_err(|e| e.to_string())
    }
}

enum WatcherPhase {
    Starting,
    Running,
    Failed,
}

/// A queued scope or base change; applied by the next refresh and published with its rows.
#[derive(Debug, Clone)]
enum Change {
    Scope(Scope),
    Base(Option<String>),
    Mark(String),
    Act(ActJob),
}

/// A planned action: its form and patch are decided when the command arrives, the pre-image when
/// the refresh runs it.
#[derive(Debug, Clone)]
struct ActJob {
    action: Action,
    form: actions::Form,
    patch: Vec<u8>,
    /// Decided when the refresh popped it: the Arc was replaced in the meantime.
    refusal: Option<String>,
}

/// How the refresh obtains the base: keep and re-verify, run the resolution steps, or verify a pick
/// (`Pick(None)` is the reset row: forget the remembered pick and resolve from step 2).
enum BaseJob {
    Keep(Option<Base>),
    Resolve,
    Pick(Option<String>),
}

/// One refresh: the Phase 1 status, then the rows of `scope` under the base `base` yields.
struct Job {
    cwd: String,
    with_head: bool,
    known_branch: Option<String>,
    scope: Scope,
    base: BaseJob,
    inputs: ResolveInputs,
    default_base: Option<String>,
    previous_mark: Option<Mark>,
    previous_unread: Option<(String, BTreeSet<String>)>,
    change: Option<Change>,
    status_delay: Option<Duration>,
    /// The repository the forms run in; `None` answers an action with `not a git repository`.
    toplevel: Option<String>,
    /// K6's lane: no diff task reads git while a form runs.
    lane: Arc<Semaphore>,
}

type Marked = (Mark, BTreeSet<String>, Result<(), String>, Option<String>);

/// What a refresh loaded, published only as a whole.
struct Loaded {
    scope: Scope,
    base: Option<Base>,
    default_base: Option<String>,
    mark: Option<Mark>,
    unread: BTreeSet<String>,
    unread_at: Option<String>,
    base_error: Option<String>,
    files: Vec<ChangedFile>,
    rename_sources: BTreeMap<String, String>,
    /// Worktree scope's renames by side, (path, staged) -> source; empty in branch scope.
    worktree_renames: BTreeMap<(String, bool), String>,
    /// `Some` after a pick or reset: whether `bases.json` took it.
    persisted: Option<Result<(), String>>,
    /// The mark answered by this refresh and whether it was remembered.
    marked: Option<Marked>,
}

struct State {
    snapshot: Snapshot,
    branch: Option<String>,
    worktree: Option<String>,
    last_watcher_refresh: Instant,
    watcher: WatcherPhase,
    git_missing: bool,
    status_in_flight: bool,
    status_dirty: bool,
    status_dirty_head: bool,
    diff_generation: u64,
    diff_in_flight: Option<(u64, FileKey, Comparison)>,
    diff_dirty: bool,
    /// The scope the next refresh loads; equals the published scope except before the first rows.
    requested_scope: Scope,
    inputs: ResolveInputs,
    session_mark: Option<MarkRecord>,
    unread_at: Option<String>,
    /// Run the resolution steps in the next refresh (start, `r`, and after a pick).
    resolve_pending: bool,
    changes: VecDeque<Change>,
    /// The mark a refresh is carrying right now, so key repeat cannot queue it again.
    in_flight_mark: Option<String>,
    /// The commit the published row list was loaded at; `None` before the first rows.
    rows_at: Option<String>,
    /// The refresh in flight was asked to run the resolution steps.
    in_flight_resolve: bool,
    pick_seq: u64,
    mark_seq: u64,
    /// The side map the diff task looks a worktree rename up in.
    worktree_renames: BTreeMap<(String, bool), String>,
    /// K6: diff tasks run git one at a time.
    diff_lane: Arc<Semaphore>,
    /// The newest generation, read by waiting tasks to skip superseded work.
    latest_generation: Arc<AtomicU64>,
    status_delay: Option<Duration>,
    /// An action is queued or being carried; a second one is answered at once.
    act_pending: bool,
    /// The Arc a form ran on: never eligible again until a later diff replaces it.
    acted: Option<Arc<LoadedDiff>>,
    /// The first diff after an attempted action skips the unchanged shortcut.
    fresh_arc_pending: bool,
    action_seq: u64,
}

fn comparison_of(snapshot: &Snapshot) -> Comparison {
    match (snapshot.scope, &snapshot.base) {
        (
            Scope::Branch,
            Some(Base {
                merge_base: Some(m),
                ..
            }),
        ) => Comparison::Branch {
            merge_base: m.clone(),
        },
        _ => Comparison::Worktree,
    }
}

/// The diff's commit, the confirmed commit for an empty list, or no loaded content.
fn head_of(snapshot: &Snapshot, confirmed: Option<String>) -> Option<String> {
    match &snapshot.diff {
        DiffState::Ready(d) => d.read_at.clone(),
        DiffState::Idle if snapshot.files.is_empty() => confirmed,
        _ => None,
    }
}

/// The commit this publication may be marked at.
fn markable(
    next: &Snapshot,
    previous_head: Option<String>,
    head_sampled: bool,
    confirmed: Option<String>,
    rows_at: Option<&str>,
) -> Option<String> {
    let id = if !head_sampled {
        // A sample that could not run keeps the previous id for an empty list.
        head_of(next, previous_head)
    } else {
        // A successful sample with no commit invalidates even a retained diff's id.
        next.head_seen.as_ref()?;
        head_of(next, confirmed)
    };
    // Both surfaces must answer for the same commit. A diff read after a commit the row list
    // has not seen yet would otherwise let `M` acknowledge files that are not on screen, and
    // they would arrive with no dot; a diff older than the rows cannot vouch for them either.
    id.filter(|id| rows_at == Some(id.as_str()))
}

enum Done {
    GitCheck(Result<gitver::GitVersion, gitver::GitCheckError>),
    Watcher(Result<(), String>),
    Status {
        response: Result<GitStatusResponse, String>,
        head: Option<(Option<String>, Option<String>)>,
        head_seen: Option<String>,
        head_sampled: bool,
        confirmed: Option<String>,
        /// `None` when the status failed or the directory is not a repository.
        loaded: Option<Result<Box<Loaded>, String>>,
        change: Option<Change>,
        /// The carried action's result and whether a form ran; `None` without an action.
        acted: Option<(Result<(), String>, bool)>,
    },
    Diff {
        generation: u64,
        key: FileKey,
        comparison: Comparison,
        read_at: Option<String>,
        pre_image: Option<PreImage>,
        result: Result<(GetGitDiffResponse, Vec<u8>), String>,
    },
    Refs {
        token: u64,
        result: Result<(Vec<String>, bool), String>,
        quick: Vec<QuickBase>,
    },
}

/// The rows of one refresh. `Err` means the requested comparison could not be loaded; the
/// caller keeps what it published.
async fn load_rows(
    job: &Job,
    status: &GitStatusResponse,
    head: Option<&(Option<String>, Option<String>)>,
    head_id: Option<&str>,
) -> Result<Loaded, String> {
    let toplevel = status.repo_root.as_str();
    let branch_changed = matches!(head, Some((Some(b), _)) if Some(b) != job.known_branch.as_ref());
    let mut base_error = None;
    let mut default_base = job.default_base.clone();
    let mut base = match &job.base {
        BaseJob::Keep(base) if !branch_changed => base.clone(),
        BaseJob::Keep(_) | BaseJob::Resolve => {
            let r = base::resolve(toplevel, &job.inputs).await;
            base_error = r.skipped.first().cloned();
            default_base = r.default;
            r.base
        }
        BaseJob::Pick(None) => {
            let inputs = ResolveInputs {
                session_pick: Some(None),
                ..job.inputs.clone()
            };
            let r = base::resolve(toplevel, &inputs).await;
            base_error = r.skipped.first().cloned();
            default_base = r.default;
            r.base
        }
        BaseJob::Pick(Some(text)) => {
            let commit = base::verify(toplevel, text).await?;
            let inputs = ResolveInputs {
                session_pick: Some(None),
                ..job.inputs.clone()
            };
            default_base = base::resolve(toplevel, &inputs).await.default;
            Some(Base {
                requested: text.clone(),
                commit,
                merge_base: None,
                source: BaseSource::Picked,
            })
        }
    };
    if job.scope == Scope::Branch {
        if let Some(b) = base.as_mut() {
            // Re-verified every refresh in branch scope, so a moved ref changes rows and diffs together.
            if matches!(job.base, BaseJob::Keep(_)) {
                b.commit = base::verify(toplevel, &b.requested).await?;
            }
            let head = head_id.ok_or_else(|| "no commit yet".to_string())?;
            b.merge_base = Some(base::merge_base_of(toplevel, head, &b.commit).await?);
        }
    }
    let scope = if base.is_some() {
        job.scope
    } else {
        Scope::Worktree
    };
    if job.scope == Scope::Branch && base.is_none() {
        base_error = base_error.or_else(|| Some(NO_BASE_NOTICE.to_string()));
    }
    let (mut files, rename_sources, worktree_renames) = match (scope, &base) {
        (Scope::Branch, Some(b)) => {
            let rows = branch::rows(
                toplevel,
                b.merge_base.as_deref().unwrap_or_default(),
                status.files.clone(),
            )
            .await?;
            (rows.files, rows.rename_sources, BTreeMap::new())
        }
        _ => {
            let sides = worktree::rename_sources(toplevel).await;
            // The panel shows the source on both rows of an RM path; the diff task asks by side.
            let display = sides
                .iter()
                .map(|((path, _), old)| (path.clone(), old.clone()))
                .collect();
            (status.files.clone(), display, sides)
        }
    };
    // Selection relies on unique keys; paths that decode alike keep only their first row.
    let mut seen = std::collections::HashSet::new();
    files.retain(|f| seen.insert(FileKey::of(f)));
    let persisted = match (&job.base, &job.inputs.state_dir) {
        (BaseJob::Pick(pick), Some(dir)) => {
            Some(base::save_pick(dir, toplevel, pick.as_deref()).map_err(|e| e.to_string()))
        }
        (BaseJob::Pick(_), None) => Some(Err("no state directory".to_string())),
        _ => None,
    };
    let marked = match &job.change {
        Some(Change::Mark(commit)) => {
            let id = base::verify(toplevel, commit).await.map_err(|_| {
                format!(
                    "not a commit: {}",
                    commit.chars().take(7).collect::<String>()
                )
            })?;
            let at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let record = MarkRecord { commit: id, at };
            let written = match &job.inputs.state_dir {
                Some(dir) => base::save_mark(dir, toplevel, &record).map_err(|e| e.to_string()),
                None => Err("no state directory".to_string()),
            };
            let (mark, set, read_at) = match head_id {
                Some(head) if scope == Scope::Branch => {
                    let (mark, set) = marks::classify(toplevel, &record, head, None, None).await;
                    (mark, set, Some(head.to_string()))
                }
                _ => (
                    Mark {
                        commit: record.commit,
                        at: record.at,
                        state: MarkState::Current,
                        classified_at: None,
                    },
                    BTreeSet::new(),
                    None,
                ),
            };
            Some((mark, set, written, read_at))
        }
        _ => None,
    };
    let (mark, unread, unread_at) = if let Some((mark, set, _, read_at)) = &marked {
        (Some(mark.clone()), set.clone(), read_at.clone())
    } else {
        let mark_record = if matches!(job.base, BaseJob::Keep(_)) && !branch_changed {
            job.previous_mark.as_ref().map(|mark| MarkRecord {
                commit: mark.commit.clone(),
                at: mark.at,
            })
        } else {
            match (&job.inputs.session_mark, &job.inputs.state_dir) {
                (Some(record), _) => Some(record.clone()),
                (None, Some(dir)) => {
                    let (marks, problem) = base::load_marks(dir);
                    if let Some(problem) = problem {
                        base::note_problem(dir, &problem);
                    }
                    marks.get(toplevel).cloned()
                }
                (None, None) => None,
            }
        };
        match (&mark_record, head_id) {
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
                (Some(mark), set, Some(head.to_string()))
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
                None,
            ),
            (None, _) => (None, BTreeSet::new(), None),
        }
    };
    Ok(Loaded {
        scope,
        base,
        default_base,
        mark,
        unread,
        unread_at,
        base_error,
        files,
        rename_sources,
        worktree_renames,
        persisted,
        marked,
    })
}

async fn run_job(job: Job) -> Done {
    // The forms run before the status read, so the rows published with the answer are git's after it.
    let acted = match (&job.change, &job.toplevel) {
        (Some(Change::Act(act)), Some(toplevel)) => Some(match &act.refusal {
            Some(refusal) => (Err(refusal.clone()), false),
            None => {
                // No diff task reads git while a form runs; one started earlier was superseded at the pop.
                let _lane = job.lane.acquire().await.expect("diff lane closed");
                actions::run(toplevel, &act.action, act.form, &act.patch).await
            }
        }),
        (Some(Change::Act(_)), None) => Some((Err("not a git repository".to_string()), false)),
        _ => None,
    };
    if let Some(delay) = job.status_delay {
        tokio::time::sleep(delay).await;
    }
    let sampled = base::read_head(&job.cwd).await;
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
    let sampled_head = match &sampled {
        Ok(id) => id.clone(),
        Err(_) => None,
    };
    let loaded = match &response {
        Ok(status) if !status.repo_root.is_empty() => Some(
            load_rows(&job, status, head.as_ref(), sampled_head.as_deref())
                .await
                .map(Box::new),
        ),
        _ => None,
    };
    // Only agreeing samples can make an empty list markable.
    let confirmed = match (&sampled, base::read_head(&job.cwd).await) {
        (Ok(first), Ok(second)) if *first == second => first.clone(),
        _ => None,
    };
    Done::Status {
        response,
        head,
        head_seen: sampled_head,
        head_sampled: sampled.is_ok(),
        confirmed,
        loaded,
        change: job.change,
        acted,
    }
}

impl State {
    /// One refresh: status, head when asked, and the rows of the current or requested comparison.
    fn request_status(
        &mut self,
        cwd: &str,
        with_head: bool,
        results: &UnboundedSender<Done>,
        refreshes: &AtomicUsize,
    ) {
        if self.status_in_flight {
            self.status_dirty = true;
            self.status_dirty_head |= with_head;
            return;
        }
        self.status_in_flight = true;
        refreshes.fetch_add(1, Ordering::SeqCst);
        let change = self.changes.pop_front();
        if let Some(Change::Mark(commit)) = &change {
            self.in_flight_mark = Some(commit.clone());
        }
        let change = match change {
            Some(Change::Act(mut job)) => {
                // A diff result may have replaced the Arc while the action waited in the queue.
                let same = matches!(&self.snapshot.diff, DiffState::Ready(d) if Arc::ptr_eq(d, &job.action.diff));
                if !same {
                    job.refusal = Some(actions::NOTICE_CHANGED.to_string());
                }
                Some(Change::Act(job))
            }
            other => other,
        };
        if let Some(Change::Act(_)) = &change {
            // Nothing read before the mutation may be published after it: superseded now, before any form runs.
            self.diff_generation += 1;
            self.latest_generation
                .store(self.diff_generation, Ordering::SeqCst);
            self.diff_in_flight = None;
        }
        let scope = match &change {
            Some(Change::Scope(scope)) => *scope,
            Some(Change::Base(_)) => Scope::Branch,
            Some(Change::Mark(_)) | Some(Change::Act(_)) | None => self.requested_scope,
        };
        // Explicit changes resolve preferences; polls only re-verify ids.
        let base = match (&change, std::mem::take(&mut self.resolve_pending)) {
            (Some(Change::Base(pick)), _) => BaseJob::Pick(pick.clone()),
            // A mark or an action changes no comparison, but must not swallow a resolution `r` asked for.
            (Some(Change::Scope(_)), _)
            | (Some(Change::Mark(_)), true)
            | (Some(Change::Act(_)), true)
            | (None, true) => BaseJob::Resolve,
            (Some(Change::Mark(_)), false) | (Some(Change::Act(_)), false) | (None, false) => {
                BaseJob::Keep(self.snapshot.base.clone())
            }
        };
        self.in_flight_resolve = matches!(base, BaseJob::Resolve);
        let job = Job {
            cwd: cwd.to_string(),
            with_head,
            known_branch: self.branch.clone(),
            scope,
            base,
            inputs: ResolveInputs {
                session_mark: self.session_mark.clone(),
                ..self.inputs.clone()
            },
            default_base: self.snapshot.default_base.clone(),
            previous_mark: self.snapshot.mark.clone(),
            previous_unread: self
                .unread_at
                .clone()
                .map(|at| (at, (*self.snapshot.unread).clone())),
            change,
            status_delay: self.status_delay,
            toplevel: match &self.snapshot.repo {
                RepoState::Repo { toplevel, .. } => Some(toplevel.clone()),
                _ => None,
            },
            lane: self.diff_lane.clone(),
        };
        let results = results.clone();
        tokio::spawn(async move {
            let _ = results.send(run_job(job).await);
        });
    }

    // The test hooks ride along as parameters, the way `head_samples` already did.
    #[allow(clippy::too_many_arguments)]
    fn request_diff(
        &mut self,
        key: FileKey,
        cwd: &str,
        delay: Option<Duration>,
        gate: Option<Arc<Semaphore>>,
        results: &UnboundedSender<Done>,
        head_samples: &Arc<AtomicUsize>,
        pre_images: &Arc<AtomicUsize>,
        discarded: &Arc<AtomicUsize>,
    ) {
        let comparison = comparison_of(&self.snapshot);
        // While an action is queued or carried, nothing reads git for a diff: a task spawned now
        // could read pre-mutation content and be published after the answer. The carrying refresh
        // requests the selected row's diff when it completes, the newest selection included.
        if self.act_pending {
            return;
        }
        if matches!(&self.diff_in_flight, Some((_, pending, under)) if pending == &key && under == &comparison)
        {
            self.diff_dirty = true;
            return;
        }
        self.diff_generation += 1;
        let generation = self.diff_generation;
        self.latest_generation
            .store(self.diff_generation, Ordering::SeqCst);
        self.diff_in_flight = Some((generation, key.clone(), comparison.clone()));
        self.diff_dirty = false;
        let toplevel = match &self.snapshot.repo {
            RepoState::Repo { toplevel, .. } => toplevel.clone(),
            _ => cwd.to_string(),
        };
        let old = match &comparison {
            Comparison::Worktree => self
                .worktree_renames
                .get(&(key.path.clone(), key.staged))
                .cloned(),
            Comparison::Branch { .. } => self.snapshot.rename_sources.get(&key.path).cloned(),
        };
        let results = results.clone();
        let head_samples = head_samples.clone();
        let pre_images = pre_images.clone();
        let discarded = discarded.clone();
        let lane = self.diff_lane.clone();
        let latest = self.latest_generation.clone();
        tokio::spawn(async move {
            // One git at a time; a request superseded while it waited never spawns one.
            let _lane = lane.acquire().await.expect("diff lane closed");
            if latest.load(Ordering::SeqCst) != generation {
                discarded.fetch_add(1, Ordering::SeqCst);
                return;
            }
            // Open the bracket under the lane and before any delay or gate.
            let before = base::read_head(&toplevel).await;
            head_samples.fetch_add(1, Ordering::SeqCst);
            // The first pre-image reading precedes the delay and the gate, so the two readings
            // bracket everything that can wait; an edit landing in between leaves no pre-image.
            let pre = match &comparison {
                Comparison::Worktree => worktree::pre_image(&toplevel, &key.path, old.as_deref())
                    .await
                    .ok(),
                Comparison::Branch { .. } => None,
            };
            pre_images.fetch_add(1, Ordering::SeqCst);
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
                Comparison::Branch { .. } => worktree::untracked_diff(&toplevel, &key.path).await,
                Comparison::Worktree => worktree::diff(&toplevel, &key, old.as_deref()).await,
            };
            let post = match &comparison {
                Comparison::Worktree => worktree::pre_image(&toplevel, &key.path, old.as_deref())
                    .await
                    .ok(),
                Comparison::Branch { .. } => None,
            };
            pre_images.fetch_add(1, Ordering::SeqCst);
            // Kept only when both readings agree; a disagreement means an edit landed mid-read.
            let pre_image = match (pre, post) {
                (Some(a), Some(b)) if a == b => Some(a),
                _ => None,
            };
            // Equal endpoints, not a proof of stillness: HEAD could have gone A -> B -> A
            // within this read. The bracket is kept as is because that window errs the safe
            // way -- the id is then older than the content, and a mark that is too old only
            // shows dots for changes the reader has already seen. The dangerous direction,
            // an id newer than the content, is what the samples do rule out.
            let read_at = match (before, base::read_head(&toplevel).await) {
                (Ok(Some(a)), Ok(Some(b))) if a == b => Some(a),
                _ => None,
            };
            let _ = results.send(Done::Diff {
                generation,
                key,
                comparison,
                read_at,
                pre_image,
                result,
            });
        });
    }
}

fn fingerprint(s: &Snapshot) -> String {
    let diff = match &s.diff {
        DiffState::Idle => "idle".to_string(),
        DiffState::Loading => "loading".to_string(),
        DiffState::Failed(e) => format!("failed:{e}"),
        DiffState::Ready(d) => format!("ready:{:p}", Arc::as_ptr(d)),
    };
    format!(
        "{:?}|{}|{:?}|{}|{:?}|{:?}|{}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{}|{}|{}|{:?}|{:?}|{}|{:?}|{:?}|{:?}|{}|{:?}|{}",
        s.repo,
        serde_json::to_string(&s.files).unwrap_or_default(),
        s.selected,
        diff,
        s.status_error,
        s.watcher_error,
        s.refreshing,
        s.head,
        s.head_seen,
        s.scope,
        s.base,
        s.base_error,
        s.default_base,
        s.rename_sources,
        s.refs.as_ref().map(Arc::as_ptr),
        s.refs_overflow,
        s.refs_seq,
        s.pick_seq,
        s.pick_error,
        s.mark,
        s.mark_seq,
        s.mark_error,
        s.unread,
        s.quick.as_ref().map(Arc::as_ptr),
        s.action_seq,
        s.action_error,
        s.action_applied
    )
}

fn publish(state: &mut State, mut next: Snapshot, out: &mpsc::Sender<Arc<Snapshot>>) {
    if state.snapshot.revision > 0 && fingerprint(&state.snapshot) == fingerprint(&next) {
        return;
    }
    next.revision = state.snapshot.revision + 1;
    state.snapshot = next.clone();
    let _ = out.send(Arc::new(next));
}

fn start_watcher(
    state: &mut State,
    config: &SessionConfig,
    cwd: &str,
    sink: &Arc<dyn EventSink>,
    results: &UnboundedSender<Done>,
) {
    state.watcher = WatcherPhase::Starting;
    let watcher = config.watcher.clone();
    let cwd = cwd.to_string();
    let sink = sink.clone();
    let results = results.clone();
    tokio::spawn(async move {
        let result = watcher.start(cwd.clone(), sink).await;
        let started = result.is_ok();
        if results.send(Done::Watcher(result)).is_err() && started {
            let _ = watcher.stop(cwd).await;
        }
    });
}

async fn check_git(
    check: Arc<dyn Fn() -> Result<gitver::GitVersion, gitver::GitCheckError> + Send + Sync>,
) -> Result<gitver::GitVersion, gitver::GitCheckError> {
    tokio::task::spawn_blocking(move || check())
        .await
        .unwrap_or_else(|e| {
            Err(gitver::GitCheckError::Missing(format!(
                "git check failed: {e}"
            )))
        })
}

async fn stop_watcher(
    state: &mut State,
    config: &SessionConfig,
    cwd: &str,
    results: &mut UnboundedReceiver<Done>,
) {
    if matches!(state.watcher, WatcherPhase::Starting) {
        let _ = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(done) = results.recv().await {
                if let Done::Watcher(result) = done {
                    state.watcher = if result.is_ok() {
                        WatcherPhase::Running
                    } else {
                        WatcherPhase::Failed
                    };
                    break;
                }
            }
        })
        .await;
    }
    // Closing before draining assigns late registrations to their start task.
    results.close();
    while let Some(done) = results.recv().await {
        if let Done::Watcher(result) = done {
            state.watcher = if result.is_ok() {
                WatcherPhase::Running
            } else {
                WatcherPhase::Failed
            };
        }
    }
    if matches!(state.watcher, WatcherPhase::Running) {
        let _ = config.watcher.stop(cwd.to_string()).await;
    }
}

fn key_of(file: &ChangedFile) -> FileKey {
    FileKey::of(file)
}

#[allow(clippy::too_many_arguments)]
async fn run(
    config: SessionConfig,
    mut commands: UnboundedReceiver<Command>,
    snapshots: mpsc::Sender<Arc<Snapshot>>,
    refreshes: Arc<AtomicUsize>,
    refs_answered: Arc<AtomicUsize>,
    diffs_discarded: Arc<AtomicUsize>,
    head_samples: Arc<AtomicUsize>,
    pre_images: Arc<AtomicUsize>,
) {
    let mut state = State {
        snapshot: Snapshot::empty(&config.path.to_string_lossy()),
        branch: None,
        worktree: None,
        last_watcher_refresh: Instant::now(),
        watcher: WatcherPhase::Failed,
        git_missing: false,
        status_in_flight: false,
        status_dirty: false,
        status_dirty_head: false,
        diff_generation: 0,
        diff_in_flight: None,
        diff_dirty: false,
        requested_scope: config.scope,
        inputs: ResolveInputs {
            session_pick: None,
            session_mark: None,
            config: config.base_ref.clone(),
            state_dir: config.state_dir.clone(),
        },
        session_mark: None,
        unread_at: None,
        resolve_pending: true,
        changes: VecDeque::new(),
        in_flight_mark: None,
        rows_at: None,
        in_flight_resolve: false,
        pick_seq: 0,
        mark_seq: 0,
        worktree_renames: BTreeMap::new(),
        diff_lane: Arc::new(Semaphore::new(1)),
        latest_generation: Arc::new(AtomicU64::new(0)),
        status_delay: config.status_delay,
        act_pending: false,
        acted: None,
        fresh_arc_pending: false,
        action_seq: 0,
    };
    let path = match config.path.canonicalize() {
        Ok(path) if path.is_dir() => path,
        result => {
            let reason = match result {
                Ok(path) => format!("not a directory: {}", path.display()),
                Err(e) => format!("invalid path '{}': {e}", config.path.display()),
            };
            let mut next = state.snapshot.clone();
            next.repo = RepoState::Unusable { reason };
            publish(&mut state, next, &snapshots);
            return;
        }
    };
    let cwd = path.to_string_lossy().into_owned();
    let (triggers_tx, mut triggers) = unbounded_channel();
    let sink: Arc<dyn EventSink> = Arc::new(ChannelSink { tx: triggers_tx });
    let (results_tx, mut results) = unbounded_channel();
    match check_git(config.git_check.clone()).await {
        Ok(_) => {
            start_watcher(&mut state, &config, &cwd, &sink, &results_tx);
            state.request_status(&cwd, true, &results_tx, &refreshes);
        }
        Err(error) => {
            let mut next = state.snapshot.clone();
            match error {
                gitver::GitCheckError::TooOld(reason) => {
                    next.repo = RepoState::Unusable { reason };
                    publish(&mut state, next, &snapshots);
                    return;
                }
                gitver::GitCheckError::Missing(reason) => {
                    state.git_missing = true;
                    next.status_error = Some(reason);
                    publish(&mut state, next, &snapshots);
                }
            }
        }
    }
    let mut git_check_in_flight = false;
    let mut interval = tokio::time::interval_at(
        tokio::time::Instant::now() + config.poll_interval,
        config.poll_interval,
    );
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break };
                match command {
                    Command::Shutdown => break,
                    Command::Refresh => {
                        state.resolve_pending = true;
                        let mut next = state.snapshot.clone();
                        next.refreshing = true;
                        publish(&mut state, next, &snapshots);
                        if state.git_missing {
                            if !git_check_in_flight {
                                git_check_in_flight = true;
                                let check = config.git_check.clone();
                                let results = results_tx.clone();
                                tokio::spawn(async move {
                                    let _ = results.send(Done::GitCheck(check_git(check).await));
                                });
                            }
                        } else {
                            if matches!(state.watcher, WatcherPhase::Failed) {
                                start_watcher(&mut state, &config, &cwd, &sink, &results_tx);
                            }
                            state.request_status(&cwd, true, &results_tx, &refreshes);
                        }
                    }
                    Command::SetScope(scope) => {
                        if state.status_in_flight || scope != state.snapshot.scope || scope != state.requested_scope {
                            state.changes.push_back(Change::Scope(scope));
                            let mut next = state.snapshot.clone();
                            next.refreshing = true;
                            publish(&mut state, next, &snapshots);
                            state.request_status(&cwd, false, &results_tx, &refreshes);
                        }
                    }
                    Command::SetBase(pick) => {
                        state.changes.push_back(Change::Base(pick));
                        let mut next = state.snapshot.clone();
                        next.refreshing = true;
                        publish(&mut state, next, &snapshots);
                        state.request_status(&cwd, false, &results_tx, &refreshes);
                    }
                    Command::MarkReviewed(commit) => {
                        // Key repeat must not queue the same commit twice: each queued change
                        // costs a refresh and a write, and the second would answer nothing new.
                        // Only the latest pending mark counts -- one queued behind another
                        // commit is the user changing their mind back, not a repeat -- and a
                        // deliberate retry still lands, because the answer that prompts it
                        // clears the in-flight mark first. A press dropped here never reaches
                        // the queue and so owes no answer; every mark that does reach it still
                        // advances `mark_seq` exactly once.
                        let latest_pending = state
                            .changes
                            .iter()
                            .rev()
                            .find_map(|change| match change {
                                Change::Mark(queued) => Some(queued.as_str()),
                                _ => None,
                            })
                            .or(state.in_flight_mark.as_deref());
                        if latest_pending != Some(commit.as_str()) {
                            state.changes.push_back(Change::Mark(commit));
                            let mut next = state.snapshot.clone();
                            next.refreshing = true;
                            publish(&mut state, next, &snapshots);
                            state.request_status(&cwd, false, &results_tx, &refreshes);
                        }
                    }
                    Command::LoadRefs(token) => {
                        let toplevel = match &state.snapshot.repo {
                            RepoState::Repo { toplevel, .. } => Some(toplevel.clone()),
                            _ => None,
                        };
                        let results = results_tx.clone();
                        tokio::spawn(async move {
                            let (result, quick) = match toplevel {
                                Some(toplevel) => (
                                    base::list_refs(&toplevel).await,
                                    base::quick_bases(&toplevel).await,
                                ),
                                None => (Ok((Vec::new(), false)), Vec::new()),
                            };
                            let _ = results.send(Done::Refs { token, result, quick });
                        });
                    }
                    Command::Act(action) => {
                        let published = match &state.snapshot.diff {
                            DiffState::Ready(d) => Some(d),
                            _ => None,
                        };
                        let same = published.is_some_and(|d| Arc::ptr_eq(d, &action.diff));
                        let acted = state.acted.as_ref().is_some_and(|a| Arc::ptr_eq(a, &action.diff));
                        let toplevel = match &state.snapshot.repo {
                            RepoState::Repo { toplevel, .. } => Some(toplevel.clone()),
                            _ => None,
                        };
                        // Decided here, without git: concurrency, identity, eligibility, a repository.
                        let planned = if state.act_pending {
                            Err(actions::NOTICE_RUNNING.to_string())
                        } else if !same || acted {
                            Err(actions::NOTICE_CHANGED.to_string())
                        } else if toplevel.is_none() {
                            Err("not a git repository".to_string())
                        } else {
                            actions::plan(&action)
                        };
                        match planned {
                            Err(refusal) => {
                                state.action_seq += 1;
                                let mut next = state.snapshot.clone();
                                next.action_seq = state.action_seq;
                                next.action_error = Some(refusal);
                                next.action_applied = false;
                                publish(&mut state, next, &snapshots);
                            }
                            Ok((form, patch)) => {
                                state.act_pending = true;
                                state.changes.push_back(Change::Act(ActJob { action, form, patch, refusal: None }));
                                let mut next = state.snapshot.clone();
                                next.refreshing = true;
                                publish(&mut state, next, &snapshots);
                                state.request_status(&cwd, false, &results_tx, &refreshes);
                            }
                        }
                    }
                    selection => {
                        let selected = match selection {
                            Command::Select(key) => state.snapshot.files.iter()
                                .find(|f| key_of(f) == key).map(key_of),
                            Command::SelectNext | Command::SelectPrev if !state.snapshot.files.is_empty() => {
                                let current = state.snapshot.files.iter()
                                    .position(|f| Some(key_of(f)) == state.snapshot.selected).unwrap_or(0);
                                let step = if matches!(selection, Command::SelectNext) { 1 } else { -1 };
                                let index = (current as isize + step).rem_euclid(state.snapshot.files.len() as isize);
                                Some(key_of(&state.snapshot.files[index as usize]))
                            }
                            _ => None,
                        };
                        if let Some(key) = selected {
                            let mut next = state.snapshot.clone();
                            next.selected = Some(key.clone());
                            next.diff = DiffState::Loading;
                            next.head = None;
                            publish(&mut state, next, &snapshots);
                            state.request_diff(key, &cwd, config.diff_delay, config.diff_gate.clone(), &results_tx, &head_samples, &pre_images, &diffs_discarded);
                        }
                    }
                }
            }
            Some(trigger) = triggers.recv() => {
                let mut with_head = matches!(trigger, Trigger::Head);
                while let Ok(trigger) = triggers.try_recv() {
                    with_head |= matches!(trigger, Trigger::Head);
                }
                if !state.git_missing {
                    state.last_watcher_refresh = Instant::now();
                    state.request_status(&cwd, with_head, &results_tx, &refreshes);
                }
            }
            _ = interval.tick() => {
                if !state.git_missing {
                    if !matches!(state.watcher, WatcherPhase::Running) {
                        state.request_status(&cwd, true, &results_tx, &refreshes);
                    } else if state.last_watcher_refresh.elapsed() >= config.poll_interval {
                        state.request_status(&cwd, false, &results_tx, &refreshes);
                    }
                }
            }
            Some(done) = results.recv() => {
                let mut next = state.snapshot.clone();
                match done {
                    Done::GitCheck(result) => {
                        git_check_in_flight = false;
                        match result {
                            Ok(_) => {
                                state.git_missing = false;
                                next.status_error = None;
                                start_watcher(&mut state, &config, &cwd, &sink, &results_tx);
                                state.request_status(&cwd, true, &results_tx, &refreshes);
                            }
                            Err(gitver::GitCheckError::Missing(reason)) => {
                                next.status_error = Some(reason);
                                next.refreshing = false;
                            }
                            Err(gitver::GitCheckError::TooOld(reason)) => {
                                next.repo = RepoState::Unusable { reason };
                                next.refreshing = false;
                                publish(&mut state, next, &snapshots);
                                break;
                            }
                        }
                        publish(&mut state, next, &snapshots);
                    }
                    Done::Refs { token, result, quick } => {
                        if token > next.refs_seq {
                            let (refs, overflow) = result.unwrap_or_default();
                            next.refs = Some(Arc::new(refs));
                            next.quick = Some(Arc::new(quick));
                            next.refs_overflow = overflow;
                            next.refs_seq = token;
                            publish(&mut state, next, &snapshots);
                        }
                        refs_answered.fetch_add(1, Ordering::SeqCst);
                    }
                    Done::Watcher(result) => {
                        state.watcher = if result.is_ok() { WatcherPhase::Running } else { WatcherPhase::Failed };
                        next.watcher_error = result.err();
                        publish(&mut state, next, &snapshots);
                    }
                    Done::Status { response, head, head_seen, head_sampled, confirmed, loaded, change, acted } => {
                        state.status_in_flight = false;
                        state.in_flight_mark = None;
                        if let (Some(Change::Act(act)), Some((result, applied))) = (&change, &acted) {
                            state.act_pending = false;
                            state.action_seq += 1;
                            next.action_seq = state.action_seq;
                            next.action_error = result.clone().err();
                            next.action_applied = *applied;
                            if *applied {
                                state.acted = Some(act.action.diff.clone());
                                state.fresh_arc_pending = true;
                            }
                        }
                        // The rows may only claim a commit the refresh bracketed: the opening
                        // sample alone would let rows loaded across a move name a commit whose
                        // files they never listed.
                        let head_seen_for_rows = confirmed.clone();
                        let before = comparison_of(&state.snapshot);
                        let mut succeeded = false;
                        let mut change_error = None;
                        let mut mark_answer = None;
                        let mut mark_answered = false;
                        match response {
                            Err(e) => {
                                if matches!(change, Some(Change::Mark(_))) {
                                    mark_answer = Some(e.clone());
                                }
                                change_error = Some(e.clone());
                                next.status_error = Some(e);
                            }
                            Ok(response) => {
                                if let Some((branch, worktree)) = head {
                                    state.branch = branch;
                                    state.worktree = worktree;
                                }
                                next.repo = if response.repo_root.is_empty() {
                                    RepoState::NotARepo { cwd: cwd.clone() }
                                } else {
                                    RepoState::Repo {
                                        toplevel: response.repo_root.clone(),
                                        branch: state.branch.clone(),
                                        worktree: state.worktree.clone(),
                                    }
                                };
                                match loaded {
                                    None if matches!(change, Some(Change::Base(_))) => {
                                        change_error = Some("not a git repository".into());
                                    }
                                    None if matches!(change, Some(Change::Mark(_))) => {
                                        mark_answer = Some("not a git repository".into());
                                    }
                                    Some(Err(e)) => match &change {
                                        // Failed picks and switches report notices; refresh errors keep the rows.
                                        Some(Change::Base(_)) => change_error = Some(e),
                                        Some(Change::Mark(_)) => mark_answer = Some(e),
                                        Some(Change::Scope(_)) => next.base_error = Some(e),
                                        Some(Change::Act(_)) | None => next.status_error = Some(e),
                                    },
                                    other => {
                                        next.status_error = None;
                                        let loaded = match other {
                                            Some(Ok(loaded)) => *loaded,
                                            _ => Loaded {
                                                scope: Scope::Worktree,
                                                base: None,
                                                default_base: None,
                                                mark: None,
                                                unread: BTreeSet::new(),
                                                unread_at: None,
                                                base_error: None,
                                                files: response.files,
                                                rename_sources: BTreeMap::new(),
                                                worktree_renames: BTreeMap::new(),
                                                persisted: None,
                                                marked: None,
                                            },
                                        };
                                        let switched = loaded.scope != next.scope;
                                        let previous = next.selected.take();
                                        let old_index = previous.as_ref().and_then(|key| next.files.iter().position(|f| &key_of(f) == key));
                                        next.scope = loaded.scope;
                                        next.base = loaded.base;
                                        next.base_error = loaded.base_error;
                                        // `Keep` echoes the previous default; a resolution publishes its own, `None` included.
                                        next.default_base = loaded.default_base;
                                        next.files = loaded.files;
                                        next.rename_sources = Arc::new(loaded.rename_sources);
                                        state.worktree_renames = loaded.worktree_renames;
                                        next.mark = loaded.mark;
                                        next.unread = Arc::new(loaded.unread);
                                        state.unread_at = loaded.unread_at;
                                        next.selected = if switched {
                                            // The path survives a scope switch; the unstaged row wins when both exist.
                                            previous.as_ref().and_then(|key| {
                                                next.files.iter().filter(|f| f.path == key.path).min_by_key(|f| f.staged).map(key_of)
                                            })
                                        } else {
                                            previous.clone().filter(|key| next.files.iter().any(|f| &key_of(f) == key))
                                                .or_else(|| old_index.and_then(|i| next.files.get(i.min(next.files.len().saturating_sub(1))).map(key_of)))
                                        }
                                        .or_else(|| next.files.first().map(key_of));
                                        if let Some(Change::Act(act)) = &change {
                                            // The acted row is gone: the other row of the same path takes the selection,
                                            // unless the user selected something else while the action was carried.
                                            let key = &act.action.diff.key;
                                            let listed = next.files.iter().any(|f| &key_of(f) == key);
                                            if !listed && previous.as_ref() == Some(key) {
                                                if let Some(sibling) = next.files.iter().find(|f| f.path == key.path) {
                                                    next.selected = Some(key_of(sibling));
                                                }
                                            }
                                        }
                                        if let Some(Change::Base(pick)) = &change {
                                            match loaded.persisted {
                                                Some(Ok(())) => state.inputs.session_pick = None,
                                                Some(Err(e)) => {
                                                    state.inputs.session_pick = Some(pick.clone());
                                                    next.base_error = Some(format!("pick not remembered: {e}"));
                                                }
                                                None => {}
                                            }
                                        }
                                        if let Some((mark, set, written, read_at)) = loaded.marked {
                                            mark_answered = true;
                                            state.mark_seq += 1;
                                            next.mark_seq = state.mark_seq;
                                            next.mark = Some(mark.clone());
                                            next.unread = Arc::new(set);
                                            state.unread_at = read_at;
                                            match written {
                                                Ok(()) => {
                                                    state.session_mark = None;
                                                    next.mark_error = None;
                                                }
                                                Err(e) => {
                                                    state.session_mark = Some(MarkRecord {
                                                        commit: mark.commit,
                                                        at: mark.at,
                                                    });
                                                    next.mark_error = Some(format!("mark not remembered: {e}"));
                                                }
                                            }
                                        }
                                        state.rows_at = head_seen_for_rows.clone();
                                        succeeded = true;
                                    }
                                }
                            }
                        }
                        if matches!(change, Some(Change::Mark(_))) && !mark_answered {
                            state.mark_seq += 1;
                            next.mark_seq = state.mark_seq;
                            next.mark_error = mark_answer.or_else(|| Some("no answer".into()));
                        }
                        // A refresh asked to resolve that delivered no rows puts the request
                        // back: an explicit `r` must not be lost to a failed status or to a mark
                        // whose id would not verify.
                        if !succeeded && state.in_flight_resolve {
                            state.resolve_pending = true;
                        }
                        state.in_flight_resolve = false;
                        if head_sampled {
                            // Published even when the load failed: this is the observation, not
                            // a claim about the rows. A failed refresh keeps the previous rows,
                            // mark and dots, so `head_seen` can name a newer commit than the set
                            // describes; every consumer gates on a matching id instead
                            // (`classified_at` for the warning, `unread_at` for the cache).
                            next.head_seen = head_seen;
                        }
                        state.requested_scope = next.scope;
                        let comparison = comparison_of(&next);
                        if comparison != before {
                            // Results computed under the previous comparison are stale from here on.
                            state.diff_generation += 1;
                            state.diff_in_flight = None;
                        }
                        let same = matches!(&next.diff, DiffState::Ready(d) if Some(&d.key) == next.selected.as_ref() && d.comparison == comparison);
                        if succeeded && !same {
                            next.diff = if next.selected.is_some() { DiffState::Loading } else { DiffState::Idle };
                        }
                        let previous_head = next.head.clone();
                        next.head = markable(
                            &next,
                            previous_head,
                            head_sampled,
                            confirmed.filter(|_| succeeded),
                            state.rows_at.as_deref(),
                        );
                        if matches!(change, Some(Change::Base(_))) {
                            state.pick_seq += 1;
                            next.pick_seq = state.pick_seq;
                            next.pick_error = change_error;
                        }
                        let selected = next.selected.clone().filter(|_| succeeded);
                        if selected.is_none() {
                            next.refreshing = !state.changes.is_empty() || state.status_dirty;
                        }
                        publish(&mut state, next, &snapshots);
                        if let Some(key) = selected {
                            state.request_diff(key, &cwd, config.diff_delay, config.diff_gate.clone(), &results_tx, &head_samples, &pre_images, &diffs_discarded);
                        }
                        if state.status_dirty || !state.changes.is_empty() {
                            state.status_dirty = false;
                            let with_head = std::mem::take(&mut state.status_dirty_head);
                            state.request_status(&cwd, with_head, &results_tx, &refreshes);
                        }
                    }
                    Done::Diff { generation, key, comparison, read_at, pre_image, result } => {
                        if generation != state.diff_generation || comparison != comparison_of(&state.snapshot) {
                            diffs_discarded.fetch_add(1, Ordering::SeqCst);
                            continue;
                        }
                        state.diff_in_flight = None;
                        if Some(&key) == next.selected.as_ref() {
                            match result {
                                Ok((response, patch)) => {
                                    // A working-tree edit can change a staged row's pre-image without changing its diff,
                                    // and the first diff after an attempted action is always a fresh Arc.
                                    let fresh = std::mem::take(&mut state.fresh_arc_pending);
                                    let unchanged = !fresh
                                        && matches!(&next.diff, DiffState::Ready(d) if d.key == key && d.comparison == comparison && d.raw_diff == response.raw_diff && d.read_at == read_at && d.pre_image == pre_image);
                                    if !unchanged {
                                        next.diff = DiffState::Ready(Arc::new(LoadedDiff::build(key, comparison, read_at.clone(), response, patch, pre_image)));
                                    }
                                    if state.acted.as_ref().is_some_and(|a| !matches!(&next.diff, DiffState::Ready(d) if Arc::ptr_eq(a, d))) {
                                        state.acted = None;
                                    }
                                    // Only when the rows on screen came from this commit too.
                                    next.head =
                                        read_at.filter(|id| state.rows_at.as_deref() == Some(id.as_str()));
                                }
                                Err(e) => {
                                    next.diff = DiffState::Failed(e);
                                    next.head = None;
                                }
                            }
                            if !state.status_in_flight && !state.diff_dirty {
                                next.refreshing = false;
                            }
                            publish(&mut state, next, &snapshots);
                        }
                        if std::mem::take(&mut state.diff_dirty) {
                            if let Some(key) = state.snapshot.selected.clone() {
                                state.request_diff(key, &cwd, config.diff_delay, config.diff_gate.clone(), &results_tx, &head_samples, &pre_images, &diffs_discarded);
                            }
                        }
                    }
                }
            }
        }
    }
    stop_watcher(&mut state, &config, &cwd, &mut results).await;
}

/// Spawns the loop on `runtime` and returns immediately.
pub fn spawn(runtime: &tokio::runtime::Handle, config: SessionConfig) -> EngineHandle {
    let (commands, commands_rx) = unbounded_channel();
    let (snapshots_tx, snapshots) = mpsc::channel();
    let refreshes = Arc::new(AtomicUsize::new(0));
    let refs_answered = Arc::new(AtomicUsize::new(0));
    let diffs_discarded = Arc::new(AtomicUsize::new(0));
    let head_samples = Arc::new(AtomicUsize::new(0));
    let pre_images = Arc::new(AtomicUsize::new(0));
    runtime.spawn(run(
        config,
        commands_rx,
        snapshots_tx,
        refreshes.clone(),
        refs_answered.clone(),
        diffs_discarded.clone(),
        head_samples.clone(),
        pre_images.clone(),
    ));
    EngineHandle {
        commands,
        snapshots,
        refreshes,
        refs_answered,
        diffs_discarded,
        head_samples,
        pre_images,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::ChangedFileStatus;
    use std::process::Command as Proc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// The engine polls this repository while the test mutates it, and both take `index.lock`.
    /// That one failure is retried; anything else fails the test with git's own message.
    fn git(dir: &std::path::Path, args: &[&str]) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let out = Proc::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .unwrap();
            if out.status.success() {
                return;
            }
            let err = String::from_utf8_lossy(&out.stderr).into_owned();
            assert!(
                err.contains("index.lock") && Instant::now() < deadline,
                "git {args:?}: {err}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        git(p, &["config", "user.email", "t@example.com"]);
        git(p, &["config", "user.name", "t"]);
        std::fs::write(p.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(p.join("b.txt"), "b\n").unwrap();
        git(p, &["add", "-A"]);
        git(p, &["commit", "-q", "-m", "init"]);
        std::fs::write(p.join("a.txt"), "one\nTWO\n").unwrap();
        std::fs::write(p.join("b.txt"), "B\n").unwrap();
        dir
    }

    fn ok_git() -> Arc<dyn Fn() -> Result<gitver::GitVersion, gitver::GitCheckError> + Send + Sync>
    {
        Arc::new(|| {
            Ok(gitver::GitVersion {
                major: 2,
                minor: 99,
            })
        })
    }

    /// Fails to start until `allow` is set; never emits events.
    struct FlakyWatcher {
        allow: Arc<AtomicBool>,
    }
    impl WatcherControl for FlakyWatcher {
        fn start(
            &self,
            _cwd: String,
            _sink: Arc<dyn crate::runtime::EventSink>,
        ) -> BoxFut<Result<(), String>> {
            let ok = self.allow.load(Ordering::SeqCst);
            Box::pin(async move {
                if ok {
                    Ok(())
                } else {
                    Err("inotify limit".to_string())
                }
            })
        }
        fn stop(&self, _cwd: String) -> BoxFut<Result<(), String>> {
            Box::pin(async { Ok(()) })
        }
    }

    fn start(
        dir: &std::path::Path,
        allow: Arc<AtomicBool>,
    ) -> (tokio::runtime::Runtime, EngineHandle) {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let handle = spawn(
            rt.handle(),
            SessionConfig {
                scope: Scope::Worktree,
                base_ref: None,
                state_dir: None,
                path: dir.to_path_buf(),
                poll_interval: Duration::from_millis(50),
                watcher: Arc::new(FlakyWatcher { allow }),
                git_check: ok_git(),
                diff_delay: None,
                diff_gate: None,
                status_delay: None,
            },
        );
        (rt, handle)
    }

    fn wait_for(
        handle: &EngineHandle,
        what: &str,
        pred: impl Fn(&Snapshot) -> bool,
    ) -> Arc<Snapshot> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut last = None;
        while Instant::now() < deadline {
            while let Ok(s) = handle.snapshots.try_recv() {
                if pred(&s) {
                    return s;
                }
                last = Some(s);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for: {what}; last = {last:?}");
    }

    fn ready(s: &Snapshot) -> Option<&LoadedDiff> {
        match &s.diff {
            DiffState::Ready(d) => Some(d),
            _ => None,
        }
    }

    #[test]
    fn first_load_selects_the_first_row_and_loads_its_diff() {
        let dir = fixture();
        let (_rt, h) = start(dir.path(), Arc::new(AtomicBool::new(true)));
        let s = wait_for(&h, "ready diff", |s| ready(s).is_some());
        assert_eq!(s.files.len(), 2);
        assert_eq!(
            s.selected,
            Some(FileKey {
                path: "a.txt".into(),
                staged: false,
                untracked: false,
            })
        );
        assert_eq!(ready(&s).unwrap().file_diff.hunks.len(), 1);
        assert!(matches!(s.repo, RepoState::Repo { .. }));
    }

    #[test]
    fn startup_runs_exactly_one_status_refresh() {
        let dir = fixture();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let h = spawn(
            rt.handle(),
            SessionConfig {
                scope: Scope::Worktree,
                base_ref: None,
                state_dir: None,
                path: dir.path().to_path_buf(),
                poll_interval: Duration::from_secs(3600),
                watcher: Arc::new(SlowWatcher {
                    starts: Arc::new(AtomicUsize::new(0)),
                    stops: Arc::new(AtomicUsize::new(0)),
                    fail_first: AtomicBool::new(false),
                    release: None,
                }),
                git_check: ok_git(),
                diff_delay: None,
                diff_gate: None,
                status_delay: None,
            },
        );
        wait_for(&h, "first ready diff", |s| ready(s).is_some());
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(h.refreshes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn select_next_wraps_and_loads_the_other_file() {
        let dir = fixture();
        let (_rt, h) = start(dir.path(), Arc::new(AtomicBool::new(true)));
        wait_for(&h, "first", |s| ready(s).is_some());
        h.commands.send(Command::SelectNext).unwrap();
        wait_for(&h, "b selected", |s| {
            ready(s).map(|d| d.key.path == "b.txt").unwrap_or(false)
        });
        h.commands.send(Command::SelectNext).unwrap();
        wait_for(&h, "wrapped to a", |s| {
            ready(s).map(|d| d.key.path == "a.txt").unwrap_or(false)
        });
    }

    #[test]
    fn degraded_mode_converges_on_a_repeated_edit_and_recovers_on_refresh() {
        let dir = fixture();
        let allow = Arc::new(AtomicBool::new(false));
        let (_rt, h) = start(dir.path(), allow.clone());
        let s = wait_for(&h, "degraded", |s| {
            s.watcher_error.is_some() && ready(s).is_some()
        });
        let before = ready(&s).unwrap().raw_diff.clone();
        std::fs::write(dir.path().join("a.txt"), "one\nTWO AGAIN\n").unwrap();
        wait_for(&h, "poll picks up the second edit", |s| {
            ready(s).map(|d| d.raw_diff != before).unwrap_or(false)
        });
        git(dir.path(), &["stash", "-q"]);
        git(dir.path(), &["switch", "-q", "-c", "other"]);
        wait_for(
            &h,
            "branch through the tick",
            |s| matches!(&s.repo, RepoState::Repo { branch: Some(b), .. } if b == "other"),
        );
        allow.store(true, Ordering::SeqCst);
        h.commands.send(Command::Refresh).unwrap();
        wait_for(&h, "watcher recovered", |s| s.watcher_error.is_none());
    }

    #[test]
    fn an_unchanged_repository_publishes_nothing_more() {
        let dir = fixture();
        let (_rt, h) = start(dir.path(), Arc::new(AtomicBool::new(false)));
        let s = wait_for(&h, "settled", |s| {
            ready(s).is_some() && s.watcher_error.is_some()
        });
        std::thread::sleep(Duration::from_millis(400)); // several poll ticks
        let mut latest = s.revision;
        while let Ok(n) = h.snapshots.try_recv() {
            latest = n.revision;
            assert!(ready(&n).is_some(), "diff must stay Ready");
        }
        assert_eq!(
            latest, s.revision,
            "no publication on an unchanged repository"
        );
    }

    #[test]
    fn a_directory_that_is_not_a_repository_reports_not_a_repo() {
        let dir = tempfile::tempdir().unwrap();
        let (_rt, h) = start(dir.path(), Arc::new(AtomicBool::new(true)));
        wait_for(&h, "not a repo", |s| {
            matches!(s.repo, RepoState::NotARepo { .. }) && s.revision > 0
        });
        git(dir.path(), &["init", "-q", "-b", "main"]);
        h.commands.send(Command::Refresh).unwrap();
        wait_for(&h, "upgraded", |s| matches!(s.repo, RepoState::Repo { .. }));
    }

    #[test]
    fn a_diff_is_never_published_for_a_deselected_row() {
        let dir = fixture();
        let (_rt, h) = start(dir.path(), Arc::new(AtomicBool::new(true)));
        wait_for(&h, "first", |s| ready(s).is_some());
        for _ in 0..6 {
            h.commands.send(Command::SelectNext).unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            while let Ok(s) = h.snapshots.try_recv() {
                if let Some(d) = ready(&s) {
                    assert_eq!(
                        Some(&d.key),
                        s.selected.as_ref(),
                        "published a diff for a row that is not selected"
                    );
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Starts fine and hands the sink to the test, so it can emit watcher events.
    struct EmittingWatcher {
        sink: Arc<std::sync::Mutex<Option<Arc<dyn crate::runtime::EventSink>>>>,
    }
    impl WatcherControl for EmittingWatcher {
        fn start(
            &self,
            _cwd: String,
            sink: Arc<dyn crate::runtime::EventSink>,
        ) -> BoxFut<Result<(), String>> {
            *self.sink.lock().unwrap() = Some(sink);
            Box::pin(async { Ok(()) })
        }
        fn stop(&self, _cwd: String) -> BoxFut<Result<(), String>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[test]
    fn a_burst_of_watcher_events_is_coalesced() {
        let dir = fixture();
        let slot = Arc::new(std::sync::Mutex::new(None));
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let h = spawn(
            rt.handle(),
            SessionConfig {
                scope: Scope::Worktree,
                base_ref: None,
                state_dir: None,
                path: dir.path().to_path_buf(),
                poll_interval: Duration::from_secs(3600),
                watcher: Arc::new(EmittingWatcher { sink: slot.clone() }),
                git_check: ok_git(),
                diff_delay: None,
                diff_gate: None,
                status_delay: None,
            },
        );
        wait_for(&h, "first", |s| ready(s).is_some());
        let sink = wait_until(|| slot.lock().unwrap().clone());
        let before = h.refreshes.load(Ordering::SeqCst);
        for _ in 0..20 {
            sink.emit_json("git-status-changed", serde_json::json!({ "cwds": [] }))
                .unwrap();
        }
        std::thread::sleep(Duration::from_millis(1500));
        let runs = h.refreshes.load(Ordering::SeqCst) - before;
        assert!(
            (1..=3).contains(&runs),
            "20 events must coalesce into at most 3 refreshes, got {runs}"
        );
    }

    #[test]
    fn a_slow_diff_is_not_starved_by_frequent_polls() {
        let dir = fixture();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        // the watcher never starts, so every 50 ms tick refreshes while each diff takes 400 ms
        let h = spawn(
            rt.handle(),
            SessionConfig {
                scope: Scope::Worktree,
                base_ref: None,
                state_dir: None,
                path: dir.path().to_path_buf(),
                poll_interval: Duration::from_millis(50),
                watcher: Arc::new(FlakyWatcher {
                    allow: Arc::new(AtomicBool::new(false)),
                }),
                git_check: ok_git(),
                diff_delay: Some(Duration::from_millis(400)),
                diff_gate: None,
                status_delay: None,
            },
        );
        wait_for(&h, "a diff despite constant polling", |s| {
            ready(s).is_some()
        });
    }

    #[test]
    fn a_refresh_stays_busy_until_a_diff_started_after_it_completes() {
        let dir = fixture();
        let gate = Arc::new(Semaphore::new(0));
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let h = spawn(
            rt.handle(),
            SessionConfig {
                scope: Scope::Worktree,
                base_ref: None,
                state_dir: None,
                path: dir.path().to_path_buf(),
                poll_interval: Duration::from_secs(3600),
                watcher: Arc::new(FlakyWatcher {
                    allow: Arc::new(AtomicBool::new(true)),
                }),
                git_check: ok_git(),
                diff_delay: None,
                diff_gate: Some(gate.clone()),
                status_delay: None,
            },
        );
        // release the initial load, then hold the selected diff (D1).
        gate.add_permits(1);
        wait_for(&h, "first ready diff", |s| ready(s).is_some());
        h.commands.send(Command::SelectNext).unwrap();
        let loading = wait_for(&h, "next selection loading", |s| {
            matches!(s.diff, DiffState::Loading)
        });
        let key = loading.selected.clone().unwrap();
        // refresh starts while D1 is still blocked.
        h.commands.send(Command::Refresh).unwrap();
        wait_for(&h, "refresh busy", |s| s.refreshing);
        // D1's own snapshot must stay busy while the follow-up (D2) is gated.
        gate.add_permits(1);
        let first = wait_for(&h, "pre-refresh diff ready", |s| {
            ready(s).is_some_and(|d| d.key == key)
        });
        assert!(
            first.refreshing,
            "refresh cleared when the pre-refresh diff completed"
        );
        // release D2 to finish the refresh.
        gate.add_permits(1);
        let refreshed = wait_for(&h, "refresh completion", |s| !s.refreshing);
        assert_eq!(ready(&refreshed).map(|d| &d.key), Some(&key));
    }

    #[test]
    fn refresh_without_a_selected_row_clears_the_busy_flag() {
        let dir = fixture();
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "clean"]);
        let (_rt, h) = start(dir.path(), Arc::new(AtomicBool::new(true)));
        wait_for(&h, "clean repository", |s| {
            matches!(s.repo, RepoState::Repo { .. }) && s.files.is_empty()
        });
        h.commands.send(Command::Refresh).unwrap();
        wait_for(&h, "busy", |s| s.refreshing);
        wait_for(&h, "idle again", |s| !s.refreshing);
    }

    /// Waits for a release signal or 300 ms and counts calls.
    struct SlowWatcher {
        starts: Arc<std::sync::atomic::AtomicUsize>,
        stops: Arc<std::sync::atomic::AtomicUsize>,
        fail_first: AtomicBool,
        release: Option<Arc<tokio::sync::Notify>>,
    }
    impl WatcherControl for SlowWatcher {
        fn start(
            &self,
            _cwd: String,
            _sink: Arc<dyn crate::runtime::EventSink>,
        ) -> BoxFut<Result<(), String>> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            let fail = self.fail_first.swap(false, Ordering::SeqCst);
            let release = self.release.clone();
            Box::pin(async move {
                if let Some(release) = release {
                    release.notified().await;
                } else {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
                if fail {
                    Err("first start fails".to_string())
                } else {
                    Ok(())
                }
            })
        }
        fn stop(&self, _cwd: String) -> BoxFut<Result<(), String>> {
            self.stops.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }
    }

    #[test]
    fn a_pending_watcher_start_is_not_repeated_and_is_stopped_exactly_once() {
        let dir = fixture();
        let starts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let stops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let h = spawn(
            rt.handle(),
            SessionConfig {
                scope: Scope::Worktree,
                base_ref: None,
                state_dir: None,
                path: dir.path().to_path_buf(),
                poll_interval: Duration::from_secs(3600),
                watcher: Arc::new(SlowWatcher {
                    starts: starts.clone(),
                    stops: stops.clone(),
                    fail_first: AtomicBool::new(false),
                    release: None,
                }),
                git_check: ok_git(),
                diff_delay: None,
                diff_gate: None,
                status_delay: None,
            },
        );
        for _ in 0..3 {
            h.commands.send(Command::Refresh).unwrap(); // arrives while the first start is pending
        }
        h.commands.send(Command::Shutdown).unwrap(); // also while it is pending
        std::thread::sleep(Duration::from_millis(1200));
        assert_eq!(
            starts.load(Ordering::SeqCst),
            1,
            "a pending start must not be repeated"
        );
        assert_eq!(
            stops.load(Ordering::SeqCst),
            1,
            "a registered watcher must be stopped exactly once"
        );
    }

    #[test]
    fn a_watcher_registered_after_shutdown_is_stopped_exactly_once() {
        let dir = fixture();
        let starts = Arc::new(AtomicUsize::new(0));
        let stops = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(tokio::sync::Notify::new());
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let h = spawn(
            rt.handle(),
            SessionConfig {
                scope: Scope::Worktree,
                base_ref: None,
                state_dir: None,
                path: dir.path().to_path_buf(),
                poll_interval: Duration::from_secs(3600),
                watcher: Arc::new(SlowWatcher {
                    starts: starts.clone(),
                    stops: stops.clone(),
                    fail_first: AtomicBool::new(false),
                    release: Some(release.clone()),
                }),
                git_check: ok_git(),
                diff_delay: None,
                diff_gate: None,
                status_delay: None,
            },
        );
        h.commands.send(Command::Shutdown).unwrap();
        loop {
            match h.snapshots.recv_timeout(Duration::from_secs(7)) {
                Ok(_) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(e) => panic!("shutdown did not finish while the watcher was pending: {e}"),
            }
        }
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert_eq!(stops.load(Ordering::SeqCst), 0);
        release.notify_one();
        wait_until(|| (stops.load(Ordering::SeqCst) > 0).then_some(()));
        assert_eq!(stops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_missing_git_is_retryable_and_an_old_git_is_fatal() {
        let dir = fixture();
        let fixed = Arc::new(AtomicBool::new(false));
        let flag = fixed.clone();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let h = spawn(
            rt.handle(),
            SessionConfig {
                scope: Scope::Worktree,
                base_ref: None,
                state_dir: None,
                path: dir.path().to_path_buf(),
                poll_interval: Duration::from_millis(50),
                watcher: Arc::new(FlakyWatcher {
                    allow: Arc::new(AtomicBool::new(true)),
                }),
                git_check: Arc::new(move || {
                    if flag.load(Ordering::SeqCst) {
                        Ok(gitver::GitVersion {
                            major: 2,
                            minor: 99,
                        })
                    } else {
                        Err(gitver::GitCheckError::Missing(
                            "Failed to spawn git: not found".into(),
                        ))
                    }
                }),
                diff_delay: None,
                diff_gate: None,
                status_delay: None,
            },
        );
        let s = wait_for(&h, "missing git reported", |s| s.status_error.is_some());
        assert!(
            !matches!(s.repo, RepoState::Unusable { .. }),
            "a missing git must stay retryable"
        );
        fixed.store(true, Ordering::SeqCst);
        h.commands.send(Command::Refresh).unwrap();
        wait_for(&h, "recovered", |s| {
            s.status_error.is_none() && ready(s).is_some()
        });

        let old = spawn(
            rt.handle(),
            SessionConfig {
                scope: Scope::Worktree,
                base_ref: None,
                state_dir: None,
                path: dir.path().to_path_buf(),
                poll_interval: Duration::from_millis(50),
                watcher: Arc::new(FlakyWatcher {
                    allow: Arc::new(AtomicBool::new(true)),
                }),
                git_check: Arc::new(|| {
                    Err(gitver::GitCheckError::TooOld("git 2.20 is too old".into()))
                }),
                diff_delay: None,
                diff_gate: None,
                status_delay: None,
            },
        );
        wait_for(
            &old,
            "unusable",
            |s| matches!(&s.repo, RepoState::Unusable { reason } if reason.contains("2.20")),
        );
    }

    #[test]
    fn rapid_selection_does_not_wait_for_superseded_diffs() {
        let dir = fixture();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let h = spawn(
            rt.handle(),
            SessionConfig {
                scope: Scope::Worktree,
                base_ref: None,
                state_dir: None,
                path: dir.path().to_path_buf(),
                poll_interval: Duration::from_secs(3600),
                watcher: Arc::new(FlakyWatcher {
                    allow: Arc::new(AtomicBool::new(true)),
                }),
                git_check: ok_git(),
                diff_delay: Some(Duration::from_millis(400)),
                diff_gate: None,
                status_delay: None,
            },
        );
        wait_for(&h, "first", |s| ready(s).is_some());
        let started = Instant::now();
        for _ in 0..51 {
            h.commands.send(Command::SelectNext).unwrap(); // an odd number of selections ends on b
        }
        wait_for(&h, "the last selection loaded", |s| {
            s.selected
                .as_ref()
                .map(|k| k.path == "b.txt")
                .unwrap_or(false)
                && ready(s).map(|d| d.key.path == "b.txt").unwrap_or(false)
        });
        // back to back, 51 delayed requests would take over 20 s.
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "superseded diffs were waited for: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn superseded_selections_never_reach_git() {
        let dir = fixture();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let h = spawn(
            rt.handle(),
            SessionConfig {
                scope: Scope::Worktree,
                base_ref: None,
                state_dir: None,
                path: dir.path().to_path_buf(),
                poll_interval: Duration::from_secs(3600),
                watcher: Arc::new(FlakyWatcher {
                    allow: Arc::new(AtomicBool::new(true)),
                }),
                git_check: ok_git(),
                diff_delay: Some(Duration::from_millis(300)),
                diff_gate: None,
                status_delay: None,
            },
        );
        wait_for(&h, "first", |s| ready(s).is_some());
        let samples_before = h.head_samples.load(Ordering::SeqCst);
        for _ in 0..5 {
            h.commands.send(Command::SelectNext).unwrap(); // ends on b.txt
        }
        wait_for(&h, "the last selection loaded", |s| {
            ready(s).map(|d| d.key.path == "b.txt").unwrap_or(false) && !s.refreshing
        });
        // Let every queued task reach the lane before counting: with the check only the one in
        // flight when the burst began and the last requested sample; without it all of them would.
        std::thread::sleep(Duration::from_millis(1800));
        let samples = h.head_samples.load(Ordering::SeqCst) - samples_before;
        assert!(samples <= 2, "{samples} diff tasks reached git");
        assert!(
            h.diffs_discarded.load(Ordering::SeqCst) >= 3,
            "the middle selections were not skipped"
        );
    }

    #[test]
    fn both_rows_of_a_renamed_path_load_their_own_patch() {
        let dir = fixture();
        let p = dir.path();
        let ctx = "c1\nc2\nc3\nc4\nc5\nc6\nc7\n";
        std::fs::write(p.join("old.txt"), format!("one\n{ctx}two\n")).unwrap();
        git(p, &["add", "old.txt"]);
        git(p, &["commit", "-q", "-m", "old"]);
        git(p, &["mv", "old.txt", "new.txt"]);
        std::fs::write(p.join("new.txt"), format!("ONE\n{ctx}two\n")).unwrap();
        git(p, &["add", "new.txt"]);
        std::fs::write(p.join("new.txt"), format!("ONE\n{ctx}TWO\n")).unwrap(); // RM
        let (_rt, h) = start(p, Arc::new(AtomicBool::new(true)));
        let key = |staged: bool| FileKey {
            path: "new.txt".into(),
            staged,
            untracked: false,
        };
        wait_for(&h, "both rows listed", |s| {
            s.files.iter().filter(|f| f.path == "new.txt").count() == 2
        });
        h.commands.send(Command::Select(key(true))).unwrap();
        let s = wait_for(&h, "staged row", |s| {
            ready(s).is_some_and(|d| d.key == key(true))
        });
        let d = ready(&s).unwrap();
        assert_eq!(d.file_diff.old_path.as_deref(), Some("old.txt"));
        assert!(
            d.raw_diff.contains("+ONE") && !d.raw_diff.contains("+TWO"),
            "{}",
            d.raw_diff
        );
        h.commands.send(Command::Select(key(false))).unwrap();
        let s = wait_for(&h, "unstaged row", |s| {
            ready(s).is_some_and(|d| d.key == key(false))
        });
        let d = ready(&s).unwrap();
        assert_eq!(d.file_diff.old_path, None);
        assert!(
            d.raw_diff.contains("+TWO") && !d.raw_diff.contains("+ONE"),
            "{}",
            d.raw_diff
        );
        assert_eq!(
            s.rename_sources.get("new.txt").map(String::as_str),
            Some("old.txt"),
            "the panel's map"
        );
    }

    fn wait_until<T>(mut f: impl FnMut() -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(v) = f() {
                return v;
            }
            assert!(Instant::now() < deadline, "timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_missing_path_is_unusable() {
        let (_rt, h) = start(
            std::path::Path::new("/definitely/not/here"),
            Arc::new(AtomicBool::new(true)),
        );
        wait_for(&h, "unusable", |s| {
            matches!(s.repo, RepoState::Unusable { .. })
        });
    }

    /// `main` -> `feat`: a.txt edited and committed, then edited again; old.txt renamed to
    /// new.txt and committed; d.txt added and committed; u.txt untracked.
    fn branch_fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        git(p, &["config", "user.email", "t@example.com"]);
        git(p, &["config", "user.name", "t"]);
        std::fs::write(p.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(p.join("b.txt"), "b\n").unwrap();
        std::fs::write(p.join("old.txt"), "same\ncontent\nhere\n").unwrap();
        git(p, &["add", "-A"]);
        git(p, &["commit", "-q", "-m", "init"]);
        git(p, &["switch", "-q", "-c", "feat"]);
        std::fs::write(p.join("a.txt"), "one\nTWO\n").unwrap();
        std::fs::write(p.join("d.txt"), "d\n").unwrap();
        git(p, &["mv", "old.txt", "new.txt"]);
        git(p, &["add", "-A"]);
        git(p, &["commit", "-q", "-m", "feat"]);
        std::fs::write(p.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        std::fs::write(p.join("u.txt"), "u\n").unwrap();
        dir
    }

    fn start_with(
        dir: &std::path::Path,
        scope: Scope,
        state_dir: Option<std::path::PathBuf>,
        gate: Option<Arc<Semaphore>>,
    ) -> (tokio::runtime::Runtime, EngineHandle) {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let handle = spawn(
            rt.handle(),
            SessionConfig {
                path: dir.to_path_buf(),
                poll_interval: Duration::from_millis(50),
                watcher: Arc::new(FlakyWatcher {
                    allow: Arc::new(AtomicBool::new(true)),
                }),
                git_check: ok_git(),
                diff_delay: None,
                diff_gate: gate,
                status_delay: None,
                scope,
                base_ref: None,
                state_dir,
            },
        );
        (rt, handle)
    }

    fn git_out(dir: &std::path::Path, args: &[&str]) -> String {
        let out = Proc::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn rows_of(s: &Snapshot) -> Vec<(String, bool)> {
        s.files
            .iter()
            .map(|f| {
                (
                    f.path.clone(),
                    matches!(f.status, ChangedFileStatus::Untracked),
                )
            })
            .collect()
    }

    fn select_and_wait(h: &EngineHandle, path: &str, untracked: bool) -> Arc<Snapshot> {
        h.commands
            .send(Command::Select(FileKey {
                path: path.into(),
                staged: false,
                untracked,
            }))
            .unwrap();
        wait_for(h, &format!("diff of {path}"), |s| {
            ready(s)
                .map(|d| d.key.path == path && d.key.untracked == untracked)
                .unwrap_or(false)
        })
    }

    #[tokio::test]
    async fn a_stale_untracked_row_is_dropped_when_the_path_is_now_tracked() {
        let dir = branch_fixture();
        let toplevel = dir.path().to_string_lossy().into_owned();
        let baseline = base::verify(&toplevel, "refs/heads/main").await.unwrap();
        let status = git::git_status_inner(toplevel.clone()).await.unwrap();
        git(dir.path(), &["add", "u.txt"]);
        let rows = branch::rows(&toplevel, &baseline, status.files)
            .await
            .unwrap();
        let added: Vec<_> = rows.files.iter().filter(|f| f.path == "u.txt").collect();
        assert_eq!(added.len(), 1);
        assert!(matches!(added[0].status, ChangedFileStatus::Added));
    }

    #[test]
    fn branch_scope_lists_every_change_the_branch_carries_once() {
        let dir = branch_fixture();
        let (_rt, h) = start_with(dir.path(), Scope::Worktree, None, None);
        let s = wait_for(&h, "worktree rows", |s| ready(s).is_some());
        assert_eq!(
            rows_of(&s),
            [("a.txt".to_string(), false), ("u.txt".to_string(), true)]
        );
        assert_eq!(
            s.base.as_ref().map(|b| b.requested.as_str()),
            Some("refs/heads/main")
        );
        assert_eq!(s.base.as_ref().map(|b| b.source), Some(BaseSource::Default));
        assert_eq!(s.default_base.as_deref(), Some("refs/heads/main"));
        assert!(s.rename_sources.is_empty());

        h.commands.send(Command::SetScope(Scope::Branch)).unwrap();
        let s = wait_for(&h, "branch rows", |s| {
            s.scope == Scope::Branch && ready(s).is_some()
        });
        assert_eq!(
            rows_of(&s),
            [
                ("a.txt".to_string(), false),
                ("d.txt".to_string(), false),
                ("new.txt".to_string(), false),
                ("u.txt".to_string(), true)
            ]
        );
        let merge_base = git_out(dir.path(), &["merge-base", "HEAD", "refs/heads/main"])
            .trim()
            .to_string();
        assert_eq!(
            s.base.as_ref().unwrap().merge_base.as_deref(),
            Some(merge_base.as_str())
        );
        assert_eq!(
            s.rename_sources.get("new.txt").map(String::as_str),
            Some("old.txt")
        );
        let renamed = s.files.iter().find(|f| f.path == "new.txt").unwrap();
        assert!(matches!(renamed.status, ChangedFileStatus::Renamed));
        assert_eq!((renamed.insertions, renamed.deletions), (Some(0), Some(0)));

        // The committed-then-edited file shows both changes in one diff, equal to git's own.
        let a = ready(&s).unwrap();
        assert_eq!(a.key.path, "a.txt");
        assert_eq!(
            a.comparison,
            Comparison::Branch {
                merge_base: merge_base.clone()
            }
        );
        let expected = git_out(
            dir.path(),
            &[
                "diff",
                &merge_base,
                "--no-color",
                "--no-ext-diff",
                "--src-prefix=a/",
                "--dst-prefix=b/",
                "--",
                "a.txt",
            ],
        );
        assert_eq!(a.raw_diff, expected);
        let lines: Vec<&str> = a
            .file_diff
            .hunks
            .iter()
            .flat_map(|h| h.lines.iter().map(|l| l.content.as_str()))
            .collect();
        assert!(
            lines.contains(&"TWO") && lines.contains(&"three"),
            "{lines:?}"
        );

        let s = select_and_wait(&h, "new.txt", false);
        let d = ready(&s).unwrap();
        assert_eq!(d.file_diff.old_path.as_deref(), Some("old.txt"));
        let expected = git_out(
            dir.path(),
            &[
                "diff",
                &merge_base,
                "--no-color",
                "--no-ext-diff",
                "--src-prefix=a/",
                "--dst-prefix=b/",
                "-M",
                "--",
                "old.txt",
                "new.txt",
            ],
        );
        assert_eq!(d.raw_diff, expected);
        let s = select_and_wait(&h, "u.txt", true);
        let lines: Vec<&str> = ready(&s)
            .unwrap()
            .file_diff
            .hunks
            .iter()
            .flat_map(|h| h.lines.iter().map(|l| l.content.as_str()))
            .collect();
        assert_eq!(lines, ["u"]);

        // After a commit the worktree is clean and the branch rows are the same four paths.
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "more"]);
        let s = wait_for(&h, "u.txt became an added row", |s| {
            s.scope == Scope::Branch && rows_of(s).iter().any(|(p, u)| p == "u.txt" && !u)
        });
        assert_eq!(
            rows_of(&s),
            [
                ("a.txt".to_string(), false),
                ("d.txt".to_string(), false),
                ("new.txt".to_string(), false),
                ("u.txt".to_string(), false)
            ]
        );
        h.commands.send(Command::SetScope(Scope::Worktree)).unwrap();
        let s = wait_for(&h, "clean worktree", |s| {
            s.scope == Scope::Worktree && s.files.is_empty()
        });
        assert!(matches!(s.diff, DiffState::Idle));
    }

    #[test]
    fn a_path_deleted_on_the_branch_and_recreated_untracked_is_two_rows() {
        let dir = branch_fixture();
        git(dir.path(), &["rm", "-q", "b.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "drop b"]);
        std::fs::write(dir.path().join("b.txt"), "again\n").unwrap();
        let (_rt, h) = start_with(dir.path(), Scope::Branch, None, None);
        let s = wait_for(&h, "branch rows", |s| {
            s.scope == Scope::Branch && ready(s).is_some()
        });
        let b: Vec<(String, bool)> = rows_of(&s)
            .into_iter()
            .filter(|(p, _)| p == "b.txt")
            .collect();
        assert_eq!(
            b,
            [("b.txt".to_string(), false), ("b.txt".to_string(), true)]
        );
        let deleted = s
            .files
            .iter()
            .find(|f| f.path == "b.txt" && !matches!(f.status, ChangedFileStatus::Untracked))
            .unwrap();
        assert!(matches!(deleted.status, ChangedFileStatus::Deleted));
        let s = select_and_wait(&h, "b.txt", false);
        let lines: Vec<&str> = ready(&s)
            .unwrap()
            .file_diff
            .hunks
            .iter()
            .flat_map(|h| h.lines.iter().map(|l| l.content.as_str()))
            .collect();
        assert_eq!(
            lines,
            ["b"],
            "the deletion diff ignores the untracked content"
        );
        let s = select_and_wait(&h, "b.txt", true);
        let lines: Vec<&str> = ready(&s)
            .unwrap()
            .file_diff
            .hunks
            .iter()
            .flat_map(|h| h.lines.iter().map(|l| l.content.as_str()))
            .collect();
        assert_eq!(lines, ["again"]);
    }

    #[test]
    fn set_base_validates_loads_persists_and_switches_scope() {
        let dir = branch_fixture();
        git(dir.path(), &["branch", "other", "main"]);
        let state = tempfile::tempdir().unwrap();
        let (_rt, h) = start_with(
            dir.path(),
            Scope::Worktree,
            Some(state.path().to_path_buf()),
            None,
        );
        wait_for(&h, "first", |s| ready(s).is_some());

        h.commands
            .send(Command::SetBase(Some("nope".into())))
            .unwrap();
        let s = wait_for(&h, "rejected pick", |s| s.pick_seq == 1);
        assert_eq!(s.pick_error.as_deref(), Some("not a commit: nope"));
        assert_eq!(s.scope, Scope::Worktree);
        assert!(!state.path().join("bases.json").exists());

        h.commands
            .send(Command::SetBase(Some("refs/heads/other".into())))
            .unwrap();
        let s = wait_for(&h, "picked", |s| s.pick_seq == 2);
        assert!(s.pick_error.is_none());
        assert_eq!(s.scope, Scope::Branch);
        let base = s.base.as_ref().unwrap();
        assert_eq!(
            (base.requested.as_str(), base.source),
            ("refs/heads/other", BaseSource::Picked)
        );
        assert!(base.merge_base.is_some());
        assert_eq!(s.files.len(), 4);
        let toplevel = dir
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let picks: std::collections::BTreeMap<String, String> = serde_json::from_str(
            &std::fs::read_to_string(state.path().join("bases.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            picks.get(&toplevel).map(String::as_str),
            Some("refs/heads/other")
        );

        // A new session on the same worktree starts from the remembered pick.
        drop(h);
        let (_rt2, h) = start_with(
            dir.path(),
            Scope::Worktree,
            Some(state.path().to_path_buf()),
            None,
        );
        let s = wait_for(&h, "remembered", |s| s.base.is_some());
        assert_eq!(s.base.as_ref().unwrap().requested, "refs/heads/other");
        assert_eq!(s.default_base.as_deref(), Some("refs/heads/main"));

        h.commands.send(Command::SetBase(None)).unwrap();
        let s = wait_for(&h, "reset", |s| s.pick_seq == 1);
        assert!(s.pick_error.is_none());
        assert_eq!(
            s.scope,
            Scope::Branch,
            "picking the reset row keeps branch scope"
        );
        assert_eq!(
            s.base.as_ref().map(|b| (b.requested.as_str(), b.source)),
            Some(("refs/heads/main", BaseSource::Default))
        );
        let picks: std::collections::BTreeMap<String, String> = serde_json::from_str(
            &std::fs::read_to_string(state.path().join("bases.json")).unwrap(),
        )
        .unwrap();
        assert!(!picks.contains_key(&toplevel));
    }

    #[test]
    fn queued_base_picks_each_receive_an_answer_in_order() {
        let dir = branch_fixture();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let h = spawn(
            rt.handle(),
            SessionConfig {
                // Only the queued commands should contribute to the busy flag.
                poll_interval: Duration::from_secs(60),
                watcher: Arc::new(FlakyWatcher {
                    allow: Arc::new(AtomicBool::new(true)),
                }),
                git_check: ok_git(),
                ..SessionConfig::production(dir.path().to_path_buf())
            },
        );
        wait_for(&h, "first", |s| ready(s).is_some());
        let picks = ["missing-one", "missing-two", "missing-three"];
        for pick in picks {
            h.commands
                .send(Command::SetBase(Some(pick.to_string())))
                .unwrap();
        }
        for (index, pick) in picks.into_iter().enumerate() {
            let s = wait_for(&h, "pick answer", |s| s.pick_seq == index as u64 + 1);
            assert_eq!(s.pick_error, Some(format!("not a commit: {pick}")));
            assert_eq!(s.scope, Scope::Worktree);
            assert_eq!(s.refreshing, index + 1 < picks.len());
        }
    }

    #[test]
    fn a_queued_scope_change_can_return_to_the_current_scope() {
        let dir = branch_fixture();
        let (_rt, h) = start_with(dir.path(), Scope::Worktree, None, None);
        wait_for(&h, "first", |s| ready(s).is_some());
        h.commands.send(Command::SetScope(Scope::Branch)).unwrap();
        h.commands.send(Command::SetScope(Scope::Worktree)).unwrap();
        wait_for(&h, "branch", |s| s.scope == Scope::Branch);
        let s = wait_for(&h, "back to worktree", |s| s.scope == Scope::Worktree);
        assert_eq!(s.base.as_ref().unwrap().merge_base, None);
        assert!(s.rename_sources.is_empty());
    }

    #[test]
    fn a_pick_with_unrelated_history_is_reported_and_not_persisted() {
        let dir = branch_fixture();
        git(dir.path(), &["checkout", "-q", "--orphan", "lonely"]);
        git(
            dir.path(),
            &["commit", "-q", "--allow-empty", "-m", "lonely"],
        );
        git(dir.path(), &["switch", "-q", "feat"]);
        let state = tempfile::tempdir().unwrap();
        let (_rt, h) = start_with(
            dir.path(),
            Scope::Worktree,
            Some(state.path().to_path_buf()),
            None,
        );
        wait_for(&h, "first", |s| ready(s).is_some());
        h.commands
            .send(Command::SetBase(Some("refs/heads/lonely".into())))
            .unwrap();
        let s = wait_for(&h, "answered", |s| s.pick_seq == 1);
        assert!(
            s.pick_error
                .as_deref()
                .unwrap()
                .starts_with("no merge-base with "),
            "{:?}",
            s.pick_error
        );
        assert_eq!(s.scope, Scope::Worktree);
        assert_eq!(s.base.as_ref().unwrap().requested, "refs/heads/main");
        assert!(!state.path().join("bases.json").exists());
    }

    #[test]
    fn a_pick_that_cannot_be_remembered_is_kept_for_the_session() {
        let dir = branch_fixture();
        git(dir.path(), &["branch", "other", "main"]);
        let (_rt, h) = start_with(dir.path(), Scope::Worktree, None, None);
        wait_for(&h, "first", |s| ready(s).is_some());
        h.commands
            .send(Command::SetBase(Some("refs/heads/other".into())))
            .unwrap();
        let s = wait_for(&h, "picked", |s| s.pick_seq == 1);
        assert!(s.pick_error.is_none());
        assert!(
            s.base_error
                .as_deref()
                .unwrap()
                .starts_with("pick not remembered: "),
            "{:?}",
            s.base_error
        );
        h.commands.send(Command::Refresh).unwrap();
        wait_for(&h, "refreshed", |s| !s.refreshing && s.revision > 1);
        h.commands.send(Command::SetScope(Scope::Worktree)).unwrap();
        let s = wait_for(&h, "worktree", |s| s.scope == Scope::Worktree);
        assert_eq!(
            s.base.as_ref().unwrap().requested,
            "refs/heads/other",
            "r and scope switches keep the override"
        );
    }

    #[test]
    fn a_stale_diff_from_the_previous_comparison_is_discarded_and_the_key_reloaded() {
        let dir = branch_fixture();
        let gate = Arc::new(Semaphore::new(0));
        let (_rt, h) = start_with(dir.path(), Scope::Worktree, None, Some(gate.clone()));
        gate.add_permits(1);
        let s = wait_for(&h, "worktree diff", |s| ready(s).is_some());
        assert_eq!(ready(&s).unwrap().comparison, Comparison::Worktree);
        // D1: a worktree diff of a.txt blocked on the gate, requested by a refresh.
        h.commands.send(Command::Refresh).unwrap();
        wait_for(&h, "refresh in flight", |s| s.refreshing);
        // The scope switch changes the comparison while D1 is still blocked.
        h.commands.send(Command::SetScope(Scope::Branch)).unwrap();
        let s = wait_for(&h, "branch rows, diff loading", |s| {
            s.scope == Scope::Branch && matches!(s.diff, DiffState::Loading)
        });
        assert_eq!(
            s.selected.as_ref().map(|k| k.path.as_str()),
            Some("a.txt"),
            "the path survives the switch"
        );
        gate.add_permits(1); // releases D1, whose result must be discarded
        gate.add_permits(1); // releases the branch diff of a.txt
        let deadline = Instant::now() + Duration::from_secs(10);
        while h.diffs_discarded.load(Ordering::SeqCst) < 1 {
            assert!(Instant::now() < deadline, "stale diff was not discarded");
            std::thread::yield_now();
        }
        let s = wait_for(&h, "branch diff", |s| ready(s).is_some());
        assert!(matches!(
            ready(&s).unwrap().comparison,
            Comparison::Branch { .. }
        ));
        assert_eq!(ready(&s).unwrap().key.path, "a.txt");
        // No snapshot after the switch carries a worktree-comparison diff.
        while let Ok(s) = h.snapshots.try_recv() {
            assert!(ready(&s)
                .map(|d| d.comparison != Comparison::Worktree)
                .unwrap_or(true));
        }
    }

    #[test]
    fn a_scope_switch_re_resolves_and_sees_another_viewers_pick() {
        let dir = branch_fixture();
        git(dir.path(), &["branch", "other", "main"]);
        let state = tempfile::tempdir().unwrap();
        let (_rt, h) = start_with(
            dir.path(),
            Scope::Worktree,
            Some(state.path().to_path_buf()),
            None,
        );
        let s = wait_for(&h, "first", |s| ready(s).is_some());
        assert_eq!(s.base.as_ref().unwrap().requested, "refs/heads/main");
        // Another viewer on the same worktree saves a pick.
        let toplevel = dir
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        crate::engine::base::save_pick(state.path(), &toplevel, Some("refs/heads/other")).unwrap();
        h.commands.send(Command::SetScope(Scope::Branch)).unwrap();
        let s = wait_for(&h, "branch", |s| s.scope == Scope::Branch);
        assert_eq!(
            s.base.as_ref().map(|b| (b.requested.as_str(), b.source)),
            Some(("refs/heads/other", BaseSource::Picked))
        );
        // A default that vanished is published as `None`, so the reset row reads `default (none)`.
        crate::engine::base::save_pick(state.path(), &toplevel, None).unwrap();
        git(dir.path(), &["branch", "-D", "main"]);
        git(dir.path(), &["branch", "-D", "other"]);
        h.commands.send(Command::Refresh).unwrap();
        let s = wait_for(&h, "nothing resolves", |s| s.base.is_none());
        assert_eq!(s.default_base, None);
        assert_eq!(s.scope, Scope::Worktree);
    }

    #[test]
    fn load_refs_answers_on_the_snapshot_and_no_base_falls_back_to_worktree_with_the_notice() {
        let dir = branch_fixture();
        let (_rt, h) = start_with(dir.path(), Scope::Worktree, None, None);
        wait_for(&h, "first", |s| ready(s).is_some());
        h.commands.send(Command::LoadRefs(1)).unwrap();
        let s = wait_for(&h, "refs", |s| s.refs.is_some());
        let refs = s.refs.as_ref().unwrap();
        assert!(
            refs.contains(&"refs/heads/main".to_string())
                && refs.contains(&"refs/heads/feat".to_string())
        );
        assert!(!s.refs_overflow);
        assert_eq!(s.refs_seq, 1);
        assert!(s
            .quick
            .as_ref()
            .unwrap()
            .iter()
            .any(|q| q.submits == "HEAD~1"));
        h.commands.send(Command::LoadRefs(2)).unwrap();
        wait_for(&h, "second answer", |s| s.refs_seq == 2);

        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "dev"]);
        git(dir.path(), &["config", "user.email", "t@example.com"]);
        git(dir.path(), &["config", "user.name", "t"]);
        std::fs::write(dir.path().join("a.txt"), "a\n").unwrap();
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "init"]);
        std::fs::write(dir.path().join("a.txt"), "A\n").unwrap();
        let (_rt2, h) = start_with(dir.path(), Scope::Branch, None, None);
        let s = wait_for(&h, "fell back", |s| ready(s).is_some());
        assert_eq!(s.scope, Scope::Worktree);
        assert!(s.base.is_none());
        assert_eq!(s.base_error.as_deref(), Some(NO_BASE_NOTICE));
    }

    #[test]
    fn an_older_ref_reply_cannot_replace_a_newer_openings_list() {
        let dir = branch_fixture();
        let (_rt, h) = start_with(dir.path(), Scope::Worktree, None, None);
        wait_for(&h, "first", |s| ready(s).is_some());
        h.commands.send(Command::LoadRefs(2)).unwrap();
        let newest = wait_for(&h, "newer ref reply", |s| s.refs_seq == 2);
        h.commands.send(Command::LoadRefs(1)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while h.refs_answered.load(Ordering::SeqCst) != 2 {
            assert!(
                Instant::now() < deadline,
                "both ref replies were not handled"
            );
            std::thread::yield_now();
        }
        let latest = h
            .snapshots
            .try_iter()
            .last()
            .unwrap_or_else(|| newest.clone());
        assert_eq!(latest.refs_seq, 2, "an older ref reply replaced the token");
        assert!(Arc::ptr_eq(
            latest.refs.as_ref().unwrap(),
            newest.refs.as_ref().unwrap()
        ));
        assert!(Arc::ptr_eq(
            latest.quick.as_ref().unwrap(),
            newest.quick.as_ref().unwrap()
        ));
    }

    #[test]
    fn a_queued_plain_refresh_keeps_empty_rows_refreshing() {
        let dir = fixture();
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "clean"]);
        let state = tempfile::tempdir().unwrap();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let h = spawn(
            rt.handle(),
            SessionConfig {
                poll_interval: Duration::from_secs(60),
                state_dir: Some(state.path().to_path_buf()),
                watcher: Arc::new(FlakyWatcher {
                    allow: Arc::new(AtomicBool::new(true)),
                }),
                git_check: ok_git(),
                ..SessionConfig::production(dir.path().to_path_buf())
            },
        );
        wait_for(&h, "clean repository", |s| {
            matches!(s.repo, RepoState::Repo { .. }) && !s.refreshing
        });
        let before = h.refreshes.load(Ordering::SeqCst);
        // Hold persistence until the FIFO command queue has consumed the plain refresh.
        crate::actions::reuse::with_lock(state.path(), || {
            h.commands
                .send(Command::SetBase(Some("refs/heads/main".into())))
                .unwrap();
            h.commands.send(Command::Refresh).unwrap();
            h.commands.send(Command::LoadRefs(1)).unwrap();
            wait_for(&h, "commands consumed", |s| s.refs_seq == 1);
        })
        .unwrap();
        let picked = wait_for(&h, "pick reply", |s| s.pick_seq == 1);
        assert!(picked.pick_error.is_none());
        assert!(picked.selected.is_none() && picked.files.is_empty());
        assert!(
            picked.refreshing,
            "the queued plain refresh still has work to do"
        );
        wait_for(&h, "plain refresh finished", |s| {
            s.pick_seq == 1 && !s.refreshing
        });
        assert_eq!(h.refreshes.load(Ordering::SeqCst), before + 2);
    }

    #[test]
    fn a_base_pick_outside_a_repository_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let (_rt, h) = start_with(dir.path(), Scope::Worktree, None, None);
        h.commands
            .send(Command::SetBase(Some("HEAD".into())))
            .unwrap();
        let s = wait_for(&h, "failed pick", |s| s.pick_seq == 1);
        assert_eq!(s.pick_error.as_deref(), Some("not a git repository"));
        assert_eq!(s.scope, Scope::Worktree);
        assert!(s.base.is_none() && s.files.is_empty() && s.selected.is_none());
        assert!(matches!(s.repo, RepoState::NotARepo { .. }));
    }

    #[test]
    fn a_snapshot_carries_the_head_its_content_was_read_at() {
        let dir = branch_fixture();
        let (_rt, h) = start_with(dir.path(), Scope::Branch, None, None);
        let s = wait_for(&h, "first branch diff", |s| ready(s).is_some());
        let head = git_out(dir.path(), &["rev-parse", "HEAD"])
            .trim()
            .to_string();
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
        assert_eq!(
            s.head.as_deref(),
            Some(head.as_str()),
            "an empty list is markable"
        );

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
        // Block the first diff after its opening sample.
        let gate = Arc::new(Semaphore::new(0));
        let (_rt, h) = start_with(dir.path(), Scope::Branch, None, Some(gate.clone()));
        let deadline = Instant::now() + Duration::from_secs(10);
        while h.head_samples.load(Ordering::SeqCst) == 0 {
            assert!(
                Instant::now() < deadline,
                "no diff task reached its first sample"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        std::fs::write(dir.path().join("late.txt"), "late\n").unwrap();
        git(dir.path(), &["add", "late.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "late"]);
        gate.add_permits(4);
        let seen = wait_for(
            &h,
            "a diff with no id",
            |s| matches!(&s.diff, DiffState::Ready(d) if d.read_at.is_none()),
        );
        assert_eq!(
            seen.head, None,
            "a snapshot whose diff spanned a move is unmarkable"
        );
        // Once the tree settles, a later diff brackets cleanly and the id comes back.
        gate.add_permits(8);
        let settled = wait_for(
            &h,
            "a clean bracket",
            |s| matches!(&s.diff, DiffState::Ready(d) if d.read_at.is_some()),
        );
        assert!(settled.head.is_some());
    }

    #[test]
    fn a_sample_that_could_not_run_keeps_the_previous_id() {
        let mut snap = Snapshot::empty("/r");
        let previous = "a".repeat(40);
        let confirmed = "b".repeat(40);
        let rows = |id: &String| Some(id.clone());
        // An empty list gets its id from the sample.
        assert_eq!(
            markable(
                &snap,
                Some(previous.clone()),
                true,
                Some(confirmed.clone()),
                rows(&confirmed).as_deref()
            ),
            None,
            "sampled, and there is no commit"
        );
        snap.head_seen = Some(confirmed.clone());
        assert_eq!(
            markable(
                &snap,
                Some(previous.clone()),
                true,
                Some(confirmed.clone()),
                rows(&confirmed).as_deref()
            ),
            Some(confirmed.clone()),
            "a confirmed sample is what the empty list is marked at"
        );
        assert_eq!(
            markable(
                &snap,
                Some(previous.clone()),
                true,
                None,
                rows(&confirmed).as_deref()
            ),
            None,
            "sampled, but the two samples disagreed"
        );
        assert_eq!(
            markable(
                &snap,
                Some(previous.clone()),
                false,
                None,
                rows(&previous).as_deref()
            ),
            Some(previous.clone()),
            "the sample could not run: 8.6 keeps the previous id"
        );
        // The rows must answer for the same commit as the content.
        assert_eq!(
            markable(
                &snap,
                Some(previous.clone()),
                true,
                Some(confirmed.clone()),
                rows(&previous).as_deref()
            ),
            None,
            "rows from another commit cannot vouch for this one"
        );
        assert_eq!(
            markable(
                &snap,
                Some(previous.clone()),
                true,
                Some(confirmed.clone()),
                None
            ),
            None,
            "no rows yet, nothing to mark"
        );
        // A loading diff cannot vouch for an id.
        snap.diff = DiffState::Loading;
        assert_eq!(
            markable(
                &snap,
                Some(previous.clone()),
                false,
                None,
                rows(&previous).as_deref()
            ),
            None
        );
    }

    #[test]
    fn marking_writes_the_record_and_answers_on_its_own_channel() {
        let dir = branch_fixture();
        let state = tempfile::tempdir().unwrap();
        let (_rt, h) = start_with(
            dir.path(),
            Scope::Branch,
            Some(state.path().to_path_buf()),
            None,
        );
        let s = wait_for(&h, "first", |s| ready(s).is_some());
        let head = s.head.clone().expect("a markable id");

        h.commands
            .send(Command::MarkReviewed(head.clone()))
            .unwrap();
        let s = wait_for(&h, "answered", |s| s.mark_seq == 1);
        assert!(s.mark_error.is_none());
        assert_eq!(s.pick_seq, 0, "a mark is not a pick");
        let mark = s.mark.clone().expect("the mark is published");
        assert_eq!(mark.commit, head);
        assert!(mark.at > 1_600_000_000, "a real timestamp");
        assert_eq!(mark.state, crate::engine::MarkState::Current);
        assert_eq!(
            s.base.as_ref().map(|b| b.requested.as_str()),
            Some("refs/heads/main"),
            "the base is untouched"
        );

        let toplevel = dir
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let (marks, _) = crate::engine::base::load_marks(state.path());
        assert_eq!(
            marks.get(&toplevel).map(|m| m.commit.clone()),
            Some(head.clone())
        );
        assert!(
            !state.path().join("bases.json").exists(),
            "no base was written"
        );

        // A second mark replaces the record.
        std::fs::write(dir.path().join("more.txt"), "more\n").unwrap();
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "more"]);
        let s = wait_for(&h, "the new head is markable", |s| {
            s.head.as_deref().is_some_and(|h| h != head)
        });
        let head2 = s.head.clone().unwrap();
        h.commands
            .send(Command::MarkReviewed(head2.clone()))
            .unwrap();
        let s = wait_for(&h, "second answer", |s| s.mark_seq == 2);
        assert_eq!(
            s.mark.as_ref().map(|m| m.commit.clone()),
            Some(head2.clone())
        );
        let (marks, _) = crate::engine::base::load_marks(state.path());
        assert_eq!(marks.get(&toplevel).map(|m| m.commit.clone()), Some(head2));
    }

    #[test]
    fn a_repeated_mark_does_not_queue_twice() {
        let dir = branch_fixture();
        let state = tempfile::tempdir().unwrap();
        let (_rt, h) = start_with(
            dir.path(),
            Scope::Branch,
            Some(state.path().to_path_buf()),
            None,
        );
        let s = wait_for(&h, "first", |s| ready(s).is_some());
        let head = s.head.clone().unwrap();
        // Key repeat: the same commit arrives many times before the first is answered.
        for _ in 0..5 {
            h.commands
                .send(Command::MarkReviewed(head.clone()))
                .unwrap();
        }
        let s = wait_for(&h, "answered and settled", |s| {
            s.mark_seq >= 1 && !s.refreshing
        });
        assert_eq!(s.mark_seq, 1, "consecutive identical marks are one request");
        assert_eq!(s.mark.as_ref().map(|m| m.commit.clone()), Some(head));
    }

    #[test]
    fn a_mark_queued_behind_another_commit_is_not_coalesced() {
        let dir = branch_fixture();
        let state = tempfile::tempdir().unwrap();
        let (_rt, h) = start_with(
            dir.path(),
            Scope::Branch,
            Some(state.path().to_path_buf()),
            None,
        );
        let s = wait_for(&h, "first", |s| ready(s).is_some());
        let a = s.head.clone().unwrap();
        let b = git_out(dir.path(), &["rev-parse", "HEAD~1"])
            .trim()
            .to_string();
        // Changing one's mind and back: the last press must win, however the three land.
        for commit in [&a, &b, &a] {
            h.commands
                .send(Command::MarkReviewed(commit.clone()))
                .unwrap();
        }
        let s = wait_for(&h, "all three answered", |s| s.mark_seq == 3);
        assert_eq!(
            s.mark.as_ref().map(|m| m.commit.clone()),
            Some(a.clone()),
            "the final press decides the mark"
        );
        let (marks, _) = crate::engine::base::load_marks(state.path());
        let toplevel = dir
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(marks.get(&toplevel).map(|m| m.commit.clone()), Some(a));
    }

    #[test]
    fn a_mark_does_not_swallow_a_pending_resolution() {
        let dir = branch_fixture();
        let state = tempfile::tempdir().unwrap();
        let (_rt, h) = start_with(
            dir.path(),
            Scope::Branch,
            Some(state.path().to_path_buf()),
            None,
        );
        let s = wait_for(&h, "first", |s| ready(s).is_some());
        let a = s.head.clone().unwrap();
        let b = git_out(dir.path(), &["rev-parse", "HEAD~1"])
            .trim()
            .to_string();
        assert_eq!(
            s.base.as_ref().map(|base| base.requested.clone()),
            Some("refs/heads/main".to_string())
        );

        // Another viewer remembers a different base. `r` asks for the resolution steps; the
        // mark queued behind it must not consume that request and leave the pick unread.
        git(dir.path(), &["branch", "side", "HEAD~1"]);
        let toplevel = dir
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        base::save_pick(state.path(), &toplevel, Some("refs/heads/side")).unwrap();
        h.commands.send(Command::MarkReviewed(a.clone())).unwrap();
        h.commands.send(Command::Refresh).unwrap();
        h.commands.send(Command::MarkReviewed(b)).unwrap();
        // Both conditions in one predicate: which refresh carries the resolution depends on
        // whether the first mark has finished when `r` arrives, and on a loaded machine the
        // base can land while only one mark has been answered.
        wait_for(
            &h,
            "the remembered pick is resolved and both marks answered",
            |s| {
                s.base.as_ref().map(|base| base.requested.as_str()) == Some("refs/heads/side")
                    && s.mark_seq == 2
            },
        );

        // The same holds when the mark that carries the resolution cannot be verified. The
        // first mark's refresh is in flight, so `r` only records the request, and the failing
        // mark's refresh is the one that takes it: its rows never load, so the request must go
        // back rather than be consumed with them.
        git(dir.path(), &["branch", "later", "HEAD~1"]);
        base::save_pick(state.path(), &toplevel, Some("refs/heads/later")).unwrap();
        h.commands.send(Command::MarkReviewed(a)).unwrap();
        h.commands.send(Command::Refresh).unwrap();
        h.commands
            .send(Command::MarkReviewed("b".repeat(40)))
            .unwrap();
        let s = wait_for(&h, "the failed mark did not eat the resolution", |s| {
            s.base.as_ref().map(|base| base.requested.as_str()) == Some("refs/heads/later")
                && s.mark_seq == 4
        });
        assert_eq!(s.mark_error.as_deref(), Some("not a commit: bbbbbbb"));
    }

    #[test]
    fn a_mark_that_cannot_be_validated_or_written_says_so() {
        let dir = branch_fixture();
        let (_rt, h) = start_with(dir.path(), Scope::Branch, None, None);
        wait_for(&h, "first", |s| ready(s).is_some());

        // An id that is not a commit: answered, nothing written, previous mark untouched.
        h.commands
            .send(Command::MarkReviewed("b".repeat(40)))
            .unwrap();
        let s = wait_for(&h, "rejected", |s| s.mark_seq == 1);
        assert_eq!(s.mark_error.as_deref(), Some("not a commit: bbbbbbb"));
        assert!(s.mark.is_none());

        // With no state directory the mark holds for the session and says so.
        let head = s.head.clone().expect("a markable id");
        h.commands
            .send(Command::MarkReviewed(head.clone()))
            .unwrap();
        let s = wait_for(&h, "session mark", |s| s.mark_seq == 2);
        assert_eq!(
            s.mark.as_ref().map(|m| m.commit.clone()),
            Some(head.clone())
        );
        assert!(
            s.mark_error
                .as_deref()
                .unwrap()
                .starts_with("mark not remembered: "),
            "{:?}",
            s.mark_error
        );
        assert_eq!(s.pick_seq, 0, "a mark never answers on the pick channel");
        // It survives an ordinary refresh, like the session pick of 7.3.
        let revision = s.revision;
        h.commands.send(Command::Refresh).unwrap();
        let s = wait_for(&h, "after refresh", |s| {
            s.revision > revision && !s.refreshing
        });
        assert_eq!(
            s.mark.as_ref().map(|m| m.commit.clone()),
            Some(head.clone())
        );

        // Rejections preserve a previous mark and cannot panic on Unicode input.
        for (index, invalid) in ["--output=x", "猫猫猫猫"].into_iter().enumerate() {
            h.commands
                .send(Command::MarkReviewed(invalid.into()))
                .unwrap();
            let s = wait_for(&h, "rejected replacement", |s| {
                s.mark_seq == index as u64 + 3
            });
            assert_eq!(s.mark.as_ref().map(|m| &m.commit), Some(&head));
            assert_eq!(s.pick_seq, 0);
            assert_eq!(
                s.mark_error,
                Some(format!(
                    "not a commit: {}",
                    invalid.chars().take(7).collect::<String>()
                ))
            );
        }
    }

    #[test]
    fn marks_are_reloaded_on_resolution_and_failed_writes_keep_the_session_mark() {
        let dir = branch_fixture();
        let state = tempfile::tempdir().unwrap();
        let top = dir
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let previous = MarkRecord {
            commit: git_out(dir.path(), &["rev-parse", "HEAD~1"]).trim().into(),
            at: 1,
        };
        let current = MarkRecord {
            commit: git_out(dir.path(), &["rev-parse", "HEAD"]).trim().into(),
            at: 2,
        };
        base::save_mark(state.path(), &top, &previous).unwrap();
        let (_rt, h) = start_with(
            dir.path(),
            Scope::Worktree,
            Some(state.path().to_path_buf()),
            None,
        );
        let s = wait_for(&h, "stored mark", |s| ready(s).is_some());
        assert_eq!(s.mark.as_ref().map(|m| &m.commit), Some(&previous.commit));
        assert_eq!(s.mark.as_ref().unwrap().classified_at, None);

        base::save_mark(state.path(), &top, &current).unwrap();
        std::fs::write(dir.path().join("a.txt"), "polled edit\n").unwrap();
        let s = wait_for(&h, "polled edit", |s| {
            ready(s).is_some_and(|d| d.raw_diff.contains("polled edit"))
        });
        assert_eq!(s.mark.as_ref().map(|m| &m.commit), Some(&previous.commit));
        h.commands.send(Command::Refresh).unwrap();
        let s = wait_for(&h, "another viewer's mark", |s| {
            s.mark.as_ref().is_some_and(|m| m.at == 2)
        });
        assert_eq!(s.mark.as_ref().map(|m| &m.commit), Some(&current.commit));
        assert_eq!(s.mark_seq, 0);

        base::save_mark(state.path(), &top, &previous).unwrap();
        let lock = state.path().join("split-panes.lock");
        std::fs::remove_file(&lock).unwrap();
        std::fs::create_dir(&lock).unwrap();
        h.commands
            .send(Command::MarkReviewed(current.commit.clone()))
            .unwrap();
        let s = wait_for(&h, "failed save", |s| s.mark_seq == 1);
        assert!(s
            .mark_error
            .as_deref()
            .unwrap()
            .starts_with("mark not remembered: "));
        assert!(s.base_error.is_none());
        assert_eq!(s.scope, Scope::Worktree);
        assert_eq!(base::load_marks(state.path()).0.get(&top), Some(&previous));
        let revision = s.revision;
        h.commands.send(Command::Refresh).unwrap();
        let s = wait_for(&h, "session override", |s| {
            s.revision > revision && !s.refreshing
        });
        assert_eq!(s.mark.as_ref().map(|m| &m.commit), Some(&current.commit));

        std::fs::remove_dir(&lock).unwrap();
        h.commands
            .send(Command::MarkReviewed(current.commit.clone()))
            .unwrap();
        let s = wait_for(&h, "save recovered", |s| s.mark_seq == 2);
        assert!(s.mark_error.is_none());
        base::save_mark(state.path(), &top, &previous).unwrap();
        h.commands.send(Command::Refresh).unwrap();
        let s = wait_for(&h, "override cleared", |s| {
            s.mark.as_ref().is_some_and(|m| m.at == 1)
        });
        assert_eq!(s.mark.as_ref().map(|m| &m.commit), Some(&previous.commit));
        assert_eq!((s.mark_seq, s.pick_seq), (2, 0));

        std::fs::write(
            state.path().join("marks.json"),
            r#"{"/r/bad":{"commit":"--output=x","at":1}}"#,
        )
        .unwrap();
        h.commands.send(Command::Refresh).unwrap();
        wait_for(&h, "bad record absent", |s| s.mark.is_none());
        assert!(
            std::fs::read_to_string(state.path().join("config-problems.log"))
                .unwrap()
                .contains("marks.json: 1 unusable record(s)")
        );
    }

    #[test]
    fn queued_marks_answer_once_even_without_a_repository_or_a_successful_status() {
        for broken_status in [false, true] {
            let dir = if broken_status {
                branch_fixture()
            } else {
                tempfile::tempdir().unwrap()
            };
            let (_rt, h) = start_with(dir.path(), Scope::Worktree, None, None);
            wait_for(&h, "first", |s| {
                if broken_status {
                    ready(s).is_some()
                } else {
                    s.revision > 0
                }
            });
            if broken_status {
                std::fs::write(dir.path().join(".git/index"), "broken index").unwrap();
            }
            // Two consumed commands, each answered once. They are sent one after the other
            // rather than in a burst because identical marks coalesce while one is pending.
            for seq in 1..=2 {
                h.commands
                    .send(Command::MarkReviewed("b".repeat(40)))
                    .unwrap();
                let s = wait_for(&h, "mark error", |s| s.mark_seq == seq);
                assert!(s.mark.is_none());
                assert_eq!(s.pick_seq, 0);
                if broken_status {
                    assert_eq!(s.mark_error, s.status_error);
                    assert!(s
                        .mark_error
                        .as_deref()
                        .unwrap()
                        .starts_with("git status failed:"));
                } else {
                    assert_eq!(s.mark_error.as_deref(), Some("not a git repository"));
                }
            }
        }
    }

    #[test]
    fn the_unread_set_follows_the_mark_and_empties_on_a_new_one() {
        let dir = branch_fixture();
        let state = tempfile::tempdir().unwrap();
        let (_rt, h) = start_with(
            dir.path(),
            Scope::Branch,
            Some(state.path().to_path_buf()),
            None,
        );
        let s = wait_for(&h, "first", |s| ready(s).is_some());
        assert!(s.unread.is_empty(), "no mark, no dots");
        let head = s.head.clone().unwrap();

        h.commands.send(Command::MarkReviewed(head)).unwrap();
        let s = wait_for(&h, "marked", |s| s.mark_seq == 1);
        assert!(
            s.unread.is_empty(),
            "marking the head leaves nothing unread"
        );

        // Commit only fresh.txt, leaving a.txt edited and u.txt untracked.
        std::fs::write(dir.path().join("fresh.txt"), "fresh\n").unwrap();
        git(dir.path(), &["add", "fresh.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "fresh"]);
        // Wait for the new commit's diff before marking again.
        let fresh_head = git_out(dir.path(), &["rev-parse", "HEAD"])
            .trim()
            .to_string();
        let s = wait_for(&h, "the new commit is unread and drawn", |s| {
            s.unread.contains("fresh.txt") && s.head.as_deref() == Some(fresh_head.as_str())
        });
        assert!(!s.unread.contains("a.txt"), "{:?}", s.unread);
        assert!(
            !s.unread.contains("u.txt"),
            "untracked rows are never in the set"
        );
        assert_eq!(
            s.mark.as_ref().map(|m| m.state.clone()),
            Some(crate::engine::MarkState::Current)
        );

        // Uncommitted work raises nothing, and a second mark clears the set on a dirty tree.
        std::fs::write(dir.path().join("a.txt"), "one\nTWO\nthree\nfour\n").unwrap();
        let s = wait_for(&h, "the edit is listed", |s| {
            s.files.iter().any(|f| f.path == "a.txt")
                && ready(s).is_some_and(|d| d.raw_diff.contains("+four"))
                && s.head.as_deref() == Some(fresh_head.as_str())
        });
        assert!(
            !s.unread.contains("a.txt"),
            "an uncommitted edit is not a dot"
        );
        let head = s.head.clone().unwrap();
        h.commands.send(Command::MarkReviewed(head)).unwrap();
        let s = wait_for(&h, "marked again", |s| s.mark_seq == 2);
        assert!(
            s.unread.is_empty(),
            "M clears every dot even with a dirty worktree"
        );
    }

    #[test]
    fn a_scope_switch_and_back_keeps_the_dots() {
        let dir = branch_fixture();
        let state = tempfile::tempdir().unwrap();
        let (_rt, h) = start_with(
            dir.path(),
            Scope::Branch,
            Some(state.path().to_path_buf()),
            None,
        );
        let s = wait_for(&h, "first", |s| ready(s).is_some());
        h.commands
            .send(Command::MarkReviewed(s.head.clone().unwrap()))
            .unwrap();
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
        assert!(
            s.unread.contains("fresh.txt"),
            "the dots come back: {:?}",
            s.unread
        );
    }

    #[test]
    fn an_amended_mark_is_rewritten_and_one_mark_repairs_it() {
        let dir = branch_fixture();
        let state = tempfile::tempdir().unwrap();
        let (_rt, h) = start_with(
            dir.path(),
            Scope::Branch,
            Some(state.path().to_path_buf()),
            None,
        );
        let s = wait_for(&h, "first", |s| ready(s).is_some());
        h.commands
            .send(Command::MarkReviewed(s.head.clone().unwrap()))
            .unwrap();
        wait_for(&h, "marked", |s| s.mark_seq == 1);

        // A new message guarantees a different commit ID.
        git(dir.path(), &["commit", "-q", "--amend", "-m", "amended"]);
        let amended = git_out(dir.path(), &["rev-parse", "HEAD"])
            .trim()
            .to_string();
        let s = wait_for(&h, "rewritten", |s| {
            matches!(
                s.mark.as_ref().map(|m| m.state.clone()),
                Some(crate::engine::MarkState::Rewritten)
            ) && s.head.as_deref() == Some(amended.as_str())
        });
        assert!(
            s.unread.is_empty(),
            "the view flags every row from the state, not the set"
        );
        assert!(s.files.len() >= 4, "the rows and the base keep working");

        // Marking the pre-amend frame must still classify the stale commit as rewritten.
        let stale = s.mark.as_ref().unwrap().commit.clone();
        h.commands.send(Command::MarkReviewed(stale)).unwrap();
        let s = wait_for(&h, "stale frame marked", |s| s.mark_seq == 2);
        let mark = s.mark.as_ref().unwrap();
        assert_eq!(mark.state, MarkState::Rewritten);
        assert_eq!(mark.classified_at.as_deref(), Some(amended.as_str()));

        let head = s.head.clone().unwrap();
        h.commands.send(Command::MarkReviewed(head)).unwrap();
        let s = wait_for(&h, "repaired", |s| s.mark_seq == 3);
        assert_eq!(
            s.mark.as_ref().map(|m| m.state.clone()),
            Some(crate::engine::MarkState::Current)
        );
    }

    #[test]
    fn stored_marks_are_classified_on_arrival_without_changing_the_base() {
        let dir = branch_fixture();
        let state = tempfile::tempdir().unwrap();
        let top = dir
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let (_rt, h) = start_with(
            dir.path(),
            Scope::Branch,
            Some(state.path().to_path_buf()),
            None,
        );
        let initial = wait_for(&h, "first", |s| ready(s).is_some());
        let main = git_out(dir.path(), &["rev-parse", "refs/heads/main"])
            .trim()
            .to_string();
        for (commit, unreadable) in [("b".repeat(40), true), (main, false)] {
            base::save_mark(
                state.path(),
                &top,
                &MarkRecord {
                    commit: commit.clone(),
                    at: 1,
                },
            )
            .unwrap();
            h.commands.send(Command::Refresh).unwrap();
            let s = wait_for(&h, "stored mark classified", |s| {
                s.mark
                    .as_ref()
                    .is_some_and(|m| m.commit == commit && m.classified_at == s.head_seen)
            });
            assert_eq!(s.head_seen, initial.head_seen);
            assert_eq!(s.base, initial.base);
            assert_eq!(rows_of(&s), rows_of(&initial));
            assert!(s.status_error.is_none());
            assert_eq!(s.mark_seq, 0);
            if unreadable {
                assert!(matches!(&s.mark.as_ref().unwrap().state,
                    MarkState::Unreadable(reason) if reason.starts_with("git diff --name-only failed:")));
                assert!(s.unread.is_empty());
            } else {
                assert_eq!(s.mark.as_ref().unwrap().state, MarkState::Current);
                assert!(s.unread.contains("a.txt"));
                assert!(s.unread.contains("old.txt") && s.unread.contains("new.txt"));
            }
        }
    }

    use crate::engine::{ActionKind, WorktreeKind};

    /// mm.txt: two hunks staged, two unstaged. am.txt: staged creation plus an unstaged edit.
    /// del.txt: worktree deletion. sdel.txt and sdel2.txt: staged deletions. ren.txt -> renamed.txt
    /// staged with two hunks. u.txt untracked, empty.txt untracked and empty, bin.dat untracked and binary.
    fn action_fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        git(p, &["config", "user.email", "t@example.com"]);
        git(p, &["config", "user.name", "t"]);
        let ctx = "c1\nc2\nc3\nc4\nc5\nc6\nc7\n";
        let base = format!("alpha\n{ctx}beta\n{ctx}gamma\n{ctx}delta\n");
        std::fs::write(p.join("mm.txt"), &base).unwrap();
        std::fs::write(p.join("del.txt"), "d\n").unwrap();
        std::fs::write(p.join("sdel.txt"), "s\n").unwrap();
        std::fs::write(p.join("sdel2.txt"), "s2\n").unwrap();
        std::fs::write(p.join("ren.txt"), format!("one\n{ctx}two\n")).unwrap();
        git(p, &["add", "-A"]);
        git(p, &["commit", "-q", "-m", "init"]);
        std::fs::write(
            p.join("mm.txt"),
            format!("ALPHA\n{ctx}BETA\n{ctx}gamma\n{ctx}delta\n"),
        )
        .unwrap();
        git(p, &["add", "mm.txt"]);
        std::fs::write(
            p.join("mm.txt"),
            format!("ALPHA\n{ctx}BETA\n{ctx}GAMMA\n{ctx}DELTA\n"),
        )
        .unwrap();
        std::fs::write(p.join("am.txt"), "a\n").unwrap();
        git(p, &["add", "am.txt"]);
        std::fs::write(p.join("am.txt"), "a\nb\n").unwrap();
        std::fs::remove_file(p.join("del.txt")).unwrap();
        git(p, &["rm", "-q", "sdel.txt", "sdel2.txt"]);
        git(p, &["mv", "ren.txt", "renamed.txt"]);
        std::fs::write(p.join("renamed.txt"), format!("ONE\n{ctx}TWO\n")).unwrap();
        git(p, &["add", "renamed.txt"]);
        std::fs::write(p.join("u.txt"), "u\n").unwrap();
        std::fs::write(p.join("empty.txt"), "").unwrap();
        std::fs::write(p.join("bin.dat"), [0u8, 1, 2]).unwrap();
        dir
    }

    /// Like `start`, with the poll an hour away: only commands and the watcher move the engine.
    fn start_quiet(dir: &std::path::Path) -> (tokio::runtime::Runtime, EngineHandle) {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let handle = spawn(
            rt.handle(),
            SessionConfig {
                scope: Scope::Worktree,
                base_ref: None,
                state_dir: None,
                path: dir.to_path_buf(),
                poll_interval: Duration::from_secs(3600),
                watcher: Arc::new(FlakyWatcher {
                    allow: Arc::new(AtomicBool::new(true)),
                }),
                git_check: ok_git(),
                diff_delay: None,
                diff_gate: None,
                status_delay: None,
            },
        );
        (rt, handle)
    }

    /// Like `start_quiet`, with every refresh held `status_delay` before its status read.
    fn start_delayed(
        dir: &std::path::Path,
        status_delay: Duration,
    ) -> (tokio::runtime::Runtime, EngineHandle) {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let handle = spawn(
            rt.handle(),
            SessionConfig {
                scope: Scope::Worktree,
                base_ref: None,
                state_dir: None,
                path: dir.to_path_buf(),
                poll_interval: Duration::from_secs(3600),
                watcher: Arc::new(FlakyWatcher {
                    allow: Arc::new(AtomicBool::new(true)),
                }),
                git_check: ok_git(),
                diff_delay: None,
                diff_gate: None,
                status_delay: Some(status_delay),
            },
        );
        (rt, handle)
    }

    /// A gated session: every diff task waits for one permit before its git call.
    fn start_gated(
        dir: &std::path::Path,
        gate: Arc<Semaphore>,
    ) -> (tokio::runtime::Runtime, EngineHandle) {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let handle = spawn(
            rt.handle(),
            SessionConfig {
                scope: Scope::Worktree,
                base_ref: None,
                state_dir: None,
                path: dir.to_path_buf(),
                poll_interval: Duration::from_secs(3600),
                watcher: Arc::new(FlakyWatcher {
                    allow: Arc::new(AtomicBool::new(true)),
                }),
                git_check: ok_git(),
                diff_delay: None,
                diff_gate: Some(gate),
                status_delay: None,
            },
        );
        (rt, handle)
    }

    /// A `Refresh` first, so the listing is republished even when its last snapshot was already
    /// consumed (an unchanged repository publishes nothing); then a `Select` for a listed key.
    /// The refresh's own reload can publish the key first (the previous row vanished, or the key
    /// was already selected) and the `Select` then replaces that Arc, so the engine's last word
    /// is taken: the snapshot that still stands once nothing new has arrived for a while.
    fn select(h: &EngineHandle, path: &str, staged: bool, untracked: bool) -> Arc<Snapshot> {
        let key = FileKey {
            path: path.into(),
            staged,
            untracked,
        };
        h.commands.send(Command::Refresh).unwrap();
        wait_for(h, &format!("{key:?} listed"), |s| {
            s.files.iter().any(|f| FileKey::of(f) == key)
        });
        h.commands.send(Command::Select(key.clone())).unwrap();
        let settled = |s: &Snapshot| !s.refreshing && ready(s).is_some_and(|d| d.key == key);
        let mut last = wait_for(h, &format!("{key:?} ready"), settled);
        loop {
            std::thread::sleep(Duration::from_millis(150));
            let mut newer = None;
            while let Ok(s) = h.snapshots.try_recv() {
                newer = Some(s);
            }
            match newer {
                None => return last,
                Some(s) if settled(&s) => last = s,
                Some(_) => last = wait_for(h, &format!("{key:?} ready again"), settled),
            }
        }
    }

    /// Sends the action against the very Arc the snapshot holds and returns the snapshot that answers it.
    fn act(h: &EngineHandle, s: &Snapshot, kind: ActionKind, hunk: Option<usize>) -> Arc<Snapshot> {
        let diff = match &s.diff {
            DiffState::Ready(d) => d.clone(),
            _ => panic!("no ready diff"),
        };
        let seq = s.action_seq + 1;
        h.commands
            .send(Command::Act(Action { kind, diff, hunk }))
            .unwrap();
        wait_for(h, "the answer", |n| n.action_seq == seq)
    }

    /// The post-action rows and diff: waits until the refresh and the diff reload have settled.
    fn settled(h: &EngineHandle, path: &str, staged: bool) -> Arc<Snapshot> {
        wait_for(h, "settled", |s| {
            !s.refreshing && ready(s).is_some_and(|d| d.key.path == path && d.key.staged == staged)
        })
    }

    fn settled_any(h: &EngineHandle) -> Arc<Snapshot> {
        wait_for(h, "settled", |s| !s.refreshing && ready(s).is_some())
    }

    fn arc_of(s: &Snapshot) -> Arc<LoadedDiff> {
        match &s.diff {
            DiffState::Ready(d) => d.clone(),
            _ => unreachable!("no ready diff"),
        }
    }

    #[test]
    fn a_hunk_is_staged_unstaged_and_discarded_and_the_rest_is_untouched() {
        let dir = action_fixture();
        let p = dir.path();
        let (_rt, h) = start_quiet(p);
        let s = select(&h, "mm.txt", false, false);
        assert_eq!(ready(&s).unwrap().file_diff.hunks.len(), 2);
        let answered = act(&h, &s, ActionKind::Stage, Some(0));
        assert_eq!(answered.action_error, None);
        assert!(answered.action_applied);
        let cached = git_out(p, &["diff", "--cached", "--", "mm.txt"]);
        assert!(
            cached.contains("+GAMMA") && !cached.contains("+DELTA"),
            "{cached}"
        );
        let worktree = git_out(p, &["diff", "--", "mm.txt"]);
        assert!(
            !worktree.contains("+GAMMA") && worktree.contains("+DELTA"),
            "{worktree}"
        );
        // The unstaged row survives (DELTA); the diff on screen is reloaded.
        let s = settled(&h, "mm.txt", false);
        assert_eq!(ready(&s).unwrap().file_diff.hunks.len(), 1);
        // Unstage it back from the staged row: it is the third hunk there.
        let s = select(&h, "mm.txt", true, false);
        assert_eq!(ready(&s).unwrap().file_diff.hunks.len(), 3);
        let answered = act(&h, &s, ActionKind::Stage, Some(2));
        assert_eq!(answered.action_error, None);
        assert!(!git_out(p, &["diff", "--cached", "--", "mm.txt"]).contains("+GAMMA"));
        assert!(git_out(p, &["diff", "--", "mm.txt"]).contains("+GAMMA"));
        // Discard the unstaged GAMMA hunk: the file keeps DELTA and the staged hunks.
        let s = select(&h, "mm.txt", false, false);
        assert_eq!(ready(&s).unwrap().file_diff.hunks.len(), 2);
        let answered = act(&h, &s, ActionKind::Discard, Some(0));
        assert_eq!(answered.action_error, None);
        let text = std::fs::read_to_string(p.join("mm.txt")).unwrap();
        assert!(
            text.contains("gamma\n") && text.contains("DELTA\n") && text.starts_with("ALPHA\n"),
            "{text}"
        );
        assert!(git_out(p, &["diff", "--cached", "--", "mm.txt"]).contains("+BETA"));
    }

    #[test]
    fn a_staged_discard_is_atomic_and_refused_while_the_file_has_unstaged_changes() {
        let dir = action_fixture();
        let p = dir.path();
        let (_rt, h) = start_quiet(p);
        let s = select(&h, "mm.txt", true, false);
        let answered = act(&h, &s, ActionKind::Discard, Some(0));
        let error = answered.action_error.clone().unwrap();
        assert!(error.contains("does not match index"), "{error}");
        assert!(answered.action_applied, "git was asked and refused");
        assert!(git_out(p, &["diff", "--cached", "--", "mm.txt"]).contains("+ALPHA"));
        assert!(git_out(p, &["diff", "--", "mm.txt"]).contains("+GAMMA"));
        // A refused form on unchanged content still publishes a fresh Arc.
        let before = Arc::as_ptr(&arc_of(&s));
        let fresh = wait_for(
            &h,
            "a fresh arc",
            |n| matches!(&n.diff, DiffState::Ready(d) if d.key.staged && !std::ptr::eq(Arc::as_ptr(d), before)),
        );
        assert_eq!(ready(&fresh).unwrap().file_diff.hunks.len(), 2);
        // Stage the rest, then the discard is clean and removes the hunk from both sides.
        git(p, &["add", "mm.txt"]);
        let s = select(&h, "mm.txt", true, false);
        assert_eq!(ready(&s).unwrap().file_diff.hunks.len(), 4);
        let answered = act(&h, &s, ActionKind::Discard, Some(1));
        assert_eq!(answered.action_error, None);
        assert!(!git_out(p, &["diff", "--cached", "--", "mm.txt"]).contains("+BETA"));
        assert!(std::fs::read_to_string(p.join("mm.txt"))
            .unwrap()
            .contains("beta\n"));
        assert_eq!(
            git_out(p, &["status", "--porcelain=v1", "--", "mm.txt"]).trim(),
            "M  mm.txt"
        );
    }

    #[test]
    fn whole_file_discards_follow_the_row_and_an_md_path_keeps_its_file_absent() {
        let dir = action_fixture();
        let p = dir.path();
        let (_rt, h) = start_quiet(p);
        // D on the unstaged row of am.txt: the staged creation stays.
        let s = select(&h, "am.txt", false, false);
        assert_eq!(
            act(&h, &s, ActionKind::DiscardFile, None).action_error,
            None
        );
        assert_eq!(
            git_out(p, &["status", "--porcelain=v1", "--", "am.txt"]).trim(),
            "A  am.txt"
        );
        assert_eq!(std::fs::read_to_string(p.join("am.txt")).unwrap(), "a\n");
        // D on the staged creation: index entry and file both go.
        let s = settled(&h, "am.txt", true);
        assert_eq!(
            act(&h, &s, ActionKind::DiscardFile, None).action_error,
            None
        );
        assert_eq!(
            git_out(p, &["status", "--porcelain=v1", "--", "am.txt"]).trim(),
            ""
        );
        assert!(!p.join("am.txt").exists());
        // d on a worktree deletion recreates the file from the index.
        let s = select(&h, "del.txt", false, false);
        assert_eq!(act(&h, &s, ActionKind::Discard, Some(0)).action_error, None);
        assert_eq!(std::fs::read_to_string(p.join("del.txt")).unwrap(), "d\n");
        // D on a clean staged deletion: the index has no entry, so --index -R restores entry and file.
        let s = select(&h, "sdel.txt", true, false);
        assert_eq!(
            act(&h, &s, ActionKind::DiscardFile, None).action_error,
            None
        );
        assert_eq!(std::fs::read_to_string(p.join("sdel.txt")).unwrap(), "s\n");
        assert_eq!(
            git_out(p, &["status", "--porcelain=v1", "--", "sdel.txt"]).trim(),
            ""
        );
        // s on a staged deletion moves it to the worktree side; D on that unstaged row recreates the file.
        let s = select(&h, "sdel2.txt", true, false);
        assert_eq!(act(&h, &s, ActionKind::Stage, Some(0)).action_error, None);
        assert_eq!(
            git_out(p, &["status", "--porcelain=v1", "--", "sdel2.txt"]).trim(),
            "D sdel2.txt"
        );
        let s = select(&h, "sdel2.txt", false, false);
        assert_eq!(
            act(&h, &s, ActionKind::DiscardFile, None).action_error,
            None
        );
        assert_eq!(
            std::fs::read_to_string(p.join("sdel2.txt")).unwrap(),
            "s2\n"
        );
        // MD: staged edit, file removed from the worktree. D on the staged row must not recreate it.
        std::fs::write(p.join("md.txt"), "m\n").unwrap();
        git(p, &["add", "md.txt"]);
        git(p, &["commit", "-q", "-m", "md"]);
        std::fs::write(p.join("md.txt"), "M\n").unwrap();
        git(p, &["add", "md.txt"]);
        std::fs::remove_file(p.join("md.txt")).unwrap();
        let s = select(&h, "md.txt", true, false);
        let answered = act(&h, &s, ActionKind::DiscardFile, None);
        assert_eq!(
            answered.action_error.as_deref(),
            Some("md.txt: does not match index")
        );
        assert!(!answered.action_applied, "refused before git");
        assert!(!p.join("md.txt").exists(), "git would have recreated it");
        assert!(git_out(p, &["diff", "--cached", "--", "md.txt"]).contains("+M"));
    }

    #[test]
    fn untracked_rows_are_staged_or_deleted_whole_and_a_binary_one_is_refused() {
        let dir = action_fixture();
        let p = dir.path();
        let (_rt, h) = start_quiet(p);
        let s = select(&h, "u.txt", false, true);
        assert_eq!(act(&h, &s, ActionKind::Stage, None).action_error, None);
        assert_eq!(
            git_out(p, &["status", "--porcelain=v1", "--", "u.txt"]).trim(),
            "A  u.txt"
        );
        let s = select(&h, "empty.txt", false, true);
        assert_eq!(ready(&s).unwrap().file_diff.hunks.len(), 0);
        assert_eq!(act(&h, &s, ActionKind::Stage, None).action_error, None);
        assert_eq!(
            git_out(p, &["status", "--porcelain=v1", "--", "empty.txt"]).trim(),
            "A  empty.txt"
        );
        let s = select(&h, "bin.dat", false, true);
        let answered = act(&h, &s, ActionKind::DiscardFile, None);
        assert_eq!(
            answered.action_error.as_deref(),
            Some(actions::NOTICE_BINARY)
        );
        assert!(!answered.action_applied && p.join("bin.dat").exists());
        std::fs::write(p.join("gone.txt"), "g\n").unwrap();
        let s = select(&h, "gone.txt", false, true);
        assert_eq!(act(&h, &s, ActionKind::Discard, Some(0)).action_error, None);
        assert!(!p.join("gone.txt").exists());
        // The row is gone; the selection moved to a listed row.
        let s = settled_any(&h);
        assert!(s.files.iter().any(|f| Some(FileKey::of(f)) == s.selected));
    }

    #[test]
    fn one_hunk_of_a_staged_rename_is_unstaged_and_the_rename_stands() {
        let dir = action_fixture();
        let p = dir.path();
        let (_rt, h) = start_quiet(p);
        let s = select(&h, "renamed.txt", true, false);
        assert_eq!(
            ready(&s).unwrap().file_diff.old_path.as_deref(),
            Some("ren.txt")
        );
        assert_eq!(ready(&s).unwrap().file_diff.hunks.len(), 2);
        assert_eq!(act(&h, &s, ActionKind::Stage, Some(0)).action_error, None);
        assert_eq!(
            git_out(
                p,
                &["status", "--porcelain=v1", "--", "ren.txt", "renamed.txt"]
            )
            .trim(),
            "RM ren.txt -> renamed.txt"
        );
        assert!(git_out(p, &["diff", "--cached", "-M"]).contains("rename from ren.txt"));
        assert!(git_out(p, &["diff", "--", "renamed.txt"]).contains("+ONE"));
    }

    #[test]
    fn a_type_change_row_is_discarded_whole() {
        let dir = action_fixture();
        let p = dir.path();
        std::fs::remove_file(p.join("del.txt")).ok();
        std::os::unix::fs::symlink("u.txt", p.join("del.txt")).unwrap();
        let (_rt, h) = start_quiet(p);
        let s = select(&h, "del.txt", false, false);
        assert_eq!(
            ready(&s).unwrap().file_diff.hunks.len(),
            2,
            "a deletion and a creation"
        );
        assert_eq!(
            act(&h, &s, ActionKind::DiscardFile, None).action_error,
            None
        );
        assert!(p.join("del.txt").symlink_metadata().unwrap().is_file());
        assert_eq!(std::fs::read_to_string(p.join("del.txt")).unwrap(), "d\n");
    }

    #[test]
    fn every_act_is_answered_once_and_the_refusals_run_no_git() {
        let dir = action_fixture();
        let p = dir.path();
        let (_rt, h) = start_quiet(p);
        let s = select(&h, "mm.txt", false, false);
        // Out of range: the engine decides eligibility itself.
        let a = act(&h, &s, ActionKind::Stage, Some(9));
        assert_eq!(
            (a.action_seq, a.action_error.as_deref(), a.action_applied),
            (1, Some(actions::NOTICE_NO_HUNK), false)
        );
        // Scope: a diff really published in branch scope (the fixture's base is refs/heads/main).
        h.commands.send(Command::SetScope(Scope::Branch)).unwrap();
        let b = wait_for(&h, "branch diff", |n| {
            n.scope == Scope::Branch && ready(n).is_some() && !n.refreshing
        });
        let a = act(&h, &b, ActionKind::DiscardFile, None);
        assert_eq!(
            (a.action_seq, a.action_error.as_deref()),
            (2, Some(actions::NOTICE_SCOPE))
        );
        h.commands.send(Command::SetScope(Scope::Worktree)).unwrap();
        wait_for(&h, "worktree back", |n| {
            n.scope == Scope::Worktree && !n.refreshing
        });
        let s = select(&h, "mm.txt", false, false);
        let diff = arc_of(&s);
        // A stale Arc: a clone with the same content is not the published one.
        h.commands
            .send(Command::Act(Action {
                kind: ActionKind::Stage,
                diff: Arc::new((*diff).clone()),
                hunk: Some(0),
            }))
            .unwrap();
        let a = wait_for(&h, "stale answer", |n| n.action_seq == 3);
        assert_eq!(a.action_error.as_deref(), Some(actions::NOTICE_CHANGED));
        assert!(
            git_out(p, &["diff", "--cached", "--", "mm.txt"]).contains("+ALPHA")
                && !git_out(p, &["diff", "--cached"]).contains("+GAMMA")
        );
        // Two in a row: the second is answered without git while the first is queued or in flight.
        let first = Action {
            kind: ActionKind::Stage,
            diff: diff.clone(),
            hunk: Some(0),
        };
        h.commands.send(Command::Act(first.clone())).unwrap();
        h.commands
            .send(Command::Act(Action {
                hunk: Some(1),
                ..first
            }))
            .unwrap();
        let _ = wait_for(&h, "both answered", |n| n.action_seq == 5);
        // One of the two applied GAMMA, the other was refused as running; the index holds exactly one new hunk.
        let cached = git_out(p, &["diff", "--cached", "--", "mm.txt"]);
        assert!(
            cached.contains("+GAMMA") && !cached.contains("+DELTA"),
            "{cached}"
        );
    }

    #[test]
    fn an_acted_on_arc_is_refused_until_its_replacement_arrives() {
        let dir = action_fixture();
        let p = dir.path();
        let gate = Arc::new(Semaphore::new(0));
        let (_rt, h) = start_gated(p, gate.clone());
        gate.add_permits(1);
        wait_for(&h, "first", |s| ready(s).is_some());
        gate.add_permits(2); // `select` requests the diff twice: once for its Refresh, once for the Select
                             // A hunk of mm.txt's unstaged row: the row survives (DELTA stays), so 3.3 keeps Ready(old) on screen.
        let s = select(&h, "mm.txt", false, false);
        let diff = arc_of(&s);
        // The reload the carrying refresh requests waits at the gate, so the acted Arc stays published.
        h.commands
            .send(Command::Act(Action {
                kind: ActionKind::Stage,
                diff: diff.clone(),
                hunk: Some(0),
            }))
            .unwrap();
        let a = wait_for(&h, "answered", |n| n.action_seq == 1);
        assert_eq!((a.action_error.as_deref(), a.action_applied), (None, true));
        assert!(
            matches!(&a.diff, DiffState::Ready(d) if Arc::ptr_eq(d, &diff)),
            "the old Arc is still published"
        );
        h.commands
            .send(Command::Act(Action {
                kind: ActionKind::Stage,
                diff: diff.clone(),
                hunk: Some(0),
            }))
            .unwrap();
        let a = wait_for(&h, "the acted arc's answer", |n| n.action_seq == 2);
        assert_eq!(
            (a.action_error.as_deref(), a.action_applied),
            (Some(actions::NOTICE_CHANGED), false)
        );
        let cached = git_out(p, &["diff", "--cached", "--", "mm.txt"]);
        assert!(
            cached.contains("+GAMMA") && !cached.contains("+DELTA"),
            "staged once: {cached}"
        );
        gate.add_permits(1);
        wait_for(
            &h,
            "a fresh arc",
            |n| matches!(&n.diff, DiffState::Ready(d) if !Arc::ptr_eq(d, &diff)),
        );
    }

    #[test]
    fn a_second_act_while_one_is_queued_is_answered_without_git() {
        let dir = action_fixture();
        let p = dir.path();
        let gate = Arc::new(Semaphore::new(0));
        let (_rt, h) = start_gated(p, gate.clone());
        gate.add_permits(1);
        wait_for(&h, "first", |s| ready(s).is_some());
        gate.add_permits(2); // `select` requests the diff twice: once for its Refresh, once for the Select
        let s = select(&h, "mm.txt", false, false);
        let diff = arc_of(&s);
        // No permit is left: the first action's reload will wait at the gate, which holds it in flight.
        let before = h.refreshes.load(Ordering::SeqCst);
        h.commands
            .send(Command::Act(Action {
                kind: ActionKind::Stage,
                diff: diff.clone(),
                hunk: Some(0),
            }))
            .unwrap();
        h.commands
            .send(Command::Act(Action {
                kind: ActionKind::Stage,
                diff: diff.clone(),
                hunk: Some(1),
            }))
            .unwrap();
        let second = wait_for(&h, "the running refusal", |n| {
            n.action_error.as_deref() == Some(actions::NOTICE_RUNNING)
        });
        assert_eq!(
            second.action_seq, 1,
            "the refusal is answered first, without a refresh"
        );
        gate.add_permits(1);
        let first = wait_for(&h, "the first answer", |n| n.action_seq == 2);
        assert_eq!(first.action_error, None);
        assert!(h.refreshes.load(Ordering::SeqCst) - before >= 1);
    }

    #[test]
    fn a_pre_image_that_changed_refuses_the_form_before_git() {
        let dir = action_fixture();
        let p = dir.path();
        let (_rt, h) = start_quiet(p);
        let s = select(&h, "mm.txt", false, false);
        // The watcher never emits and the poll is an hour away (`start_quiet`): the engine does not see this edit.
        let ctx = "c1\nc2\nc3\nc4\nc5\nc6\nc7\n";
        std::fs::write(
            p.join("mm.txt"),
            format!("ALPHA\n{ctx}BETA\n{ctx}GAMMA\n{ctx}DELTA\nextra\n"),
        )
        .unwrap();
        let a = act(&h, &s, ActionKind::Discard, Some(0));
        assert_eq!(a.action_error.as_deref(), Some(actions::NOTICE_CHANGED));
        assert!(!a.action_applied);
        assert!(std::fs::read_to_string(p.join("mm.txt"))
            .unwrap()
            .contains("GAMMA\n"));
        // A `--cached` form reads the index: an index change refuses it the same way.
        let s = select(&h, "mm.txt", false, false);
        assert!(ready(&s).unwrap().raw_diff.contains("+extra"));
        git(p, &["add", "mm.txt"]);
        let a = act(&h, &s, ActionKind::Stage, Some(0));
        assert_eq!(a.action_error.as_deref(), Some(actions::NOTICE_CHANGED));
    }

    #[test]
    fn a_working_tree_form_on_a_directory_is_refused() {
        let dir = action_fixture();
        let p = dir.path();
        std::fs::write(p.join("tools"), "x\n").unwrap();
        git(p, &["add", "tools"]);
        git(p, &["commit", "-q", "-m", "tools"]);
        git(p, &["rm", "-q", "tools"]);
        std::fs::create_dir(p.join("tools")).unwrap();
        std::fs::write(p.join("tools/run"), "r\n").unwrap();
        git(p, &["add", "tools/run"]);
        let (_rt, h) = start_quiet(p);
        let s = select(&h, "tools", true, false);
        assert_eq!(
            ready(&s)
                .unwrap()
                .pre_image
                .as_ref()
                .map(|i| i.worktree.clone()),
            Some(WorktreeKind::Directory)
        );
        let a = act(&h, &s, ActionKind::DiscardFile, None);
        assert_eq!(a.action_error.as_deref(), Some(actions::NOTICE_NOT_A_FILE));
        assert!(!a.action_applied && p.join("tools/run").exists());
        // The index-only form needs no file, but git refuses an index holding both `tools2` and a
        // descendant, so the restore is shown on a fixture whose descendant is untracked.
        std::fs::write(p.join("tools2"), "y\n").unwrap();
        git(p, &["add", "tools2"]);
        git(p, &["commit", "-q", "-m", "tools2"]);
        git(p, &["rm", "-q", "tools2"]);
        std::fs::create_dir(p.join("tools2")).unwrap();
        std::fs::write(p.join("tools2/run"), "r\n").unwrap();
        let s = select(&h, "tools2", true, false);
        assert_eq!(act(&h, &s, ActionKind::Stage, Some(0)).action_error, None);
        assert!(git_out(p, &["ls-files", "--", "tools2"]).trim() == "tools2");
    }

    #[test]
    fn an_edit_during_the_diff_read_leaves_no_pre_image_and_refuses_the_key() {
        let dir = action_fixture();
        let p = dir.path();
        let gate = Arc::new(Semaphore::new(0));
        let (_rt, h) = start_gated(p, gate.clone());
        gate.add_permits(1);
        wait_for(&h, "first", |s| ready(s).is_some());
        gate.add_permits(2);
        let s = select(&h, "u.txt", false, true);
        assert!(ready(&s).unwrap().pre_image.is_some());
        // The reload takes its first pre-image reading, then waits at the gate before the diff:
        // the edit lands between the two readings, provably.
        let readings = h.pre_images.load(Ordering::SeqCst);
        h.commands.send(Command::Refresh).unwrap();
        wait_until(|| (h.pre_images.load(Ordering::SeqCst) > readings).then_some(()));
        std::fs::write(p.join("u.txt"), "u\nmore\n").unwrap();
        gate.add_permits(1);
        let s = wait_for(&h, "the reload", |n| {
            !n.refreshing && ready(n).is_some_and(|d| d.raw_diff.contains("+more"))
        });
        assert!(
            ready(&s).unwrap().pre_image.is_none(),
            "the two readings disagreed"
        );
        let a = act(&h, &s, ActionKind::Stage, None);
        assert_eq!(
            (a.action_error.as_deref(), a.action_applied),
            (Some(actions::NOTICE_CHANGED), false)
        );
        // The next reload, with nothing moving, carries a pre-image again. Three permits: the
        // refused action's carrying refresh reloads the row too, before `select`'s two.
        gate.add_permits(3);
        let s = select(&h, "u.txt", false, true);
        assert!(ready(&s).unwrap().pre_image.is_some());
        gate.add_permits(1); // the carrying refresh's reload
        assert_eq!(act(&h, &s, ActionKind::Stage, None).action_error, None);
    }

    #[test]
    fn an_index_lock_is_retried_and_given_up_after_two_seconds() {
        let dir = action_fixture();
        let p = dir.path();
        let (_rt, h) = start_quiet(p);
        let s = select(&h, "u.txt", false, true);
        // Held for half a second: the retry absorbs it.
        std::fs::write(p.join(".git/index.lock"), "").unwrap();
        let lock = p.join(".git/index.lock");
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            std::fs::remove_file(lock).unwrap();
        });
        let started = Instant::now();
        let a = act(&h, &s, ActionKind::Stage, None);
        release.join().unwrap();
        assert_eq!(a.action_error, None);
        assert!(
            started.elapsed() >= Duration::from_millis(400),
            "the first attempt must have hit the lock"
        );
        assert!(git_out(p, &["ls-files", "--", "u.txt"]).contains("u.txt"));
        // Held past the bound: git's own message, after two seconds.
        std::fs::write(p.join("v.txt"), "v\n").unwrap();
        let s = select(&h, "v.txt", false, true);
        std::fs::write(p.join(".git/index.lock"), "").unwrap();
        let started = Instant::now();
        let a = act(&h, &s, ActionKind::Stage, None);
        std::fs::remove_file(p.join(".git/index.lock")).unwrap();
        assert!(
            a.action_error
                .as_deref()
                .is_some_and(|e| e.contains("index.lock")),
            "{:?}",
            a.action_error
        );
        assert!(a.action_applied);
        assert!(started.elapsed() >= Duration::from_secs(2));
    }

    #[test]
    fn a_diff_read_before_the_action_is_never_published_after_it() {
        let dir = action_fixture();
        let p = dir.path();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let h = spawn(
            rt.handle(),
            SessionConfig {
                scope: Scope::Worktree,
                base_ref: None,
                state_dir: None,
                path: p.to_path_buf(),
                poll_interval: Duration::from_secs(3600),
                watcher: Arc::new(FlakyWatcher {
                    allow: Arc::new(AtomicBool::new(true)),
                }),
                git_check: ok_git(),
                diff_delay: Some(Duration::from_millis(400)),
                diff_gate: None,
                status_delay: None,
            },
        );
        let s = select(&h, "u.txt", false, true);
        let diff = arc_of(&s);
        let discarded_before = h.diffs_discarded.load(Ordering::SeqCst);
        // A slow reload of the row is in flight when the action is queued.
        h.commands.send(Command::Refresh).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        h.commands
            .send(Command::Act(Action {
                kind: ActionKind::Stage,
                diff,
                hunk: None,
            }))
            .unwrap();
        let answered = wait_for(&h, "answered", |n| n.action_seq == 1);
        assert_eq!(answered.action_error, None);
        let after = wait_for(&h, "the post-action diff", |n| {
            !n.refreshing && ready(n).is_some_and(|d| d.key.path == "u.txt" && d.key.staged)
        });
        assert!(
            h.diffs_discarded.load(Ordering::SeqCst) > discarded_before,
            "the pre-action read was published"
        );
        assert!(ready(&after).unwrap().raw_diff.contains("+u"));
    }

    #[test]
    fn an_act_outside_a_repository_is_answered_not_a_git_repository() {
        let dir = tempfile::tempdir().unwrap();
        let (_rt, h) = start_quiet(dir.path());
        let s = wait_for(&h, "not a repo", |s| {
            matches!(s.repo, RepoState::NotARepo { .. })
        });
        let diff = Arc::new(LoadedDiff::build(
            FileKey {
                path: "x".into(),
                staged: false,
                untracked: true,
            },
            Comparison::Worktree,
            None,
            crate::git::GetGitDiffResponse {
                file_diff: crate::git::FileDiff {
                    file_path: "x".into(),
                    old_path: None,
                    new_path: None,
                    hunks: Vec::new(),
                },
                old_text: String::new(),
                new_text: String::new(),
                raw_diff: String::new(),
                repo_root: String::new(),
            },
            Vec::new(),
            None,
        ));
        h.commands
            .send(Command::Act(Action {
                kind: ActionKind::Stage,
                diff,
                hunk: None,
            }))
            .unwrap();
        let a = wait_for(&h, "answer", |n| n.action_seq == 1);
        assert_eq!(
            a.action_error.as_deref(),
            Some("the diff changed; look again"),
            "no diff is published, so identity fails first; {}",
            s.revision
        );
    }

    #[test]
    fn a_mark_and_an_action_queued_together_answer_in_order() {
        let dir = action_fixture();
        let p = dir.path();
        let (_rt, h) = start_quiet(p);
        let s = select(&h, "u.txt", false, true);
        let head = git_out(p, &["rev-parse", "HEAD"]).trim().to_string();
        let diff = arc_of(&s);
        h.commands.send(Command::MarkReviewed(head)).unwrap();
        h.commands
            .send(Command::Act(Action {
                kind: ActionKind::Stage,
                diff,
                hunk: None,
            }))
            .unwrap();
        let s = wait_for(&h, "both", |n| n.mark_seq == 1 && n.action_seq == 1);
        assert_eq!(s.action_error, None);
        assert_eq!(
            s.mark_error.as_deref(),
            Some("mark not remembered: no state directory")
        );
    }

    #[test]
    fn the_watchers_trigger_for_the_actions_writes_coalesces() {
        let dir = action_fixture();
        let p = dir.path();
        let slot = Arc::new(std::sync::Mutex::new(None));
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let h = spawn(
            rt.handle(),
            SessionConfig {
                scope: Scope::Worktree,
                base_ref: None,
                state_dir: None,
                path: p.to_path_buf(),
                poll_interval: Duration::from_secs(3600),
                watcher: Arc::new(EmittingWatcher { sink: slot.clone() }),
                git_check: ok_git(),
                diff_delay: None,
                diff_gate: None,
                // Every refresh, the carrying one included, stays in flight 400 ms after its forms.
                status_delay: Some(Duration::from_millis(400)),
            },
        );
        let s = select(&h, "u.txt", false, true);
        let sink = wait_until(|| slot.lock().unwrap().clone());
        let before = h.refreshes.load(Ordering::SeqCst);
        let diff = arc_of(&s);
        h.commands
            .send(Command::Act(Action {
                kind: ActionKind::Stage,
                diff,
                hunk: None,
            }))
            .unwrap();
        // The carrying refresh has started and is held: the events land while it is in flight.
        wait_until(|| (h.refreshes.load(Ordering::SeqCst) > before).then_some(()));
        for _ in 0..5 {
            sink.emit_json("git-status-changed", serde_json::json!({ "cwds": [] }))
                .unwrap();
        }
        let answered = wait_for(&h, "answered", |n| n.action_seq == 1);
        assert_eq!(answered.action_error, None);
        let settled = wait_for(&h, "settled", |n| !n.refreshing && n.action_seq == 1);
        std::thread::sleep(Duration::from_millis(600)); // long enough for any extra refresh to start
        let runs = h.refreshes.load(Ordering::SeqCst) - before;
        assert_eq!(
            runs, 2,
            "the carrying refresh and exactly one follow-up for five events"
        );
        assert_eq!(settled.action_seq, 1, "answered once");
    }

    #[test]
    fn a_selection_during_a_carried_action_waits_for_the_answer() {
        let dir = action_fixture();
        let p = dir.path();
        let (_rt, h) = start_delayed(p, Duration::from_millis(400));
        let s = select(&h, "mm.txt", false, false);
        let diff = arc_of(&s);
        let key = diff.key.clone();
        h.commands
            .send(Command::Act(Action {
                kind: ActionKind::Discard,
                diff: diff.clone(),
                hunk: Some(0),
            }))
            .unwrap();
        h.commands.send(Command::Select(key)).unwrap();
        // No diff task runs while the action is carried: until the answer, the only diff on
        // screen is the Arc the action was sent against or `Loading`; the answer itself carries
        // `Loading`, because the reload the carrying refresh requests comes after it.
        let a = wait_for(&h, "the answer", |n| {
            match &n.diff {
                DiffState::Loading => {}
                DiffState::Ready(d) if Arc::ptr_eq(d, &diff) => {}
                other => panic!("a diff read during the action reached the screen: {other:?}"),
            }
            n.action_seq == 1
        });
        assert_eq!(a.action_error, None);
        assert!(
            matches!(a.diff, DiffState::Loading),
            "the answer must precede the reload"
        );
        // The first diff published after the answer was read after the mutation.
        let s = settled(&h, "mm.txt", false);
        let d = ready(&s).unwrap();
        assert!(!d.raw_diff.contains("+GAMMA"), "{}", d.raw_diff);
        assert_eq!(d.file_diff.hunks.len(), 1);
        // Staging from it stages what is really there, never the discarded hunk.
        let answered = act(&h, &s, ActionKind::Stage, Some(0));
        assert_eq!(answered.action_error, None);
        let cached = git_out(p, &["diff", "--cached", "--", "mm.txt"]);
        assert!(
            cached.contains("+DELTA") && !cached.contains("+GAMMA"),
            "{cached}"
        );
    }

    #[test]
    fn one_hunk_of_an_unstaged_rename_is_staged_with_the_rename() {
        let dir = fixture();
        let p = dir.path();
        let ctx = "c1\nc2\nc3\nc4\nc5\nc6\nc7\n";
        std::fs::write(p.join("old.txt"), format!("one\n{ctx}two\n")).unwrap();
        git(p, &["add", "old.txt"]);
        git(p, &["commit", "-q", "-m", "old"]);
        std::fs::rename(p.join("old.txt"), p.join("new.txt")).unwrap();
        git(p, &["add", "-N", "new.txt"]);
        std::fs::write(p.join("new.txt"), format!("ONE\n{ctx}TWO\n")).unwrap();
        let (_rt, h) = start_quiet(p);
        let s = select(&h, "new.txt", false, false);
        assert_eq!(
            ready(&s).unwrap().file_diff.old_path.as_deref(),
            Some("old.txt")
        );
        assert_eq!(ready(&s).unwrap().file_diff.hunks.len(), 2);
        assert_eq!(act(&h, &s, ActionKind::Stage, Some(0)).action_error, None);
        assert_eq!(
            git_out(p, &["status", "--porcelain=v1", "--", "old.txt", "new.txt"]).trim(),
            "RM old.txt -> new.txt"
        );
        let cached = git_out(p, &["diff", "--cached", "-M"]);
        assert!(
            cached.contains("rename from old.txt")
                && cached.contains("+ONE")
                && !cached.contains("+TWO"),
            "{cached}"
        );
        assert!(git_out(p, &["diff", "--", "new.txt"]).contains("+TWO"));
    }

    #[test]
    fn a_newer_selection_survives_the_acted_rows_disappearance() {
        let dir = action_fixture();
        let p = dir.path();
        let (_rt, h) = start_delayed(p, Duration::from_millis(400));
        let s = select(&h, "am.txt", false, false);
        assert_eq!(ready(&s).unwrap().file_diff.hunks.len(), 1);
        let diff = arc_of(&s);
        let acted = diff.key.clone();
        h.commands
            .send(Command::Act(Action {
                kind: ActionKind::Stage,
                diff,
                hunk: Some(0),
            }))
            .unwrap();
        let other = FileKey {
            path: "mm.txt".into(),
            staged: true,
            untracked: false,
        };
        h.commands.send(Command::Select(other.clone())).unwrap();
        let a = wait_for(&h, "the answer", |n| n.action_seq == 1);
        assert_eq!(a.action_error, None);
        assert!(
            !a.files.iter().any(|f| FileKey::of(f) == acted),
            "the acted row is gone"
        );
        assert_eq!(
            a.selected.as_ref(),
            Some(&other),
            "the newer selection stands"
        );
    }

    #[test]
    fn a_changed_rename_source_entry_refuses_the_forward_form_before_git() {
        let dir = fixture();
        let p = dir.path();
        let ctx = "c1\nc2\nc3\nc4\nc5\nc6\nc7\n";
        std::fs::write(p.join("old.txt"), format!("one\n{ctx}two\n")).unwrap();
        git(p, &["add", "old.txt"]);
        git(p, &["commit", "-q", "-m", "old"]);
        std::fs::rename(p.join("old.txt"), p.join("new.txt")).unwrap();
        git(p, &["add", "-N", "new.txt"]);
        std::fs::write(p.join("new.txt"), format!("ONE\n{ctx}TWO\n")).unwrap();
        let (_rt, h) = start_quiet(p);
        let s = select(&h, "new.txt", false, false);
        assert_eq!(
            ready(&s).unwrap().file_diff.old_path.as_deref(),
            Some("old.txt")
        );
        // The engine is quiet: the source's index entry changes under it, unseen.
        let blob = {
            let out = Proc::new("git")
                .arg("-C")
                .arg(p)
                .args(["hash-object", "-w", "--stdin"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            use std::io::Write;
            let mut child = out;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(format!("one\n{ctx}changed\n").as_bytes())
                .unwrap();
            let out = child.wait_with_output().unwrap();
            assert!(out.status.success());
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        git(
            p,
            &[
                "update-index",
                "--cacheinfo",
                &format!("100644,{blob},old.txt"),
            ],
        );
        let a = act(&h, &s, ActionKind::Stage, Some(0));
        assert_eq!(
            (a.action_error.as_deref(), a.action_applied),
            (Some(actions::NOTICE_CHANGED), false)
        );
        // The index carries the harness's own edit of old.txt and nothing of the action.
        let cached = git_out(p, &["diff", "--cached", "-M"]);
        assert!(
            !cached.contains("new.txt")
                && !cached.contains("rename from")
                && !cached.contains("+ONE"),
            "{cached}"
        );
        assert_eq!(
            git_out(p, &["status", "--porcelain=v1", "--", "new.txt"]).trim_end(),
            " A new.txt"
        );
    }
}
