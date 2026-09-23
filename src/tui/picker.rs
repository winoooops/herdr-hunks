//! The base picker (spec 7.4): one input line that filters the ref list and takes free text.

use crate::engine::{ref_label, BaseSource, RepoState, Snapshot};
use crate::tui::dialog::{Panel, Row};
use crate::tui::format::truncate;
use crate::tui::sanitize::sanitize;

pub const WIDTH: u16 = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerRow {
    /// `default (<label>)`: forget the pick and re-resolve.
    Reset(String),
    /// `use "<input>"`: the typed text as a base.
    Typed(String),
    /// A listed ref: the qualified name it submits and its markers.
    Ref {
        qualified: String,
        markers: Vec<&'static str>,
    },
}

impl PickerRow {
    /// What `SetBase` is sent; `None` is the reset row.
    pub fn submit(&self) -> Option<String> {
        match self {
            Self::Reset(_) => None,
            Self::Typed(text)
            | Self::Ref {
                qualified: text, ..
            } => Some(text.clone()),
        }
    }
}

#[derive(Debug, Default)]
pub struct Picker {
    pub input: String,
    /// Index into `rows()`.
    pub cursor: usize,
    /// First list row drawn; kept so the window follows the cursor.
    pub offset: usize,
    /// Shown under the input line.
    pub error: Option<String>,
    /// Reply sequence preceding this pick; its answer is the first snapshot beyond it.
    pub pending: Option<u64>,
    /// Set when the answer was a success: the shell drops the picker.
    pub done: bool,
    /// The token sent with this opening's `LoadRefs`.
    pub token: u64,
    /// The last `refs_seq` the cursor was placed for.
    pub seen_refs_seq: u64,
}

impl Picker {
    pub fn open(token: u64) -> Self {
        Self {
            token,
            ..Self::default()
        }
    }

