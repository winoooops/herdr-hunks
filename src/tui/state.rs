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
    pub requested_mode: ViewMode,
    pub mode: ViewMode,
    pub cursor: Option<usize>,
    pub cursor_id: Option<(Side, u32)>,
    pub offset: usize,
    pub hscroll: usize,
    pub files_panel: FilesPanel,
    pub mouse_requested: bool,
    pub hover: Option<(u16, u16)>,
    pub popup: bool,
    pub help_open: bool,
    pub picker: Option<crate::tui::picker::Picker>,
    /// Last submitted pick's reply sequence, retained when the picker closes.
    pub submitted_pick_seq: u64,
    pub notice: Option<String>,
    pub rows: Option<Rows>,
    pub body_height: u16,
    pub help_offset: usize,
    seen_base_error: Option<String>,
    width: u16,
    /// The diff the rows were built from. Holding the Arc makes `Arc::ptr_eq` a safe,
    /// allocation-free "did anything change" test; the engine swaps the Arc only on change.
    built_from: Option<(std::sync::Arc<LoadedDiff>, ViewMode)>,
}

impl ViewState {
    pub fn new(mode: ViewMode, files_panel: FilesPanel, mouse: bool) -> Self {
        Self {
            requested_mode: mode,
            mode,
            cursor: None,
            cursor_id: None,
            offset: 0,
            hscroll: 0,
            files_panel,
            mouse_requested: mouse,
            hover: None,
            popup: false,
            help_open: false,
            picker: None,
            submitted_pick_seq: 0,
            notice: None,
            rows: None,
            body_height: 0,
            help_offset: 0,
            seen_base_error: None,
            width: 0,
            built_from: None,
        }
    }

    /// Resolves the effective mode and clamps offsets before each frame.
    pub fn resize(&mut self, width: u16, body_height: u16) {
        self.width = width;
        self.mode = if width < crate::tui::view::MIN_SPLIT_WIDTH {
            ViewMode::Unified
        } else {
            self.requested_mode
        };
        self.hscroll = self.hscroll.min(self.max_hscroll());
        if self.body_height == body_height {
            return;
        }
        self.body_height = body_height;
        if let Some(rows) = &self.rows {
            self.offset = clamp_scroll(self.offset, rows.rows.len(), body_height);
            self.keep_cursor_visible();
        }
    }

