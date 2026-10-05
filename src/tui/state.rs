//! Everything the TUI owns that is not in the snapshot, and the 4.8 reconciliation rules.
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::engine::nav::{self, Side, ViewMode};
use crate::engine::{Command, DiffState, LoadedDiff, Snapshot, Target, TargetState};
use crate::tui::layout::{clamp_scroll, ensure_visible, reanchor, LineSpan};
use crate::tui::rows::{self, EditorAnchor, EditorPlace, Row, Rows};
use crate::tui::{cards, review};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesPanel {
    Hidden,
    Shown,
    Pinned,
}

/// Which of `observe`'s notices is on screen. Everything the shell and the keys set is `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoticeKind {
    Other,
    Rewrite,
    BaseError,
}

/// A confirmed action between `y` and the moment its keys are free again (spec 9.3).
pub struct PendingAction {
    pub diff: std::sync::Arc<LoadedDiff>,
    pub done: String,
    pub verb: &'static str,
    pub answered: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    /// Shown above the status and watcher lines until a body key acknowledges it.
    pub urgent: bool,
}

type BuiltFrom = (
    Option<Arc<LoadedDiff>>,
    ViewMode,
    Arc<Vec<crate::engine::comments::Comment>>,
    u64,
    u16,
    Option<EditorPlace>,
);

pub struct ViewState {
    pub drawn_head: Option<String>,
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
    /// The y/n box of spec 9.3, open from the main view only.
    pub confirm: Option<crate::tui::confirm::Confirm>,
    /// The action `y` sent, until its answer and, when a form ran, its Arc have both passed.
    pub pending_action: Option<PendingAction>,
    pub panes: Option<crate::tui::panes::PanePicker>,
    pub panes_token: u64,
    pub editor: Option<crate::tui::review::Editor>,
    pub visual: Option<crate::tui::review::Visual>,
    pub orphan: Option<usize>,
    pub comment_token: u64,
    pub review_box: Option<review::ReviewBox>,
    /// The OSC 52 sequence the shell takes once before the next frame.
    pub pending_copy: Option<String>,
    /// A command from observe, drained by the shell once.
    pub pending_command: Option<Command>,

    pub pick_token: u64,
    pub socket_path: Option<String>,
    pub refs_token: u64,
    /// Last submitted pick's reply sequence, retained when the picker closes.
    pub submitted_pick_seq: u64,
    pub notice: Option<Notice>,
    pub rows: Option<Rows>,
    pub body_height: u16,
    pub help_offset: usize,
    seen_base_error: Option<String>,
    /// A base error taken from a snapshot and still owed to the user.
    pending_base_error: Option<String>,
    /// What the notice on screen is, so displacing one can put it back.
    notice_kind: NoticeKind,
    seen_target_seq: u64,
    seen_comment_seq: u64,
    seen_send_seq: u64,
    seen_copy_seq: u64,
    deferred: VecDeque<String>,
    seen_mark_seq: u64,
    seen_action_seq: u64,
    seen_rewrite: Option<(String, String, crate::engine::MarkState)>,
    width: u16,
    /// The diff the rows were built from. Holding the Arc makes `Arc::ptr_eq` a safe,
    /// allocation-free "did anything change" test; the engine swaps the Arc only on change.
    built_from: Option<BuiltFrom>,
    last_diff: Option<Arc<LoadedDiff>>,
}

impl ViewState {
    pub fn new(mode: ViewMode, files_panel: FilesPanel, mouse: bool) -> Self {
        Self {
            drawn_head: None,
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
            confirm: None,
            pending_action: None,
            panes: None,
            panes_token: 0,
            editor: None,
            visual: None,
            orphan: None,
            comment_token: 0,
            review_box: None,
            pending_copy: None,
            pending_command: None,
            pick_token: 0,
            socket_path: None,
            refs_token: 0,
            submitted_pick_seq: 0,
            notice: None,
            rows: None,
            body_height: 0,
            help_offset: 0,
            seen_base_error: None,
            pending_base_error: None,
            notice_kind: NoticeKind::Other,
            seen_target_seq: 0,
            seen_comment_seq: 0,
            seen_send_seq: 0,
            seen_copy_seq: 0,
            deferred: VecDeque::new(),
            seen_mark_seq: 0,
            seen_action_seq: 0,
            seen_rewrite: None,
            width: 0,
            built_from: None,
            last_diff: None,
        }
    }

