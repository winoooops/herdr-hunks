//! The editor card and the visual selection of spec 10.3.
use crate::engine::comments::{self, Anchor, Category, Comment, Span as AnchorSpan};
use crate::engine::nav::Side;
use crate::engine::{Command, LoadedDiff};
use crate::tui::cards::CardLine;
use crate::tui::format::width;
use crate::tui::style::{Role, Semantic, Span, Style};

pub const NO_LINE: &str = "No diff line selected for comment.";
pub const NO_COMMENT: &str = "No comment selected.";
pub const CHOOSE_PANE: &str = "choose the pane this review goes to";
pub const EDITOR_FOOTER: &str = "enter save · ctrl+j newline · esc cancel";

pub struct Editor {
    pub anchor: Anchor,
    pub category: Category,
    pub text: String,
    /// The record `u`/`U` opened, kept as seen for the edit's transaction.
    pub editing: Option<Comment>,
    pub at_limit: bool,
    /// The token a save was sent under (`ViewState.comment_token`, one per save). Keys wait and the draft
    /// stays until the answer carrying it: accepted closes the editor, a refusal (the cap, a record that
    /// changed) hands the text back. Another save's answer, or a notice from the refresh, is not this one's.
    pub pending: Option<u64>,
}

impl Editor {
    pub fn new(anchor: Anchor) -> Self {
        Self {
            anchor,
            category: Category::Change,
            text: String::new(),
            editing: None,
            at_limit: false,
            pending: None,
        }
    }

    pub fn edit(comment: &Comment) -> Self {
        Self {
            anchor: comment.anchor.clone(),
            category: comment.category,
            text: comment.text.clone(),
            editing: Some(comment.clone()),
            at_limit: false,
            pending: None,
        }
    }

    fn try_push(&mut self, ch: char) {
        let mut candidate = self.text.clone();
        candidate.push(ch);
        if !comments::within_caps(&candidate) {
            self.at_limit = true;
            return;
        }
        self.text = candidate;
        self.at_limit = false;
    }

    pub fn insert(&mut self, ch: char) {
        if !ch.is_control() {
            self.try_push(ch);
        }
    }

    pub fn newline(&mut self) {
        self.try_push('\n');
    }

    pub fn backspace(&mut self) {
        self.text.pop();
        self.at_limit = false;
    }

    pub fn place_label(&self) -> String {
        match self.anchor.span {
            AnchorSpan::File => "file".to_string(),
            AnchorSpan::Line => format!("{}{}", side_letter(self.anchor.side), self.anchor.line),
            AnchorSpan::Range { end } => format!(
                "{}{}-{end}",
                side_letter(self.anchor.side),
                self.anchor.line
            ),
        }
    }

    /// `comment on R16 · Question Change Bug Suggestion` with the chosen word in reverse video, or the limit.
    pub fn title(&self) -> Vec<Span> {
        if self.at_limit {
            return vec![Span::new(
                comments::NOTICE_LIMIT,
                Style::semantic(Role::Emphasis, Semantic::Warn),
            )];
        }
        let mut spans = vec![Span::body(format!("comment on {} · ", self.place_label()))];
        for (i, category) in Category::ALL.iter().enumerate() {
            if i > 0 {
                spans.push(Span::body(" "));
            }
            let mut style = Style::role(Role::Body);
            style.reverse = *category == self.category;
            spans.push(Span::new(category.short(), style));
        }
        spans
    }

    pub fn lines(&self, width_cells: usize) -> Vec<CardLine> {
        let inner = width_cells.max(crate::tui::cards::MIN_WIDTH) - 4;
        let mut lines = vec![CardLine::Top {
            title: String::new(),
            tone: None,
            dim: false,
        }];
        let mut text = crate::tui::cards::wrap_sanitized(&self.text, inner);

        match text.last_mut() {
            Some(last) if width(last) < inner => last.push('_'),
            _ => text.push("_".to_string()),
        }
        lines.extend(text.into_iter().map(CardLine::Text));
        lines.push(CardLine::Bottom);
        lines
    }

