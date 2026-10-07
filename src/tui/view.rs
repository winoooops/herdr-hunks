//! Pure view: snapshot + view state in, styled lines and hit regions out.
use crate::engine::nav::ViewMode;
use crate::engine::{DiffState, FileKey, RepoState, Scope, Snapshot};
use crate::git::{ChangedFile, ChangedFileStatus};
use crate::tui::format::{pad, truncate, width};
use crate::tui::rows::Row;
use crate::tui::sanitize::sanitize;
use crate::tui::state::{FilesPanel, ViewState};
use crate::tui::style::{Line, Role, Semantic, Span, Style};
use crate::tui::{cards, dialog, keys, layout, picker, review};

pub const FILES_WIDTH: u16 = 18;
pub const MIN_SPLIT_WIDTH: u16 = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    PrevFile,
    NextFile,
    PrevHunk,
    NextHunk,
    ToggleScope,
    ToggleView,
    ToggleFiles,
    Refresh,
    StageHunk,
    DiscardHunk,
    DiscardFile,
    SelectFile(usize),
    CursorToRow(usize),
    PickRow(usize),
    PickPane,
    PickPaneRow(usize),
    EditorCategory(crate::engine::comments::Category),
}

impl Action {
    pub fn key_action(&self) -> Option<keys::KeyAction> {
        use keys::KeyAction;
        Some(match self {
            Self::PrevFile => KeyAction::FilePrev,
            Self::NextFile => KeyAction::FileNext,
            Self::PrevHunk => KeyAction::HunkPrev,
            Self::NextHunk => KeyAction::HunkNext,
            Self::ToggleScope => KeyAction::ToggleScope,
            Self::ToggleView => KeyAction::ToggleView,
            Self::ToggleFiles => KeyAction::ToggleFiles,
            Self::Refresh => KeyAction::Refresh,
            Self::PickPane => KeyAction::PickPane,
            Self::StageHunk => KeyAction::StageHunk,
            Self::DiscardHunk => KeyAction::DiscardHunk,
            Self::DiscardFile => KeyAction::DiscardFile,
            Self::SelectFile(_)
            | Self::CursorToRow(_)
            | Self::PickRow(_)
            | Self::PickPaneRow(_)
            | Self::EditorCategory(_) => return None,
        })
    }
}

pub struct Hit {
    pub y: u16,
    pub x0: u16,
    pub x1: u16,
    pub action: Action,
}