    pub fn notify(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice {
            text: text.into(),
            urgent: false,
        });
        self.notice_kind = NoticeKind::Other;
    }

    pub fn warn(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice {
            text: text.into(),
            urgent: true,
        });
        self.notice_kind = NoticeKind::Other;
    }

    /// Spec 9.3's guard: unanswered, or answered as applied while its Arc is still on screen.
    pub fn action_running(&self, snapshot: &Snapshot) -> bool {
        match &self.pending_action {
            None => false,
            Some(pending) if !pending.answered => true,
            Some(pending) => {
                matches!(&snapshot.diff, DiffState::Ready(d) if std::sync::Arc::ptr_eq(d, &pending.diff))
            }
        }
    }

    /// Record only a drawn body's id; any snapshot without an id clears the last one.
    pub fn record_drawn(&mut self, snapshot: &Snapshot, drew_body: bool) {
        self.drawn_head = if drew_body {
            snapshot.head.clone()
        } else {
            self.drawn_head.take().filter(|_| snapshot.head.is_some())
        };
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

    pub fn body_width(&self) -> u16 {
        self.width
            .saturating_sub(if self.files_panel == FilesPanel::Hidden {
                0
            } else {
                crate::tui::view::FILES_WIDTH + 1
            })
    }

    pub fn editor_place(&self) -> Option<EditorPlace> {
        let editor = self.editor.as_ref()?;
        let after = editor
            .editing
            .as_ref()
            .filter(|c| {
                self.rows
                    .as_ref()
                    .is_some_and(|rows| rows.orphan_tops.iter().any(|(_, id)| id == &c.id))
            })
            .map_or_else(
                || EditorAnchor::Anchor(editor.anchor.clone()),
                |c| EditorAnchor::Orphan(c.id.clone()),
            );
        Some(EditorPlace {
            after,
            lines: editor
                .lines(cards::card_width(usize::from(self.body_width()), self.mode))
                .len(),
        })
    }

    pub fn keep_cursor_visible(&mut self) {
        let Some(rows) = &self.rows else { return };
        let span = if self.editor.is_some() {
            let first = rows
                .rows
                .iter()
                .position(|r| matches!(r, Row::Editor { .. }));
            let last = rows
                .rows
                .iter()
                .rposition(|r| matches!(r, Row::Editor { .. }));
            first.zip(last).map(|(first, last)| {
                if last - first + 1 > usize::from(self.body_height) {
                    LineSpan {
                        start: last.saturating_sub(1),
                        height: 1,
                    }
                } else {
                    LineSpan {
                        start: first,
                        height: last - first + 1,
                    }
                }
            })
        } else if let Some(index) = self.orphan {
            rows.orphan_tops.get(index).map(|(row, _)| LineSpan {
                start: *row,
                height: 1,
            })
        } else {
            self.cursor
                .and_then(|c| rows.row_of_target.get(c))
                .map(|row| LineSpan {
                    start: *row,
                    height: 1,
                })
        };
        if let Some(span) = span {
            self.offset = ensure_visible(self.offset, span, self.body_height, rows.rows.len());
        }
    }

    /// Called once per snapshot the shell receives, before the next frame.
    pub fn observe(&mut self, snapshot: &Snapshot) {
        use crate::engine::MarkState;

        // Close the picker before showing any warning from the same reply.
        if let Some(picker) = &mut self.picker {
            picker.observe(snapshot);
            if picker.done {
                self.picker = None;
            }
        }
        if let Some(picker) = &mut self.panes {
            picker.observe(snapshot);
            if picker.done {
                if snapshot.target.is_some() {
                    match &picker.return_to {
                        crate::tui::panes::ReturnTo::Editor(anchor) => {
                            self.editor = Some(review::Editor::new(anchor.clone()))
                        }
                        crate::tui::panes::ReturnTo::Finish => {
                            self.review_box = Some(review::ReviewBox::finish())
                        }
                        crate::tui::panes::ReturnTo::Request(scope) => {
                            let mut b = review::ReviewBox::request(snapshot);
                            b.scope = scope.clone();
                            self.review_box = Some(b);
                        }
                        crate::tui::panes::ReturnTo::Nothing => {}
                    }
                }
                self.panes = None;
            }
        }
        let answered_target = snapshot.target_seq != self.seen_target_seq;
        if answered_target {
            self.seen_target_seq = snapshot.target_seq;
            if let Some(error) = &snapshot.target_error {
                self.displace_urgent();
                self.warn(crate::tui::sanitize::sanitize(error));
            }
        }
        let answered_comment = snapshot.comment_seq != self.seen_comment_seq;
        if answered_comment {
            self.seen_comment_seq = snapshot.comment_seq;
            if let Some(error) = &snapshot.comment_error {
                self.displace_urgent();
                self.warn(crate::tui::sanitize::sanitize(error));
            }
            if self
                .editor
                .as_ref()
                .is_some_and(|e| e.pending.is_some() && e.pending == snapshot.comment_token)
            {
                if snapshot.comment_refused {
                    if let Some(editor) = self.editor.as_mut() {
                        editor.pending = None;
                    }
                } else {
                    self.editor = None;
                }
            }
        }
        let send_answered = snapshot.send_seq != self.seen_send_seq;
        if send_answered {
            self.seen_send_seq = snapshot.send_seq;
            let answered_box = self
                .review_box
                .as_ref()
                .is_some_and(|b| b.pending.is_some_and(|p| snapshot.send_seq > p));
            match (&snapshot.send_error, &snapshot.send_outcome) {
                (Some(error), _) if answered_box => {
                    if let Some(b) = self.review_box.as_mut() {
                        b.refuse(
                            error.clone(),
                            snapshot
                                .send_refusal
                                .clone()
                                .unwrap_or(crate::engine::dispatch::Refusal::Other),
                        );
                    }
                }
                (Some(error), _) => {
                    self.displace_urgent();
                    self.notify(crate::tui::sanitize::sanitize(error));
                }
                (None, Some(outcome)) => {
                    self.review_box = None;
                    let (text, urgent) = crate::tui::review::outcome_notice(outcome, snapshot);
                    self.displace_urgent();
                    if urgent {
                        self.warn(text)
                    } else {
                        self.notify(text)
                    }
                }
                (None, None) => {}
            }
        }
        let copy_answered = snapshot.copy_seq != self.seen_copy_seq;
        if copy_answered {
            self.seen_copy_seq = snapshot.copy_seq;
            if let Some(copy) = &snapshot.copy {
                self.pending_copy = copy.osc.clone();
                if !send_answered {
                    self.displace_urgent();
                    if copy.urgent {
                        self.warn(copy.notice.clone())
                    } else {
                        self.notify(copy.notice.clone())
                    }
                }
            }
        }
        if let (
            Some(b),
            Some(Target::Pane { agent, pane, .. }),
            TargetState::Left | TargetState::Gone,
        ) = (&self.review_box, &snapshot.target, &snapshot.target_state)
        {
            if b.pending.is_none() && !b.offers(snapshot).nothing {
                let return_to = match b.kind {
                    crate::tui::review::BoxKind::Finish => crate::tui::panes::ReturnTo::Finish,
                    crate::tui::review::BoxKind::Request => {
                        crate::tui::panes::ReturnTo::Request(b.scope.clone())
                    }
                };
                self.review_box = None;
                self.displace_urgent();
                self.notify(format!(
                    "{} · {} is gone · pick a pane",
                    crate::tui::sanitize::sanitize(agent),
                    crate::tui::sanitize::sanitize(pane)
                ));
                self.panes_token += 1;
                self.panes = Some(crate::tui::panes::PanePicker::open(
                    self.panes_token,
                    return_to,
                ));
                self.pending_command = Some(Command::LoadPanes(self.panes_token));
            }
        }
        let mut answered_mark = false;
        if snapshot.mark_seq != self.seen_mark_seq {
            self.seen_mark_seq = snapshot.mark_seq;
            answered_mark = true;
            self.displace_urgent();
            match (&snapshot.mark_error, &snapshot.mark) {
                (Some(error), _) => self.warn(crate::tui::sanitize::sanitize(error)),
                (None, Some(mark)) => {
                    let short = &mark.commit[..mark.commit.len().min(7)];
                    self.notify(format!("marked {short} as reviewed"));
                }
                (None, None) => {}
            }
        }
        let mut answered_action = false;
        if snapshot.action_seq != self.seen_action_seq {
            self.seen_action_seq = snapshot.action_seq;
            answered_action = true;
            if let Some(pending) = self.pending_action.take() {
                self.displace_urgent();
                match &snapshot.action_error {
                    Some(error) => {
                        let mut text = format!(
                            "{} failed: {}",
                            pending.verb,
                            crate::tui::sanitize::sanitize(error)
                        );
                        if error.contains("does not match index") {
                            text.push_str("; unstage it first (s)");
                        }
                        self.warn(text);
                    }
                    None => self.notify(pending.done.clone()),
                }
                if snapshot.action_applied {
                    self.pending_action = Some(PendingAction {
                        answered: true,
                        ..pending
                    });
                }
            }
        }
        // An applied action holds the keys until its Arc has left the screen.
        if self.pending_action.as_ref().is_some_and(|p| p.answered)
            && !self.action_running(snapshot)
        {
            self.pending_action = None;
        }
        let answered_now =
            answered_target || answered_mark || answered_action || send_answered || copy_answered;
        // Warn only for the pair the engine conclusively classified.
        let rewritten = snapshot.mark.as_ref().and_then(|mark| {
            let at = mark.classified_at.clone()?;
            (Some(&at) == snapshot.head_seen.as_ref() && mark.state != MarkState::Current)
                .then(|| (mark.commit.clone(), at, mark.state.clone()))
        });
        // Leave the warning pending until an urgent answer has been acknowledged.
        let urgent_stands = self.notice.as_ref().is_some_and(|n| n.urgent);
        if rewritten != self.seen_rewrite && !answered_now && !urgent_stands {
            self.seen_rewrite = rewritten.clone();
            match rewritten.map(|(_, _, state)| state) {
                Some(MarkState::Rewritten) => {
                    self.warn("the marked commit is no longer on this branch; press M again");
                    self.notice_kind = NoticeKind::Rewrite;
                }
                Some(MarkState::Unreadable(reason)) => {
                    self.warn(format!(
                        "the mark cannot be read: {}",
                        crate::tui::sanitize::sanitize(&reason)
                    ));
                    self.notice_kind = NoticeKind::Rewrite;
                }
                _ => {}
            }
        }
        // One rule for every notice that has not been read: nothing overwrites it, and nothing
        // deferred is forgotten. A base error is taken from the snapshot once and held here,
        // because a later refresh can publish none and the snapshot is no place to keep
        // something still owed to the user.
        if snapshot.base_error != self.seen_base_error {
            self.seen_base_error = snapshot.base_error.clone();
            if let (Some(error), None) = (&snapshot.base_error, &self.picker) {
                self.pending_base_error = Some(crate::tui::sanitize::sanitize(error));
            }
        }
        // It then waits behind a mark answer from this same snapshot and behind any unread
        // warning -- including the one the branch above just set, whose pair is already
        // recorded and would never speak again.
        if let Some(error) = self.pending_base_error.clone() {
            if !answered_now && !self.notice.as_ref().is_some_and(|n| n.urgent) {
                self.pending_base_error = None;
                self.warn(error);
                self.notice_kind = NoticeKind::BaseError;
            }
        }
        if !answered_now && self.notice.is_none() {
            if let Some(text) = self.deferred.pop_front() {
                self.warn(text);
            }
        }
    }

    /// Urgent answers displace unread warnings; each warning retains its way back.
    fn displace_urgent(&mut self) {
        if let Some(displaced) = self.notice.as_ref().filter(|n| n.urgent) {
            match self.notice_kind {
                NoticeKind::Rewrite => self.seen_rewrite = None,
                NoticeKind::BaseError => self.pending_base_error = Some(displaced.text.clone()),
                NoticeKind::Other => self.deferred.push_back(displaced.text.clone()),
            }
        }
    }

    pub fn reconcile(&mut self, snapshot: &Snapshot) {
        let diff = match &snapshot.diff {
            DiffState::Ready(d) => Some(d),
            _ => None,
        };
        if self
            .visual
            .as_ref()
            .is_some_and(|v| diff.is_none_or(|d| !Arc::ptr_eq(d, &v.diff)))
        {
            self.visual = None;
        }
        let body_width = self.body_width();
        let mut files = std::collections::hash_map::DefaultHasher::new();
        (snapshot.scope == crate::engine::Scope::Branch).hash(&mut files);
        snapshot.files.len().hash(&mut files);
        for file in &snapshot.files {
            crate::engine::FileKey::of(file).hash(&mut files);
        }
        let files = files.finish();
        let mut editor = self.editor_place();
        if diff.is_none() {
            if let Some(editor) = &mut editor {
                if matches!(editor.after, EditorAnchor::Anchor(_)) {
                    editor.after = EditorAnchor::End;
                }
            }
        }
        if let Some((built, mode, comments, built_files, width, built_editor)) = &self.built_from {
            let same_diff = match (built, diff) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            };
            if same_diff
                && *mode == self.mode
                && Arc::ptr_eq(comments, &snapshot.comments)
                && *built_files == files
                && *width == body_width
                && *built_editor == editor
            {
                return;
            }
        }
        let keep_scrolled = editor.is_none()
            && self.orphan.is_none()
            && self.built_from.as_ref().is_some_and(|(built, mode, ..)| {
                *mode == self.mode
                    && built
                        .as_ref()
                        .zip(diff)
                        .is_some_and(|(a, b)| Arc::ptr_eq(a, b))
            })
            && self
                .cursor
                .and_then(|c| self.rows.as_ref()?.row_of_target.get(c))
                .is_some_and(|row| {
                    *row < self.offset || *row >= self.offset + usize::from(self.body_height)
                });
        self.built_from = Some((
            diff.cloned(),
            self.mode,
            snapshot.comments.clone(),
            files,
            body_width,
            editor.clone(),
        ));
        let orphans = rows::orphans(&snapshot.comments, snapshot);
        let card_width = cards::card_width(usize::from(body_width), self.mode);
        let Some(diff) = diff else {
            if self.orphan.is_some_and(|i| i >= orphans.len()) {
                self.orphan = None;
            }
            self.rows = if !orphans.is_empty() || editor.is_some() {
                Some(rows::orphans_only(&orphans, card_width, editor.as_ref()))
            } else {
                None
            };
            self.cursor = None;
            self.offset = self.rows.as_ref().map_or(0, |rows| {
                clamp_scroll(self.offset, rows.rows.len(), self.body_height)
            });
            self.hscroll = 0;
            self.keep_cursor_visible();
            return;
        };
        let same_file = self.last_diff.as_ref().is_some_and(|built| {
            built.key == diff.key
                || (built.comparison != diff.comparison && built.key.path == diff.key.path)
        });
        let old_row = match (&self.rows, self.cursor) {
            (Some(rows), Some(c)) => rows.row_of_target.get(c).copied(),
            _ => None,
        };
        let rows = rows::build(
            diff,
            self.mode,
            &rows::in_place(
                &snapshot.comments,
                diff,
                snapshot.scope == crate::engine::Scope::Branch,
            ),
            &orphans,
            card_width,
            editor.as_ref(),
        );
        if self.orphan.is_some_and(|i| i >= rows.orphan_tops.len()) {
            self.orphan = None;
        }

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
            same_file && !keep_scrolled,
            old_row,
            self.cursor.and_then(|c| rows.row_of_target.get(c).copied()),
        ) {
            self.offset = reanchor(self.offset, old, new, self.body_height, rows.rows.len());
        }
        self.rows = Some(rows);
        self.hscroll = self.hscroll.min(self.max_hscroll());
        if keep_scrolled {
            self.offset = clamp_scroll(
                self.offset,
                self.rows.as_ref().unwrap().rows.len(),
                self.body_height,
            );
        } else {
            self.keep_cursor_visible();
        }
        self.last_diff = Some(diff.clone());
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::engine::{Comparison, DiffState, FileKey, LoadedDiff, RepoState, Snapshot};
    use crate::git::{DiffHunk, DiffLine, DiffLineType, FileDiff, GetGitDiffResponse};
    use std::sync::Arc;

    pub(crate) fn snapshot(path: &str, raw: &str, hunks: &[(u32, &str)]) -> Snapshot {
        let hunks_spec = hunks;
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
        // The patch mirrors the parsed hunks, so the slicer and the box have real bytes to work on.
        let mut patch = format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n");
        for (start, kinds) in hunks_spec {
            let old = kinds.chars().filter(|c| *c != '+').count();
            let new = kinds.chars().filter(|c| *c != '-').count();
            patch.push_str(&format!("@@ -{start},{old} +{start},{new} @@\n"));
            for k in kinds.chars() {
                patch.push_str(&format!(
                    "{}line {k}\n",
                    if k == '+' || k == '-' { k } else { ' ' }
                ));
            }
        }
        let pre_image = Some(crate::engine::PreImage {
            index: Some("100644 0000000000000000000000000000000000000000 0".into()),
            source: None,
            worktree: crate::engine::WorktreeKind::File(0),
        });
        let loaded = LoadedDiff::build(
            key.clone(),
            Comparison::Worktree,
            None,
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
            patch.into_bytes(),
            pre_image,
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
        assert!(Arc::ptr_eq(
            st.built_from.as_ref().unwrap().0.as_ref().unwrap(),
            diff
        ));
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
            st.notice.as_ref().map(|n| n.text.as_str()),
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
        assert!(st
            .notice
            .as_ref()
            .unwrap()
            .text
            .contains("no longer on this branch"));

        st.notice = None;
        st.observe(&snap);
        assert!(st.notice.is_none(), "once per classified pair");

        snap.head_seen = Some("h2".repeat(20));
        st.observe(&snap);
        assert!(
            st.notice.is_none(),
            "the old classification cannot warn for a new head"
        );
        snap.mark.as_mut().unwrap().classified_at = snap.head_seen.clone();
        st.observe(&snap);
        assert!(st.notice.is_some(), "a new pair warns again");

        st.notice = None;
        snap.mark.as_mut().unwrap().state = MarkState::Unreadable("gone\u{1b}".into());
        st.observe(&snap);
        assert!(st.notice.as_ref().unwrap().urgent);
        assert_eq!(
            st.notice.as_ref().unwrap().text,
            "the mark cannot be read: gone\u{241b}"
        );
        st.notice = None;
        st.observe(&snap);
        assert!(
            st.notice.is_none(),
            "an unreadable pair also warns only once"
        );
    }

    #[test]
    fn a_base_error_does_not_overwrite_a_mark_answer_from_the_same_snapshot() {
        use crate::engine::{Mark, MarkState};
        let mut snap = snapshot("a.rs", "r1", &[(1, "+")]);
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        // One refresh answers the mark and reports a base problem at once.
        snap.mark_seq = 1;
        snap.mark_error = Some("mark not remembered: no state directory".into());
        snap.mark = Some(Mark {
            commit: "m".repeat(40),
            at: 1,
            state: MarkState::Current,
            classified_at: None,
        });
        snap.base_error = Some("remembered pick: not a commit: gone".into());
        st.observe(&snap);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some("mark not remembered: no state directory"),
            "the mark answer is not overwritten before it is drawn"
        );

        // The base error is not lost either: it speaks at the next snapshot.
        st.notice = None;
        st.observe(&snap);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some("remembered pick: not a commit: gone")
        );
    }

    #[test]
    fn a_mark_answer_over_an_unread_base_error_lets_it_speak_again() {
        use crate::engine::{Mark, MarkState};
        let mut snap = snapshot("a.rs", "r1", &[(1, "+")]);
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        // The pick that could not be written is on screen, unread.
        snap.base_error = Some("pick not remembered: no state directory".into());
        st.observe(&snap);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some("pick not remembered: no state directory")
        );

        // A mark answers before the next draw: it is shown, and the warning goes back.
        snap.base_error = None;
        snap.mark_seq = 1;
        snap.mark = Some(Mark {
            commit: "m".repeat(40),
            at: 1,
            state: MarkState::Current,
            classified_at: None,
        });
        st.observe(&snap);
        assert!(st.notice.as_ref().unwrap().text.contains("as reviewed"));

        st.notice = None;
        st.observe(&snap);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some("pick not remembered: no state directory"),
            "the pick failure was not lost behind the answer"
        );
        st.notice = None;
        st.observe(&snap);
        assert!(st.notice.is_none(), "and then it rests");
    }

    #[test]
    fn a_deferred_base_error_survives_a_refresh_that_no_longer_carries_it() {
        use crate::engine::{Mark, MarkState};
        let mut snap = snapshot("a.rs", "r1", &[(1, "+")]);
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        // An unwritable mark holds the screen; the pick that failed with it must wait.
        snap.mark_seq = 1;
        snap.mark_error = Some("mark not remembered: no state directory".into());
        snap.mark = Some(Mark {
            commit: "m".repeat(40),
            at: 1,
            state: MarkState::Current,
            classified_at: None,
        });
        snap.base_error = Some("pick not remembered: no state directory".into());
        st.observe(&snap);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some("mark not remembered: no state directory")
        );

        // An ordinary poll carries neither: the deferred warning is held here, not there.
        snap.mark_error = None;
        snap.base_error = None;
        st.observe(&snap);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some("mark not remembered: no state directory"),
            "the answer still stands"
        );
        st.notice = None;
        st.observe(&snap);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some("pick not remembered: no state directory"),
            "and the pick failure is still owed"
        );
        st.notice = None;
        st.observe(&snap);
        assert!(st.notice.is_none(), "each spoke exactly once");
    }

    #[test]
    fn a_mark_answer_over_an_unread_warning_lets_it_speak_again() {
        use crate::engine::{Mark, MarkState};
        let mut snap = snapshot("a.rs", "r1", &[(1, "+")]);
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        snap.head_seen = Some("h1".repeat(20));
        snap.mark = Some(Mark {
            commit: "m".repeat(40),
            at: 1,
            state: MarkState::Rewritten,
            classified_at: snap.head_seen.clone(),
        });
        st.observe(&snap);
        assert!(st
            .notice
            .as_ref()
            .unwrap()
            .text
            .contains("no longer on this branch"));

        // A mark requested before the warning appeared now answers, and fails: the answer is
        // shown, but the dots it does not explain are still on screen.
        snap.mark_seq = 1;
        snap.mark_error = Some("not a commit: bbbbbbb".into());
        st.observe(&snap);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some("not a commit: bbbbbbb"),
            "the answer to the key just pressed is not delayed"
        );

        // Reading it brings the warning back, because the classification still stands.
        st.notice = None;
        st.observe(&snap);
        assert!(st
            .notice
            .as_ref()
            .unwrap()
            .text
            .contains("no longer on this branch"));
        st.notice = None;
        st.observe(&snap);
        assert!(st.notice.is_none(), "and then it rests again");

        // An answer that resolves the classification leaves nothing behind it.
        st.observe(&snap);
        snap.mark_seq = 2;
        snap.mark_error = None;
        snap.mark.as_mut().unwrap().state = MarkState::Current;
        st.observe(&snap);
        assert!(st.notice.as_ref().unwrap().text.contains("as reviewed"));
        st.notice = None;
        st.observe(&snap);
        assert!(st.notice.is_none(), "nothing was queued behind it");
    }

    #[test]
    fn a_base_error_waits_behind_a_rewrite_warning_and_then_speaks() {
        use crate::engine::{Mark, MarkState};
        let mut snap = snapshot("a.rs", "r1", &[(1, "+")]);
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        snap.head_seen = Some("h1".repeat(20));
        // One refresh classifies the mark as rewritten and reports a base problem at once.
        snap.mark = Some(Mark {
            commit: "m".repeat(40),
            at: 1,
            state: MarkState::Rewritten,
            classified_at: snap.head_seen.clone(),
        });
        snap.base_error = Some("remembered pick: not a commit: gone".into());
        st.observe(&snap);
        assert!(
            st.notice
                .as_ref()
                .unwrap()
                .text
                .contains("no longer on this branch"),
            "the warning for the pair just classified is not overwritten"
        );

        // Its pair is recorded and would never warn again, so the base error had to wait; it
        // speaks once the warning is acknowledged.
        st.notice = None;
        st.observe(&snap);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some("remembered pick: not a commit: gone")
        );
        st.notice = None;
        st.observe(&snap);
        assert!(st.notice.is_none(), "each of them spoke exactly once");
    }

    #[test]
    fn a_mark_answer_outranks_a_rewrite_warning_in_the_same_snapshot() {
        use crate::engine::{Mark, MarkState};
        let mut snap = snapshot("a.rs", "r1", &[(1, "+")]);
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        snap.head_seen = Some("h1".repeat(20));
        // The mark answer and classification arrive together.
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
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some("mark not remembered: no state directory"),
            "the answer the user is owed is not overwritten"
        );

        // Further snapshots must not overwrite an unacknowledged answer.
        st.observe(&snap);
        st.observe(&snap);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some("mark not remembered: no state directory"),
        );

        // The warning follows once a body key clears the answer.
        st.notice = None;
        st.observe(&snap);
        assert!(st
            .notice
            .as_ref()
            .unwrap()
            .text
            .contains("no longer on this branch"));
    }

    use crate::tui::input::handle_key;
    use crate::tui::input::tests::{anchor_on, comment_at, key, review_setup};
    use crate::tui::review::Editor;
    use crate::tui::rows::Row;

    #[test]
    fn reconcile_rebuilds_on_a_new_comments_arc_and_on_width_and_not_otherwise() {
        let (mut snap, mut st) = review_setup(&[(10, " + ")]);
        let before = st.rows.as_ref().unwrap().rows.as_ptr();
        st.reconcile(&snap);
        assert_eq!(st.rows.as_ref().unwrap().rows.as_ptr(), before);
        snap.comments = Arc::new(vec![comment_at(
            &anchor_on(&snap, 11),
            &"字".repeat(200),
            1,
        )]);
        st.reconcile(&snap);
        assert!(st
            .rows
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .any(|r| matches!(r, Row::Card { .. })));
        let rows = st.rows.as_ref().unwrap().rows.as_ptr();
        st.reconcile(&snap);
        assert_eq!(st.rows.as_ref().unwrap().rows.as_ptr(), rows);
        let len = st.rows.as_ref().unwrap().rows.len();
        st.resize(100, 22);
        st.reconcile(&snap);
        assert!(st.rows.as_ref().unwrap().rows.len() > len);
        st.editor = Some(Editor::new(anchor_on(&snap, 11)));
        st.reconcile(&snap);
        let rows = st.rows.as_ref().unwrap().rows.as_ptr();
        st.editor.as_mut().unwrap().insert('a');
        st.reconcile(&snap);
        assert_eq!(
            st.rows.as_ref().unwrap().rows.as_ptr(),
            rows,
            "typing within one row reuses rows"
        );
        st.editor = None;
        snap.files.clear();
        st.reconcile(&snap);
        assert_eq!(
            st.rows.as_ref().unwrap().orphan_tops.len(),
            1,
            "file membership also rebuilds"
        );
    }

    #[test]
    fn an_editor_opened_on_the_bottom_row_scrolls_into_view() {
        let (snap, mut st) = review_setup(&[(1, &"+".repeat(40))]);
        st.resize(120, 10);
        handle_key(&mut st, &snap, key("G"), 120);
        handle_key(&mut st, &snap, key("i"), 120);
        st.reconcile(&snap);
        let rows = &st.rows.as_ref().unwrap().rows;
        let first = rows
            .iter()
            .position(|r| matches!(r, Row::Editor { .. }))
            .expect("editor rows");
        let last = rows
            .iter()
            .rposition(|r| matches!(r, Row::Editor { .. }))
            .unwrap();
        assert!(first >= st.offset && last < st.offset + 10);
    }

    #[test]
    fn an_editor_taller_than_the_body_keeps_its_caret_visible() {
        let (snap, mut st) = review_setup(&[(1, "+")]);
        st.resize(120, 8);
        handle_key(&mut st, &snap, key("i"), 120);
        for _ in 0..120 {
            handle_key(&mut st, &snap, key("ctrl+j"), 120);
            st.reconcile(&snap);
        }
        let editor = st.editor.as_ref().expect("editor");
        assert_eq!(editor.text.split('\n').count(), 100);
        let plain = crate::tui::view::render(&snap, &st, 120, 10).plain();
        assert!(
            plain[8].contains('_'),
            "the caret is on the body's bottom row: {plain:?}"
        );
    }

    #[test]
    fn a_partially_visible_editor_whose_anchor_scrolled_away_still_draws() {
        let (snap, mut st) = review_setup(&[(1, &"+".repeat(40))]);
        st.resize(120, 8);
        handle_key(&mut st, &snap, key("i"), 120);
        for _ in 0..5 {
            handle_key(&mut st, &snap, key("ctrl+j"), 120);
        }
        handle_key(&mut st, &snap, key("z"), 120);
        st.reconcile(&snap);
        handle_key(&mut st, &snap, key("ctrl+d"), 120);
        st.reconcile(&snap);
        assert!(
            matches!(st.rows.as_ref().unwrap().rows[st.offset], Row::Editor { line } if line > 0)
        );
        let plain = crate::tui::view::render(&snap, &st, 120, 10).plain();
        assert!(plain[1].trim_start().starts_with('│'), "{plain:?}");
        assert!(plain.iter().any(|l| l.contains("z_")));
    }

    #[test]
    fn an_accepted_comment_save_does_not_hide_a_base_error() {
        let (mut snap, mut st) = review_setup(&[(10, " + ")]);
        let mut editor = Editor::new(anchor_on(&snap, 11));
        editor.text = "saved".into();
        editor.submit(1);
        st.editor = Some(editor);
        snap.comment_seq = 1;
        snap.comment_token = Some(1);
        snap.base_error = Some("base failed".into());
        st.observe(&snap);
        assert!(st.editor.is_none());
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some("base failed")
        );
    }
}
