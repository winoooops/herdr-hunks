//! The comments of a review (spec 10.3): records beside the diff, shared through comments.json.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use super::base;
use super::nav::Side;
use super::prompt::is_nonce;
use super::target::Destination;
use super::FileKey;
use crate::actions::reuse;

pub const CAP: usize = 50;
pub const MAX_CHARS: usize = 4_000;
pub const MAX_LINES: usize = 100;
pub const EXPIRY: Duration = Duration::from_secs(60);
pub const NOTICE_CAP: &str = "50 comments unsent; send or delete some first.";
pub const NOTICE_LIMIT: &str = "comment limit: 4,000 characters, 100 lines";
/// An accepted operation retained only in memory or in the journal.
pub const NOTICE_NOT_REMEMBERED: &str = "comments not remembered: ";
pub const NOTICE_NOTHING: &str = "nothing to send: another viewer sent these comments";
const COMMENTS_FILE: &str = "comments.json";
const REQUESTS_FILE: &str = "requests.json";

fn notice_repository_changed(count: usize) -> String {
    format!(
        "the repository changed; {count} unsaved comment change{} dropped",
        if count == 1 { "" } else { "s" }
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Question,
    Change,
    Bug,
    Suggestion,
}

impl Category {
    pub const ALL: [Category; 4] = [Self::Question, Self::Change, Self::Bug, Self::Suggestion];

    pub fn label(self) -> &'static str {
        match self {
            Self::Question => "Question",
            Self::Change => "Change request",
            Self::Bug => "Bug",
            Self::Suggestion => "Suggestion",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Self::Question => "Question",
            Self::Change => "Change",
            Self::Bug => "Bug",
            Self::Suggestion => "Suggestion",
        }
    }

    pub fn instruction(self) -> &'static str {
        match self {
            Self::Question => "Answer inline in your reply. Do not edit files.",
            Self::Change => "Make this change.",
            Self::Bug => "Fix this.",
            Self::Suggestion => "Apply this if you agree.",
        }
    }

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|c| *c == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub fn previous(self) -> Self {
        let i = Self::ALL.iter().position(|c| *c == self).unwrap_or(0);
        Self::ALL[(i + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "span", rename_all = "lowercase")]
pub enum Span {
    Line,
    Range { end: u32 },
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum AnchorComparison {
    Worktree,
    Branch { merge_base: String, label: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    pub key: FileKey,
    pub side: Side,
    pub line: u32,
    #[serde(flatten)]
    pub span: Span,
    pub comparison: AnchorComparison,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    pub at: u64,
    pub nonce: String,
    pub item: u32,
    pub to: Destination,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum CommentState {
    Pending,
    Sending {
        #[serde(flatten)]
        stamp: Stamp,
        /// Earlier uncertain attempts, newest first, restored on a definite failure.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        before: Vec<Stamp>,
    },
    Unconfirmed {
        #[serde(flatten)]
        stamp: Stamp,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        before: Vec<Stamp>,
    },
    Sent(Stamp),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    pub anchor: Anchor,
    pub category: Category,
    pub text: String,
    pub created_at: u64,
    #[serde(flatten)]
    pub state: CommentState,
}

impl Comment {
    pub fn is_unsent(&self) -> bool {
        !matches!(self.state, CommentState::Sent(_))
    }

    pub fn is_pending(&self) -> bool {
        matches!(self.state, CommentState::Pending)
    }

    pub fn is_editable(&self) -> bool {
        matches!(
            self.state,
            CommentState::Pending | CommentState::Unconfirmed { .. }
        )
    }

    pub fn stamp(&self) -> Option<&Stamp> {
        match &self.state {
            CommentState::Pending => None,
            CommentState::Sending { stamp, .. } | CommentState::Unconfirmed { stamp, .. } => {
                Some(stamp)
            }
            CommentState::Sent(s) => Some(s),
        }
    }
}

pub fn new_id() -> String {
    use sha2::Digest;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    // A monotonic time prefix orders comments created in the same second.
    let mut stamp = nanos as u64;
    loop {
        let last = LAST.load(std::sync::atomic::Ordering::SeqCst);
        if stamp <= last {
            stamp = last + 1;
        }
        if LAST
            .compare_exchange(
                last,
                stamp,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_ok()
        {
            break;
        }
    }
    let mut hasher = sha2::Sha256::new();
    hasher.update(stamp.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hasher.update(counter.to_le_bytes());
    let digest = hasher.finalize();
    let tail: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("{stamp:016x}{tail}")
}

pub fn line_count(text: &str) -> usize {
    text.split('\n').count()
}

pub fn within_caps(text: &str) -> bool {
    text.chars().count() <= MAX_CHARS && line_count(text) <= MAX_LINES
}

pub fn check_text(text: &str) -> Result<String, String> {
    let clean: String = text
        .chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .collect();
    if !within_caps(&clean) {
        return Err(NOTICE_LIMIT.to_string());
    }
    Ok(clean)
}

fn stamp_ok(stamp: &Stamp) -> bool {
    is_nonce(&stamp.nonce) && stamp.item >= 1 && stamp.to.is_well_formed()
}

fn path_ok(path: &str) -> bool {
    let p = Path::new(path);
    !path.is_empty()
        && !path.contains('\0')
        && p.is_relative()
        && !p
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
}

fn comparison_ok(comparison: &AnchorComparison) -> bool {
    match comparison {
        AnchorComparison::Worktree => true,
        AnchorComparison::Branch { merge_base, label } => {
            base::is_object_id(merge_base) && super::target::printable(label)
        }
    }
}

pub fn check_record(comment: &Comment) -> bool {
    let path_valid = path_ok(&comment.anchor.key.path);
    let span_ok = match comment.anchor.span {
        Span::Line => comment.anchor.line >= 1,
        Span::Range { end } => comment.anchor.line >= 1 && end >= comment.anchor.line,
        Span::File => comment.anchor.line == 0 && comment.anchor.side == Side::Additions,
    };
    let comparison_valid = comparison_ok(&comment.anchor.comparison);
    let text_ok =
        comment.text.chars().all(|c| c == '\n' || !c.is_control()) && within_caps(&comment.text);
    let id_ok = (comment.id.len() == 32 && comment.id.bytes().all(|b| b.is_ascii_hexdigit()))
        || (!comment.id.is_empty()
            && comment.id.len() <= 32
            && comment.id.bytes().all(|b| b.is_ascii_alphanumeric()));
    let state_ok = match &comment.state {
        CommentState::Pending => true,
        CommentState::Sending { stamp, before } | CommentState::Unconfirmed { stamp, before } => {
            stamp_ok(stamp) && before.iter().all(stamp_ok)
        }
        CommentState::Sent(s) => stamp_ok(s),
    };
    path_valid && span_ok && comparison_valid && text_ok && id_ok && state_ok
}

pub fn expire(comments: &mut [Comment], now: u64) -> bool {
    let mut changed = false;
    for comment in comments {
        if let CommentState::Sending { stamp, before } = &comment.state {
            if now.saturating_sub(stamp.at) >= EXPIRY.as_secs() {
                comment.state = CommentState::Unconfirmed {
                    stamp: stamp.clone(),
                    before: before.clone(),
                };
                changed = true;
            }
        }
    }
    changed
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claimed {
    pub comments: Vec<Comment>,
    pub text: String,
    pub nonce: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settlement {
    Sent,
    Unconfirmed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    Add(Comment),
    Edit {
        id: String,
        category: Category,
        text: String,
        seen: Comment,
    },
    Delete {
        id: String,
        seen: Comment,
    },
}

impl Operation {
    fn is_add(&self) -> bool {
        matches!(self, Self::Add(_))
    }

    fn describe(&self) -> &'static str {
        match self {
            Self::Add(_) => "add",
            Self::Edit { .. } => "edit",
            Self::Delete { .. } => "deletion",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TxError {
    Refused(String),
    Io(String),
}

fn apply(comments: &mut Vec<Comment>, op: &Operation) -> Result<bool, String> {
    match op {
        Operation::Add(comment) => {
            if comments.iter().any(|c| c.id == comment.id) {
                return Ok(true);
            }
            if comments.iter().filter(|c| c.is_unsent()).count() >= CAP {
                return Err(NOTICE_CAP.to_string());
            }
            comments.push(comment.clone());
            Ok(true)
        }
        Operation::Edit {
            id,
            category,
            text,
            seen,
        } => {
            let Some(current) = comments.iter_mut().find(|c| &c.id == id) else {
                return Ok(false);
            };
            if current != seen || !current.is_editable() {
                return Ok(false);
            }
            current.category = *category;
            current.text = text.clone();
            // A late answer for the old text must never mark this edit sent.
            current.state = CommentState::Pending;
            Ok(true)
        }
        Operation::Delete { id, seen } => {
            let Some(index) = comments.iter().position(|c| &c.id == id) else {
                return Ok(false);
            };
            if &comments[index] != seen || !comments[index].is_editable() {
                return Ok(false);
            }
            comments.remove(index);
            Ok(true)
        }
    }
}

fn read_file(path: &Path) -> Result<Option<(String, SystemTime)>, String> {
    match std::fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!(
            "{}: {e}",
            path.file_name().unwrap_or_default().to_string_lossy()
        )),
        Ok(text) => {
            let mtime = std::fs::metadata(path)
                .and_then(|m| m.modified())
                .map_err(|e| e.to_string())?;
            Ok(Some((text, mtime)))
        }
    }
}

fn parse_comments(
    text: &str,
    toplevel: &str,
) -> (
    BTreeMap<String, serde_json::Value>,
    Vec<Comment>,
    Option<String>,
) {
    let mut all: BTreeMap<String, serde_json::Value> = match serde_json::from_str(text) {
        Ok(all) => all,
        Err(e) => {
            return (
                BTreeMap::new(),
                Vec::new(),
                Some(format!("{COMMENTS_FILE}: {e}")),
            )
        }
    };
    let raw = match all.remove(toplevel) {
        None => Vec::new(),
        Some(serde_json::Value::Array(items)) => items,
        Some(_) => {
            return (
                all,
                Vec::new(),
                Some(format!(
                    "{COMMENTS_FILE}: this worktree's entry is not a list"
                )),
            )
        }
    };
    let total = raw.len();
    let mine: Vec<Comment> = raw
        .into_iter()
        .filter_map(|v| serde_json::from_value::<Comment>(v).ok())
        .filter(check_record)
        .collect();
    let dropped = total - mine.len();
    let problem = (dropped > 0).then(|| format!("{COMMENTS_FILE}: {dropped} unusable record(s)"));
    (all, mine, problem)
}

pub struct Store {
    state_dir: Option<PathBuf>,
    toplevel: String,
    comments: Vec<Comment>,
    /// The file's records before unsaved adds are overlaid for display.
    persisted: Vec<Comment>,
    journal: Vec<Operation>,
    /// Why unsaved operations prevent a send from claiming comments.
    journal_reason: Option<String>,
    /// Neighbouring worktrees are written back unchanged.
    others: BTreeMap<String, serde_json::Value>,
    mtime: Option<SystemTime>,
    memory_warning_shown: bool,
    #[cfg(test)]
    reads: usize,
    #[cfg(test)]
    fail_writes: bool,
}

impl Store {
    pub fn open(state_dir: Option<PathBuf>, toplevel: &str, now: u64) -> (Self, Vec<String>) {
        let mut store = Self {
            state_dir,
            toplevel: toplevel.to_string(),
            comments: Vec::new(),
            persisted: Vec::new(),
            journal: Vec::new(),
            journal_reason: None,
            others: BTreeMap::new(),
            mtime: None,
            memory_warning_shown: false,
            #[cfg(test)]
            reads: 0,
            #[cfg(test)]
            fail_writes: false,
        };
        let mut problems = Vec::new();
        if store.state_dir.is_some() {
            match store.locked(|store| store.read_and_apply(None, now)) {
                Ok(Ok((_, problem))) => problems.extend(problem),
                Ok(Err(TxError::Refused(e) | TxError::Io(e)))
                | Err(TxError::Refused(e) | TxError::Io(e)) => problems.push(e),
            }
        }
        (store, problems)
    }

    pub fn comments(&self) -> &[Comment] {
        &self.comments
    }

    pub fn toplevel(&self) -> &str {
        &self.toplevel
    }

    pub fn rebind(mut self, toplevel: &str, now: u64) -> (Store, Vec<String>, Option<String>) {
        if self.toplevel == toplevel {
            return (self, Vec::new(), None);
        }
        let (mut problems, replay_notice) = if self.journal.is_empty() {
            (Vec::new(), None)
        } else {
            self.refresh(now)
        };
        let lost = if self.state_dir.is_some() {
            self.journal.len()
        } else {
            self.comments.iter().filter(|c| c.is_editable()).count()
        };
        let (store, opened) = Store::open(self.state_dir, toplevel, now);
        problems.extend(opened);
        let notice = match (
            replay_notice,
            (lost > 0).then(|| notice_repository_changed(lost)),
        ) {
            (Some(replay), Some(loss)) => Some(format!("{replay} · {loss}")),
            (replay, loss) => replay.or(loss),
        };
        (store, problems, notice)
    }

    pub fn journal_len(&self) -> usize {
        self.journal.len()
    }

    pub fn path(&self) -> Option<PathBuf> {
        self.state_dir.as_ref().map(|d| d.join(COMMENTS_FILE))
    }

    fn locked<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> Result<T, TxError> {
        let Some(dir) = self.state_dir.clone() else {
            return Err(TxError::Io("no state directory".to_string()));
        };
        if !dir.is_absolute() {
            return Err(TxError::Io("state directory must be absolute".to_string()));
        }
        let mut result = None;
        reuse::with_lock(&dir, || result = Some(f(self)))
            .map_err(|e| TxError::Io(format!("{COMMENTS_FILE}: {e}")))?;
        Ok(result.expect("the locked closure ran"))
    }

    fn show_journaled_adds(&mut self) {
        // A full shared store must not hide this viewer's unsaved text.
        for entry in &self.journal {
            if let Operation::Add(comment) = entry {
                if !self.comments.iter().any(|c| c.id == comment.id) {
                    self.comments.push(comment.clone());
                }
            }
        }
    }

    fn read_and_apply(
        &mut self,
        op: Option<&Operation>,
        now: u64,
    ) -> Result<(Option<String>, Option<String>), TxError> {
        let dir = self
            .state_dir
            .clone()
            .expect("locked() checked the directory");
        let path = dir.join(COMMENTS_FILE);
        #[cfg(test)]
        {
            self.reads += 1;
        }
        let (all, mut comments, problem) = match read_file(&path).map_err(TxError::Io)? {
            None => (BTreeMap::new(), Vec::new(), None),
            Some((text, _)) => parse_comments(&text, &self.toplevel),
        };
        self.others = all.clone();
        // Failed expiry writes still publish the records just read.
        let loaded = comments.clone();
        let mut changed = false;
        let mut dropped = None;
        let mut kept_journal = Vec::new();
        for entry in self.journal.clone() {
            if matches!(&entry, Operation::Add(c) if comments.iter().any(|saved| saved.id == c.id))
            {
                continue;
            }
            match apply(&mut comments, &entry) {
                Ok(true) => changed = true,
                Ok(false) => {
                    dropped = Some(format!(
                        "a comment changed under you; your {} was dropped",
                        entry.describe()
                    ))
                }
                Err(_) => kept_journal.push(entry),
            }
        }
        let refusal = match op {
            Some(op) => match apply(&mut comments, op) {
                Ok(true) => {
                    changed = true;
                    None
                }
                Ok(false) => Some(match op {
                    Operation::Edit { .. } | Operation::Delete { .. } => {
                        "No comment selected.".to_string()
                    }
                    Operation::Add(_) => unreachable!("an add never meets a changed record"),
                }),
                Err(e) => Some(e),
            },
            None => None,
        };
        changed |= expire(&mut comments, now);
        if changed {
            if let Some(problem) = &problem {
                base::note_problem(&dir, &format!("{problem}; rewritten without them"));
            }
            let written = self.write_locked_with(&comments);
            if let Err(e) = written {
                // Restore the file's records, then overlay this viewer's unsaved work.
                self.persisted = loaded.clone();
                self.comments = loaded;
                for entry in &self.journal {
                    let _ = apply(&mut self.comments, entry);
                }
                self.show_journaled_adds();
                self.mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                return Err(e);
            }
        }
        self.journal = kept_journal;
        if self.journal.is_empty() {
            self.journal_reason = None;
        }
        self.mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        self.persisted = comments.clone();
        self.comments = comments;
        self.show_journaled_adds();
        if let Some(refusal) = refusal {
            return Err(TxError::Refused(match dropped {
                Some(dropped) => format!("{refusal} {dropped}"),
                None => refusal,
            }));
        }
        Ok((dropped, problem))
    }

    fn write_locked_with(&self, comments: &[Comment]) -> Result<(), TxError> {
        let dir = self
            .state_dir
            .as_ref()
            .expect("locked() checked the directory");
        #[cfg(test)]
        if self.fail_writes {
            return Err(TxError::Io(format!("{COMMENTS_FILE}: write failed (test)")));
        }
        let mut all = self.others.clone();
        all.insert(
            self.toplevel.clone(),
            serde_json::to_value(comments).map_err(|e| TxError::Io(e.to_string()))?,
        );
        let tmp = dir.join(format!("{COMMENTS_FILE}.{}.tmp", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec_pretty(&all).unwrap_or_default())
            .map_err(|e| TxError::Io(format!("{COMMENTS_FILE}: {e}")))?;
        std::fs::rename(&tmp, dir.join(COMMENTS_FILE))
            .map_err(|e| TxError::Io(format!("{COMMENTS_FILE}: {e}")))
    }

    pub fn transact(&mut self, op: Operation, now: u64) -> Result<Option<String>, String> {
        if self.state_dir.is_none() {
            let applied = apply(&mut self.comments, &op)?;
            if !applied {
                return Err("No comment selected.".to_string());
            }
            return if op.is_add() && !std::mem::replace(&mut self.memory_warning_shown, true) {
                Err(format!("{NOTICE_NOT_REMEMBERED}no state directory"))
            } else {
                Ok(None)
            };
        }
        if let Operation::Delete { id, seen } | Operation::Edit { id, seen, .. } = &op {
            // A record held only by an unsaved add is changed in the journal itself.
            if let Some(index) = self
                .journal
                .iter()
                .position(|e| matches!(e, Operation::Add(c) if &c.id == id))
            {
                if !self
                    .comments
                    .iter()
                    .any(|c| &c.id == id && c == seen && c.is_editable())
                {
                    return Err("No comment selected.".to_string());
                }
                match &op {
                    Operation::Delete { .. } => {
                        self.journal.remove(index);
                        if self.journal.is_empty() {
                            self.journal_reason = None;
                        }
                        self.comments.retain(|c| &c.id != id);
                    }
                    Operation::Edit { category, text, .. } => {
                        if let Operation::Add(c) = &mut self.journal[index] {
                            c.category = *category;
                            c.text = text.clone();
                        }
                        if let Some(c) = self.comments.iter_mut().find(|c| &c.id == id) {
                            c.category = *category;
                            c.text = text.clone();
                        }
                    }
                    Operation::Add(_) => unreachable!(),
                }
                return Ok(None);
            }
        }
        match self.locked(|store| store.read_and_apply(Some(&op), now)) {
            Ok(Ok((dropped, _))) => Ok(dropped),
            Ok(Err(TxError::Refused(notice))) | Err(TxError::Refused(notice)) => Err(notice),
            Ok(Err(TxError::Io(e))) | Err(TxError::Io(e)) => {
                // Only operations accepted by the same rules may enter the journal.
                match apply(&mut self.comments, &op) {
                    Err(refusal) => return Err(refusal),
                    Ok(false) => return Err("No comment selected.".to_string()),
                    Ok(true) => {}
                }
                self.journal.push(op);
                self.journal_reason = Some(e.clone());
                Err(format!("{NOTICE_NOT_REMEMBERED}{e}"))
            }
        }
    }

    pub fn refresh(&mut self, now: u64) -> (Vec<String>, Option<String>) {
        let Some(path) = self.path() else {
            expire(&mut self.comments, now);
            return (Vec::new(), None);
        };
        let moved = std::fs::metadata(&path).and_then(|m| m.modified()).ok() != self.mtime;
        let expiring = self.comments.iter().any(|c| matches!(&c.state, CommentState::Sending { stamp, .. } if now.saturating_sub(stamp.at) >= EXPIRY.as_secs()));
        if !moved && !expiring && self.journal.is_empty() {
            return (Vec::new(), None);
        }
        match self.locked(|store| store.read_and_apply(None, now)) {
            Ok(Ok((dropped, problem))) => (problem.into_iter().collect(), dropped),
            Ok(Err(TxError::Refused(e) | TxError::Io(e)))
            | Err(TxError::Refused(e) | TxError::Io(e)) => (vec![e], None),
        }
    }

    /// Spec 10.4 steps 2 and 3 in one transaction: the nonce is chosen against the records as they
    /// are under the lock, so no viewer that waited on `send.lock` can have introduced it since.
    /// `build` receives the eligible records numbered `[#1..n]` and the nonce, and returns the text
    /// to send, or a refusal (the bound) that stamps nothing.
    pub fn claim(
        &mut self,
        now: u64,
        to: &Destination,
        make: &dyn Fn(u64) -> String,
        counter: &mut u64,
        guard: &dyn Fn() -> Result<(), String>,
        build: impl FnOnce(&[(u32, &Comment)], &str) -> Result<String, String>,
    ) -> Result<Claimed, String> {
        if !self.journal.is_empty() {
            return Err(format!(
                "comments not saved: {}; fix it before sending",
                self.journal_reason
                    .clone()
                    .unwrap_or_else(|| "the store could not be written".into())
            ));
        }
        let mut build = Some(build);
        let mut body = |store: &mut Self| -> Result<Claimed, TxError> {
            let mut working = if store.state_dir.is_some() {
                store.read_and_apply(None, now)?;
                store.persisted.clone()
            } else {
                expire(&mut store.comments, now);
                store.comments.clone()
            };
            // The caller's last word before anything is stamped: the send's selection generation.
            guard().map_err(TxError::Refused)?;
            let used = match &store.state_dir {
                Some(dir) => {
                    nonces_in_use(dir, &working).map_err(|e| TxError::Io(format!("nonces: {e}")))?
                }
                None => nonces_of(&working),
            };
            let nonce = loop {
                *counter += 1;
                let candidate = make(*counter);
                if !used.contains(&candidate) {
                    break candidate;
                }
            };
            // Order by creation time and id, including replayed comments.
            let mut ordered: Vec<&Comment> = working
                .iter()
                .filter(|c| {
                    matches!(
                        c.state,
                        CommentState::Pending | CommentState::Unconfirmed { .. }
                    )
                })
                .collect();
            ordered.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
            let eligible: Vec<(u32, &Comment)> = ordered
                .into_iter()
                .enumerate()
                .map(|(i, c)| (i as u32 + 1, c))
                .collect();
            if eligible.is_empty() {
                return Err(TxError::Refused(NOTICE_NOTHING.to_string()));
            }
            let text =
                (build.take().expect("built once"))(&eligible, &nonce).map_err(TxError::Refused)?;
            let ids: Vec<(String, u32)> =
                eligible.iter().map(|(n, c)| (c.id.clone(), *n)).collect();
            // Stamped on a working copy: the array shows the claim only once the file holds it.
            let mut claimed = Vec::new();
            for comment in &mut working {
                if let Some((_, item)) = ids.iter().find(|(id, _)| id == &comment.id) {
                    // Retain every uncertain attempt, newest first.
                    let before = match &comment.state {
                        CommentState::Unconfirmed { stamp, before } => {
                            std::iter::once(stamp.clone())
                                .chain(before.iter().cloned())
                                .collect()
                        }
                        _ => Vec::new(),
                    };
                    comment.state = CommentState::Sending {
                        stamp: Stamp {
                            at: now,
                            nonce: nonce.clone(),
                            item: *item,
                            to: to.clone(),
                        },
                        before,
                    };
                    claimed.push(comment.clone());
                }
            }
            if store.state_dir.is_some() {
                store.write_locked_with(&working)?;
            }
            store.persisted = working.clone();
            store.comments = working;
            store.show_journaled_adds();
            Ok(Claimed {
                comments: claimed,
                text,
                nonce,
            })
        };
        let outcome = if self.state_dir.is_some() {
            self.locked(|store| body(store))
        } else {
            // The array is the store (spec 10.3): a claim and its settlement run in memory.
            Ok(body(self))
        };
        match outcome {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(TxError::Refused(e) | TxError::Io(e)))
            | Err(TxError::Refused(e) | TxError::Io(e)) => Err(e),
        }
    }

    /// Spec 10.4 step 5: records still `Sending` under `nonce`, and `Unconfirmed` ones when a late
    /// answer arrives after the engine's wait (spec 10.6); never the cap. On a working copy, so a
    /// failed write leaves the array as the file has it.
    pub fn settle(&mut self, now: u64, nonce: &str, outcome: Settlement) -> Result<(), String> {
        let body = |store: &mut Self| -> Result<(), TxError> {
            let mut working = if store.state_dir.is_some() {
                store.read_and_apply(None, now)?;
                store.persisted.clone()
            } else {
                store.comments.clone()
            };
            let mut changed = false;
            for comment in &mut working {
                // Late successes settle earlier attempts; failures remove them from the chain.
                if let CommentState::Sending { before, .. }
                | CommentState::Unconfirmed { before, .. } = &mut comment.state
                {
                    if let Some(index) = before.iter().position(|b| b.nonce == nonce) {
                        let arrived = match outcome {
                            Settlement::Sent => Some(before[index].clone()),
                            Settlement::Failed => {
                                before.remove(index);
                                None
                            }
                            Settlement::Unconfirmed => None,
                        };
                        if let Some(earlier) = arrived {
                            comment.state = CommentState::Sent(earlier);
                        }
                        changed = true;
                        continue;
                    }
                }
                let (stamp, before) = match &comment.state {
                    CommentState::Sending { stamp, before } if stamp.nonce == nonce => {
                        (stamp.clone(), before.clone())
                    }
                    // A late answer overrides the timeout for the same nonce.
                    CommentState::Unconfirmed { stamp, before }
                        if stamp.nonce == nonce && outcome != Settlement::Unconfirmed =>
                    {
                        (stamp.clone(), before.clone())
                    }
                    _ => continue,
                };
                comment.state = match outcome {
                    Settlement::Sent => CommentState::Sent(stamp),
                    Settlement::Unconfirmed => CommentState::Unconfirmed { stamp, before },
                    // Restore the newest earlier attempt because it may have arrived.
                    Settlement::Failed => match before.split_first() {
                        Some((earlier, rest)) => CommentState::Unconfirmed {
                            stamp: earlier.clone(),
                            before: rest.to_vec(),
                        },
                        None => CommentState::Pending,
                    },
                };
                changed = true;
            }
            if changed {
                if store.state_dir.is_some() {
                    store.write_locked_with(&working)?;
                }
                store.persisted = working.clone();
                store.comments = working;
                store.show_journaled_adds();
            }
            Ok(())
        };
        let result = if self.state_dir.is_some() {
            self.locked(body)
        } else {
            Ok(body(self))
        };
        match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(TxError::Refused(e) | TxError::Io(e)))
            | Err(TxError::Refused(e) | TxError::Io(e)) => Err(e),
        }
    }

    #[cfg(test)]
    pub fn comments_mut_for_tests(&mut self) -> &mut Vec<Comment> {
        &mut self.comments
    }

    #[cfg(test)]
    pub fn write_for_tests(&mut self) {
        let comments = self.comments.clone();
        self.set_for_tests(comments);
    }

    #[cfg(test)]
    pub fn set_for_tests(&mut self, comments: Vec<Comment>) {
        let dir = self.state_dir.clone().unwrap();
        let path = dir.join(COMMENTS_FILE);
        let mut all: BTreeMap<String, serde_json::Value> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        all.insert(
            self.toplevel.clone(),
            serde_json::to_value(&comments).unwrap(),
        );
        std::fs::write(&path, serde_json::to_vec_pretty(&all).unwrap()).unwrap();
        self.persisted = comments.clone();
        self.comments = comments;
        self.mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    }

    #[cfg(test)]
    pub fn reads_for_tests(&self) -> usize {
        self.reads
    }

    #[cfg(test)]
    pub fn open_for_tests_with_failing_writes(state_dir: &Path, toplevel: &str, now: u64) -> Self {
        let mut store = Self {
            state_dir: Some(state_dir.to_path_buf()),
            toplevel: toplevel.to_string(),
            comments: Vec::new(),
            persisted: Vec::new(),
            journal: Vec::new(),
            journal_reason: None,
            others: BTreeMap::new(),
            mtime: None,
            memory_warning_shown: false,
            reads: 0,
            fail_writes: true,
        };
        let _ = store.locked(|store| store.read_and_apply(None, now));
        store
    }

    #[cfg(test)]
    pub fn fail_writes_for_tests(&mut self, fail: bool) {
        self.fail_writes = fail;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestFile {
    pub key: FileKey,
    pub comparison: AnchorComparison,
    pub additions: Vec<(u32, u32)>,
    pub deletions: Vec<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestRecord {
    pub nonce: String,
    pub at: u64,
    pub target: Destination,
    pub files: Vec<RequestFile>,
}

fn check_request(record: &RequestRecord) -> bool {
    is_nonce(&record.nonce)
        && record.target.is_well_formed()
        && record.files.iter().all(|f| {
            path_ok(&f.key.path)
                && comparison_ok(&f.comparison)
                && f.additions
                    .iter()
                    .chain(&f.deletions)
                    .all(|(a, b)| *a >= 1 && a <= b)
        })
}

fn read_request_map(
    path: &Path,
) -> std::io::Result<(BTreeMap<String, serde_json::Value>, Option<String>)> {
    match std::fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((BTreeMap::new(), None)),
        Err(e) => Err(e),
        Ok(text) => match serde_json::from_str::<BTreeMap<String, serde_json::Value>>(&text) {
            Ok(all) => Ok((all, None)),
            Err(e) => Ok((
                BTreeMap::new(),
                Some(format!(
                    "{REQUESTS_FILE}: not a JSON object ({e}); its records are discarded"
                )),
            )),
        },
    }
}

fn parse_requests(
    all: &BTreeMap<String, serde_json::Value>,
    toplevel: &str,
) -> (Vec<RequestRecord>, Option<String>) {
    let raw: Vec<serde_json::Value> = match all.get(toplevel) {
        None => Vec::new(),
        Some(serde_json::Value::Array(items)) => items.clone(),
        Some(_) => {
            return (
                Vec::new(),
                Some(format!(
                    "{REQUESTS_FILE}: this worktree's entry is not a list"
                )),
            )
        }
    };
    let total = raw.len();
    let kept: Vec<RequestRecord> = raw
        .into_iter()
        .filter_map(|v| serde_json::from_value::<RequestRecord>(v).ok())
        .filter(check_request)
        .collect();
    let dropped = total - kept.len();
    (
        kept,
        (dropped > 0).then(|| format!("{REQUESTS_FILE}: {dropped} unusable record(s)")),
    )
}

pub fn load_requests(state_dir: &Path, toplevel: &str) -> (Vec<RequestRecord>, Option<String>) {
    match read_request_map(&state_dir.join(REQUESTS_FILE)) {
        Err(e) => (Vec::new(), Some(format!("{REQUESTS_FILE}: {e}"))),
        Ok((_, Some(problem))) => (Vec::new(), Some(problem)),
        Ok((all, None)) => parse_requests(&all, toplevel),
    }
}

fn write_request_map(
    state_dir: &Path,
    all: &BTreeMap<String, serde_json::Value>,
) -> std::io::Result<()> {
    let tmp = state_dir.join(format!("{REQUESTS_FILE}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(all).unwrap_or_default())?;
    std::fs::rename(tmp, state_dir.join(REQUESTS_FILE))
}

fn append_request(state_dir: &Path, toplevel: &str, record: &RequestRecord) -> std::io::Result<()> {
    let (mut all, problem) = read_request_map(&state_dir.join(REQUESTS_FILE))?;
    if let Some(problem) = problem {
        base::note_problem(state_dir, &problem);
    }
    let (mut mine, dropped) = parse_requests(&all, toplevel);
    if let Some(dropped) = dropped {
        base::note_problem(state_dir, &format!("{dropped}; rewritten without them"));
    }
    mine.push(record.clone());
    all.insert(
        toplevel.to_string(),
        serde_json::Value::Array(
            mine.iter()
                .map(|r| serde_json::to_value(r).unwrap_or_default())
                .collect(),
        ),
    );
    write_request_map(state_dir, &all)
}

pub fn record_request(
    state_dir: &Path,
    toplevel: &str,
    record: &RequestRecord,
) -> std::io::Result<()> {
    if !state_dir.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "state directory must be absolute",
        ));
    }
    reuse::with_lock(state_dir, || append_request(state_dir, toplevel, record))?
}

pub fn record_request_fresh(
    state_dir: &Path,
    toplevel: &str,
    make: &dyn Fn(u64) -> String,
    counter: &mut u64,
    guard: &dyn Fn() -> Result<(), String>,
    build: impl FnOnce(&str) -> Result<(RequestRecord, String), String>,
) -> std::io::Result<(String, String)> {
    if !state_dir.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "state directory must be absolute",
        ));
    }
    let mut build = Some(build);
    let mut chosen = (String::new(), String::new());
    reuse::with_lock(state_dir, || -> std::io::Result<()> {
        guard().map_err(|e| std::io::Error::new(std::io::ErrorKind::Interrupted, e))?;
        // Read both files under the lock before choosing a nonce.
        let used = nonces_in_use(state_dir, &[])?;
        let nonce = loop {
            *counter += 1;
            let candidate = make(*counter);
            if !used.contains(&candidate) {
                break candidate;
            }
        };
        let (record, text) = (build.take().expect("built once"))(&nonce)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Interrupted, e))?;
        append_request(state_dir, toplevel, &record)?;
        chosen = (nonce, text);
        Ok(())
    })??;
    Ok(chosen)
}

pub fn write_clipboard(
    state_dir: &Path,
    text: &str,
    guard: &dyn Fn() -> Result<(), String>,
) -> std::io::Result<PathBuf> {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if !state_dir.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "state directory must be absolute",
        ));
    }
    reuse::with_lock(state_dir, || -> std::io::Result<PathBuf> {
        guard().map_err(|e| std::io::Error::new(std::io::ErrorKind::Interrupted, e))?;
        let path = state_dir.join("clipboard.md");
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = state_dir.join(format!("clipboard.md.{}.{n}.tmp", std::process::id()));
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &path)?;
        Ok(path)
    })?
}