    /// `Enter`: a whitespace-only text is inert. `token` is the number the answer will carry back.
    pub fn submit(&mut self, token: u64) -> Option<Command> {
        if self.text.trim().is_empty() {
            return None;
        }
        self.pending = Some(token);
        Some(match &self.editing {
            Some(seen) => Command::EditComment {
                token,
                seen: seen.clone(),
                category: self.category,
                text: self.text.clone(),
            },
            None => Command::AddComment {
                token,
                anchor: self.anchor.clone(),
                category: self.category,
                text: self.text.clone(),
            },
        })
    }
}

fn side_letter(side: Side) -> char {
    match side {
        Side::Additions => 'R',
        Side::Deletions => 'L',
    }
}

/// The target the selection began on; the cursor is its other end. The selection belongs to one
/// loaded diff: `reconcile` drops it when another `Arc` is on screen, so an index never names a line
/// of another file or comparison.
pub struct Visual {
    pub start: usize,
    pub diff: std::sync::Arc<LoadedDiff>,
}

/// `(side, first line, last line)` of the selection on the start's side; `None` when the cursor
/// crossed to the other side (a selection is one side's lines).
pub fn selection(diff: &LoadedDiff, visual: &Visual, cursor: usize) -> Option<(Side, u32, u32)> {
    let start = diff.targets.get(visual.start)?;
    let end = diff.targets.get(cursor)?;
    if start.side != end.side {
        return None;
    }
    let (a, b) = (
        start.line_number.min(end.line_number),
        start.line_number.max(end.line_number),
    );
    Some((start.side, a, b))
}

/// The selected lines' text, for `y`: the side's content, line by line.
pub fn selection_text(diff: &LoadedDiff, side: Side, start: u32, end: u32) -> String {
    use crate::git::DiffLineType;
    let mut lines = Vec::new();
    for hunk in &diff.file_diff.hunks {
        for line in &hunk.lines {
            let (number, mine) = match (side, &line.line_type) {
                (Side::Additions, DiffLineType::Removed)
                | (Side::Deletions, DiffLineType::Added) => continue,
                (Side::Additions, _) => (line.new_line_number, true),
                (Side::Deletions, _) => (line.old_line_number, true),
            };
            if let (Some(n), true) = (number, mine) {
                if n >= start && n <= end {
                    lines.push(line.content.clone());
                }
            }
        }
    }
    lines.join("\n")
}

