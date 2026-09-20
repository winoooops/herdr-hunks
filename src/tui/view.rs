//! Pure view: snapshot + view state in, styled lines and hit regions out.
use crate::engine::nav::ViewMode;
use crate::engine::{DiffState, RepoState, Snapshot};
use crate::git::ChangedFileStatus;
use crate::tui::format::{pad, truncate, width};
use crate::tui::rows::Row;
use crate::tui::sanitize::sanitize;
use crate::tui::state::{FilesPanel, ViewState};
use crate::tui::style::{Line, Role, Semantic, Span, Style};

pub const FILES_WIDTH: u16 = 18;
pub const MIN_SPLIT_WIDTH: u16 = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    PrevFile,
    NextFile,
    PrevHunk,
    NextHunk,
    ToggleView,
    ToggleFiles,
    Refresh,
    SelectFile(usize),
    CursorToRow(usize),
}

pub struct Hit {
    pub y: u16,
    pub x0: u16,
    pub x1: u16,
    pub action: Action,
}

pub struct Rendered {
    pub lines: Vec<Line>,
    pub hits: Vec<Hit>,
}

impl Rendered {
    pub fn plain(&self) -> Vec<String> {
        self.lines
            .iter()
            .map(|l| {
                l.iter()
                    .map(|s| s.text.as_str())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    pub fn hit(&self, x: u16, y: u16) -> Option<&Action> {
        self.hits
            .iter()
            .find(|h| h.y == y && x >= h.x0 && x < h.x1)
            .map(|h| &h.action)
    }
}

/// The one-line notice above the footer, by priority.
pub fn notice(state: &ViewState, snapshot: &Snapshot) -> Option<String> {
    if let (Some(error), false) = (&snapshot.status_error, snapshot.files.is_empty()) {
        return Some(format!(
            "status failed: {} · showing the last good list · r retries",
            sanitize(error)
        ));
    }
    if let Some(reason) = &snapshot.watcher_error {
        return Some(format!(
            "live refresh degraded: {} · polling every 5 s · r retries",
            sanitize(reason)
        ));
    }
    state.notice.clone()
}

pub fn body_height(state: &ViewState, snapshot: &Snapshot, height: u16) -> u16 {
    height.saturating_sub(2 + u16::from(notice(state, snapshot).is_some()))
}

/// The frozen parser yields zero hunks for a combined diff; the raw text still says so.
fn is_conflict(raw_diff: &str) -> bool {
    raw_diff.lines().any(|l| {
        l.starts_with("diff --cc") || l.starts_with("diff --combined") || l.starts_with("@@@")
    })
}

/// One toolbar item: its text pieces (clickable when they carry an action) and its drop order.
type ToolbarItem = (Vec<(String, Option<Action>)>, u8);

/// Toolbar items left to right; a higher drop order drops first.
fn toolbar_items(snapshot: &Snapshot, state: &ViewState, total_width: u16) -> Vec<ToolbarItem> {
    let total = snapshot.files.len();
    let index = snapshot.selected.as_ref().and_then(|k| {
        snapshot
            .files
            .iter()
            .position(|f| f.path == k.path && f.staged == k.staged)
    });
    let name = snapshot
        .selected
        .as_ref()
        .map(|k| sanitize(k.path.rsplit('/').next().unwrap_or(&k.path)))
        .unwrap_or_else(|| "no file".into());
    let position = format!(" {}/{} ", index.map(|i| i + 1).unwrap_or(0), total);
    let name = truncate(
        &name,
        usize::from(total_width).saturating_sub(7 + width(&position)),
    );
    let (hunk_pos, hunk_total, stats, staged) = match &snapshot.diff {
        DiffState::Ready(d) => {
            let pos = state
                .cursor
                .and_then(|c| d.targets.get(c))
                .map(|t| t.hunk_index + 1)
                .unwrap_or(0);
            let file = index.and_then(|i| snapshot.files.get(i));
            let stats = file
                .map(|f| {
                    format!(
                        "+{} −{}",
                        f.insertions.unwrap_or(0),
                        f.deletions.unwrap_or(0)
                    )
                })
                .unwrap_or_default();
            (pos, d.file_diff.hunks.len(), stats, d.key.staged)
        }
        _ => (
            0,
            0,
            String::new(),
            snapshot
                .selected
                .as_ref()
                .map(|k| k.staged)
                .unwrap_or(false),
        ),
    };
    let busy = if snapshot.refreshing || matches!(snapshot.diff, DiffState::Loading) {
        "…"
    } else {
        "⟳"
    };
    vec![
        (
            vec![
                ("‹".into(), Some(Action::PrevFile)),
                (format!(" {name}{position}"), None),
                ("›".into(), Some(Action::NextFile)),
            ],
            0,
        ),
        (
            vec![
                (format!("{{}} {hunk_pos}/{hunk_total} "), None),
                ("↑".into(), Some(Action::PrevHunk)),
                (" ".into(), None),
                ("↓".into(), Some(Action::NextHunk)),
            ],
            1,
        ),
        (
            vec![(
                if state.mode == ViewMode::Split {
                    "split"
                } else {
                    "unified"
                }
                .into(),
                Some(Action::ToggleView),
            )],
            2,
        ),
        (
            vec![(if staged { "STAGED" } else { "UNSTAGED" }.into(), None)],
            3,
        ),
        (vec![(stats, None)], 4),
        (vec![("files".into(), Some(Action::ToggleFiles))], 5),
        (vec![(busy.into(), Some(Action::Refresh))], 6),
    ]
}

fn toolbar(snapshot: &Snapshot, state: &ViewState, total_width: u16) -> (Line, Vec<Hit>) {
    let mut items = toolbar_items(snapshot, state, total_width);
    let item_width =
        |item: &Vec<(String, Option<Action>)>| item.iter().map(|(t, _)| width(t)).sum::<usize>();
    // drop from the right until it fits; steppers (drop order 0 and 1) go last
    while items.len() > 1
        && 1 + items.iter().map(|(i, _)| item_width(i) + 3).sum::<usize>()
            > usize::from(total_width)
    {
        let worst = items
            .iter()
            .enumerate()
            .max_by_key(|(_, (_, order))| *order)
            .map(|(i, _)| i)
            .unwrap();
        items.remove(worst);
    }
    let mut line: Line = vec![Span::body(" ")];
    let mut hits = Vec::new();
    let mut x = 1u16;
    for (item, _) in items {
        for (text, action) in item {
            let w = width(&text) as u16;
            if let Some(action) = action {
                hits.push(Hit {
                    y: 0,
                    x0: x,
                    x1: x + w,
                    action,
                });
                line.push(Span::emphasis(text));
            } else {
                line.push(Span::body(text));
            }
            x += w;
        }
        line.push(Span::body("   "));
        x += 3;
    }
    (line, hits)
}

fn state_message(snapshot: &Snapshot) -> Option<String> {
    match &snapshot.repo {
        RepoState::Unusable { reason } => return Some(sanitize(reason)),
        RepoState::NotARepo { .. } => return Some("not a git repository".into()),
        RepoState::Repo { .. } => {}
    }
    if snapshot.files.is_empty() {
        return Some(
            snapshot
                .status_error
                .as_ref()
                .map(|e| sanitize(e))
                .unwrap_or_else(|| "working tree clean".into()),
        );
    }
    match &snapshot.diff {
        DiffState::Loading => Some("loading…".into()),
        DiffState::Failed(error) => Some(sanitize(error)),
        DiffState::Ready(diff) if diff.file_diff.hunks.is_empty() => Some(
            if is_conflict(&diff.raw_diff) {
                "unmerged path: resolve conflicts to see hunks"
            } else {
                "binary file or no textual changes"
            }
            .into(),
        ),
        _ => None,
    }
}

fn files_lines(snapshot: &Snapshot, _state: &ViewState, height: u16) -> (Vec<Line>, Vec<Hit>) {
    let mut lines = Vec::new();
    let mut hits = Vec::new();
    if height == 0 {
        return (lines, hits);
    }
    lines.push(vec![Span::emphasis(pad(
        &format!("CHANGED {}", snapshot.files.len()),
        FILES_WIDTH.into(),
    ))]);
    let selected = snapshot.selected.as_ref().and_then(|key| {
        snapshot
            .files
            .iter()
            .position(|file| file.path == key.path && file.staged == key.staged)
    });
    let count = usize::from(height.saturating_sub(2));
    let first = selected
        .unwrap_or(0)
        .saturating_sub(count.saturating_sub(1));
    for (i, file) in snapshot.files.iter().enumerate().skip(first).take(count) {
        let marker = if selected == Some(i) { '▸' } else { ' ' };
        let status = match file.status {
            ChangedFileStatus::Modified => 'M',
            ChangedFileStatus::Added => 'A',
            ChangedFileStatus::Deleted => 'D',
            ChangedFileStatus::Renamed => 'R',
            ChangedFileStatus::Untracked => '?',
        };
        let (dir, name) = file.path.rsplit_once('/').unwrap_or((".", &file.path));
        let duplicate = snapshot.files.iter().any(|other| {
            let (other_dir, other_name) = other.path.rsplit_once('/').unwrap_or((".", &other.path));
            other_name == name && other_dir != dir
        });
        let mut line = vec![
            Span::body(format!("{marker}{status} ")),
            Span::body(sanitize(name)),
        ];
        if duplicate {
            line.push(Span::label(format!(" {}/", sanitize(dir))));
        }
        let mut line = fit_line(line, usize::from(FILES_WIDTH - 2));
        line.push(Span::body(if file.staged { "S " } else { "  " }));
        hits.push(Hit {
            y: lines.len() as u16 + 1,
            x0: 0,
            x1: FILES_WIDTH,
            action: Action::SelectFile(i),
        });
        lines.push(line);
    }
    if height > 1 {
        while lines.len() < usize::from(height - 1) {
            lines.push(vec![Span::body(" ".repeat(FILES_WIDTH.into()))]);
        }
        let additions: u64 = snapshot
            .files
            .iter()
            .map(|f| u64::from(f.insertions.unwrap_or(0)))
            .sum();
        let deletions: u64 = snapshot
            .files
            .iter()
            .map(|f| u64::from(f.deletions.unwrap_or(0)))
            .sum();
        lines.push(vec![Span::label(pad(
            &format!("+{additions} −{deletions} · {} files", snapshot.files.len()),
            FILES_WIDTH.into(),
        ))]);
    }
    (lines, hits)
}

fn fit_line(mut line: Line, cells: usize) -> Line {
    let mut remaining = cells;
    for span in &mut line {
        span.text = truncate(&span.text, remaining);
        remaining = remaining.saturating_sub(width(&span.text));
    }
    if remaining > 0 {
        line.push(Span::body(" ".repeat(remaining)));
    }
    line
}

fn scroll_text(text: &str, cells: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    if cells == 0 {
        return text.to_string();
    }
    let mut skipped = 0;
    for (byte, ch) in text.char_indices() {
        let ch_width = ch.width().unwrap_or(0);
        if skipped >= cells && ch_width > 0 {
            return text[byte..].to_string();
        }
        skipped += ch_width;
        if skipped > cells {
            return format!(
                "{}{}",
                " ".repeat(skipped - cells),
                &text[byte + ch.len_utf8()..]
            );
        }
    }
    String::new()
}

fn body_line(row: &Row, columns: u16, hscroll: usize, _mode: ViewMode, cursor: bool) -> Line {
    let cells = usize::from(columns);
    let number = |no: Option<u32>| no.map(|n| n.to_string()).unwrap_or_default();
    let line_style = |sign| match sign {
        '+' => Style::semantic(Role::Body, Semantic::Good),
        '-' => Style::semantic(Role::Body, Semantic::Bad),
        _ => Style::role(Role::Body),
    };
    let line = match row {
        Row::FileHeader { path } => vec![Span::emphasis(scroll_text(path, hscroll))],
        Row::HunkHeader { text, .. } => vec![Span::new(
            scroll_text(text, hscroll),
            Style::semantic(Role::Label, Semantic::Accent),
        )],
        Row::Gap { lines } => vec![Span::label(format!("··· {lines} unmodified lines ···"))],
        Row::Truncated { lines } => vec![Span::label(format!("… {lines} more lines not shown"))],
        Row::Unified {
            old_no,
            new_no,
            sign,
            text,
            ..
        } => vec![
            Span::label(format!("{:>5} {:>5} ", number(*old_no), number(*new_no))),
            Span::new(
                format!("{sign} {}", scroll_text(text, hscroll)),
                line_style(*sign),
            ),
        ],
        Row::Split { left, right } => {
            let half = cells.saturating_sub(1) / 2;
            let side = |cell: &Option<crate::tui::rows::Cell>| {
                fit_line(
                    match cell {
                        Some(cell) => vec![
                            Span::label(format!("{:>5} ", number(cell.no))),
                            Span::new(
                                format!("{} {}", cell.sign, scroll_text(&cell.text, hscroll)),
                                line_style(cell.sign),
                            ),
                        ],
                        None => Vec::new(),
                    },
                    half,
                )
            };
            let mut line = side(left);
            line.push(Span::new("│", Style::role(Role::Rule)));
            line.extend(side(right));
            line
        }
    };
    let mut line = fit_line(line, cells);
    if cursor {
        for span in &mut line {
            span.style.reverse = true;
        }
    }
    line
}

pub fn render(snapshot: &Snapshot, state: &ViewState, columns: u16, height: u16) -> Rendered {
    if columns < 40 || height < 10 {
        return Rendered {
            lines: vec![vec![Span::body("terminal too small")]],
            hits: Vec::new(),
        };
    }
    let (bar, mut hits) = toolbar(snapshot, state, columns);
    let mut lines = vec![fit_line(bar, columns.into())];
    let notice = notice(state, snapshot);
    let height_body = height.saturating_sub(2 + u16::from(notice.is_some()));
    let panel_width = if state.files_panel == FilesPanel::Hidden {
        0
    } else {
        FILES_WIDTH + 1
    };
    let body_width = columns - panel_width;
    let (panel, file_hits) = if panel_width > 0 {
        files_lines(snapshot, state, height_body)
    } else {
        (Vec::new(), Vec::new())
    };
    hits.extend(file_hits);
    let message = state_message(snapshot);
    for y in 0..height_body {
        let mut line = panel.get(usize::from(y)).cloned().unwrap_or_default();
        if panel_width > 0 {
            line.push(Span::new("│", Style::role(Role::Rule)));
        }
        let mut body = Vec::new();
        if let Some(message) = &message {
            if y == height_body / 2 {
                let text = truncate(message, body_width.into());
                let spaces = (usize::from(body_width) - width(&text)) / 2;
                body.push(Span::body(format!("{}{text}", " ".repeat(spaces))));
            }
        } else if let Some(rows) = &state.rows {
            let row_index = state.offset.saturating_add(usize::from(y));
            if let Some(row) = rows.rows.get(row_index) {
                let cursor = state
                    .cursor
                    .and_then(|c| rows.row_of_target.get(c))
                    .copied()
                    == Some(row_index);
                body = body_line(row, body_width, state.hscroll, state.mode, cursor);
                hits.push(Hit {
                    y: y + 1,
                    x0: panel_width,
                    x1: columns,
                    action: Action::CursorToRow(row_index),
                });
            }
        }
        line.extend(fit_line(body, body_width.into()));
        lines.push(line);
    }
    if let Some(notice) = notice {
        lines.push(vec![Span::label(pad(&sanitize(&notice), columns.into()))]);
    }
    lines.push(vec![Span::label(pad(
        "j/k line  [ ] hunk  n/p file  t view  e files  r refresh  ? help  q quit",
        columns.into(),
    ))]);
    for hit in &mut hits {
        hit.x1 = hit.x1.min(columns);
    }
    hits.retain(|hit| hit.x0 < hit.x1);
    Rendered { lines, hits }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{RepoState, Snapshot};
    use crate::git::{ChangedFile, ChangedFileStatus};
    use crate::tui::state::tests::snapshot;
    use crate::tui::state::{FilesPanel, ViewState};

    fn files(mut s: Snapshot) -> Snapshot {
        s.files = vec![
            ChangedFile {
                path: "src/a.rs".into(),
                status: ChangedFileStatus::Modified,
                staged: true,
                insertions: Some(1),
                deletions: Some(1),
            },
            ChangedFile {
                path: "a.rs".into(),
                status: ChangedFileStatus::Modified,
                staged: false,
                insertions: Some(4),
                deletions: Some(3),
            },
        ];
        s
    }

    fn rendered(width: u16, height: u16, panel: FilesPanel) -> (Rendered, ViewState) {
        let snap = files(snapshot("a.rs", "r1", &[(10, " --+ ")]));
        let mut st = ViewState::new(ViewMode::Unified, panel, true);
        st.resize(body_height(&st, &snap, height));
        st.reconcile(&snap);
        (render(&snap, &st, width, height), st)
    }

    #[test]
    fn the_toolbar_shows_both_steppers_and_every_item_is_clickable() {
        let (r, _) = rendered(120, 20, FilesPanel::Hidden);
        let bar = &r.plain()[0];
        for piece in [
            "‹ a.rs 2/2 ›",
            "{} 1/1 ↑ ↓",
            "unified",
            "UNSTAGED",
            "+4 −3",
            "files",
            "⟳",
        ] {
            assert!(bar.contains(piece), "toolbar `{bar}` lacks `{piece}`");
        }
        for action in [
            Action::PrevFile,
            Action::NextFile,
            Action::PrevHunk,
            Action::NextHunk,
            Action::ToggleView,
            Action::ToggleFiles,
            Action::Refresh,
        ] {
            assert!(r.hits.iter().any(|h| h.y == 0
                && std::mem::discriminant(&h.action) == std::mem::discriminant(&action)));
        }
    }

    #[test]
    fn a_narrow_toolbar_drops_items_from_the_right_and_keeps_the_steppers() {
        let (r, _) = rendered(50, 20, FilesPanel::Hidden);
        let bar = &r.plain()[0];
        assert!(bar.contains("‹ a.rs 2/2 ›") && bar.contains("{} 1/1"));
        assert!(!bar.contains("⟳") && !bar.contains("files"));
    }

    #[test]
    fn the_unified_body_shows_numbers_signs_gap_and_cursor() {
        let (r, st) = rendered(80, 20, FilesPanel::Hidden);
        let text = r.plain();
        assert!(text[1].starts_with("a.rs"));
        assert!(text[2].contains("··· 9 unmodified lines ···"));
        assert!(text[3].contains("@@ -10 +10 @@"));
        assert!(text[5].contains("11") && text[5].contains("- line -"));
        assert!(text[7].contains("11") && text[7].contains("+ line +"));
        assert_eq!(st.cursor, Some(1));
        assert!(
            r.lines[5].iter().all(|span| span.style.reverse),
            "the cursor row is reverse video"
        );
    }

    #[test]
    fn the_pinned_files_panel_lists_rows_and_is_clickable() {
        let (r, _) = rendered(120, 20, FilesPanel::Pinned);
        let text = r.plain();
        assert!(text[1].starts_with("CHANGED 2"));
        assert!(
            text[2].contains("M a.rs src/") && text[2].contains("S"),
            "{}",
            text[2]
        );
        assert!(text[3].starts_with("▸M a.rs ./"), "{}", text[3]);
        assert!(r
            .hits
            .iter()
            .any(|h| matches!(h.action, Action::SelectFile(0)) && h.y == 2));
    }

    #[test]
    fn empty_and_error_states_are_one_centred_line() {
        let mut snap = Snapshot::empty("/x");
        snap.revision = 1;
        let st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        assert!(render(&snap, &st, 80, 12)
            .plain()
            .iter()
            .any(|l| l.contains("not a git repository")));
        snap.repo = RepoState::Repo {
            toplevel: "/x".into(),
            branch: None,
            worktree: None,
        };
        assert!(render(&snap, &st, 80, 12)
            .plain()
            .iter()
            .any(|l| l.contains("working tree clean")));
        snap.repo = RepoState::Unusable {
            reason: "git 2.20 is too old".into(),
        };
        assert!(render(&snap, &st, 80, 12)
            .plain()
            .iter()
            .any(|l| l.contains("git 2.20 is too old")));
        snap.watcher_error = Some("inotify limit".into());
        assert!(render(&snap, &st, 80, 12)
            .plain()
            .iter()
            .any(|l| l.contains("live refresh degraded: inotify limit")));
    }

    #[test]
    fn a_conflict_is_named_and_a_status_failure_stays_visible() {
        let mut snap = files(snapshot(
            "a.rs",
            "diff --cc a.rs\nindex 1,2..3\n@@@ -1,1 -1,1 +1,3 @@@\n",
            &[],
        ));
        let st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        assert!(render(&snap, &st, 80, 12)
            .plain()
            .iter()
            .any(|l| l.contains("unmerged path")));
        snap.status_error = Some("git command timed out after 30s".into());
        let text = render(&snap, &st, 80, 12).plain();
        assert!(
            text.iter()
                .any(|l| l.contains("status failed: git command timed out after 30s")),
            "{text:?}"
        );
    }

    #[test]
    fn a_tiny_terminal_draws_one_line() {
        let (r, _) = rendered(30, 5, FilesPanel::Hidden);
        assert_eq!(r.plain(), vec!["terminal too small".to_string()]);
    }

    #[test]
    fn long_names_keep_the_file_arrows_visible_and_hits_inside_the_frame() {
        let name = "猫".repeat(80);
        let mut snap = files(snapshot(&name, "r1", &[(1, "+")]));
        snap.files[1].path = name;
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        st.resize(body_height(&st, &snap, 10));
        st.reconcile(&snap);
        let r = render(&snap, &st, 40, 10);
        assert_eq!(r.lines.len(), 10);
        for line in &r.lines {
            let text: String = line.iter().map(|s| s.text.as_str()).collect();
            assert_eq!(width(&text), 40);
        }
        assert!(r.hits.iter().any(|h| h.action == Action::NextFile));
        for hit in &r.hits {
            assert!(hit.x0 < hit.x1 && hit.x1 <= 40 && hit.y < 10);
            assert_eq!(r.hit(hit.x0, hit.y), Some(&hit.action));
            assert_ne!(r.hit(hit.x1, hit.y), Some(&hit.action));
        }
    }

    #[test]
    fn split_rows_scroll_by_cells_and_reverse_the_filler_too() {
        let row = Row::Split {
            left: Some(crate::tui::rows::Cell {
                target: Some(0),
                no: Some(11),
                sign: '-',
                text: "猫ab".into(),
            }),
            right: None,
        };
        let line = body_line(&row, 41, 1, ViewMode::Split, true);
        let text: String = line.iter().map(|s| s.text.as_str()).collect();
        let (left, right) = text.split_once('│').unwrap();
        assert_eq!(left.trim_end(), "   11 -  ab");
        assert_eq!(width(left), 20);
        assert_eq!(right, " ".repeat(20));
        assert!(line.iter().all(|s| s.style.reverse));
        assert!(line
            .iter()
            .any(|s| s.style.role == Role::Label && s.text.contains("11")));
        assert!(line.iter().any(|s| s.style.semantic == Some(Semantic::Bad)));
    }

    #[test]
    fn panel_directories_are_dim_and_the_footer_has_totals() {
        let (r, _) = rendered(120, 20, FilesPanel::Pinned);
        assert!(r.lines[2]
            .iter()
            .any(|s| s.text.contains("src/") && s.style.role == Role::Label));
        assert!(r.lines[3]
            .iter()
            .any(|s| s.text.contains("./") && s.style.role == Role::Label));
        assert!(r.plain()[18].contains("+5 −4 · 2 files"));
        assert_eq!(r.hit(FILES_WIDTH, 2), None);
        assert_eq!(r.hit(FILES_WIDTH + 1, 2), Some(&Action::CursorToRow(1)));
    }

    #[test]
    fn errors_and_file_text_are_sanitized_and_notices_keep_the_body() {
        let name = "a\x1b\u{202e}.rs";
        let mut snap = files(snapshot(name, "r1", &[(1, "+")]));
        snap.files[1].path = name.into();
        snap.status_error = Some("status\x1b\u{2066}".into());
        snap.watcher_error = Some("watcher\x07".into());
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Shown, true);
        st.resize(body_height(&st, &snap, 12));
        st.reconcile(&snap);
        let r = render(&snap, &st, 100, 12);
        let text = r.plain().join(" ");
        assert!(!text.contains('\x1b') && !text.contains('\u{202e}') && !text.contains('\u{2066}'));
        assert!(text.contains("a\u{241b}\u{fffd}.rs"));
        assert!(text.contains("line +"));
        assert!(r.plain()[10].contains("status failed: status\u{241b}\u{fffd}"));
        snap.status_error = None;
        assert!(render(&snap, &st, 100, 12).plain()[10].contains("watcher\u{2407}"));
        snap.watcher_error = None;
        snap.diff = DiffState::Failed("diff\x1b failed".into());
        assert!(render(&snap, &st, 100, 12)
            .plain()
            .iter()
            .any(|s| s.contains("diff\u{241b} failed")));
    }
}
