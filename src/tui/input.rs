//! Pure input handling: change view state or request an engine command.
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use crate::engine::comments::{self, Anchor, AnchorComparison, Span as AnchorSpan};
use crate::engine::nav::{self, Side, ViewMode};
use crate::engine::{dispatch, Target, TargetState};
use crate::engine::{Command, Comparison, DiffState, FileKey, Snapshot, NO_BASE_NOTICE};
use crate::tui::keys::{self, KeyAction};
use crate::tui::layout::clamp_scroll;
use crate::tui::panes;
use crate::tui::sanitize::sanitize;
use crate::tui::state::{FilesPanel, ViewState};
use crate::tui::view::{self, Action, Rendered};
use crate::tui::{dialog, format};
use crate::tui::{review, rows};

#[derive(Debug, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)] // Commands carry the comment the editor saw.
pub enum Outcome {
    Quit,
    Redraw,
    Inert,
    Engine(Command),
}

fn scroll(offset: &mut usize, delta: isize, total: usize, height: u16) -> Outcome {
    let next = clamp_scroll(offset.saturating_add_signed(delta), total, height);
    if next == *offset {
        Outcome::Inert
    } else {
        *offset = next;
        Outcome::Redraw
    }
}

fn scroll_help(state: &mut ViewState, snapshot: &Snapshot, width: u16, delta: isize) -> Outcome {
    let total = dialog::line_count(&keys::help_panel(state.popup), width.min(60));
    // The sheet covers the body and notice; four rows belong to its frame and footer.
    let height = state
        .body_height
        .saturating_add(u16::from(view::notice(state, snapshot).is_some()))
        .saturating_sub(4);
    scroll(&mut state.help_offset, delta, total, height)
}

pub fn handle_key(
    state: &mut ViewState,
    snapshot: &Snapshot,
    key: KeyEvent,
    width: u16,
) -> Outcome {
    if key.kind == KeyEventKind::Release {
        return Outcome::Inert;
    }
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return Outcome::Quit;
    }
    let action = keys::lookup(&key);
    if state.help_open {
        if (key.code == KeyCode::Esc && key.modifiers.is_empty())
            || matches!(action, Some(KeyAction::Help | KeyAction::Quit))
        {
            state.help_open = false;
            state.help_offset = 0;
            state.notice = state.notice.take().filter(|n| n.urgent);
            return Outcome::Redraw;
        }
        return match action {
            Some(KeyAction::LineDown) => scroll_help(state, snapshot, width, 1),
            Some(KeyAction::LineUp) => scroll_help(state, snapshot, width, -1),
            _ => Outcome::Inert,
        };
    }
    if state.confirm.is_some() {
        return confirm_key(state, key);
    }
    if state.review_box.is_some() {
        return box_key(state, snapshot, key);
    }
    if state.editor.is_some() {
        let outcome = editor_key(state, snapshot, key);
        if outcome != Outcome::Inert {
            state.reconcile(snapshot);
            if !(key.modifiers == KeyModifiers::CONTROL
                && matches!(key.code, KeyCode::Char('d' | 'u')))
            {
                state.keep_cursor_visible();
            }
        }
        return outcome;
    }
    if state.panes.is_some() {
        return panes_key(state, snapshot, key, width);
    }
    if state.picker.is_some() {
        return picker_key(state, snapshot, key);
    }
    if state.visual.is_some() && key.code == KeyCode::Esc && key.modifiers.is_empty() {
        state.visual = None;
        return Outcome::Redraw;
    }
    if state.popup && key.code == KeyCode::Esc && key.modifiers.is_empty() {
        return Outcome::Quit;
    }
    if key.code == KeyCode::Char('y') && key.modifiers.is_empty() {
        if let (DiffState::Ready(diff), Some(visual), Some(cursor)) =
            (&snapshot.diff, &state.visual, state.cursor)
        {
            if std::sync::Arc::ptr_eq(diff, &visual.diff) {
                if let Some((side, start, end)) = review::selection(diff, visual, cursor) {
                    let text = review::selection_text(diff, side, start, end);
                    state.visual = None;
                    return Outcome::Engine(Command::Copy(dispatch::CopyRequest {
                        what: dispatch::CopyWhat::Selection(text),
                    }));
                }
            }
        }
        return Outcome::Inert;
    }
    match action {
        Some(action) => apply_action(state, snapshot, action, width),
        None => Outcome::Inert,
    }
}

pub fn handle_paste(state: &mut ViewState, snapshot: &Snapshot, text: &str) -> Outcome {
    if state.help_open || state.confirm.is_some() || state.review_box.is_some() {
        return Outcome::Inert;
    }
    if let Some(editor) = state.editor.as_mut() {
        if editor.pending.is_some() {
            return Outcome::Inert;
        }
        let mut chars = text.chars().peekable();
        while let Some(ch) = chars.next() {
            let accepted = match ch {
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    editor.newline()
                }
                '\n' => editor.newline(),
                ch if ch.is_control() => continue,
                ch => editor.insert(ch),
            };
            if !accepted {
                break;
            }
        }
        state.reconcile(snapshot);
        state.keep_cursor_visible();
        return Outcome::Redraw;
    }
    if let Some(picker) = state.panes.as_mut() {
        picker
            .input
            .extend(text.chars().filter(|ch| !ch.is_control()));
        picker.retarget(snapshot);
        return Outcome::Redraw;
    }
    if let Some(picker) = state.picker.as_mut() {
        let before = picker.input.len();
        picker
            .input
            .extend(text.chars().filter(|ch| !ch.is_control()));
        if picker.input.len() == before {
            return Outcome::Inert;
        }
        picker.retarget(snapshot);
        return Outcome::Redraw;
    }
    Outcome::Inert
}

fn box_key(state: &mut ViewState, snapshot: &Snapshot, key: KeyEvent) -> Outcome {
    let Some(b) = state.review_box.as_mut() else {
        return Outcome::Inert;
    };
    if b.pending.is_some() {
        return Outcome::Inert;
    }
    match (key.code, key.modifiers) {
        (KeyCode::Char('n'), KeyModifiers::NONE) | (KeyCode::Esc, KeyModifiers::NONE) => {
            state.review_box = None;
            Outcome::Redraw
        }
        (KeyCode::Char('Y'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
            if !b.drawn {
                return Outcome::Inert;
            }
            match b.submit(snapshot) {
                Some(command) => {
                    b.pending = Some(snapshot.send_seq);
                    Outcome::Engine(command)
                }
                None => Outcome::Inert,
            }
        }
        (KeyCode::Char('A'), KeyModifiers::SHIFT | KeyModifiers::NONE)
            if b.offers(snapshot).pick =>
        {
            let return_to = match b.kind {
                review::BoxKind::Finish => panes::ReturnTo::Finish,
                review::BoxKind::Request => panes::ReturnTo::Request(b.scope.clone()),
            };
            state.review_box = None;
            state.panes_token += 1;
            state.panes = Some(panes::PanePicker::open(state.panes_token, return_to));
            Outcome::Engine(Command::LoadPanes(state.panes_token))
        }
        (KeyCode::Char('c'), KeyModifiers::NONE) if b.offers(snapshot).copy => {
            let command = b.copy();
            state.review_box = None;
            Outcome::Engine(command)
        }
        (KeyCode::Char('f'), KeyModifiers::NONE) if b.kind == review::BoxKind::Request => {
            if let Some(key) = snapshot.selected.clone() {
                let scope = dispatch::ReviewScope::File(key);
                if b.scope_available(snapshot, &scope) {
                    b.scope = scope;
                    return Outcome::Redraw;
                }
            }
            Outcome::Inert
        }
        (KeyCode::Char('a'), KeyModifiers::NONE) if b.kind == review::BoxKind::Request => {
            if b.scope_available(snapshot, &dispatch::ReviewScope::All) {
                b.scope = dispatch::ReviewScope::All;
                Outcome::Redraw
            } else {
                Outcome::Inert
            }
        }
        _ => Outcome::Inert,
    }
}

/// The box of spec 9.3: `y` confirms once the box was drawn, `n` cancels, everything else is inert.
fn confirm_key(state: &mut ViewState, key: KeyEvent) -> Outcome {
    match (key.code, key.modifiers) {
        (KeyCode::Char('n'), KeyModifiers::NONE) => {
            state.confirm = None;
            Outcome::Redraw
        }
        (KeyCode::Char('y'), KeyModifiers::NONE) => {
            let Some(confirm) = state.confirm.take_if(|c| c.drawn) else {
                return Outcome::Inert;
            };
            state.pending_action = Some(crate::tui::state::PendingAction {
                diff: confirm.action.diff.clone(),
                done: confirm.done,
                verb: confirm.verb,
                answered: false,
            });
            Outcome::Engine(Command::Act(confirm.action))
        }
        _ => Outcome::Inert,
    }
}

/// Spec 9.2's refusals in the TUI's order, then the box; the engine re-decides all of them (9.4).
fn open_box(state: &mut ViewState, snapshot: &Snapshot, action: KeyAction) -> Outcome {
    use crate::engine::actions::{self, NOTICE_NO_HUNK, NOTICE_RUNNING, NOTICE_SCOPE};
    use crate::engine::{ActionKind, Scope};
    if state.orphan.is_some() {
        state.notify(NOTICE_NO_HUNK);
        return Outcome::Redraw;
    }
    if snapshot.scope == Scope::Branch {
        state.notify(NOTICE_SCOPE);
        return Outcome::Redraw;
    }
    if state.action_running(snapshot) {
        state.notify(NOTICE_RUNNING);
        return Outcome::Redraw;
    }
    let DiffState::Ready(diff) = &snapshot.diff else {
        state.notify(NOTICE_NO_HUNK);
        return Outcome::Redraw;
    };
    let kind = match action {
        KeyAction::StageHunk => ActionKind::Stage,
        KeyAction::DiscardHunk => ActionKind::Discard,
        _ => ActionKind::DiscardFile,
    };
    let hunk = if kind == ActionKind::DiscardFile || diff.key.untracked {
        None
    } else {
        state
            .cursor
            .and_then(|c| diff.targets.get(c))
            .map(|t| t.hunk_index)
    };
    let candidate = crate::engine::Action {
        kind,
        diff: diff.clone(),
        hunk,
    };
    if let Err(notice) = actions::plan(&candidate) {
        state.notify(notice);
        return Outcome::Redraw;
    }
    let total = diff.file_diff.hunks.len();
    state.confirm = Some(crate::tui::confirm::Confirm::open(
        kind,
        diff.clone(),
        hunk,
        total,
    ));
    Outcome::Redraw
}

fn editor_key(state: &mut ViewState, _snapshot: &Snapshot, key: KeyEvent) -> Outcome {
    let Some(editor) = state.editor.as_mut() else {
        return Outcome::Inert;
    };

    if editor.pending.is_some() && key.code != KeyCode::Esc {
        return Outcome::Inert;
    }
    if key.modifiers == KeyModifiers::CONTROL && matches!(key.code, KeyCode::Char('d' | 'u')) {
        let Some(rows) = &state.rows else {
            return Outcome::Inert;
        };
        let delta = isize::try_from(state.body_height / 2).unwrap_or(0);
        return scroll(
            &mut state.offset,
            if key.code == KeyCode::Char('d') {
                delta
            } else {
                -delta
            },
            rows.rows.len(),
            state.body_height,
        );
    }
    match (key.code, key.modifiers) {
        (KeyCode::Esc, KeyModifiers::NONE) => {
            state.editor = None;
            Outcome::Redraw
        }
        (KeyCode::Enter, KeyModifiers::NONE) => {
            state.comment_token += 1;
            match editor.submit(state.comment_token) {
                Some(command) => Outcome::Engine(command),
                None => Outcome::Inert,
            }
        }
        (KeyCode::Char('j'), KeyModifiers::CONTROL) => {
            editor.newline();
            Outcome::Redraw
        }
        (KeyCode::Char('h'), KeyModifiers::CONTROL) => {
            editor.category = editor.category.previous();
            Outcome::Redraw
        }
        (KeyCode::Char('l'), KeyModifiers::CONTROL) => {
            editor.category = editor.category.next();
            Outcome::Redraw
        }
        (KeyCode::Backspace, KeyModifiers::NONE) => {
            editor.backspace();
            Outcome::Redraw
        }
        (KeyCode::Char(ch), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
            editor.insert(ch);
            Outcome::Redraw
        }
        _ => Outcome::Inert,
    }
}

fn comment_key(state: &mut ViewState, snapshot: &Snapshot, action: KeyAction) -> Outcome {
    use KeyAction::*;

    if let Some(index) = state.orphan {
        let id = state
            .rows
            .as_ref()
            .and_then(|r| r.orphan_tops.get(index))
            .map(|(_, id)| id.clone());
        let comment = id.and_then(|id| {
            snapshot
                .comments
                .iter()
                .find(|c| c.id == id && c.is_editable())
                .cloned()
        });
        return match (action, comment) {
            (EditComment, Some(c)) => {
                state.editor = Some(review::Editor::edit(&c));
                Outcome::Redraw
            }
            (DeleteComment, Some(c)) => Outcome::Engine(Command::DeleteComment { seen: c }),
            _ => Outcome::Inert,
        };
    }
    let DiffState::Ready(diff) = &snapshot.diff else {
        state.notify(review::NO_LINE);
        return Outcome::Redraw;
    };
    let branch = snapshot.scope == crate::engine::Scope::Branch;
    let comparison = match (&diff.comparison, &snapshot.base) {
        (Comparison::Branch { merge_base }, Some(base)) => AnchorComparison::Branch {
            merge_base: merge_base.clone(),
            label: base.label().to_string(),
        },
        _ => AnchorComparison::Worktree,
    };
    let cursor_target = state
        .cursor
        .and_then(|c| diff.targets.get(c).map(|t| (c, t.side, t.line_number)));
    let in_place = rows::in_place(&snapshot.comments, diff, branch);

    let on_cursor_row = || -> Option<comments::Comment> {
        let cursor = state.cursor?;
        in_place
            .iter()
            .filter(|c| c.is_editable())
            .filter(|c| rows::anchor_target(diff, &c.anchor) == Some(cursor))
            .max_by_key(|c| (c.created_at, c.id.clone()))
            .map(|c| (*c).clone())
    };
    let file_comment = || -> Option<comments::Comment> {
        in_place
            .iter()
            .filter(|c| c.is_editable() && matches!(c.anchor.span, AnchorSpan::File))
            .max_by_key(|c| (c.created_at, c.id.clone()))
            .map(|c| (*c).clone())
    };
    let at_cap = snapshot.comments.iter().filter(|c| c.is_unsent()).count() >= comments::CAP;
    match action {
        Visual => {
            let Some((cursor, ..)) = cursor_target else {
                state.notify(review::NO_LINE);
                return Outcome::Redraw;
            };
            state.visual = Some(review::Visual {
                start: cursor,
                diff: diff.clone(),
            });
            Outcome::Redraw
        }
        Comment | CommentFile => {
            if at_cap {
                state.notify(comments::NOTICE_CAP);
                return Outcome::Redraw;
            }
            let anchor = if action == CommentFile {
                state.visual = None;
                Anchor {
                    key: diff.key.clone(),
                    side: Side::Additions,
                    line: 0,
                    span: AnchorSpan::File,
                    comparison,
                }
            } else {
                let Some((cursor, side, line)) = cursor_target else {
                    state.notify(review::NO_LINE);
                    return Outcome::Redraw;
                };

                match state
                    .visual
                    .take()
                    .filter(|v| std::sync::Arc::ptr_eq(&v.diff, diff))
                    .and_then(|v| review::selection(diff, &v, cursor))
                {
                    Some((side, start, end)) if start != end => Anchor {
                        key: diff.key.clone(),
                        side,
                        line: start,
                        span: AnchorSpan::Range { end },
                        comparison,
                    },
                    _ => Anchor {
                        key: diff.key.clone(),
                        side,
                        line,
                        span: AnchorSpan::Line,
                        comparison,
                    },
                }
            };
            if snapshot.target.is_none() {
                state.notify(review::CHOOSE_PANE);
                state.panes_token += 1;
                state.panes = Some(crate::tui::panes::PanePicker::open(
                    state.panes_token,
                    crate::tui::panes::ReturnTo::Editor(anchor),
                ));
                return Outcome::Engine(Command::LoadPanes(state.panes_token));
            }
            state.editor = Some(review::Editor::new(anchor));
            Outcome::Redraw
        }
        EditComment | DeleteComment => {
            let Some(comment) = on_cursor_row() else {
                state.notify(if cursor_target.is_none() {
                    review::NO_LINE
                } else {
                    review::NO_COMMENT
                });
                return Outcome::Redraw;
            };
            if action == EditComment {
                state.editor = Some(review::Editor::edit(&comment));
                Outcome::Redraw
            } else {
                Outcome::Engine(Command::DeleteComment { seen: comment })
            }
        }
        EditFileComment | DeleteFileComment => {
            let Some(comment) = file_comment() else {
                state.notify(review::NO_COMMENT);
                return Outcome::Redraw;
            };
            if action == EditFileComment {
                state.editor = Some(review::Editor::edit(&comment));
                Outcome::Redraw
            } else {
                Outcome::Engine(Command::DeleteComment { seen: comment })
            }
        }
        _ => Outcome::Inert,
    }
}

fn panes_key(state: &mut ViewState, snapshot: &Snapshot, key: KeyEvent, width: u16) -> Outcome {
    let overlay = state
        .body_height
        .saturating_add(u16::from(view::notice(state, snapshot).is_some()));
    let Some(picker) = state.panes.as_mut() else {
        return Outcome::Inert;
    };
    let choices = picker.choices(snapshot);
    let visible = picker.visible(snapshot, width.min(panes::WIDTH), overlay);
    match (key.code, key.modifiers) {
        (KeyCode::Esc, KeyModifiers::NONE) => {
            state.panes = None;
            Outcome::Redraw
        }
        (KeyCode::Enter, KeyModifiers::NONE) => pick_pane(state, snapshot, picker_cursor(state)),
        (KeyCode::Down, KeyModifiers::NONE) | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
            redraw_if(picker.move_by(1, &choices, visible))
        }
        (KeyCode::Up, KeyModifiers::NONE) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
            redraw_if(picker.move_by(-1, &choices, visible))
        }
        (KeyCode::Backspace, KeyModifiers::NONE) => {
            if picker.input.pop().is_some() {
                picker.retarget(snapshot);
                Outcome::Redraw
            } else {
                Outcome::Inert
            }
        }
        (KeyCode::Char(ch), KeyModifiers::NONE | KeyModifiers::SHIFT) if !ch.is_control() => {
            picker.input.push(ch);
            picker.retarget(snapshot);
            Outcome::Redraw
        }
        _ => Outcome::Inert,
    }
}

fn redraw_if(moved: bool) -> Outcome {
    if moved {
        Outcome::Redraw
    } else {
        Outcome::Inert
    }
}

fn picker_cursor(state: &ViewState) -> usize {
    state.panes.as_ref().map(|p| p.cursor).unwrap_or(0)
}