/// The next (`delta > 0`) or previous target on `side` by line number; the cursor when there is none.
pub fn step_on_side(diff: &LoadedDiff, cursor: usize, side: Side, delta: isize) -> usize {
    let Some(current) = diff.targets.get(cursor) else {
        return cursor;
    };
    let mut candidates: Vec<(u32, usize)> = diff
        .targets
        .iter()
        .enumerate()
        .filter(|(_, t)| t.side == side)
        .map(|(i, t)| (t.line_number, i))
        .collect();
    candidates.sort();
    let at = candidates
        .iter()
        .position(|(_, i)| *i == cursor)
        .or_else(|| {
            candidates
                .iter()
                .position(|(line, _)| *line >= current.line_number)
        });
    match at {
        Some(at) => candidates
            .get((at as isize + delta).clamp(0, candidates.len() as isize - 1) as usize)
            .map(|(_, i)| *i)
            .unwrap_or(cursor),
        None => cursor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::comments::{AnchorComparison, CommentState};
    use crate::engine::{DiffState, FileKey};
    use std::sync::Arc;

    fn anchor() -> Anchor {
        Anchor {
            key: FileKey {
                path: "a.rs".into(),
                staged: false,
                untracked: false,
            },
            side: Side::Additions,
            line: 16,
            span: AnchorSpan::Line,
            comparison: AnchorComparison::Worktree,
        }
    }

    #[test]
    fn the_editor_title_names_the_place_and_the_chosen_category() {
        let mut editor = Editor::new(anchor());
        assert_eq!(editor.place_label(), "R16");
        assert!(editor
            .title()
            .iter()
            .any(|s| s.text == "Change" && s.style.reverse));
        editor.category = editor.category.next();
        assert!(editor
            .title()
            .iter()
            .any(|s| s.text == "Bug" && s.style.reverse));
        editor.anchor.side = Side::Deletions;
        editor.anchor.line = 3;
        editor.anchor.span = AnchorSpan::Range { end: 9 };
        assert_eq!(editor.place_label(), "L3-9");
        editor.anchor.span = AnchorSpan::File;
        assert_eq!(editor.place_label(), "file");
    }

    #[test]
    fn the_editor_stops_at_the_caps_and_says_so() {
        let mut editor = Editor::new(anchor());
        for _ in 0..4_000 {
            editor.insert('字');
        }
        assert_eq!(editor.text.chars().count(), 4_000);
        assert!(!editor.at_limit);
        editor.insert('!');
        assert!(editor.at_limit);
        assert_eq!(editor.text.chars().count(), 4_000);
        assert_eq!(editor.title()[0].text, comments::NOTICE_LIMIT);
        editor.backspace();
        assert!(!editor.at_limit);
        editor.text.clear();
        for _ in 0..99 {
            editor.newline();
        }
        assert!(!editor.at_limit);
        editor.newline();
        assert!(editor.at_limit);
        assert_eq!(editor.text.chars().count(), 99);
        editor.insert('\u{1b}');
        assert_eq!(editor.text.chars().count(), 99);
        assert!(matches!(editor.lines(30).last(), Some(CardLine::Bottom)));
    }

    #[test]
    fn submit_is_inert_on_whitespace_and_edits_keep_the_id() {
        let mut editor = Editor::new(anchor());
        editor.text = " \n ".into();
        assert_eq!(editor.submit(1), None);
        assert_eq!(editor.pending, None);
        let comment = Comment {
            id: "original".into(),
            anchor: anchor(),
            category: Category::Question,
            text: "why?".into(),
            created_at: 1,
            state: CommentState::Pending,
        };
        let mut editor = Editor::edit(&comment);
        editor.insert('!');
        assert!(
            matches!(editor.submit(2), Some(Command::EditComment { token: 2, seen, text, .. }) if seen == comment && text == "why?!")
        );
        assert_eq!(editor.pending, Some(2));
        let mut editor = Editor::new(anchor());
        editor.insert('a');
        assert!(
            matches!(editor.submit(3), Some(Command::AddComment { token: 3, anchor: a, .. }) if a == anchor())
        );
    }

    #[test]
    fn a_selection_spans_one_side_and_yields_its_lines() {
        let snap = crate::tui::state::tests::snapshot("a.rs", "raw", &[(10, " --++ ")]);
        let DiffState::Ready(mut diff) = snap.diff else {
            panic!()
        };
        let lines = &mut Arc::make_mut(&mut diff).file_diff.hunks[0].lines;
        for (i, (old, new)) in [
            (Some(10), Some(10)),
            (Some(11), None),
            (Some(12), None),
            (None, Some(11)),
            (None, Some(12)),
            (Some(13), Some(13)),
        ]
        .into_iter()
        .enumerate()
        {
            lines[i].old_line_number = old;
            lines[i].new_line_number = new;
            lines[i].content = format!("text {i}");
        }
        let start = crate::engine::nav::find(&diff.targets, Side::Deletions, 11).unwrap();
        let end = crate::engine::nav::find(&diff.targets, Side::Deletions, 12).unwrap();
        let other = crate::engine::nav::find(&diff.targets, Side::Additions, 12).unwrap();
        let visual = Visual {
            start,
            diff: diff.clone(),
        };
        assert_eq!(
            selection(&diff, &visual, end),
            Some((Side::Deletions, 11, 12))
        );
        assert_eq!(selection(&diff, &visual, other), None);
        assert_eq!(
            selection_text(&diff, Side::Deletions, 11, 12),
            "text 1\ntext 2"
        );
        assert_eq!(
            selection_text(&diff, Side::Additions, 11, 13),
            "text 3\ntext 4\ntext 5"
        );
        assert_eq!(step_on_side(&diff, start, Side::Deletions, 1), end);
        assert_eq!(step_on_side(&diff, end, Side::Deletions, -1), start);
    }
}
