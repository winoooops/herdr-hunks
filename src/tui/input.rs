//! Pure input handling: change view state or request an engine command.
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use crate::engine::nav::{self, Side, ViewMode};
use crate::engine::{Command, DiffState, FileKey, Snapshot};
use crate::tui::keys::{self, KeyAction};
use crate::tui::layout::clamp_scroll;
use crate::tui::state::{FilesPanel, ViewState};
use crate::tui::view::{self, Action, Rendered};
use crate::tui::{dialog, format};

#[derive(Debug, PartialEq, Eq)]
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
    let total = dialog::line_count(&keys::help_panel(), width.min(60));
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
            state.notice = None;
            return Outcome::Redraw;
        }
        return match action {
            Some(KeyAction::LineDown) => scroll_help(state, snapshot, width, 1),
            Some(KeyAction::LineUp) => scroll_help(state, snapshot, width, -1),
            _ => Outcome::Inert,
        };
    }
    match action {
        Some(action) => apply_action(state, snapshot, action, width),
        None => Outcome::Inert,
    }
}

fn apply_action(
    state: &mut ViewState,
    snapshot: &Snapshot,
    action: KeyAction,
    width: u16,
) -> Outcome {
    let cleared_notice = state.notice.take().is_some();
    let outcome = act(state, snapshot, action, width);
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
        FileNext => return Outcome::Engine(Command::SelectNext),
        FilePrev => return Outcome::Engine(Command::SelectPrev),
        ToggleView => {
            if state.mode == ViewMode::Unified && width < view::MIN_SPLIT_WIDTH {
                state.notice = Some("split view needs 100 columns".into());
            } else {
                state.mode = match state.mode {
                    ViewMode::Unified => ViewMode::Split,
                    ViewMode::Split => ViewMode::Unified,
                };
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
        ToggleMouse => state.mouse_requested = !state.mouse_requested,
        Help => {
            state.help_open = true;
            state.help_offset = 0;
        }
        _ => return move_cursor(state, snapshot, action),
    }
    Outcome::Redraw
}

fn move_cursor(state: &mut ViewState, snapshot: &Snapshot, action: KeyAction) -> Outcome {
    use KeyAction::*;
    let (DiffState::Ready(diff), Some(cursor)) = (&snapshot.diff, state.cursor) else {
        return Outcome::Inert;
    };
    let Some(current) = diff.targets.get(cursor) else {
        return Outcome::Inert;
    };
    let target = match action {
        LineDown | LineUp => Some(nav::move_line(
            &diff.targets,
            &diff.unified_order,
            cursor,
            if action == LineDown { 1 } else { -1 },
            state.mode,
        )),
        SideDeletions | SideAdditions => Some(nav::move_side(
            &diff.targets,
            cursor,
            if action == SideDeletions {
                Side::Deletions
            } else {
                Side::Additions
            },
            state.mode,
        )),
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
            return scroll(
                &mut state.hscroll,
                if action == ScrollRight { 8 } else { -8 },
                usize::MAX,
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
    if !ev.modifiers.is_empty() {
        return Outcome::Inert;
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
    let action = match rendered.hit(ev.column, ev.row) {
        Some(Action::PrevFile) => KeyAction::FilePrev,
        Some(Action::NextFile) => KeyAction::FileNext,
        Some(Action::PrevHunk) => KeyAction::HunkPrev,
        Some(Action::NextHunk) => KeyAction::HunkNext,
        Some(Action::ToggleView) => KeyAction::ToggleView,
        Some(Action::ToggleFiles) => KeyAction::ToggleFiles,
        Some(Action::Refresh) => KeyAction::Refresh,
        Some(Action::SelectFile(index)) => {
            return snapshot
                .files
                .get(*index)
                .map(|file| {
                    Outcome::Engine(Command::Select(FileKey {
                        path: file.path.clone(),
                        staged: file.staged,
                    }))
                })
                .unwrap_or(Outcome::Inert)
        }
        Some(Action::CursorToRow(row)) => {
            if let (DiffState::Ready(diff), Some(rows)) = (&snapshot.diff, &state.rows) {
                if let Some(target) = rows.row_of_target.iter().position(|r| r == row) {
                    if state.cursor != Some(target) {
                        state.set_cursor(diff, target);
                        return Outcome::Redraw;
                    }
                }
            }
            return Outcome::Inert;
        }
        None => return Outcome::Inert,
    };
    apply_action(state, snapshot, action, width)
}

#[cfg(test)]
mod tests {
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

    fn key(text: &str) -> KeyEvent {
        match text.strip_prefix("ctrl+") {
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

    fn setup(hunks: &[(u32, &str)]) -> (crate::engine::Snapshot, ViewState) {
        let snap = snapshot("a.rs", "r1", hunks);
        let mut st = ViewState::new(ViewMode::Split, FilesPanel::Hidden, true);
        st.resize(body_height(&st, &snap, 24));
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
        st.resize(body_height(&st, &snap, 12));
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
                let (snap, mut st) = setup(&[(10, " --+ "), (40, long.as_str())]);
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
        st.mode = ViewMode::Unified;
        st.reconcile(&snap);
        handle_key(&mut st, &snap, key("t"), 90);
        assert_eq!(st.mode, ViewMode::Unified);
        assert!(st.notice.as_deref().unwrap_or("").contains("100 columns"));
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
        st.resize(body_height(&st, &snap, 12));
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
        let (snap, mut st) = setup(&[(1, kinds.as_str())]);
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
        st.resize(body_height(&st, &snap, 24));
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
        let (snap, mut st) = setup(&[(10, " --+ ")]);
        st.mode = ViewMode::Unified;
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
        st.resize(body_height(&st, &snap, 12));
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
        assert_eq!(st.help_offset, KEYS.len() - 6);
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
        assert_eq!(st.help_offset, KEYS.len() - 9);
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
        assert_eq!(handle_key(&mut st, &snap, key("s"), 99), Outcome::Inert);
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
                staged: true
            }))
        );
        for kind in [
            MouseEventKind::Down(MouseButton::Right),
            MouseEventKind::Down(MouseButton::Middle),
            MouseEventKind::Up(MouseButton::Left),
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::Moved,
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
}
