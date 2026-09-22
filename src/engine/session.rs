//! Non-blocking repository sessions and immutable snapshots.
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::Semaphore;

use super::{gitver, Command, DiffState, FileKey, LoadedDiff, RepoState, Snapshot};
use crate::git::{self, ChangedFile, ChangedFileStatus, GetGitDiffResponse, GitStatusResponse};
use crate::runtime::EventSink;

pub type BoxFut<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>;

pub trait WatcherControl: Send + Sync + 'static {
    fn start(&self, cwd: String, sink: Arc<dyn EventSink>) -> BoxFut<Result<(), String>>;
    fn stop(&self, cwd: String) -> BoxFut<Result<(), String>>;
}

pub struct SessionConfig {
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
    diff_in_flight: Option<(u64, FileKey)>,
    diff_dirty: bool,
}

fn fingerprint(s: &Snapshot) -> String {
    let diff = match &s.diff {
        DiffState::Idle => "idle".to_string(),
        DiffState::Loading => "loading".to_string(),
        DiffState::Failed(e) => format!("failed:{e}"),
        DiffState::Ready(d) => format!("ready:{:p}", Arc::as_ptr(d)),
    };
    format!(
        "{:?}|{}|{:?}|{}|{:?}|{:?}|{}",
        s.repo,
        serde_json::to_string(&s.files).unwrap_or_default(),
        s.selected,
        diff,
        s.status_error,
        s.watcher_error,
        s.refreshing
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

enum Done {
    GitCheck(Result<gitver::GitVersion, gitver::GitCheckError>),
    Watcher(Result<(), String>),
    Status {
        response: Result<GitStatusResponse, String>,
        head: Option<(Option<String>, Option<String>)>,
    },
    Diff {
        generation: u64,
        key: FileKey,
        result: Result<GetGitDiffResponse, String>,
    },
}

impl State {
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
        let cwd = cwd.to_string();
        let results = results.clone();
        tokio::spawn(async move {
            let response = git::git_status_inner(cwd.clone()).await;
            let head = if with_head {
                let (branch, worktree) = tokio::join!(
                    git::git_branch_inner(cwd.clone()),
                    git::git_worktree_name_inner(cwd)
                );
                Some((branch.ok(), worktree.ok().flatten()))
            } else {
                None
            };
            let _ = results.send(Done::Status { response, head });
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
        if matches!(&self.diff_in_flight, Some((_, pending)) if pending == &key) {
            self.diff_dirty = true;
            return;
        }
        self.diff_generation += 1;
        let generation = self.diff_generation;
        self.diff_in_flight = Some((generation, key.clone()));
        self.diff_dirty = false;
        let untracked = self
            .snapshot
            .files
            .iter()
            .any(|f| key_of(f) == key && matches!(f.status, ChangedFileStatus::Untracked));
        let cwd = cwd.to_string();
        let results = results.clone();
        tokio::spawn(async move {
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            if let Some(gate) = gate {
                gate.acquire().await.expect("diff gate closed").forget();
            }
            let result =
                git::get_git_diff_inner(cwd, key.path.clone(), key.staged, Some(untracked)).await;
            let _ = results.send(Done::Diff {
                generation,
                key,
                result,
            });
        });
    }
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
    FileKey {
        path: file.path.clone(),
        staged: file.staged,
    }
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
                    Done::Watcher(result) => {
                        state.watcher = if result.is_ok() { WatcherPhase::Running } else { WatcherPhase::Failed };
                        next.watcher_error = result.err();
                        publish(&mut state, next, &snapshots);
                    }
                    Done::Status { response, head } => {
                        state.status_in_flight = false;
                        let succeeded = response.is_ok();
                        match response {
                            Err(e) => next.status_error = Some(e),
                            Ok(response) => {
                                next.status_error = None;
                                if let Some((branch, worktree)) = head {
                                    state.branch = branch;
                                    state.worktree = worktree;
                                }
                                next.repo = if response.repo_root.is_empty() {
                                    RepoState::NotARepo { cwd: cwd.clone() }
                                } else {
                                    RepoState::Repo {
                                        toplevel: response.repo_root,
                                        branch: state.branch.clone(),
                                        worktree: state.worktree.clone(),
                                    }
                                };
                                let old_index = next.files.iter().position(|f| Some(key_of(f)) == next.selected);
                                next.files = response.files;
                                next.selected = next.selected.clone().filter(|key| next.files.iter().any(|f| &key_of(f) == key))
                                    .or_else(|| old_index.and_then(|i| next.files.get(i.min(next.files.len().saturating_sub(1))).map(key_of)))
                                    .or_else(|| next.files.first().map(key_of));
                                let same_key = matches!(&next.diff, DiffState::Ready(d) if Some(&d.key) == next.selected.as_ref());
                                if !same_key {
                                    next.diff = if next.selected.is_some() { DiffState::Loading } else { DiffState::Idle };
                                }
                            }
                        }
                        let selected = next.selected.clone().filter(|_| succeeded);
                        if selected.is_none() {
                            next.refreshing = false;
                        }
                        publish(&mut state, next, &snapshots);
                        if let Some(key) = selected {
                            state.request_diff(key, &cwd, config.diff_delay, config.diff_gate.clone(), &results_tx);
                        }
                        if state.status_dirty {
                            state.status_dirty = false;
                            let with_head = std::mem::take(&mut state.status_dirty_head);
                            state.request_status(&cwd, with_head, &results_tx, &refreshes);
                        }
                    }
                    Done::Diff { generation, key, result } => {
                        if generation != state.diff_generation {
                            continue;
                        }
                        state.diff_in_flight = None;
                        if Some(&key) == next.selected.as_ref() {
                            match result {
                                Ok(response) => {
                                    let unchanged = matches!(&next.diff, DiffState::Ready(d) if d.key == key && d.raw_diff == response.raw_diff);
                                    if !unchanged {
                                        next.diff = DiffState::Ready(Arc::new(LoadedDiff::build(key, response)));
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
                staged: false
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
}