pub fn remove_request(state_dir: &Path, toplevel: &str, nonce: &str) -> std::io::Result<()> {
    if !state_dir.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "state directory must be absolute",
        ));
    }
    reuse::with_lock(state_dir, || {
        let (mut all, problem) = read_request_map(&state_dir.join(REQUESTS_FILE))?;
        if let Some(problem) = problem {
            base::note_problem(state_dir, &problem);
        }
        let (mine, dropped) = parse_requests(&all, toplevel);
        if let Some(dropped) = dropped {
            base::note_problem(state_dir, &format!("{dropped}; rewritten without them"));
        }
        let kept: Vec<_> = mine
            .into_iter()
            .filter(|r| r.nonce != nonce)
            .map(|r| serde_json::to_value(r).unwrap_or_default())
            .collect();
        all.insert(toplevel.to_string(), serde_json::Value::Array(kept));
        write_request_map(state_dir, &all)
    })?
}

pub fn nonces_of(comments: &[Comment]) -> BTreeSet<String> {
    comments
        .iter()
        .flat_map(|c| {
            let before: Vec<String> = match &c.state {
                CommentState::Sending { before, .. } | CommentState::Unconfirmed { before, .. } => {
                    before.iter().map(|b| b.nonce.clone()).collect()
                }
                _ => Vec::new(),
            };
            c.stamp().map(|s| s.nonce.clone()).into_iter().chain(before)
        })
        .collect()
}