impl Hit {
    fn hovered(&self, state: &ViewState) -> bool {
        state.mouse_requested
            && !state.help_open
            && state.picker.is_none()
            && state.panes.is_none()
            && state.confirm.is_none()
            && state.review_box.is_none()
            && state.editor.is_none()
            && state
                .hover
                .is_some_and(|(x, y)| self.y == y && x >= self.x0 && x < self.x1)
    }
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
    if let Some(notice) = state.notice.as_ref().filter(|n| n.urgent) {
        return Some(notice.text.clone());
    }
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
    state.notice.as_ref().map(|n| n.text.clone())
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

/// A toolbar item carries its text pieces, drop order, optional tone and dim flag.
type ToolbarItem = (Vec<(String, Option<Action>)>, u8, Option<Semantic>, bool);

/// Toolbar items left to right; a higher drop order drops first.
fn toolbar_items(snapshot: &Snapshot, state: &ViewState, total_width: u16) -> Vec<ToolbarItem> {
    let total = snapshot.files.len();
    let index = snapshot
        .selected
        .as_ref()
        .and_then(|k| snapshot.files.iter().position(|f| FileKey::of(f) == *k));
    let name = snapshot
        .selected
        .as_ref()
        .map(|k| sanitize(k.path.rsplit('/').next().unwrap_or(&k.path)))
        .unwrap_or_else(|| "no file".into());
    let position = format!(" {}/{} ", index.map(|i| i + 1).unwrap_or(0), total);
    let name = truncate(
        &name,
        usize::from(total_width).saturating_sub(11 + width(&position)),
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
    let scope_chip = match (snapshot.scope, &snapshot.base) {
        (Scope::Branch, Some(base)) => {
            let names_mark = snapshot
                .mark
                .as_ref()
                .is_some_and(|mark| mark.commit == base.requested);
            let label = if names_mark { "reviewed" } else { base.label() };
            format!("vs {}", truncate(&sanitize(label), 16))
        }
        _ => "worktree".to_string(),
    };
    let mut items: Vec<ToolbarItem> = vec![
        (
            vec![
                ("‹".into(), Some(Action::PrevFile)),
                (format!(" {name}{position}"), None),
                ("›".into(), Some(Action::NextFile)),
            ],
            0,
            None,
            false,
        ),
        (
            vec![
                (format!("{{}} {hunk_pos}/{hunk_total} "), None),
                ("↑".into(), Some(Action::PrevHunk)),
                (" ".into(), None),
                ("↓".into(), Some(Action::NextHunk)),
            ],
            1,
            None,
            false,
        ),
        (
            vec![(scope_chip, Some(Action::ToggleScope))],
            3,
            None,
            false,
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
            4,
            None,
            false,
        ),
    ];
    if snapshot.scope == Scope::Worktree {
        // The group of spec 9.3 sits between the steppers and is the first item dropped.
        let ready = matches!(snapshot.diff, DiffState::Ready(_));
        let stage = if staged { "unstage" } else { "stage" };
        let chip = |text: &str, action: Action| (text.to_string(), ready.then_some(action));
        items.insert(
            1,
            (
                vec![
                    chip(stage, Action::StageHunk),
                    (" ".into(), None),
                    chip("discard", Action::DiscardHunk),
                    (" ".into(), None),
                    chip("discard file", Action::DiscardFile),
                ],
                9,
                None,
                false,
            ),
        );
        items.push((
            vec![(if staged { "STAGED" } else { "UNSTAGED" }.into(), None)],
            5,
            None,
            false,
        ));
    }
    items.push((vec![(stats, None)], 6, None, false));
    let (chip_text, tone, dim) = crate::tui::panes::chip(snapshot);
    items.push((vec![(chip_text, Some(Action::PickPane))], 2, tone, dim));
    items.push((
        vec![("files".into(), Some(Action::ToggleFiles))],
        7,
        None,
        false,
    ));
    items.push((vec![(busy.into(), Some(Action::Refresh))], 8, None, false));
    items
}

fn toolbar(snapshot: &Snapshot, state: &ViewState, total_width: u16) -> (Line, Vec<Hit>) {
    let mut items = toolbar_items(snapshot, state, total_width);
    for (item, _, _, _) in &mut items {
        for (text, action) in item {
            if action.is_some() {
                *text = format!(" {text} ");
            }
        }
    }
    let item_width =
        |item: &Vec<(String, Option<Action>)>| item.iter().map(|(t, _)| width(t)).sum::<usize>();
    // drop from the right until it fits; steppers (drop order 0 and 1) go last
    while items.len() > 1
        && 1 + items
            .iter()
            .map(|(i, _, _, _)| item_width(i) + 3)
            .sum::<usize>()
            > usize::from(total_width)
    {
        let worst = items
            .iter()
            .enumerate()
            .max_by_key(|(_, (_, order, _, _))| *order)
            .map(|(i, _)| i)
            .unwrap();
        items.remove(worst);
    }
    let mut line: Line = vec![Span::body(" ")];
    let mut hits = Vec::new();
    let mut x = 1u16;
    for (item, _, tone, dim) in items {
        for (text, action) in item {
            let w = width(&text) as u16;
            if let Some(action) = action {
                let enabled = match action {
                    Action::PrevFile | Action::NextFile => snapshot.files.len() >= 2,
                    Action::PrevHunk | Action::NextHunk => {
                        matches!(&snapshot.diff, DiffState::Ready(d) if d.file_diff.hunks.len() >= 2)
                    }
                    Action::ToggleView => {
                        total_width >= MIN_SPLIT_WIDTH || state.requested_mode == ViewMode::Split
                    }
                    Action::ToggleScope => snapshot.base.is_some(),
                    _ => true,
                };
                if enabled {
                    let hit = Hit {
                        y: 0,
                        x0: x,
                        x1: x + w,
                        action,
                    };
                    line.push(Span::new(
                        text,
                        if dim {
                            Style::role(Role::Label)
                        } else {
                            Style {
                                reverse: true,
                                semantic: if hit.hovered(state) {
                                    None
                                } else {
                                    Some(tone.unwrap_or(Semantic::Accent))
                                },
                                ..Style::role(Role::Emphasis)
                            }
                        },
                    ));
                    hits.push(hit);
                } else {
                    line.push(Span::label(text));
                }
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
    if snapshot.revision == 0 || matches!(snapshot.repo, RepoState::NotARepo { .. }) {
        if let Some(error) = &snapshot.status_error {
            return Some(sanitize(error));
        }
    }
    if snapshot.revision == 0 {
        return Some("loading…".into());
    }
    match &snapshot.repo {
        RepoState::Unusable { reason } => return Some(sanitize(reason)),
        RepoState::NotARepo { .. } => return Some("not a git repository".into()),
        RepoState::Repo { .. } => {}
    }
    if snapshot.files.is_empty() {
        if let Some(error) = &snapshot.status_error {
            return Some(sanitize(error));
        }
        return Some(match (snapshot.scope, &snapshot.base) {
            (Scope::Branch, Some(base)) => format!(
                "nothing on this branch since {}",
                truncate(&sanitize(base.label()), 16)
            ),
            _ => "working tree clean".into(),
        });
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

/// Flag unread paths, or every tracked row when the mark cannot divide this history.
fn unread_marker(snapshot: &Snapshot, file: &ChangedFile) -> bool {
    use crate::engine::MarkState;
    if snapshot.scope != Scope::Branch || matches!(file.status, ChangedFileStatus::Untracked) {
        return false;
    }
    match snapshot.mark.as_ref().map(|mark| &mark.state) {
        None => false,
        Some(MarkState::Current) => {
            snapshot.unread.contains(&file.path)
                || snapshot
                    .rename_sources
                    .get(&file.path)
                    .is_some_and(|old| snapshot.unread.contains(old))
        }
        Some(_) => true,
    }
}

fn files_lines(snapshot: &Snapshot, state: &ViewState, height: u16) -> (Vec<Line>, Vec<Hit>) {
    let mut lines = Vec::new();
    let mut hits = Vec::new();
    if height == 0 {
        return (lines, hits);
    }
    let pending = snapshot.comments.iter().filter(|c| c.is_pending()).count();
    let mut heading = format!("CHANGED {}", snapshot.files.len());
    if pending > 0 {
        heading.push_str(&format!(" · ✎ {pending}"));
    }
    lines.push(vec![Span::emphasis(pad(&heading, FILES_WIDTH.into()))]);
    let selected = snapshot.selected.as_ref().and_then(|key| {
        snapshot
            .files
            .iter()
            .position(|file| FileKey::of(file) == *key)
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
        let commented = snapshot.comments.iter().any(|c| {
            c.anchor.key.path == file.path
                && matches!(
                    c.anchor.comparison,
                    crate::engine::comments::AnchorComparison::Branch { .. }
                ) == (snapshot.scope == Scope::Branch)
        });
        let mut line = fit_line(line, usize::from(FILES_WIDTH - 2 - u16::from(commented)));
        if commented {
            line.push(Span::body("✎"));
        }
        line.push(if unread_marker(snapshot, file) {
            Span::new("● ", Style::semantic(Role::Emphasis, Semantic::Accent))
        } else {
            Span::body(if file.staged { "S " } else { "  " })
        });
        let hit = Hit {
            y: lines.len() as u16 + 1,
            x0: 0,
            x1: FILES_WIDTH,
            action: Action::SelectFile(i),
        };
        if hit.hovered(state) {
            for span in &mut line {
                span.style.role = Role::Emphasis;
            }
        }
        hits.push(hit);
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

fn clip_line(line: &Line, start: usize, cells: usize) -> Line {
    use unicode_width::UnicodeWidthChar;
    let end = start.saturating_add(cells);
    let (mut position, mut used, mut kept) = (0usize, 0usize, false);
    let mut clipped = Vec::new();
    for span in line {
        let mut text = String::new();
        for ch in span.text.chars() {
            let size = ch.width().unwrap_or(0);
            if size == 0 {
                if kept {
                    text.push(ch);
                }
                continue;
            }
            let next = position + size;
            let visible = next.min(end).saturating_sub(position.max(start));
            kept = visible == size;
            if kept {
                text.push(ch);
            } else {
                text.push_str(&" ".repeat(visible));
            }
            used += visible;
            position = next;
        }
        if !text.is_empty() {
            clipped.push(Span::new(text, span.style));
        }
    }
    if used < cells {
        clipped.push(Span::body(" ".repeat(cells - used)));
    }
    clipped
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

fn body_line(
    row: &Row,
    columns: u16,
    hscroll: usize,
    mode: ViewMode,
    cursor: bool,
    selected: [bool; 2],
    editor: Option<(&review::Editor, &[cards::CardLine])>,
) -> Line {
    let cells = usize::from(columns);
    let number = |no: Option<u32>| no.map(|n| n.to_string()).unwrap_or_default();
    let line_style = |sign| match sign {
        '+' => Style::semantic(Role::Body, Semantic::Good),
        '-' => Style::semantic(Role::Body, Semantic::Bad),
        _ => Style::role(Role::Body),
    };
    let line = match row {
        Row::Orphans { count } => vec![Span::label(format!(
            "✎ on changes no longer shown ({count})"
        ))],
        Row::Editor { line } => {
            let card_width = cards::card_width(cells, mode);
            let mut framed = vec![Span::body(" ".repeat(cells.saturating_sub(card_width)))];
            if let Some((editor, lines)) = editor {
                framed.extend(match lines.get(*line) {
                    Some(cards::CardLine::Top { .. }) => {
                        let mut title = editor.title();
                        if !editor.at_limit {
                            let used: usize = title.iter().map(|s| width(&s.text)).sum();
                            if let Some(room) = card_width
                                .saturating_sub(5)
                                .checked_sub(used + width(" ctrl+h/l"))
                            {
                                title.push(Span::label(format!("{} ctrl+h/l", " ".repeat(room))));
                            }
                        }
                        cards::frame_top(title, card_width)
                    }
                    Some(cards::CardLine::Text(text)) => cards::frame_text(text, card_width),
                    Some(cards::CardLine::Bottom) => cards::frame_bottom(
                        if editor.pending.is_some() {
                            "saving…"
                        } else {
                            review::EDITOR_FOOTER
                        },
                        card_width,
                    ),
                    None => Vec::new(),
                });
            }
            framed
        }
        Row::Card { line, .. } => {
            let card_width = crate::tui::cards::card_width(cells, mode);
            let mut framed = vec![Span::body(" ".repeat(cells.saturating_sub(card_width)))];
            framed.extend(match line {
                crate::tui::cards::CardLine::Top { title, tone, dim } => {
                    crate::tui::cards::frame_top(
                        vec![Span::new(
                            title,
                            Style {
                                semantic: *tone,
                                role: if *dim { Role::Label } else { Role::Emphasis },
                                ..Style::role(Role::Body)
                            },
                        )],
                        card_width,
                    )
                }
                crate::tui::cards::CardLine::Text(text) => {
                    crate::tui::cards::frame_text(text, card_width)
                }
                crate::tui::cards::CardLine::Bottom => {
                    crate::tui::cards::frame_bottom("", card_width)
                }
            });
            framed
        }
        Row::FileHeader { path } => vec![Span::emphasis(scroll_text(path, hscroll))],
        Row::HunkHeader { text, .. } => vec![Span::new(
            scroll_text(text, hscroll),
            Style::semantic(Role::Label, Semantic::Accent),
        )],
        Row::Gap { lines } => vec![Span::label(format!(
            "··· {lines} unmodified line{} ···",
            if *lines == 1 { "" } else { "s" }
        ))],
        Row::Truncated { lines } => vec![Span::label(format!(
            "… {lines} more line{} not shown",
            if *lines == 1 { "" } else { "s" }
        ))],
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
                Style {
                    reverse: selected[0],
                    ..line_style(*sign)
                },
            ),
        ],
        Row::Split { left, right } => {
            let half = cells.saturating_sub(1) / 2;
            let side = |cell: &Option<crate::tui::rows::Cell>, selected: bool| {
                fit_line(
                    match cell {
                        Some(cell) => vec![
                            Span::label(format!("{:>5} ", number(cell.no))),
                            Span::new(
                                format!("{} {}", cell.sign, scroll_text(&cell.text, hscroll)),
                                Style {
                                    reverse: selected,
                                    ..line_style(cell.sign)
                                },
                            ),
                        ],
                        None => Vec::new(),
                    },
                    half,
                )
            };
            let mut line = side(left, selected[0]);
            line.push(Span::new("│", Style::role(Role::Rule)));
            line.extend(side(right, selected[1]));
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

/// Whether the frame shows loaded content or an empty list without a covering modal.
pub fn body_is_drawn(state: &ViewState, snapshot: &Snapshot, columns: u16, height: u16) -> bool {
    columns >= 40
        && height >= 10
        && !state.help_open
        && state.picker.is_none()
        && state.panes.is_none()
        && state.confirm.is_none()
        && state.review_box.is_none()
        && (matches!(&snapshot.diff, DiffState::Ready(_))
            || snapshot.files.is_empty()
            || state.editor.is_some())
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
    let message = if state.rows.as_ref().is_some_and(|rows| {
        rows.rows
            .iter()
            .any(|row| matches!(row, Row::Card { .. } | Row::Editor { .. }))
    }) {
        None
    } else {
        state_message(snapshot)
    };
    let selected: std::collections::HashSet<usize> =
        match (&snapshot.diff, &state.visual, state.cursor) {
            (DiffState::Ready(diff), Some(visual), Some(cursor))
                if std::sync::Arc::ptr_eq(diff, &visual.diff) =>
            {
                review::selection(diff, visual, cursor)
                    .map(|(side, start, end)| {
                        diff.targets
                            .iter()
                            .enumerate()
                            .filter(|(_, t)| {
                                t.side == side && t.line_number >= start && t.line_number <= end
                            })
                            .map(|(i, _)| i)
                            .collect()
                    })
                    .unwrap_or_default()
            }
            _ => Default::default(),
        };
    let editor_lines = state
        .editor
        .as_ref()
        .map(|e| e.lines(cards::card_width(usize::from(body_width), state.mode)));
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
                let cursor = if let Some(orphan) = state.orphan {
                    matches!(row, Row::Card { id, .. } if rows.orphan_tops.get(orphan).is_some_and(|(_, c)| id == c))
                } else {
                    state.visual.is_none()
                        && state
                            .cursor
                            .and_then(|c| rows.row_of_target.get(c))
                            .copied()
                            == Some(row_index)
                };
                let selection = match row {
                    Row::Unified { target, .. } => [selected.contains(target), false],
                    Row::Split { left, right } => [left, right].map(|cell| {
                        cell.as_ref()
                            .and_then(|c| c.target)
                            .is_some_and(|t| selected.contains(&t))
                    }),
                    _ => [false; 2],
                };
                body = body_line(
                    row,
                    body_width,
                    state.hscroll,
                    state.mode,
                    cursor,
                    selection,
                    state.editor.as_ref().zip(editor_lines.as_deref()),
                );
                if matches!(row, Row::Editor { line: 0 }) {
                    let mut x = usize::from(panel_width);
                    for span in &body {
                        let end = x + width(&span.text);
                        if let Some(category) = crate::engine::comments::Category::ALL
                            .iter()
                            .find(|c| c.short() == span.text)
                        {
                            if end <= usize::from(columns) {
                                hits.push(Hit {
                                    y: y + 1,
                                    x0: x as u16,
                                    x1: end as u16,
                                    action: Action::EditorCategory(*category),
                                });
                            }
                        }
                        x = end;
                    }
                } else if !matches!(row, Row::Editor { .. }) {
                    hits.push(Hit {
                        y: y + 1,
                        x0: panel_width,
                        x1: columns,
                        action: Action::CursorToRow(row_index),
                    });
                }
            }
        }
        line.extend(fit_line(body, body_width.into()));
        lines.push(line);
    }
    if let Some(notice) = notice {
        lines.push(vec![Span::label(pad(&sanitize(&notice), columns.into()))]);
    }
    let mut hints = vec![
        "j/k line",
        "[ ] hunk",
        "n/p file",
        "t view",
        "e files",
        "r refresh",
    ];
    if snapshot.scope == Scope::Worktree {
        hints.extend(["s stage", "d discard", "D file"]);
    }
    let (pending, _, _) = review::counts(snapshot);
    let finish = format!("Y finish ({pending})");
    if pending > 0 {
        hints.push(&finish);
    }
    hints.push("@ request");
    hints.extend(["? help", "q quit"]);
    while width(&hints.join("  ")) > usize::from(columns) {
        // Keep help and quit until the other hints have gone.
        hints.remove(hints.len().saturating_sub(3));
    }
    let hint = hits
        .iter()
        .find(|hit| hit.hovered(state))
        .and_then(|hit| hit.action.key_action())
        .and_then(|action| keys::KEYS.iter().find(|binding| binding.action == action))
        .map(|binding| {
            let hint = format!("{} · {}", binding.label, binding.key);
            if binding.action == keys::KeyAction::ToggleScope {
                if let Some(base) = &snapshot.base {
                    let commit: String = base.commit.chars().take(7).collect();
                    let names_mark = snapshot
                        .mark
                        .as_ref()
                        .is_some_and(|mark| mark.commit == base.requested);
                    let label = if names_mark { "reviewed" } else { base.label() };
                    return format!("{hint} · {} @ {}", sanitize(label), sanitize(&commit));
                }
            }
            hint
        })
        .unwrap_or_else(|| hints.join("  "));
    lines.push(vec![Span::label(pad(&hint, columns.into()))]);
    for hit in &mut hits {
        hit.x1 = hit.x1.min(columns);
    }
    hits.retain(|hit| hit.x0 < hit.x1);
    if state.help_open {
        let panel_width = columns.min(60);
        let panel_height = height.saturating_sub(2);
        let x = usize::from((columns - panel_width) / 2);
        let mut panel = keys::help_panel(state.popup);
        panel.offset = layout::clamp_scroll(
            state.help_offset,
            dialog::line_count(&panel, panel_width),
            panel_height.saturating_sub(4),
        );
        for (y, overlay) in dialog::render(&panel, panel_width, panel_height)
            .into_iter()
            .enumerate()
        {
            let background = &lines[y + 1];
            let mut line = clip_line(background, 0, x);
            line.extend(overlay);
            let right = x + usize::from(panel_width);
            line.extend(clip_line(background, right, usize::from(columns) - right));
            lines[y + 1] = line;
        }
        hits.clear();
    }
    if let Some(picker) = &state.picker {
        let panel_width = columns.min(picker::WIDTH);
        let panel_height = height.saturating_sub(2);
        let x = usize::from((columns - panel_width) / 2);
        let panel = picker.panel(snapshot, panel_width, panel_height);
        for (y, overlay) in dialog::render(&panel, panel_width, panel_height)
            .into_iter()
            .enumerate()
        {
            let background = &lines[y + 1];
            let mut line = clip_line(background, 0, x);
            line.extend(overlay);
            let right = x + usize::from(panel_width);
            line.extend(clip_line(background, right, usize::from(columns) - right));
            lines[y + 1] = line;
        }
        hits.clear();
        // One hit per drawn list row: every panel row is one line, so row i is overlay line i + 1.
        let head = panel
            .rows
            .iter()
            .take_while(|r| !matches!(r, dialog::Row::Entry { .. }))
            .count();
        let first = picker.window(picker.visible(panel_height));
        let listed = panel.rows.len() - head;
        for i in 0..listed {
            hits.push(Hit {
                y: (2 + head + i) as u16,
                x0: x as u16,
                x1: (x + usize::from(panel_width)) as u16,
                action: Action::PickRow(first + i),
            });
        }
    }
    if let Some(picker) = &state.panes {
        let panel_width = columns.min(crate::tui::panes::WIDTH);
        let panel_height = height.saturating_sub(2);
        let x = usize::from((columns - panel_width) / 2);
        let panel = picker.panel(snapshot, panel_width, panel_height);
        for (y, overlay) in dialog::render(&panel, panel_width, panel_height)
            .into_iter()
            .enumerate()
        {
            let background = &lines[y + 1];
            let mut line = clip_line(background, 0, x);
            line.extend(overlay);
            let right = x + usize::from(panel_width);
            line.extend(clip_line(background, right, usize::from(columns) - right));
            lines[y + 1] = line;
        }
        hits.clear();
        let first = picker.window(picker.visible(snapshot, panel_width, panel_height));
        let listed = panel
            .rows
            .iter()
            .filter(|row| matches!(row, dialog::Row::Entry { .. }))
            .count();
        for index in first..first + listed {
            if let Some(line) = picker.panel_line(snapshot, panel_width, panel_height, index) {
                hits.push(Hit {
                    y: (2 + line) as u16,
                    x0: x as u16,
                    x1: (x + usize::from(panel_width)) as u16,
                    action: Action::PickPaneRow(index),
                });
            }
        }
    }
    if let Some(confirm) = &state.confirm {
        let panel_width = columns.min(60);
        let panel = confirm.panel(panel_width);
        let panel_height =
            (dialog::line_count(&panel, panel_width) + 4).min(usize::from(height) - 2) as u16;
        let x = usize::from((columns - panel_width) / 2);
        for (y, overlay) in dialog::render(&panel, panel_width, panel_height)
            .into_iter()
            .enumerate()
        {
            let background = &lines[y + 1];
            let mut line = clip_line(background, 0, x);
            line.extend(overlay);
            let right = x + usize::from(panel_width);
            line.extend(clip_line(background, right, usize::from(columns) - right));
            lines[y + 1] = line;
        }
        hits.clear();
    }
    if let Some(b) = &state.review_box {
        let panel_width = columns.min(72);
        let panel = b.panel(snapshot, panel_width);
        let panel_height =
            (dialog::line_count(&panel, panel_width) + 4).min(usize::from(height) - 2) as u16;
        let x = usize::from((columns - panel_width) / 2);
        let scope = match panel.rows.first() {
            Some(dialog::Row::Text(text)) if b.offers(snapshot).scope_line => Some(text.as_str()),
            _ => None,
        };
        for (y, mut overlay) in dialog::render(&panel, panel_width, panel_height)
            .into_iter()
            .enumerate()
        {
            if let Some(scope) = scope {
                let inner = usize::from(panel_width) - 4;
                if y > 0 && y <= scope.len().div_ceil(inner) {
                    let start = (y - 1) * inner;
                    let f = scope.find("f this file").unwrap();
                    let a = scope.find("a all changes").unwrap();
                    let file_available = snapshot.selected.as_ref().is_some_and(|key| {
                        b.scope_available(
                            snapshot,
                            &crate::engine::dispatch::ReviewScope::File(key.clone()),
                        )
                    });
                    for (from, to, selected, enabled) in [
                        (
                            f,
                            f + "f this file".len(),
                            matches!(b.scope, crate::engine::dispatch::ReviewScope::File(_)),
                            file_available,
                        ),
                        (
                            a,
                            scope.len(),
                            b.scope == crate::engine::dispatch::ReviewScope::All,
                            true,
                        ),
                    ] {
                        let from = from.max(start);
                        let to = to.min(start + inner);
                        if from < to {
                            let col = 3 + from - start;
                            let mut styled = clip_line(&overlay, 0, col);
                            let mut words = clip_line(&overlay, col, to - from);
                            for word in &mut words {
                                word.style.reverse = selected;
                                if !enabled {
                                    word.style.role = Role::Label;
                                }
                            }
                            styled.extend(words);
                            let right = col + to - from;
                            styled.extend(clip_line(
                                &overlay,
                                right,
                                usize::from(panel_width) - right,
                            ));
                            overlay = styled;
                        }
                    }
                }
            }
            let background = &lines[y + 1];
            let mut line = clip_line(background, 0, x);
            line.extend(overlay);
            let right = x + usize::from(panel_width);
            line.extend(clip_line(background, right, usize::from(columns) - right));
            lines[y + 1] = line;
        }
        hits.clear();
    }
    Rendered { lines, hits }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{RepoState, Snapshot};
    use crate::git::{ChangedFile, ChangedFileStatus};
    use crate::tui::state::tests::snapshot;
    use crate::tui::state::{FilesPanel, ViewState};

    #[test]
    fn the_first_frame_is_loading_until_a_snapshot_arrives() {
        let mut snap = Snapshot::empty("/r");
        let state = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        let text = render(&snap, &state, 80, 12).plain().join("\n");
        assert!(text.contains("loading…"));
        assert!(!text.contains("not a git repository"));
        snap.revision = 1;
        let text = render(&snap, &state, 80, 12).plain().join("\n");
        assert!(text.contains("not a git repository"));
        assert!(!text.contains("loading…"));
    }

    #[test]
    fn gap_and_truncation_labels_use_singular_for_one_line() {
        for (count, noun) in [(0, "lines"), (1, "line"), (2, "lines")] {
            for (row, expected) in [
                (
                    Row::Gap { lines: count },
                    format!("··· {count} unmodified {noun} ···"),
                ),
                (
                    Row::Truncated {
                        lines: count as usize,
                    },
                    format!("… {count} more {noun} not shown"),
                ),
            ] {
                let line = body_line(&row, 80, 0, ViewMode::Unified, false, [false; 2], None);
                let text: String = line.iter().map(|span| span.text.as_str()).collect();
                assert_eq!(text.trim_end(), expected);
            }
        }
    }

    #[test]
    fn help_does_not_add_ellipses_outside_the_panel() {
        let mut snap = Snapshot::empty("/r");
        snap.revision = 1;
        let mut state = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        state.help_open = true;
        for columns in [80, 120] {
            let rendered = render(&snap, &state, columns, 12);
            let left = usize::from((columns - 60) / 2);
            for row in &rendered.plain()[1..11] {
                let outside: String = row
                    .chars()
                    .take(left)
                    .chain(row.chars().skip(left + 60))
                    .collect();
                assert!(!outside.contains('…'), "{row}");
            }
        }
    }

    #[test]
    fn overlay_clipping_pads_partial_wide_cells_and_preserves_styles() {
        let line = vec![Span::emphasis("ab猫"), Span::label("cd")];
        for (start, cells, expected) in [
            (0, 3, "ab "),
            (3, 3, " cd"),
            (2, 3, "猫c"),
            (5, 3, "d  "),
            (0, 0, ""),
        ] {
            let clipped = clip_line(&line, start, cells);
            let text: String = clipped.iter().map(|span| span.text.as_str()).collect();
            assert_eq!(text, expected);
            assert_eq!(width(&text), cells);
        }
        let clipped = clip_line(&line, 3, 3);
        assert_eq!(clipped, vec![Span::emphasis(" "), Span::label("cd")]);
        let combining = clip_line(&vec![Span::body("a\u{301}猫\u{301}b")], 0, 2);
        assert_eq!(combining, vec![Span::body("a\u{301} ")]);
    }

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
        let snap = files(snapshot("a.rs", "r1", &[(10, " --+ "), (30, "+")]));
        let mut st = ViewState::new(ViewMode::Unified, panel, true);
        st.resize(width, body_height(&st, &snap, height));
        st.reconcile(&snap);
        (render(&snap, &st, width, height), st)
    }

    #[test]
    fn the_toolbar_shows_both_steppers_and_every_item_is_clickable() {
        let (r, _) = rendered(120, 20, FilesPanel::Hidden);
        let bar = &r.plain()[0];
        for piece in [
            "‹  a.rs 2/2  ›",
            "{} 1/2  ↑   ↓",
            "worktree",
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
        let (r, _) = rendered(65, 20, FilesPanel::Hidden);
        let bar = &r.plain()[0];
        assert!(bar.contains("‹  a.rs 2/2  ›") && bar.contains("{} 1/2"));
        assert!(bar.contains("worktree") && !bar.contains("unified"));
        assert!(!bar.contains("⟳") && !bar.contains("files"));
    }

    #[test]
    fn footer_drops_whole_hints_and_preserves_help_and_quit() {
        let (narrow, _) = rendered(40, 12, FilesPanel::Hidden);
        let text = narrow.plain();
        let footer = text.last().unwrap();
        assert!(!footer.contains('…'), "{footer}");
        assert_eq!(footer, "j/k line  [ ] hunk  ? help  q quit");
        let (wide, _) = rendered(120, 12, FilesPanel::Hidden);
        let text = wide.plain();
        let footer = text.last().unwrap();
        for hint in [
            "j/k line",
            "[ ] hunk",
            "n/p file",
            "t view",
            "e files",
            "r refresh",
            "s stage",
            "d discard",
            "D file",
            "? help",
            "q quit",
        ] {
            assert!(footer.contains(hint), "{footer} lacks {hint}");
        }
    }

    #[test]
    fn help_is_centred_and_fits_after_resizing_with_a_notice() {
        let mut snap = files(snapshot("猫.rs", "r1", &[(1, "+")]));
        snap.watcher_error = Some("unavailable".into());
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Shown, true);
        st.help_open = true;
        st.help_offset = 18;
        let full_height = dialog::line_count(&keys::help_panel(false), 60) as u16 + 7;
        for (columns, height) in [(40, 10), (61, 12), (120, full_height)] {
            st.resize(columns, body_height(&st, &snap, height));
            st.reconcile(&snap);
            let r = render(&snap, &st, columns, height);
            assert_eq!(r.lines.len(), usize::from(height));
            for line in &r.lines {
                let text: String = line.iter().map(|span| span.text.as_str()).collect();
                assert_eq!(width(&text), usize::from(columns));
            }
            let plain = r.plain();
            let title = &plain[1];
            let (left, _) = title.split_once("┌ Keys ").unwrap();
            assert_eq!(width(left), usize::from(columns.saturating_sub(60) / 2));
            assert!(r.hits.is_empty());
            if height == full_height {
                for binding in keys::KEYS {
                    assert!(plain.iter().any(|line| line.contains(binding.label)));
                }
            }
        }
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
        st.resize(40, body_height(&st, &snap, 10));
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
        let line = body_line(&row, 41, 1, ViewMode::Split, true, [false; 2], None);
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
        st.resize(100, body_height(&st, &snap, 12));
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

    #[test]
    fn toolbar_chips_are_padded_reversed_accent_bold_and_plain_text_is_not() {
        let mut snap = files(snapshot("a.rs", "chips", &[(1, "+"), (20, "+")]));
        for (mode, busy) in [(ViewMode::Unified, false), (ViewMode::Split, true)] {
            snap.refreshing = busy;
            let mut st = ViewState::new(mode, FilesPanel::Hidden, true);
            st.resize(160, 20);
            st.reconcile(&snap);
            let r = render(&snap, &st, 160, 22);
            let mut x = 0;
            let mut chips = 0;
            for span in &r.lines[0] {
                let end = x + width(&span.text) as u16;
                if let Some(action) = r.hit(x, 0) {
                    chips += 1;
                    assert!(
                        span.text.starts_with(' ') && span.text.ends_with(' '),
                        "{}",
                        span.text
                    );
                    if *action == Action::PickPane {
                        assert_eq!(span.text, " → no agent ");
                        assert_eq!(span.style, Style::role(Role::Label));
                    } else {
                        assert_eq!(
                            span.style,
                            Style {
                                reverse: true,
                                ..Style::semantic(Role::Emphasis, Semantic::Accent)
                            }
                        );
                    }
                    for cell in x..end {
                        assert_eq!(r.hit(cell, 0), Some(action));
                    }
                } else {
                    assert!(!span.style.reverse);
                }
                x = end;
            }
            // The seven viewer chips, three action chips and target chip.
            assert_eq!(chips, 11);
            assert!(r.lines[0]
                .iter()
                .any(|s| s.text == if busy { " … " } else { " ⟳ " }));
            assert!(r.lines[0].iter().any(|s| s.text
                == if mode == ViewMode::Split {
                    " split "
                } else {
                    " unified "
                }));
        }
    }

    #[test]
    fn disabled_chips_are_dim_without_reverse_or_hits() {
        for (file_count, hunks, loaded, columns, requested) in [
            (0, 0, false, 120, ViewMode::Unified),
            (1, 1, true, 120, ViewMode::Unified),
            (2, 0, true, 120, ViewMode::Unified),
            (2, 2, true, 99, ViewMode::Unified),
            (2, 2, true, 99, ViewMode::Split),
        ] {
            let mut snap = files(snapshot(
                "a.rs",
                "disabled",
                &[(1, "+"), (20, "+")][..hunks],
            ));
            snap.files.truncate(file_count);
            if !loaded {
                snap.diff = DiffState::Loading;
            }
            let mut st = ViewState::new(requested, FilesPanel::Hidden, true);
            st.resize(columns, 20);
            st.reconcile(&snap);
            let r = render(&snap, &st, columns, 22);
            for (text, action, enabled) in [
                (" ‹ ", Action::PrevFile, file_count >= 2),
                (" › ", Action::NextFile, file_count >= 2),
                (" ↑ ", Action::PrevHunk, loaded && hunks >= 2),
                (" ↓ ", Action::NextHunk, loaded && hunks >= 2),
                (" worktree ", Action::ToggleScope, false),
                (
                    " unified ",
                    Action::ToggleView,
                    columns >= MIN_SPLIT_WIDTH || requested == ViewMode::Split,
                ),
            ] {
                let span = r.lines[0].iter().find(|s| s.text == text).unwrap();
                assert_eq!(r.hits.iter().any(|h| h.action == action), enabled);
                assert_eq!(span.style.reverse, enabled);
                if !enabled {
                    assert_eq!(span.style, Style::role(Role::Label));
                }
            }
        }
    }

    #[test]
    fn hover_changes_exactly_one_chip_and_uses_key_table_footer() {
        let snap = files(snapshot("a.rs", "hover", &[(1, "+"), (20, "+")]));
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Shown, true);
        st.resize(160, 20);
        st.reconcile(&snap);
        let base = render(&snap, &st, 160, 22);
        for (action, key_action) in [
            (Action::PrevFile, keys::KeyAction::FilePrev),
            (Action::NextFile, keys::KeyAction::FileNext),
            (Action::PrevHunk, keys::KeyAction::HunkPrev),
            (Action::NextHunk, keys::KeyAction::HunkNext),
            (Action::ToggleView, keys::KeyAction::ToggleView),
            (Action::ToggleFiles, keys::KeyAction::ToggleFiles),
            (Action::Refresh, keys::KeyAction::Refresh),
        ] {
            let hit = base.hits.iter().find(|h| h.action == action).unwrap();
            st.hover = Some((hit.x0, hit.y));
            let hovered = render(&snap, &st, 160, 22);
            let differences: Vec<_> = base.lines[0]
                .iter()
                .zip(&hovered.lines[0])
                .filter(|(a, b)| a != b)
                .collect();
            assert_eq!(differences.len(), 1);
            let (before, after) = differences[0];
            assert_eq!(before.text, after.text);
            assert_eq!(
                after.style,
                Style {
                    semantic: None,
                    ..before.style
                }
            );
            let binding = keys::KEYS.iter().find(|b| b.action == key_action).unwrap();
            assert_eq!(
                hovered.plain().last().unwrap(),
                &format!("{} · {}", binding.label, binding.key)
            );
        }
        st.hover = Some((100, 5));
        assert_eq!(render(&snap, &st, 160, 22).lines, base.lines);
        st.hover = None;
        assert_eq!(render(&snap, &st, 160, 22).lines, base.lines);
    }

    #[test]
    fn hovered_file_row_is_bold_and_keeps_normal_footer() {
        let snap = files(snapshot("a.rs", "row", &[(1, "+")]));
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Shown, true);
        st.resize(120, 20);
        st.reconcile(&snap);
        let base = render(&snap, &st, 120, 22);
        let hit = base
            .hits
            .iter()
            .find(|h| h.action == Action::SelectFile(0))
            .unwrap();
        st.hover = Some((hit.x0, hit.y));
        let hovered = render(&snap, &st, 120, 22);
        let row = clip_line(
            &hovered.lines[usize::from(hit.y)],
            0,
            usize::from(FILES_WIDTH),
        );
        assert!(row.iter().all(|s| s.style.role == Role::Emphasis));
        for (y, (before, after)) in base.lines.iter().zip(&hovered.lines).enumerate() {
            if y != usize::from(hit.y) {
                assert_eq!(before, after);
            }
        }
    }

    #[test]
    fn popup_key_sheet_has_the_conditional_escape_row() {
        let snap = files(snapshot("a.rs", "sheet", &[(1, "+")]));
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        st.help_open = true;
        for popup in [false, true] {
            st.popup = popup;
            let text = render(
                &snap,
                &st,
                120,
                dialog::line_count(&keys::help_panel(popup), 60) as u16 + 7,
            )
            .plain()
            .join("\n");
            assert_eq!(
                text.lines()
                    .any(|l| l.contains("esc") && l.contains("close") && !l.contains("closes")),
                popup
            );
        }
    }

    #[test]
    fn chip_widths_keep_file_steps_at_forty_and_drop_whole_items_in_order() {
        let snap = files(snapshot(
            &"long".repeat(20),
            "narrow",
            &[(1, "+"), (20, "+")],
        ));
        let st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        let r = render(&snap, &st, 40, 12);
        for action in [Action::PrevFile, Action::NextFile] {
            let hit = r.hits.iter().find(|h| h.action == action).unwrap();
            assert_eq!(hit.x1 - hit.x0, 3);
            assert!(hit.x1 <= 40);
        }
        let snap = files(snapshot("a.rs", "narrow", &[(1, "+"), (20, "+")]));
        for columns in 40..120 {
            let r = render(&snap, &st, columns, 12);
            let text = &r.plain()[0];
            let visible = [
                text.contains("‹"),
                text.contains("{}"),
                text.contains("worktree"),
                text.contains("unified"),
                text.contains("UNSTAGED"),
                text.contains("+4"),
                text.contains("files"),
                text.contains("⟳"),
            ];
            assert!(!visible.windows(2).any(|v| !v[0] && v[1]));
            assert!(r.hits.iter().all(|h| h.x1 <= columns));
        }
    }

    #[test]
    fn startup_status_errors_replace_loading_and_not_a_repository() {
        let state = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        for revision in [0, 1] {
            let mut snap = Snapshot::empty("/repo");
            snap.revision = revision;
            snap.status_error = Some("cannot spawn git: missing\x1b[31m".into());
            let rendered = render(&snap, &state, 80, 12);
            let lines = rendered.plain();
            let expected = "cannot spawn git: missing\u{241b}[31m";
            assert_eq!(lines[6].trim(), expected);
            assert_eq!(lines[6].find(expected), Some((80 - width(expected)) / 2));
            assert!(!lines.join("\n").contains("not a git repository"));
            assert!(!lines.join("\n").contains('\x1b'));
        }
    }

    #[test]
    fn a_successful_status_outside_a_repository_keeps_its_message() {
        let mut snap = Snapshot::empty("/not-a-repo");
        snap.revision = 1;
        let state = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        assert_eq!(
            render(&snap, &state, 80, 12).plain()[6].trim(),
            "not a git repository"
        );
    }

    #[test]
    fn the_scope_chip_names_the_base_and_is_dim_without_one() {
        use crate::engine::{Base, BaseSource, Scope};
        let mut snap = snapshot("a.rs", "r1", &[(1, "+")]);
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        st.resize(120, 20);
        st.reconcile(&snap);
        // No base: the chip reads `worktree`, is dim and has no hit region.
        let r = render(&snap, &st, 120, 24);
        let bar = &r.lines[0];
        let chip = bar
            .iter()
            .find(|s| s.text == " worktree ")
            .or_else(|| bar.iter().find(|s| s.text == "worktree"));
        assert_eq!(chip.map(|s| s.style.role), Some(Role::Label));
        assert!(!r.hits.iter().any(|h| h.action == Action::ToggleScope));
        assert!(r.plain()[0].contains("UNSTAGED"));
        // A base: clickable, and the hover hint names the key.
        snap.base = Some(Base {
            requested: "refs/heads/main".into(),
            commit: "0".repeat(40),
            merge_base: None,
            source: BaseSource::Default,
        });
        let r = render(&snap, &st, 120, 24);
        let hit = r
            .hits
            .iter()
            .find(|h| h.action == Action::ToggleScope)
            .expect("chip hit");
        assert_eq!(hit.y, 0);
        st.hover = Some((hit.x0, 0));
        let r = render(&snap, &st, 120, 24);
        assert_eq!(
            r.plain().last().unwrap(),
            "switch scope · b · main @ 0000000"
        );
        snap.base.as_mut().unwrap().commit = "123456789abcdef".into();
        let r = render(&snap, &st, 120, 24);
        assert_eq!(
            r.plain().last().unwrap(),
            "switch scope · b · main @ 1234567"
        );
        snap.base.as_mut().unwrap().requested = "refs/heads/main\u{1b}".into();
        let r = render(&snap, &st, 120, 24);
        assert_eq!(
            r.plain().last().unwrap(),
            "switch scope · b · main\u{241b} @ 1234567"
        );
        snap.base.as_mut().unwrap().requested = "refs/heads/main".into();
        // Branch scope: `vs main`, and the staged label is gone.
        snap.scope = Scope::Branch;
        snap.base.as_mut().unwrap().merge_base = Some("1".repeat(40));
        st.hover = None;
        let r = render(&snap, &st, 120, 24);
        let top = &r.plain()[0];
        assert!(top.contains("vs main"), "{top}");
        assert!(
            !top.contains("UNSTAGED") && !top.contains("STAGED"),
            "{top}"
        );
        snap.base.as_mut().unwrap().requested =
            "refs/remotes/origin/a-very-long-branch-name".into();
        let r = render(&snap, &st, 120, 24);
        assert!(
            r.plain()[0].contains("vs origin/a-very-l…"),
            "{}",
            r.plain()[0]
        ); // 15 cells + the ellipsis
    }

    #[test]
    fn the_view_chip_drops_before_the_scope_chip() {
        use crate::engine::{Base, BaseSource};
        let mut snap = snapshot("a.rs", "r1", &[(1, "+")]);
        snap.base = Some(Base {
            requested: "refs/heads/main".into(),
            commit: "0".repeat(40),
            merge_base: None,
            source: BaseSource::Default,
        });
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        st.resize(120, 20);
        st.reconcile(&snap);
        let mut width = 120u16;
        let mut saw_scope_without_view = false;
        while width >= 40 {
            let top = render(&snap, &st, width, 24).plain()[0].clone();
            let has_scope = top.contains("worktree");
            let has_view = top.contains("unified");
            assert!(
                !has_view || has_scope,
                "the view chip outlived the scope chip at {width}: {top}"
            );
            saw_scope_without_view |= has_scope && !has_view;
            width -= 4;
        }
        assert!(saw_scope_without_view);
    }

    #[test]
    fn the_picker_overlays_the_body_and_clears_other_hits() {
        use crate::engine::RepoState;
        let mut snap = snapshot("a.rs", "r1", &[(1, "+")]);
        snap.refs = Some(std::sync::Arc::new(vec!["refs/heads/main".into()]));
        snap.default_base = Some("refs/heads/main".into());
        snap.repo = RepoState::Repo {
            toplevel: "/r".into(),
            branch: Some("main".into()),
            worktree: None,
        };
        snap.refs_seq = 1;
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        st.resize(120, 20);
        st.reconcile(&snap);
        st.picker = Some(crate::tui::picker::Picker::open(1));
        let r = render(&snap, &st, 120, 24);
        let text = r.plain();
        assert!(text[1].contains("Compare against"), "{}", text[1]);
        assert!(text[2].contains("> _"), "{}", text[2]);
        assert!(text[3].contains("default (main)"), "{}", text[3]);
        assert!(
            text[4].contains("main") && text[4].contains("current"),
            "{}",
            text[4]
        );
        assert!(r
            .hits
            .iter()
            .all(|h| matches!(h.action, Action::PickRow(_))));
        assert_eq!(r.hits.len(), 2);
        assert_eq!(r.hits[0].y, 3);
        assert_eq!(text.len(), 24);
    }

    #[test]
    fn only_a_drawn_body_vouches_for_an_id() {
        let mut snap = snapshot("a.rs", "r1", &[(1, "+")]);
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        st.resize(120, 20);
        st.reconcile(&snap);
        assert!(body_is_drawn(&st, &snap, 120, 24));
        assert!(
            !body_is_drawn(&st, &snap, 39, 24),
            "too narrow: the size notice is drawn"
        );
        assert!(
            !body_is_drawn(&st, &snap, 120, 9),
            "too short: the size notice is drawn"
        );
        st.help_open = true;
        assert!(
            !body_is_drawn(&st, &snap, 120, 24),
            "the key sheet covers the diff"
        );
        st.help_open = false;
        st.picker = Some(crate::tui::picker::Picker::open(0));
        assert!(
            !body_is_drawn(&st, &snap, 120, 24),
            "the picker covers the diff"
        );
        st.picker = None;
        // Give the empty fixture a row before checking an unloaded diff.
        snap.files = vec![crate::git::ChangedFile {
            path: "a.rs".into(),
            status: crate::git::ChangedFileStatus::Modified,
            staged: false,
            insertions: Some(1),
            deletions: Some(0),
        }];
        snap.diff = crate::engine::DiffState::Loading;
        assert!(
            !body_is_drawn(&st, &snap, 120, 24),
            "a row whose diff is not loaded"
        );
        snap.files.clear();
        snap.diff = crate::engine::DiffState::Idle;
        assert!(
            body_is_drawn(&st, &snap, 120, 24),
            "an empty list is a drawn body"
        );
    }
    #[test]
    fn the_panel_marks_unread_rows_in_branch_scope_only() {
        use crate::engine::{Base, BaseSource, Mark, MarkState, Scope};
        use crate::git::{ChangedFile, ChangedFileStatus};
        let mut snap = snapshot("a.rs", "r1", &[(1, "+")]);
        snap.files = vec![
            ChangedFile {
                path: "a.rs".into(),
                status: ChangedFileStatus::Modified,
                staged: false,
                insertions: Some(1),
                deletions: Some(0),
            },
            ChangedFile {
                path: "b.rs".into(),
                status: ChangedFileStatus::Added,
                staged: false,
                insertions: Some(1),
                deletions: Some(0),
            },
            ChangedFile {
                path: "u.rs".into(),
                status: ChangedFileStatus::Untracked,
                staged: false,
                insertions: None,
                deletions: None,
            },
        ];
        snap.scope = Scope::Branch;
        snap.base = Some(Base {
            requested: "refs/heads/main".into(),
            commit: "0".repeat(40),
            merge_base: Some("1".repeat(40)),
            source: BaseSource::Default,
        });
        snap.mark = Some(Mark {
            commit: "2".repeat(40),
            at: 1,
            state: MarkState::Current,
            classified_at: Some("3".repeat(40)),
        });
        snap.unread = std::sync::Arc::new(["a.rs".into(), "u.rs".into()].into_iter().collect());
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Pinned, true);
        st.resize(120, 20);
        st.reconcile(&snap);
        // Only the panel's own columns: line 0 is the toolbar, which also names the file.
        let panel = |text: &[String], name: &str| -> String {
            text.iter()
                .skip(1)
                .map(|l| l.chars().take(usize::from(FILES_WIDTH)).collect::<String>())
                .find(|l| l.contains(name))
                .unwrap_or_default()
        };
        let rendered = render(&snap, &st, 120, 24);
        let dot = rendered.lines[2].iter().find(|s| s.text == "● ").unwrap();
        assert_eq!(dot.style, Style::semantic(Role::Emphasis, Semantic::Accent));
        let text = rendered.plain();
        assert!(
            panel(&text, "a.rs").contains('●'),
            "{}",
            panel(&text, "a.rs")
        );
        assert!(
            !panel(&text, "b.rs").contains('●'),
            "{}",
            panel(&text, "b.rs")
        );
        assert!(
            !panel(&text, "u.rs").contains('●'),
            "untracked rows never carry one"
        );

        std::sync::Arc::make_mut(&mut snap.rename_sources).insert("b.rs".into(), "old.rs".into());
        snap.unread = std::sync::Arc::new(["old.rs".to_string()].into_iter().collect());
        let text = render(&snap, &st, 120, 24).plain();
        assert!(!panel(&text, "a.rs").contains('●'));
        assert!(panel(&text, "b.rs").contains('●'));

        // An unusable mark flags every tracked row, regardless of the unread set.
        snap.unread = std::sync::Arc::new(Default::default());
        for state in [MarkState::Rewritten, MarkState::Unreadable("gone".into())] {
            snap.mark.as_mut().unwrap().state = state;
            let text = render(&snap, &st, 120, 24).plain();
            for name in ["a.rs", "b.rs"] {
                assert!(panel(&text, name).contains('●'), "{}", panel(&text, name));
            }
            assert!(!panel(&text, "u.rs").contains('●'));
        }

        // Worktree scope never marks, whatever the set says.
        snap.scope = Scope::Worktree;
        snap.mark.as_mut().unwrap().state = MarkState::Current;
        snap.unread = std::sync::Arc::new(["a.rs".to_string()].into_iter().collect());
        let text = render(&snap, &st, 120, 24).plain();
        assert!(!text.iter().skip(1).any(|l| l.contains('●')));

        snap.files[0].staged = true;
        let text = render(&snap, &st, 120, 24).plain();
        assert!(panel(&text, "a.rs").ends_with("S "));

        snap.scope = Scope::Branch;
        snap.files[0].staged = false;
        snap.mark = None;
        let text = render(&snap, &st, 120, 24).plain();
        assert!(!text.iter().skip(1).any(|l| l.contains('●')));
    }

    #[test]
    fn an_urgent_notice_outranks_the_watcher_line_and_an_ordinary_one_does_not() {
        let mut snap = snapshot("a.rs", "r1", &[(1, "+")]);
        snap.watcher_error = Some("inotify limit".into());
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        st.resize(120, 20);
        st.reconcile(&snap);
        st.notify("marked 1234567 as reviewed");
        assert!(notice(&st, &snap)
            .unwrap()
            .contains("live refresh degraded"));
        st.warn("the mark cannot be read: gone");
        assert_eq!(
            notice(&st, &snap).as_deref(),
            Some("the mark cannot be read: gone")
        );
        snap.files.push(ChangedFile {
            path: "a.rs".into(),
            status: ChangedFileStatus::Modified,
            staged: false,
            insertions: None,
            deletions: None,
        });
        snap.status_error = Some("status unavailable".into());
        assert_eq!(
            notice(&st, &snap).as_deref(),
            Some("the mark cannot be read: gone")
        );
        st.notify("marked 1234567 as reviewed");
        assert!(notice(&st, &snap).unwrap().starts_with("status failed:"));
    }

    #[test]
    fn an_empty_branch_list_names_the_base() {
        use crate::engine::{Base, BaseSource, Scope};
        let mut snap = snapshot("a.rs", "r1", &[(1, "+")]);
        snap.files.clear();
        snap.diff = crate::engine::DiffState::Idle;
        snap.selected = None;
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        st.resize(120, 20);
        st.reconcile(&snap);
        assert!(render(&snap, &st, 120, 24)
            .plain()
            .iter()
            .any(|l| l.contains("working tree clean")));
        snap.scope = Scope::Branch;
        snap.base = Some(Base {
            requested: "refs/heads/main".into(),
            commit: "0".repeat(40),
            merge_base: Some("1".repeat(40)),
            source: BaseSource::Default,
        });
        let text = render(&snap, &st, 120, 24).plain();
        assert!(
            text.iter()
                .any(|l| l.contains("nothing on this branch since main")),
            "{text:?}"
        );
    }

    #[test]
    fn the_chip_says_reviewed_only_while_the_base_is_the_mark() {
        use crate::engine::{Base, BaseSource, Mark, MarkState, Scope};
        let mut snap = snapshot("a.rs", "r1", &[(1, "+")]);
        let commit = "a".repeat(40);
        snap.scope = Scope::Branch;
        snap.base = Some(Base {
            requested: commit.clone(),
            commit: commit.clone(),
            merge_base: Some(commit.clone()),
            source: BaseSource::Picked,
        });
        snap.mark = Some(Mark {
            commit: commit.clone(),
            at: 1,
            state: MarkState::Current,
            classified_at: None,
        });
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        st.resize(120, 20);
        st.reconcile(&snap);
        let rendered = render(&snap, &st, 120, 24);
        assert!(rendered.plain()[0].contains("vs reviewed"));
        let hit = rendered
            .hits
            .iter()
            .find(|h| h.action == Action::ToggleScope)
            .unwrap();
        st.hover = Some((hit.x0, 0));
        assert_eq!(
            render(&snap, &st, 120, 24).plain().last().unwrap(),
            "switch scope · b · reviewed @ aaaaaaa"
        );
        // A later mark leaves the base where it was: the alias stops, the id is shown.
        snap.mark.as_mut().unwrap().commit = "b".repeat(40);
        let top = render(&snap, &st, 120, 24).plain()[0].clone();
        assert!(top.contains("vs aaaaaaa"), "{top}");
        assert_eq!(
            render(&snap, &st, 120, 24).plain().last().unwrap(),
            &format!("switch scope · b · {commit} @ aaaaaaa")
        );
        snap.mark.as_mut().unwrap().commit = commit;
        snap.base.as_mut().unwrap().requested = "refs/heads/main".into();
        assert!(render(&snap, &st, 120, 24).plain()[0].contains("vs main"));
    }

    #[test]
    fn the_toolbar_group_sits_between_the_steppers_and_drops_first() {
        // The action group fits at 160 columns and drops first when space runs out.
        let (r, _) = rendered(160, 20, FilesPanel::Hidden);
        let bar = &r.plain()[0];
        let stepper = bar.find("‹  a.rs").unwrap();
        let group = bar.find(" stage ").unwrap();
        let hunks = bar.find("{} 1/2").unwrap();
        assert!(stepper < group && group < hunks, "{bar}");
        assert!(
            bar.contains(" discard ") && bar.contains(" discard file "),
            "{bar}"
        );
        for action in [Action::StageHunk, Action::DiscardHunk, Action::DiscardFile] {
            assert!(
                r.hits.iter().any(|h| h.y == 0 && h.action == action),
                "{action:?}"
            );
        }
        for columns in [120, 80] {
            let (r, _) = rendered(columns, 20, FilesPanel::Hidden);
            let bar = &r.plain()[0];
            assert!(
                !bar.contains("discard") && bar.contains("{} 1/2"),
                "{columns}: {bar}"
            );
        }
    }

    #[test]
    fn the_group_reads_unstage_on_a_staged_row_and_is_absent_in_branch_scope_and_dim_while_loading()
    {
        let mut snap = files(snapshot("a.rs", "r1", &[(1, "+")]));
        if let DiffState::Ready(d) = &mut snap.diff {
            std::sync::Arc::make_mut(d).key.staged = true;
        }
        snap.selected.as_mut().unwrap().staged = true;
        let st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        let bar = render(&snap, &st, 160, 20).plain()[0].clone();
        assert!(
            bar.contains(" unstage ") && !bar.contains(" stage "),
            "{bar}"
        );
        snap.scope = Scope::Branch;
        snap.base = Some(crate::engine::Base {
            requested: "refs/heads/main".into(),
            commit: "c".repeat(40),
            merge_base: Some("m".repeat(40)),
            source: crate::engine::BaseSource::Default,
        });
        let r = render(&snap, &st, 160, 20);
        assert!(!r.plain()[0].contains("discard"));
        assert!(!r.plain().last().unwrap().contains("s stage"));
        snap.scope = Scope::Worktree;
        snap.diff = DiffState::Loading;
        let r = render(&snap, &st, 160, 20);
        assert!(r.plain()[0].contains("discard"));
        assert!(!r.hits.iter().any(|h| matches!(
            h.action,
            Action::StageHunk | Action::DiscardHunk | Action::DiscardFile
        )));
    }

    #[test]
    fn the_box_covers_the_body_and_fits_the_minimum_terminal() {
        let snap = files(snapshot(
            "a/very/long/directory/name/that/goes/on/and/on/values.ts",
            "r1",
            &[(1, "+")],
        ));
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        st.resize(40, body_height(&st, &snap, 10));
        st.reconcile(&snap);
        crate::tui::input::handle_key(
            &mut st,
            &snap,
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('D'),
                crossterm::event::KeyModifiers::SHIFT,
            ),
            40,
        );
        assert!(st.confirm.is_some());
        let r = render(&snap, &st, 40, 10);
        let text = r.plain();
        assert!(text.iter().any(|l| l.contains("Discard file?")), "{text:?}");
        assert!(
            text.iter().any(|l| l.contains("cannot be undone")),
            "{text:?}"
        );
        assert!(text.iter().any(|l| l.contains("y yes · n no")), "{text:?}");
        assert!(r.hits.is_empty());
        assert!(!body_is_drawn(&st, &snap, 40, 10));
        assert_eq!(
            render(&snap, &st, 39, 10).plain(),
            vec!["terminal too small"]
        );
    }

    #[test]
    fn the_target_chip_is_drawn_and_drops_last_but_one() {
        let mut snap = files(snapshot("a.rs", "chip", &[(1, "+"), (20, "+")]));
        snap.target = Some(crate::engine::Target::Pane {
            pane: "w4:p2".into(),
            socket: "/s".into(),
            agent: "codex".into(),
            session: None,
            title: String::new(),
        });
        snap.target_state = crate::engine::TargetState::Live("idle".into());
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        for columns in [160, 70] {
            st.resize(columns, body_height(&st, &snap, 24));
            st.reconcile(&snap);
            let r = render(&snap, &st, columns, 24);
            let bar = &r.plain()[0];
            assert!(bar.contains("→ codex w4:p2"), "{bar}");
            if columns == 70 {
                assert!(!bar.contains("files") && !bar.contains("+4 −3"), "{bar}");
            }
            let span = r.lines[0]
                .iter()
                .find(|s| s.text.contains("→ codex"))
                .unwrap();
            assert_eq!(
                span.style,
                Style {
                    reverse: true,
                    ..Style::semantic(Role::Emphasis, Semantic::Accent)
                }
            );
            let hit = r
                .hits
                .iter()
                .find(|h| h.action == Action::PickPane)
                .unwrap();
            assert_eq!(r.hit(hit.x0, 0), Some(&Action::PickPane));
        }
    }

    #[test]
    fn the_pane_picker_overlay_lists_groups_and_hits_its_rows() {
        use crate::engine::{host::PaneRecord, PaneRow};
        use crate::tui::panes::{PanePicker, ReturnTo, CLIPBOARD_ROW};
        let mut snap = files(snapshot("a.rs", "panes", &[(1, "+")]));
        snap.panes = Some(std::sync::Arc::new(vec![
            PaneRow {
                record: PaneRecord {
                    pane_id: "w1:p2".into(),
                    agent: Some("codex".into()),
                    ..Default::default()
                },
                this_worktree: true,
            },
            PaneRow {
                record: PaneRecord {
                    pane_id: "w2:p1".into(),
                    agent: Some("kimi".into()),
                    ..Default::default()
                },
                this_worktree: false,
            },
        ]));
        snap.panes_seq = 1;
        let mut st = ViewState::new(ViewMode::Unified, FilesPanel::Hidden, true);
        st.resize(120, body_height(&st, &snap, 24));
        st.reconcile(&snap);
        st.panes = Some(PanePicker::open(1, ReturnTo::Nothing));
        let r = render(&snap, &st, 120, 24);
        let text = r.plain();
        for piece in ["Send to", "this worktree", "other panes", CLIPBOARD_ROW] {
            assert!(text.iter().any(|line| line.contains(piece)), "{text:?}");
        }
        let first: Vec<_> = r
            .hits
            .iter()
            .filter(|hit| hit.action == Action::PickPaneRow(0))
            .collect();
        assert_eq!(first.len(), 1);
        assert!(text[usize::from(first[0].y)].contains("codex  w1:p2"));
        let other = r
            .hits
            .iter()
            .find(|hit| hit.action == Action::PickPaneRow(1))
            .unwrap();
        assert_eq!(other.y, 7);
        assert!(text[usize::from(other.y)].contains("kimi  w2:p1"));
        assert_eq!(r.hit(other.x0, other.y), Some(&Action::PickPaneRow(1)));
        assert!(r
            .hits
            .iter()
            .all(|h| matches!(h.action, Action::PickPaneRow(_))));
        assert!(!body_is_drawn(&st, &snap, 120, 24));

        snap.panes = Some(std::sync::Arc::new(Vec::new()));
        st.resize(44, body_height(&st, &snap, 24));
        let r = render(&snap, &st, 44, 24);
        assert_eq!(r.hits.len(), 1);
        let hit = &r.hits[0];
        assert_eq!((hit.y, &hit.action), (6, &Action::PickPaneRow(0)));
        assert!(r.plain()[6].contains("✂ clipboard"), "{:?}", r.plain());

        st.resize(40, body_height(&st, &snap, 10));
        let r = render(&snap, &st, 40, 10);
        let hit = r
            .hits
            .iter()
            .find(|hit| hit.action == Action::PickPaneRow(0))
            .expect("the clipboard must be clickable at 40×10");
        assert!(r.plain()[usize::from(hit.y)].contains("✂ clipboard"));
    }

    use crate::engine::comments::{self, Category, CommentState};
    use crate::tui::input::handle_key;
    use crate::tui::input::tests::{anchor_on, comment_at, key, review_setup};
    use crate::tui::review::{Editor, EDITOR_FOOTER};

    #[test]
    fn a_card_is_wrapped_and_framed_at_one_width() {
        for columns in [80, 120] {
            for mode in [ViewMode::Unified, ViewMode::Split] {
                let (mut snap, mut st) = review_setup(&[(10, " + ")]);
                let text = "x".repeat(200);
                snap.comments =
                    std::sync::Arc::new(vec![comment_at(&anchor_on(&snap, 11), &text, 1)]);
                st.requested_mode = mode;
                st.resize(columns, 22);
                st.mode = mode;
                st.reconcile(&snap);
                let output = render(&snap, &st, columns, 24).plain();
                let shown: String = output
                    .iter()
                    .filter_map(|line| {
                        line.trim_start()
                            .strip_prefix("│ ")
                            .and_then(|s| s.strip_suffix(" │"))
                            .map(str::trim_end)
                    })
                    .collect();
                assert_eq!(shown, text, "{columns} {mode:?}: {output:?}");
                assert!(output.iter().all(|l| width(l) <= usize::from(columns)));
            }
        }
    }

    #[test]
    fn cards_are_drawn_under_their_lines_with_their_titles() {
        let (mut snap, mut st) = review_setup(&[(10, " ++ ")]);
        st.requested_mode = ViewMode::Unified;
        st.resize(120, 22);
        let bug = comment_at(&anchor_on(&snap, 11), "fix this", 1);
        let mut sent = comment_at(&anchor_on(&snap, 12), "why this?", 2);
        sent.category = Category::Question;
        sent.state = CommentState::Sent(comments::Stamp {
            at: 1,
            nonce: "abc123".into(),
            item: 1,
            to: crate::engine::target::Destination::clipboard(),
        });
        snap.comments = std::sync::Arc::new(vec![bug, sent]);
        st.reconcile(&snap);
        let rendered = render(&snap, &st, 120, 24);
        let plain = rendered.plain();
        for (title, text, number) in [
            ("Bug · pending", "fix this", "11"),
            ("Question · sent", "why this?", "12"),
        ] {
            let at = plain
                .iter()
                .position(|l| l.contains(&format!("╭─ {title}")))
                .expect("card title");
            assert!(plain[at - 1].contains(number) && plain[at - 1].contains("+ line +"));
            assert!(plain[at + 1].contains(&format!("│ {text}")));
        }
        let sent = rendered
            .lines
            .iter()
            .flatten()
            .find(|s| s.text == "Question · sent")
            .unwrap();
        assert_eq!(sent.style.role, Role::Label);
    }

    #[test]
    fn the_panel_marks_files_with_comments_and_counts_pending() {
        let (mut snap, mut st) = review_setup(&[(10, " + ")]);
        snap = files(snap);
        st.files_panel = FilesPanel::Shown;
        let mut pending = comment_at(&anchor_on(&snap, 11), "pending", 1);
        pending.anchor.key = FileKey::of(&snap.files[0]);
        let mut sent = comment_at(&anchor_on(&snap, 11), "sent", 2);
        sent.state = CommentState::Sent(comments::Stamp {
            at: 1,
            nonce: "abc123".into(),
            item: 2,
            to: crate::engine::target::Destination::clipboard(),
        });
        snap.comments = std::sync::Arc::new(vec![pending, sent]);
        st.reconcile(&snap);
        let (panel, _) = files_lines(&snap, &st, 22);
        let plain: Vec<String> = panel
            .iter()
            .map(|l| l.iter().map(|s| s.text.as_str()).collect())
            .collect();
        assert!(plain[0].starts_with("CHANGED 2 · ✎ 1"));
        assert_eq!(
            plain[1].chars().nth(usize::from(FILES_WIDTH - 3)),
            Some('✎')
        );
        assert!(plain[1].ends_with("✎S ") && plain[2].ends_with("✎  "));
        let mut branch = snap.comments[0].clone();
        branch.anchor.comparison = comments::AnchorComparison::Branch {
            merge_base: "0".repeat(40),
            label: "main".into(),
        };
        snap.comments = std::sync::Arc::new(vec![branch]);
        let (panel, _) = files_lines(&snap, &st, 22);
        assert!(
            panel[0].iter().any(|s| s.text.contains("✎ 1")),
            "the count spans scopes"
        );
        assert!(
            !panel[1].iter().any(|s| s.text.contains('✎')),
            "the mark follows the scope"
        );
    }

    #[test]
    fn the_editor_is_spliced_after_its_anchor_row() {
        let (snap, mut st) = review_setup(&[(10, " +++ ")]);
        st.requested_mode = ViewMode::Unified;
        st.resize(120, 22);
        st.editor = Some(Editor::new(anchor_on(&snap, 12)));
        st.reconcile(&snap);
        let output = render(&snap, &st, 120, 24);
        let plain = output.plain();
        let at = plain
            .iter()
            .position(|s| s.contains("comment on R12 ·"))
            .expect("editor title");
        assert!(plain[at - 1].contains("12 + line +"));
        assert!(plain[at].contains("ctrl+h/l"));
        assert!(plain[at + 1].contains('_'));
        assert!(plain[at + 2].contains(EDITOR_FOOTER));
        assert!(plain[at + 3].contains("13 + line +"));
        assert!(body_is_drawn(&st, &snap, 120, 24));
    }

    #[test]
    fn an_orphan_section_closes_the_body() {
        let (mut snap, mut st) = review_setup(&[(10, " + ")]);
        let mut orphan = comment_at(&anchor_on(&snap, 16), "left behind", 1);
        orphan.anchor.key.path = "gone.rs".into();
        snap.comments = std::sync::Arc::new(vec![orphan]);
        st.reconcile(&snap);
        let plain = render(&snap, &st, 120, 24).plain();
        let at = plain
            .iter()
            .position(|s| s.contains("✎ on changes no longer shown (1)"))
            .expect("orphan section");
        assert!(plain[at + 1].contains("gone.rs:16 · Bug · pending"));
        assert!(plain[at + 2].contains("│ left behind"));
    }

    #[test]
    fn a_visual_selection_is_drawn_in_reverse_over_its_lines() {
        for mode in [ViewMode::Unified, ViewMode::Split] {
            let (snap, mut st) = review_setup(&[(10, " --++ ")]);
            st.requested_mode = mode;
            st.resize(120, 22);
            st.reconcile(&snap);
            handle_key(&mut st, &snap, key("v"), 120);
            handle_key(&mut st, &snap, key("j"), 120);
            let output = render(&snap, &st, 120, 24);
            let text_spans: Vec<_> = output
                .lines
                .iter()
                .flatten()
                .filter(|s| s.text.contains("line "))
                .collect();
            let selected: Vec<_> = text_spans.iter().filter(|s| s.style.reverse).collect();
            assert_eq!(selected.len(), 2, "{mode:?}");
            assert!(selected.iter().all(|s| s.text.contains("- line -")));
            assert!(
                text_spans
                    .iter()
                    .filter(|s| !s.text.contains("- line -"))
                    .all(|s| !s.style.reverse),
                "the other side is not selected"
            );
        }
    }

    #[test]
    fn cards_fit_beside_the_panel_at_the_minimum_terminal_width() {
        let (mut snap, mut st) = review_setup(&[(10, " + ")]);
        st.files_panel = FilesPanel::Shown;
        st.resize(40, 58);
        let comment = comment_at(&anchor_on(&snap, 11), &"z".repeat(200), 1);
        snap.comments = std::sync::Arc::new(vec![comment.clone()]);
        for editing in [false, true] {
            if editing {
                snap.comments = std::sync::Arc::new(Vec::new());
                st.editor = Some(Editor::edit(&comment));
            }
            st.reconcile(&snap);
            let output = render(&snap, &st, 40, 60).plain();
            assert_eq!(
                output
                    .iter()
                    .flat_map(|s| s.chars())
                    .filter(|c| *c == 'z')
                    .count(),
                200,
                "{output:?}"
            );
            assert!(output.iter().all(|s| width(s) <= 40));
            if editing {
                assert!(output.iter().any(|s| s.contains('_')));
            }
        }
    }

    #[test]
    fn the_finish_box_is_drawn_for_each_row_of_the_table() {
        use crate::engine::{Target, TargetState};
        use crate::tui::input::tests::pane_target;
        for (state, text, keys) in [
            (
                TargetState::Live("idle".into()),
                "Send 2 comments across 1 file to codex · w4:p2?",
                vec!["Y send", "A pick another pane", "c copy", "n cancel"],
            ),
            (
                TargetState::Live("blocked".into()),
                "codex is waiting for an approval in w4:p2.",
                vec!["A pick another pane", "c copy", "n cancel"],
            ),
            (
                TargetState::NoHost,
                "No host: this viewer runs outside herdr.",
                vec!["c copy", "n cancel"],
            ),
            (
                TargetState::Clipboard,
                "Copy 2 comments across 1 file to the clipboard?",
                vec!["Y copy", "A pick another pane", "n cancel"],
            ),
        ] {
            let (mut snap, mut st) = review_setup(&[(10, " + ")]);
            snap.target = Some(if state == TargetState::Clipboard {
                Target::Clipboard
            } else {
                pane_target()
            });
            snap.target_state = state;
            snap.comments = std::sync::Arc::new(vec![
                comment_at(&anchor_on(&snap, 11), "one", 1),
                comment_at(&anchor_on(&snap, 11), "two", 2),
            ]);
            st.review_box = Some(review::ReviewBox::finish());
            st.reconcile(&snap);
            let r = render(&snap, &st, 72, 24);
            let plain = r.plain().join("\n");
            assert!(plain.contains("┌ Finish"), "{plain}");
            assert!(plain.contains(text), "{plain}");
            for key in keys {
                assert!(plain.contains(key), "{plain}");
            }
            if snap.target_state == TargetState::NoHost
                || snap.target_state == TargetState::Live("blocked".into())
            {
                assert!(!plain.contains("Y send"));
            }
            assert!(r.hits.is_empty());
            assert!(!body_is_drawn(&st, &snap, 72, 24));
            let b = st.review_box.as_ref().unwrap();
            assert!(b.fits(&snap, 72, 24));
            assert!(!b.fits(&snap, 39, 24) && !b.fits(&snap, 72, 9));
            assert_eq!(
                render(&snap, &st, 39, 10).plain(),
                vec!["terminal too small"]
            );
            assert!(render(&snap, &st, 40, 10)
                .plain()
                .iter()
                .any(|line| line.contains("n cancel")));
        }
    }

    #[test]
    fn the_request_box_draws_its_scope_line_in_three_shapes() {
        use crate::engine::dispatch::ReviewScope;
        let (mut snap, mut st) = review_setup(&[(10, " + ")]);
        snap.selected = Some(FileKey {
            path: "a.rs".into(),
            staged: false,
            untracked: false,
        });
        snap.files.push(ChangedFile {
            path: "b.rs".into(),
            status: ChangedFileStatus::Modified,
            staged: false,
            insertions: None,
            deletions: None,
        });
        st.review_box = Some(review::ReviewBox::request(&snap));
        for columns in [40, 72] {
            for scope in [
                ReviewScope::All,
                ReviewScope::File(snap.selected.clone().unwrap()),
            ] {
                st.review_box.as_mut().unwrap().scope = scope.clone();
                let r = render(&snap, &st, columns, 24);
                assert!(r.plain().join("\n").contains("Scope  f this file"));
                let scope_rows = "Scope  f this file   a all changes (2)"
                    .len()
                    .div_ceil(usize::from(columns) - 4);
                let selected: String = r.lines[2..2 + scope_rows]
                    .iter()
                    .flatten()
                    .filter(|span| span.style.reverse)
                    .map(|span| span.text.as_str())
                    .collect();
                assert_eq!(
                    selected,
                    if scope == ReviewScope::All {
                        "a all changes (2)"
                    } else {
                        "f this file"
                    }
                );
                for line in &r.lines {
                    assert_eq!(
                        line.iter().map(|span| width(&span.text)).sum::<usize>(),
                        usize::from(columns)
                    );
                }
            }
        }
        snap.files.truncate(1);
        st.review_box = Some(review::ReviewBox::request(&snap));
        assert!(!render(&snap, &st, 72, 24)
            .plain()
            .join("\n")
            .contains("Scope"));
        snap.diff = DiffState::Loading;
        let r = render(&snap, &st, 72, 24);
        assert!(r
            .plain()
            .join("\n")
            .contains("Scope  f this file   a all changes (1)"));
        assert!(r.lines[2].iter().any(|span| span.text == "f this file"
            && span.style.role == Role::Label
            && !span.style.reverse));
        snap.files.clear();
        let plain = render(&snap, &st, 72, 24).plain().join("\n");
        assert!(plain.contains("nothing to review") && plain.contains("n cancel"));
        assert!(!plain.contains("Scope") && !plain.contains("Y copy") && !plain.contains("c copy"));
    }

    #[test]
    fn the_footer_offers_y_finish_with_the_pending_count() {
        let (mut snap, st) = review_setup(&[(10, " + ")]);
        let footer = render(&snap, &st, 200, 24).plain().pop().unwrap();
        assert!(footer.contains("@ request") && !footer.contains("Y finish"));
        snap.comments = std::sync::Arc::new(vec![
            comment_at(&anchor_on(&snap, 11), "one", 1),
            comment_at(&anchor_on(&snap, 11), "two", 2),
        ]);
        let footer = render(&snap, &st, 200, 24).plain().pop().unwrap();
        assert!(footer.contains("Y finish (2)") && footer.contains("@ request"));
        let footer = render(&snap, &st, 40, 10).plain().pop().unwrap();
        assert!(footer.contains("? help") && footer.contains("q quit"));
        assert!(!footer.contains("Y finish") && !footer.contains("@ request"));
    }

    #[test]
    fn the_key_sheet_lists_the_new_rows() {
        let (snap, mut st) = review_setup(&[(10, " + ")]);
        st.help_open = true;
        let plain = render(&snap, &st, 72, 80).plain().join("\n");
        for text in [
            "finish: send the review",
            "request a review",
            "in the Finish and Request boxes:",
            "in the editor: Enter save",
            "y copies a selection made with v",
        ] {
            assert!(plain.contains(text), "{text}: {plain}");
        }
    }
}