/// Enter or a click submits a target under a fresh token and waits for its echoed answer.
fn pick_pane(state: &mut ViewState, snapshot: &Snapshot, index: usize) -> Outcome {
    let socket = state.socket_path.clone().unwrap_or_default();
    let Some(picker) = state.panes.as_mut() else {
        return Outcome::Inert;
    };
    if picker.pending.is_some() {
        return Outcome::Inert;
    }
    let choices = picker.choices(snapshot);
    let Some(choice) = choices.get(index) else {
        return Outcome::Inert;
    };
    picker.cursor = index;
    state.pick_token += 1;
    picker.pending = Some(state.pick_token);
    Outcome::Engine(Command::SetTarget {
        token: state.pick_token,
        target: choice.target(&socket),
    })
}

fn picker_key(state: &mut ViewState, snapshot: &Snapshot, key: KeyEvent) -> Outcome {
    let visible = state
        .picker
        .as_ref()
        .map(|p| {
            p.visible(
                state
                    .body_height
                    .saturating_add(u16::from(view::notice(state, snapshot).is_some())),
            )
        })
        .unwrap_or(1);
    let Some(picker) = state.picker.as_mut() else {
        return Outcome::Inert;
    };
    let rows = picker.rows(snapshot);
    let moved = |picker: &mut crate::tui::picker::Picker, delta: isize| {
        if picker.move_by(delta, &rows, visible) {
            Outcome::Redraw
        } else {
            Outcome::Inert
        }
    };
    match (key.code, key.modifiers) {
        (KeyCode::Esc, KeyModifiers::NONE) => {
            state.picker = None;
            Outcome::Redraw
        }
        (KeyCode::Enter, KeyModifiers::NONE) => {
            let index = picker.cursor;
            pick_row(state, snapshot, index)
        }
        (KeyCode::Down, KeyModifiers::NONE) | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
            moved(picker, 1)
        }
        (KeyCode::Up, KeyModifiers::NONE) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
            moved(picker, -1)
        }
        (KeyCode::Backspace, KeyModifiers::NONE) => {
            if picker.input.pop().is_some() {
                picker.retarget(snapshot);
                Outcome::Redraw
            } else {
                Outcome::Inert
            }
        }
        (KeyCode::Char(ch), KeyModifiers::NONE | KeyModifiers::SHIFT) if !ch.is_control() => {
            picker.input.push(ch);
            picker.retarget(snapshot);
            Outcome::Redraw
        }
        _ => Outcome::Inert,
    }
}

fn pick_row(state: &mut ViewState, snapshot: &Snapshot, index: usize) -> Outcome {
    let Some(picker) = state.picker.as_mut() else {
        return Outcome::Inert;
    };
    if picker.pending.is_some() {
        return Outcome::Inert;
    }
    let rows = picker.rows(snapshot);
    let Some(row) = rows.get(index) else {
        return Outcome::Inert;
    };
    let sent = snapshot.pick_seq.max(state.submitted_pick_seq);
    state.submitted_pick_seq = sent + 1;
    picker.set_cursor(index, &rows);
    picker.pending = Some(sent);
    picker.error = None;
    Outcome::Engine(Command::SetBase(row.submit()))
}

fn apply_action(
    state: &mut ViewState,
    snapshot: &Snapshot,
    action: KeyAction,
    width: u16,
) -> Outcome {
    let cleared_notice = state.notice.take().is_some();
    let outcome = act(state, snapshot, action, width);
    // A warning held back behind that answer has no new snapshot to arrive on: a settled
    // repository publishes none. Give it this frame, now that the answer is acknowledged.
    if cleared_notice && state.notice.is_none() {
        state.observe(snapshot);
    }
    if cleared_notice && outcome == Outcome::Inert {
        Outcome::Redraw
    } else {
        outcome
    }
}

fn act(state: &mut ViewState, snapshot: &Snapshot, action: KeyAction, width: u16) -> Outcome {
    use KeyAction::*;
    match action {
        Quit => return Outcome::Quit,
        Refresh => return Outcome::Engine(Command::Refresh),
        ToggleScope => {
            return match snapshot.base {
                None => {
                    state.notify(NO_BASE_NOTICE);
                    Outcome::Redraw
                }
                Some(_) => Outcome::Engine(Command::SetScope(snapshot.scope.other())),
            };
        }
        Comment | CommentFile | Visual | EditComment | EditFileComment | DeleteComment
        | DeleteFileComment => {
            return comment_key(state, snapshot, action);
        }
        Finish | RequestReview => {
            let kind = if action == Finish {
                review::BoxKind::Finish
            } else {
                review::BoxKind::Request
            };
            if action == Finish && review::counts(snapshot) == (0, 0, 0) {
                state.notify(dispatch::NOTICE_NO_PENDING);
                return Outcome::Redraw;
            }
            // Nothing to review: the box says so with `n` alone (10.4), whatever the target; no picker.
            if action == RequestReview && snapshot.files.is_empty() {
                state.review_box = Some(review::ReviewBox::request(snapshot));
                return Outcome::Redraw;
            }
            match (&snapshot.target, &snapshot.target_state) {
                (None, _) | (Some(_), TargetState::Left | TargetState::Gone) => {
                    if let Some(Target::Pane { agent, pane, .. }) = &snapshot.target {
                        state.notify(format!(
                            "{} · {} is gone · pick a pane",
                            sanitize(agent),
                            sanitize(pane)
                        ));
                    }
                    let return_to = if action == Finish {
                        panes::ReturnTo::Finish
                    } else {
                        panes::ReturnTo::Request(dispatch::ReviewScope::All)
                    };
                    state.panes_token += 1;
                    state.panes = Some(panes::PanePicker::open(state.panes_token, return_to));
                    return Outcome::Engine(Command::LoadPanes(state.panes_token));
                }
                _ => {}
            }
            state.review_box = Some(match kind {
                review::BoxKind::Finish => review::ReviewBox::finish(),
                review::BoxKind::Request => review::ReviewBox::request(snapshot),
            });
        }
        PickPane => {
            state.panes_token += 1;
            state.panes = Some(crate::tui::panes::PanePicker::open(
                state.panes_token,
                crate::tui::panes::ReturnTo::Nothing,
            ));
            return Outcome::Engine(Command::LoadPanes(state.panes_token));
        }
        PickBase => {
            state.refs_token += 1;
            state.picker = Some(crate::tui::picker::Picker::open(state.refs_token));
            return Outcome::Engine(Command::LoadRefs(state.refs_token));
        }
        MarkReviewed => {
            return match state.drawn_head.clone() {
                None => {
                    state.notify("nothing to mark: no commit yet");
                    Outcome::Redraw
                }
                Some(commit) => Outcome::Engine(Command::MarkReviewed(commit)),
            };
        }
        FileNext | FilePrev => {
            state.orphan = None;
            return Outcome::Engine(if action == FileNext {
                Command::SelectNext
            } else {
                Command::SelectPrev
            });
        }
        ToggleView => {
            if state.requested_mode == ViewMode::Unified && width < view::MIN_SPLIT_WIDTH {
                state.notify("split view needs 100 columns");
            } else {
                state.requested_mode = match state.requested_mode {
                    ViewMode::Unified => ViewMode::Split,
                    ViewMode::Split => ViewMode::Unified,
                };
                state.resize(width, state.body_height);
            }
        }
        ToggleFiles => {
            state.files_panel = match state.files_panel {
                FilesPanel::Hidden => FilesPanel::Shown,
                _ => FilesPanel::Hidden,
            };
        }
        PinFiles => {
            state.files_panel = match state.files_panel {
                FilesPanel::Pinned => FilesPanel::Shown,
                _ => FilesPanel::Pinned,
            };
        }
        ToggleMouse => {
            state.mouse_requested = !state.mouse_requested;
            if !state.mouse_requested {
                state.hover = None;
            }
        }
        Help => {
            state.help_open = true;
            state.help_offset = 0;
        }
        StageHunk | DiscardHunk | DiscardFile => return open_box(state, snapshot, action),
        _ => {
            let orphan = state.orphan;
            let outcome = move_cursor(state, snapshot, action);
            if orphan != state.orphan {
                state.keep_cursor_visible();
                if outcome == Outcome::Inert {
                    return Outcome::Redraw;
                }
            }
            return outcome;
        }
    }
    Outcome::Redraw
}

fn move_cursor(state: &mut ViewState, snapshot: &Snapshot, action: KeyAction) -> Outcome {
    use KeyAction::*;
    if (state.orphan.is_some() || state.cursor.is_none())
        && matches!(action, HalfPageDown | HalfPageUp)
    {
        let Some(rows) = &state.rows else {
            return Outcome::Inert;
        };
        let step = state.body_height as isize;
        return scroll(
            &mut state.offset,
            if action == HalfPageDown { step } else { -step },
            rows.rows.len(),
            state.body_height,
        );
    }
    let orphan_count = state.rows.as_ref().map_or(0, |r| r.orphan_tops.len());
    if let Some(index) = state.orphan {
        match action {
            LineDown => {
                if index + 1 >= orphan_count {
                    return Outcome::Inert;
                }
                state.orphan = Some(index + 1);
                state.keep_cursor_visible();
                return Outcome::Redraw;
            }
            LineUp => {
                state.orphan = index.checked_sub(1);
                state.keep_cursor_visible();
                return Outcome::Redraw;
            }
            _ => state.orphan = None,
        }
    } else if action == LineDown && orphan_count > 0 && state.visual.is_none() {
        let at_end = match (&snapshot.diff, state.cursor) {
            (DiffState::Ready(diff), Some(cursor)) => match state.mode {
                ViewMode::Unified => diff.unified_order.last() == Some(&cursor),
                ViewMode::Split => state.rows.as_ref().is_some_and(|rows| {
                    rows.row_of_target.get(cursor) == rows.row_of_target.last()
                }),
            },
            _ => true,
        };
        if at_end {
            state.orphan = Some(0);
            state.keep_cursor_visible();
            return Outcome::Redraw;
        }
    }
    let (DiffState::Ready(diff), Some(cursor)) = (&snapshot.diff, state.cursor) else {
        return Outcome::Inert;
    };
    let Some(current) = diff.targets.get(cursor) else {
        return Outcome::Inert;
    };
    let target = match action {
        LineDown | LineUp => {
            let delta = if action == LineDown { 1 } else { -1 };
            Some(
                match state
                    .visual
                    .as_ref()
                    .filter(|v| std::sync::Arc::ptr_eq(&v.diff, diff))
                {
                    Some(visual) => {
                        review::step_on_side(diff, cursor, diff.targets[visual.start].side, delta)
                    }
                    None => nav::move_line(
                        &diff.targets,
                        &diff.unified_order,
                        cursor,
                        delta as i32,
                        state.mode,
                    ),
                },
            )
        }
        SideDeletions | SideAdditions => {
            let target = nav::move_side(
                &diff.targets,
                cursor,
                if action == SideDeletions {
                    Side::Deletions
                } else {
                    Side::Additions
                },
                state.mode,
            );
            if state.mode == ViewMode::Split && target != cursor {
                if let Some(visual) = &mut state.visual {
                    visual.start = target;
                }
            }
            Some(target)
        }
        HunkPrev | HunkNext => {
            let hunk = if action == HunkNext {
                current.hunk_index.checked_add(1)
            } else {
                current.hunk_index.checked_sub(1)
            };
            hunk.and_then(|hunk| nav::target_index_for_hunk(&diff.targets, hunk))
        }
        First | Last => match (state.mode, action) {
            (ViewMode::Unified, First) => diff.unified_order.first().copied(),
            (ViewMode::Unified, _) => diff.unified_order.last().copied(),
            (ViewMode::Split, First) => Some(0),
            (ViewMode::Split, _) => diff.targets.len().checked_sub(1),
        },
        HalfPageDown | HalfPageUp => {
            let Some(rows) = &state.rows else {
                return Outcome::Inert;
            };
            let step = (state.body_height / 2) as isize;
            let outcome = scroll(
                &mut state.offset,
                if action == HalfPageDown { step } else { -step },
                rows.rows.len(),
                state.body_height,
            );
            if outcome == Outcome::Inert {
                return outcome;
            }
            let centre = state.offset + usize::from(state.body_height / 2);
            if let Some((target, _)) = rows
                .row_of_target
                .iter()
                .enumerate()
                .min_by_key(|(_, row)| row.abs_diff(centre))
            {
                state.set_cursor(diff, target);
            }
            return Outcome::Redraw;
        }
        ScrollLeft | ScrollRight => {
            let limit = state.max_hscroll();
            return scroll(
                &mut state.hscroll,
                if action == ScrollRight { 8 } else { -8 },
                limit,
                0,
            );
        }
        _ => return Outcome::Inert,
    };
    match target {
        Some(target) if target != cursor => {
            state.set_cursor(diff, target);
            Outcome::Redraw
        }
        _ => Outcome::Inert,
    }
}