pub fn nonces_in_use(state_dir: &Path, own: &[Comment]) -> std::io::Result<BTreeSet<String>> {
    let mut used: BTreeSet<String> = nonces_of(own);
    match std::fs::read_to_string(state_dir.join(COMMENTS_FILE)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
        Ok(text) => {
            if let Ok(all) = serde_json::from_str::<BTreeMap<String, serde_json::Value>>(&text) {
                for toplevel in all.keys() {
                    used.extend(nonces_of(&parse_comments(&text, toplevel).1));
                }
            }
        }
    }
    match std::fs::read_to_string(state_dir.join(REQUESTS_FILE)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
        Ok(text) => {
            if let Ok(all) = serde_json::from_str::<BTreeMap<String, serde_json::Value>>(&text) {
                used.extend(
                    all.values()
                        .filter_map(|v| v.as_array())
                        .flatten()
                        .filter_map(|v| v["nonce"].as_str().map(str::to_string)),
                );
            }
        }
    }
    Ok(used)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::nav::Side;

    #[test]
    fn rebind_returns_replay_notices_before_any_loss_notice() {
        for lose_add in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let mut store = open(dir.path());
            let seen = comment("original", "original", 1);
            store.transact(Operation::Add(seen.clone()), 1).unwrap();
            store.fail_writes_for_tests(true);
            store
                .transact(
                    Operation::Edit {
                        id: seen.id.clone(),
                        seen: seen.clone(),
                        category: Category::Bug,
                        text: "unsaved edit".into(),
                    },
                    2,
                )
                .unwrap_err();
            if lose_add {
                store
                    .transact(Operation::Add(comment("local", "unsaved add", 2)), 2)
                    .unwrap_err();
            }
            let mut other = open(dir.path());
            other
                .transact(
                    Operation::Edit {
                        id: seen.id.clone(),
                        seen,
                        category: Category::Question,
                        text: "other viewer".into(),
                    },
                    3,
                )
                .unwrap();
            if lose_add {
                for i in 1..CAP {
                    other
                        .transact(
                            Operation::Add(comment(&format!("other{i}"), "shared", 3)),
                            3,
                        )
                        .unwrap();
                }
            }
            store.fail_writes_for_tests(false);
            let (store, problems, notice) = store.rebind("/other", 4);
            let expected = if lose_add {
                "a comment changed under you; your edit was dropped · the repository changed; 1 unsaved comment change dropped"
            } else {
                "a comment changed under you; your edit was dropped"
            };
            assert_eq!(notice.as_deref(), Some(expected));
            assert!(problems.is_empty(), "{problems:?}");
            assert!(!dir.path().join("config-problems.log").exists());
            assert_eq!(store.toplevel(), "/other");
            assert_eq!(open(dir.path()).comments(), other.comments());
        }
    }

    #[test]
    fn rebind_to_the_same_toplevel_preserves_comments_without_io() {
        let dir = tempfile::tempdir().unwrap();
        for state_dir in [None, Some(dir.path().to_path_buf())] {
            let (mut store, _) = Store::open(state_dir, "/repo", 1);
            let pending = comment("a", "pending", 1);
            store.fail_writes_for_tests(true);
            store
                .transact(Operation::Add(pending.clone()), 1)
                .unwrap_err();
            let reads = store.reads;
            let journal = store.journal.clone();
            let (store, problems, notice) = store.rebind("/repo", 2);
            assert_eq!(store.comments(), &[pending]);
            assert!(problems.is_empty());
            assert_eq!(notice, None);
            assert_eq!(store.reads, reads);
            assert_eq!(store.journal, journal);
            assert!(store.fail_writes);
        }
    }

    #[test]
    fn rebind_reads_the_new_toplevel_without_changing_the_old_one() {
        let dir = tempfile::tempdir().unwrap();
        let old = comment("old", "old repo", 1);
        let new = comment("new", "new repo", 2);
        let mut store = open(dir.path());
        store.transact(Operation::Add(old.clone()), 1_000).unwrap();
        let mut other = Store::open(Some(dir.path().to_path_buf()), "/other", 1_000).0;
        other.transact(Operation::Add(new.clone()), 1_000).unwrap();
        let (store, problems, notice) = store.rebind("/other", 1_000);
        assert_eq!(store.toplevel(), "/other");
        assert_eq!(store.comments(), &[new]);
        assert!(problems.is_empty());
        assert_eq!(notice, None);
        assert_eq!(open(dir.path()).comments(), &[old]);
    }

    #[test]
    fn rebind_replays_the_old_journal_once_and_reports_what_is_lost() {
        for failing in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let mut store = open(dir.path());
            store.fail_writes_for_tests(true);
            let old = comment("old", "unsaved", 1);
            store
                .transact(Operation::Add(old.clone()), 1_000)
                .unwrap_err();
            assert_eq!(store.journal_len(), 1);
            store.fail_writes_for_tests(failing);
            let (store, problems, notice) = store.rebind("/other", 1_000);
            assert_eq!(store.toplevel(), "/other");
            assert!(store.comments().is_empty());
            assert_eq!(store.journal_len(), 0);
            if failing {
                assert_eq!(problems, ["comments.json: write failed (test)"]);
                assert_eq!(
                    notice.as_deref(),
                    Some("the repository changed; 1 unsaved comment change dropped")
                );
                assert!(open(dir.path()).comments().is_empty());
            } else {
                assert!(problems.is_empty());
                assert_eq!(notice, None);
                assert_eq!(open(dir.path()).comments(), &[old]);
            }
        }
    }

    #[test]
    fn rebind_counts_pending_and_unconfirmed_memory_comments() {
        for uncertain in [false, true] {
            let (mut store, _) = Store::open(None, "/repo", 1_000);
            store
                .transact(Operation::Add(comment("a", "first", 1)), 1_000)
                .unwrap_err();
            store
                .transact(Operation::Add(comment("b", "second", 2)), 1_000)
                .unwrap();
            if uncertain {
                store.comments[1].state = CommentState::Unconfirmed {
                    stamp: stamp("abcdef"),
                    before: Vec::new(),
                };
            }
            let mut sent = comment("s", "already sent", 3);
            sent.state = CommentState::Sent(stamp("abcdef"));
            store.comments.push(sent);
            let (store, problems, notice) = store.rebind("/other", 1_000);
            assert_eq!(store.toplevel(), "/other");
            assert!(store.comments().is_empty());
            assert!(problems.is_empty());
            assert_eq!(
                notice.as_deref(),
                Some("the repository changed; 2 unsaved comment changes dropped")
            );
        }
    }

    #[test]
    fn the_memory_only_warning_is_not_repeated_after_deleting_every_comment() {
        let (mut store, _) = Store::open(None, "/repo", 1);
        let seen = comment("a", "first", 1);
        store.transact(Operation::Add(seen.clone()), 1).unwrap_err();
        store
            .transact(
                Operation::Delete {
                    id: seen.id.clone(),
                    seen,
                },
                2,
            )
            .unwrap();
        assert!(store
            .transact(Operation::Add(comment("b", "second", 3)), 3)
            .is_ok());
    }

    #[test]
    fn journal_only_edits_and_deletions_still_compare_the_seen_record() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        store.fail_writes_for_tests(true);
        let seen = comment("a", "first", 1);
        store.transact(Operation::Add(seen.clone()), 1).unwrap_err();
        store
            .transact(
                Operation::Edit {
                    id: seen.id.clone(),
                    category: Category::Question,
                    text: "new".into(),
                    seen: seen.clone(),
                },
                2,
            )
            .unwrap();
        assert_eq!(
            store
                .transact(
                    Operation::Edit {
                        id: seen.id.clone(),
                        category: Category::Bug,
                        text: "stale".into(),
                        seen: seen.clone(),
                    },
                    3
                )
                .unwrap_err(),
            "No comment selected."
        );
        assert_eq!(
            store
                .transact(
                    Operation::Delete {
                        id: seen.id.clone(),
                        seen
                    },
                    4
                )
                .unwrap_err(),
            "No comment selected."
        );
        assert_eq!(store.comments()[0].text, "new");
        assert_eq!(store.journal_len(), 1);
    }

    #[test]
    fn a_non_list_comment_entry_is_reported_and_its_neighbours_survive() {
        let dir = tempfile::tempdir().unwrap();
        let other = comment("other", "kept", 1);
        std::fs::write(
            dir.path().join(COMMENTS_FILE),
            serde_json::json!({
                "/repo": "garbage", "/other": [other.clone()]
            })
            .to_string(),
        )
        .unwrap();
        let (mut store, problems) = Store::open(Some(dir.path().to_path_buf()), "/repo", 1);
        assert_eq!(
            problems,
            ["comments.json: this worktree's entry is not a list"]
        );
        store
            .transact(Operation::Add(comment("new", "new", 2)), 2)
            .unwrap();
        assert_eq!(
            Store::open(Some(dir.path().to_path_buf()), "/other", 2)
                .0
                .comments(),
            &[other]
        );
        assert!(
            std::fs::read_to_string(dir.path().join("config-problems.log"))
                .unwrap()
                .contains("rewritten without them")
        );
    }

    #[test]
    fn request_removal_rejects_relative_state_directories() {
        // A file blocks directory creation even before the absolute-path guard exists.
        assert!(Path::new("Cargo.toml").is_file());
        assert_eq!(
            remove_request(Path::new("Cargo.toml"), "/repo", "abcdef")
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
        let dir = tempfile::tempdir().unwrap();
        let path = write_clipboard(dir.path(), "first", &|| Ok(())).unwrap();
        assert_eq!(
            write_clipboard(dir.path(), "second", &|| Ok(())).unwrap(),
            path
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), "second");
    }

    fn anchor(path: &str, line: u32) -> Anchor {
        Anchor {
            key: FileKey {
                path: path.into(),
                staged: false,
                untracked: false,
            },
            side: Side::Additions,
            line,
            span: Span::Line,
            comparison: AnchorComparison::Worktree,
        }
    }

    fn comment(id: &str, text: &str, created_at: u64) -> Comment {
        Comment {
            id: id.into(),
            anchor: anchor("src/cart.py", 16),
            category: Category::Bug,
            text: text.into(),
            created_at,
            state: CommentState::Pending,
        }
    }

    fn stamp(nonce: &str) -> Stamp {
        Stamp {
            at: 100,
            nonce: nonce.into(),
            item: 1,
            to: Destination::clipboard(),
        }
    }

    fn open(dir: &std::path::Path) -> Store {
        Store::open(Some(dir.to_path_buf()), "/repo", 1_000).0
    }

    #[test]
    fn settlement_keeps_a_cap_refused_add_in_the_journal_and_off_disk() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = open(dir.path());
        let mut uncertain = comment("sent", "maybe delivered", 1);
        uncertain.state = CommentState::Unconfirmed {
            stamp: stamp("aaaaaa"),
            before: Vec::new(),
        };
        a.transact(Operation::Add(uncertain), 100).unwrap();
        a.fail_writes_for_tests(true);
        a.transact(Operation::Add(comment("local", "unsaved", 2)), 101)
            .unwrap_err();
        a.fail_writes_for_tests(false);
        let mut b = open(dir.path());
        for i in 1..CAP {
            b.transact(Operation::Add(comment(&format!("b{i}"), "shared", 3)), 102)
                .unwrap();
        }
        a.settle(103, "aaaaaa", Settlement::Failed).unwrap();
        assert_eq!(open(dir.path()).comments().len(), CAP);
        assert_eq!(a.comments().len(), CAP + 1);
        a.refresh(104);
        assert_eq!(a.journal, [Operation::Add(comment("local", "unsaved", 2))]);
        let before = std::fs::read(a.path().unwrap()).unwrap();
        assert!(a
            .claim(
                105,
                &Destination::clipboard(),
                &|_| "bbbbbb".into(),
                &mut 0,
                &|| Ok(()),
                |_, _| Ok(String::new())
            )
            .unwrap_err()
            .starts_with("comments not saved: "));
        assert_eq!(std::fs::read(a.path().unwrap()).unwrap(), before);
        let claimed = b
            .claim(
                106,
                &Destination::clipboard(),
                &|_| "bbbbbb".into(),
                &mut 0,
                &|| Ok(()),
                |_, _| Ok(String::new()),
            )
            .unwrap();
        assert_eq!(claimed.comments.len(), CAP);
        assert!(claimed.comments.iter().all(|c| c.id != "local"));
    }

    #[test]
    fn replay_of_an_add_already_on_disk_drops_it_without_a_write() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = open(dir.path());
        let added = comment("same", "landed", 1);
        a.fail_writes_for_tests(true);
        a.transact(Operation::Add(added.clone()), 100).unwrap_err();
        let mut b = open(dir.path());
        b.transact(Operation::Add(added), 101).unwrap();
        a.fail_writes_for_tests(false);
        let path = a.path().unwrap();
        let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        a.refresh(102);
        assert_eq!(
            std::fs::metadata(path).unwrap().modified().unwrap(),
            modified
        );
        assert_eq!(a.journal_len(), 0);
    }

    #[test]
    fn a_claim_guard_refusal_stamps_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        store
            .transact(Operation::Add(comment("pending", "kept", 1)), 100)
            .unwrap();
        let path = store.path().unwrap();
        let before = std::fs::read(&path).unwrap();
        let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        let result = store.claim(
            101,
            &Destination::clipboard(),
            &|_| "aaaaaa".into(),
            &mut 0,
            &|| Err("changed".into()),
            |_, _| Ok(String::new()),
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(
            std::fs::metadata(path).unwrap().modified().unwrap(),
            modified
        );
        assert!(store.comments().iter().all(Comment::is_pending));
        assert_eq!(result.unwrap_err(), "changed");
    }

    #[test]
    fn adds_edits_and_deletes_are_transactions_two_viewers_interleave() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = open(dir.path());
        let mut b = open(dir.path());
        a.transact(Operation::Add(comment("a1", "first", 1)), 1)
            .unwrap();
        b.transact(Operation::Add(comment("b1", "second", 2)), 2)
            .unwrap();
        a.transact(Operation::Add(comment("a2", "third", 3)), 3)
            .unwrap();
        let ids: Vec<_> = a.comments().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["a1", "b1", "a2"], "a's write kept b's add");
        a.transact(
            Operation::Edit {
                id: "b1".into(),
                category: Category::Question,
                text: "why".into(),
                seen: comment("b1", "second", 2),
            },
            4,
        )
        .unwrap();
        b.refresh(5);
        assert_eq!(b.comments()[1].text, "why");
        assert_eq!(b.comments()[1].category, Category::Question);
        b.transact(
            Operation::Delete {
                id: "a1".into(),
                seen: comment("a1", "first", 1),
            },
            6,
        )
        .unwrap();
        a.refresh(7);
        assert_eq!(a.comments().len(), 2);
        let mut sent = comment("a2", "third", 3);
        sent.state = CommentState::Sent(stamp("abc123"));
        a.transact(
            Operation::Edit {
                id: "a2".into(),
                category: Category::Bug,
                text: "x".into(),
                seen: sent.clone(),
            },
            8,
        )
        .unwrap_err();
        assert_eq!(a.comments()[1].text, "third");
        let mut unconfirmed = comment("u1", "maybe", 9);
        unconfirmed.state = CommentState::Unconfirmed {
            before: Vec::new(),
            stamp: stamp("abc123"),
        };
        a.transact(Operation::Add(unconfirmed.clone()), 9).unwrap();
        a.transact(
            Operation::Edit {
                id: "u1".into(),
                category: Category::Bug,
                text: "maybe not".into(),
                seen: unconfirmed.clone(),
            },
            10,
        )
        .unwrap();
        let edited = a.comments().iter().find(|c| c.id == "u1").unwrap().clone();
        assert_eq!(
            (edited.text.as_str(), edited.is_pending()),
            ("maybe not", true)
        );
        a.transact(
            Operation::Delete {
                id: "u1".into(),
                seen: edited,
            },
            11,
        )
        .unwrap();
        assert!(a.comments().iter().all(|c| c.id != "u1"));
    }

    #[test]
    fn the_cap_counts_unsent_records_as_the_file_holds_them() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = open(dir.path());
        let mut b = open(dir.path());
        for i in 0..49 {
            a.transact(Operation::Add(comment(&format!("c{i}"), "t", i)), 10)
                .unwrap();
        }
        a.transact(Operation::Add(comment("a50", "t", 50)), 10)
            .unwrap();
        assert_eq!(
            b.transact(Operation::Add(comment("b50", "t", 50)), 10)
                .unwrap_err(),
            NOTICE_CAP
        );
        for c in a.comments_mut_for_tests() {
            c.state = CommentState::Sending {
                stamp: stamp("aaaaaa"),
                before: Vec::new(),
            };
        }
        a.write_for_tests();
        b.refresh(11);
        assert_eq!(
            b.transact(Operation::Add(comment("b51", "t", 51)), 11)
                .unwrap_err(),
            NOTICE_CAP
        );
        for c in a.comments_mut_for_tests() {
            c.state = CommentState::Sent(stamp("aaaaaa"));
        }
        a.write_for_tests();
        b.refresh(12);
        b.transact(Operation::Add(comment("b51", "t", 51)), 12)
            .unwrap();
    }

    #[test]
    fn every_shape_rule_drops_one_record_and_the_next_write_rewrites_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let good = comment("good", "fine", 1);
        let mut bad = Vec::new();
        let mut c = comment("path", "x", 1);
        c.anchor.key.path = "../x".into();
        bad.push(c);
        let mut c = comment("empty", "x", 1);
        c.anchor.key.path = String::new();
        bad.push(c);
        let mut c = comment("range", "x", 1);
        c.anchor.span = Span::Range { end: 3 };
        c.anchor.line = 9;
        bad.push(c);
        let mut c = comment("file", "x", 1);
        c.anchor.span = Span::File;
        c.anchor.line = 4;
        bad.push(c);
        bad.push(comment("ctrl", "a\u{1b}b", 1));
        bad.push(comment("long", &"x".repeat(MAX_CHARS + 1), 1));
        let mut c = comment("mb", "x", 1);
        c.anchor.comparison = AnchorComparison::Branch {
            merge_base: "nothex".into(),
            label: "main".into(),
        };
        bad.push(c);
        let mut c = comment("nonce", "x", 1);
        c.state = CommentState::Sent(stamp("ab"));
        bad.push(c);
        let mut c = comment("to", "x", 1);
        c.state = CommentState::Sent(Stamp {
            to: Destination::Pane {
                pane: "nope".into(),
                agent: "codex".into(),
                session: None,
            },
            ..stamp("abcdef")
        });
        bad.push(c);
        let mut all = vec![good.clone()];
        all.extend(bad.clone());
        let mut file = serde_json::Map::new();
        file.insert("/repo".into(), serde_json::to_value(&all).unwrap());
        let mut fifth = serde_json::to_value(&good).unwrap();
        fifth["id"] = "fifth".into();
        fifth["category"] = "praise".into();
        let mut unknown = serde_json::to_value(&good).unwrap();
        unknown["id"] = "unknown".into();
        unknown["state"] = "queued".into();
        file["/repo"]
            .as_array_mut()
            .unwrap()
            .extend([fifth, unknown]);
        std::fs::write(
            dir.path().join("comments.json"),
            serde_json::to_vec(&file).unwrap(),
        )
        .unwrap();
        let (store, problems) = Store::open(Some(dir.path().to_path_buf()), "/repo", 1);
        assert_eq!(
            store
                .comments()
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            ["good"]
        );
        assert_eq!(
            problems,
            vec!["comments.json: 11 unusable record(s)".to_string()]
        );
        let mut store = store;
        store
            .transact(Operation::Add(comment("new", "n", 2)), 2)
            .unwrap();
        let text = std::fs::read_to_string(dir.path().join("comments.json")).unwrap();
        assert!(!text.contains("praise") && !text.contains("queued") && !text.contains("../x"));
    }

    #[test]
    fn an_unknown_state_drops_that_record_alone() {
        let dir = tempfile::tempdir().unwrap();
        let mut keep = serde_json::to_value(comment("keep", "k", 1)).unwrap();
        keep["future_field"] = serde_json::json!({ "x": 1 }); // extra fields are ignored
        let mut drop = serde_json::to_value(comment("drop", "d", 1)).unwrap();
        drop["state"] = "replied".into();
        std::fs::write(
            dir.path().join("comments.json"),
            serde_json::json!({ "/repo": [keep, drop] }).to_string(),
        )
        .unwrap();
        let (store, problems) = Store::open(Some(dir.path().to_path_buf()), "/repo", 1);
        assert_eq!(store.comments().len(), 1);
        assert_eq!(store.comments()[0].id, "keep");
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn the_caps_count_characters_not_bytes() {
        let wide = "字".repeat(MAX_CHARS); // 12,000 bytes, 4,000 chars
        assert_eq!(check_text(&wide).unwrap(), wide);
        assert_eq!(check_text(&format!("{wide}x")).unwrap_err(), NOTICE_LIMIT);
        let lines = "a\n".repeat(MAX_LINES).trim_end().to_string();
        assert!(check_text(&lines).is_ok());
        assert_eq!(
            check_text(&format!("{lines}\nb")).unwrap_err(),
            NOTICE_LIMIT
        );
        assert_eq!(check_text(&format!("{lines}\n")).unwrap_err(), NOTICE_LIMIT);
        assert_eq!(line_count(""), 1);
        assert_eq!(check_text("a\u{1b}[31mb\r\nc\x7f").unwrap(), "a[31mb\nc");
    }

    #[test]
    fn sending_expires_after_sixty_seconds_on_load_and_on_refresh_without_an_mtime_change() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        let mut young = comment("young", "y", 1);
        young.state = CommentState::Sending {
            stamp: Stamp {
                at: 1_010,
                ..stamp("aaaaaa")
            },
            before: Vec::new(),
        };
        let mut old = comment("old", "o", 1);
        old.state = CommentState::Sending {
            stamp: Stamp {
                at: 950,
                ..stamp("bbbbbb")
            },
            before: vec![stamp("cccccc")],
        };
        store.transact(Operation::Add(young), 1_000).unwrap();
        store.transact(Operation::Add(old), 1_000).unwrap();
        let (loaded, _) = Store::open(Some(dir.path().to_path_buf()), "/repo", 1_061);
        assert!(matches!(
            &loaded.comments()[0].state,
            CommentState::Sending { .. }
        ));
        assert!(
            matches!(&loaded.comments()[1].state, CommentState::Unconfirmed { stamp: s, .. } if s.nonce == "bbbbbb")
        );
        let mut store = loaded;
        let before = std::fs::metadata(dir.path().join("comments.json"))
            .unwrap()
            .modified()
            .unwrap();
        store.refresh(1_071);
        assert!(
            matches!(&store.comments()[0].state, CommentState::Unconfirmed { stamp: s, .. } if s.nonce == "aaaaaa")
        );
        assert!(
            std::fs::metadata(dir.path().join("comments.json"))
                .unwrap()
                .modified()
                .unwrap()
                >= before
        );
        let (other, _) = Store::open(Some(dir.path().to_path_buf()), "/repo", 1_071);
        assert!(other
            .comments()
            .iter()
            .all(|c| matches!(c.state, CommentState::Unconfirmed { .. })));
    }

    #[test]
    fn a_failed_expiry_write_on_load_still_shows_the_loaded_comments() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        store
            .transact(Operation::Add(comment("fine", "kept", 1)), 1_000)
            .unwrap();
        let mut stale = comment("stale", "x", 2);
        stale.state = CommentState::Sending {
            stamp: Stamp {
                at: 950,
                ..stamp("aaaaaa")
            },
            before: Vec::new(),
        };
        store.transact(Operation::Add(stale), 1_000).unwrap();
        assert!(matches!(
            store.comments()[1].state,
            CommentState::Sending { .. }
        ));
        let mut reopened = Store::open_for_tests_with_failing_writes(dir.path(), "/repo", 1_100);
        assert_eq!(
            reopened.comments().len(),
            2,
            "a failed expiry write hid the loaded comments"
        );
        assert!(
            matches!(reopened.comments()[1].state, CommentState::Sending { .. }),
            "shown as the file holds it"
        );
        reopened.fail_writes_for_tests(false);
        reopened.refresh(1_101);
        assert!(matches!(
            reopened.comments()[1].state,
            CommentState::Unconfirmed { .. }
        ));
    }

    #[test]
    fn a_moved_mtime_is_reread_and_a_still_one_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = open(dir.path());
        let mut b = open(dir.path());
        a.transact(Operation::Add(comment("a1", "x", 1)), 1)
            .unwrap();
        assert_eq!(b.refresh(2), (Vec::new(), None));
        assert_eq!(b.comments().len(), 1);
        let reads = b.reads_for_tests();
        b.refresh(3);
        assert_eq!(
            b.reads_for_tests(),
            reads,
            "an unchanged file was read again"
        );
    }

    #[test]
    fn without_a_state_directory_the_array_is_the_store() {
        let (mut store, problems) = Store::open(None, "/repo", 1);
        assert!(problems.is_empty());
        assert_eq!(
            store
                .transact(Operation::Add(comment("a", "x", 1)), 1)
                .unwrap_err(),
            "comments not remembered: no state directory"
        );
        assert_eq!(
            store.comments().len(),
            1,
            "the comment is held all the same"
        );
        store
            .transact(Operation::Add(comment("b", "x", 2)), 2)
            .unwrap();
        assert_eq!(store.comments().len(), 2);
        assert_eq!(
            store.journal_len(),
            0,
            "there is nothing to journal: nothing could ever be written"
        );
        let seen_then = comment("a", "x", 1);
        store
            .transact(
                Operation::Edit {
                    id: "a".into(),
                    category: Category::Bug,
                    text: "first edit".into(),
                    seen: seen_then.clone(),
                },
                3,
            )
            .unwrap();
        assert_eq!(
            store
                .transact(
                    Operation::Edit {
                        id: "a".into(),
                        category: Category::Bug,
                        text: "stale".into(),
                        seen: seen_then
                    },
                    4
                )
                .unwrap_err(),
            "No comment selected."
        );
        assert_eq!(store.comments()[0].text, "first edit");
    }

    #[test]
    fn a_failing_store_journals_and_replays_without_undoing_another_viewers_work() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = open(dir.path());
        let mut b = open(dir.path());
        a.transact(Operation::Add(comment("a1", "mine", 1)), 1)
            .unwrap();
        b.refresh(2);
        a.fail_writes_for_tests(true);
        let notice = a
            .transact(Operation::Add(comment("a2", "later", 3)), 3)
            .unwrap_err();
        assert!(notice.starts_with("comments not remembered: "));
        a.transact(
            Operation::Edit {
                id: "a1".into(),
                category: Category::Bug,
                text: "mine, edited".into(),
                seen: comment("a1", "mine", 1),
            },
            4,
        )
        .unwrap_err();
        assert_eq!(a.journal_len(), 2);
        assert_eq!(
            a.comments().len(),
            2,
            "the earlier unsaved add is still on screen after a second failed write"
        );
        assert!(a
            .comments()
            .iter()
            .any(|c| c.id == "a2" && c.text == "later"));
        b.transact(
            Operation::Edit {
                id: "a1".into(),
                category: Category::Question,
                text: "b's edit".into(),
                seen: comment("a1", "mine", 1),
            },
            5,
        )
        .unwrap();
        b.transact(Operation::Add(comment("b1", "b", 6)), 6)
            .unwrap();
        a.fail_writes_for_tests(false);
        let dropped = a
            .transact(Operation::Add(comment("a3", "third", 7)), 7)
            .unwrap();
        assert_eq!(
            dropped.as_deref(),
            Some("a comment changed under you; your edit was dropped")
        );
        assert_eq!(a.journal_len(), 0);
        let texts: Vec<_> = a
            .comments()
            .iter()
            .map(|c| (c.id.as_str(), c.text.as_str()))
            .collect();
        assert_eq!(
            texts,
            [
                ("a1", "b's edit"),
                ("b1", "b"),
                ("a2", "later"),
                ("a3", "third")
            ]
        );
        b.refresh(8);
        assert_eq!(b.comments().len(), 4);
    }

    #[test]
    fn a_refused_add_reports_a_dropped_journal_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = open(dir.path());
        let seen = comment("original", "original", 1);
        a.transact(Operation::Add(seen.clone()), 1).unwrap();
        let mut b = open(dir.path());
        a.fail_writes_for_tests(true);
        let notice = a
            .transact(
                Operation::Edit {
                    id: seen.id.clone(),
                    seen: seen.clone(),
                    category: Category::Bug,
                    text: "unsaved edit".into(),
                },
                2,
            )
            .unwrap_err();
        assert!(notice.starts_with(NOTICE_NOT_REMEMBERED));
        assert_eq!(a.journal_len(), 1);
        b.transact(
            Operation::Edit {
                id: seen.id.clone(),
                seen,
                category: Category::Question,
                text: "other viewer".into(),
            },
            3,
        )
        .unwrap();
        for i in 1..CAP {
            b.transact(Operation::Add(comment(&format!("b{i}"), "shared", 4)), 4)
                .unwrap();
        }
        a.fail_writes_for_tests(false);
        let notice = a
            .transact(Operation::Add(comment("refused", "new", 5)), 5)
            .unwrap_err();
        assert_eq!(
            notice,
            format!("{NOTICE_CAP} a comment changed under you; your edit was dropped")
        );
        assert_eq!(a.journal_len(), 0);
        assert_eq!(a.comments(), b.comments());
        assert_eq!(open(dir.path()).comments(), b.comments());
        assert_eq!(a.refresh(6), (Vec::new(), None));
        assert_eq!(
            a.transact(Operation::Add(comment("refused", "new", 7)), 7),
            Err(NOTICE_CAP.into())
        );
        assert_eq!(a.refresh(8), (Vec::new(), None));
    }

    #[test]
    fn a_journaled_add_the_cap_refuses_stays_visible_and_journaled() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = open(dir.path());
        let mut b = open(dir.path());
        a.fail_writes_for_tests(true);
        a.transact(Operation::Add(comment("mine", "unsaved", 1)), 1_001)
            .unwrap_err();
        for i in 0..50 {
            b.transact(Operation::Add(comment(&format!("b{i}"), "t", 2)), 1_002)
                .unwrap();
        }
        a.fail_writes_for_tests(false);
        let _ = a.refresh(1_003);
        assert_eq!(a.journal_len(), 1);
        assert!(a.comments().iter().any(|c| c.id == "mine"));
        assert_eq!(a.comments().len(), 51);
        a.transact(
            Operation::Delete {
                id: "mine".into(),
                seen: comment("mine", "unsaved", 1),
            },
            1_004,
        )
        .unwrap();
        assert_eq!(a.journal_len(), 0);
        assert!(a.comments().iter().all(|c| c.id != "mine"));
    }

    #[test]
    fn a_malformed_record_met_by_a_transaction_is_logged_before_it_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = open(dir.path());
        a.transact(Operation::Add(comment("a1", "x", 1)), 1)
            .unwrap();
        let mut file: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("comments.json")).unwrap(),
        )
        .unwrap();
        let mut odd = serde_json::to_value(comment("odd", "y", 2)).unwrap();
        odd["state"] = "resolved".into();
        file["/repo"].as_array_mut().unwrap().push(odd);
        std::fs::write(dir.path().join("comments.json"), file.to_string()).unwrap();
        a.transact(Operation::Add(comment("a2", "z", 3)), 3)
            .unwrap();
        let written = std::fs::read_to_string(dir.path().join("comments.json")).unwrap();
        assert!(!written.contains("resolved"));
        let log = std::fs::read_to_string(dir.path().join("config-problems.log")).unwrap();
        assert!(
            log.contains("comments.json: 1 unusable record(s); rewritten without them"),
            "{log}"
        );
        let bad = serde_json::json!({ "/repo": [{ "nonce": "x", "at": 1, "target": { "clipboard": true }, "files": [] }] });
        std::fs::write(dir.path().join("requests.json"), bad.to_string()).unwrap();
        record_request(
            dir.path(),
            "/repo",
            &RequestRecord {
                nonce: "abcdef".into(),
                at: 2,
                target: Destination::clipboard(),
                files: Vec::new(),
            },
        )
        .unwrap();
        assert_eq!(load_requests(dir.path(), "/repo").0.len(), 1);
        let log = std::fs::read_to_string(dir.path().join("config-problems.log")).unwrap();
        assert!(
            log.contains("requests.json: 1 unusable record(s); rewritten without them"),
            "{log}"
        );
    }

    #[test]
    fn a_failing_store_refuses_what_the_rules_refuse_instead_of_journaling_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = open(dir.path());
        let mut b = open(dir.path());
        for i in 0..50 {
            b.transact(Operation::Add(comment(&format!("b{i}"), "t", 1)), 1_001)
                .unwrap();
        }
        a.refresh(1_002);
        a.fail_writes_for_tests(true);
        assert_eq!(
            a.transact(Operation::Add(comment("mine", "x", 2)), 1_003)
                .unwrap_err(),
            NOTICE_CAP
        );
        assert_eq!(a.journal_len(), 0);
        assert_eq!(a.comments().len(), 50);
        let mut changed = comment("b0", "t", 1);
        changed.text = "not what a saw".into();
        assert_eq!(
            a.transact(
                Operation::Edit {
                    id: "b0".into(),
                    category: Category::Bug,
                    text: "mine".into(),
                    seen: changed
                },
                1_004
            )
            .unwrap_err(),
            "No comment selected."
        );
        assert_eq!(a.journal_len(), 0);
        let notice = a
            .transact(
                Operation::Edit {
                    id: "b0".into(),
                    category: Category::Bug,
                    text: "mine".into(),
                    seen: comment("b0", "t", 1),
                },
                1_005,
            )
            .unwrap_err();
        assert!(notice.starts_with(NOTICE_NOT_REMEMBERED));
        assert_eq!(a.journal_len(), 1);
    }

    #[test]
    fn a_journaled_add_stays_on_screen_through_a_failed_edit_on_a_full_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = open(dir.path());
        let mut b = open(dir.path());
        a.fail_writes_for_tests(true);
        a.transact(Operation::Add(comment("mine", "unsaved", 1)), 1_001)
            .unwrap_err();
        for i in 0..50 {
            b.transact(Operation::Add(comment(&format!("b{i}"), "t", 2)), 1_002)
                .unwrap();
        }
        let _ = a.refresh(1_003);
        assert!(a.comments().iter().any(|c| c.id == "mine"));
        let notice = a
            .transact(
                Operation::Edit {
                    id: "b0".into(),
                    category: Category::Bug,
                    text: "mine too".into(),
                    seen: comment("b0", "t", 2),
                },
                1_004,
            )
            .unwrap_err();
        assert!(notice.starts_with(NOTICE_NOT_REMEMBERED));
        assert_eq!(a.journal_len(), 2);
        assert!(
            a.comments().iter().any(|c| c.id == "mine"),
            "the journaled add vanished behind a failed edit"
        );
        assert_eq!(
            a.comments()
                .iter()
                .find(|c| c.id == "b0")
                .map(|c| c.text.as_str()),
            Some("mine too")
        );
        let _ = a.refresh(1_005);
        assert!(a.comments().iter().any(|c| c.id == "mine"));
        a.fail_writes_for_tests(false);
        let _ = a.refresh(1_006);
        assert_eq!(
            a.journal_len(),
            1,
            "the edit was written; the add still waits for room"
        );
        assert!(a.comments().iter().any(|c| c.id == "mine"));
        b.refresh(1_007);
        assert_eq!(
            b.comments()
                .iter()
                .find(|c| c.id == "b0")
                .map(|c| c.text.as_str()),
            Some("mine too")
        );
    }

    #[test]
    fn a_journaled_deletion_of_a_record_since_sent_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = open(dir.path());
        let mut b = open(dir.path());
        a.transact(Operation::Add(comment("a1", "x", 1)), 1)
            .unwrap();
        b.refresh(2);
        a.fail_writes_for_tests(true);
        a.transact(
            Operation::Delete {
                id: "a1".into(),
                seen: comment("a1", "x", 1),
            },
            3,
        )
        .unwrap_err();
        let mut sent = comment("a1", "x", 1);
        sent.state = CommentState::Sent(stamp("zzzzzz"));
        b.set_for_tests(vec![sent]);
        a.fail_writes_for_tests(false);
        let dropped = a
            .transact(Operation::Add(comment("a2", "y", 4)), 4)
            .unwrap();
        assert_eq!(
            dropped.as_deref(),
            Some("a comment changed under you; your deletion was dropped")
        );
        assert!(matches!(a.comments()[0].state, CommentState::Sent(_)));
    }

    #[test]
    fn ids_are_32_hex_and_sort_in_creation_order_within_a_second() {
        let ids: Vec<String> = (0..50).map(|_| new_id()).collect();
        for id in &ids {
            assert!(
                id.len() == 32
                    && id
                        .chars()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "{id}"
            );
            assert!(check_record(&Comment {
                id: id.clone(),
                ..comment("x", "t", 1)
            }));
        }
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(sorted, ids, "later ids sort later");
        assert_eq!(
            ids.iter().collect::<BTreeSet<_>>().len(),
            50,
            "all distinct"
        );
    }

    #[test]
    fn requests_are_recorded_and_nonces_in_use_are_known() {
        let dir = tempfile::tempdir().unwrap();
        let record = RequestRecord {
            nonce: "wvpx71".into(),
            at: 1,
            target: Destination::Pane {
                pane: "w4:p2".into(),
                agent: "codex".into(),
                session: None,
            },
            files: vec![RequestFile {
                key: FileKey {
                    path: "src/cart.py".into(),
                    staged: false,
                    untracked: false,
                },
                comparison: AnchorComparison::Worktree,
                additions: vec![(10, 12), (30, 31)],
                deletions: vec![],
            }],
        };
        record_request(dir.path(), "/repo", &record).unwrap();
        let (loaded, problem) = load_requests(dir.path(), "/repo");
        assert_eq!((loaded, problem), (vec![record.clone()], None));
        let mut sent = comment("c", "x", 1);
        sent.state = CommentState::Sent(stamp("oqzpww"));
        let used = nonces_in_use(dir.path(), &[sent.clone()]).unwrap();
        assert_eq!(
            used,
            ["oqzpww", "wvpx71"]
                .into_iter()
                .map(String::from)
                .collect::<BTreeSet<_>>()
        );
        let mut elsewhere = Store::open(Some(dir.path().to_path_buf()), "/other", 1_000).0;
        let mut theirs = comment("t", "x", 1);
        theirs.state = CommentState::Sent(stamp("zzzzz9"));
        elsewhere.transact(Operation::Add(theirs), 1_001).unwrap();
        assert!(nonces_in_use(dir.path(), &[]).unwrap().contains("zzzzz9"));
        let unreadable = tempfile::tempdir().unwrap();
        std::fs::create_dir(unreadable.path().join("comments.json")).unwrap(); // a directory where a file should be
        assert!(nonces_in_use(unreadable.path(), &[]).is_err());
        let mut store = open(dir.path());
        store.transact(Operation::Add(sent), 1_001).unwrap();
        let mut counter = 0;
        let make = |c: u64| {
            if c == 1 {
                "oqzpww".to_string()
            } else {
                format!("r{c:05}")
            }
        };
        let (nonce, text) = record_request_fresh(
            dir.path(),
            "/repo",
            &make,
            &mut counter,
            &|| Ok(()),
            |nonce| {
                Ok((
                    RequestRecord {
                        nonce: nonce.into(),
                        ..record.clone()
                    },
                    format!("text for {nonce}"),
                ))
            },
        )
        .unwrap();
        assert_eq!(
            (nonce.as_str(), text.as_str(), counter),
            ("r00002", "text for r00002", 2),
            "the nonce a comment carries was passed over"
        );
        assert!(record_request_fresh(
            dir.path(),
            "/repo",
            &make,
            &mut counter,
            &|| Err("changed".into()),
            |nonce| Ok((
                RequestRecord {
                    nonce: nonce.into(),
                    ..record.clone()
                },
                String::new()
            ))
        )
        .is_err());
        assert_eq!(
            load_requests(dir.path(), "/repo").0.len(),
            2,
            "a refused guard records nothing"
        );
        assert!(
            record_request_fresh(dir.path(), "/repo", &make, &mut counter, &|| Ok(()), |_| {
                Err("review too large".to_string())
            })
            .is_err()
        );
        assert_eq!(
            load_requests(dir.path(), "/repo").0.len(),
            2,
            "a refused build records nothing"
        );
        assert!(
            record_request_fresh(
                unreadable.path(),
                "/repo",
                &make,
                &mut counter,
                &|| Ok(()),
                |nonce| Ok((
                    RequestRecord {
                        nonce: nonce.into(),
                        ..record.clone()
                    },
                    String::new()
                ))
            )
            .is_err(),
            "no nonce is chosen blind"
        );
        let mut bad = serde_json::to_value(&record).unwrap();
        bad["nonce"] = "x".into();
        let file = serde_json::json!({ "/repo": [serde_json::to_value(&record).unwrap(), bad] });
        std::fs::write(dir.path().join("requests.json"), file.to_string()).unwrap();
        let (loaded, problem) = load_requests(dir.path(), "/repo");
        assert_eq!(loaded.len(), 1);
        assert_eq!(
            problem.as_deref(),
            Some("requests.json: 1 unusable record(s)")
        );
        let file = serde_json::json!({ "/repo": [serde_json::to_value(&record).unwrap()], "/other": "garbage", "/third": [{ "nonce": "abcdef", "at": 1, "target": { "clipboard": true }, "files": [] }] });
        std::fs::write(dir.path().join("requests.json"), file.to_string()).unwrap();
        assert_eq!(load_requests(dir.path(), "/repo").0.len(), 1);
        assert_eq!(
            load_requests(dir.path(), "/other").1.as_deref(),
            Some("requests.json: this worktree's entry is not a list")
        );
        record_request(
            dir.path(),
            "/repo",
            &RequestRecord {
                nonce: "zz9zz9".into(),
                ..record.clone()
            },
        )
        .unwrap();
        assert_eq!(load_requests(dir.path(), "/repo").0.len(), 2);
        let written: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("requests.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(written["/other"], "garbage");
        assert_eq!(
            written["/third"].as_array().map(Vec::len),
            Some(1),
            "the valid neighbour kept its record"
        );
        assert!(
            nonces_in_use(dir.path(), &[]).unwrap().contains("abcdef"),
            "its nonce still counts"
        );
        remove_request(dir.path(), "/repo", "zz9zz9").unwrap();
        let written: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("requests.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(written["/third"].as_array().map(Vec::len), Some(1));
        let file_of = |f: RequestFile| RequestRecord {
            files: vec![f],
            ..record.clone()
        };
        let ok_file = record.files[0].clone();
        let bad: Vec<RequestRecord> = vec![
            file_of(RequestFile {
                key: FileKey {
                    path: "../etc/passwd".into(),
                    staged: false,
                    untracked: false,
                },
                ..ok_file.clone()
            }),
            file_of(RequestFile {
                key: FileKey {
                    path: "/abs".into(),
                    staged: false,
                    untracked: false,
                },
                ..ok_file.clone()
            }),
            file_of(RequestFile {
                comparison: AnchorComparison::Branch {
                    merge_base: "main".into(),
                    label: "main".into(),
                },
                ..ok_file.clone()
            }),
            file_of(RequestFile {
                comparison: AnchorComparison::Branch {
                    merge_base: "0123456789abcdef0123456789abcdef01234567".into(),
                    label: "ma\u{1b}in".into(),
                },
                ..ok_file.clone()
            }),
            file_of(RequestFile {
                additions: vec![(0, 3)],
                ..ok_file.clone()
            }),
            file_of(RequestFile {
                deletions: vec![(5, 2)],
                ..ok_file.clone()
            }),
            RequestRecord {
                nonce: "no spaces!".into(),
                ..record.clone()
            },
        ];
        let good = file_of(RequestFile {
            comparison: AnchorComparison::Branch {
                merge_base: "0123456789abcdef0123456789abcdef01234567".into(),
                label: "origin/main".into(),
            },
            ..ok_file.clone()
        });
        let mut entries: Vec<serde_json::Value> = bad
            .iter()
            .map(|r| serde_json::to_value(r).unwrap())
            .collect();
        entries.push(serde_json::to_value(&good).unwrap());
        std::fs::write(
            dir.path().join("requests.json"),
            serde_json::json!({ "/repo": entries }).to_string(),
        )
        .unwrap();
        let (loaded, problem) = load_requests(dir.path(), "/repo");
        assert_eq!(loaded, vec![good]);
        assert_eq!(
            problem.as_deref(),
            Some("requests.json: 7 unusable record(s)")
        );
        std::fs::write(dir.path().join("requests.json"), "[1, 2]").unwrap();
        let (loaded, problem) = load_requests(dir.path(), "/repo");
        assert!(loaded.is_empty());
        assert!(problem
            .as_deref()
            .is_some_and(|p| p.starts_with("requests.json: not a JSON object")));
        record_request(dir.path(), "/repo", &record).unwrap();
        assert_eq!(
            load_requests(dir.path(), "/repo"),
            (vec![record.clone()], None)
        );
    }

    #[test]
    fn the_wire_shape_is_stable() {
        let mut c = comment("0123456789abcdef0123456789abcdef", "a\nb", 7);
        c.state = CommentState::Sending {
            stamp: Stamp {
                at: 9,
                nonce: "abc123".into(),
                item: 2,
                to: Destination::Pane {
                    pane: "w4:p2".into(),
                    agent: "codex".into(),
                    session: Some(crate::engine::host::SessionRef {
                        kind: "id".into(),
                        value: "s".into(),
                    }),
                },
            },
            before: vec![Stamp {
                at: 3,
                nonce: "zzz999".into(),
                item: 1,
                to: Destination::clipboard(),
            }],
        };
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(json["state"], "sending");
        assert_eq!(json["nonce"], "abc123");
        assert_eq!(json["to"]["pane"], "w4:p2");
        assert_eq!(json["before"][0]["to"]["clipboard"], true);
        let mut plain = comment("0123456789abcdef0123456789abcdef", "x", 1);
        plain.state = CommentState::Unconfirmed {
            stamp: stamp("abc123"),
            before: Vec::new(),
        };
        let value = serde_json::to_value(&plain).unwrap();
        assert!(
            value.get("before").is_none(),
            "an empty chain is not written"
        );
        let back: Comment = serde_json::from_value(value).unwrap();
        assert!(
            matches!(back.state, CommentState::Unconfirmed { before, .. } if before.is_empty()),
            "and reads back as empty"
        );
        assert_eq!(json["anchor"]["side"], "additions");
        assert_eq!(json["anchor"]["span"], "line");
        assert_eq!(json["category"], "bug");
        let back: Comment = serde_json::from_value(json).unwrap();
        assert_eq!(back, c);
    }
    #[test]
    fn retries_keep_every_uncertain_stamp_and_a_late_word_settles_the_right_one() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        let mut a = comment("c", "x", 1);
        a.state = CommentState::Unconfirmed {
            stamp: stamp("aaaaaa"),
            before: Vec::new(),
        };
        store.transact(Operation::Add(a), 1_000).unwrap();
        let to = Destination::clipboard();
        let ok = || Ok(());
        let mut counter = 0;
        let make_b = |_: u64| "bbbbbb".to_string();
        store
            .claim(
                1_001,
                &to,
                &make_b,
                &mut counter,
                &ok,
                |_, _| Ok("b".into()),
            )
            .unwrap();
        assert!(
            matches!(&store.comments()[0].state, CommentState::Sending { stamp, before } if stamp.nonce == "bbbbbb" && before.iter().map(|b| b.nonce.as_str()).collect::<Vec<_>>() == ["aaaaaa"])
        );
        store
            .settle(1_002, "bbbbbb", Settlement::Unconfirmed)
            .unwrap();
        let make_c = |_: u64| "cccccc".to_string();
        store
            .claim(
                1_003,
                &to,
                &make_c,
                &mut counter,
                &ok,
                |_, _| Ok("c".into()),
            )
            .unwrap();
        assert!(
            matches!(&store.comments()[0].state, CommentState::Sending { stamp, before } if stamp.nonce == "cccccc" && before.iter().map(|b| b.nonce.as_str()).collect::<Vec<_>>() == ["bbbbbb", "aaaaaa"]),
            "the whole chain is kept"
        );
        store.settle(1_004, "cccccc", Settlement::Failed).unwrap();
        assert!(
            matches!(&store.comments()[0].state, CommentState::Unconfirmed { stamp, before } if stamp.nonce == "bbbbbb" && before.len() == 1),
            "C's failure restores B, with A behind it"
        );
        store.settle(1_005, "bbbbbb", Settlement::Failed).unwrap();
        assert!(
            matches!(&store.comments()[0].state, CommentState::Unconfirmed { stamp, before } if stamp.nonce == "aaaaaa" && before.is_empty()),
            "B's late failure restores A, not Pending: A may have arrived"
        );
        store.settle(1_006, "aaaaaa", Settlement::Sent).unwrap();
        assert!(
            matches!(&store.comments()[0].state, CommentState::Sent(st) if st.nonce == "aaaaaa")
        );

        let mut d = comment("d", "y", 2);
        d.state = CommentState::Unconfirmed {
            stamp: stamp("eeeeee"),
            before: vec![stamp("dddddd")],
        };
        store.transact(Operation::Add(d), 1_007).unwrap();
        store.settle(1_008, "dddddd", Settlement::Sent).unwrap();
        assert!(
            matches!(&store.comments()[1].state, CommentState::Sent(st) if st.nonce == "dddddd")
        );
        // Late answers for an earlier attempt must survive a newer retry.
        let mut f = comment("f", "z", 3);
        f.state = CommentState::Unconfirmed {
            stamp: stamp("ffffff"),
            before: Vec::new(),
        };
        store.transact(Operation::Add(f), 1_009).unwrap();
        let make_g = |_: u64| "gggggg".to_string();
        store
            .claim(
                1_010,
                &to,
                &make_g,
                &mut counter,
                &ok,
                |_, _| Ok("g".into()),
            )
            .unwrap();
        store.settle(1_011, "ffffff", Settlement::Sent).unwrap();
        assert!(
            matches!(&store.comments()[2].state, CommentState::Sent(st) if st.nonce == "ffffff"),
            "F arrived while G was sending"
        );
        store.settle(1_012, "gggggg", Settlement::Failed).unwrap();
        assert!(
            matches!(&store.comments()[2].state, CommentState::Sent(st) if st.nonce == "ffffff"),
            "G's failure undoes nothing"
        );
        let mut h = comment("h", "w", 4);
        h.state = CommentState::Unconfirmed {
            stamp: stamp("hhhhhh"),
            before: Vec::new(),
        };
        store.transact(Operation::Add(h), 1_013).unwrap();
        let make_i = |_: u64| "iiiiii".to_string();
        store
            .claim(
                1_014,
                &to,
                &make_i,
                &mut counter,
                &ok,
                |_, _| Ok("i".into()),
            )
            .unwrap();
        store.settle(1_015, "hhhhhh", Settlement::Failed).unwrap();
        assert!(
            matches!(&store.comments()[3].state, CommentState::Sending { stamp, before } if stamp.nonce == "iiiiii" && before.is_empty()),
            "H's late failure left the chain"
        );
        store.settle(1_016, "iiiiii", Settlement::Failed).unwrap();
        assert!(store.comments()[3].is_pending(), "neither attempt arrived");

        let mut bare = Store::open(None, "/repo", 1_000).0;
        let mut j = comment("j", "v", 5);
        j.state = CommentState::Unconfirmed {
            stamp: stamp("kkkkkk"),
            before: vec![stamp("jjjjjj")],
        };
        // The first add of a session without a state directory says so once, and keeps the record.
        assert_eq!(
            bare.transact(Operation::Add(j), 1_017).unwrap_err(),
            format!("{NOTICE_NOT_REMEMBERED}no state directory")
        );
        assert_eq!(bare.comments().len(), 1);
        bare.transact(Operation::Add(comment("l", "u", 6)), 1_017)
            .unwrap();
        let colliding = |c: u64| ["jjjjjj", "kkkkkk", "llllll"][c as usize - 1].to_string();
        let mut counter = 0;
        let claimed = bare
            .claim(1_018, &to, &colliding, &mut counter, &ok, |_, _| {
                Ok("l".into())
            })
            .unwrap();
        assert_eq!(
            (claimed.nonce.as_str(), counter),
            ("llllll", 3),
            "a historical nonce is as taken as a current one"
        );
    }

    #[test]
    fn a_claim_takes_pending_and_unconfirmed_in_order_and_never_anothers_sending() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        store
            .transact(Operation::Add(comment("p1", "first", 1)), 1_010)
            .unwrap();
        let mut unconfirmed = comment("u1", "second", 2);
        unconfirmed.state = CommentState::Unconfirmed {
            before: Vec::new(),
            stamp: stamp("oldone"),
        };
        store.transact(Operation::Add(unconfirmed), 1_010).unwrap();
        let mut theirs = comment("s1", "third", 3);
        // Claimed by another viewer a second ago: still theirs, not expired.
        theirs.state = CommentState::Sending {
            stamp: Stamp {
                at: 1_010,
                ..stamp("theirs")
            },
            before: Vec::new(),
        };
        store.transact(Operation::Add(theirs), 1_010).unwrap();
        store
            .transact(Operation::Add(comment("p2", "fourth", 4)), 1_010)
            .unwrap();

        store
            .transact(Operation::Add(comment("p0", "zeroth", 0)), 1_010)
            .unwrap();
        // Monotonic ids order comments made within one second.
        let (first, second) = (new_id(), new_id());
        store
            .transact(
                Operation::Add(Comment {
                    id: second.clone(),
                    ..comment("x", "made second", 5)
                }),
                1_010,
            )
            .unwrap();
        store
            .transact(
                Operation::Add(Comment {
                    id: first.clone(),
                    ..comment("x", "made first", 5)
                }),
                1_010,
            )
            .unwrap();
        let to = Destination::Pane {
            pane: "w4:p2".into(),
            agent: "codex".into(),
            session: None,
        };
        let make = |counter: u64| format!("n{counter:05}");
        let ok = || Ok(());
        let mut counter = 0;
        let claimed = store
            .claim(1_011, &to, &make, &mut counter, &ok, |items, nonce| {
                let numbered: Vec<_> = items.iter().map(|(n, c)| format!("{n}:{}", c.id)).collect();
                Ok(format!("{nonce}|{}", numbered.join(",")))
            })
            .unwrap();

        assert_eq!(claimed.nonce, "n00001");

        assert_eq!(
            claimed.text,
            format!("n00001|1:p0,2:p1,3:u1,4:p2,5:{first},6:{second}")
        );
        assert_eq!(claimed.comments.len(), 6);
        for c in &claimed.comments {
            match &c.state {
                CommentState::Sending { stamp, before } => {
                    assert_eq!((stamp.nonce.as_str(), &stamp.to), ("n00001", &to));
                    let expected_item = ["p0", "p1", "u1", "p2", first.as_str(), second.as_str()]
                        .iter()
                        .position(|id| *id == c.id)
                        .unwrap() as u32
                        + 1;
                    assert_eq!(stamp.item, expected_item);
                    assert_eq!(
                        !before.is_empty(),
                        c.id == "u1",
                        "only the unconfirmed one remembers its earlier stamp"
                    );
                }
                other => panic!("{other:?}"),
            }
        }
        assert!(
            matches!(&store.comments()[2].state, CommentState::Sending { stamp, .. } if stamp.nonce == "theirs")
        );
        // Nothing eligible: another viewer took it all.
        let mut other = open(dir.path());
        assert_eq!(
            other
                .claim(1_012, &to, &make, &mut counter, &ok, |_, _| Ok(
                    String::new()
                ))
                .unwrap_err(),
            NOTICE_NOTHING
        );
        // Nonce collisions and guard or bound refusals must leave pending text untouched.
        other
            .transact(Operation::Add(comment("p3", "fifth", 5)), 1_012)
            .unwrap();
        let colliding = |counter: u64| {
            if counter == 1 {
                "n00001".to_string()
            } else {
                format!("m{counter:05}")
            }
        };
        let mut counter = 0;
        assert_eq!(
            other
                .claim(1_013, &to, &colliding, &mut counter, &ok, |_, _| Err(
                    "too large".to_string()
                ))
                .unwrap_err(),
            "too large"
        );
        assert_eq!(
            counter, 2,
            "n00001 was tried and passed over before build refused"
        );
        assert!(other.comments().iter().all(|c| !matches!(&c.state, CommentState::Sending { stamp, .. } if stamp.nonce == "m00002")), "a refusal inside build stamps nothing");
        let changed = || Err("the target changed; press Y again".to_string());
        assert_eq!(
            other
                .claim(1_014, &to, &make, &mut counter, &changed, |_, _| Ok(
                    String::new()
                ))
                .unwrap_err(),
            "the target changed; press Y again"
        );
        assert!(other
            .comments()
            .iter()
            .find(|c| c.id == "p3")
            .unwrap()
            .is_pending());
        // A write that fails leaves the array as the file has it: nothing shows Sending.
        other.fail_writes_for_tests(true);
        assert!(other
            .claim(1_015, &to, &make, &mut counter, &ok, |_, _| Ok(
                String::new()
            ))
            .is_err());
        assert!(
            other
                .comments()
                .iter()
                .find(|c| c.id == "p3")
                .unwrap()
                .is_pending(),
            "a failed write never shows a claim the file does not have"
        );
        other.fail_writes_for_tests(false);
        // Another worktree's array survives a claim and a settlement.
        let mut elsewhere = Store::open(Some(dir.path().to_path_buf()), "/other", 1_000).0;
        elsewhere
            .transact(Operation::Add(comment("e1", "theirs", 1)), 1_015)
            .unwrap();
        other
            .transact(Operation::Add(comment("p4", "sixth", 6)), 1_016)
            .unwrap();
        let claimed = other
            .claim(1_017, &to, &make, &mut counter, &ok, |_, nonce| {
                Ok(nonce.to_string())
            })
            .unwrap();
        other
            .settle(1_018, &claimed.nonce, Settlement::Sent)
            .unwrap();
        elsewhere.refresh(1_019);
        assert_eq!(
            elsewhere
                .comments()
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            ["e1"],
            "the other worktree's records were dropped by a write"
        );
        // Without a state directory the claim runs on the array alone.
        let mut bare = Store::open(None, "/repo", 1_000).0;
        bare.transact(Operation::Add(comment("x", "bare", 1)), 1_000)
            .unwrap_err();
        let claimed = bare
            .claim(
                1_001,
                &Destination::clipboard(),
                &make,
                &mut 0,
                &ok,
                |_, nonce| Ok(nonce.to_string()),
            )
            .unwrap();
        assert_eq!(claimed.comments.len(), 1);
        bare.settle(1_002, &claimed.nonce, Settlement::Unconfirmed)
            .unwrap();
        assert!(matches!(
            bare.comments()[0].state,
            CommentState::Unconfirmed { .. }
        ));
    }

    #[test]
    fn settlement_touches_only_this_nonce_and_restores_what_a_retry_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        store
            .transact(Operation::Add(comment("p1", "a", 1)), 10)
            .unwrap();
        let mut unconfirmed = comment("u1", "b", 2);
        unconfirmed.state = CommentState::Unconfirmed {
            before: Vec::new(),
            stamp: stamp("oldone"),
        };
        store.transact(Operation::Add(unconfirmed), 10).unwrap();
        let to = Destination::clipboard();
        let fixed = |name: &'static str| move |_: u64| name.to_string();
        let ok = || Ok(());
        store
            .claim(1_011, &to, &fixed("aaa111"), &mut 0, &ok, |_, _| {
                Ok(String::new())
            })
            .unwrap();
        // Another viewer's claim in between is never settled by this one.
        let mut other = open(dir.path());
        other
            .transact(Operation::Add(comment("p9", "c", 9)), 1_012)
            .unwrap();
        other
            .claim(1_012, &to, &fixed("bbb222"), &mut 0, &ok, |_, _| {
                Ok(String::new())
            })
            .unwrap();
        store.settle(1_013, "aaa111", Settlement::Failed).unwrap();
        let by_id = |store: &Store, id: &str| {
            store
                .comments()
                .iter()
                .find(|c| c.id == id)
                .unwrap()
                .state
                .clone()
        };
        assert_eq!(by_id(&store, "p1"), CommentState::Pending);
        assert!(
            matches!(by_id(&store, "u1"), CommentState::Unconfirmed { stamp: s, .. } if s.nonce == "oldone"),
            "the earlier stamp came back"
        );
        assert!(
            matches!(by_id(&store, "p9"), CommentState::Sending { stamp, .. } if stamp.nonce == "bbb222")
        );
        store
            .claim(1_014, &to, &fixed("ccc333"), &mut 0, &ok, |_, _| {
                Ok(String::new())
            })
            .unwrap();
        store
            .settle(1_015, "ccc333", Settlement::Unconfirmed)
            .unwrap();
        assert!(
            matches!(by_id(&store, "p1"), CommentState::Unconfirmed { stamp: s, .. } if s.nonce == "ccc333")
        );
        // A late answer settles records the timeout already marked unconfirmed (spec 10.6).
        store.settle(1_016, "ccc333", Settlement::Sent).unwrap();
        assert!(matches!(by_id(&store, "p1"), CommentState::Sent(s) if s.nonce == "ccc333"));
        store
            .transact(Operation::Add(comment("p2", "again", 16)), 1_016)
            .unwrap();
        store
            .claim(1_016, &to, &fixed("ddd444"), &mut 0, &ok, |_, _| {
                Ok(String::new())
            })
            .unwrap();
        store.settle(1_017, "ddd444", Settlement::Sent).unwrap();
        assert!(
            matches!(by_id(&store, "p2"), CommentState::Sent(s) if s.nonce == "ddd444" && s.item == 1)
        );
        assert!(
            matches!(by_id(&store, "u1"), CommentState::Sent(s) if s.nonce == "ccc333"),
            "a sent record is never claimed again"
        );
        // A late success must not mark replacement text as sent.
        let mut unsure = comment("e1", "old text", 17);
        unsure.state = CommentState::Unconfirmed {
            stamp: stamp("ggg777"),
            before: Vec::new(),
        };
        store
            .transact(Operation::Add(unsure.clone()), 1_017)
            .unwrap();
        store
            .transact(
                Operation::Edit {
                    id: "e1".into(),
                    category: Category::Bug,
                    text: "new text".into(),
                    seen: unsure,
                },
                1_017,
            )
            .unwrap();
        store.settle(1_017, "ggg777", Settlement::Sent).unwrap();
        assert!(
            matches!(by_id(&store, "e1"), CommentState::Pending),
            "the late success must not mark the new text sent"
        );
        // A failed settlement write leaves the array as the file has it.
        store
            .transact(Operation::Add(comment("p3", "later", 18)), 1_018)
            .unwrap();
        store
            .claim(1_018, &to, &fixed("fff666"), &mut 0, &ok, |_, _| {
                Ok(String::new())
            })
            .unwrap();
        store.fail_writes_for_tests(true);
        assert!(store.settle(1_019, "fff666", Settlement::Sent).is_err());
        assert!(
            matches!(by_id(&store, "p3"), CommentState::Sending { .. }),
            "the screen never shows a settlement the file does not have"
        );
        store.fail_writes_for_tests(false);
        // The cap never refuses a rollback: fifty pending plus fifty returning is allowed.
        let mut full = open(dir.path());
        for i in 0..50 {
            let mut c = comment(&format!("x{i}"), "t", 20);
            c.state = CommentState::Sending {
                stamp: Stamp {
                    at: 1_020,
                    ..stamp("eee555")
                },
                before: Vec::new(),
            };
            full.comments_mut_for_tests().push(c);
        }
        full.write_for_tests();
        full.settle(1_021, "eee555", Settlement::Failed).unwrap();
        assert_eq!(
            full.comments().iter().filter(|c| c.is_pending()).count(),
            50
        );
    }

    #[test]
    fn a_claim_is_refused_while_the_journal_is_not_empty() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        store
            .transact(Operation::Add(comment("p1", "a", 1)), 10)
            .unwrap();
        store.fail_writes_for_tests(true);
        store
            .transact(Operation::Add(comment("p2", "b", 2)), 11)
            .unwrap_err();
        store.fail_writes_for_tests(false);
        let error = store
            .claim(
                1_012,
                &Destination::clipboard(),
                &|c| format!("n{c:05}"),
                &mut 0,
                &|| Ok(()),
                |_, _| Ok(String::new()),
            )
            .unwrap_err();
        assert!(error.starts_with("comments not saved: "), "{error}");
        assert!(store.comments().iter().all(|c| c.is_pending()));
    }
}
