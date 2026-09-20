//! Everything the TUI owns that is not in the snapshot, and the 4.8 reconciliation rules.
use crate::engine::nav::{self, Side, ViewMode};
use crate::engine::{DiffState, LoadedDiff, Snapshot};
use crate::tui::layout::{clamp_scroll, ensure_visible, reanchor, LineSpan};
use crate::tui::rows::{self, Rows};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesPanel {
    Hidden,
    Shown,
    Pinned,
}

pub struct ViewState {
    pub mode: ViewMode,
    pub cursor: Option<usize>,
    pub cursor_id: Option<(Side, u32)>,
    pub offset: usize,
    pub hscroll: usize,
    pub files_panel: FilesPanel,
    pub mouse_requested: bool,
    pub help_open: bool,
    pub notice: Option<String>,
    pub rows: Option<Rows>,
    pub body_height: u16,
    pub help_offset: usize,
    /// The diff the rows were built from. Holding the Arc makes `Arc::ptr_eq` a safe,
    /// allocation-free "did anything change" test; the engine swaps the Arc only on change.
    built_from: Option<(std::sync::Arc<LoadedDiff>, ViewMode)>,
}

impl ViewState {
    pub fn new(mode: ViewMode, files_panel: FilesPanel, mouse: bool) -> Self {
        Self {
            mode,
            cursor: None,
            cursor_id: None,
            offset: 0,
            hscroll: 0,
            files_panel,
            mouse_requested: mouse,
            help_open: false,
            notice: None,
            rows: None,
            body_height: 0,
            help_offset: 0,
            built_from: None,
        }
    }

    /// Acts only when the height changed; the run loop calls it before every frame.
    pub fn resize(&mut self, body_height: u16) {
        if self.body_height == body_height {
            return;
        }
        self.body_height = body_height;
        if let Some(rows) = &self.rows {
            self.offset = clamp_scroll(self.offset, rows.rows.len(), body_height);
            self.keep_cursor_visible();
        }
    }

    pub fn set_cursor(&mut self, diff: &LoadedDiff, target: usize) {
        if let Some(t) = diff.targets.get(target) {
            self.cursor = Some(target);
            self.cursor_id = Some((t.side, t.line_number));
            self.keep_cursor_visible();
        }
    }

    fn keep_cursor_visible(&mut self) {
        if let (Some(rows), Some(cursor)) = (&self.rows, self.cursor) {
            if let Some(&row) = rows.row_of_target.get(cursor) {
                self.offset = ensure_visible(
                    self.offset,
                    LineSpan {
                        start: row,
                        height: 1,
                    },
                    self.body_height,
                    rows.rows.len(),
                );
            }
        }
    }

