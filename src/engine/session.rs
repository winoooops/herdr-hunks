//! Non-blocking repository sessions and immutable snapshots.
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::Semaphore;

use super::base::{self, ResolveInputs};
use super::{
    branch, gitver, Base, BaseSource, Command, Comparison, DiffState, FileKey, LoadedDiff,
    RepoState, Scope, Snapshot, NO_BASE_NOTICE,
};
use crate::git::{self, ChangedFile, GetGitDiffResponse, GitStatusResponse};
use crate::runtime::EventSink;
use std::collections::{BTreeMap, VecDeque};

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
}

pub struct EngineHandle {
    pub commands: UnboundedSender<Command>,
    pub snapshots: mpsc::Receiver<Arc<Snapshot>>,
    /// Status refreshes started by this session.
    pub refreshes: Arc<AtomicUsize>,
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
    change: Option<Change>,
}

/// What a refresh loaded, published only as a whole.
struct Loaded {
    scope: Scope,
    base: Option<Base>,
    default_base: Option<String>,
    base_error: Option<String>,
    files: Vec<ChangedFile>,
    rename_sources: BTreeMap<String, String>,
    /// `Some` after a pick or reset: whether `bases.json` took it.
    persisted: Option<Result<(), String>>,
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
    /// Run the resolution steps in the next refresh (start, `r`, and after a pick).
    resolve_pending: bool,
    changes: VecDeque<Change>,
    pick_seq: u64,
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

enum Done {
    GitCheck(Result<gitver::GitVersion, gitver::GitCheckError>),
    Watcher(Result<(), String>),
    Status {
        response: Result<GitStatusResponse, String>,
        head: Option<(Option<String>, Option<String>)>,
        /// `None` when the status failed or the directory is not a repository.
        loaded: Option<Result<Loaded, String>>,
        change: Option<Change>,
    },
    Diff {
        generation: u64,
        key: FileKey,
        comparison: Comparison,
        result: Result<GetGitDiffResponse, String>,
    },
    Refs(Result<(Vec<String>, bool), String>),
}

/// The rows of one refresh. `Err` means the requested comparison could not be loaded; the
/// caller keeps what it published.
async fn load_rows(
    job: &Job,
    status: &GitStatusResponse,
    head: Option<&(Option<String>, Option<String>)>,
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
            b.merge_base = Some(base::merge_base(toplevel, &b.commit).await?);
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
    let (files, rename_sources) = match (scope, &base) {
        (Scope::Branch, Some(b)) => {
            let rows = branch::rows(
                toplevel,
                b.merge_base.as_deref().unwrap_or_default(),
                status.files.clone(),
            )
            .await?;
            (rows.files, rows.rename_sources)
        }
        _ => (status.files.clone(), BTreeMap::new()),
    };
    let persisted = match (&job.base, &job.inputs.state_dir) {
        (BaseJob::Pick(pick), Some(dir)) => {
            Some(base::save_pick(dir, toplevel, pick.as_deref()).map_err(|e| e.to_string()))
        }
        (BaseJob::Pick(_), None) => Some(Err("no state directory".to_string())),
        _ => None,
    };
    Ok(Loaded {
        scope,
        base,
        default_base,
        base_error,
        files,
        rename_sources,
        persisted,
    })
}

async fn run_job(job: Job) -> Done {
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
    let loaded = match &response {
        Ok(status) if !status.repo_root.is_empty() => {
            Some(load_rows(&job, status, head.as_ref()).await)
        }
        _ => None,
    };
    Done::Status {
        response,
        head,
        loaded,
        change: job.change,
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
        let scope = match &change {
            Some(Change::Scope(scope)) => *scope,
            Some(Change::Base(_)) => Scope::Branch,
            None => self.requested_scope,
        };
        // Explicit changes resolve preferences; polls only re-verify ids.
        let base = match (&change, std::mem::take(&mut self.resolve_pending)) {
            (Some(Change::Base(pick)), _) => BaseJob::Pick(pick.clone()),
            (Some(Change::Scope(_)), _) | (None, true) => BaseJob::Resolve,
            (None, false) => BaseJob::Keep(self.snapshot.base.clone()),
        };
        let job = Job {
            cwd: cwd.to_string(),
            with_head,
            known_branch: self.branch.clone(),
            scope,
            base,
            inputs: self.inputs.clone(),
            default_base: self.snapshot.default_base.clone(),
            change,
        };
        let results = results.clone();
        tokio::spawn(async move {
            let _ = results.send(run_job(job).await);
        });
    }

    fn request_diff(
        &mut self,
        key: FileKey,
        cwd: &str,
        delay: Option<Duration>,
        gate: Option<Arc<Semaphore>>,
        results: &UnboundedSender<Done>,
    ) {
        let comparison = comparison_of(&self.snapshot);
        if matches!(&self.diff_in_flight, Some((_, pending, under)) if pending == &key && under == &comparison)
        {
            self.diff_dirty = true;
            return;
        }
        self.diff_generation += 1;
        let generation = self.diff_generation;
        self.diff_in_flight = Some((generation, key.clone(), comparison.clone()));
        self.diff_dirty = false;
        let toplevel = match &self.snapshot.repo {
            RepoState::Repo { toplevel, .. } => toplevel.clone(),
            _ => cwd.to_string(),
        };
        let old = self.snapshot.rename_sources.get(&key.path).cloned();
        let cwd = cwd.to_string();
        let results = results.clone();
        tokio::spawn(async move {
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
            let _ = results.send(Done::Diff {
                generation,
                key,
                comparison,
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
        "{:?}|{}|{:?}|{}|{:?}|{:?}|{}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{}|{}|{}|{:?}",
        s.repo,
        serde_json::to_string(&s.files).unwrap_or_default(),
        s.selected,
        diff,
        s.status_error,
        s.watcher_error,
        s.refreshing,
        s.scope,
        s.base,
        s.base_error,
        s.default_base,
        s.rename_sources,
        s.refs.as_ref().map(Arc::as_ptr),
        s.refs_overflow,
        s.refs_seq,
        s.pick_seq,
        s.pick_error
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

async fn run(
    config: SessionConfig,
    mut commands: UnboundedReceiver<Command>,
    snapshots: mpsc::Sender<Arc<Snapshot>>,
    refreshes: Arc<AtomicUsize>,
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
            config: config.base_ref.clone(),
            state_dir: config.state_dir.clone(),
        },
        resolve_pending: true,
        changes: VecDeque::new(),
        pick_seq: 0,
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
                    Command::LoadRefs => {
                        let toplevel = match &state.snapshot.repo {
                            RepoState::Repo { toplevel, .. } => Some(toplevel.clone()),
                            _ => None,
                        };
                        let results = results_tx.clone();
                        tokio::spawn(async move {
                            let refs = match toplevel {
                                Some(toplevel) => base::list_refs(&toplevel).await,
                                None => Ok((Vec::new(), false)),
                            };
                            let _ = results.send(Done::Refs(refs));
                        });
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
                            publish(&mut state, next, &snapshots);
                            state.request_diff(key, &cwd, config.diff_delay, config.diff_gate.clone(), &results_tx);
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
                    Done::Refs(result) => {
                        let (refs, overflow) = result.unwrap_or_default();
                        next.refs = Some(Arc::new(refs));
                        next.refs_overflow = overflow;
                        next.refs_seq += 1;
                        publish(&mut state, next, &snapshots);
                    }
                    Done::Watcher(result) => {
                        state.watcher = if result.is_ok() { WatcherPhase::Running } else { WatcherPhase::Failed };
                        next.watcher_error = result.err();
                        publish(&mut state, next, &snapshots);
                    }
                    Done::Status { response, head, loaded, change } => {
                        state.status_in_flight = false;
                        let before = comparison_of(&state.snapshot);
                        let mut succeeded = false;
                        let mut change_error = None;
                        match response {
                            Err(e) => {
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
                                    Some(Err(e)) => match &change {
                                        // Failed picks and switches report notices; refresh errors keep the rows.
                                        Some(Change::Base(_)) => change_error = Some(e),
                                        Some(Change::Scope(_)) => next.base_error = Some(e),
                                        None => next.status_error = Some(e),
                                    },
                                    other => {
                                        next.status_error = None;
                                        let loaded = match other {
                                            Some(Ok(loaded)) => loaded,
                                            _ => Loaded {
                                                scope: Scope::Worktree,
                                                base: None,
                                                default_base: None,
                                                base_error: None,
                                                files: response.files,
                                                rename_sources: BTreeMap::new(),
                                                persisted: None,
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
                                        succeeded = true;
                                    }
                                }
                            }
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
                        if matches!(change, Some(Change::Base(_))) {
                            state.pick_seq += 1;
                            next.pick_seq = state.pick_seq;
                            next.pick_error = change_error;
                        }
                        let selected = next.selected.clone().filter(|_| succeeded);
                        if selected.is_none() {
                            next.refreshing = !state.changes.is_empty();
                        }
                        publish(&mut state, next, &snapshots);
                        if let Some(key) = selected {
                            state.request_diff(key, &cwd, config.diff_delay, config.diff_gate.clone(), &results_tx);
                        }
                        if state.status_dirty || !state.changes.is_empty() {
                            state.status_dirty = false;
                            let with_head = std::mem::take(&mut state.status_dirty_head);
                            state.request_status(&cwd, with_head, &results_tx, &refreshes);
                        }
                    }
                    Done::Diff { generation, key, comparison, result } => {
                        if generation != state.diff_generation || comparison != comparison_of(&state.snapshot) {
                            continue;
                        }
                        state.diff_in_flight = None;
                        if Some(&key) == next.selected.as_ref() {
                            match result {
                                Ok(response) => {
                                    let unchanged = matches!(&next.diff, DiffState::Ready(d) if d.key == key && d.comparison == comparison && d.raw_diff == response.raw_diff);
                                    if !unchanged {
                                        next.diff = DiffState::Ready(Arc::new(LoadedDiff::build(key, comparison, response)));
                                    }
                                }
                                Err(e) => next.diff = DiffState::Failed(e),
                            }
                            if !state.status_in_flight && !state.diff_dirty {
                                next.refreshing = false;
                            }
                            publish(&mut state, next, &snapshots);
                        }
                        if std::mem::take(&mut state.diff_dirty) {
                            if let Some(key) = state.snapshot.selected.clone() {
                                state.request_diff(key, &cwd, config.diff_delay, config.diff_gate.clone(), &results_tx);
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
    runtime.spawn(run(config, commands_rx, snapshots_tx, refreshes.clone()));
    EngineHandle {
        commands,
        snapshots,
        refreshes,
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

    fn git(dir: &std::path::Path, args: &[&str]) {
        assert!(
            Proc::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .status()
                .unwrap()
                .success(),
            "git {args:?}"
        );
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
        let (_rt, h) = start_with(dir.path(), Scope::Worktree, None, None);
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
        let s = wait_for(&h, "branch diff", |s| ready(s).is_some());
        assert!(matches!(
            ready(&s).unwrap().comparison,
            Comparison::Branch { .. }
        ));
        assert_eq!(ready(&s).unwrap().key.path, "a.txt");
        // No snapshot after the switch carries a worktree-comparison diff.
        std::thread::sleep(Duration::from_millis(200));
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
        h.commands.send(Command::LoadRefs).unwrap();
        let s = wait_for(&h, "refs", |s| s.refs.is_some());
        let refs = s.refs.as_ref().unwrap();
        assert!(
            refs.contains(&"refs/heads/main".to_string())
                && refs.contains(&"refs/heads/feat".to_string())
        );
        assert!(!s.refs_overflow);
        assert_eq!(s.refs_seq, 1);
        h.commands.send(Command::LoadRefs).unwrap();
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
}