    pub fn max_hscroll(&self) -> usize {
        let columns = self
            .width
            .saturating_sub(if self.files_panel == FilesPanel::Hidden {
                0
            } else {
                crate::tui::view::FILES_WIDTH + 1
            });
        // Keep room for the line-number gutters, sign, and space before the text.
        let text_columns = match self.mode {
            ViewMode::Unified => columns.saturating_sub(14),
            ViewMode::Split => (columns.saturating_sub(1) / 2).saturating_sub(8),
        }
        .max(1);
        self.rows.as_ref().map_or(0, |rows| {
            rows.max_text_width
                .saturating_sub(usize::from(text_columns))
        })
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

    /// Called once per snapshot the shell receives, before the next frame.
    pub fn observe(&mut self, snapshot: &Snapshot) {
        // Close the picker before showing any warning from the same reply.
        if let Some(picker) = &mut self.picker {
            picker.observe(snapshot);
            if picker.done {
                self.picker = None;
            }
        }
        if snapshot.base_error != self.seen_base_error {
            self.seen_base_error = snapshot.base_error.clone();
            if let (Some(error), None) = (&snapshot.base_error, &self.picker) {
                self.notice = Some(crate::tui::sanitize::sanitize(error));
            }
        }
    }

    pub fn reconcile(&mut self, snapshot: &Snapshot) {
        let DiffState::Ready(diff) = &snapshot.diff else {
            // Keep the identity and previous diff for the next Ready.
            self.rows = None;
            self.cursor = None;
            self.offset = 0;
            self.hscroll = 0;
            return;
        };
        // Runs before every frame, so the unchanged case must cost nothing: no clone, no compare of text.
        if let Some((built, mode)) = &self.built_from {
            if std::sync::Arc::ptr_eq(built, diff) && *mode == self.mode && self.rows.is_some() {
                return;
            }
        }
        let same_file = self
            .built_from
            .as_ref()
            .map(|(built, _)| {
                built.key == diff.key
                    || (built.comparison != diff.comparison && built.key.path == diff.key.path)
            })
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
        self.hscroll = self.hscroll.min(self.max_hscroll());
        self.keep_cursor_visible();
        self.built_from = Some((diff.clone(), self.mode));
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::engine::{Comparison, DiffState, FileKey, LoadedDiff, RepoState, Snapshot};
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
            untracked: false,
        };
        let loaded = LoadedDiff::build(
            key.clone(),
            Comparison::Worktree,
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
        s.resize(120, 10);
        s
    }

    pub(crate) fn text_snapshot(raw: &str, text: &str) -> Snapshot {
        let mut snap = snapshot("a.rs", raw, &[(1, "+")]);
        let DiffState::Ready(diff) = &mut snap.diff else {
            unreachable!()
        };
        Arc::make_mut(diff).file_diff.hunks[0].lines[0].content = text.into();
        snap
    }

    #[test]
    fn width_and_row_changes_clamp_horizontal_scroll() {
        let mut st = state();
        st.reconcile(&text_snapshot("r1", &"猫".repeat(70)));
        assert_eq!(st.rows.as_ref().unwrap().max_text_width, 140);
        st.hscroll = 34;
        st.resize(140, 10);
        assert_eq!(st.hscroll, 14);
        st.reconcile(&text_snapshot("r2", &"猫".repeat(60)));
        assert_eq!(st.hscroll, 0);
    }

    #[test]
    fn a_narrow_resize_preserves_the_requested_split_and_cursor_line() {
        let mut st = ViewState::new(ViewMode::Split, FilesPanel::Hidden, true);
        let snap = snapshot("a.rs", "r1", &[(10, " --+ ")]);
        st.resize(120, 10);
        st.reconcile(&snap);
        let DiffState::Ready(diff) = &snap.diff else {
            unreachable!()
        };
        st.set_cursor(diff, 3);
        let cursor_id = st.cursor_id;
        for (width, mode) in [(90, ViewMode::Unified), (120, ViewMode::Split)] {
            st.resize(width, 10);
            st.reconcile(&snap);
            assert_eq!(st.requested_mode, ViewMode::Split);
            assert_eq!(st.mode, mode);
            assert_eq!(st.built_from.as_ref().unwrap().1, mode);
            assert_eq!(st.cursor_id, cursor_id);
        }
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
        st.requested_mode = ViewMode::Split;
        st.resize(120, st.body_height);
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
        let path = "a".repeat(200);
        let snap = snapshot(&path, "r1", &[(10, &kinds)]);
        st.reconcile(&snap);
        let DiffState::Ready(diff) = &snap.diff else {
            panic!()
        };
        st.set_cursor(diff, 10);
        st.offset = 6;
        st.hscroll = 16;
        st.reconcile(&snapshot(&path, "r2", &[(1, "++"), (10, &kinds)]));
        let row = st.rows.as_ref().unwrap().row_of_target[st.cursor.unwrap()];
        assert_eq!(row - st.offset, 7);
        assert_eq!(st.hscroll, 16);
        st.resize(120, 5);
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

    #[test]
    fn a_new_base_error_becomes_a_notice_once() {
        let mut snap = snapshot("a.rs", "r1", &[(1, "+")]);
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        st.observe(&snap);
        assert!(st.notice.is_none());
        snap.base_error = Some("remembered pick: not a commit: gone\u{1b}".into());
        st.observe(&snap);
        assert_eq!(
            st.notice.as_deref(),
            Some("remembered pick: not a commit: gone\u{241b}")
        );
        st.notice = None;
        st.observe(&snap);
        assert!(st.notice.is_none(), "the same error is not repeated");
        snap.base_error = None;
        st.observe(&snap);
        assert!(st.notice.is_none());
    }

    #[test]
    fn a_scope_switch_keeps_the_line_and_a_new_path_starts_at_the_first_change() {
        use crate::engine::Comparison;
        let mut snap = snapshot("a.rs", "r1", &[(10, " --+ "), (40, "+")]);
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        st.resize(120, 20);
        st.reconcile(&snap);
        let first_change = st.cursor.expect("first changed row");
        let DiffState::Ready(diff) = &snap.diff else {
            unreachable!()
        };
        st.set_cursor(diff, 3);
        let id = st.cursor_id.expect("cursor identity");
        snap.diff = DiffState::Loading;
        st.reconcile(&snap);
        assert!(st.cursor.is_none() && st.rows.is_none());
        let mut branch = snapshot("a.rs", "r2", &[(10, " --+ "), (40, "+")]);
        if let DiffState::Ready(d) = &mut branch.diff {
            std::sync::Arc::make_mut(d).comparison = Comparison::Branch {
                merge_base: "1".repeat(40),
            };
        }
        snap.diff = branch.diff;
        st.reconcile(&snap);
        assert_eq!(
            st.cursor_id,
            Some(id),
            "the same path under the new comparison keeps its line"
        );
        assert_eq!(st.cursor, Some(3));
        snap.diff = DiffState::Loading;
        st.reconcile(&snap);
        snap.diff = snapshot("b.rs", "r3", &[(10, " --+ "), (40, "+")]).diff;
        st.reconcile(&snap);
        assert_eq!(
            st.cursor,
            Some(first_change),
            "a new path starts at its first change"
        );
        assert_ne!(st.cursor, Some(3));
    }
}