    pub fn reconcile(&mut self, snapshot: &Snapshot) {
        let DiffState::Ready(diff) = &snapshot.diff else {
            self.rows = None;
            self.cursor = None;
            self.cursor_id = None;
            self.offset = 0;
            self.hscroll = 0;
            self.built_from = None;
            return;
        };
        // Runs before every frame, so the unchanged case must cost nothing: no clone, no compare of text.
        if let Some((built, mode)) = &self.built_from {
            if std::sync::Arc::ptr_eq(built, diff) && *mode == self.mode {
                return;
            }
        }
        let same_file = self
            .built_from
            .as_ref()
            .map(|(built, _)| built.key == diff.key)
            .unwrap_or(false);
        let old_row = match (&self.rows, self.cursor) {
            (Some(rows), Some(c)) => rows.row_of_target.get(c).copied(),
            _ => None,
        };
        let rows = rows::build(diff, self.mode);

        if !same_file {
            self.offset = 0;
            self.hscroll = 0;
            self.cursor = nav::target_index_for_hunk(&diff.targets, 0);
        } else {
            let previous = self.cursor;
            self.cursor = self.cursor_id.and_then(|(side, line)| {
                nav::find(&diff.targets, side, line)
                    .or_else(|| nav::nearest(&diff.targets, side, line))
                    .filter(|&c| diff.targets[c].side == side)
            });
            if self.cursor.is_none() {
                self.cursor = previous
                    .filter(|_| !diff.targets.is_empty())
                    .map(|c| c.min(diff.targets.len() - 1))
                    .or_else(|| nav::target_index_for_hunk(&diff.targets, 0));
            }
        }
        self.cursor_id = self
            .cursor
            .and_then(|c| diff.targets.get(c))
            .map(|t| (t.side, t.line_number));
        if self.cursor.is_none() {
            self.offset = 0;
            self.hscroll = 0;
        } else if let (true, Some(old), Some(new)) = (
            same_file,
            old_row,
            self.cursor.and_then(|c| rows.row_of_target.get(c).copied()),
        ) {
            self.offset = reanchor(self.offset, old, new, self.body_height, rows.rows.len());
        }
        self.rows = Some(rows);
        self.keep_cursor_visible();
        self.built_from = Some((diff.clone(), self.mode));
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::engine::{DiffState, FileKey, LoadedDiff, RepoState, Snapshot};
    use crate::git::{DiffHunk, DiffLine, DiffLineType, FileDiff, GetGitDiffResponse};
    use std::sync::Arc;

    pub(crate) fn snapshot(path: &str, raw: &str, hunks: &[(u32, &str)]) -> Snapshot {
        let hunks = hunks
            .iter()
            .map(|(start, kinds)| DiffHunk {
                id: String::new(),
                header: format!("@@ -{start} +{start} @@"),
                old_start: *start,
                old_lines: kinds.chars().filter(|c| *c != '+').count() as u32,
                new_start: *start,
                new_lines: kinds.chars().filter(|c| *c != '-').count() as u32,
                lines: kinds
                    .chars()
                    .map(|k| DiffLine {
                        line_type: match k {
                            '+' => DiffLineType::Added,
                            '-' => DiffLineType::Removed,
                            _ => DiffLineType::Context,
                        },
                        content: format!("line {k}"),
                        old_line_number: None,
                        new_line_number: None,
                    })
                    .collect(),
            })
            .collect();
        let key = FileKey {
            path: path.into(),
            staged: false,
        };
        let loaded = LoadedDiff::build(
            key.clone(),
            GetGitDiffResponse {
                file_diff: FileDiff {
                    file_path: path.into(),
                    old_path: None,
                    new_path: None,
                    hunks,
                },
                old_text: String::new(),
                new_text: String::new(),
                raw_diff: raw.into(),
                repo_root: "/r".into(),
            },
        );
        let mut s = Snapshot::empty("/r");
        s.revision = 1;
        s.repo = RepoState::Repo {
            toplevel: "/r".into(),
            branch: Some("main".into()),
            worktree: None,
        };
        s.selected = Some(key);
        s.diff = DiffState::Ready(Arc::new(loaded));
        s
    }

    fn state() -> ViewState {
        let mut s = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        s.resize(10);
        s
    }

    #[test]
    fn a_new_file_puts_the_cursor_on_the_first_changed_row_and_resets_offsets() {
        let mut st = state();
        st.offset = 7;
        st.hscroll = 16;
        st.reconcile(&snapshot("a.rs", "r1", &[(10, "  +  ")]));
        assert_eq!(st.cursor, Some(2));
        assert_eq!((st.offset, st.hscroll), (0, 0));
    }

    #[test]
    fn a_refresh_keeps_the_cursor_on_its_line_or_moves_to_the_nearest() {
        let mut st = state();
        st.reconcile(&snapshot("a.rs", "r1", &[(10, " + + ")]));
        let snap = snapshot("a.rs", "r1", &[(10, " + + ")]);
        if let DiffState::Ready(d) = &snap.diff {
            st.set_cursor(d, 3);
        } // additions line 13
        st.reconcile(&snapshot("a.rs", "r2", &[(10, "  + + ")])); // line 13 still exists
        assert_eq!(st.cursor_id, Some((Side::Additions, 13)));
        st.reconcile(&snapshot("a.rs", "r3", &[(10, " +")])); // line 13 is gone
        assert_eq!(st.cursor_id, Some((Side::Additions, 11)));
    }

    #[test]
    fn toggling_the_mode_keeps_the_line() {
        let mut st = state();
        let snap = snapshot("a.rs", "r1", &[(10, " --+ ")]);
        st.reconcile(&snap);
        if let DiffState::Ready(d) = &snap.diff {
            st.set_cursor(d, 3);
        } // deletions line 12
        st.mode = ViewMode::Split;
        st.reconcile(&snap);
        assert_eq!(st.cursor_id, Some((Side::Deletions, 12)));
        assert!(st.rows.is_some());
    }

    #[test]
    fn a_diff_without_targets_clears_the_cursor() {
        let mut st = state();
        st.reconcile(&snapshot("bin.dat", "r1", &[]));
        assert_eq!(st.cursor, None);
        assert_eq!((st.offset, st.hscroll), (0, 0));
    }

    #[test]
    fn unchanged_diff_reuses_the_arc_and_built_rows() {
        let snap = snapshot("a.rs", "r1", &[(1, " + ")]);
        let mut st = state();
        st.reconcile(&snap);
        let rows = st.rows.as_ref().unwrap().rows.as_ptr();
        st.reconcile(&snap);
        assert_eq!(st.rows.as_ref().unwrap().rows.as_ptr(), rows);
        let DiffState::Ready(diff) = &snap.diff else {
            panic!()
        };
        assert!(Arc::ptr_eq(&st.built_from.as_ref().unwrap().0, diff));
    }

    #[test]
    fn refresh_reanchors_the_cursor_and_resize_keeps_it_visible() {
        let mut st = state();
        let kinds = "+".repeat(30);
        let snap = snapshot("a.rs", "r1", &[(10, &kinds)]);
        st.reconcile(&snap);
        let DiffState::Ready(diff) = &snap.diff else {
            panic!()
        };
        st.set_cursor(diff, 10);
        st.offset = 6;
        st.hscroll = 16;
        st.reconcile(&snapshot("a.rs", "r2", &[(1, "++"), (10, &kinds)]));
        let row = st.rows.as_ref().unwrap().row_of_target[st.cursor.unwrap()];
        assert_eq!(row - st.offset, 7);
        assert_eq!(st.hscroll, 16);
        st.resize(5);
        assert_eq!(row - st.offset, 4);
    }

    #[test]
    fn refresh_clamps_the_previous_index_when_its_side_disappears() {
        let mut st = state();
        let snap = snapshot("a.rs", "r1", &[(1, "-----")]);
        st.reconcile(&snap);
        let DiffState::Ready(diff) = &snap.diff else {
            panic!()
        };
        st.set_cursor(diff, 3);
        st.reconcile(&snapshot("a.rs", "r2", &[(1, "++")]));
        assert_eq!(st.cursor, Some(1));
        assert_eq!(st.cursor_id, Some((Side::Additions, 2)));
    }
}
