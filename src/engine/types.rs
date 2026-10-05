//! Snapshot types the UI renders from. No terminal types here.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::engine::comments::{Anchor, Category, Comment};
use crate::engine::nav::{targets_for_diff, unified_order, Target};
use crate::engine::target::{PaneRow, TargetState};
use crate::git::{ChangedFile, ChangedFileStatus, FileDiff, GetGitDiffResponse};

pub const MAX_DIFF_LINES: usize = 200_000;

/// Shown by `b` when nothing resolves as a base.
pub const NO_BASE_NOTICE: &str = "no base branch: set [base] ref or press B";

/// vimeflow's file identity: a partially staged path is two rows. In branch
/// scope a path deleted on the branch and recreated untracked is two rows too.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct FileKey {
    pub path: String,
    pub staged: bool,
    pub untracked: bool,
}

impl FileKey {
    pub fn of(file: &ChangedFile) -> Self {
        Self {
            path: file.path.clone(),
            staged: file.staged,
            untracked: matches!(file.status, ChangedFileStatus::Untracked),
        }
    }
}

/// What the working tree holds at the row's path, from lstat (spec 9.2 "Stale content").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeKind {
    Absent,
    /// A regular file: a hash of its bytes.
    File(u64),
    /// A symlink: a hash of its target's bytes.
    Symlink(u64),
    Directory,
    Other,
}

/// The pre-image a diff was read against; `None` on `LoadedDiff` when its two readings disagreed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreImage {
    /// `<mode> <object id> <stage>` of `ls-files -s`; `None` when the index has no entry.
    pub index: Option<String>,
    /// The same for the row's rename source, whose entry a forward form applies to; `None` when
    /// the row has no source or the source has no entry.
    pub source: Option<String>,
    pub worktree: WorktreeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Worktree,
    Branch,
}