    /// The candidates of this opening; empty until `LoadRefs` is answered, so a stale list is never shown.
    pub fn refs<'a>(&self, snapshot: &'a Snapshot) -> &'a [String] {
        if snapshot.refs_seq == self.token {
            snapshot.refs.as_deref().map(Vec::as_slice).unwrap_or(&[])
        } else {
            &[]
        }
    }

    pub fn rows(&self, snapshot: &Snapshot) -> Vec<PickerRow> {
        let default_label = snapshot
            .default_base
            .as_deref()
            .map(ref_label)
            .unwrap_or("none");
        let mut rows = vec![PickerRow::Reset(format!("default ({default_label})"))];
        let refs = self.refs(snapshot);
        let listed = refs.iter().any(|r| ref_label(r) == self.input);
        if !self.input.is_empty() && !listed {
            rows.push(PickerRow::Typed(self.input.clone()));
        }
        let needle = self.input.to_lowercase();
        let current_base = snapshot.base.as_ref().map(|b| b.requested.as_str());
        let checked_out = match &snapshot.repo {
            RepoState::Repo {
                branch: Some(branch),
                ..
            } => Some(format!("refs/heads/{branch}")),
            _ => None,
        };
        let mut matching: Vec<PickerRow> = refs
            .iter()
            .filter(|r| ref_label(r).to_lowercase().contains(&needle))
            .map(|r| {
                let mut markers = Vec::new();
                if current_base == Some(r.as_str()) {
                    match snapshot.base.as_ref().map(|b| b.source) {
                        Some(BaseSource::Picked) => markers.push("picked"),
                        Some(BaseSource::Config) => markers.push("config"),
                        _ => {}
                    }
                }
                if checked_out.as_deref() == Some(r.as_str()) {
                    markers.push("current");
                }
                if r.starts_with("refs/tags/") {
                    markers.push("tag");
                }
                PickerRow::Ref {
                    qualified: r.clone(),
                    markers,
                }
            })
            .collect();
        // The current base's row comes right after the reset row.
        if let Some(i) = matching.iter().position(
            |row| matches!(row, PickerRow::Ref { qualified, .. } if Some(qualified.as_str()) == current_base),
        ) {
            let row = matching.remove(i);
            matching.insert(0, row);
        }
        rows.extend(matching);
        rows
    }

    fn head(&self) -> usize {
        1 + usize::from(self.error.is_some())
    }

    /// List rows that fit: the frame and footer take four lines, the input and error lines the rest.
    pub fn visible(&self, height: u16) -> usize {
        usize::from(height).saturating_sub(4 + self.head()).max(1)
    }

    /// First list row drawn for a window of `visible` rows: the cursor stays inside it.
    pub fn window(&self, visible: usize) -> usize {
        self.offset
            .min(self.cursor)
            .max(self.cursor.saturating_sub(visible - 1))
    }

    /// `width` is the panel's drawn width: the input and error lines are cut to it so
    /// `dialog::render` never wraps them, and every row is exactly one line.
    pub fn panel(&self, snapshot: &Snapshot, width: u16, height: u16) -> Panel {
        let rows = self.rows(snapshot);
        let listed = rows
            .iter()
            .filter(|r| matches!(r, PickerRow::Ref { .. }))
            .count();
        let footer = if self.pending.is_some() {
            "picking…".to_string()
        } else if snapshot.refs_seq != self.token {
            "loading refs…".to_string()
        } else if self.refs(snapshot).is_empty() {
            "no refs listed; type a revision".to_string()
        } else if snapshot.refs_overflow {
            format!("{listed} shown, more exist · type to filter")
        } else {
            "Enter pick · Esc cancel · type to filter".to_string()
        };
        // Two frame columns and the two-space indent of a note row.
        let line = usize::from(width).saturating_sub(4).max(1);
        let mut panel_rows = vec![Row::Text(truncate(
            &sanitize(&format!("> {}_", self.input)),
            line,
        ))];
        if let Some(error) = &self.error {
            panel_rows.push(Row::Warn(truncate(&sanitize(error), line)));
        }
        let head = panel_rows.len();
        let visible = self.visible(height);
        let offset = self.window(visible);
        for row in rows.iter().skip(offset).take(visible) {
            let (label, value) = match row {
                PickerRow::Reset(text) => (text.clone(), String::new()),
                PickerRow::Typed(text) => (format!("use \"{text}\""), String::new()),
                PickerRow::Ref { qualified, markers } => {
                    (ref_label(qualified).to_string(), markers.join(" · "))
                }
            };
            panel_rows.push(Row::Entry {
                label: truncate(&sanitize(&label), 30),
                value,
                enabled: true,
            });
        }
        Panel {
            title: "Compare against".into(),
            rows: panel_rows,
            footer,
            cursor: Some(head + self.cursor - offset),
            offset: 0,
        }
    }

    /// Moves the cursor, keeping it inside `[0, len)` and the window around it; false when nothing moved.
    pub fn move_by(&mut self, delta: isize, len: usize, visible: usize) -> bool {
        if len == 0 {
            return false;
        }
        let next = (self.cursor as isize + delta).clamp(0, len as isize - 1) as usize;
        if next == self.cursor {
            return false;
        }
        self.cursor = next;
        self.offset = self.window(visible.max(1));
        true
    }

    /// After an edit: the reset row for an empty input, else the first match, else the typed row.
    pub fn retarget(&mut self, snapshot: &Snapshot) {
        self.follow_input(snapshot);
        self.error = None;
    }

    fn follow_input(&mut self, snapshot: &Snapshot) {
        let rows = self.rows(snapshot);
        self.cursor = if self.input.is_empty() {
            0
        } else {
            rows.iter()
                .position(|r| matches!(r, PickerRow::Ref { .. }))
                .or_else(|| rows.iter().position(|r| matches!(r, PickerRow::Typed(_))))
                .unwrap_or(0)
        };
        self.offset = 0;
    }

    /// The engine's answers: a new ref list re-places the cursor; the pending `SetBase` closes or errs.
    pub fn observe(&mut self, snapshot: &Snapshot) {
        if snapshot.refs_seq == self.token && self.seen_refs_seq != self.token {
            self.seen_refs_seq = snapshot.refs_seq;
            self.follow_input(snapshot);
        }
        if let Some(sent) = self.pending {
            if snapshot.pick_seq > sent {
                self.pending = None;
                match &snapshot.pick_error {
                    Some(error) => self.error = Some(error.clone()),
                    None => self.done = true,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Base, BaseSource, RepoState, Scope, Snapshot};
    use std::sync::Arc;

    /// `refs_seq` is 1: a picker opened with `Picker::open(1)` sees the list at once.
    fn snap(refs: &[&str], base: Option<(&str, BaseSource)>) -> Snapshot {
        let mut s = Snapshot::empty("/r");
        s.revision = 1;
        s.refs_seq = 1;
        s.repo = RepoState::Repo {
            toplevel: "/r".into(),
            branch: Some("main".into()),
            worktree: None,
        };
        s.refs = Some(Arc::new(refs.iter().map(|r| r.to_string()).collect()));
        s.default_base = Some("refs/heads/main".into());
        s.base = base.map(|(requested, source)| Base {
            requested: requested.into(),
            commit: "0".repeat(40),
            merge_base: None,
            source,
        });
        s.scope = Scope::Worktree;
        s
    }

    const REFS: [&str; 5] = [
        "refs/heads/feat",
        "refs/heads/main",
        "refs/remotes/origin/main",
        "refs/tags/main",
        "refs/heads/maint-2.1",
    ];

    fn labels(rows: &[PickerRow]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                PickerRow::Reset(t) => t.clone(),
                PickerRow::Typed(t) => format!("use \"{t}\""),
                PickerRow::Ref { qualified, markers } => {
                    let mut s = crate::engine::ref_label(qualified).to_string();
                    if !markers.is_empty() {
                        s = format!("{s} [{}]", markers.join(" · "));
                    }
                    s
                }
            })
            .collect()
    }

    #[test]
    fn rows_start_with_reset_then_the_current_base_then_matches_with_markers() {
        let s = snap(&REFS, Some(("refs/tags/main", BaseSource::Picked)));
        let p = Picker::open(1);
        assert_eq!(
            labels(&p.rows(&s)),
            [
                "default (main)",
                "main [picked · tag]",
                "feat",
                "main [current]",
                "origin/main",
                "maint-2.1"
            ]
        );
        let mut p = Picker::open(1);
        p.input = "main".into();
        assert_eq!(
            labels(&p.rows(&s)),
            [
                "default (main)",
                "main [picked · tag]",
                "main [current]",
                "origin/main",
                "maint-2.1"
            ],
            "`main` is exactly a listed label, so no typed row"
        );
        p.input = "MAIN".into();
        assert_eq!(
            labels(&p.rows(&s)),
            [
                "default (main)",
                "use \"MAIN\"",
                "main [picked · tag]",
                "main [current]",
                "origin/main",
                "maint-2.1"
            ],
            "filtering is case-insensitive; the exact-label check is not"
        );
        p.input = "HEAD~2".into();
        assert_eq!(labels(&p.rows(&s)), ["default (main)", "use \"HEAD~2\""]);
        assert_eq!(p.rows(&s)[1].submit().as_deref(), Some("HEAD~2"));
        assert_eq!(p.rows(&s)[0].submit(), None);
        let s = snap(&REFS, Some(("feat", BaseSource::Config)));
        assert_eq!(labels(&Picker::open(1).rows(&s))[1], "feat");
        let s = snap(&REFS, Some(("refs/heads/feat", BaseSource::Config)));
        assert_eq!(labels(&Picker::open(1).rows(&s))[1], "feat [config]");
        let mut s = snap(&[], None);
        s.default_base = None;
        assert_eq!(labels(&Picker::open(1).rows(&s)), ["default (none)"]);
    }

    #[test]
    fn the_cursor_follows_the_input_and_moves_within_bounds() {
        let s = snap(&REFS, None);
        let mut p = Picker::open(1);
        assert_eq!(p.cursor, 0);
        p.input = "ma".into();
        p.retarget(&s);
        assert_eq!(
            p.cursor, 2,
            "the first match, after the reset and typed rows"
        );
        p.input = "zzz".into();
        p.retarget(&s);
        assert_eq!(p.cursor, 1, "the typed row");
        p.input.clear();
        p.retarget(&s);
        assert_eq!(p.cursor, 0);
        let len = p.rows(&s).len();
        assert!(!p.move_by(-1, len, 3));
        assert!(p.move_by(1, len, 3) && p.cursor == 1);
        assert!(p.move_by(10, len, 3) && p.cursor == len - 1);
        assert_eq!(p.offset, len - 3, "the window follows the cursor down");
        assert!(p.move_by(-(len as isize), len, 3) && p.cursor == 0 && p.offset == 0);
    }

    #[test]
    fn the_panel_shows_the_caret_error_markers_and_footers() {
        let s = snap(&REFS, Some(("refs/heads/main", BaseSource::Default)));
        let mut p = Picker::open(1);
        p.input = "ma".into();
        p.retarget(&s);
        let panel = p.panel(&s, WIDTH, 12);
        assert_eq!(panel.title, "Compare against");
        assert_eq!(panel.footer, "Enter pick · Esc cancel · type to filter");
        let lines: Vec<String> = crate::tui::dialog::render(&panel, WIDTH, 12)
            .iter()
            .map(|l| l.iter().map(|s| s.text.as_str()).collect::<String>())
            .collect();
        assert!(lines[1].contains("> ma_"), "{}", lines[1]);
        assert!(lines[2].contains("default (main)"), "{}", lines[2]);
        assert!(lines[3].contains("use \"ma\""), "{}", lines[3]);
        assert!(
            lines[4].contains("▸ main") && lines[4].contains("current"),
            "{}",
            lines[4]
        );
        p.error = Some("not a commit: ma\u{1b}".into());
        let panel = p.panel(&s, WIDTH, 12);
        assert!(
            matches!(&panel.rows[1], crate::tui::dialog::Row::Warn(w) if w == "not a commit: ma\u{241b}")
        );
        let mut s = snap(&REFS, None);
        s.refs_overflow = true;
        let mut p = Picker::open(1);
        p.input = "ma".into();
        assert_eq!(
            p.panel(&s, WIDTH, 12).footer,
            "4 shown, more exist · type to filter"
        );
        let s = snap(&[], None);
        assert_eq!(
            Picker::open(1).panel(&s, WIDTH, 12).footer,
            "no refs listed; type a revision"
        );
        let mut s = snap(&REFS, None);
        s.refs = None;
        assert_eq!(
            Picker::open(1).panel(&s, WIDTH, 12).footer,
            "no refs listed; type a revision"
        );
        let mut p = Picker::open(1);
        p.pending = Some(0);
        assert_eq!(p.panel(&snap(&REFS, None), WIDTH, 12).footer, "picking…");
    }

    #[test]
    fn every_panel_row_is_one_line_at_any_width() {
        let s = snap(&REFS, None);
        let mut p = Picker::open(1);
        p.input = "a".repeat(70);
        p.error = Some("not a commit: ".to_string() + &"b".repeat(70));
        for width in [40u16, 48, 60] {
            let panel = p.panel(&s, width, 10);
            assert_eq!(
                crate::tui::dialog::line_count(&panel, width),
                panel.rows.len(),
                "width {width}"
            );
            assert_eq!(panel.rows.len(), 2 + p.visible(10).min(p.rows(&s).len()));
        }
    }

    #[test]
    fn the_list_counts_only_once_this_opening_is_answered_and_the_cursor_follows_it() {
        let mut s = snap(&REFS, None);
        s.refs_seq = 0;
        let mut p = Picker::open(2);
        assert!(
            p.refs(&s).is_empty(),
            "the previous opening's list is not shown"
        );
        assert_eq!(labels(&p.rows(&s)), ["default (main)"]);
        assert_eq!(p.panel(&s, WIDTH, 12).footer, "loading refs…");
        p.input = "ma".into();
        p.retarget(&s);
        assert_eq!(p.cursor, 1, "the typed row while the list is empty");
        p.error = Some("kept".into());
        s.refs_seq = 1;
        p.observe(&s);
        assert!(
            p.refs(&s).is_empty(),
            "an earlier opening's reply stays hidden"
        );
        assert_eq!(p.panel(&s, WIDTH, 12).footer, "loading refs…");
        assert_eq!(p.cursor, 1, "a stale reply must not move the cursor");
        s.refs_seq = 2;
        p.observe(&s);
        assert_eq!(p.refs(&s).len(), REFS.len());
        assert_eq!(p.cursor, 2, "the first match once the list arrived");
        assert_eq!(
            p.error.as_deref(),
            Some("kept"),
            "the matching reply keeps the error"
        );
        p.cursor = 3;
        p.observe(&s);
        assert_eq!(p.cursor, 3, "the same reply only places the cursor once");
        assert_eq!(
            p.error.as_deref(),
            Some("kept"),
            "a new list does not clear an error"
        );
    }

    #[test]
    fn display_text_is_sanitized_without_changing_submitted_revisions() {
        let mut s = snap(&["refs/heads/a\u{202e}"], None);
        s.default_base = Some("refs/heads/a\u{202e}".into());
        let mut p = Picker::open(1);
        for input in ["", "x\u{202e}\n"] {
            p.input = input.into();
            p.retarget(&s);
            let panel = p.panel(&s, WIDTH, 12);
            assert_eq!(
                crate::tui::dialog::line_count(&panel, WIDTH),
                panel.rows.len()
            );
            let text: String = crate::tui::dialog::render(&panel, WIDTH, 12)
                .iter()
                .flatten()
                .map(|span| span.text.as_str())
                .collect();
            assert!(!text.contains(['\u{202e}', '\n']), "{text:?}");
            assert!(text.contains("default (a\u{fffd})"));
            if !input.is_empty() {
                assert!(text.contains("> x\u{fffd}\u{240a}_"));
                assert!(text.contains("use \"x\u{fffd}\u{240a}\""));
                assert_eq!(p.rows(&s)[1].submit().as_deref(), Some(input));
            } else {
                assert_eq!(
                    p.rows(&s)[1].submit().as_deref(),
                    Some("refs/heads/a\u{202e}")
                );
            }
        }
    }

    #[test]
    fn the_reply_closes_the_picker_or_shows_the_error() {
        let mut s = snap(&REFS, None);
        let mut p = Picker::open(1);
        p.pending = Some(s.pick_seq);
        p.observe(&s);
        assert!(p.pending.is_some() && !p.done, "no answer yet");
        s.pick_seq += 1;
        s.pick_error = Some("not a commit: x".into());
        p.observe(&s);
        assert!(p.pending.is_none() && !p.done);
        assert_eq!(p.error.as_deref(), Some("not a commit: x"));
        p.pending = Some(s.pick_seq);
        s.pick_seq += 1;
        s.pick_error = None;
        p.observe(&s);
        assert!(p.done);
    }
}
