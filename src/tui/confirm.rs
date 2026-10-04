//! The y/n box of spec 9.3: one reusable dialog panel, its keys handled in one place (input.rs).
use std::sync::Arc;

use crate::engine::{Action, ActionKind, LoadedDiff};
use crate::tui::dialog::{self, Panel, Row};
use crate::tui::format::{truncate_left, width};
use crate::tui::sanitize::sanitize;

pub const FOOTER: &str = "y yes · n no";
const CANNOT_UNDO: &str = "This cannot be undone.";
const NOT_IN_GIT: &str = "It is not in git and cannot be recovered.";

pub struct Confirm {
    pub action: Action,
    pub title: &'static str,
    pub warning: Option<&'static str>,
    /// Set after each frame: the whole box was on screen (spec 9.3), so `y` may confirm.
    pub drawn: bool,
    /// The success notice (`staged hunk 2/3 of src/x`).
    pub done: String,
    /// The failure prefix (`stage`).
    pub verb: &'static str,
    before: String,
    after: &'static str,
}

impl Confirm {
    pub fn open(
        kind: ActionKind,
        diff: Arc<LoadedDiff>,
        hunk: Option<usize>,
        hunk_total: usize,
    ) -> Self {
        let path = sanitize(&diff.key.path);
        let position = hunk
            .map(|h| format!("{}/{}", h + 1, hunk_total))
            .unwrap_or_default();
        let (title, before, after, warning, done, verb) =
            match (diff.key.untracked, kind, diff.key.staged) {
                (true, ActionKind::Stage, _) => (
                    "Stage file?",
                    "Add ".to_string(),
                    " to the index?",
                    None,
                    format!("staged {path}"),
                    "stage",
                ),
                (true, _, _) => (
                    "Delete untracked file?",
                    "Delete ".to_string(),
                    "?",
                    Some(NOT_IN_GIT),
                    format!("deleted {path}"),
                    "delete",
                ),
                (false, ActionKind::Stage, false) => (
                    "Stage hunk?",
                    format!("Stage hunk {position} of "),
                    "?",
                    None,
                    format!("staged hunk {position} of {path}"),
                    "stage",
                ),
                (false, ActionKind::Stage, true) => (
                    "Unstage hunk?",
                    format!("Move hunk {position} of "),
                    " out of the index?",
                    None,
                    format!("unstaged hunk {position} of {path}"),
                    "unstage",
                ),
                (false, ActionKind::Discard, _) => (
                    "Discard hunk?",
                    format!("Discard hunk {position} of "),
                    "?",
                    Some(CANNOT_UNDO),
                    format!("discarded hunk {position} of {path}"),
                    "discard",
                ),
                (false, ActionKind::DiscardFile, _) => (
                    "Discard file?",
                    "Discard every change this row shows for ".to_string(),
                    "?",
                    Some(CANNOT_UNDO),
                    format!("discarded {path}"),
                    "discard",
                ),
            };
        Self {
            action: Action { kind, diff, hunk },
            title,
            warning,
            drawn: false,
            done,
            verb,
            before,
            after,
        }
    }

    fn rows_with(&self, body: String) -> Vec<Row> {
        let mut rows = vec![Row::Note(body)];
        if let Some(warning) = self.warning {
            rows.push(Row::Warn(warning.into()));
        }
        rows
    }

    /// The body at `columns`: the path shortened from the left until the dialog's own wrapping
    /// of the body and the warning fits four lines, so the frame makes eight.
    pub fn body(&self, columns: u16) -> String {
        let full = sanitize(&self.action.diff.key.path);
        let mut keep = width(&full);
        loop {
            let body = format!(
                "{}{}{}",
                self.before,
                truncate_left(&full, keep),
                self.after
            );
            let probe = Panel {
                title: String::new(),
                rows: self.rows_with(body.clone()),
                footer: String::new(),
                cursor: None,
                offset: 0,
            };
            if dialog::line_count(&probe, columns) <= 4 || keep <= 2 {
                return body;
            }
            keep -= 1;
        }
    }

