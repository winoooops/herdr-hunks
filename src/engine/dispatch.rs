//! Sending and copying (spec 10.4): two gates, one lock, a claim before the call, a settlement by nonce.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::Semaphore;

use super::comments::{self, RequestFile, RequestRecord, Settlement, Store};
use super::host::{self, HostClient, SessionRef};
use super::prompt;
use super::target::{self, Destination, Target, TargetState};
use super::{branch, worktree, DiffState, FileKey, Snapshot};
use crate::git::ChangedFileStatus;

pub const SEND_LOCK: &str = "send.lock";
pub const LOCK_WAIT: Duration = Duration::from_secs(12);
pub const ENTER_MARGIN: Duration = Duration::from_millis(500);
pub const REQUEST_BOUND: usize = 512 * 1024;
pub const OSC_LIMIT: usize = 100_000;
pub const CLIPBOARD_FILE: &str = "clipboard.md";
pub const NOTICE_IN_PROGRESS: &str = "a send is in progress";
pub const NOTICE_WAITING: &str = "another viewer is sending";
pub use super::comments::NOTICE_NOTHING;
pub const NOTICE_NO_PENDING: &str = "nothing to send: no pending comments";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewScope {
    File(FileKey),
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendKind {
    Feedback,
    Review { scope: ReviewScope },
}

/// What a "send anyway" row stands for: the status it named, the session it found restarted.
/// `restarted` is `None` for no restart accepted, `Some(session)` for a restart into exactly that
/// session, `Some(None)` for one into a pane whose agent reports no session at all.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Accepted {
    pub busy: bool,
    pub restarted: Option<Option<SessionRef>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendRequest {
    pub kind: SendKind,
    pub accepted: Accepted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyWhat {
    Review,
    Request { scope: ReviewScope },
    Selection(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyRequest {
    pub what: CopyWhat,
}

/// What the shell writes to the terminal once per `copy_seq`, and the notice to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyOut {
    pub osc: Option<String>,
    pub notice: String,
    pub urgent: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendOutcome {
    pub kind: SendKind,
    pub items: u32,
    pub to: Destination,
    pub unconfirmed: bool,
    pub copy: Option<CopyOut>,
}

/// Everything a send or copy needs from the session, taken when the command arrives (and kept with a
/// queued send, so the send runs against the target it was confirmed for).
pub struct Context {
    pub toplevel: String,
    pub state_dir: Option<PathBuf>,
    pub host: Option<Arc<dyn HostClient>>,
    /// How long a host call may run before its outcome is uncertain: `ENGINE_WAIT`, or a test's seam.
    pub host_wait: Duration,
    /// Where a late answer to a timed-out prompt reports, settled by the session by nonce.
    pub late: tokio::sync::mpsc::UnboundedSender<(String, Settlement)>,
    pub socket_path: Option<String>,
    pub target: Option<Target>,
    /// The session's side map for worktree renames, `(path, staged) -> source` (`State.worktree_renames`).
    pub worktree_renames: std::collections::BTreeMap<(String, bool), String>,
    /// The generation this command was issued under, and the live one to compare with at the claim.
    pub generation: u64,
    pub latest_generation: Arc<AtomicU64>,
    pub copy_generation: u64,
    pub latest_copy: Arc<AtomicU64>,
    pub nonce: Arc<dyn Fn(u64) -> String + Send + Sync>,
    pub nonce_counter: u64,
    pub snapshot: Arc<Snapshot>,
    pub lane: Arc<Semaphore>,
    /// The time, sampled where a stamp is made, not when the command arrived: a send may queue first.
    pub clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    /// A test's seam, run on the blocking pool after the transaction and before the last generation check.
    pub send_gate: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// The engine's part of the answer, beside the store it borrowed.
pub struct Finished {
    pub store: Option<Store>,
    pub outcome: Result<SendOutcome, (String, Refusal)>,
    /// A copy that reached the terminal or the file although the outcome is an error (the stamps could
    /// not be written after it): the session publishes it so the shell still writes the sequence.
    pub copy: Option<CopyOut>,
    /// A `Restarted` the reviewer accepted: the target record adopts this session (spec 10.4 step 1),
    /// if the selection is still the one this send was made for.
    pub adopt_session: Option<Option<SessionRef>>,
    /// The selection generation the send was issued under.
    pub generation: u64,
    /// Where the nonce counter stands after this send; the session continues from it.
    pub counter: u64,
}

/// One blocking store or file operation, off the async executor.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.expect("blocking task")
}

pub fn encoded_len(pane: &str, text: &str) -> usize {
    serde_json::to_string(
        &json!({ "id": "0", "method": "agent.prompt", "params": { "target": pane, "text": text } }),
    )
    .map(|s| s.len() + 1)
    .unwrap_or(usize::MAX)
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// `ESC ] 52 ; c ; <base64> BEL`, or `None` when the text is over what terminals take.
pub fn osc52(text: &str) -> Option<String> {
    (text.len() <= OSC_LIMIT).then(|| format!("\x1b]52;c;{}\x07", base64(text.as_bytes())))
}

/// Diff-side hunk ranges, inclusive: `(additions, deletions)`, from the parsed file before the
/// display cap of 3.2 cuts hunks, so a request's ranges cover the whole file.
#[allow(clippy::type_complexity)]
pub fn ranges_of(file_diff: &crate::git::FileDiff) -> (Vec<(u32, u32)>, Vec<(u32, u32)>) {
    let mut additions = Vec::new();
    let mut deletions = Vec::new();
    for hunk in &file_diff.hunks {
        if hunk.new_lines > 0 {
            additions.push((hunk.new_start, hunk.new_start + hunk.new_lines - 1));
        }
        if hunk.old_lines > 0 {
            deletions.push((hunk.old_start, hunk.old_start + hunk.old_lines - 1));
        }
    }
    (additions, deletions)
}

fn comparison_of(snapshot: &Snapshot) -> comments::AnchorComparison {
    match (&snapshot.scope, &snapshot.base) {
        (super::Scope::Branch, Some(base)) if base.merge_base.is_some() => {
            comments::AnchorComparison::Branch {
                merge_base: base.merge_base.clone().unwrap_or_default(),
                label: base.label().to_string(),
            }
        }
        _ => comments::AnchorComparison::Worktree,
    }
}

/// One file of a review request: its prompt line and its ranges, the diff loaded when it is not the
/// one on screen (spec 10.4 "Request review"), under the diff lane, one row after another.
pub async fn request_files(
    ctx: &Context,
    scope: &ReviewScope,
) -> Result<(Vec<prompt::RequestLine>, Vec<RequestFile>), String> {
    let snapshot = &ctx.snapshot;
    let comparison = comparison_of(snapshot);
    let ready = match &snapshot.diff {
        DiffState::Ready(d) => Some(d.clone()),
        _ => None,
    };
    let keys: Vec<FileKey> = match scope {
        ReviewScope::File(key) => vec![key.clone()],
        ReviewScope::All => snapshot.files.iter().map(FileKey::of).collect(),
    };
    if keys.is_empty() {
        return Err("nothing to review".to_string());
    }
    let mut lines = Vec::new();
    let mut files = Vec::new();
    for key in keys {
        let file = snapshot.files.iter().find(|f| FileKey::of(f) == key);
        let untracked = file.is_some_and(|f| matches!(f.status, ChangedFileStatus::Untracked));
        // Reload truncated diffs so recorded ranges cover every hunk.
        let on_screen = ready
            .as_ref()
            .filter(|d| d.key == key && d.truncated_lines == 0)
            .map(|d| ranges_of(&d.file_diff));
        let ranges = match on_screen {
            Some(ranges) => Some(ranges),
            None => {
                if matches!(scope, ReviewScope::File(_))
                    && !ready.as_ref().is_some_and(|d| d.key == key)
                {
                    return Err("no diff loaded for this file".to_string());
                }
                let _lane = ctx.lane.acquire().await.map_err(|e| e.to_string())?;
                // Resolve rename sources by side in worktree scope and by path in branch scope.
                let old = match &comparison {
                    comments::AnchorComparison::Worktree => ctx
                        .worktree_renames
                        .get(&(key.path.clone(), key.staged))
                        .cloned(),
                    comments::AnchorComparison::Branch { .. } => {
                        snapshot.rename_sources.get(&key.path).cloned()
                    }
                };
                let result = match &comparison {
                    comments::AnchorComparison::Branch { merge_base, .. } if !key.untracked => {
                        branch::diff(&ctx.toplevel, merge_base, &key.path, old.as_deref()).await
                    }
                    comments::AnchorComparison::Branch { .. } => {
                        worktree::untracked_diff(&ctx.toplevel, &key.path).await
                    }
                    comments::AnchorComparison::Worktree => {
                        worktree::diff(&ctx.toplevel, &key, old.as_deref()).await
                    }
                };
                // A row whose diff fails is listed without ranges; the request goes out.
                result
                    .ok()
                    .map(|(response, _patch)| ranges_of(&response.file_diff))
            }
        };
        let (additions, deletions) = ranges.unwrap_or_default();
        lines.push(prompt::RequestLine {
            path: key.path.clone(),
            staged: key.staged,
            untracked,
        });
        files.push(RequestFile {
            key,
            comparison: comparison.clone(),
            additions,
            deletions,
        });
    }
    Ok((lines, files))
}

/// Spec 10.4 step 1: the fresh check and its two gates. `Ok(record)` passes; `Err` is the refusal.
/// What kind of refusal the gates gave, so the box knows what the next Y may accept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    Busy,
    Restarted(Option<SessionRef>),
    Other,
}

fn gates(
    target: &Target,
    state: Option<TargetState>,
    record: Option<&host::PaneRecord>,
    accepted: &Accepted,
    failure: Option<&host::HostFailure>,
) -> Result<(), (String, Refusal)> {
    let Target::Pane { pane, agent, .. } = target else {
        return Ok(());
    };
    let Some(state) = state else {
        // A failed or mismatched check never authorizes a send.
        let reason = match (failure, record) {
            (Some(f), _) => f.message(),
            (None, Some(r)) if r.pane_id != *pane => {
                format!("the host answered about {}", r.pane_id)
            }
            (None, _) => "no answer".to_string(),
        };
        return Err((format!("could not verify {pane}: {reason}"), Refusal::Other));
    };
    // The continuity gate.
    match &state {
        TargetState::Left | TargetState::Gone => {
            return Err((
                format!("{agent} · {pane} is gone · pick a pane"),
                Refusal::Other,
            ))
        }
        TargetState::NoHost => {
            return Err((
                "No host: this viewer runs outside herdr.".to_string(),
                Refusal::Other,
            ))
        }
        TargetState::Restarted(_) => {
            let now = record.and_then(|r| r.agent_session.clone());
            // Accepted only for the very session the check found, a missing one included.
            if accepted.restarted.as_ref() != Some(&now) {
                return Err((
                    format!("{agent} in {pane} was restarted since you picked it and has not seen earlier messages."),
                    Refusal::Restarted(now),
                ));
            }
        }
        TargetState::Live(_) | TargetState::Unverified | TargetState::Clipboard => {}
    }
    // The status gate, whatever the continuity.
    let status = match &state {
        TargetState::Live(s) | TargetState::Restarted(s) => s.as_str(),
        _ => "unknown",
    };
    // Unfamiliar statuses require the same acceptance as unknown.
    match status {
        "blocked" => Err((format!("{agent} is waiting for an approval in {pane}. Answer it there, or press A to pick another pane."), Refusal::Other)),
        "idle" | "done" => Ok(()),
        _ if accepted.busy => Ok(()),
        "working" => Err((format!("{agent} is working in {pane}; the review would queue behind its current turn."), Refusal::Busy)),
        "unknown" => Err((format!("{agent}'s state in {pane} is unknown to the host."), Refusal::Busy)),
        other => Err((format!("{agent} reports {} in {pane}, a state this viewer does not know.", strip_status(other)), Refusal::Busy)),
    }
}

/// A status the host invented, in a message: letters, digits and a few marks, cut short.
fn strip_status(status: &str) -> String {
    status
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .take(24)
        .collect()
}

fn now_label(kind: &SendKind) -> &'static str {
    match kind {
        SendKind::Feedback => "Y",
        SendKind::Review { .. } => "@",
    }
}

/// `clipboard.md` under the state lock; `Err` is the reason.
fn write_clipboard(
    state_dir: Option<&Path>,
    text: &str,
    guard: &dyn Fn() -> Result<(), String>,
) -> Result<PathBuf, String> {
    let dir = state_dir.ok_or_else(|| "no state directory".to_string())?;
    comments::write_clipboard(dir, text, guard).map_err(|e| e.to_string())
}

fn shown(path: &Path) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let text = path.to_string_lossy();
    match text
        .strip_prefix(home.as_str())
        .filter(|_| !home.is_empty())
    {
        Some(rest) => format!("~{rest}"),
        None => text.into_owned(),
    }
}

/// The next nonce `make` yields that `used` does not hold: the loop `Store::claim` and
/// `record_request_fresh` run under their locks, for the paths that have no file to lock.
fn fresh_nonce(
    make: &dyn Fn(u64) -> String,
    counter: &mut u64,
    used: &std::collections::BTreeSet<String>,
) -> String {
    loop {
        *counter += 1;
        let candidate = make(*counter);
        if !used.contains(&candidate) {
            return candidate;
        }
    }
}

/// The copy of 10.4: the sequence when the text fits, the file when it can be written; the notice names both.
fn copy_out(
    state_dir: Option<&Path>,
    text: &str,
    what: &str,
    guard: &dyn Fn() -> Result<(), String>,
) -> Option<(CopyOut, Result<PathBuf, String>)> {
    guard().ok()?;
    let file = write_clipboard(state_dir, text, guard);
    guard().ok()?;
    let osc = osc52(text);
    let notice = match (&file, &osc) {
        (Ok(path), Some(_)) => format!("copied {what} · also in {}", shown(path)),
        (Ok(path), None) => format!(
            "copied {what} to {} · too long for the terminal's clipboard",
            shown(path)
        ),
        (Err(reason), Some(_)) => format!("copied {what} · not saved to clipboard.md: {reason}"),
        (Err(reason), None) => format!("nothing could receive the copy: {reason}"),
    };
    Some((
        CopyOut {
            osc,
            notice,
            urgent: file.is_err(),
        },
        file,
    ))
}

/// The send task of spec 10.4, one at a time per viewer (the session refuses a second) and per user
/// (the send lock). Returns the store it borrowed with the answer.
pub async fn send(
    ctx: Context,
    request: SendRequest,
    store: Store,
    waiting: impl Fn(bool) + Send + Sync + 'static,
) -> Finished {
    let generation = ctx.generation;
    let mut counter = ctx.nonce_counter;
    let waiting = Arc::new(waiting);
    let mut salvaged = None;
    let (store, outcome) =
        send_inner(&ctx, &request, store, &mut counter, waiting, &mut salvaged).await;
    Finished {
        store: Some(store),
        adopt_session: outcome.as_ref().ok().and_then(|(_, adopt)| adopt.clone()),
        outcome: outcome.map(|(o, _)| o),
        copy: salvaged,
        generation,
        counter,
    }
}

/// The store travels into every blocking step and back, so no transaction runs on an async worker.
async fn send_inner(
    ctx: &Context,
    request: &SendRequest,
    mut store: Store,
    counter: &mut u64,
    waiting: Arc<dyn Fn(bool) + Send + Sync>,
    salvaged: &mut Option<CopyOut>,
) -> (
    Store,
    Result<(SendOutcome, Option<Option<SessionRef>>), (String, Refusal)>,
) {
    // Every refusal that is not one of the gates' is `Refusal::Other`.
    macro_rules! bail {
        ($store:expr, $err:expr) => {
            return ($store, Err(($err, Refusal::Other)))
        };
    }
    let Some(target) = ctx.target.clone() else {
        bail!(store, "no target: press A".to_string());
    };
    // Read ranges before verifying the target.
    let request_files = match &request.kind {
        SendKind::Review { scope } => match request_files(ctx, scope).await {
            Ok(files) => Some(files),
            Err(e) => bail!(store, e),
        },
        SendKind::Feedback => None,
    };
    // Serialize every viewer's sends through the shared lock.
    let mut _held = match ctx.state_dir.as_ref().map(|dir| dir.join(SEND_LOCK)) {
        Some(path) => match hold_send_lock(path, waiting.clone()).await {
            Ok(lock) => Some(lock),
            Err(e) => bail!(store, e),
        },
        None => None,
    };
    let again = || {
        format!(
            "the target changed; press {} again",
            now_label(&request.kind)
        )
    };
    // Step 1b, the fresh check, under the selection generation.
    if ctx.latest_generation.load(Ordering::SeqCst) != ctx.generation {
        bail!(store, again());
    }
    let (state, record, failure) = match &target {
        Target::Clipboard => (Some(TargetState::Clipboard), None, None),
        Target::Pane { pane, .. } => {
            let pane = pane.clone();
            let fresh = call_host(ctx.host.clone(), move |h| h.pane_get(&pane)).await;
            let state = target::compare(&target, ctx.socket_path.as_deref(), &fresh);
            let (record, failure) = match fresh {
                Ok(r) => (Some(r), None),
                Err(f) => (None, Some(f)),
            };
            (state, record, failure)
        }
    };
    if let Err((message, refusal)) = gates(
        &target,
        state.clone(),
        record.as_ref(),
        &request.accepted,
        failure.as_ref(),
    ) {
        return (store, Err((message, refusal)));
    }
    if ctx.latest_generation.load(Ordering::SeqCst) != ctx.generation {
        bail!(store, again());
    }
    // Remember accepted restarts and the first session a sessionless target sees.
    let adopt = match (&state, &record) {
        (Some(TargetState::Restarted(_)), Some(r)) => Some(r.agent_session.clone()),
        (Some(TargetState::Live(_)), Some(r)) => target::adopted_session(&target, r).map(Some),
        _ => None,
    };
    let to = match (&target, &record) {
        (Target::Clipboard, _) => Destination::clipboard(),
        (Target::Pane { .. }, Some(r)) => target::destination_of(r),
        (
            Target::Pane {
                pane,
                agent,
                session,
                ..
            },
            None,
        ) => Destination::Pane {
            pane: pane.clone(),
            agent: agent.clone(),
            session: session.clone(),
        },
    };
    let pane_for_bound = match &to {
        Destination::Pane { pane, .. } => pane.clone(),
        Destination::Clipboard { .. } => String::new(),
    };
    // Shared by the closures below, each of which runs on the blocking pool with its own clone.
    let bound = Arc::new(move |text: &str| {
        let len = encoded_len(&pane_for_bound, text);
        if len > REQUEST_BOUND {
            return Err(format!(
                "review too large to send at once ({} KiB of 512): delete or shorten comments",
                len.div_ceil(1024)
            ));
        }
        Ok(())
    });
    // Sample the claim time after waiting for the lock and host.
    let toplevel = ctx.toplevel.clone();
    let make = ctx.nonce.clone();
    let clock = ctx.clock.clone();
    let now = clock();
    // Check the selection under the state lock before stamping.
    let (latest, generation) = (ctx.latest_generation.clone(), ctx.generation);
    let label = now_label(&request.kind);
    let guard = Arc::new(move || -> Result<(), String> {
        if latest.load(Ordering::SeqCst) != generation {
            Err(format!("the target changed; press {label} again"))
        } else {
            Ok(())
        }
    });
    let (text, items, nonce, claimed) = match (&request.kind, request_files) {
        (SendKind::Feedback, _) => {
            let (to2, guard2, bound2, clock2) =
                (to.clone(), guard.clone(), bound.clone(), clock.clone());
            let mut c = *counter;
            let (returned, result) = blocking(move || {
                // The stamp's time is the claim's own: the expiry of 10.4 runs from it.
                let result = store.claim(
                    clock2(),
                    &to2,
                    make.as_ref(),
                    &mut c,
                    guard2.as_ref(),
                    |eligible, nonce| {
                        let items: Vec<prompt::Item<'_>> = eligible
                            .iter()
                            .map(|(n, c)| prompt::Item {
                                number: *n,
                                comment: c,
                            })
                            .collect();
                        let text = prompt::review(&toplevel, &items, nonce);
                        bound2(&text)?;
                        Ok(text)
                    },
                );
                (store, result.map(|claimed| (claimed, c)))
            })
            .await;
            store = returned;
            match result {
                Ok((claimed, c)) => {
                    *counter = c;
                    (
                        claimed.text,
                        claimed.comments.len() as u32,
                        claimed.nonce,
                        true,
                    )
                }
                Err(e) => bail!(store, e),
            }
        }
        (SendKind::Review { .. }, Some((lines, files))) => {
            let scope = match comparison_of(&ctx.snapshot) {
                comments::AnchorComparison::Branch { merge_base, .. } => Some(merge_base),
                comments::AnchorComparison::Worktree => None,
            };
            // Build the text with the nonce reserved under the lock.
            let text_of = Arc::new(move |nonce: &str| {
                prompt::request(
                    &toplevel,
                    &lines,
                    match &scope {
                        Some(mb) => prompt::RequestScope::Branch { merge_base: mb },
                        None => prompt::RequestScope::Worktree,
                    },
                    nonce,
                )
            });
            let items = files.len() as u32;
            // Check the encoded text before reserving anything.
            let over_bound = |text: &str| bound(text).err();
            match (&to, &ctx.state_dir) {
                // Reserve before calling the host; remove on definite failure.
                (Destination::Pane { .. }, Some(dir)) => {
                    // An oversized request leaves the file untouched.
                    let (dir2, top, make, to2, guard2, bound2, mut c) = (
                        dir.clone(),
                        ctx.toplevel.clone(),
                        ctx.nonce.clone(),
                        to.clone(),
                        guard.clone(),
                        bound.clone(),
                        *counter,
                    );
                    let recorded = blocking(move || {
                        let (nonce, text) = comments::record_request_fresh(
                            &dir2,
                            &top,
                            make.as_ref(),
                            &mut c,
                            guard2.as_ref(),
                            |nonce| {
                                let text = text_of(nonce);
                                bound2(&text)?;
                                Ok((
                                    RequestRecord {
                                        nonce: nonce.to_string(),
                                        at: now,
                                        target: to2,
                                        files,
                                    },
                                    text,
                                ))
                            },
                        )?;
                        Ok::<_, std::io::Error>((nonce, text, c))
                    })
                    .await;
                    match recorded {
                        Ok((nonce, text, c)) => {
                            *counter = c;
                            (text, items, nonce, false)
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => bail!(
                            store,
                            e.into_inner().map(|e| e.to_string()).unwrap_or_default()
                        ),
                        Err(e) => bail!(store, format!("request not recorded: {e}")),
                    }
                }
                // Without disk state, avoid every nonce retained in memory.
                (Destination::Pane { .. }, None) => {
                    if let Err(e) = guard() {
                        bail!(store, e);
                    }
                    let nonce = fresh_nonce(
                        ctx.nonce.as_ref(),
                        counter,
                        &comments::nonces_of(store.comments()),
                    );
                    let text = text_of(&nonce);
                    if let Some(e) = over_bound(&text) {
                        bail!(store, e);
                    }
                    (text, items, nonce, false)
                }
                // Reserve first; remove the record if the copy reaches nothing.
                (Destination::Clipboard { .. }, dir) => {
                    let (nonce, text) = match dir {
                        Some(dir) => {
                            let (dir, top, make, guard2, bound2, mut c, files2) = (
                                dir.clone(),
                                ctx.toplevel.clone(),
                                ctx.nonce.clone(),
                                guard.clone(),
                                bound.clone(),
                                *counter,
                                files.clone(),
                            );
                            let text_of2 = text_of.clone();
                            let recorded = blocking(move || {
                                let (nonce, text) = comments::record_request_fresh(
                                    &dir,
                                    &top,
                                    make.as_ref(),
                                    &mut c,
                                    guard2.as_ref(),
                                    |nonce| {
                                        let text = text_of2(nonce);
                                        bound2(&text)?;
                                        Ok((
                                            RequestRecord {
                                                nonce: nonce.to_string(),
                                                at: now,
                                                target: Destination::clipboard(),
                                                files: files2,
                                            },
                                            text,
                                        ))
                                    },
                                )?;
                                Ok::<_, std::io::Error>((nonce, text, c))
                            })
                            .await;
                            match recorded {
                                Ok((nonce, text, c)) => {
                                    *counter = c;
                                    (nonce, text)
                                }
                                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => bail!(
                                    store,
                                    e.into_inner().map(|e| e.to_string()).unwrap_or_default()
                                ),
                                Err(e) => bail!(store, format!("request not recorded: {e}")),
                            }
                        }
                        None => {
                            if let Err(e) = guard() {
                                bail!(store, e);
                            }
                            let nonce = fresh_nonce(
                                ctx.nonce.as_ref(),
                                counter,
                                &comments::nonces_of(store.comments()),
                            );
                            let text = text_of(&nonce);
                            if let Some(e) = over_bound(&text) {
                                bail!(store, e);
                            }
                            (nonce, text)
                        }
                    };
                    // Recheck the selection after reserving and before copying.
                    if let Some(gate) = ctx.send_gate.clone() {
                        blocking(move || gate()).await;
                    }
                    if ctx.latest_generation.load(Ordering::SeqCst) != ctx.generation {
                        if let Some(dir) = dir.clone() {
                            let (top, n) = (ctx.toplevel.clone(), nonce.clone());
                            let _ =
                                blocking(move || comments::remove_request(&dir, &top, &n)).await;
                        }
                        bail!(store, again());
                    }
                    let (dir2, text2) = (dir.clone(), text.clone());
                    let (copy, file) = blocking(move || {
                        copy_out(dir2.as_deref(), &text2, "a review request", &|| Ok(()))
                            .expect("send copies are never superseded")
                    })
                    .await;
                    if file.is_err() && copy.osc.is_none() {
                        if let Some(dir) = dir.clone() {
                            let (top, n) = (ctx.toplevel.clone(), nonce.clone());
                            let _ =
                                blocking(move || comments::remove_request(&dir, &top, &n)).await;
                        }
                        bail!(store, copy.notice);
                    }
                    return (
                        store,
                        Ok((
                            SendOutcome {
                                kind: request.kind.clone(),
                                items,
                                to,
                                unconfirmed: file.is_err(),
                                copy: Some(copy),
                            },
                            adopt,
                        )),
                    );
                }
            }
        }
        (SendKind::Review { .. }, None) => unreachable!("request files are read first"),
    };
    // Undo the reservation if a pick changed during the transaction.
    if let Some(gate) = ctx.send_gate.clone() {
        blocking(move || gate()).await;
    }
    if ctx.latest_generation.load(Ordering::SeqCst) != ctx.generation {
        if claimed {
            let n = nonce.clone();
            let (returned, _) = blocking(move || {
                let settled = store.settle(now, &n, Settlement::Failed);
                (store, settled)
            })
            .await;
            store = returned;
        } else if let Some(dir) = ctx.state_dir.clone() {
            let (top, n) = (ctx.toplevel.clone(), nonce.clone());
            let _ = blocking(move || comments::remove_request(&dir, &top, &n)).await;
        }
        bail!(store, again());
    }
    // Steps 4 and 5, the call and the settlement.
    match &to {
        Destination::Clipboard { .. } => {
            let (dir, text2) = (ctx.state_dir.clone(), text.clone());
            let (copy, file) = blocking(move || {
                copy_out(
                    dir.as_deref(),
                    &text2,
                    &format!("{items} comments"),
                    &|| Ok(()),
                )
                .expect("send copies are never superseded")
            })
            .await;
            let settlement = match (&file, &copy.osc) {
                (Ok(_), _) => Settlement::Sent,
                (Err(_), Some(_)) => Settlement::Unconfirmed,
                (Err(_), None) => Settlement::Failed,
            };
            if claimed {
                let n = nonce.clone();
                let (returned, settled) = blocking(move || {
                    let settled = store.settle(now, &n, settlement);
                    (store, settled)
                })
                .await;
                store = returned;
                if let Err(e) = settled {
                    if settlement == Settlement::Failed {
                        bail!(store, format!("{} · {e}", copy.notice));
                    }
                    // Preserve the sequence when the copy succeeds but settlement fails.
                    let notice = format!(
                        "{} · but the comments were not marked sent: {e}",
                        copy.notice
                    );
                    *salvaged = Some(copy);
                    bail!(store, notice);
                }
            }
            if settlement == Settlement::Failed {
                bail!(store, copy.notice);
            }
            (
                store,
                Ok((
                    SendOutcome {
                        kind: request.kind.clone(),
                        items,
                        to,
                        unconfirmed: settlement == Settlement::Unconfirmed,
                        copy: Some(copy),
                    },
                    adopt,
                )),
            )
        }
        Destination::Pane { pane, .. } => {
            let (pane_id, text_sent) = (pane.clone(), text.clone());
            // Late calls retain the lock until the host answers plus the Enter margin.
            let (answer, held_back) = call_host_late(
                ctx,
                move |h| h.agent_prompt(&pane_id, &text_sent),
                nonce.clone(),
                claimed,
                _held.take(),
            )
            .await;
            _held = held_back;
            // Keep the lock until the host has pressed Enter.
            tokio::time::sleep(ENTER_MARGIN).await;
            let settlement = match &answer {
                Ok(()) => Settlement::Sent,
                Err(host::HostFailure::After(_)) => Settlement::Unconfirmed,
                Err(
                    host::HostFailure::NoHost(_)
                    | host::HostFailure::Before(_)
                    | host::HostFailure::Api { .. },
                ) => Settlement::Failed,
            };
            if claimed {
                let n = nonce.clone();
                let (returned, settled) = blocking(move || {
                    let settled = store.settle(now, &n, settlement);
                    (store, settled)
                })
                .await;
                store = returned;
                if let Err(e) = settled {
                    bail!(store, e);
                }
            } else if settlement == Settlement::Failed {
                if let Some(dir) = ctx.state_dir.clone() {
                    let (top, n) = (ctx.toplevel.clone(), nonce.clone());
                    let _ = blocking(move || comments::remove_request(&dir, &top, &n)).await;
                }
            }
            match (answer, settlement) {
                (Err(failure), Settlement::Failed) => {
                    (store, Err((failure.message(), Refusal::Other)))
                }
                (_, settlement) => (
                    store,
                    Ok((
                        SendOutcome {
                            kind: request.kind.clone(),
                            items,
                            to,
                            unconfirmed: settlement == Settlement::Unconfirmed,
                            copy: None,
                        },
                        adopt,
                    )),
                ),
            }
        }
    }
}

/// The send lock, taken on the blocking pool; `waiting(true)` while another viewer holds it.
async fn hold_send_lock(
    path: PathBuf,
    waiting: Arc<dyn Fn(bool) + Send + Sync>,
) -> Result<SendLock, String> {
    // Publish a waiting notice only when the first lock attempt finds contention.
    let first = path.clone();
    match blocking(move || SendLock::try_take(&first)).await {
        Ok(Some(lock)) => return Ok(lock),
        Ok(None) => {}
        Err(e) => return Err(format!("{SEND_LOCK}: {e}")),
    }
    waiting(true);
    let taken = tokio::task::spawn_blocking(move || SendLock::take(&path, LOCK_WAIT))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r.map_err(|e| format!("{SEND_LOCK}: {e}")));
    waiting(false);
    taken
}

/// An exclusive `flock` on `send.lock`, released on drop. `Send`, so a late call can carry it.
pub struct SendLock {
    _file: std::fs::File,
}

impl SendLock {
    fn open(path: &Path) -> std::io::Result<std::fs::File> {
        if !path.is_absolute() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "send lock path must be absolute",
            ));
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
    }

    fn try_take(path: &Path) -> std::io::Result<Option<Self>> {
        use std::os::unix::io::AsRawFd;
        let file = Self::open(path)?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(Some(Self { _file: file }));
        }
        let error = std::io::Error::last_os_error();
        if matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
        ) {
            Ok(None)
        } else {
            Err(error)
        }
    }

    fn take(path: &Path, wait: Duration) -> std::io::Result<Self> {
        let start = std::time::Instant::now();
        loop {
            if let Some(lock) = Self::try_take(path)? {
                return Ok(lock);
            }
            if start.elapsed() >= wait {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "another viewer is still sending",
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// One host call on the blocking pool, bounded by the engine's wait (`ctx.host_wait`; `ENGINE_WAIT`
/// in production, shared with session.rs through `SessionConfig.host_wait`).
pub async fn call_host<T: Send + 'static>(
    host: Option<Arc<dyn HostClient>>,
    call: impl FnOnce(&dyn HostClient) -> Result<T, host::HostFailure> + Send + 'static,
) -> Result<T, host::HostFailure> {
    call_host_within(host, host::ENGINE_WAIT, call).await.0
}

/// `call_host` with the wait given; on a timeout the blocking task's handle comes back, so its late
/// answer can still be consumed.
pub async fn call_host_within<T: Send + 'static>(
    host: Option<Arc<dyn HostClient>>,
    wait: Duration,
    call: impl FnOnce(&dyn HostClient) -> Result<T, host::HostFailure> + Send + 'static,
) -> (
    Result<T, host::HostFailure>,
    Option<tokio::task::JoinHandle<Result<T, host::HostFailure>>>,
) {
    let Some(host) = host else {
        return (
            Err(host::HostFailure::NoHost(
                "no host: HERDR_SOCKET_PATH is unset".into(),
            )),
            None,
        );
    };
    let mut handle = tokio::task::spawn_blocking(move || call(host.as_ref()));
    match tokio::time::timeout(wait, &mut handle).await {
        Ok(Ok(result)) => (result, None),
        Ok(Err(e)) => (
            Err(host::HostFailure::After(format!("host call failed: {e}"))),
            None,
        ),
        Err(_) => (
            Err(host::HostFailure::After(
                "the host did not answer in time".into(),
            )),
            Some(handle),
        ),
    }
}

/// The prompt call: when the engine's wait passes, the records are settled `Unconfirmed` by the
/// caller, and the late answer, when it comes, settles them again by nonce (spec 10.6): `Sent` on
/// success, their earlier stamp or `Pending` on a definite failure; an `After` failure says nothing
/// new and leaves them. The send lock goes with a call that outlives the wait and is released only
/// after that call has answered and the Enter delay has passed; a call that answered in time gives
/// the lock back to the caller.
async fn call_host_late(
    ctx: &Context,
    call: impl FnOnce(&dyn HostClient) -> Result<(), host::HostFailure> + Send + 'static,
    nonce: String,
    claimed: bool,
    held: Option<SendLock>,
) -> (Result<(), host::HostFailure>, Option<SendLock>) {
    let (answer, late) = call_host_within(ctx.host.clone(), ctx.host_wait, call).await;
    let Some(handle) = late else {
        return (answer, held);
    };
    let (state_dir, toplevel, late_tx) = (
        ctx.state_dir.clone(),
        ctx.toplevel.clone(),
        ctx.late.clone(),
    );
    tokio::spawn(async move {
        let result = handle.await;
        // Release only after the host's delayed Enter.
        tokio::time::sleep(ENTER_MARGIN).await;
        drop(held);
        let Ok(result) = result else { return };
        let settlement = match result {
            Ok(()) => Settlement::Sent,
            Err(host::HostFailure::After(_)) => return,
            Err(_) => Settlement::Failed,
        };
        if claimed {
            // The store is the session's; the late settlement goes through it (StoreOp::LateAnswer).
            let _ = late_tx.send((nonce, settlement));
        } else if settlement == Settlement::Failed {
            if let Some(dir) = state_dir {
                let _ = blocking(move || comments::remove_request(&dir, &toplevel, &nonce)).await;
            }
        }
    });
    (answer, None)
}

/// `c` in a box, `y` on a selection: nothing is claimed; a request copy is recorded once the text
/// reached a destination (spec 10.4). Returns `None` for a superseded copy.
pub async fn copy(
    ctx: Context,
    request: CopyRequest,
    comments: Vec<comments::Comment>,
) -> Result<Option<(CopyOut, u64)>, String> {
    let (latest, generation) = (ctx.latest_copy.clone(), ctx.copy_generation);
    let guard = move || {
        if latest.load(Ordering::SeqCst) == generation {
            Ok(())
        } else {
            Err("copy superseded".to_string())
        }
    };
    if guard().is_err() {
        return Ok(None);
    }
    let mut counter = ctx.nonce_counter;
    let (text, what) = match &request.what {
        CopyWhat::Selection(text) => (text.clone(), "selection".to_string()),
        CopyWhat::Review => {
            let eligible: Vec<&comments::Comment> = comments
                .iter()
                .filter(|c| {
                    matches!(
                        c.state,
                        comments::CommentState::Pending
                            | comments::CommentState::Unconfirmed { .. }
                    )
                })
                .collect();
            if eligible.is_empty() {
                return Err(NOTICE_NO_PENDING.to_string());
            }
            // An unreadable store must not prevent copying the comments held in memory.
            let in_memory = || comments::nonces_of(&comments);
            let used: std::collections::BTreeSet<String> = match ctx.state_dir.clone() {
                Some(dir) => {
                    let own = comments.clone();
                    blocking(move || comments::nonces_in_use(&dir, &own))
                        .await
                        .unwrap_or_else(|_| in_memory())
                }
                None => in_memory(),
            };
            let nonce = loop {
                counter += 1;
                let candidate = (ctx.nonce)(counter);
                if !used.contains(&candidate) {
                    break candidate;
                }
            };
            let mut eligible = eligible;
            eligible.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
            let items: Vec<prompt::Item<'_>> = eligible
                .iter()
                .enumerate()
                .map(|(i, c)| prompt::Item {
                    number: i as u32 + 1,
                    comment: c,
                })
                .collect();
            (
                prompt::review(&ctx.toplevel, &items, &nonce),
                format!("{} comments", eligible.len()),
            )
        }
        CopyWhat::Request { scope } => {
            let (lines, files) = request_files(&ctx, scope).await?;
            if guard().is_err() {
                return Ok(None);
            }
            let scope_of = match comparison_of(&ctx.snapshot) {
                comments::AnchorComparison::Branch { merge_base, .. } => Some(merge_base),
                comments::AnchorComparison::Worktree => None,
            };
            // Reserve the nonce first and remove the record if the copy reaches nothing.
            let (toplevel, scope_of2, lines2) =
                (ctx.toplevel.clone(), scope_of.clone(), lines.clone());
            let text_of = Arc::new(move |nonce: &str| {
                prompt::request(
                    &toplevel,
                    &lines2,
                    match &scope_of2 {
                        Some(mb) => prompt::RequestScope::Branch { merge_base: mb },
                        None => prompt::RequestScope::Worktree,
                    },
                    nonce,
                )
            });
            let (nonce, text) = match &ctx.state_dir {
                Some(dir) => {
                    let (dir, top, make, mut c, files2, now, text_of2) = (
                        dir.clone(),
                        ctx.toplevel.clone(),
                        ctx.nonce.clone(),
                        counter,
                        files.clone(),
                        (ctx.clock)(),
                        text_of.clone(),
                    );
                    let (latest, generation) = (ctx.latest_generation.clone(), ctx.generation);
                    let copy_guard = guard.clone();
                    let (nonce, text, c) = blocking(move || {
                        // Refuse a copy whose target changed while it waited.
                        let guard = || {
                            copy_guard()?;
                            if latest.load(Ordering::SeqCst) == generation {
                                Ok(())
                            } else {
                                Err("the target changed; press @ again".to_string())
                            }
                        };
                        let (nonce, text) = comments::record_request_fresh(
                            &dir,
                            &top,
                            make.as_ref(),
                            &mut c,
                            &guard,
                            |nonce| {
                                Ok((
                                    RequestRecord {
                                        nonce: nonce.to_string(),
                                        at: now,
                                        target: Destination::clipboard(),
                                        files: files2,
                                    },
                                    text_of2(nonce),
                                ))
                            },
                        )?;
                        Ok::<_, std::io::Error>((nonce, text, c))
                    })
                    .await
                    .map_err(|e| format!("request not recorded: {e}"))?;
                    counter = c;
                    (nonce, text)
                }
                None => {
                    let nonce = fresh_nonce(
                        ctx.nonce.as_ref(),
                        &mut counter,
                        &comments::nonces_of(&comments),
                    );
                    (nonce.clone(), text_of(&nonce))
                }
            };
            let (dir, text2) = (ctx.state_dir.clone(), text.clone());
            let copied = blocking(move || {
                copy_out(
                    dir.as_deref(),
                    &text2,
                    &format!("a review request for {} files", lines.len()),
                    &guard,
                )
            })
            .await;
            if copied
                .as_ref()
                .is_none_or(|(out, file)| file.is_err() && out.osc.is_none())
            {
                if let Some(dir) = ctx.state_dir.clone() {
                    let (top, n) = (ctx.toplevel.clone(), nonce.clone());
                    let _ = blocking(move || comments::remove_request(&dir, &top, &n)).await;
                }
            }
            let Some((out, file)) = copied else {
                return Ok(None);
            };
            if file.is_err() && out.osc.is_none() {
                return Err(out.notice);
            }
            return Ok(Some((out, counter)));
        }
    };
    let (dir, text2) = (ctx.state_dir.clone(), text.clone());
    let Some((out, file)) = blocking(move || copy_out(dir.as_deref(), &text2, &what, &guard)).await
    else {
        return Ok(None);
    };
    if file.is_err() && out.osc.is_none() {
        return Err(out.notice);
    }
    Ok(Some((out, counter)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_out_rechecks_supersession_under_the_lock_and_before_osc() {
        for superseded_at in [2, 3] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(CLIPBOARD_FILE);
            std::fs::write(&path, "newer").unwrap();
            let calls = std::cell::Cell::new(0);
            let guard = || {
                calls.set(calls.get() + 1);
                if calls.get() >= superseded_at {
                    Err("copy superseded".into())
                } else {
                    Ok(())
                }
            };
            assert!(copy_out(Some(dir.path()), "older", "selection", &guard).is_none());
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                if superseded_at == 2 { "newer" } else { "older" }
            );
        }
    }

    #[test]
    fn send_lock_rejects_relative_paths_before_opening_them() {
        let error = SendLock::open(Path::new(".")).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(error.to_string(), "send lock path must be absolute");
    }
}
