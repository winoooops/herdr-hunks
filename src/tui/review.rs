//! The editor card and the visual selection of spec 10.3.
use crate::engine::comments::CommentState;
use crate::engine::comments::{self, Anchor, Category, Comment, Span as AnchorSpan};
use crate::engine::dispatch::{
    Accepted, CopyRequest, CopyWhat, Refusal, ReviewScope, SendKind, SendOutcome, SendRequest,
};
use crate::engine::nav::Side;
use crate::engine::{
    Command, Destination, DiffState, FileKey, LoadedDiff, Snapshot, Target, TargetState,
};
use crate::tui::cards::CardLine;
use crate::tui::dialog::{self, Panel, Row};
use crate::tui::format::width;
use crate::tui::sanitize::sanitize;
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

    fn try_push(&mut self, ch: char) -> bool {
        let mut candidate = self.text.clone();
        candidate.push(ch);
        if !comments::within_caps(&candidate) {
            self.at_limit = true;
            return false;
        }
        self.text = candidate;
        self.at_limit = false;
        true
    }

    pub fn insert(&mut self, ch: char) -> bool {
        !ch.is_control() && self.try_push(ch)
    }

    pub fn newline(&mut self) -> bool {
        self.try_push('\n')
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxKind {
    Finish,
    Request,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offers {
    pub y: Option<&'static str>,
    pub pick: bool,
    pub copy: bool,
    pub scope_line: bool,
    pub nothing: bool,
}

pub struct ReviewBox {
    pub kind: BoxKind,
    pub scope: ReviewScope,
    /// The engine's refusal, shown in place of the first line; what Y may accept rides beside it.
    pub refusal: Option<(String, Refusal)>,
    /// Every condition accepted with "send anyway" since the box opened, for the same target.
    pub accepted: Accepted,
    /// The `send_seq` at the time of the press; the answer moves past it.
    pub pending: Option<u64>,
    pub drawn: bool,
}

/// Pending comments, unconfirmed ones, and the files they are on, across both scopes.
pub fn counts(snapshot: &Snapshot) -> (usize, usize, usize) {
    let pending = snapshot.comments.iter().filter(|c| c.is_pending()).count();
    let unconfirmed = snapshot
        .comments
        .iter()
        .filter(|c| matches!(c.state, CommentState::Unconfirmed { .. }))
        .count();
    let files: std::collections::HashSet<&str> = snapshot
        .comments
        .iter()
        .filter(|c| {
            matches!(
                c.state,
                CommentState::Pending | CommentState::Unconfirmed { .. }
            )
        })
        .map(|c| c.anchor.key.path.as_str())
        .collect();
    (pending, unconfirmed, files.len())
}

fn count_phrase(pending: usize, unconfirmed: usize) -> String {
    match unconfirmed {
        0 => format!("{pending} comment{}", if pending == 1 { "" } else { "s" }),
        _ => format!("{pending} pending + {unconfirmed} unconfirmed"),
    }
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

fn pane_label(target: &Target) -> (String, String) {
    match target {
        Target::Pane { pane, agent, .. } => (sanitize(agent), sanitize(pane)),
        Target::Clipboard => ("clipboard".into(), String::new()),
    }
}

impl ReviewBox {
    pub fn finish() -> Self {
        Self {
            kind: BoxKind::Finish,
            scope: ReviewScope::All,
            refusal: None,
            accepted: Accepted::default(),
            pending: None,
            drawn: false,
        }
    }

    /// The scope starts on all changes (spec 10.4).
    pub fn request(_snapshot: &Snapshot) -> Self {
        Self {
            kind: BoxKind::Request,
            scope: ReviewScope::All,
            refusal: None,
            accepted: Accepted::default(),
            pending: None,
            drawn: false,
        }
    }

    /// A refusal the engine typed: shown in the box, and folded into what the next Y accepts.
    pub fn refuse(&mut self, message: String, refusal: Refusal) {
        match &refusal {
            Refusal::Busy => self.accepted.busy = true,
            // `Some(None)` accepts a restart into an agent that reports no session.
            Refusal::Restarted(session) => self.accepted.restarted = Some(session.clone()),
            Refusal::Other => {}
        }
        self.refusal = Some((message, refusal));
        self.pending = None;
    }

    pub fn scope_available(&self, snapshot: &Snapshot, scope: &ReviewScope) -> bool {
        match scope {
            ReviewScope::All => !snapshot.files.is_empty(),
            ReviewScope::File(key) => {
                matches!(&snapshot.diff, DiffState::Ready(d) if &d.key == key)
            }
        }
    }

    fn selected_key(snapshot: &Snapshot) -> Option<FileKey> {
        snapshot.selected.clone()
    }

    /// The state line of 10.4's table and what the keys row offers; the refusal replaces the state line.
    fn state_line(&self, snapshot: &Snapshot) -> (String, Offers) {
        let (pending, unconfirmed, files) = counts(snapshot);
        let what = count_phrase(pending, unconfirmed);
        let Some(target) = &snapshot.target else {
            return (
                "no target: press A".into(),
                Offers {
                    y: None,
                    pick: true,
                    copy: true,
                    scope_line: false,
                    nothing: false,
                },
            );
        };
        let (agent, pane) = pane_label(target);
        let all = Offers {
            y: Some("send"),
            pick: true,
            copy: true,
            scope_line: false,
            nothing: false,
        };
        let anyway = Offers {
            y: Some("send anyway"),
            ..all
        };
        let no_y = Offers { y: None, ..all };
        // A refusal may relabel Y, but cannot offer more than the live table allows.
        let (line, table) =
            self.table_line(snapshot, &agent, &pane, &what, files, all, anyway, no_y);
        if let Some((message, refusal)) = &self.refusal {
            // A plain send under an accepted refusal reads "send anyway"; nothing else is relabelled.
            let y = match (table.y, refusal) {
                (Some("send") | Some("delegate"), Refusal::Busy | Refusal::Restarted(_)) => {
                    Some("send anyway")
                }
                (y, _) => y,
            };
            return (sanitize(message), Offers { y, ..table });
        }
        (line, table)
    }

    #[allow(clippy::too_many_arguments)]
    fn table_line(
        &self,
        snapshot: &Snapshot,
        agent: &str,
        pane: &str,
        what: &str,
        files: usize,
        all: Offers,
        anyway: Offers,
        no_y: Offers,
    ) -> (String, Offers) {
        match (&snapshot.target_state, self.kind) {
            (TargetState::Clipboard, BoxKind::Finish) => (
                format!("Copy {what} across {} to the clipboard?", plural(files, "file")),
                Offers { y: Some("copy"), pick: true, copy: false, scope_line: false, nothing: false },
            ),
            (TargetState::Clipboard, BoxKind::Request) => (
                "Copy the review request to the clipboard?".into(),
                Offers { y: Some("copy"), pick: true, copy: false, scope_line: false, nothing: false },
            ),
            (TargetState::NoHost, _) => ("No host: this viewer runs outside herdr.".into(), Offers { y: None, pick: false, copy: true, scope_line: false, nothing: false }),
            (TargetState::Live(s) | TargetState::Restarted(s), _) if s == "blocked" => (
                format!("{agent} is waiting for an approval in {pane}. Answer it there, or press A to pick another pane."),
                no_y,
            ),
            (TargetState::Restarted(_), _) => (format!("{agent} in {pane} was restarted since you picked it and has not seen earlier messages."), anyway),
            (TargetState::Live(s), _) if s == "working" => (format!("{agent} is working in {pane}; the review would queue behind its current turn."), anyway),
            (TargetState::Live(s), _) if s == "unknown" => (format!("{agent}'s state in {pane} is unknown to the host."), anyway),
            // A status outside the documented five is as good as unknown (the engine's gate agrees).
            (TargetState::Live(s), _) if s != "idle" && s != "done" => (format!("{agent} reports {} in {pane}, a state this viewer does not know.", sanitize(s)), anyway),
            (TargetState::Unverified, _) => (format!("{pane} has not been checked; it will be before sending."), all),
            (TargetState::Live(_), BoxKind::Finish) => (format!("Send {what} across {} to {agent} · {pane}?", plural(files, "file")), all),
            (TargetState::Live(_), BoxKind::Request) => (
                format!("Delegate a review of {} to {agent} · {pane}?", match &self.scope { ReviewScope::All => "all changes".to_string(), ReviewScope::File(k) => sanitize(&k.path) }),
                Offers { y: Some("delegate"), ..all },
            ),
            // Left and Gone never reach the box: the key opens the picker instead.
            (TargetState::Left | TargetState::Gone, _) => (format!("{agent} · {pane} is gone · pick a pane"), no_y),
        }
    }

    pub fn offers(&self, snapshot: &Snapshot) -> Offers {
        let (_, mut offers) = self.state_line(snapshot);
        if self.kind == BoxKind::Request {
            let one_loaded_row = snapshot.files.len() == 1
                && self.scope_available(snapshot, &ReviewScope::All)
                && Self::selected_key(snapshot)
                    .is_some_and(|k| self.scope_available(snapshot, &ReviewScope::File(k)));
            offers.scope_line = !snapshot.files.is_empty() && !one_loaded_row;
            offers.nothing = snapshot.files.is_empty();
            if offers.nothing {
                // Nothing to review: `n` alone (10.4), neither a pane to pick for it nor a copy of it.
                offers = Offers {
                    y: None,
                    pick: false,
                    copy: false,
                    scope_line: false,
                    nothing: true,
                };
            } else if !self.scope_available(snapshot, &self.scope) {
                offers.y = None;
            }
            if offers.y == Some("send") {
                offers.y = Some("delegate");
            }
        }
        offers
    }

    pub fn panel(&self, snapshot: &Snapshot, columns: u16) -> Panel {
        let (line, offers) = (self.state_line(snapshot).0, self.offers(snapshot));
        let mut rows = Vec::new();
        if offers.nothing {
            rows.push(Row::Text("nothing to review".into()));
        } else {
            if offers.scope_line {
                let n = snapshot.files.len();
                // The chosen scope is drawn in reverse video by view.rs from this marker pair.
                rows.push(Row::Text(format!(
                    "Scope  f this file   a all changes ({n})"
                )));
            }
            rows.push(Row::Note(line));
        }
        let mut keys = Vec::new();
        if let Some(y) = offers.y {
            keys.push(format!("Y {y}"));
        }
        if offers.pick {
            keys.push("A pick another pane".into());
        }
        if offers.copy {
            keys.push("c copy".into());
        }
        keys.push("n cancel".into());
        if width(&keys.join(" · ")) + 3 > usize::from(columns) {
            keys = Vec::new();
            if let Some(y) = offers.y {
                keys.push(format!("Y {y}"));
            }
            if offers.pick {
                keys.push("A".into());
            }
            if offers.copy {
                keys.push("c".into());
            }
            keys.push("n cancel".into());
        }
        if self.pending.is_some() {
            keys = vec![if snapshot.send_waiting {
                crate::engine::dispatch::NOTICE_WAITING.to_string()
            } else {
                "sending…".to_string()
            }];
        }
        Panel {
            title: match self.kind {
                BoxKind::Finish => "Finish",
                BoxKind::Request => "Request review",
            }
            .into(),
            rows,
            footer: keys.join(" · "),
            cursor: None,
            offset: 0,
        }
    }

    /// Whether the whole box fits the frame, as for the hunk confirmation.
    pub fn fits(&self, snapshot: &Snapshot, columns: u16, height: u16) -> bool {
        let panel_width = columns.min(72);
        columns >= 40
            && height >= 10
            && dialog::line_count(&self.panel(snapshot, panel_width), panel_width) + 4
                <= usize::from(height) - 2
    }

    /// What Y sends: the kind, and every condition accepted so far plus the one the box shows now.
    pub fn submit(&self, snapshot: &Snapshot) -> Option<Command> {
        let offers = self.offers(snapshot);
        offers.y?;
        let mut accepted = self.accepted.clone();
        if offers.y == Some("send anyway") && self.refusal.is_none() {
            // The chip's own state is what the box showed: a busy agent, or a restart.
            match &snapshot.target_state {
                TargetState::Live(_) => accepted.busy = true,
                TargetState::Restarted(_) => {
                    // The engine returns the new session in its typed refusal for the next Y.
                    accepted.restarted = Some(snapshot.target.as_ref().and_then(|t| match t {
                        Target::Pane { session, .. } => session.clone(),
                        _ => None,
                    }));
                }
                _ => {}
            }
        }
        let kind = match self.kind {
            BoxKind::Finish => SendKind::Feedback,
            BoxKind::Request => SendKind::Review {
                scope: self.scope.clone(),
            },
        };
        Some(Command::Send(SendRequest { kind, accepted }))
    }

    pub fn copy(&self) -> Command {
        Command::Copy(CopyRequest {
            what: match self.kind {
                BoxKind::Finish => CopyWhat::Review,
                BoxKind::Request => CopyWhat::Request {
                    scope: self.scope.clone(),
                },
            },
        })
    }
}

/// The notice for a successful send (spec 10.4): urgent when the host did not confirm.
pub fn outcome_notice(outcome: &SendOutcome, _snapshot: &Snapshot) -> (String, bool) {
    if let Some(copy) = &outcome.copy {
        return (copy.notice.clone(), copy.urgent || outcome.unconfirmed);
    }
    let to = match &outcome.to {
        Destination::Pane { pane, agent, .. } => {
            format!("{} · {}", sanitize(agent), sanitize(pane))
        }
        Destination::Clipboard { .. } => "the clipboard".to_string(),
    };
    if outcome.unconfirmed {
        return (
            format!(
                "sent? the host did not confirm · {} marked sent? until the next finish",
                plural(outcome.items as usize, "comment")
            ),
            true,
        );
    }
    match outcome.kind {
        SendKind::Feedback => (
            format!("sent {} to {to}", plural(outcome.items as usize, "item")),
            false,
        ),
        SendKind::Review { .. } => (format!("review requested from {to}"), false),
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

    use crate::engine::comments::Stamp;
    use crate::engine::dispatch::{Accepted, CopyOut, Refusal, ReviewScope, SendKind, SendOutcome};
    use crate::engine::host::SessionRef;
    use crate::engine::{Destination, Snapshot, Target, TargetState};
    use crate::tui::dialog::{Panel, Row};
    use crate::tui::input::tests::comment_at;

    fn box_anchor(path: &str, line: u32) -> Anchor {
        let mut a = anchor();
        a.key.path = path.into();
        a.line = line;
        a
    }

    fn stamp(nonce: &str) -> Stamp {
        Stamp {
            at: 1,
            nonce: nonce.into(),
            item: 1,
            to: Destination::clipboard(),
        }
    }
    fn with_comments(
        target: Option<Target>,
        state: TargetState,
        pending: usize,
        unconfirmed: usize,
    ) -> Snapshot {
        let mut s = crate::tui::state::tests::snapshot("src/cart.py", "raw", &[(10, " + ")]);
        // `snapshot` lists no files; the box's scope rules read `files` and `selected`.
        s.files = vec![crate::git::ChangedFile {
            path: "src/cart.py".into(),
            status: crate::git::ChangedFileStatus::Modified,
            staged: false,
            insertions: None,
            deletions: None,
        }];
        s.selected = Some(FileKey {
            path: "src/cart.py".into(),
            staged: false,
            untracked: false,
        });
        s.target = target;
        s.target_state = state;
        let mut comments = Vec::new();
        for i in 0..pending {
            comments.push(comment_at(
                &box_anchor("src/cart.py", 10),
                &format!("p{i}"),
                i as u64,
            ));
        }
        for i in 0..unconfirmed {
            let mut c = comment_at(&box_anchor("other.rs", 1), &format!("u{i}"), 100 + i as u64);
            c.state = CommentState::Unconfirmed {
                before: Vec::new(),
                stamp: stamp("aaaaaa"),
            };
            comments.push(c);
        }
        s.comments = std::sync::Arc::new(comments);
        s
    }

    fn pane() -> Target {
        Target::Pane {
            pane: "w4:p2".into(),
            socket: "/s".into(),
            agent: "codex".into(),
            session: None,
            title: "demo".into(),
        }
    }

    fn texts(panel: &Panel) -> Vec<String> {
        panel
            .rows
            .iter()
            .filter_map(|r| match r {
                Row::Text(t) | Row::Note(t) | Row::Warn(t) => Some(t.clone()),
                Row::Entry { label, value, .. } => Some(format!("{label} {value}")),
                Row::Rule => None,
            })
            .collect()
    }

    #[test]
    fn the_finish_box_says_what_the_table_says_for_each_state() {
        let cases: Vec<(TargetState, &str, Option<&str>, bool, bool)> = vec![
            (TargetState::Live("idle".into()), "Send 2 comments across 1 file to codex · w4:p2?", Some("send"), true, true),
            (TargetState::Live("done".into()), "Send 2 comments across 1 file to codex · w4:p2?", Some("send"), true, true),
            (TargetState::Live("working".into()), "codex is working in w4:p2; the review would queue behind its current turn.", Some("send anyway"), true, true),
            (TargetState::Live("unknown".into()), "codex's state in w4:p2 is unknown to the host.", Some("send anyway"), true, true),
            (TargetState::Live("sleeping".into()), "codex reports sleeping in w4:p2, a state this viewer does not know.", Some("send anyway"), true, true),
            (TargetState::Unverified, "w4:p2 has not been checked; it will be before sending.", Some("send"), true, true),
            (TargetState::Live("blocked".into()), "codex is waiting for an approval in w4:p2. Answer it there, or press A to pick another pane.", None, true, true),
            (TargetState::Restarted("blocked".into()), "codex is waiting for an approval in w4:p2. Answer it there, or press A to pick another pane.", None, true, true),
            (TargetState::Restarted("idle".into()), "codex in w4:p2 was restarted since you picked it and has not seen earlier messages.", Some("send anyway"), true, true),
            (TargetState::NoHost, "No host: this viewer runs outside herdr.", None, false, true),
        ];
        for (state, first, y, pick, copy) in cases {
            let s = with_comments(Some(pane()), state.clone(), 2, 0);
            let b = ReviewBox::finish();
            let panel = b.panel(&s, 60);
            assert_eq!(texts(&panel)[0], first, "{state:?}");
            let offers = b.offers(&s);
            assert_eq!(
                (offers.y, offers.pick, offers.copy),
                (y, pick, copy),
                "{state:?}"
            );
            assert_eq!(panel.title, "Finish");
        }
        let s = with_comments(Some(Target::Clipboard), TargetState::Clipboard, 2, 1);
        let panel = ReviewBox::finish().panel(&s, 60);
        assert_eq!(
            texts(&panel)[0],
            "Copy 2 pending + 1 unconfirmed across 2 files to the clipboard?"
        );
        assert_eq!(ReviewBox::finish().offers(&s).y, Some("copy"));
        assert!(
            !ReviewBox::finish().offers(&s).copy,
            "copy is the Y of a clipboard target"
        );
        // The counts name unconfirmed comments separately, across both scopes.
        let s = with_comments(Some(pane()), TargetState::Live("idle".into()), 1, 2);
        assert_eq!(
            texts(&ReviewBox::finish().panel(&s, 60))[0],
            "Send 1 pending + 2 unconfirmed across 2 files to codex · w4:p2?"
        );
        assert_eq!(counts(&s), (1, 2, 2));
    }

    #[test]
    fn y_carries_what_the_box_showed_and_a_refusal_relabels_it() {
        let s = with_comments(Some(pane()), TargetState::Live("working".into()), 1, 0);
        let b = ReviewBox::finish();
        let Some(Command::Send(request)) = b.submit(&s) else {
            panic!()
        };
        assert_eq!(
            request.accepted,
            Accepted {
                busy: true,
                restarted: None
            }
        );
        let s = with_comments(Some(pane()), TargetState::Live("idle".into()), 1, 0);
        let Some(Command::Send(request)) = ReviewBox::finish().submit(&s) else {
            panic!()
        };
        assert_eq!(request.accepted, Accepted::default());
        // The next Y accepts the typed refusal.
        let mut refused = ReviewBox::finish();
        let session = SessionRef {
            kind: "id".into(),
            value: "s2".into(),
        };
        refused.refuse(
            "codex in w4:p2 was restarted since you picked it and has not seen earlier messages."
                .into(),
            Refusal::Restarted(Some(session.clone())),
        );
        assert_eq!(
            texts(&refused.panel(&s, 60))[0],
            "codex in w4:p2 was restarted since you picked it and has not seen earlier messages."
        );
        assert_eq!(refused.offers(&s).y, Some("send anyway"));
        let Some(Command::Send(request)) = refused.submit(&s) else {
            panic!()
        };
        assert_eq!(
            request.accepted,
            Accepted {
                busy: false,
                restarted: Some(Some(session.clone()))
            }
        );
        // Accepting another gate preserves the earlier acceptance.
        refused.refuse(
            "codex is working in w4:p2; the review would queue behind its current turn.".into(),
            Refusal::Busy,
        );
        let Some(Command::Send(request)) = refused.submit(&s) else {
            panic!()
        };
        assert_eq!(
            request.accepted,
            Accepted {
                busy: true,
                restarted: Some(Some(session))
            },
            "a restarted and working agent asks twice, not forever"
        );
        // A sessionless restart is an explicit acceptance.
        let mut sessionless = ReviewBox::finish();
        sessionless.refuse(
            "codex in w4:p2 was restarted since you picked it and has not seen earlier messages."
                .into(),
            Refusal::Restarted(None),
        );
        let Some(Command::Send(request)) = sessionless.submit(&s) else {
            panic!()
        };
        assert_eq!(request.accepted.restarted, Some(None));
        let mut other = ReviewBox::finish();
        other.refuse("could not verify w4:p2: deadline".into(), Refusal::Other);
        assert_eq!(
            other.offers(&s).y,
            Some("send"),
            "an unacceptable refusal keeps a plain retry"
        );
        // The current state still limits the offered keys.
        let blocked = with_comments(Some(pane()), TargetState::Live("blocked".into()), 1, 0);
        assert_eq!(
            refused.offers(&blocked).y,
            None,
            "send anyway is withdrawn while the agent is blocked"
        );
        assert_eq!(
            texts(&refused.panel(&blocked, 60))[0],
            "codex is working in w4:p2; the review would queue behind its current turn.",
            "the refusal is still what the box says"
        );
        let no_host = with_comments(Some(pane()), TargetState::NoHost, 1, 0);
        let offers = other.offers(&no_host);
        assert!(offers.y.is_none() && !offers.pick && offers.copy);
        // Blocked offers no Y at all.
        let s = with_comments(Some(pane()), TargetState::Live("blocked".into()), 1, 0);
        assert!(ReviewBox::finish().submit(&s).is_none());
    }

    #[test]
    fn the_request_box_scopes_as_the_spec_says() {
        let mut s = with_comments(Some(pane()), TargetState::Live("idle".into()), 0, 0);
        // Two rows, a diff loaded: all changes first, both available.
        s.files.push(crate::git::ChangedFile {
            path: "b.rs".into(),
            status: crate::git::ChangedFileStatus::Modified,
            staged: false,
            insertions: None,
            deletions: None,
        });
        let b = ReviewBox::request(&s);
        assert_eq!(b.scope, ReviewScope::All);
        assert_eq!(
            texts(&b.panel(&s, 60))[0],
            "Scope  f this file   a all changes (2)"
        );
        assert_eq!(
            texts(&b.panel(&s, 60))[1],
            "Delegate a review of all changes to codex · w4:p2?"
        );
        assert_eq!(b.offers(&s).y, Some("delegate"));
        assert!(b.scope_available(
            &s,
            &ReviewScope::File(FileKey {
                path: "src/cart.py".into(),
                staged: false,
                untracked: false
            })
        ));
        // One row whose diff is loaded: the scopes coincide, no scope line.
        s.files.truncate(1);
        let b = ReviewBox::request(&s);
        assert!(!b.offers(&s).scope_line);
        // One row whose diff is not loaded: the line is drawn with f unavailable.
        s.diff = DiffState::Loading;
        let b = ReviewBox::request(&s);
        assert!(b.offers(&s).scope_line);
        assert!(!b.scope_available(
            &s,
            &ReviewScope::File(FileKey {
                path: "src/cart.py".into(),
                staged: false,
                untracked: false
            })
        ));
        assert_eq!(
            b.offers(&s).y,
            Some("delegate"),
            "all changes is available with any row"
        );
        // An empty list: nothing to review, n alone.
        s.files.clear();
        let b = ReviewBox::request(&s);
        let offers = b.offers(&s);
        assert!(offers.nothing && offers.y.is_none() && !offers.pick && !offers.copy);
        let panel = b.panel(&s, 60);
        assert_eq!(texts(&panel)[0], "nothing to review");
        assert_eq!(panel.footer, "n cancel");
        assert!(b.submit(&s).is_none());
    }

    #[test]
    fn outcome_notices_name_the_destination_and_the_unconfirmed_case_is_urgent() {
        let s = with_comments(Some(pane()), TargetState::Live("idle".into()), 0, 0);
        let sent = SendOutcome {
            kind: SendKind::Feedback,
            items: 2,
            to: Destination::Pane {
                pane: "w4:p2".into(),
                agent: "codex".into(),
                session: None,
            },
            unconfirmed: false,
            copy: None,
        };
        assert_eq!(
            outcome_notice(&sent, &s),
            ("sent 2 items to codex · w4:p2".to_string(), false)
        );
        let one = SendOutcome {
            items: 1,
            ..sent.clone()
        };
        assert_eq!(outcome_notice(&one, &s).0, "sent 1 item to codex · w4:p2");
        let unsure = SendOutcome {
            unconfirmed: true,
            ..sent.clone()
        };
        assert_eq!(
            outcome_notice(&unsure, &s),
            (
                "sent? the host did not confirm · 2 comments marked sent? until the next finish"
                    .to_string(),
                true
            )
        );
        let requested = SendOutcome {
            kind: SendKind::Review {
                scope: ReviewScope::All,
            },
            ..sent.clone()
        };
        assert_eq!(
            outcome_notice(&requested, &s).0,
            "review requested from codex · w4:p2"
        );
        let copied = SendOutcome {
            to: Destination::clipboard(),
            copy: Some(CopyOut {
                osc: None,
                notice: "copied 2 comments · also in ~/x/clipboard.md".into(),
                urgent: false,
            }),
            ..sent
        };
        assert_eq!(
            outcome_notice(&copied, &s).0,
            "copied 2 comments · also in ~/x/clipboard.md"
        );
    }
}