pub fn handle_mouse(
    state: &mut ViewState,
    snapshot: &Snapshot,
    rendered: &Rendered,
    ev: MouseEvent,
) -> Outcome {
    if !state.mouse_requested || !ev.modifiers.is_empty() {
        return Outcome::Inert;
    }
    // The box has to be answered: no click or wheel reaches anything under it.
    if state.confirm.is_some() || state.review_box.is_some() {
        return Outcome::Inert;
    }
    if let Some(editor) = state.editor.as_mut() {
        return match ev.kind {
            MouseEventKind::Down(MouseButton::Left) if editor.pending.is_none() => {
                match rendered.hit(ev.column, ev.row) {
                    Some(Action::EditorCategory(category)) => {
                        editor.category = *category;
                        Outcome::Redraw
                    }
                    _ => Outcome::Inert,
                }
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let Some(rows) = &state.rows else {
                    return Outcome::Inert;
                };
                scroll(
                    &mut state.offset,
                    if ev.kind == MouseEventKind::ScrollDown {
                        3
                    } else {
                        -3
                    },
                    rows.rows.len(),
                    state.body_height,
                )
            }
            _ => Outcome::Inert,
        };
    }
    if ev.kind == MouseEventKind::Moved {
        let target = |point: Option<(u16, u16)>| {
            point
                .and_then(|(x, y)| rendered.hit(x, y))
                .filter(|action| !matches!(action, Action::CursorToRow(_)))
        };
        let previous = target(state.hover);
        state.hover = Some((ev.column, ev.row));
        return if !state.help_open && previous != target(state.hover) {
            Outcome::Redraw
        } else {
            Outcome::Inert
        };
    }
    let width = rendered
        .lines
        .first()
        .map(|line| {
            line.iter()
                .map(|span| format::width(&span.text))
                .sum::<usize>()
        })
        .unwrap_or(0)
        .min(usize::from(u16::MAX)) as u16;
    if matches!(
        ev.kind,
        MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
    ) {
        let delta = if ev.kind == MouseEventKind::ScrollDown {
            3
        } else {
            -3
        };
        let overlay = state
            .body_height
            .saturating_add(u16::from(view::notice(state, snapshot).is_some()));
        if let Some(picker) = state.panes.as_mut() {
            let choices = picker.choices(snapshot);
            let visible = picker.visible(snapshot, width.min(panes::WIDTH), overlay);
            return redraw_if(picker.move_by(delta, &choices, visible));
        }
        if let Some(picker) = state.picker.as_mut() {
            let visible = picker.visible(overlay);
            let rows = picker.rows(snapshot);
            return if picker.move_by(delta, &rows, visible) {
                Outcome::Redraw
            } else {
                Outcome::Inert
            };
        }
        if state.help_open {
            return scroll_help(state, snapshot, width, delta);
        }
        let Some(rows) = &state.rows else {
            return Outcome::Inert;
        };
        return scroll(&mut state.offset, delta, rows.rows.len(), state.body_height);
    }
    if state.help_open || ev.kind != MouseEventKind::Down(MouseButton::Left) {
        return Outcome::Inert;
    }
    if state.panes.is_some() {
        return match rendered.hit(ev.column, ev.row) {
            Some(Action::PickPaneRow(index)) => pick_pane(state, snapshot, *index),
            _ => Outcome::Inert,
        };
    }
    if state.picker.is_some() {
        return match rendered.hit(ev.column, ev.row) {
            Some(Action::PickRow(index)) => pick_row(state, snapshot, *index),
            _ => Outcome::Inert,
        };
    }
    let hit = rendered.hit(ev.column, ev.row);
    if let Some(action) = hit.and_then(Action::key_action) {
        return apply_action(state, snapshot, action, width);
    }
    match hit {
        Some(Action::SelectFile(index)) => snapshot
            .files
            .get(*index)
            .map(|file| {
                state.orphan = None;
                Outcome::Engine(Command::Select(FileKey::of(file)))
            })
            .unwrap_or(Outcome::Inert),
        Some(Action::CursorToRow(row)) => {
            let Some(rows) = &state.rows else {
                return Outcome::Inert;
            };
            if let Some(rows::Row::Card { id, .. }) = rows.rows.get(*row) {
                if let Some(index) = rows.orphan_tops.iter().position(|(_, card)| card == id) {
                    if state.orphan == Some(index) {
                        return Outcome::Inert;
                    }
                    state.orphan = Some(index);
                    state.keep_cursor_visible();
                    return Outcome::Redraw;
                }
            }
            if let DiffState::Ready(diff) = &snapshot.diff {
                let target = match rows.rows.get(*row) {
                    Some(rows::Row::Card { target, .. }) => *target,
                    _ => rows.row_of_target.iter().position(|r| r == row),
                };
                if let Some(target) = target {
                    let changed = state.cursor != Some(target) || state.orphan.is_some();
                    state.orphan = None;
                    if changed {
                        state.set_cursor(diff, target);
                        return Outcome::Redraw;
                    }
                }
            }
            Outcome::Inert
        }
        _ => Outcome::Inert,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::engine::nav::ViewMode;
    use crate::engine::{Command, DiffState};
    use crate::tui::keys::{KEYS, RESERVED};
    use crate::tui::state::tests::snapshot;
    use crate::tui::state::{FilesPanel, ViewState};
    use crate::tui::view::{body_height, render};
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };

    pub(crate) fn key(text: &str) -> KeyEvent {
        let code = match text {
            "Enter" => KeyCode::Enter,
            "Esc" => KeyCode::Esc,
            "Backspace" => KeyCode::Backspace,
            "Down" => KeyCode::Down,
            "Up" => KeyCode::Up,
            _ => {
                return match text.strip_prefix("ctrl+") {
                    Some(c) => KeyEvent::new(
                        KeyCode::Char(c.chars().next().unwrap()),
                        KeyModifiers::CONTROL,
                    ),
                    None => KeyEvent::new(
                        KeyCode::Char(text.chars().next().unwrap()),
                        KeyModifiers::NONE,
                    ),
                }
            }
        };
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn setup(hunks: &[(u32, &str)]) -> (crate::engine::Snapshot, ViewState) {
        let snap = snapshot("a.rs", "r1", hunks);
        let mut st = ViewState::new(ViewMode::Split, FilesPanel::Hidden, true);
        st.resize(120, body_height(&st, &snap, 24));
        st.reconcile(&snap);
        (snap, st)
    }

    #[test]
    fn ctrl_c_quits_with_or_without_help_and_plain_c_stays_inert() {
        for help_open in [false, true] {
            let (snap, mut st) = setup(&[(1, "+")]);
            st.help_open = help_open;
            assert_eq!(handle_key(&mut st, &snap, key("c"), 120), Outcome::Inert);
            assert_eq!(
                handle_key(&mut st, &snap, key("ctrl+c"), 120),
                Outcome::Quit
            );
            let snap = crate::engine::Snapshot::empty("/r");
            st.reconcile(&snap);
            assert_eq!(
                handle_key(&mut st, &snap, key("ctrl+c"), 120),
                Outcome::Quit
            );
        }
    }

    #[test]
    fn down_and_up_scroll_help_like_j_and_k() {
        let (snap, mut st) = setup(&[(1, "+")]);
        st.resize(120, body_height(&st, &snap, 12));
        handle_key(&mut st, &snap, key("?"), 120);
        for (code, expected_offset) in [(KeyCode::Down, 1), (KeyCode::Up, 0)] {
            assert_eq!(
                handle_key(&mut st, &snap, KeyEvent::new(code, KeyModifiers::NONE), 120),
                Outcome::Redraw
            );
            assert_eq!(st.help_offset, expected_offset);
        }
        assert_eq!(
            handle_key(
                &mut st,
                &snap,
                KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT),
                120
            ),
            Outcome::Inert
        );
        assert_eq!(st.help_offset, 0);
    }

    #[test]
    fn every_key_on_the_sheet_does_something_and_reserved_keys_do_nothing() {
        // A key may be inert at an edge, so each one is tried from four positions:
        // the start (a deletion of a replacement pair), its additions side, the
        // middle of the diff, and a scrolled viewport.
        let long = "+".repeat(80);
        let prefixes: [&[&str]; 4] = [&[], &["l"], &["]", "L"], &["ctrl+d"]];
        for binding in KEYS {
            let live = prefixes.iter().any(|prefix| {
                let (mut snap, mut st) = setup(&[(10, " --+ "), (40, long.as_str())]);
                if let DiffState::Ready(diff) = &mut snap.diff {
                    std::sync::Arc::make_mut(diff).file_diff.hunks[0].lines[0].content =
                        "x".repeat(200);
                }
                st.reconcile(&snap);
                for p in prefix.iter() {
                    handle_key(&mut st, &snap, key(p), 120);
                }
                !matches!(
                    handle_key(&mut st, &snap, key(binding.key), 120),
                    Outcome::Inert
                )
            });
            assert!(
                live,
                "`{}` is on the key sheet but does nothing",
                binding.key
            );
        }
        for reserved in RESERVED {
            let (snap, mut st) = setup(&[(10, " --+ ")]);
            assert!(
                matches!(
                    handle_key(&mut st, &snap, key(reserved), 120),
                    Outcome::Inert
                ),
                "`{reserved}` must stay unbound"
            );
            assert!(crate::tui::keys::KEYS.iter().all(|b| b.key != *reserved));
        }
    }

    #[test]
    fn hunk_keys_land_on_the_first_changed_row() {
        let (snap, mut st) = setup(&[(10, "  + "), (40, " - ")]);
        assert!(matches!(
            handle_key(&mut st, &snap, key("]"), 120),
            Outcome::Redraw
        ));
        assert_eq!(
            st.cursor_id,
            Some((crate::engine::nav::Side::Deletions, 41))
        );
        assert!(matches!(
            handle_key(&mut st, &snap, key("["), 120),
            Outcome::Redraw
        ));
        assert_eq!(
            st.cursor_id,
            Some((crate::engine::nav::Side::Additions, 12))
        );
    }

    #[test]
    fn file_keys_and_refresh_become_engine_commands() {
        let (snap, mut st) = setup(&[(10, " + ")]);
        assert!(matches!(
            handle_key(&mut st, &snap, key("n"), 120),
            Outcome::Engine(Command::SelectNext)
        ));
        assert!(matches!(
            handle_key(&mut st, &snap, key("p"), 120),
            Outcome::Engine(Command::SelectPrev)
        ));
        assert!(matches!(
            handle_key(&mut st, &snap, key("r"), 120),
            Outcome::Engine(Command::Refresh)
        ));
        assert!(matches!(
            handle_key(&mut st, &snap, key("q"), 120),
            Outcome::Quit
        ));
    }

    #[test]
    fn split_is_refused_below_100_columns() {
        let (snap, mut st) = setup(&[(10, " + ")]);
        st.requested_mode = ViewMode::Unified;
        st.resize(120, st.body_height);
        st.reconcile(&snap);
        handle_key(&mut st, &snap, key("t"), 90);
        assert_eq!(st.mode, ViewMode::Unified);
        assert!(st
            .notice
            .as_ref()
            .map(|n| n.text.as_str())
            .unwrap_or("")
            .contains("100 columns"));
        assert_eq!(st.requested_mode, ViewMode::Unified);
    }

    #[test]
    fn scroll_right_stops_at_the_row_bound_and_becomes_inert() {
        let snap = crate::tui::state::tests::text_snapshot("r1", &"猫".repeat(70));
        for (mode, panel, limit) in [
            (ViewMode::Unified, FilesPanel::Hidden, 34),
            (ViewMode::Split, FilesPanel::Hidden, 89),
            (ViewMode::Split, FilesPanel::Shown, 98),
        ] {
            let mut st = ViewState::new(mode, panel, true);
            st.resize(120, 22);
            st.reconcile(&snap);
            while st.hscroll < limit {
                let previous = st.hscroll;
                assert_eq!(handle_key(&mut st, &snap, key("L"), 120), Outcome::Redraw);
                assert_eq!(st.hscroll, (previous + 8).min(limit));
            }
            assert_eq!(handle_key(&mut st, &snap, key("L"), 120), Outcome::Inert);
            assert_eq!(st.hscroll, limit);
            assert_eq!(handle_key(&mut st, &snap, key("H"), 120), Outcome::Redraw);
            assert_eq!(st.hscroll, limit - 8);
        }
    }

    #[test]
    fn toggling_while_narrow_changes_the_request_without_losing_the_cursor() {
        let (snap, mut st) = setup(&[(10, " --+ ")]);
        let cursor = st.cursor_id;
        st.resize(90, st.body_height);
        st.reconcile(&snap);
        assert_eq!(st.mode, ViewMode::Unified);
        assert_eq!(handle_key(&mut st, &snap, key("t"), 90), Outcome::Redraw);
        assert_eq!(st.requested_mode, ViewMode::Unified);
        assert_eq!(handle_key(&mut st, &snap, key("t"), 90), Outcome::Redraw);
        assert_eq!(st.requested_mode, ViewMode::Unified);
        assert!(st
            .notice
            .as_ref()
            .map(|n| n.text.as_str())
            .unwrap()
            .contains("100 columns"));
        st.resize(120, st.body_height);
        st.reconcile(&snap);
        assert_eq!(st.mode, ViewMode::Unified);
        assert_eq!(st.cursor_id, cursor);
        handle_key(&mut st, &snap, key("t"), 120);
        st.reconcile(&snap);
        st.resize(90, st.body_height);
        st.reconcile(&snap);
        st.resize(120, st.body_height);
        st.reconcile(&snap);
        assert_eq!(st.mode, ViewMode::Split);
        assert_eq!(st.cursor_id, cursor);
    }

    #[test]
    fn last_row_is_reachable_in_a_diff_longer_than_u16() {
        let kinds = "+".repeat(70_000);
        let (snap, mut st) = setup(&[(1, kinds.as_str())]);
        handle_key(&mut st, &snap, key("G"), 120);
        assert_eq!(st.cursor, Some(69_999));
        let rows = st.rows.as_ref().unwrap().rows.len();
        assert!(
            st.offset + usize::from(st.body_height) >= rows,
            "the last row is on screen"
        );
    }

    #[test]
    fn every_binding_is_reachable_on_the_key_sheet_in_a_short_terminal() {
        let (snap, mut st) = setup(&[(10, " + ")]);
        st.resize(120, body_height(&st, &snap, 12));
        handle_key(&mut st, &snap, key("?"), 120);
        let mut seen = String::new();
        for _ in 0..40 {
            seen.push_str(&render(&snap, &st, 120, 12).plain().join("\n"));
            if matches!(handle_key(&mut st, &snap, key("j"), 120), Outcome::Inert) {
                break;
            }
        }
        for binding in KEYS {
            assert!(
                seen.contains(binding.label),
                "`{}` never appears on the sheet at 12 rows",
                binding.label
            );
        }
        assert!(
            matches!(handle_key(&mut st, &snap, key("q"), 120), Outcome::Redraw),
            "q closes the sheet"
        );
        assert!(!st.help_open);
    }

    #[test]
    fn a_diff_without_targets_makes_movement_inert() {
        let (snap, mut st) = setup(&[]);
        for k in [
            "j", "k", "[", "]", "h", "l", "g", "G", "ctrl+d", "ctrl+u", "H", "L",
        ] {
            assert!(
                matches!(handle_key(&mut st, &snap, key(k), 120), Outcome::Inert),
                "`{k}`"
            );
        }
    }

    #[test]
    fn a_toolbar_click_acts_like_its_key_and_the_wheel_scrolls_three_rows() {
        let kinds = "+".repeat(200);
        let (mut snap, mut st) = setup(&[(1, kinds.as_str())]);
        snap.files = ["a.rs", "b.rs"]
            .into_iter()
            .map(|path| crate::git::ChangedFile {
                path: path.into(),
                status: crate::git::ChangedFileStatus::Modified,
                staged: false,
                insertions: Some(200),
                deletions: Some(0),
            })
            .collect();
        let r = render(&snap, &st, 120, 24);
        let hit = r
            .hits
            .iter()
            .find(|h| matches!(h.action, crate::tui::view::Action::NextFile))
            .unwrap();
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: hit.x0,
            row: hit.y,
            modifiers: KeyModifiers::NONE,
        };
        assert!(matches!(
            handle_mouse(&mut st, &snap, &r, click),
            Outcome::Engine(Command::SelectNext)
        ));
        let wheel = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 50,
            row: 10,
            modifiers: KeyModifiers::NONE,
        };
        assert!(matches!(
            handle_mouse(&mut st, &snap, &r, wheel),
            Outcome::Redraw
        ));
        assert_eq!(st.offset, 3);
        // the run loop's redraw preparation must not pull the viewport back to the cursor
        st.resize(120, body_height(&st, &snap, 24));
        st.reconcile(&snap);
        assert_eq!(st.offset, 3, "wheel scrolling was undone by the next frame");
        let shifted = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: hit.x0,
            row: hit.y,
            modifiers: KeyModifiers::SHIFT,
        };
        assert!(matches!(
            handle_mouse(&mut st, &snap, &r, shifted),
            Outcome::Inert
        ));
        let _ = DiffState::Idle;
    }

    #[test]
    fn movement_uses_display_order_and_half_pages_snap_to_the_centre() {
        let (mut snap, mut st) = setup(&[(10, " --+ ")]);
        if let DiffState::Ready(diff) = &mut snap.diff {
            std::sync::Arc::make_mut(diff).file_diff.hunks[0].lines[0].content = "x".repeat(200);
        }
        st.requested_mode = ViewMode::Unified;
        st.resize(120, st.body_height);
        st.reconcile(&snap);
        assert_eq!(st.cursor, Some(1));
        assert_eq!(handle_key(&mut st, &snap, key("j"), 120), Outcome::Redraw);
        assert_eq!(st.cursor, Some(3));
        handle_key(&mut st, &snap, key("j"), 120);
        assert_eq!(st.cursor, Some(2));
        assert_eq!(handle_key(&mut st, &snap, key("h"), 120), Outcome::Inert);
        handle_key(&mut st, &snap, key("g"), 120);
        assert_eq!(st.cursor, Some(0));
        for k in ["k", "[", "H"] {
            assert_eq!(handle_key(&mut st, &snap, key(k), 120), Outcome::Inert);
        }
        handle_key(&mut st, &snap, key("L"), 120);
        assert_eq!(st.hscroll, 8);
        handle_key(&mut st, &snap, key("H"), 120);
        assert_eq!(st.hscroll, 0);

        let (snap, mut st) = setup(&[(1, &"+".repeat(200))]);
        for (k, expected_offset) in [
            ("ctrl+d", 11),
            ("ctrl+d", 22),
            ("ctrl+u", 11),
            ("ctrl+u", 0),
        ] {
            assert_eq!(handle_key(&mut st, &snap, key(k), 120), Outcome::Redraw);
            assert_eq!(st.offset, expected_offset);
            assert_eq!(
                st.rows.as_ref().unwrap().row_of_target[st.cursor.unwrap()],
                st.offset + 11
            );
        }
        assert_eq!(
            handle_key(&mut st, &snap, key("ctrl+u"), 120),
            Outcome::Inert
        );
        handle_key(&mut st, &snap, key("G"), 120);
        assert_eq!(st.cursor, Some(199));
        for k in ["j", "]", "ctrl+d"] {
            assert_eq!(handle_key(&mut st, &snap, key(k), 120), Outcome::Inert);
        }
    }

    #[test]
    fn movement_needs_a_ready_diff_and_a_cursor() {
        for diff in [
            DiffState::Idle,
            DiffState::Loading,
            DiffState::Failed("failed".into()),
        ] {
            let (mut snap, mut st) = setup(&[(1, "+")]);
            snap.diff = diff;
            st.reconcile(&snap);
            for k in [
                "j", "k", "[", "]", "h", "l", "g", "G", "ctrl+d", "ctrl+u", "H", "L",
            ] {
                assert_eq!(handle_key(&mut st, &snap, key(k), 120), Outcome::Inert);
            }
        }
        let (snap, mut st) = setup(&[(1, "+")]);
        st.cursor = None;
        assert_eq!(handle_key(&mut st, &snap, key("L"), 120), Outcome::Inert);
        assert_eq!(handle_key(&mut st, &snap, key("g"), 120), Outcome::Inert);
    }

    #[test]
    fn help_is_modal_and_scrolls_with_keys_and_wheel_to_both_ends() {
        let (snap, mut st) = setup(&[(1, &"+".repeat(80))]);
        st.resize(120, body_height(&st, &snap, 12));
        let background = render(&snap, &st, 40, 12);
        let wheel = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 20,
            row: 5,
            modifiers: KeyModifiers::NONE,
        };
        handle_key(&mut st, &snap, key("?"), 40);
        assert_eq!(handle_key(&mut st, &snap, key("k"), 40), Outcome::Inert);
        for binding in KEYS
            .iter()
            .filter(|b| !["j", "k", "?", "q"].contains(&b.key))
        {
            assert_eq!(
                handle_key(&mut st, &snap, key(binding.key), 40),
                Outcome::Inert
            );
        }
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 0,
            ..wheel
        };
        assert_eq!(
            handle_mouse(&mut st, &snap, &background, click),
            Outcome::Inert
        );
        assert_eq!(
            handle_mouse(&mut st, &snap, &background, wheel),
            Outcome::Redraw
        );
        assert_eq!((st.help_offset, st.offset, st.cursor), (3, 0, Some(0)));
        while handle_mouse(&mut st, &snap, &background, wheel) != Outcome::Inert {}
        assert_eq!(
            st.help_offset,
            dialog::line_count(&keys::help_panel(false), 40) - 6
        );
        let sheet = render(&snap, &st, 40, 12);
        assert!(sheet.plain().iter().any(|line| line.contains("quit")));
        assert!(sheet.hits.is_empty());
        let up = MouseEvent {
            kind: MouseEventKind::ScrollUp,
            ..wheel
        };
        assert_eq!(
            handle_mouse(&mut st, &snap, &background, up),
            Outcome::Redraw
        );
        assert_eq!(
            st.help_offset,
            dialog::line_count(&keys::help_panel(false), 40) - 9
        );
        while handle_key(&mut st, &snap, key("k"), 40) != Outcome::Inert {}
        assert_eq!(st.help_offset, 0);
        for close in [
            key("?"),
            key("q"),
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        ] {
            assert_eq!(handle_key(&mut st, &snap, close, 40), Outcome::Redraw);
            assert!(!st.help_open);
            assert_eq!(st.help_offset, 0);
            handle_key(&mut st, &snap, key("?"), 40);
        }
    }

    #[test]
    fn panels_mouse_and_notices_change_only_for_handled_keys() {
        let (snap, mut st) = setup(&[(1, "+")]);
        for (k, expected) in [
            ("e", FilesPanel::Shown),
            ("E", FilesPanel::Pinned),
            ("E", FilesPanel::Shown),
            ("e", FilesPanel::Hidden),
            ("E", FilesPanel::Pinned),
            ("e", FilesPanel::Hidden),
        ] {
            assert_eq!(handle_key(&mut st, &snap, key(k), 120), Outcome::Redraw);
            assert_eq!(st.files_panel, expected);
        }
        handle_key(&mut st, &snap, key("m"), 120);
        assert!(!st.mouse_requested);
        handle_key(&mut st, &snap, key("m"), 120);
        assert!(st.mouse_requested);
        handle_key(&mut st, &snap, key("t"), 120);
        assert_eq!(st.mode, ViewMode::Unified);
        st.reconcile(&snap);
        handle_key(&mut st, &snap, key("t"), 99);
        assert!(st.notice.is_some());
        assert_eq!(handle_key(&mut st, &snap, key("/"), 99), Outcome::Inert);
        assert!(st.notice.is_some());
        assert_eq!(handle_key(&mut st, &snap, key("k"), 99), Outcome::Redraw);
        assert!(st.notice.is_none());
        handle_key(&mut st, &snap, key("t"), 100);
        assert_eq!(st.mode, ViewMode::Split);
    }

    #[test]
    fn mouse_routes_toolbar_files_and_diff_rows_and_ignores_other_events() {
        use crate::git::{ChangedFile, ChangedFileStatus};
        let (mut snap, mut st) = setup(&[(1, " --+ "), (20, "+")]);
        snap.files = [false, true]
            .into_iter()
            .map(|staged| ChangedFile {
                path: "a.rs".into(),
                status: ChangedFileStatus::Modified,
                staged,
                insertions: Some(2),
                deletions: Some(2),
            })
            .collect();
        st.files_panel = FilesPanel::Shown;
        let rendered = render(&snap, &st, 120, 24);
        for (action, k) in [
            (Action::PrevFile, "p"),
            (Action::NextFile, "n"),
            (Action::PrevHunk, "["),
            (Action::NextHunk, "]"),
            (Action::ToggleView, "t"),
            (Action::ToggleFiles, "e"),
            (Action::Refresh, "r"),
        ] {
            let hit = rendered
                .hits
                .iter()
                .find(|hit| hit.action == action)
                .unwrap();
            let click = MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: hit.x0,
                row: hit.y,
                modifiers: KeyModifiers::NONE,
            };
            let (_, mut keyboard) = setup(&[(1, " --+ "), (20, "+")]);
            let (_, mut mouse) = setup(&[(1, " --+ "), (20, "+")]);
            assert_eq!(
                handle_mouse(&mut mouse, &snap, &rendered, click),
                handle_key(&mut keyboard, &snap, key(k), 120)
            );
            assert_eq!(
                (mouse.cursor, mouse.mode, mouse.files_panel),
                (keyboard.cursor, keyboard.mode, keyboard.files_panel)
            );
        }
        let hit = rendered
            .hits
            .iter()
            .find(|hit| hit.action == Action::SelectFile(1))
            .unwrap();
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: hit.x0,
            row: hit.y,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            handle_mouse(&mut st, &snap, &rendered, click),
            Outcome::Engine(Command::Select(FileKey {
                path: "a.rs".into(),
                staged: true,
                untracked: false,
            }))
        );
        assert_eq!(
            handle_mouse(
                &mut st,
                &snap,
                &rendered,
                MouseEvent {
                    kind: MouseEventKind::Moved,
                    ..click
                }
            ),
            Outcome::Redraw
        );
        assert_eq!(
            handle_mouse(
                &mut st,
                &snap,
                &rendered,
                MouseEvent {
                    kind: MouseEventKind::Moved,
                    ..click
                }
            ),
            Outcome::Inert
        );
        for kind in [
            MouseEventKind::Down(MouseButton::Right),
            MouseEventKind::Down(MouseButton::Middle),
            MouseEventKind::Up(MouseButton::Left),
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::ScrollLeft,
            MouseEventKind::ScrollRight,
        ] {
            assert_eq!(
                handle_mouse(&mut st, &snap, &rendered, MouseEvent { kind, ..click }),
                Outcome::Inert
            );
        }
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::ScrollDown,
        ] {
            assert_eq!(
                handle_mouse(
                    &mut st,
                    &snap,
                    &rendered,
                    MouseEvent {
                        kind,
                        modifiers: KeyModifiers::CONTROL,
                        ..click
                    }
                ),
                Outcome::Inert
            );
        }
        let row = st.rows.as_ref().unwrap().row_of_target[0];
        let hit = rendered
            .hits
            .iter()
            .find(|hit| hit.action == Action::CursorToRow(row))
            .unwrap();
        let click = MouseEvent {
            column: hit.x0,
            row: hit.y,
            ..click
        };
        assert_eq!(
            handle_mouse(&mut st, &snap, &rendered, click),
            Outcome::Redraw
        );
        assert_eq!(st.cursor, Some(0));
        assert_eq!(
            handle_mouse(&mut st, &snap, &rendered, click),
            Outcome::Inert
        );
        let header = rendered
            .hits
            .iter()
            .find(|hit| hit.action == Action::CursorToRow(0))
            .unwrap();
        assert_eq!(
            handle_mouse(
                &mut st,
                &snap,
                &rendered,
                MouseEvent {
                    column: header.x0,
                    row: header.y,
                    ..click
                }
            ),
            Outcome::Inert
        );

        let (snap, mut st) = setup(&[(1, &"+".repeat(80))]);
        let wheel = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            ..click
        };
        for _ in 0..40 {
            handle_mouse(&mut st, &snap, &rendered, wheel);
        }
        assert_eq!(
            st.offset,
            st.rows.as_ref().unwrap().rows.len() - usize::from(st.body_height)
        );
        assert_eq!(
            handle_mouse(&mut st, &snap, &rendered, wheel),
            Outcome::Inert
        );
        let wheel = MouseEvent {
            kind: MouseEventKind::ScrollUp,
            ..wheel
        };
        for _ in 0..40 {
            handle_mouse(&mut st, &snap, &rendered, wheel);
        }
        assert_eq!(st.offset, 0);
        assert_eq!(
            handle_mouse(&mut st, &snap, &rendered, wheel),
            Outcome::Inert
        );
    }

    #[test]
    fn popup_escape_closes_only_the_topmost_view() {
        let escape = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        for popup in [false, true] {
            let (snap, mut st) = setup(&[(1, "+")]);
            st.popup = popup;
            assert_eq!(
                handle_key(&mut st, &snap, escape, 120),
                if popup { Outcome::Quit } else { Outcome::Inert }
            );
            assert_eq!(
                handle_key(
                    &mut st,
                    &snap,
                    KeyEvent::new(KeyCode::Esc, KeyModifiers::ALT),
                    120
                ),
                Outcome::Inert
            );
            st.help_open = true;
            assert_eq!(handle_key(&mut st, &snap, escape, 120), Outcome::Redraw);
            assert!(!st.help_open);
        }
    }

    #[test]
    fn hover_redraws_only_between_targets_and_mouse_off_clears_it() {
        let (snap, mut st) = setup(&[(1, " --+ "), (20, "+")]);
        st.files_panel = FilesPanel::Shown;
        let rendered = render(&snap, &st, 120, 24);
        let hit = rendered
            .hits
            .iter()
            .find(|h| h.action == Action::ToggleView)
            .unwrap();
        let moved = MouseEvent {
            kind: MouseEventKind::Moved,
            column: hit.x0,
            row: hit.y,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            handle_mouse(&mut st, &snap, &rendered, moved),
            Outcome::Redraw
        );
        assert_eq!(st.hover, Some((hit.x0, hit.y)));
        assert_eq!(
            handle_mouse(
                &mut st,
                &snap,
                &rendered,
                MouseEvent {
                    column: hit.x1 - 1,
                    ..moved
                }
            ),
            Outcome::Inert
        );
        let other = rendered
            .hits
            .iter()
            .find(|h| h.action == Action::Refresh)
            .unwrap();
        assert_eq!(
            handle_mouse(
                &mut st,
                &snap,
                &rendered,
                MouseEvent {
                    column: other.x0,
                    ..moved
                }
            ),
            Outcome::Redraw
        );
        assert_eq!(
            handle_mouse(
                &mut st,
                &snap,
                &rendered,
                MouseEvent {
                    column: 60,
                    row: 2,
                    ..moved
                }
            ),
            Outcome::Redraw
        );
        for row in 2..10 {
            assert_eq!(
                handle_mouse(
                    &mut st,
                    &snap,
                    &rendered,
                    MouseEvent {
                        column: 61,
                        row,
                        ..moved
                    }
                ),
                Outcome::Inert
            );
        }
        st.hover = Some((hit.x0, 0));
        handle_key(&mut st, &snap, key("m"), 120);
        assert_eq!(st.hover, None);
        assert_eq!(
            handle_mouse(&mut st, &snap, &rendered, moved),
            Outcome::Inert
        );
        assert_eq!(st.hover, None);
    }

    #[test]
    fn mouse_capture_off_clears_hover() {
        let (snap, mut st) = setup(&[(1, "+")]);
        st.hover = Some((1, 0));
        assert_eq!(handle_key(&mut st, &snap, key("m"), 120), Outcome::Redraw);
        assert_eq!(st.hover, None);
    }

    #[test]
    fn b_switches_scope_or_says_there_is_no_base() {
        use crate::engine::{Base, BaseSource, Scope, NO_BASE_NOTICE};
        let (mut snap, mut st) = setup(&[(1, "+")]);
        assert_eq!(handle_key(&mut st, &snap, key("b"), 120), Outcome::Redraw);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some(NO_BASE_NOTICE)
        );
        snap.base = Some(Base {
            requested: "refs/heads/main".into(),
            commit: "0".repeat(40),
            merge_base: None,
            source: BaseSource::Default,
        });
        assert_eq!(
            handle_key(&mut st, &snap, key("b"), 120),
            Outcome::Engine(Command::SetScope(Scope::Branch))
        );
        assert!(st.notice.is_none(), "any handled key clears the notice");
        snap.scope = Scope::Branch;
        assert_eq!(
            handle_key(&mut st, &snap, key("b"), 120),
            Outcome::Engine(Command::SetScope(Scope::Worktree))
        );
        for reserved in RESERVED {
            assert!(
                matches!(
                    handle_key(&mut st, &snap, key(reserved), 120),
                    Outcome::Inert
                ),
                "{reserved} in branch scope"
            );
        }
    }

    #[test]
    fn reopening_the_picker_does_not_mistake_an_earlier_pick_reply_for_its_own() {
        for mouse in [false, true] {
            let (mut snap, mut st) = setup(&[(1, "+")]);
            handle_key(&mut st, &snap, key("B"), 120);
            let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(
                handle_key(&mut st, &snap, enter, 120),
                Outcome::Engine(Command::SetBase(None))
            );
            handle_key(
                &mut st,
                &snap,
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                120,
            );
            handle_key(&mut st, &snap, key("B"), 120);
            let picker = st.picker.as_mut().unwrap();
            picker.input = "second".into();
            picker.retarget(&snap);
            let outcome = if mouse {
                let rendered = render(&snap, &st, 120, 24);
                let hit = rendered
                    .hits
                    .iter()
                    .find(|hit| hit.action == Action::PickRow(1))
                    .unwrap();
                handle_mouse(
                    &mut st,
                    &snap,
                    &rendered,
                    MouseEvent {
                        kind: MouseEventKind::Down(MouseButton::Left),
                        column: hit.x0,
                        row: hit.y,
                        modifiers: KeyModifiers::NONE,
                    },
                )
            } else {
                handle_key(&mut st, &snap, enter, 120)
            };
            assert_eq!(
                outcome,
                Outcome::Engine(Command::SetBase(Some("second".into())))
            );
            snap.pick_seq = 1;
            st.observe(&snap);
            assert!(
                st.picker.as_ref().is_some_and(|p| p.pending.is_some()),
                "the first pick's success must not close the second picker"
            );
            snap.pick_seq = 2;
            snap.pick_error = Some("not a commit: second".into());
            st.observe(&snap);
            let picker = st.picker.as_ref().unwrap();
            assert!(picker.pending.is_none());
            assert_eq!(picker.error.as_deref(), Some("not a commit: second"));
            assert_eq!(
                handle_key(&mut st, &snap, enter, 120),
                Outcome::Engine(Command::SetBase(Some("second".into())))
            );
            snap.pick_seq = 3;
            snap.pick_error = None;
            st.observe(&snap);
            assert!(st.picker.is_none());
        }
    }

    #[test]
    fn unbound_modifiers_do_not_edit_submit_or_close_the_picker() {
        let (snap, mut st) = setup(&[(1, "+")]);
        handle_key(&mut st, &snap, key("B"), 120);
        st.picker.as_mut().unwrap().input = "kept".into();
        for modifiers in [
            KeyModifiers::ALT,
            KeyModifiers::SUPER,
            KeyModifiers::HYPER,
            KeyModifiers::META,
            KeyModifiers::CONTROL | KeyModifiers::ALT,
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ] {
            for code in [
                KeyCode::Char('n'),
                KeyCode::Char('p'),
                KeyCode::Enter,
                KeyCode::Esc,
                KeyCode::Backspace,
            ] {
                assert_eq!(
                    handle_key(&mut st, &snap, KeyEvent::new(code, modifiers), 120),
                    Outcome::Inert,
                    "{code:?} {modifiers:?}"
                );
            }
        }
        for code in [
            KeyCode::Down,
            KeyCode::Up,
            KeyCode::Enter,
            KeyCode::Esc,
            KeyCode::Backspace,
        ] {
            assert_eq!(
                handle_key(
                    &mut st,
                    &snap,
                    KeyEvent::new(code, KeyModifiers::SHIFT),
                    120
                ),
                Outcome::Inert,
                "shift {code:?}"
            );
        }
        assert_eq!(st.picker.as_ref().unwrap().input, "kept");
        assert!(st.picker.as_ref().unwrap().pending.is_none());
        assert_eq!(
            handle_key(
                &mut st,
                &snap,
                KeyEvent::new(KeyCode::Char('Z'), KeyModifiers::SHIFT),
                120
            ),
            Outcome::Redraw
        );
        assert_eq!(st.picker.as_ref().unwrap().input, "keptZ");
    }

    #[test]
    fn each_picker_opening_requests_its_own_ref_token() {
        let (mut snap, mut st) = setup(&[(1, "+")]);
        assert_eq!(
            handle_key(&mut st, &snap, key("B"), 120),
            Outcome::Engine(Command::LoadRefs(1))
        );
        handle_key(
            &mut st,
            &snap,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            120,
        );
        assert_eq!(
            handle_key(&mut st, &snap, key("B"), 120),
            Outcome::Engine(Command::LoadRefs(2))
        );
        snap.refs = Some(std::sync::Arc::new(vec!["refs/heads/main".into()]));
        snap.refs_seq = 1;
        st.observe(&snap);
        assert!(st.picker.as_ref().unwrap().refs(&snap).is_empty());
        assert_eq!(
            st.picker.as_ref().unwrap().panel(&snap, 60, 12).footer,
            "loading refs…"
        );
        snap.refs_seq = 2;
        st.observe(&snap);
        assert_eq!(st.picker.as_ref().unwrap().refs(&snap), ["refs/heads/main"]);
    }

    #[test]
    fn capital_b_opens_the_picker_and_picker_keys_edit_move_pick_and_close() {
        use crate::engine::{RepoState, Scope};
        use crate::tui::picker::PickerRow;
        let (mut snap, mut st) = setup(&[(1, "+")]);
        snap.refs = Some(std::sync::Arc::new(vec![
            "refs/heads/main".into(),
            "refs/heads/feat".into(),
            "refs/tags/v1".into(),
        ]));
        snap.default_base = Some("refs/heads/main".into());
        snap.repo = RepoState::Repo {
            toplevel: "/r".into(),
            branch: Some("feat".into()),
            worktree: None,
        };
        assert_eq!(
            handle_key(&mut st, &snap, key("B"), 120),
            Outcome::Engine(Command::LoadRefs(1))
        );
        assert!(st.picker.is_some());
        assert!(
            st.picker.as_ref().unwrap().refs(&snap).is_empty(),
            "a list from before this opening is not shown"
        );
        snap.refs_seq += 1; // the engine answers this opening's LoadRefs
        st.observe(&snap);
        assert_eq!(st.picker.as_ref().unwrap().refs(&snap).len(), 3);
        // Body keys are inert while the picker is open; `j` and `?` are text.
        assert_eq!(handle_key(&mut st, &snap, key("j"), 120), Outcome::Redraw);
        assert_eq!(st.picker.as_ref().unwrap().input, "j");
        assert_eq!(
            handle_key(
                &mut st,
                &snap,
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
                120
            ),
            Outcome::Redraw
        );
        assert_eq!(
            handle_key(
                &mut st,
                &snap,
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
                120
            ),
            Outcome::Inert
        );
        assert_eq!(handle_key(&mut st, &snap, key("?"), 120), Outcome::Redraw);
        assert!(!st.help_open);
        assert_eq!(
            handle_key(
                &mut st,
                &snap,
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
                120
            ),
            Outcome::Redraw
        );
        assert_eq!(
            handle_key(
                &mut st,
                &snap,
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
                120
            ),
            Outcome::Redraw
        );
        assert_eq!(
            handle_key(&mut st, &snap, key("ctrl+n"), 120),
            Outcome::Redraw
        );
        assert_eq!(
            handle_key(&mut st, &snap, key("ctrl+p"), 120),
            Outcome::Redraw
        );
        assert_eq!(st.picker.as_ref().unwrap().cursor, 1);
        let rows = st.picker.as_ref().unwrap().rows(&snap);
        assert!(
            matches!(&rows[1], PickerRow::Ref { qualified, .. } if qualified == "refs/heads/main")
        );
        assert_eq!(
            handle_key(
                &mut st,
                &snap,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                120
            ),
            Outcome::Engine(Command::SetBase(Some("refs/heads/main".into())))
        );
        assert_eq!(st.picker.as_ref().unwrap().pending, Some(snap.pick_seq));
        assert_eq!(
            handle_key(
                &mut st,
                &snap,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                120
            ),
            Outcome::Inert,
            "one pick at a time"
        );
        // The engine answers: a failure keeps the picker open with the error; a success closes it.
        snap.pick_seq += 1;
        snap.pick_error = Some("not a commit: refs/heads/main".into());
        st.observe(&snap);
        assert_eq!(
            st.picker.as_ref().unwrap().error.as_deref(),
            Some("not a commit: refs/heads/main")
        );
        assert_eq!(
            handle_key(
                &mut st,
                &snap,
                KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
                120
            ),
            Outcome::Redraw
        );
        assert_eq!(
            handle_key(
                &mut st,
                &snap,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                120
            ),
            Outcome::Engine(Command::SetBase(None)),
            "the reset row"
        );
        snap.pick_seq += 1;
        snap.pick_error = None;
        snap.scope = Scope::Branch;
        snap.base_error = Some("pick not remembered: no state directory".into());
        st.observe(&snap);
        assert!(st.picker.is_none());
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some("pick not remembered: no state directory"),
            "the warning survives the picker closing"
        );
        // Esc closes without a change; Ctrl+C still quits; the popup's Esc stays with the picker.
        handle_key(&mut st, &snap, key("B"), 120);
        st.popup = true;
        assert_eq!(
            handle_key(
                &mut st,
                &snap,
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                120
            ),
            Outcome::Redraw
        );
        assert!(st.picker.is_none());
        assert_eq!(
            handle_key(
                &mut st,
                &snap,
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                120
            ),
            Outcome::Quit
        );
        handle_key(&mut st, &snap, key("B"), 120);
        assert_eq!(
            handle_key(&mut st, &snap, key("ctrl+c"), 120),
            Outcome::Quit
        );
    }

    #[test]
    fn a_click_on_a_picker_row_picks_it_and_the_wheel_moves_the_cursor() {
        use crate::engine::RepoState;
        let (mut snap, mut st) = setup(&[(1, "+")]);
        snap.refs = Some(std::sync::Arc::new(
            (0..30).map(|i| format!("refs/heads/b{i:02}")).collect(),
        ));
        snap.default_base = Some("refs/heads/b00".into());
        snap.repo = RepoState::Repo {
            toplevel: "/r".into(),
            branch: Some("b00".into()),
            worktree: None,
        };
        handle_key(&mut st, &snap, key("B"), 120);
        snap.refs_seq += 1;
        st.observe(&snap);
        let rendered = render(&snap, &st, 120, 24);
        let hit = rendered
            .hits
            .iter()
            .find(|h| matches!(h.action, Action::PickRow(2)))
            .expect("row hit");
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: hit.x0,
            row: hit.y,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            handle_mouse(&mut st, &snap, &rendered, click),
            Outcome::Engine(Command::SetBase(Some("refs/heads/b01".into())))
        );
        st.picker.as_mut().unwrap().pending = None;
        let wheel = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 10,
            row: 10,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            handle_mouse(&mut st, &snap, &rendered, wheel),
            Outcome::Redraw
        );
        assert_eq!(st.picker.as_ref().unwrap().cursor, 5);
        let toolbar = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            handle_mouse(&mut st, &snap, &rendered, toolbar),
            Outcome::Inert,
            "toolbar hits are inert under the picker"
        );
    }
    #[test]
    fn capital_m_marks_the_drawn_head_and_refuses_without_one() {
        let (mut snap, mut st) = setup(&[(1, "+")]);
        assert_eq!(handle_key(&mut st, &snap, key("M"), 120), Outcome::Redraw);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some("nothing to mark: no commit yet")
        );
        st.drawn_head = Some("c".repeat(40));
        snap.head = Some("d".repeat(40));
        for scope in [crate::engine::Scope::Worktree, crate::engine::Scope::Branch] {
            snap.scope = scope;
            assert_eq!(
                handle_key(&mut st, &snap, key("M"), 120),
                Outcome::Engine(Command::MarkReviewed("c".repeat(40)))
            );
            assert_eq!(snap.scope, scope);
        }
    }

    #[test]
    fn plain_answers_displaced_in_one_batch_return_after_a_key() {
        let (mut snap, mut st) = setup(&[(1, "+")]);
        snap.mark_seq = 1;
        snap.mark = Some(crate::engine::Mark {
            commit: "a".repeat(40),
            at: 1,
            state: crate::engine::MarkState::Current,
            classified_at: None,
        });
        st.observe(&snap);
        assert_eq!(
            st.notice.as_ref().unwrap().text,
            "marked aaaaaaa as reviewed"
        );
        snap.send_seq = 1;
        snap.send_outcome = Some(dispatch::SendOutcome {
            kind: dispatch::SendKind::Feedback,
            items: 1,
            to: crate::engine::Destination::Pane {
                pane: "w4:p2".into(),
                agent: "codex".into(),
                session: None,
            },
            unconfirmed: false,
            copy: None,
        });
        st.observe(&snap);
        assert_eq!(
            st.notice.as_ref().unwrap().text,
            "sent 1 item to codex · w4:p2"
        );
        handle_key(&mut st, &snap, key("t"), 120);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some("marked aaaaaaa as reviewed")
        );
        handle_key(&mut st, &snap, key("t"), 120);
        assert!(st.notice.is_none());
    }

    #[test]
    fn a_plain_key_notice_returns_after_a_mark_answer() {
        let (mut snap, mut st) = setup(&[]);
        handle_key(&mut st, &snap, key("s"), 120);
        assert_eq!(
            st.notice.as_ref().unwrap().text,
            crate::engine::actions::NOTICE_NO_HUNK
        );
        snap.mark_seq = 1;
        snap.mark_error = Some("nothing to mark".into());
        st.observe(&snap);
        assert_eq!(st.notice.as_ref().unwrap().text, "nothing to mark");
        handle_key(&mut st, &snap, key("t"), 120);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some(crate::engine::actions::NOTICE_NO_HUNK)
        );
        handle_key(&mut st, &snap, key("t"), 120);
        assert!(st.notice.is_none());
    }

    #[test]
    fn a_plain_notice_holds_back_rewrite_and_base_warnings() {
        let (mut snap, mut st) = setup(&[]);
        handle_key(&mut st, &snap, key("s"), 120);
        snap.head_seen = Some("b".repeat(40));
        snap.mark = Some(crate::engine::Mark {
            commit: "a".repeat(40),
            at: 1,
            state: crate::engine::MarkState::Rewritten,
            classified_at: snap.head_seen.clone(),
        });
        snap.base_error = Some("base failed".into());
        st.observe(&snap);
        assert_eq!(
            st.notice.as_ref().unwrap().text,
            crate::engine::actions::NOTICE_NO_HUNK
        );
        handle_key(&mut st, &snap, key("t"), 120);
        assert!(st
            .notice
            .as_ref()
            .unwrap()
            .text
            .contains("no longer on this branch"));
        handle_key(&mut st, &snap, key("t"), 120);
        assert_eq!(st.notice.as_ref().unwrap().text, "base failed");
        handle_key(&mut st, &snap, key("t"), 120);
        assert!(st.notice.is_none());
    }

    #[test]
    fn acknowledging_an_answer_shows_the_warning_that_waited_behind_it() {
        use crate::engine::{Mark, MarkState};
        let (mut snap, mut st) = setup(&[(1, "+")]);
        snap.head_seen = Some("h1".repeat(20));
        // One snapshot carries both the unwritable mark and its rewritten classification.
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
            st.notice.as_ref().map(|n| n.text.clone()).as_deref(),
            Some("mark not remembered: no state directory")
        );

        // A settled repository publishes no further snapshot, so the body key that clears the
        // answer is the only chance the warning gets.
        handle_key(&mut st, &snap, key("t"), 120);
        let notice = st.notice.as_ref().expect("the warning follows the answer");
        assert!(notice.urgent);
        assert!(notice.text.contains("no longer on this branch"));

        // And the next body key clears that one, leaving nothing behind.
        handle_key(&mut st, &snap, key("t"), 120);
        assert!(st.notice.is_none());
    }

    #[test]
    fn a_mark_answer_becomes_a_notice_even_behind_a_modal() {
        let (mut snap, mut st) = setup(&[(1, "+")]);
        st.help_open = true;
        snap.mark_seq = 1;
        snap.mark_error = Some("not a commit: abcdef1".into());
        st.observe(&snap);
        assert_eq!(
            st.notice.as_ref().map(|n| (n.text.clone(), n.urgent)),
            Some(("not a commit: abcdef1".to_string(), true))
        );
        // Modal keys preserve the answer until the next body key.
        handle_key(&mut st, &snap, key("j"), 120);
        assert!(
            st.notice.is_some(),
            "the key sheet's own key does not clear it"
        );
        handle_key(
            &mut st,
            &snap,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            120,
        );
        assert!(
            st.notice.is_some(),
            "closing the sheet does not clear an urgent notice"
        );
        handle_key(&mut st, &snap, key("t"), 120);
        assert!(st.notice.is_none());

        // The same holds behind the picker, and for a mark that was taken but not written.
        handle_key(&mut st, &snap, key("B"), 120);
        assert_eq!(
            handle_key(
                &mut st,
                &snap,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                120
            ),
            Outcome::Engine(Command::SetBase(None))
        );
        snap.mark_seq = 2;
        snap.mark_error = Some("mark not remembered: no state directory".into());
        snap.mark = Some(crate::engine::Mark {
            commit: "d".repeat(40),
            at: 1,
            state: crate::engine::MarkState::Current,
            classified_at: None,
        });
        st.observe(&snap);
        let picker = st
            .picker
            .as_ref()
            .expect("the mark answer leaves the pick pending");
        assert_eq!(picker.pending, Some(snap.pick_seq));
        assert!(picker.error.is_none());
        handle_key(&mut st, &snap, key("M"), 120);
        assert!(st.notice.as_ref().unwrap().urgent);
        handle_key(
            &mut st,
            &snap,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            120,
        );
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some("mark not remembered: no state directory")
        );
        // The same error answered twice shows twice.
        handle_key(&mut st, &snap, key("t"), 120);
        assert!(st.notice.is_none());
        snap.mark_seq = 3;
        st.observe(&snap);
        assert!(st.notice.is_some());

        handle_key(&mut st, &snap, key("t"), 120);
        snap.mark_seq = 4;
        snap.mark_error = None;
        st.observe(&snap);
        let notice = st.notice.as_ref().unwrap();
        assert_eq!(notice.text, "marked ddddddd as reviewed");
        assert!(!notice.urgent);
    }

    use crate::engine::actions::{
        NOTICE_CHANGED, NOTICE_CUT, NOTICE_NO_HUNK, NOTICE_RUNNING, NOTICE_SCOPE,
    };
    use crate::engine::{ActionKind, Scope};

    fn press_y(st: &mut ViewState, snap: &crate::engine::Snapshot) -> Outcome {
        if let Some(c) = st.confirm.as_mut() {
            c.drawn = true;
        }
        handle_key(st, snap, key("y"), 120)
    }

    fn arc_of(snap: &crate::engine::Snapshot) -> std::sync::Arc<crate::engine::LoadedDiff> {
        match &snap.diff {
            DiffState::Ready(d) => d.clone(),
            _ => unreachable!(),
        }
    }

    #[test]
    fn s_d_and_capital_d_open_their_boxes_and_y_sends_the_act() {
        for (k, kind, hunk, title) in [
            ("s", ActionKind::Stage, Some(0), "Stage hunk?"),
            ("d", ActionKind::Discard, Some(0), "Discard hunk?"),
            ("D", ActionKind::DiscardFile, None, "Discard file?"),
        ] {
            let (snap, mut st) = setup(&[(10, " --+ "), (40, "+")]);
            assert_eq!(handle_key(&mut st, &snap, key(k), 120), Outcome::Redraw);
            let confirm = st.confirm.as_ref().expect("a box");
            assert_eq!(confirm.title, title);
            assert_eq!((confirm.action.kind, confirm.action.hunk), (kind, hunk));
            let diff = arc_of(&snap);
            let outcome = press_y(&mut st, &snap);
            assert_eq!(
                outcome,
                Outcome::Engine(Command::Act(crate::engine::Action { kind, diff, hunk }))
            );
            assert!(st.confirm.is_none());
            assert!(st.action_running(&snap));
        }
        // On the staged row `s` is an unstage and the hunk follows the cursor.
        let (mut snap, mut st) = setup(&[(10, " --+ "), (40, "+")]);
        if let DiffState::Ready(d) = &mut snap.diff {
            std::sync::Arc::make_mut(d).key.staged = true;
        }
        st.reconcile(&snap);
        handle_key(&mut st, &snap, key("]"), 120);
        handle_key(&mut st, &snap, key("s"), 120);
        let confirm = st.confirm.as_ref().unwrap();
        assert_eq!(
            (confirm.title, confirm.action.hunk),
            ("Unstage hunk?", Some(1))
        );
    }

    #[test]
    fn n_closes_the_box_and_every_other_key_and_mouse_is_inert_in_it() {
        let (snap, mut st) = setup(&[(1, "+")]);
        handle_key(&mut st, &snap, key("d"), 120);
        for k in ["j", "k", "q", "s", "d", "D", "M", "b", "?", "Y", "N"] {
            assert_eq!(
                handle_key(&mut st, &snap, key(k), 120),
                Outcome::Inert,
                "{k}"
            );
            assert!(st.confirm.is_some(), "{k} closed the box");
        }
        assert_eq!(
            handle_key(
                &mut st,
                &snap,
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                120
            ),
            Outcome::Inert
        );
        let rendered = view::render(&snap, &st, 120, 24);
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            handle_mouse(&mut st, &snap, &rendered, click),
            Outcome::Inert
        );
        assert_eq!(
            handle_key(&mut st, &snap, key("ctrl+c"), 120),
            Outcome::Quit
        );
        assert_eq!(handle_key(&mut st, &snap, key("n"), 120), Outcome::Redraw);
        assert!(st.confirm.is_none() && st.pending_action.is_none());
    }

    #[test]
    fn y_is_inert_until_the_box_was_drawn() {
        let (snap, mut st) = setup(&[(1, "+")]);
        handle_key(&mut st, &snap, key("s"), 120);
        assert_eq!(handle_key(&mut st, &snap, key("y"), 120), Outcome::Inert);
        assert!(st.confirm.is_some());
        assert!(matches!(
            press_y(&mut st, &snap),
            Outcome::Engine(Command::Act(_))
        ));
    }

    #[test]
    fn branch_scope_and_the_refusals_show_a_notice_and_no_box() {
        let (mut snap, mut st) = setup(&[(1, "+")]);
        snap.scope = Scope::Branch;
        for k in ["s", "d", "D"] {
            assert_eq!(handle_key(&mut st, &snap, key(k), 120), Outcome::Redraw);
            assert_eq!(
                st.notice.as_ref().map(|n| n.text.as_str()),
                Some(NOTICE_SCOPE)
            );
            assert!(st.confirm.is_none());
        }
        snap.scope = Scope::Worktree;
        snap.diff = DiffState::Loading;
        st.reconcile(&snap);
        handle_key(&mut st, &snap, key("s"), 120);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some(NOTICE_NO_HUNK)
        );
        assert!(st.confirm.is_none());
    }

    #[test]
    fn a_second_key_while_an_action_is_running_opens_nothing() {
        let (mut snap, mut st) = setup(&[(1, "+")]);
        handle_key(&mut st, &snap, key("s"), 120);
        assert!(matches!(press_y(&mut st, &snap), Outcome::Engine(_)));
        handle_key(&mut st, &snap, key("s"), 120);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some(NOTICE_RUNNING)
        );
        assert!(st.confirm.is_none());
        // Answered and applied, with the same Arc on screen: still running.
        snap.action_seq = 1;
        snap.action_applied = true;
        st.observe(&snap);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some("staged hunk 1/1 of a.rs")
        );
        assert!(st.action_running(&snap));
        handle_key(&mut st, &snap, key("d"), 120);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some(NOTICE_RUNNING)
        );
        // A fresh Arc frees the keys.
        let fresh = snapshot("a.rs", "r2", &[(1, "+")]);
        snap.diff = fresh.diff.clone();
        st.observe(&snap);
        assert!(!st.action_running(&snap));
        st.reconcile(&snap);
        handle_key(&mut st, &snap, key("d"), 120);
        assert!(st.confirm.is_some());
    }

    #[test]
    fn a_refused_answer_frees_the_keys_at_once_and_carries_the_unstage_hint() {
        let (mut snap, mut st) = setup(&[(1, "+")]);
        handle_key(&mut st, &snap, key("d"), 120);
        press_y(&mut st, &snap);
        snap.action_seq = 1;
        snap.action_error = Some("f.txt: does not match index".into());
        snap.action_applied = true;
        st.observe(&snap);
        let notice = st.notice.as_ref().unwrap();
        assert!(notice.urgent);
        assert_eq!(
            notice.text,
            "discard failed: f.txt: does not match index; unstage it first (s)"
        );
        assert!(
            st.action_running(&snap),
            "applied on the same Arc still holds"
        );
        snap.action_seq = 2;
        snap.action_error = Some(NOTICE_CHANGED.into());
        snap.action_applied = false;
        st.pending_action = Some(crate::tui::state::PendingAction {
            diff: arc_of(&snap),
            done: "x".into(),
            verb: "stage",
            answered: false,
        });
        st.observe(&snap);
        assert!(
            !st.action_running(&snap),
            "a pre-git refusal frees the keys"
        );
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some("stage failed: the diff changed; look again")
        );
    }

    #[test]
    fn a_cut_diff_refuses_every_key() {
        let (mut snap, mut st) = setup(&[(1, "+++++")]);
        if let DiffState::Ready(d) = &mut snap.diff {
            let d = std::sync::Arc::make_mut(d);
            d.truncated_lines = 2;
        }
        st.reconcile(&snap);
        for k in ["s", "d", "D"] {
            handle_key(&mut st, &snap, key(k), 120);
            assert_eq!(
                st.notice.as_ref().map(|n| n.text.as_str()),
                Some(NOTICE_CUT),
                "{k}"
            );
            assert!(st.confirm.is_none());
        }
    }

    #[test]
    fn an_action_answer_over_an_unread_warning_puts_it_back() {
        use crate::engine::{Mark, MarkState};
        let (mut snap, mut st) = setup(&[(1, "+")]);
        handle_key(&mut st, &snap, key("s"), 120);
        press_y(&mut st, &snap);
        // One snapshot carries both the answer and a rewritten classification: the answer speaks
        // first (8.5's rule), and the warning waits for the body key that acknowledges it.
        snap.head_seen = Some("h1".repeat(20));
        snap.mark = Some(Mark {
            commit: "m".repeat(40),
            at: 1,
            state: MarkState::Rewritten,
            classified_at: snap.head_seen.clone(),
        });
        snap.action_seq = 1;
        snap.action_applied = true;
        st.observe(&snap);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some("staged hunk 1/1 of a.rs")
        );
        handle_key(&mut st, &snap, key("t"), 120);
        let notice = st.notice.as_ref().expect("the warning follows the answer");
        assert!(notice.urgent && notice.text.contains("no longer on this branch"));
    }
    #[test]
    fn a_opens_the_pane_picker_and_enter_picks_a_target() {
        let (mut snap, mut st) = setup(&[(10, " --+ ")]);
        st.socket_path = Some("/run/h.sock".into());
        assert_eq!(
            handle_key(&mut st, &snap, key("A"), 120),
            Outcome::Engine(Command::LoadPanes(1))
        );
        assert!(st.panes.is_some());
        // This opening receives its agent row.
        snap.panes = Some(std::sync::Arc::new(vec![crate::engine::PaneRow {
            record: crate::engine::host::PaneRecord {
                pane_id: "w1:p2".into(),
                agent: Some("codex".into()),
                ..Default::default()
            },
            this_worktree: true,
        }]));
        snap.panes_seq = 1;
        st.observe(&snap);
        let outcome = handle_key(&mut st, &snap, key("Enter"), 120);
        assert!(
            matches!(outcome, Outcome::Engine(Command::SetTarget { token: 1, target: crate::engine::Target::Pane { pane, socket, .. } }) if pane == "w1:p2" && socket == "/run/h.sock")
        );
        // A pending pick accepts no second Enter and closes on its token.
        assert_eq!(
            handle_key(&mut st, &snap, key("Enter"), 120),
            Outcome::Inert
        );
        snap.target_seq = 1;
        snap.target_token = Some(1);
        st.observe(&snap);
        assert!(st.panes.is_none());
        // Esc keeps the target as it is.
        handle_key(&mut st, &snap, key("A"), 120);
        assert_eq!(handle_key(&mut st, &snap, key("Esc"), 120), Outcome::Redraw);
        assert!(st.panes.is_none());
        // A refused pick (not remembered) is the viewer's notice, once.
        snap.target_seq = 2;
        snap.target_error = Some("target not remembered: read-only".into());
        st.observe(&snap);
        assert_eq!(
            st.notice.as_ref().map(|n| (n.text.as_str(), n.urgent)),
            Some(("target not remembered: read-only", true))
        );
    }

    #[test]
    fn two_answers_before_a_key_keep_the_first_warning() {
        let (mut snap, mut st) = setup(&[(10, " --+ ")]);
        snap.target_seq = 1;
        snap.target_error = Some("target not remembered: read-only".into());
        st.observe(&snap);
        // A second answer before the first is read: it shows at once, the first waits.
        snap.target_seq = 2;
        snap.target_error = Some("target not remembered: disk full".into());
        st.observe(&snap);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some("target not remembered: disk full")
        );
        // An unrelated snapshot leaves the unread answer on screen.
        snap.refreshing = !snap.refreshing;
        st.observe(&snap);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some("target not remembered: disk full")
        );
        // Read: the first speaks; read again: nothing more is owed.
        handle_key(&mut st, &snap, key("j"), 120);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some("target not remembered: read-only"),
            "an answer never loses the warning it displaced"
        );
        handle_key(&mut st, &snap, key("j"), 120);
        assert!(st.notice.is_none());
    }
    use crate::engine::comments::{self, Anchor};
    use crate::tui::{review, rows::Row};
    /// `setup` plus what the review loop reads: the diff's row in `files`, numbered lines, a clipboard target.
    pub(crate) fn review_setup(hunks: &[(u32, &str)]) -> (crate::engine::Snapshot, ViewState) {
        let (mut snap, mut st) = setup(hunks);
        snap.files = vec![crate::git::ChangedFile {
            path: "a.rs".into(),
            status: crate::git::ChangedFileStatus::Modified,
            staged: false,
            insertions: None,
            deletions: None,
        }];
        if let DiffState::Ready(diff) = &mut snap.diff {
            let mut numbered = (**diff).clone();
            for hunk in &mut numbered.file_diff.hunks {
                let (mut old, mut new) = (hunk.old_start, hunk.new_start);
                for line in &mut hunk.lines {
                    match line.line_type {
                        crate::git::DiffLineType::Added => {
                            line.new_line_number = Some(new);
                            new += 1;
                        }
                        crate::git::DiffLineType::Removed => {
                            line.old_line_number = Some(old);
                            old += 1;
                        }
                        _ => {
                            line.old_line_number = Some(old);
                            line.new_line_number = Some(new);
                            old += 1;
                            new += 1;
                        }
                    }
                }
            }
            snap.diff = DiffState::Ready(std::sync::Arc::new(numbered));
        }
        snap.target = Some(crate::engine::Target::Clipboard);
        snap.target_state = crate::engine::TargetState::Clipboard;
        st.observe(&snap);
        st.reconcile(&snap);
        (snap, st)
    }

    pub(crate) fn comment_at(anchor: &Anchor, text: &str, created_at: u64) -> comments::Comment {
        comments::Comment {
            id: format!("c{created_at}"),
            anchor: anchor.clone(),
            category: comments::Category::Bug,
            text: text.into(),
            created_at,
            state: comments::CommentState::Pending,
        }
    }

    pub(crate) fn anchor_on(snap: &crate::engine::Snapshot, line: u32) -> Anchor {
        let DiffState::Ready(diff) = &snap.diff else {
            panic!("ready")
        };
        Anchor {
            key: diff.key.clone(),
            side: Side::Additions,
            line,
            span: comments::Span::Line,
            comparison: comments::AnchorComparison::Worktree,
        }
    }

    #[test]
    fn i_without_a_target_opens_the_picker_and_keeps_the_anchor() {
        let (mut snap, mut st) = review_setup(&[(10, " --+ ")]);
        snap.target = None;
        snap.target_state = crate::engine::TargetState::Unverified;
        assert_eq!(
            handle_key(&mut st, &snap, key("i"), 120),
            Outcome::Engine(Command::LoadPanes(1))
        );
        assert!(st.editor.is_none());
        assert!(matches!(
            st.panes.as_ref().map(|p| &p.return_to),
            Some(crate::tui::panes::ReturnTo::Editor(_))
        ));
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some(review::CHOOSE_PANE)
        );

        handle_key(&mut st, &snap, key("Esc"), 120);
        assert!(st.panes.is_none() && st.editor.is_none());

        handle_key(&mut st, &snap, key("i"), 120);
        snap.panes = Some(std::sync::Arc::new(Vec::new()));
        snap.panes_seq = 2;
        st.observe(&snap);
        assert!(matches!(
            handle_key(&mut st, &snap, key("Enter"), 120),
            Outcome::Engine(Command::SetTarget {
                target: crate::engine::Target::Clipboard,
                ..
            })
        ));
        snap.target = Some(crate::engine::Target::Clipboard);
        snap.target_state = crate::engine::TargetState::Clipboard;
        snap.target_seq = 1;
        snap.target_token = Some(1);
        st.observe(&snap);
        assert!(st.panes.is_none());
        let editor = st
            .editor
            .as_ref()
            .expect("the editor opened on the kept anchor");
        assert_eq!(editor.place_label(), "L11"); // the fixture's first changed row is the deletion of old line 11
    }

    #[test]
    fn paste_in_the_editor_inserts_newlines_without_submitting() {
        for text in [
            "first line\rd\ry",
            "first line\r\nd\r\ny",
            "first line\nd\ny",
        ] {
            let (snap, mut st) = review_setup(&[(10, " --+ ")]);
            handle_key(&mut st, &snap, key("i"), 120);
            let token = st.comment_token;
            let outcome = handle_paste(&mut st, &snap, text);
            let editor = st.editor.as_ref().unwrap();
            assert_eq!(editor.text, "first line\nd\ny");
            assert_eq!(outcome, Outcome::Redraw);
            assert!(editor.pending.is_none());
            assert_eq!(st.comment_token, token);
            assert!(st.confirm.is_none());
        }
    }

    #[test]
    fn paste_outside_text_inputs_never_runs_shortcuts_or_confirms() {
        let (snap, mut st) = review_setup(&[(10, " --+ ")]);
        let before = (
            st.confirm.is_some(),
            st.editor.is_some(),
            st.panes.is_some(),
            st.cursor,
        );
        assert_eq!(
            handle_paste(&mut st, &snap, "first line\rd\ry"),
            Outcome::Inert
        );
        assert_eq!(
            (
                st.confirm.is_some(),
                st.editor.is_some(),
                st.panes.is_some(),
                st.cursor
            ),
            before
        );
        handle_key(&mut st, &snap, key("d"), 120);
        st.confirm.as_mut().unwrap().drawn = true;
        assert_eq!(handle_paste(&mut st, &snap, "y\rn"), Outcome::Inert);
        assert!(st.confirm.as_ref().unwrap().drawn);
        assert!(st.pending_action.is_none());
    }

    #[test]
    fn paste_in_the_base_picker_filters_without_submitting() {
        let (mut snap, mut st) = setup(&[(1, "+")]);
        assert_eq!(
            handle_key(&mut st, &snap, key("B"), 120),
            Outcome::Engine(Command::LoadRefs(1))
        );
        snap.refs_seq = 1;
        snap.refs = Some(std::sync::Arc::new(vec!["refs/remotes/origin/main".into()]));
        st.observe(&snap);
        let submitted = st.submitted_pick_seq;
        let outcome = handle_paste(&mut st, &snap, "origin/main\n");
        let picker = st.picker.as_ref().unwrap();
        assert_eq!(picker.input, "origin/main");
        assert_eq!(outcome, Outcome::Redraw);
        assert_eq!(
            picker.rows(&snap)[picker.cursor].submit().as_deref(),
            Some("refs/remotes/origin/main")
        );
        assert!(picker.pending.is_none() && !picker.done);
        assert_eq!(st.submitted_pick_seq, submitted);
    }

    #[test]
    fn paste_of_only_controls_in_the_base_picker_is_inert() {
        let (snap, mut st) = setup(&[(1, "+")]);
        handle_key(&mut st, &snap, key("B"), 120);
        assert_eq!(
            handle_paste(&mut st, &snap, "\r\n\t\0\u{1b}\u{7f}"),
            Outcome::Inert
        );
        let picker = st.picker.as_ref().unwrap();
        assert!(picker.input.is_empty());
        assert!(picker.pending.is_none() && !picker.done);
    }

    #[test]
    fn paste_in_the_pane_picker_filters_without_submitting() {
        let (mut snap, mut st) = review_setup(&[(10, " --+ ")]);
        handle_key(&mut st, &snap, key("A"), 120);
        snap.panes_seq = st.panes.as_ref().unwrap().token;
        snap.panes = Some(std::sync::Arc::new(
            ["codex", "claude"]
                .into_iter()
                .map(|agent| crate::engine::PaneRow {
                    record: crate::engine::host::PaneRecord {
                        pane_id: format!("w1:p{}", if agent == "codex" { 1 } else { 2 }),
                        agent: Some(agent.into()),
                        ..Default::default()
                    },
                    this_worktree: true,
                })
                .collect(),
        ));
        assert_eq!(
            handle_paste(&mut st, &snap, "co\r\nd\nex\t"),
            Outcome::Redraw
        );
        let picker = st.panes.as_ref().unwrap();
        assert_eq!(picker.input, "codex");
        let choices = picker.choices(&snap);
        assert_eq!(choices.len(), 2);
        assert!(
            matches!(&choices[0], panes::Choice::Pane(row) if row.record.agent.as_deref() == Some("codex"))
        );
        assert!(picker.pending.is_none());
    }

    #[test]
    fn paste_stops_at_the_first_rejected_newline() {
        for newline in ["\n", "\r", "\r\n"] {
            let (snap, mut st) = review_setup(&[(10, " --+ ")]);
            handle_key(&mut st, &snap, key("i"), 120);
            let text = format!("{}tail", format!("a{newline}").repeat(comments::MAX_LINES));
            assert_eq!(handle_paste(&mut st, &snap, &text), Outcome::Redraw);
            let editor = st.editor.as_ref().unwrap();
            assert_eq!(
                editor.text,
                format!("{}a", "a\n".repeat(comments::MAX_LINES - 1))
            );
            assert_eq!(comments::line_count(&editor.text), comments::MAX_LINES);
            assert!(!editor.text.contains("tail"));
            assert!(editor.at_limit);
            assert_eq!(editor.title()[0].text, comments::NOTICE_LIMIT);
        }
    }

    #[test]
    fn paste_within_the_caps_keeps_all_printable_text() {
        for (text, expected) in [
            ("first\n字\ntail", "first\n字\ntail"),
            ("first\t\0\u{1b}\u{7f}\n字\ntail", "first\n字\ntail"),
        ] {
            let (snap, mut st) = review_setup(&[(10, " --+ ")]);
            handle_key(&mut st, &snap, key("i"), 120);
            assert_eq!(handle_paste(&mut st, &snap, text), Outcome::Redraw);
            let editor = st.editor.as_ref().unwrap();
            assert_eq!(editor.text, expected);
            assert!(!editor.at_limit);
        }
    }

    #[test]
    fn paste_stops_at_the_character_cap() {
        let (snap, mut st) = review_setup(&[(10, " --+ ")]);
        handle_key(&mut st, &snap, key("i"), 120);
        let accepted = "字".repeat(comments::MAX_CHARS);
        assert_eq!(
            handle_paste(&mut st, &snap, &format!("{accepted}tail\nmore")),
            Outcome::Redraw
        );
        let editor = st.editor.as_ref().unwrap();
        assert_eq!(editor.text, accepted);
        assert!(editor.at_limit);
        assert_eq!(editor.title()[0].text, comments::NOTICE_LIMIT);
    }

    #[test]
    fn paste_respects_editor_caps_and_a_pending_save() {
        let (snap, mut st) = review_setup(&[(10, " --+ ")]);
        handle_key(&mut st, &snap, key("i"), 120);
        handle_paste(&mut st, &snap, &"字".repeat(comments::MAX_CHARS + 1));
        let editor = st.editor.as_mut().unwrap();
        assert_eq!(editor.text.chars().count(), comments::MAX_CHARS);
        assert!(editor.at_limit);
        editor.text.clear();
        handle_paste(&mut st, &snap, &"\r\n".repeat(comments::MAX_LINES + 1));
        let editor = st.editor.as_mut().unwrap();
        assert!(comments::within_caps(&editor.text));
        assert!(editor.at_limit);
        editor.text = "saving".into();
        editor.pending = Some(1);
        assert_eq!(handle_paste(&mut st, &snap, "extra"), Outcome::Inert);
        assert_eq!(st.editor.as_ref().unwrap().text, "saving");
    }

    #[test]
    fn the_editor_saves_edits_and_deletes_the_most_recent_card_of_the_line() {
        let (mut snap, mut st) = review_setup(&[(10, " --+ ")]);
        handle_key(&mut st, &snap, key("i"), 120);
        for ch in "needs a test".chars() {
            handle_key(&mut st, &snap, key(&ch.to_string()), 120);
        }
        handle_key(&mut st, &snap, key("ctrl+l"), 120);
        let outcome = handle_key(&mut st, &snap, key("Enter"), 120);
        let Outcome::Engine(Command::AddComment {
            token,
            anchor,
            category,
            text,
        }) = outcome
        else {
            panic!("{outcome:?}")
        };
        assert_eq!(
            (anchor.side, anchor.line, category, text.as_str()),
            (Side::Deletions, 11, comments::Category::Bug, "needs a test")
        );

        assert_eq!(st.editor.as_ref().and_then(|e| e.pending), Some(token));
        assert_eq!(
            handle_key(&mut st, &snap, key("x"), 120),
            Outcome::Inert,
            "keys wait for the answer"
        );
        snap.comment_seq += 1;
        snap.comment_token = None; // a refresh's notice: not this save's answer
        st.observe(&snap);
        assert!(
            st.editor.is_some(),
            "another answer leaves the editor waiting"
        );
        snap.comment_seq += 1;
        snap.comment_token = Some(token);
        st.observe(&snap);
        assert!(st.editor.is_none());

        let older = comment_at(&anchor, "older", 1);
        let newer = comment_at(&anchor, "newer", 2);
        snap.comments = std::sync::Arc::new(vec![older, newer.clone()]);
        st.observe(&snap);
        st.reconcile(&snap);
        handle_key(&mut st, &snap, key("u"), 120);
        assert_eq!(st.editor.as_ref().map(|e| e.text.as_str()), Some("newer"));
        for _ in 0..5 {
            handle_key(&mut st, &snap, key("Backspace"), 120);
        }
        for ch in "edited".chars() {
            handle_key(&mut st, &snap, key(&ch.to_string()), 120);
        }
        let outcome = handle_key(&mut st, &snap, key("Enter"), 120);
        let Outcome::Engine(Command::EditComment {
            token, seen, text, ..
        }) = outcome
        else {
            panic!("{outcome:?}")
        };
        assert!(seen == newer && text == "edited");
        snap.comment_seq += 1;
        snap.comment_token = Some(token);
        st.observe(&snap);
        assert!(st.editor.is_none());
        assert!(
            matches!(handle_key(&mut st, &snap, key("x"), 120), Outcome::Engine(Command::DeleteComment { seen }) if seen.id == newer.id)
        );

        let mut first = comment_at(&anchor, "made first", 7);
        first.id = "00000000000000010000000000000000".into();
        let mut second = comment_at(&anchor, "made second", 7);
        second.id = "00000000000000020000000000000000".into();
        snap.comments = std::sync::Arc::new(vec![second.clone(), first.clone()]);
        st.observe(&snap);
        st.reconcile(&snap);
        assert!(
            matches!(handle_key(&mut st, &snap, key("x"), 120), Outcome::Engine(Command::DeleteComment { seen }) if seen.id == second.id),
            "the same-second tie goes to the later id"
        );
        handle_key(&mut st, &snap, key("u"), 120);
        assert_eq!(
            st.editor.as_ref().map(|e| e.text.as_str()),
            Some("made second")
        );
        handle_key(&mut st, &snap, key("Esc"), 120);

        handle_key(&mut st, &snap, key("i"), 120);
        handle_key(&mut st, &snap, key(" "), 120);
        assert_eq!(
            handle_key(&mut st, &snap, key("Enter"), 120),
            Outcome::Inert
        );
        assert_eq!(handle_key(&mut st, &snap, key("Esc"), 120), Outcome::Redraw);
        assert!(st.editor.is_none());
    }

    #[test]
    fn a_refused_save_hands_the_draft_back_and_a_journaled_one_counts_as_saved() {
        let (mut snap, mut st) = review_setup(&[(10, " --+ ")]);
        handle_key(&mut st, &snap, key("i"), 120);
        for ch in "kept".chars() {
            handle_key(&mut st, &snap, key(&ch.to_string()), 120);
        }
        let Outcome::Engine(Command::AddComment { token, .. }) =
            handle_key(&mut st, &snap, key("Enter"), 120)
        else {
            panic!()
        };

        snap.comment_seq += 1;
        snap.comment_error = Some(comments::NOTICE_CAP.to_string());
        snap.comment_refused = true;
        snap.comment_token = Some(token);
        st.observe(&snap);
        let editor = st.editor.as_ref().expect("the draft survives a refusal");
        assert_eq!((editor.text.as_str(), editor.pending), ("kept", None));
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some(comments::NOTICE_CAP)
        );

        assert_eq!(handle_key(&mut st, &snap, key("!"), 120), Outcome::Redraw);
        assert_eq!(st.editor.as_ref().unwrap().text, "kept!");

        let Outcome::Engine(Command::AddComment { token, .. }) =
            handle_key(&mut st, &snap, key("Enter"), 120)
        else {
            panic!()
        };
        snap.comment_seq += 1;
        snap.comment_error = Some(format!("{}disk full", comments::NOTICE_NOT_REMEMBERED));
        snap.comment_refused = false;
        snap.comment_token = Some(token);
        st.observe(&snap);
        assert!(st.editor.is_none(), "a journaled save closes the editor");

        handle_key(&mut st, &snap, key("i"), 120);
        handle_key(&mut st, &snap, key("a"), 120);
        let Outcome::Engine(Command::AddComment { token: first, .. }) =
            handle_key(&mut st, &snap, key("Enter"), 120)
        else {
            panic!()
        };
        assert_eq!(handle_key(&mut st, &snap, key("Esc"), 120), Outcome::Redraw);
        handle_key(&mut st, &snap, key("i"), 120);
        handle_key(&mut st, &snap, key("b"), 120);
        let Outcome::Engine(Command::AddComment { token: second, .. }) =
            handle_key(&mut st, &snap, key("Enter"), 120)
        else {
            panic!()
        };
        assert!(second > first);
        snap.comment_seq += 1;
        snap.comment_error = None;
        snap.comment_refused = false;
        snap.comment_token = Some(first);
        st.observe(&snap);
        assert_eq!(
            st.editor.as_ref().map(|e| (e.text.as_str(), e.pending)),
            Some(("b", Some(second))),
            "the first save's answer is not the second's"
        );
        snap.comment_seq += 1;
        snap.comment_error = Some(comments::NOTICE_CAP.to_string());
        snap.comment_refused = true;
        snap.comment_token = Some(second);
        st.observe(&snap);
        assert_eq!(
            st.editor.as_ref().map(|e| (e.text.as_str(), e.pending)),
            Some(("b", None)),
            "the second's refusal hands its draft back"
        );
        handle_key(&mut st, &snap, key("Esc"), 120);

        let DiffState::Ready(diff) = &snap.diff else {
            panic!("ready")
        };

        let anchor = Anchor {
            key: diff.key.clone(),
            side: Side::Deletions,
            line: 11,
            span: comments::Span::Line,
            comparison: comments::AnchorComparison::Worktree,
        };
        let card = comment_at(&anchor, "theirs", 1);
        snap.comments = std::sync::Arc::new(vec![card.clone()]);
        st.observe(&snap);
        st.reconcile(&snap);
        handle_key(&mut st, &snap, key("u"), 120);
        for ch in " mine".chars() {
            handle_key(&mut st, &snap, key(&ch.to_string()), 120);
        }
        let Outcome::Engine(Command::EditComment { token, .. }) =
            handle_key(&mut st, &snap, key("Enter"), 120)
        else {
            panic!()
        };
        snap.comment_seq += 1;
        snap.comment_error = Some("No comment selected.".to_string());
        snap.comment_refused = true;
        snap.comment_token = Some(token);
        st.observe(&snap);
        assert_eq!(
            st.editor.as_ref().map(|e| e.text.as_str()),
            Some("theirs mine")
        );
    }

    #[test]
    fn a_draft_stays_on_screen_when_the_diff_goes_away() {
        let (mut snap, mut st) = review_setup(&[(10, " + ")]);
        handle_key(&mut st, &snap, key("i"), 120);
        for ch in "half".chars() {
            handle_key(&mut st, &snap, key(&ch.to_string()), 120);
        }

        snap.files.clear();
        snap.diff = DiffState::Idle;
        st.observe(&snap);
        st.reconcile(&snap);
        assert!(
            st.rows
                .as_ref()
                .is_some_and(|r| r.rows.iter().any(|row| matches!(row, Row::Editor { .. }))),
            "the editor's rows survive the diff"
        );
        let rendered = crate::tui::view::render(&snap, &st, 120, 24);
        assert!(
            rendered.plain().iter().any(|line| line.contains("half_")),
            "the draft is drawn, caret included"
        );
        assert_eq!(handle_key(&mut st, &snap, key("!"), 120), Outcome::Redraw);
        assert!(
            matches!(handle_key(&mut st, &snap, key("Enter"), 120), Outcome::Engine(Command::AddComment { text, .. }) if text == "half!")
        );
    }

    #[test]
    fn a_draft_keeps_its_file_when_another_diff_takes_the_screen() {
        let (mut snap, mut st) = review_setup(&[(10, " + ")]);
        handle_key(&mut st, &snap, key("i"), 120);
        handle_key(&mut st, &snap, key("k"), 120);

        let other = crate::tui::state::tests::snapshot("b.rs", "r2", &[(10, " + ")]);
        snap.diff = other.diff.clone();
        snap.files = other.files.clone();
        snap.selected = other.selected.clone();
        st.observe(&snap);
        st.reconcile(&snap);
        let rows = st.rows.as_ref().unwrap();
        let editor_rows: Vec<usize> = rows
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| matches!(r, Row::Editor { .. }))
            .map(|(i, _)| i)
            .collect();
        assert!(!editor_rows.is_empty());
        assert_eq!(
            *editor_rows.last().unwrap(),
            rows.rows.len() - 1,
            "the draft sits at the end, under no line of b.rs"
        );
        let outcome = handle_key(&mut st, &snap, key("Enter"), 120);
        assert!(
            matches!(&outcome, Outcome::Engine(Command::AddComment { anchor, .. }) if anchor.key.path == "a.rs"),
            "{outcome:?}"
        );
    }

    #[test]
    fn in_split_mode_u_and_x_take_the_card_of_the_cursors_side_only() {
        let (mut snap, mut st) = review_setup(&[(10, " -+ ")]);
        st.requested_mode = ViewMode::Split;
        st.resize(120, 20);
        let DiffState::Ready(diff) = &snap.diff else {
            panic!()
        };
        let left = comment_at(
            &Anchor {
                key: diff.key.clone(),
                side: Side::Deletions,
                line: 11,
                span: comments::Span::Line,
                comparison: comments::AnchorComparison::Worktree,
            },
            "left",
            1,
        );
        let right = comment_at(
            &Anchor {
                key: diff.key.clone(),
                side: Side::Additions,
                line: 11,
                span: comments::Span::Line,
                comparison: comments::AnchorComparison::Worktree,
            },
            "right",
            2,
        );
        snap.comments = std::sync::Arc::new(vec![left.clone(), right.clone()]);
        st.observe(&snap);
        st.reconcile(&snap);

        handle_key(&mut st, &snap, key("h"), 120);
        assert!(
            matches!(handle_key(&mut st, &snap, key("x"), 120), Outcome::Engine(Command::DeleteComment { seen }) if seen.id == left.id)
        );
        handle_key(&mut st, &snap, key("l"), 120);
        assert!(
            matches!(handle_key(&mut st, &snap, key("x"), 120), Outcome::Engine(Command::DeleteComment { seen }) if seen.id == right.id)
        );

        let rendered = crate::tui::view::render(&snap, &st, 120, 24);
        let rows = st.rows.as_ref().unwrap();
        let (row, target) = rows
            .rows
            .iter()
            .enumerate()
            .find_map(|(i, r)| match r {
                Row::Card {
                    id,
                    target: Some(t),
                    ..
                } if *id == right.id => Some((i, *t)),
                _ => None,
            })
            .unwrap();
        assert_eq!(diff.targets[target].side, Side::Additions);
        let y = (row - st.offset) as u16 + 1;
        let hit = rendered.hit(60, y).expect("the card row is a hit");
        assert!(matches!(hit, crate::tui::view::Action::CursorToRow(r) if *r == row));
    }

    #[test]
    fn file_comments_use_capital_keys_and_the_notices_say_what_is_missing() {
        let (mut snap, mut st) = review_setup(&[(10, " --+ ")]);
        handle_key(&mut st, &snap, key("I"), 120);
        assert_eq!(
            st.editor.as_ref().map(|e| e.place_label()),
            Some("file".into())
        );
        handle_key(&mut st, &snap, key("Esc"), 120);
        handle_key(&mut st, &snap, key("U"), 120);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some(review::NO_COMMENT)
        );
        handle_key(&mut st, &snap, key("x"), 120);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some(review::NO_COMMENT)
        );
        snap.diff = DiffState::Loading;
        handle_key(&mut st, &snap, key("i"), 120);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some(review::NO_LINE)
        );
    }

    #[test]
    fn v_selects_a_range_on_one_side_and_i_comments_on_it() {
        let (snap, mut st) = review_setup(&[(10, " --++ ")]);
        st.requested_mode = ViewMode::Split;
        st.resize(120, 20);
        st.reconcile(&snap);
        handle_key(&mut st, &snap, key("v"), 120);
        handle_key(&mut st, &snap, key("j"), 120);
        handle_key(&mut st, &snap, key("i"), 120);
        let editor = st.editor.as_ref().unwrap();
        assert!(
            matches!(editor.anchor.span, comments::Span::Range { .. }),
            "{:?}",
            editor.anchor
        );
        assert!(st.visual.is_none(), "i ends the selection");
        handle_key(&mut st, &snap, key("Esc"), 120);
        handle_key(&mut st, &snap, key("v"), 120);
        assert_eq!(handle_key(&mut st, &snap, key("Esc"), 120), Outcome::Redraw);
        assert!(st.visual.is_none());
        assert_eq!(
            handle_key(&mut st, &snap, key("y"), 120),
            Outcome::Inert,
            "y is inert without a selection"
        );

        handle_key(&mut st, &snap, key("v"), 120);
        let other = crate::tui::state::tests::snapshot("b.rs", "r2", &[(5, " + ")]);
        st.observe(&other);
        st.reconcile(&other);
        assert!(
            st.visual.is_none(),
            "a selection made on a.rs cannot name lines of b.rs"
        );
    }

    #[test]
    fn j_past_the_last_line_lands_on_an_orphan_card_where_u_and_x_act() {
        let (mut snap, mut st) = review_setup(&[(10, " + ")]);
        let gone = comment_at(
            &Anchor {
                key: FileKey {
                    path: "gone".into(),
                    staged: false,
                    untracked: false,
                },
                side: Side::Additions,
                line: 1,
                span: comments::Span::Line,
                comparison: comments::AnchorComparison::Worktree,
            },
            "lost",
            1,
        );
        snap.comments = std::sync::Arc::new(vec![gone.clone()]);
        st.observe(&snap);
        st.reconcile(&snap);
        let last = st.cursor.unwrap();
        handle_key(&mut st, &snap, key("G"), 120);
        handle_key(&mut st, &snap, key("j"), 120);
        assert_eq!(st.orphan, Some(0));
        assert_eq!(handle_key(&mut st, &snap, key("i"), 120), Outcome::Inert);
        handle_key(&mut st, &snap, key("u"), 120);
        assert_eq!(st.editor.as_ref().map(|e| e.text.as_str()), Some("lost"));
        handle_key(&mut st, &snap, key("Esc"), 120);
        assert!(
            matches!(handle_key(&mut st, &snap, key("x"), 120), Outcome::Engine(Command::DeleteComment { seen }) if seen.id == gone.id)
        );

        for action in ["s", "d"] {
            assert_eq!(
                handle_key(&mut st, &snap, key(action), 120),
                Outcome::Redraw,
                "{action}"
            );
            assert!(
                st.confirm.is_none(),
                "{action} opened a box for a hunk under a hidden cursor"
            );
            assert_eq!(
                st.notice.as_ref().map(|n| n.text.as_str()),
                Some(crate::engine::actions::NOTICE_NO_HUNK)
            );
            st.notice = None;
        }
        handle_key(&mut st, &snap, key("k"), 120);
        assert_eq!(st.orphan, None);
        let _ = last;
    }

    #[test]
    fn clicking_another_file_leaves_the_orphan_and_comments_on_the_new_diff() {
        let (mut snap, mut st) = review_setup(&[(10, "++")]);
        let mut other_file = snap.files[0].clone();
        other_file.path = "b.rs".into();
        snap.files.push(other_file);
        let mut orphan = comment_at(&anchor_on(&snap, 10), "gone", 1);
        orphan.anchor.key.path = "gone.rs".into();
        snap.comments = std::sync::Arc::new(vec![orphan]);
        st.files_panel = FilesPanel::Shown;
        st.reconcile(&snap);
        handle_key(&mut st, &snap, key("G"), 120);
        assert_eq!(handle_key(&mut st, &snap, key("j"), 120), Outcome::Redraw);
        assert_eq!(st.orphan, Some(0));

        let rendered = render(&snap, &st, 120, 24);
        let hit = rendered
            .hits
            .iter()
            .find(|hit| hit.action == Action::SelectFile(1))
            .unwrap();
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: hit.x0,
            row: hit.y,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            handle_mouse(&mut st, &snap, &rendered, click),
            Outcome::Engine(Command::Select(FileKey::of(&snap.files[1])))
        );
        assert_eq!(st.orphan, None);

        let other = snapshot("b.rs", "r2", &[(30, "++")]);
        snap.selected = other.selected;
        snap.diff = DiffState::Loading;
        st.observe(&snap);
        st.reconcile(&snap);
        snap.diff = other.diff;
        st.observe(&snap);
        st.reconcile(&snap);
        assert_eq!(st.rows.as_ref().unwrap().orphan_tops.len(), 1);
        assert_eq!(st.orphan, None);
        assert_eq!(handle_key(&mut st, &snap, key("i"), 120), Outcome::Redraw);
        assert_eq!(st.editor.as_ref().unwrap().anchor, anchor_on(&snap, 30));
    }

    #[test]
    fn a_missing_line_comment_is_reachable_editable_and_deletable() {
        let (mut snap, mut st) = review_setup(&[(10, "+++")]);
        let missing = comment_at(&anchor_on(&snap, 999), "missing hunk", 1);
        snap.comments = std::sync::Arc::new(vec![missing.clone()]);
        st.reconcile(&snap);
        handle_key(&mut st, &snap, key("G"), 120);
        assert_eq!(handle_key(&mut st, &snap, key("j"), 120), Outcome::Redraw);
        st.reconcile(&snap);
        assert_eq!(st.orphan, Some(0));
        assert_eq!(handle_key(&mut st, &snap, key("u"), 120), Outcome::Redraw);
        st.reconcile(&snap);
        assert_eq!(
            st.editor.as_ref().map(|e| e.text.as_str()),
            Some("missing hunk")
        );
        assert_eq!(st.orphan, Some(0));
        handle_key(&mut st, &snap, key("Esc"), 120);
        st.reconcile(&snap);
        assert_eq!(st.orphan, Some(0));
        assert!(matches!(
            handle_key(&mut st, &snap, key("x"), 120),
            Outcome::Engine(Command::DeleteComment { seen }) if seen.id == missing.id
        ));
    }

    #[test]
    fn page_keys_scroll_a_tall_orphan_without_losing_its_cursor() {
        for ready in [false, true] {
            let (mut snap, mut st) = review_setup(if ready { &[(10, "+++")] } else { &[] });
            let mut comment = comment_at(&anchor_on(&snap, 999), &["line"; 100].join("\n"), 1);
            comment.anchor.key.path = "gone.rs".into();
            snap.comments = std::sync::Arc::new(vec![comment]);
            if !ready {
                snap.files.clear();
                snap.diff = DiffState::Idle;
            }
            st.resize(120, 20);
            st.reconcile(&snap);
            handle_key(&mut st, &snap, key("G"), 120);
            assert_eq!(handle_key(&mut st, &snap, key("j"), 120), Outcome::Redraw);
            assert_eq!((st.offset, st.orphan), (0, Some(0)));
            let down = KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE);
            let up = KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE);
            let height = usize::from(st.body_height);
            for (key, pages) in [
                (down, 1),
                (down, 2),
                (up, 1),
                (key("ctrl+d"), 2),
                (key("ctrl+u"), 1),
            ] {
                assert_eq!(handle_key(&mut st, &snap, key, 120), Outcome::Redraw);
                st.reconcile(&snap);
                assert_eq!(
                    (st.offset, st.orphan),
                    (pages * height, Some(0)),
                    "ready={ready}, key={key:?}"
                );
            }
            let end = st.rows.as_ref().unwrap().rows.len() - height;
            for (key, limit) in [
                (down, end),
                (up, 0),
                (key("ctrl+d"), end),
                (key("ctrl+u"), 0),
            ] {
                while st.offset != limit {
                    let next = if limit == 0 {
                        st.offset.saturating_sub(height)
                    } else {
                        (st.offset + height).min(end)
                    };
                    assert_eq!(handle_key(&mut st, &snap, key, 120), Outcome::Redraw);
                    st.reconcile(&snap);
                    assert_eq!((st.offset, st.orphan), (next, Some(0)));
                }
                assert_eq!(handle_key(&mut st, &snap, key, 120), Outcome::Inert);
                assert_eq!((st.offset, st.orphan), (limit, Some(0)));
            }
        }
    }

    #[test]
    fn an_empty_list_with_orphans_is_reachable_and_editable() {
        let (mut snap, mut st) = review_setup(&[]);
        snap.files.clear();
        snap.diff = DiffState::Idle;
        let gone = comment_at(
            &Anchor {
                key: FileKey {
                    path: "gone".into(),
                    staged: false,
                    untracked: false,
                },
                side: Side::Additions,
                line: 1,
                span: comments::Span::Line,
                comparison: comments::AnchorComparison::Worktree,
            },
            "left behind",
            1,
        );
        let mut unsure = gone.clone();
        unsure.id = "u".into();
        unsure.state = comments::CommentState::Unconfirmed {
            stamp: comments::Stamp {
                at: 1,
                nonce: "abc123".into(),
                item: 1,
                to: crate::engine::target::Destination::clipboard(),
            },
            before: Vec::new(),
        };
        snap.comments = std::sync::Arc::new(vec![gone.clone(), unsure.clone()]);
        st.observe(&snap);
        st.reconcile(&snap);
        assert!(
            st.rows.as_ref().is_some_and(|r| r.orphan_tops.len() == 2),
            "the orphan section is built without a diff"
        );
        assert_eq!(st.cursor, None);

        let long = "x".repeat(300);
        let mut wide = gone.clone();
        wide.id = "wide".into();
        wide.text = long.clone();
        snap.comments = std::sync::Arc::new(vec![wide]);
        for mode in [ViewMode::Unified, ViewMode::Split] {
            st.requested_mode = mode;
            st.resize(120, 20);
            st.observe(&snap);
            st.reconcile(&snap);
            let rendered = crate::tui::view::render(&snap, &st, 120, 24).plain();
            let pieces: Vec<String> = st
                .rows
                .as_ref()
                .unwrap()
                .rows
                .iter()
                .filter_map(|r| match r {
                    Row::Card {
                        line: crate::tui::cards::CardLine::Text(t),
                        ..
                    } => Some(t.clone()),
                    _ => None,
                })
                .collect();
            assert_eq!(pieces.concat(), long, "{mode:?}");
            for piece in &pieces {
                assert!(
                    rendered.iter().any(|l| l.contains(piece.as_str())),
                    "{mode:?}: a wrapped piece was cut: {piece}"
                );
            }
        }
        snap.comments = std::sync::Arc::new(vec![gone.clone(), unsure.clone()]);
        st.requested_mode = ViewMode::Unified;
        st.resize(120, 20);
        st.observe(&snap);
        st.reconcile(&snap);
        handle_key(&mut st, &snap, key("j"), 120);
        assert_eq!(
            st.orphan,
            Some(0),
            "j from nothing enters the orphan section"
        );
        handle_key(&mut st, &snap, key("j"), 120);
        assert_eq!(st.orphan, Some(1));

        handle_key(&mut st, &snap, key("u"), 120);
        assert_eq!(
            st.editor
                .as_ref()
                .and_then(|e| e.editing.as_ref())
                .map(|c| c.id.as_str()),
            Some("u")
        );
        handle_key(&mut st, &snap, key("Esc"), 120);
        assert!(
            matches!(handle_key(&mut st, &snap, key("x"), 120), Outcome::Engine(Command::DeleteComment { seen }) if seen.id == "u")
        );
        let rendered = crate::tui::view::render(&snap, &st, 120, 24);
        let plain = rendered.plain().join("\n");
        assert!(
            plain.contains("✎ on changes no longer shown (2)")
                && !plain.contains("working tree clean"),
            "{plain}"
        );
    }

    #[test]
    fn clicks_under_an_open_editor_are_inert_except_the_category_words() {
        let (snap, mut st) = review_setup(&[(10, " + ")]);
        st.files_panel = FilesPanel::Shown;
        handle_key(&mut st, &snap, key("i"), 120);
        st.reconcile(&snap);
        let output = render(&snap, &st, 120, 24);
        let click = |hit: &crate::tui::view::Hit| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: hit.x0,
            row: hit.y,
            modifiers: KeyModifiers::NONE,
        };
        for hit in output.hits.iter().filter(|h| {
            matches!(
                h.action,
                Action::PickPane | Action::SelectFile(_) | Action::CursorToRow(_)
            )
        }) {
            assert_eq!(
                handle_mouse(&mut st, &snap, &output, click(hit)),
                Outcome::Inert
            );
        }
        assert!(st.panes.is_none() && st.confirm.is_none() && st.editor.is_some());
        let hit = output
            .hits
            .iter()
            .find(|h| h.action == Action::EditorCategory(comments::Category::Question))
            .expect("category hit");
        assert_eq!(
            handle_mouse(&mut st, &snap, &output, click(hit)),
            Outcome::Redraw
        );
        assert_eq!(
            st.editor.as_ref().unwrap().category,
            comments::Category::Question
        );
        let text: String = output.lines[usize::from(hit.y)]
            .iter()
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(
            text.chars()
                .skip(usize::from(hit.x0))
                .take(usize::from(hit.x1 - hit.x0))
                .collect::<String>(),
            "Question"
        );
        assert_eq!(
            handle_key(&mut st, &snap, key("ctrl+c"), 120),
            Outcome::Quit
        );
    }

    #[test]
    fn editing_a_line_card_replaces_it_until_escape() {
        for mode in [ViewMode::Unified, ViewMode::Split] {
            let (mut snap, mut st) = review_setup(&[(10, " + ")]);
            st.requested_mode = mode;
            st.resize(120, 20);
            let card = comment_at(&anchor_on(&snap, 11), "line card words", 1);
            let id = card.id.clone();
            snap.comments = std::sync::Arc::new(vec![card]);
            st.reconcile(&snap);
            handle_key(&mut st, &snap, key("u"), 120);
            st.reconcile(&snap);
            let plain = render(&snap, &st, 120, 24).plain().join("\n");
            assert_eq!(plain.matches("line card words").count(), 1, "{plain}");
            assert!(plain.contains("line card words_"));
            assert!(!st
                .rows
                .as_ref()
                .unwrap()
                .rows
                .iter()
                .any(|r| matches!(r, Row::Card { id: shown, .. } if shown == &id)));
            handle_key(&mut st, &snap, key("Esc"), 120);
            st.reconcile(&snap);
            assert!(st
                .rows
                .as_ref()
                .unwrap()
                .rows
                .iter()
                .any(|r| matches!(r, Row::Card { id: shown, .. } if shown == &id)));
        }
    }

    #[test]
    fn editing_an_orphan_replaces_its_card_until_the_editor_closes() {
        for ready in [false, true] {
            let (mut snap, mut st) = review_setup(&[(10, " + ")]);
            let mut orphan = comment_at(&anchor_on(&snap, 11), "orphan words", 1);
            orphan.anchor.key.path = "gone.rs".into();
            snap.comments = std::sync::Arc::new(vec![orphan]);
            if !ready {
                snap.diff = DiffState::Idle;
            }
            st.reconcile(&snap);
            handle_key(&mut st, &snap, key("G"), 120);
            handle_key(&mut st, &snap, key("j"), 120);
            handle_key(&mut st, &snap, key("u"), 120);
            st.reconcile(&snap);
            let plain = render(&snap, &st, 120, 24).plain().join("\n");
            assert_eq!(plain.matches("orphan words").count(), 1, "{plain}");
            assert!(plain.contains("orphan words_"));
            handle_key(&mut st, &snap, key("Esc"), 120);
            st.reconcile(&snap);
            assert!(st
                .rows
                .as_ref()
                .unwrap()
                .rows
                .iter()
                .any(|r| matches!(r, Row::Card { .. })));
            assert_eq!(st.orphan, Some(0));
        }
    }

    #[test]
    fn leaving_an_orphan_always_redraws_or_changes_file() {
        for movement in ["h", "l", "g", "n", "p"] {
            let (mut snap, mut st) = review_setup(&[(10, "+")]);
            let mut orphan = comment_at(&anchor_on(&snap, 10), "gone", 1);
            orphan.anchor.key.path = "gone.rs".into();
            snap.comments = std::sync::Arc::new(vec![orphan]);
            st.reconcile(&snap);
            handle_key(&mut st, &snap, key("j"), 120);
            assert_eq!(st.orphan, Some(0));
            let outcome = handle_key(&mut st, &snap, key(movement), 120);
            assert_eq!(st.orphan, None, "{movement}");
            assert_ne!(
                outcome,
                Outcome::Inert,
                "{movement}: a changed cursor needs a frame"
            );
        }
    }

    #[test]
    fn side_keys_collapse_a_selection_only_when_the_cursor_changes_side() {
        let (snap, mut st) = review_setup(&[(10, " --++ ")]);
        handle_key(&mut st, &snap, key("v"), 120);
        let start = st.visual.as_ref().unwrap().start;
        handle_key(&mut st, &snap, key("j"), 120);
        assert_ne!(st.cursor, Some(start));
        assert_eq!(handle_key(&mut st, &snap, key("h"), 120), Outcome::Inert);
        assert_eq!(
            st.visual.as_ref().unwrap().start,
            start,
            "an inert key preserves the visible selection"
        );
        assert_eq!(handle_key(&mut st, &snap, key("l"), 120), Outcome::Redraw);
        assert_eq!(st.visual.as_ref().unwrap().start, st.cursor.unwrap());
        let DiffState::Ready(diff) = &snap.diff else {
            panic!()
        };
        assert_eq!(diff.targets[st.cursor.unwrap()].side, Side::Additions);
    }

    pub(crate) fn pane_target() -> Target {
        Target::Pane {
            pane: "w4:p2".into(),
            socket: "/s".into(),
            agent: "codex".into(),
            session: None,
            title: "demo".into(),
        }
    }
    #[test]
    fn y_opens_the_finish_box_and_sends_what_it_showed() {
        let (mut snap, mut st) = review_setup(&[(10, " + ")]);
        snap.target = Some(pane_target());
        snap.target_state = crate::engine::TargetState::Live("idle".into());
        handle_key(&mut st, &snap, key("Y"), 120);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some(dispatch::NOTICE_NO_PENDING)
        );
        snap.comments = std::sync::Arc::new(vec![comment_at(&anchor_on(&snap, 10), "p", 1)]);
        handle_key(&mut st, &snap, key("Y"), 120);
        assert!(st.review_box.is_some());
        assert_eq!(
            handle_key(&mut st, &snap, key("Y"), 120),
            Outcome::Inert,
            "not drawn yet"
        );
        st.review_box.as_mut().unwrap().drawn = true;
        let outcome = handle_key(&mut st, &snap, key("Y"), 120);
        assert!(
            matches!(outcome, Outcome::Engine(Command::Send(dispatch::SendRequest { kind: dispatch::SendKind::Feedback, accepted })) if accepted == dispatch::Accepted::default())
        );
        assert_eq!(
            handle_key(&mut st, &snap, key("Y"), 120),
            Outcome::Inert,
            "a second Y while pending"
        );
        // A refusal keeps the box open, relabelled; the next Y accepts it.
        snap.send_seq = 1;
        snap.send_error = Some(
            "codex is working in w4:p2; the review would queue behind its current turn.".into(),
        );
        snap.send_refusal = Some(dispatch::Refusal::Busy);
        st.observe(&snap);
        let b = st.review_box.as_ref().unwrap();
        assert_eq!(b.offers(&snap).y, Some("send anyway"));
        let outcome = handle_key(&mut st, &snap, key("Y"), 120);
        assert!(matches!(outcome, Outcome::Engine(Command::Send(r)) if r.accepted.busy));
        // Success closes it with the notice.
        snap.send_seq = 2;
        snap.send_error = None;
        snap.send_refusal = None;
        snap.send_outcome = Some(dispatch::SendOutcome {
            kind: dispatch::SendKind::Feedback,
            items: 1,
            to: crate::engine::Destination::Pane {
                pane: "w4:p2".into(),
                agent: "codex".into(),
                session: None,
            },
            unconfirmed: false,
            copy: None,
        });
        st.observe(&snap);
        assert!(st.review_box.is_none());
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some("sent 1 item to codex · w4:p2")
        );
    }

    #[test]
    fn a_pick_made_for_the_finish_box_returns_to_it_without_sending() {
        let (mut snap, mut st) = review_setup(&[(10, " + ")]);
        snap.target = None;
        snap.target_state = crate::engine::TargetState::Unverified;
        snap.comments = std::sync::Arc::new(vec![comment_at(&anchor_on(&snap, 10), "p", 1)]);
        assert_eq!(
            handle_key(&mut st, &snap, key("Y"), 120),
            Outcome::Engine(Command::LoadPanes(1))
        );
        assert!(st.review_box.is_none() && st.panes.is_some());
        // Y again while the rows load: inert in the picker (typed as a filter character, harmless).
        handle_key(&mut st, &snap, key("Y"), 120);
        snap.panes = Some(std::sync::Arc::new(Vec::new()));
        snap.panes_seq = 1;
        st.observe(&snap);
        st.panes.as_mut().unwrap().input.clear();
        st.panes.as_mut().unwrap().retarget(&snap);
        assert!(matches!(
            handle_key(&mut st, &snap, key("Enter"), 120),
            Outcome::Engine(Command::SetTarget {
                target: crate::engine::Target::Clipboard,
                ..
            })
        ));
        snap.target = Some(crate::engine::Target::Clipboard);
        snap.target_state = crate::engine::TargetState::Clipboard;
        snap.target_seq = 1;
        snap.target_token = Some(1);
        st.observe(&snap);
        assert!(st.panes.is_none());
        let b = st.review_box.as_ref().expect("the box reopened");
        assert_eq!(
            (b.kind, b.pending),
            (review::BoxKind::Finish, None),
            "nothing was sent by the pick"
        );
        assert_eq!(b.offers(&snap).y, Some("copy"));
    }

    #[test]
    fn a_comment_answer_and_a_send_answer_in_a_row_both_speak() {
        let (mut snap, mut st) = review_setup(&[(10, " + ")]);
        // Two answers before a key: the second shows at once, the first waits, then speaks.
        snap.comment_seq = 1;
        snap.comment_error = Some(comments::NOTICE_CAP.to_string());
        snap.comment_refused = true;
        st.observe(&snap);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some(comments::NOTICE_CAP)
        );
        snap.send_seq = 1;
        snap.send_error = Some("could not verify w4:p2: deadline".to_string());
        st.observe(&snap);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some("could not verify w4:p2: deadline")
        );
        // An unrelated snapshot cannot displace the unread answer.
        snap.refreshing = !snap.refreshing;
        st.observe(&snap);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some("could not verify w4:p2: deadline")
        );
        // Read (a key clears it): the displaced warning is back; read again: nothing more.
        handle_key(&mut st, &snap, key("j"), 120);
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some(comments::NOTICE_CAP),
            "the first answer was not lost"
        );
        handle_key(&mut st, &snap, key("j"), 120);
        assert!(st.notice.is_none());
    }

    #[test]
    fn a_box_whose_target_goes_away_becomes_the_picker_and_asks_for_rows() {
        let (mut snap, mut st) = review_setup(&[(10, " + ")]);
        snap.target = Some(pane_target());
        snap.target_state = crate::engine::TargetState::Live("idle".into());
        snap.comments = std::sync::Arc::new(vec![comment_at(&anchor_on(&snap, 10), "p", 1)]);
        assert_eq!(handle_key(&mut st, &snap, key("Y"), 120), Outcome::Redraw);
        assert!(st.review_box.is_some());
        // A vanished target opens a picker that asks for rows.
        snap.target_state = crate::engine::TargetState::Gone;
        st.observe(&snap);
        assert!(st.review_box.is_none());
        assert!(matches!(
            st.panes.as_ref().map(|p| &p.return_to),
            Some(crate::tui::panes::ReturnTo::Finish)
        ));
        assert_eq!(st.pending_command.take(), Some(Command::LoadPanes(1)));
        assert_eq!(st.pending_command.take(), None, "sent once");
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some("codex · w4:p2 is gone · pick a pane")
        );
        // Observed again with the picker open, nothing happens twice.
        st.observe(&snap);
        assert_eq!(st.pending_command, None);
        assert_eq!(st.panes_token, 1);
    }

    #[test]
    fn the_request_box_scopes_with_f_and_a_and_c_copies_without_claiming() {
        let (mut snap, mut st) = review_setup(&[(10, " + ")]);
        snap.target = Some(pane_target());
        snap.target_state = crate::engine::TargetState::Live("idle".into());
        snap.files.push(crate::git::ChangedFile {
            path: "b.rs".into(),
            status: crate::git::ChangedFileStatus::Modified,
            staged: false,
            insertions: None,
            deletions: None,
        });
        snap.selected = Some(FileKey {
            path: "a.rs".into(),
            staged: false,
            untracked: false,
        });
        handle_key(&mut st, &snap, key("@"), 120);
        let b = st.review_box.as_ref().unwrap();
        assert_eq!(b.scope, dispatch::ReviewScope::All);
        handle_key(&mut st, &snap, key("f"), 120);
        assert!(
            matches!(&st.review_box.as_ref().unwrap().scope, dispatch::ReviewScope::File(k) if k.path == "a.rs")
        );
        handle_key(&mut st, &snap, key("a"), 120);
        assert_eq!(
            st.review_box.as_ref().unwrap().scope,
            dispatch::ReviewScope::All
        );
        // A pick made from a file-scoped box returns to a file-scoped box.
        handle_key(&mut st, &snap, key("f"), 120);
        assert_eq!(
            handle_key(&mut st, &snap, key("A"), 120),
            Outcome::Engine(Command::LoadPanes(1))
        );
        assert!(
            matches!(st.panes.as_ref().map(|p| &p.return_to), Some(crate::tui::panes::ReturnTo::Request(dispatch::ReviewScope::File(k))) if k.path == "a.rs")
        );
        snap.panes = Some(std::sync::Arc::new(Vec::new()));
        snap.panes_seq = 1;
        st.observe(&snap);
        st.panes.as_mut().unwrap().input.clear();
        st.panes.as_mut().unwrap().retarget(&snap);
        assert!(matches!(
            handle_key(&mut st, &snap, key("Enter"), 120),
            Outcome::Engine(Command::SetTarget {
                target: crate::engine::Target::Clipboard,
                ..
            })
        ));
        snap.target = Some(crate::engine::Target::Clipboard);
        snap.target_state = crate::engine::TargetState::Clipboard;
        snap.target_seq = 1;
        snap.target_token = Some(1);
        st.observe(&snap);
        assert!(
            matches!(&st.review_box.as_ref().expect("the box reopened").scope, dispatch::ReviewScope::File(k) if k.path == "a.rs"),
            "the pick kept the scope"
        );
        snap.target = Some(pane_target());
        snap.target_state = crate::engine::TargetState::Live("idle".into());
        handle_key(&mut st, &snap, key("a"), 120);
        let outcome = handle_key(&mut st, &snap, key("c"), 120);
        assert!(matches!(
            outcome,
            Outcome::Engine(Command::Copy(dispatch::CopyRequest {
                what: dispatch::CopyWhat::Request {
                    scope: dispatch::ReviewScope::All
                }
            }))
        ));
        assert!(st.review_box.is_none());
        // Gone target: @ opens the picker with the notice (the second picker of this test).
        snap.target_state = crate::engine::TargetState::Gone;
        assert_eq!(
            handle_key(&mut st, &snap, key("@"), 120),
            Outcome::Engine(Command::LoadPanes(2))
        );
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some("codex · w4:p2 is gone · pick a pane")
        );
        handle_key(&mut st, &snap, key("Esc"), 120);
        // An empty review offers only cancellation.
        snap.files.clear();
        snap.target = None;
        snap.target_state = crate::engine::TargetState::Unverified;
        assert_eq!(handle_key(&mut st, &snap, key("@"), 120), Outcome::Redraw);
        assert!(st.panes.is_none());
        let b = st.review_box.as_ref().expect("the box opened");
        let panel = b.panel(&snap, 60);
        assert_eq!((panel.rows.len(), panel.footer.as_str()), (1, "n cancel"));
        assert_eq!(handle_key(&mut st, &snap, key("A"), 120), Outcome::Inert);
        assert_eq!(handle_key(&mut st, &snap, key("c"), 120), Outcome::Inert);
        assert_eq!(handle_key(&mut st, &snap, key("n"), 120), Outcome::Redraw);
        assert!(st.review_box.is_none());
    }

    #[test]
    fn y_copies_a_selection_and_n_means_no_in_a_box_and_next_file_outside() {
        let (mut snap, mut st) = review_setup(&[(10, " ++ ")]);
        assert_eq!(
            handle_key(&mut st, &snap, key("n"), 120),
            Outcome::Engine(Command::SelectNext)
        );
        handle_key(&mut st, &snap, key("v"), 120);
        handle_key(&mut st, &snap, key("j"), 120);
        let outcome = handle_key(&mut st, &snap, key("y"), 120);
        assert!(
            matches!(outcome, Outcome::Engine(Command::Copy(dispatch::CopyRequest { what: dispatch::CopyWhat::Selection(text) })) if text == "line +\nline +"),
            "the fixture's added lines read `line +`"
        );
        assert!(st.visual.is_none());
        snap.comments = std::sync::Arc::new(vec![comment_at(&anchor_on(&snap, 10), "p", 1)]);
        handle_key(&mut st, &snap, key("Y"), 120);
        assert_eq!(handle_key(&mut st, &snap, key("n"), 120), Outcome::Redraw);
        assert!(st.review_box.is_none());
        // A copy answer speaks once, with the engine's notice.
        snap.copy_seq = 1;
        snap.copy = Some(std::sync::Arc::new(dispatch::CopyOut {
            osc: Some("\x1b]52;c;AA==\x07".into()),
            notice: "copied selection · also in ~/x/clipboard.md".into(),
            urgent: false,
        }));
        st.observe(&snap);
        assert_eq!(st.pending_copy.as_deref(), Some("\x1b]52;c;AA==\x07"));
        assert_eq!(
            st.notice.as_ref().map(|n| n.text.as_str()),
            Some("copied selection · also in ~/x/clipboard.md")
        );
    }

    #[test]
    fn the_boxes_are_modal_for_the_mouse_too() {
        for request in [false, true] {
            let (mut snap, mut st) = review_setup(&[(10, &"+".repeat(80))]);
            snap.comments = std::sync::Arc::new(vec![comment_at(&anchor_on(&snap, 10), "p", 1)]);
            st.files_panel = FilesPanel::Shown;
            let background = render(&snap, &st, 120, 24);
            handle_key(&mut st, &snap, key(if request { "@" } else { "Y" }), 120);
            assert!(st.review_box.is_some());
            let offset = st.offset;
            let cursor = st.cursor;
            for hit in background
                .hits
                .iter()
                .filter(|hit| matches!(hit.action, Action::SelectFile(_)))
            {
                for kind in [
                    MouseEventKind::Down(MouseButton::Left),
                    MouseEventKind::ScrollDown,
                    MouseEventKind::ScrollUp,
                ] {
                    assert_eq!(
                        handle_mouse(
                            &mut st,
                            &snap,
                            &background,
                            MouseEvent {
                                kind,
                                column: hit.x0,
                                row: hit.y,
                                modifiers: KeyModifiers::NONE
                            }
                        ),
                        Outcome::Inert
                    );
                }
            }
            for k in ["j", "?", "s", "i", "v", "q", "y", "/", "@"] {
                assert_eq!(
                    handle_key(&mut st, &snap, key(k), 120),
                    Outcome::Inert,
                    "{k}"
                );
            }
            assert_eq!((st.offset, st.cursor), (offset, cursor));
            assert_eq!(handle_key(&mut st, &snap, key("Esc"), 120), Outcome::Redraw);
            assert!(st.review_box.is_none());
        }
    }

    #[test]
    fn a_copy_beside_a_send_answer_is_written_once_and_speaks_through_the_send() {
        for error in [false, true] {
            let (mut snap, mut st) = review_setup(&[(10, " + ")]);
            let copy = dispatch::CopyOut {
                osc: Some("\x1b]52;c;AA==\x07".into()),
                notice: "copied review".into(),
                urgent: false,
            };
            st.warn("unread warning");
            snap.send_seq = 1;
            snap.copy_seq = 1;
            snap.copy = Some(std::sync::Arc::new(copy.clone()));
            if error {
                snap.send_error = Some("copied, but could not record".into());
            } else {
                snap.send_outcome = Some(dispatch::SendOutcome {
                    kind: dispatch::SendKind::Feedback,
                    items: 1,
                    to: crate::engine::Destination::clipboard(),
                    unconfirmed: true,
                    copy: Some(copy),
                });
            }
            st.observe(&snap);
            assert_eq!(
                st.pending_copy.take().as_deref(),
                Some("\x1b]52;c;AA==\x07")
            );
            assert_eq!(
                st.notice.as_ref().map(|n| (n.text.as_str(), n.urgent)),
                Some(if error {
                    ("copied, but could not record", false)
                } else {
                    ("copied review", true)
                })
            );
            st.observe(&snap);
            assert!(st.pending_copy.is_none());
            handle_key(&mut st, &snap, key("j"), 120);
            assert_eq!(
                st.notice.as_ref().map(|n| n.text.as_str()),
                Some("unread warning")
            );
            handle_key(&mut st, &snap, key("j"), 120);
            assert!(
                st.notice.is_none(),
                "the copy did not queue a duplicate notice"
            );
        }
    }

    #[test]
    fn an_empty_request_stays_cancel_only_when_a_target_goes_away() {
        for gone in [TargetState::Left, TargetState::Gone] {
            let (mut snap, mut st) = review_setup(&[(10, " + ")]);
            snap.target = Some(pane_target());
            snap.target_state = TargetState::Live("idle".into());
            snap.files.clear();
            handle_key(&mut st, &snap, key("@"), 120);
            assert!(st.review_box.is_some());
            snap.target_state = gone;
            st.observe(&snap);
            assert!(
                st.review_box.is_some(),
                "an empty review still offers n alone"
            );
            assert!(st.panes.is_none() && st.pending_command.is_none());
            assert_eq!(
                st.review_box.as_ref().unwrap().panel(&snap, 60).footer,
                "n cancel"
            );
            st.review_box = None;
            st.notify("previous answer");
            handle_key(&mut st, &snap, key("@"), 120);
            assert!(st.review_box.is_some() && st.panes.is_none());
            assert!(st.pending_command.is_none());
        }
    }
}
