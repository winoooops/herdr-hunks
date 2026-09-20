//! Snapshot types the UI renders from. No terminal types here.
use std::sync::Arc;

use crate::engine::nav::{targets_for_diff, unified_order, Target};
use crate::git::{ChangedFile, FileDiff, GetGitDiffResponse};

pub const MAX_DIFF_LINES: usize = 200_000;

/// vimeflow's file identity: a partially staged path is two rows.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FileKey {
    pub path: String,
    pub staged: bool,
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
    pub file_diff: FileDiff,
    pub raw_diff: String,
    pub targets: Vec<Target>,
    pub unified_order: Vec<usize>,
    pub truncated_lines: usize,
}

impl LoadedDiff {
    pub fn build(key: FileKey, response: GetGitDiffResponse) -> Self {
        Self::build_with_cap(key, response, MAX_DIFF_LINES)
    }

    pub fn build_with_cap(key: FileKey, response: GetGitDiffResponse, cap: usize) -> Self {
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
            file_diff,
            raw_diff: response.raw_diff,
            targets,
            unified_order,
            truncated_lines: total - kept,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub revision: u64,
    pub repo: RepoState,
    pub files: Vec<ChangedFile>,
    pub selected: Option<FileKey>,
    pub diff: DiffState,
    pub status_error: Option<String>,
    pub watcher_error: Option<String>,
    pub refreshing: bool,
}

impl Snapshot {
    pub fn empty(cwd: &str) -> Self {
        Self {
            revision: 0,
            repo: RepoState::NotARepo {
                cwd: cwd.to_string(),
            },
            files: Vec::new(),
            selected: None,
            diff: DiffState::Idle,
            status_error: None,
            watcher_error: None,
            refreshing: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Select(FileKey),
    SelectNext,
    SelectPrev,
    Refresh,
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
        }
    }

    #[test]
    fn a_small_diff_is_kept_whole() {
        let loaded = LoadedDiff::build_with_cap(key(), response(vec![hunk(1, 3), hunk(50, 2)]), 10);
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
            response(vec![hunk(1, 6), hunk(50, 6), hunk(90, 1)]),
            10,
        );
        assert_eq!(loaded.file_diff.hunks.len(), 1);
        assert_eq!(loaded.truncated_lines, 7);
        assert_eq!(loaded.targets.len(), 6);
    }

    #[test]
    fn an_oversized_first_hunk_keeps_its_prefix() {
        let loaded = LoadedDiff::build_with_cap(key(), response(vec![hunk(1, 25)]), 10);
        assert_eq!(loaded.file_diff.hunks.len(), 1);
        assert_eq!(loaded.file_diff.hunks[0].lines.len(), 10);
        assert_eq!(loaded.truncated_lines, 15);
        assert_eq!(loaded.targets.last().map(|t| t.line_number), Some(10));
    }
}
