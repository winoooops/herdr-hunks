//! Cards (spec 10.3): a comment drawn as a rounded frame under its line; the editor shares the frame.
use crate::engine::comments::{Anchor, Category, Comment, CommentState, Span as AnchorSpan};
use crate::tui::format::{truncate, width};
use crate::tui::sanitize::sanitize;
use crate::tui::style::{Line, Role, Semantic, Span, Style};

pub const MIN_WIDTH: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardLine {
    Top {
        title: String,
        tone: Option<Semantic>,
        dim: bool,
    },
    Text(String),
    Bottom,
}

pub fn tone(category: Category) -> Option<Semantic> {
    match category {
        Category::Question => Some(Semantic::Accent),
        Category::Bug => Some(Semantic::Warn),
        Category::Change | Category::Suggestion => None,
    }
}

/// The width a card is wrapped *and* drawn at: the body's columns past the line-number gutter.
/// `rows::build` and `view::body_line` both take it from here, so no text is wrapped for one width
/// and framed at another.
pub fn card_width(body_width: usize, mode: crate::engine::nav::ViewMode) -> usize {
    let gutter = match mode {
        crate::engine::nav::ViewMode::Unified => 12,
        crate::engine::nav::ViewMode::Split => 6,
    };
    body_width.saturating_sub(gutter).max(MIN_WIDTH)
}

pub fn state_word(state: &CommentState) -> &'static str {
    match state {
        CommentState::Pending => "pending",
        CommentState::Sending { .. } => "sending",
        CommentState::Unconfirmed { .. } => "sent?",
        CommentState::Sent(_) => "sent",
    }
}

pub fn place(anchor: &Anchor) -> String {
    match anchor.span {
        AnchorSpan::File => " (file)".to_string(),
        AnchorSpan::Line => format!(":{}", anchor.line),
        AnchorSpan::Range { end } => format!(":{}-{end}", anchor.line),
    }
}

pub fn title(comment: &Comment, orphan: bool) -> String {
    let core = format!(
        "{} · {}",
        comment.category.short(),
        state_word(&comment.state)
    );
    if orphan {
        format!(
            "{}{} · {core}",
            sanitize(&comment.anchor.key.path),
            place(&comment.anchor)
        )
    } else {
        core
    }
}