    pub fn panel(&self, columns: u16) -> Panel {
        Panel {
            title: self.title.into(),
            rows: self.rows_with(self.body(columns)),
            footer: FOOTER.into(),
            cursor: None,
            offset: 0,
        }
    }

    /// Whether the whole box fits a frame of `columns` by `height`: the drawn rule of spec 9.3.
    pub fn fits(&self, columns: u16, height: u16) -> bool {
        let panel_width = columns.min(60);
        columns >= 40
            && height >= 10
            && dialog::line_count(&self.panel(panel_width), panel_width) + 4
                <= usize::from(height) - 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::DiffState;
    use crate::tui::dialog;
    use crate::tui::state::tests::snapshot;

    fn diff(path: &str) -> Arc<LoadedDiff> {
        match snapshot(path, "r", &[(1, "-+"), (9, "+")]).diff {
            DiffState::Ready(d) => d,
            _ => unreachable!(),
        }
    }

    #[test]
    fn each_kind_has_its_title_body_and_warning() {
        let c = Confirm::open(ActionKind::Stage, diff("src/values.ts"), Some(1), 2);
        assert_eq!(
            (c.title, c.body(60).as_str(), c.warning),
            ("Stage hunk?", "Stage hunk 2/2 of src/values.ts?", None)
        );
        assert_eq!(
            (c.done.as_str(), c.verb),
            ("staged hunk 2/2 of src/values.ts", "stage")
        );
        let c = Confirm::open(ActionKind::Discard, diff("src/values.ts"), Some(0), 2);
        assert_eq!(
            (c.title, c.body(60).as_str(), c.warning),
            (
                "Discard hunk?",
                "Discard hunk 1/2 of src/values.ts?",
                Some(CANNOT_UNDO)
            )
        );
        let c = Confirm::open(ActionKind::DiscardFile, diff("src/values.ts"), None, 2);
        assert_eq!(
            c.body(60),
            "Discard every change this row shows for src/values.ts?"
        );
        let mut untracked = (*diff("u.txt")).clone();
        untracked.key.untracked = true;
        let c = Confirm::open(ActionKind::Discard, Arc::new(untracked.clone()), None, 1);
        assert_eq!(
            (c.title, c.body(60).as_str(), c.warning, c.verb),
            (
                "Delete untracked file?",
                "Delete u.txt?",
                Some(NOT_IN_GIT),
                "delete"
            )
        );
        let c = Confirm::open(ActionKind::Stage, Arc::new(untracked), None, 1);
        assert_eq!(
            (c.title, c.body(60).as_str(), c.done.as_str()),
            ("Stage file?", "Add u.txt to the index?", "staged u.txt")
        );
    }

    #[test]
    fn the_box_is_at_most_eight_lines_at_the_minimum_width() {
        let long = "a/very/long/directory/name/that/goes/on/and/on/for/a/while/values.ts";
        let mut untracked = (*diff(long)).clone();
        untracked.key.untracked = true;
        let c = Confirm::open(ActionKind::DiscardFile, Arc::new(untracked), None, 1);
        let panel = c.panel(40);
        assert!(dialog::line_count(&panel, 40) <= 4, "{:?}", c.body(40));
        assert!(c.fits(40, 10) && !c.fits(40, 9) && !c.fits(39, 10));
        assert!(c.body(40).contains("…"));
        assert!(c.body(40).ends_with("values.ts?"));
        let lines = dialog::render(&panel, 40, 8);
        assert_eq!(lines.len(), 8);
        let text: Vec<String> = lines
            .iter()
            .map(|l| l.iter().map(|s| s.text.as_str()).collect())
            .collect();
        // The warning wraps at 40 columns; the whole sentence must be there, across rows.
        let joined: String = text
            .iter()
            .map(|l| l.trim_matches(|c| c == '│' || c == ' '))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            joined.contains("It is not in git and cannot be") && joined.contains("recovered."),
            "{text:?}"
        );
        // The last line is the bottom border; the footer sits above it.
        assert!(text[text.len() - 2].contains(FOOTER), "{text:?}");
    }
}