impl Scope {
    pub fn other(self) -> Self {
        match self {
            Self::Worktree => Self::Branch,
            Self::Branch => Self::Worktree,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseSource {
    Picked,
    Config,
    Default,
}

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
    /// The `head_seen` this state was classified against; `None` while unclassified.
    pub classified_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickBase {
    /// `upstream`, `last commit`, or `last 3 commits`.
    pub label: String,
    /// A ref name or short object id for display.
    pub detail: String,
    pub submits: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Base {
    /// Exactly what git is given: a qualified ref for a picked or resolved
    /// branch, the typed text for free text and config.
    pub requested: String,
    /// Object id of `requested` as of the last refresh that verified it.
    pub commit: String,
    /// merge-base(HEAD, commit) the current rows were computed against; `None` in worktree scope.
    pub merge_base: Option<String>,
    pub source: BaseSource,
}

impl Base {
    pub fn label(&self) -> &str {
        ref_label(&self.requested)
    }
}

/// `refs/heads/x` -> `x`, `refs/remotes/o/x` -> `o/x`, `refs/tags/v` -> `v`; anything else unchanged.
pub fn ref_label(requested: &str) -> &str {
    ["refs/heads/", "refs/remotes/", "refs/tags/"]
        .iter()
        .find_map(|prefix| requested.strip_prefix(prefix))
        .unwrap_or(requested)
}

/// What a diff's hunks were computed against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Comparison {
    Worktree,
    Branch { merge_base: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoState {
    Repo {
        toplevel: String,
        branch: Option<String>,
        worktree: Option<String>,
    },
    NotARepo {
        cwd: String,
    },
    Unusable {
        reason: String,
    },
}

#[derive(Debug, Clone)]
pub enum DiffState {
    Idle,
    Loading,
    Ready(Arc<LoadedDiff>),
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct LoadedDiff {
    pub key: FileKey,
    pub comparison: Comparison,
    pub read_at: Option<String>,
    pub file_diff: FileDiff,
    pub raw_diff: String,
    /// The kept sections, byte for byte: what an action applies back.
    pub patch: Vec<u8>,
    /// `None` when the two readings around the diff disagreed.
    pub pre_image: Option<PreImage>,
    pub targets: Vec<Target>,
    pub unified_order: Vec<usize>,
    pub truncated_lines: usize,
}

impl LoadedDiff {
    pub fn build(
        key: FileKey,
        comparison: Comparison,
        read_at: Option<String>,
        response: GetGitDiffResponse,
        patch: Vec<u8>,
        pre_image: Option<PreImage>,
    ) -> Self {
        Self::build_with_cap(
            key,
            comparison,
            read_at,
            response,
            patch,
            pre_image,
            MAX_DIFF_LINES,
        )
    }

    pub fn build_with_cap(
        key: FileKey,
        comparison: Comparison,
        read_at: Option<String>,
        response: GetGitDiffResponse,
        patch: Vec<u8>,
        pre_image: Option<PreImage>,
        cap: usize,
    ) -> Self {
        let total: usize = response.file_diff.hunks.iter().map(|h| h.lines.len()).sum();
        let mut file_diff = response.file_diff;
        let mut kept = 0usize;
        let mut hunks = Vec::new();
        for (index, mut hunk) in file_diff.hunks.drain(..).enumerate() {
            if kept + hunk.lines.len() <= cap {
                kept += hunk.lines.len();
                hunks.push(hunk);
            } else {
                if index == 0 {
                    hunk.lines.truncate(cap);
                    kept = hunk.lines.len();
                    hunks.push(hunk);
                }
                break;
            }
        }
        file_diff.hunks = hunks;
        let targets = targets_for_diff(&file_diff);
        let unified_order = unified_order(&targets);
        Self {
            key,
            comparison,
            read_at,
            file_diff,
            raw_diff: response.raw_diff,
            patch,
            pre_image,
            targets,
            unified_order,
            truncated_lines: total - kept,
        }
    }
}

/// What the key means; the row gives the direction (spec 9.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    Stage,
    Discard,
    DiscardFile,
}

/// A confirmed action against the very diff the box was opened on.
#[derive(Debug, Clone)]
pub struct Action {
    pub kind: ActionKind,
    /// The diff the box was opened on.
    pub diff: Arc<LoadedDiff>,
    /// The hunk under the cursor; `None` for `DiscardFile` and for every untracked row.
    pub hunk: Option<usize>,
}

impl PartialEq for Action {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.hunk == other.hunk && Arc::ptr_eq(&self.diff, &other.diff)
    }
}

impl Eq for Action {}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub revision: u64,
    pub repo: RepoState,
    /// The diff's commit, or the refresh's confirmed commit for an empty list.
    pub head: Option<String>,
    /// The id the last refresh observed, published as observed and never gated.
    pub head_seen: Option<String>,
    /// `scope`, `base`, `files` and `rename_sources` always describe one comparison.
    pub scope: Scope,
    pub base: Option<Base>,
    pub mark: Option<Mark>,
    /// A skipped resolution step, a `bases.json` problem or a pick that was not remembered; shown once.
    pub base_error: Option<String>,
    /// What steps 2-5 of the resolution order name, for the picker's reset row.
    pub default_base: Option<String>,
    pub files: Vec<ChangedFile>,
    /// A renamed row's path -> its old path, in both scopes; the diff task asks by side.
    pub rename_sources: Arc<BTreeMap<String, String>>,
    /// Branch scope paths changed since the mark; non-Current states flag every row instead.
    pub unread: Arc<BTreeSet<String>>,
    pub selected: Option<FileKey>,
    pub diff: DiffState,
    pub status_error: Option<String>,
    pub watcher_error: Option<String>,
    pub refreshing: bool,
    /// The picker's candidates, qualified, most recently created first; `None` until `LoadRefs`.
    pub refs: Option<Arc<Vec<String>>>,
    pub quick: Option<Arc<Vec<QuickBase>>>,
    pub refs_overflow: bool,
    /// The token of the opening whose `LoadRefs` was answered last.
    pub refs_seq: u64,
    /// Bumped once per answered `SetBase`; `pick_error` is that answer.
    pub pick_seq: u64,
    pub pick_error: Option<String>,
    /// Bumped once per answered `MarkReviewed`; `mark_error` is that answer.
    pub mark_seq: u64,
    pub mark_error: Option<String>,
    /// Bumped once per answered `Act`; `action_error` and `action_applied` are that answer.
    pub action_seq: u64,
    pub action_error: Option<String>,
    /// Whether any `apply` form ran for the answered `Act`.
    pub action_applied: bool,
    /// The pane or clipboard a review goes to; `None` until one is chosen (spec 10.2).
    pub target: Option<crate::engine::target::Target>,
    /// The last check's answer; `NoHost` with no target and no host, `Unverified` with no target and a host.
    pub target_state: TargetState,
    /// Bumped once per answered target write; `target_error` is that answer.
    pub target_seq: u64,
    pub target_error: Option<String>,
    /// The pick token this write answers; `None` for a comment or check's write.
    pub target_token: Option<u64>,
    /// The pane picker's rows under the opening's token, as `refs` are for the base picker.
    pub panes: Option<Arc<Vec<PaneRow>>>,
    pub panes_seq: u64,
    pub panes_error: Option<String>,
    /// This worktree's comments in every state (spec 10.3).
    pub comments: Arc<Vec<Comment>>,
    /// Advanced once per comment answer or refresh notice.
    pub comment_seq: u64,
    pub comment_error: Option<String>,
    /// False for accepted operations, including journaled ones.
    pub comment_refused: bool,
    /// The add or edit token; absent for deletions and refresh notices.
    pub comment_token: Option<u64>,
}

impl Snapshot {
    pub fn empty(cwd: &str) -> Self {
        Self {
            revision: 0,
            repo: RepoState::NotARepo {
                cwd: cwd.to_string(),
            },
            head: None,
            head_seen: None,
            scope: Scope::Worktree,
            base: None,
            mark: None,
            base_error: None,
            default_base: None,
            files: Vec::new(),
            rename_sources: Arc::new(BTreeMap::new()),
            unread: Arc::new(BTreeSet::new()),
            selected: None,
            diff: DiffState::Idle,
            status_error: None,
            watcher_error: None,
            refreshing: false,
            refs: None,
            quick: None,
            refs_overflow: false,
            refs_seq: 0,
            pick_seq: 0,
            pick_error: None,
            mark_seq: 0,
            mark_error: None,
            action_seq: 0,
            action_error: None,
            action_applied: false,
            target: None,
            target_state: TargetState::Unverified,
            target_seq: 0,
            target_error: None,
            target_token: None,
            panes: None,
            panes_seq: 0,
            panes_error: None,
            comments: Arc::new(Vec::new()),
            comment_seq: 0,
            comment_error: None,
            comment_refused: false,
            comment_token: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Select(FileKey),
    SelectNext,
    SelectPrev,
    Refresh,
    /// Load the rows of the other scope; published together with them.
    SetScope(Scope),
    /// `Some`: validate, load branch rows under it, persist, publish. `None`: forget the pick and re-resolve.
    SetBase(Option<String>),
    /// Record this commit as reviewed for this worktree. It changes no comparison.
    MarkReviewed(String),
    /// Answer with `refs` and this opening's token on the snapshot.
    LoadRefs(u64),
    /// Run the apply forms of spec 9.2 for this confirmed action; answered on `action_seq`.
    Act(Action),
    /// Answer with the agent panes and this opening's token on the snapshot (spec 10.2).
    LoadPanes(u64),
    /// Remember the target and echo this pick's token; the next refresh verifies it.
    SetTarget {
        token: u64,
        target: crate::engine::target::Target,
    },
    /// Add a comment and echo the editor's save token.
    AddComment {
        token: u64,
        anchor: Anchor,
        category: Category,
        text: String,
    },
    /// Change only the record the editor saw.
    EditComment {
        token: u64,
        seen: Comment,
        category: Category,
        text: String,
    },
    DeleteComment {
        seen: Comment,
    },
    Shutdown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{DiffHunk, DiffLine, DiffLineType, FileDiff, GetGitDiffResponse};

    fn hunk(start: u32, lines: usize) -> DiffHunk {
        DiffHunk {
            id: format!("hunk-{start}-{start}"),
            header: String::new(),
            old_start: start,
            old_lines: 0,
            new_start: start,
            new_lines: lines as u32,
            lines: (0..lines)
                .map(|_| DiffLine {
                    line_type: DiffLineType::Added,
                    content: "x".into(),
                    old_line_number: None,
                    new_line_number: None,
                })
                .collect(),
        }
    }

    fn response(hunks: Vec<DiffHunk>) -> GetGitDiffResponse {
        GetGitDiffResponse {
            file_diff: FileDiff {
                file_path: "f".into(),
                old_path: None,
                new_path: None,
                hunks,
            },
            old_text: String::new(),
            new_text: String::new(),
            raw_diff: "raw".into(),
            repo_root: "/r".into(),
        }
    }

    fn key() -> FileKey {
        FileKey {
            path: "f".into(),
            staged: false,
            untracked: false,
        }
    }

    #[test]
    fn a_small_diff_is_kept_whole() {
        let loaded = LoadedDiff::build_with_cap(
            key(),
            Comparison::Worktree,
            None,
            response(vec![hunk(1, 3), hunk(50, 2)]),
            Vec::new(),
            None,
            10,
        );
        assert_eq!(loaded.file_diff.hunks.len(), 2);
        assert_eq!(loaded.truncated_lines, 0);
        assert_eq!(loaded.targets.len(), 5);
        assert_eq!(loaded.unified_order.len(), 5);
        assert_eq!(loaded.raw_diff, "raw");
    }

    #[test]
    fn whole_hunks_are_kept_while_they_fit() {
        let loaded = LoadedDiff::build_with_cap(
            key(),
            Comparison::Worktree,
            None,
            response(vec![hunk(1, 6), hunk(50, 6), hunk(90, 1)]),
            Vec::new(),
            None,
            10,
        );
        assert_eq!(loaded.file_diff.hunks.len(), 1);
        assert_eq!(loaded.truncated_lines, 7);
        assert_eq!(loaded.targets.len(), 6);
    }

    #[test]
    fn an_oversized_first_hunk_keeps_its_prefix() {
        let loaded = LoadedDiff::build_with_cap(
            key(),
            Comparison::Worktree,
            None,
            response(vec![hunk(1, 25)]),
            Vec::new(),
            None,
            10,
        );
        assert_eq!(loaded.file_diff.hunks.len(), 1);
        assert_eq!(loaded.file_diff.hunks[0].lines.len(), 10);
        assert_eq!(loaded.truncated_lines, 15);
        assert_eq!(loaded.targets.last().map(|t| t.line_number), Some(10));
    }

    #[test]
    fn a_recreated_untracked_path_is_a_different_key_from_its_deleted_row() {
        let deleted = ChangedFile {
            path: "f".into(),
            status: ChangedFileStatus::Deleted,
            staged: false,
            insertions: None,
            deletions: None,
        };
        let untracked = ChangedFile {
            status: ChangedFileStatus::Untracked,
            ..deleted.clone()
        };
        assert_ne!(FileKey::of(&deleted), FileKey::of(&untracked));
        assert!(!FileKey::of(&deleted).untracked);
        assert!(FileKey::of(&untracked).untracked);
    }

    #[test]
    fn labels_strip_the_ref_namespace_and_leave_free_text_alone() {
        assert_eq!(ref_label("refs/heads/main"), "main");
        assert_eq!(ref_label("refs/remotes/origin/main"), "origin/main");
        assert_eq!(ref_label("refs/tags/v1"), "v1");
        assert_eq!(ref_label("HEAD~3"), "HEAD~3");
        let base = Base {
            requested: "refs/heads/feat/x".into(),
            commit: "0".repeat(40),
            merge_base: None,
            source: BaseSource::Default,
        };
        assert_eq!(base.label(), "feat/x");
        assert_eq!(Scope::Worktree.other(), Scope::Branch);
        assert_eq!(Scope::Branch.other(), Scope::Worktree);
    }

    #[test]
    fn a_loaded_diff_remembers_its_comparison() {
        let branch = Comparison::Branch {
            merge_base: "1".repeat(40),
        };
        let loaded = LoadedDiff::build(
            key(),
            branch.clone(),
            None,
            response(vec![hunk(1, 1)]),
            Vec::new(),
            None,
        );
        assert_eq!(loaded.comparison, branch);
        assert_ne!(loaded.comparison, Comparison::Worktree);
        let empty = Snapshot::empty("/r");
        assert_eq!(empty.scope, Scope::Worktree);
        assert!(empty.base.is_none() && empty.refs.is_none() && !empty.refs_overflow);
        assert_eq!((empty.pick_seq, empty.refs_seq), (0, 0));
        assert!(empty.rename_sources.is_empty());
    }
}