/// Word-wrapped by terminal cells; a newline is a hard break; a line breaks after a space when the
/// next word would not fit, a word wider than the width is cut by whole characters, and every
/// character of the text, spaces included, appears exactly once (`truncate` appends `…` and is
/// not used here): the cards show the indentation the prompt sends.
pub fn wrap(text: &str, width_cells: usize) -> Vec<String> {
    let width_cells = width_cells.max(1);
    let mut out = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();

        let mut pieces: Vec<String> = Vec::new();
        for ch in paragraph.chars() {
            match pieces.last_mut() {
                Some(last) if (last.ends_with(' ')) == (ch == ' ') => last.push(ch),
                _ => pieces.push(ch.to_string()),
            }
        }
        for piece in pieces {
            if width(&format!("{line}{piece}")) <= width_cells {
                line.push_str(&piece);
                continue;
            }

            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            for ch in piece.chars() {
                if width(&format!("{line}{ch}")) > width_cells && !line.is_empty() {
                    out.push(std::mem::take(&mut line));
                }
                line.push(ch);
            }
        }
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// Sanitize each hard line before measuring its display cells.
pub(super) fn wrap_sanitized(text: &str, width_cells: usize) -> Vec<String> {
    text.split('\n')
        .flat_map(|line| wrap(&sanitize(line), width_cells))
        .collect()
}

/// The card's rows at `width` cells: the frame takes four, the text the rest.
pub fn lines(comment: &Comment, width_cells: usize, orphan: bool) -> Vec<CardLine> {
    let inner = width_cells.max(MIN_WIDTH) - 4;
    let dim = matches!(comment.state, CommentState::Sent(_));
    let mut lines = vec![CardLine::Top {
        title: title(comment, orphan),
        tone: tone(comment.category),
        dim,
    }];

    lines.extend(
        wrap_sanitized(&comment.text, inner)
            .into_iter()
            .map(CardLine::Text),
    );
    lines.push(CardLine::Bottom);
    lines
}

fn rule(cells: usize) -> Span {
    Span::new("─".repeat(cells), Style::role(Role::Rule))
}

/// `╭─ <title> ───╮`, exactly `width_cells` wide; the title is cut before the frame is.
pub fn frame_top(title: Vec<Span>, width_cells: usize) -> Line {
    let width_cells = width_cells.max(MIN_WIDTH);
    let mut line = vec![Span::new("╭─ ", Style::role(Role::Rule))];
    let room = width_cells - 5; // "╭─ " and " ╮" around the title
    let mut used = 0;
    for span in title {
        if used >= room {
            break;
        }
        let text = truncate(&sanitize(&span.text), room - used);
        used += width(&text);
        line.push(Span::new(text, span.style));
    }
    line.push(Span::body(" "));
    line.push(rule(room - used));
    line.push(Span::new("╮", Style::role(Role::Rule)));
    line
}

/// `│ <text> │`, exactly `width_cells` wide.
pub fn frame_text(text: &str, width_cells: usize) -> Line {
    let width_cells = width_cells.max(MIN_WIDTH);
    let inner = width_cells - 4;
    let text = truncate(&sanitize(text), inner);
    let pad = inner - width(&text);
    vec![
        Span::new("│ ", Style::role(Role::Rule)),
        Span::body(format!("{text}{}", " ".repeat(pad))),
        Span::new(" │", Style::role(Role::Rule)),
    ]
}

/// `╰─ <footer> ──╯`; an empty footer draws a plain rule.
pub fn frame_bottom(footer: &str, width_cells: usize) -> Line {
    let width_cells = width_cells.max(MIN_WIDTH);
    if footer.is_empty() {
        return vec![
            Span::new("╰", Style::role(Role::Rule)),
            rule(width_cells - 2),
            Span::new("╯", Style::role(Role::Rule)),
        ];
    }
    let room = width_cells - 5;
    let footer = truncate(&sanitize(footer), room);
    vec![
        Span::new("╰─ ", Style::role(Role::Rule)),
        Span::label(footer.clone()),
        Span::body(" "),
        rule(room - width(&footer)),
        Span::new("╯", Style::role(Role::Rule)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::engine::comments::{
        Anchor, AnchorComparison, Category, Comment, CommentState, Span as AnchorSpan, Stamp,
    };
    use crate::engine::nav::Side;
    use crate::engine::target::Destination;
    use crate::engine::FileKey;
    use crate::tui::style::{Semantic, Span};

    fn test_stamp() -> Stamp {
        Stamp {
            at: 1,
            nonce: "abc123".into(),
            item: 1,
            to: Destination::clipboard(),
        }
    }

    fn test_comment() -> Comment {
        Comment {
            id: "card".into(),
            anchor: Anchor {
                key: FileKey {
                    path: "src/cart.py".into(),
                    staged: false,
                    untracked: false,
                },
                side: Side::Additions,
                line: 16,
                span: AnchorSpan::Line,
                comparison: AnchorComparison::Worktree,
            },
            category: Category::Bug,
            text: "needs a test".into(),
            created_at: 1,
            state: CommentState::Pending,
        }
    }

    #[test]
    fn a_card_wraps_wide_characters_by_cells() {
        let lines = wrap("字字字字字字", 7); // each 2 cells: three per 7-cell line
        assert_eq!(lines, ["字字字", "字字字"]);
        let lines = wrap("one two three four", 9);
        assert_eq!(lines, ["one two ", "three ", "four"]);
        assert_eq!(wrap("", 9), [""]);
        assert_eq!(wrap("abcdefghijkl", 5), ["abcde", "fghij", "kl"]);
        assert_eq!(
            wrap("字字字字", 3),
            ["字", "字", "字", "字"],
            "a cut never lands inside a character"
        );
        assert_eq!(wrap("a\nb", 9), ["a", "b"], "a newline is a hard break");
        assert_eq!(
            wrap("  indented\n    deeper", 20),
            ["  indented", "    deeper"],
            "indentation is content"
        );
        assert_eq!(wrap("one two", 7), ["one two"]);
        assert_eq!(
            wrap("one two three", 8),
            ["one two ", "three"],
            "the space that broke the line stays with it"
        );
        let text = "  a  b   c";
        assert_eq!(
            wrap(text, 4).concat(),
            text,
            "every character, spaces included, appears exactly once"
        );
        let text = "字".repeat(4_000);
        assert_eq!(wrap(&text, 80).len(), 100);
    }

    #[test]
    fn titles_name_the_category_state_and_for_an_orphan_the_place() {
        let mut c = test_comment();
        assert_eq!(title(&c, false), "Bug · pending");
        c.state = crate::engine::comments::CommentState::Unconfirmed {
            stamp: test_stamp(),
            before: Vec::new(),
        };
        assert_eq!(title(&c, false), "Bug · sent?");
        c.state = crate::engine::comments::CommentState::Sent(test_stamp());
        assert_eq!(title(&c, true), "src/cart.py:16 · Bug · sent");
        c.anchor.span = crate::engine::comments::Span::Range { end: 20 };
        assert_eq!(place(&c.anchor), ":16-20");
        c.anchor.span = crate::engine::comments::Span::File;
        assert_eq!(place(&c.anchor), " (file)");
        assert_eq!(tone(Category::Question), Some(Semantic::Accent));
        assert_eq!(tone(Category::Bug), Some(Semantic::Warn));
        assert_eq!(tone(Category::Change), None);
    }

    #[test]
    fn the_frame_fits_its_width_exactly() {
        let top = frame_top(vec![Span::body("Bug · pending")], 30);
        let text: String = top.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(crate::tui::format::width(&text), 30);
        assert!(text.starts_with("╭─ Bug · pending ─") && text.ends_with("╮"));
        let body: String = frame_text("hello", 30)
            .iter()
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(crate::tui::format::width(&body), 30);
        assert!(body.starts_with("│ hello") && body.ends_with("│"));
        let bottom: String = frame_bottom("", 30)
            .iter()
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(bottom, format!("╰{}╯", "─".repeat(28)));
        // A control character in the text never reaches a cell.
        let body: String = frame_text("a\u{1b}[31mb", 30)
            .iter()
            .map(|s| s.text.as_str())
            .collect();
        assert!(!body.contains('\u{1b}'));
    }

    #[test]
    fn sanitized_card_and_editor_text_still_fit_the_wrapped_cells() {
        let mut comment = test_comment();
        comment.text = format!("{}\n{}", "字a\u{301}".repeat(1_000), "\u{202e}".repeat(32));
        let expected = comment.text.split('\n').map(sanitize).collect::<String>();
        let mut visible = String::new();
        for line in lines(&comment, 24, false) {
            if let CardLine::Text(text) = line {
                assert!(width(&text) <= 20, "{text:?} exceeds the frame");
                visible.push_str(&text);
            }
        }
        assert_eq!(visible, expected);
        let editor = crate::tui::review::Editor::edit(&comment);
        let text: Vec<_> = editor
            .lines(24)
            .into_iter()
            .filter_map(|line| match line {
                CardLine::Text(text) => Some(text),
                _ => None,
            })
            .collect();
        assert!(text.iter().all(|t| width(t) <= 20));
        assert_eq!(text.concat(), format!("{expected}_"));
    }
}
