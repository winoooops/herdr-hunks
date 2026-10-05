//! The pane picker of spec 10.2, built like the base picker: an input line that filters, three groups.
use crate::engine::{PaneRow, Snapshot, Target, TargetState};
use crate::tui::dialog::{self, Panel, Row};
use crate::tui::format::truncate;
use crate::tui::sanitize::sanitize;
use crate::tui::style::Semantic;

pub const WIDTH: u16 = 72;
pub const NO_AGENT: &str =
    "No agent pane in this session. Start claude, codex or kimi in a pane, then press A.";
pub const CLIPBOARD_ROW: &str = "✂ clipboard · copy the review instead of sending it";

/// Where the picker returns when a pick lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReturnTo {
    Nothing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)] // Choices carry the complete pane record.
pub enum Choice {
    Pane(PaneRow),
    Clipboard,
}

impl Choice {
    pub fn target(&self, socket_path: &str) -> Target {
        match self {
            Self::Clipboard => Target::Clipboard,
            Self::Pane(row) => Target::Pane {
                pane: row.record.pane_id.clone(),
                socket: socket_path.to_string(),
                agent: row.record.agent.clone().unwrap_or_default(),
                session: row.record.agent_session.clone(),
                title: row
                    .record
                    .label
                    .clone()
                    .or_else(|| row.record.title.clone())
                    .unwrap_or_default(),
            },
        }
    }
}

/// `~`-relative when under HOME, as the picker shows directories.
fn relative_to_home(dir: &str) -> String {
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            if let Ok(relative) = std::path::Path::new(dir).strip_prefix(home) {
                return if relative.as_os_str().is_empty() {
                    "~".into()
                } else {
                    format!("~/{}", relative.display())
                };
            }
        }
    }
    dir.to_string()
}

fn columns(row: &PaneRow) -> (String, String, String, String, String) {
    let record = &row.record;
    let dir = record
        .foreground_cwd
        .clone()
        .or_else(|| record.cwd.clone())
        .map(|d| relative_to_home(&d))
        .unwrap_or_default();
    (
        sanitize(record.agent.as_deref().unwrap_or("?")),
        sanitize(&record.pane_id),
        sanitize(record.agent_status.as_deref().unwrap_or("unknown")),
        sanitize(&dir),
        sanitize(
            record
                .label
                .as_deref()
                .or(record.title.as_deref())
                .unwrap_or(""),
        ),
    )
}

#[derive(Debug)]
pub struct PanePicker {
    pub input: String,
    pub cursor: usize,
    pub offset: usize,
    pub token: u64,
    /// The submitted pick token; only its echoed answer closes the picker.
    pub pending: Option<u64>,
    pub done: bool,
    pub return_to: ReturnTo,
    seen_panes_seq: u64,
}

impl PanePicker {
    pub fn open(token: u64, return_to: ReturnTo) -> Self {
        Self {
            input: String::new(),
            cursor: 0,
            offset: 0,
            token,
            pending: None,
            done: false,
            return_to,
            seen_panes_seq: 0,
        }
    }

    fn loaded(&self, snapshot: &Snapshot) -> bool {
        snapshot.panes_seq == self.token
    }

    fn matches(&self, row: &PaneRow) -> bool {
        if self.input.is_empty() {
            return true;
        }
        let needle = self.input.to_lowercase();
        let (agent, id, status, dir, title) = columns(row);
        [agent, id, status, dir, title]
            .iter()
            .any(|column| column.to_lowercase().contains(&needle))
    }

    /// This worktree's panes, then the others, then the clipboard, which no filter removes.
    pub fn choices(&self, snapshot: &Snapshot) -> Vec<Choice> {
        let mut choices = Vec::new();
        if self.loaded(snapshot) {
            if let Some(panes) = &snapshot.panes {
                choices.extend(
                    panes
                        .iter()
                        .filter(|r| r.this_worktree && self.matches(r))
                        .cloned()
                        .map(Choice::Pane),
                );
                choices.extend(
                    panes
                        .iter()
                        .filter(|r| !r.this_worktree && self.matches(r))
                        .cloned()
                        .map(Choice::Pane),
                );
            }
        }
        choices.push(Choice::Clipboard);
        choices
    }

    pub fn visible(&self, height: u16) -> usize {
        // The frame, the input line, two headings and the footer leave this many rows for entries.
        usize::from(height.saturating_sub(8)).max(1)
    }

    pub(super) fn window(&self, visible: usize) -> usize {
        self.offset
            .min(self.cursor)
            .max(self.cursor.saturating_sub(visible - 1))
    }

    /// The panel's rows: the input line, the error or the no-agent note, the groups with their
    /// headings, the clipboard. Returns the rows and, for each drawn choice, its row index.
    fn rows_and_positions(
        &self,
        snapshot: &Snapshot,
        width: u16,
        height: u16,
    ) -> (Vec<Row>, Vec<usize>) {
        let line = usize::from(width).saturating_sub(4).max(1);
        let mut rows = vec![Row::Text(truncate(
            &sanitize(&format!("> {}_", self.input)),
            line,
        ))];
        let choices = self.choices(snapshot);
        if self.loaded(snapshot) {
            if let Some(error) = &snapshot.panes_error {
                rows.push(Row::Warn(sanitize(error)));
            } else if snapshot.panes.as_ref().is_none_or(|panes| panes.is_empty()) {
                rows.push(Row::Note(NO_AGENT.to_string()));
            }
        }
        let visible = self.visible(height);
        let first = self.window(visible);
        let mut positions = vec![usize::MAX; choices.len()];
        let mut last_group: Option<bool> = None;
        for (index, choice) in choices.iter().enumerate().skip(first).take(visible) {
            if let Choice::Pane(row) = choice {
                if last_group != Some(row.this_worktree) {
                    last_group = Some(row.this_worktree);
                    rows.push(Row::Text(
                        if row.this_worktree {
                            "this worktree"
                        } else {
                            "other panes"
                        }
                        .to_string(),
                    ));
                }
            }
            positions[index] = rows.len();
            rows.push(match choice {
                Choice::Clipboard => Row::Entry {
                    label: CLIPBOARD_ROW.to_string(),
                    value: String::new(),
                    enabled: true,
                },
                Choice::Pane(row) => {
                    let (agent, id, status, dir, title) = columns(row);
                    Row::Entry {
                        label: truncate(
                            &format!("{agent}  {id}  {status}  {dir}"),
                            line.saturating_sub(20),
                        ),
                        value: truncate(&title, 18),
                        enabled: true,
                    }
                }
            });
        }
        (rows, positions)
    }

    /// The terminal line (within the panel's rows, frame excluded) where `choices[choice]` is drawn: the
    /// rendered height of every row before it, because a `Note` or `Warn` wraps to several lines at the
    /// panel's width while an `Entry` is one.
    pub fn panel_line(
        &self,
        snapshot: &Snapshot,
        width: u16,
        height: u16,
        choice: usize,
    ) -> Option<usize> {
        let (rows, positions) = self.rows_and_positions(snapshot, width, height);
        let row = positions
            .get(choice)
            .copied()
            .filter(|p| *p != usize::MAX)?;
        let before = Panel {
            title: String::new(),
            rows: rows[..row].to_vec(),
            footer: String::new(),
            cursor: None,
            offset: 0,
        };
        let line = dialog::line_count(&before, width);
        (line < usize::from(height.saturating_sub(4))).then_some(line)
    }

    pub fn panel(&self, snapshot: &Snapshot, width: u16, height: u16) -> Panel {
        let (rows, positions) = self.rows_and_positions(snapshot, width, height);
        let footer = if self.pending.is_some() {
            "picking…".to_string()
        } else if !self.loaded(snapshot) {
            "loading panes…".to_string()
        } else {
            "Enter pick · Esc cancel · type to filter".to_string()
        };
        Panel {
            title: "Send to".into(),
            rows,
            footer,
            cursor: positions
                .get(self.cursor)
                .copied()
                .filter(|p| *p != usize::MAX),
            offset: 0,
        }
    }

    pub fn move_by(&mut self, delta: isize, choices: &[Choice], visible: usize) -> bool {
        let len = choices.len();
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

    /// After an edit: the first match, the clipboard when nothing matches.
    pub fn retarget(&mut self, snapshot: &Snapshot) {
        let choices = self.choices(snapshot);
        self.cursor = self.cursor.min(choices.len().saturating_sub(1));
        if !self.input.is_empty() {
            self.cursor = 0;
        }
        self.offset = 0;
    }

    /// A new row list re-places the cursor; the answered pick closes the picker.
    pub fn observe(&mut self, snapshot: &Snapshot) {
        if self.loaded(snapshot) && self.seen_panes_seq != snapshot.panes_seq {
            self.seen_panes_seq = snapshot.panes_seq;
            self.retarget(snapshot);
        }
        // Only the answer to this pick echoes its token.
        if let Some(token) = self.pending {
            if snapshot.target_token == Some(token) {
                self.pending = None;
                self.done = true;
            }
        }
    }
}

/// The chip of spec 10.2: `(text, tone, dim)`. Host absence outranks target absence.
pub fn chip(snapshot: &Snapshot) -> (String, Option<Semantic>, bool) {
    let state = &snapshot.target_state;
    match (&snapshot.target, state) {
        (_, TargetState::NoHost) => ("→ no host".to_string(), None, true),
        (None, _) => ("→ no agent".to_string(), None, true),
        (Some(Target::Clipboard), _) => ("→ clipboard".to_string(), None, false),
        (Some(Target::Pane { pane, agent, .. }), state) => {
            let pane = truncate(&sanitize(pane), 12);
            let agent = truncate(&sanitize(agent), 12);
            match state {
                TargetState::Live(status) if status == "idle" || status == "done" => {
                    (format!("→ {agent} {pane}"), Some(Semantic::Accent), false)
                }
                TargetState::Live(status) if status == "unknown" => {
                    (format!("→ {agent} {pane} · unknown"), None, true)
                }
                TargetState::Live(status) => (
                    format!("→ {agent} {pane} · {}", truncate(&sanitize(status), 8)),
                    Some(Semantic::Warn),
                    false,
                ),
                TargetState::Unverified => (format!("→ {agent} {pane} · ?"), None, true),
                TargetState::Restarted(_) => {
                    (format!("→ {pane} · restarted"), Some(Semantic::Warn), false)
                }
                TargetState::Left => (format!("→ {pane} · left"), Some(Semantic::Bad), false),
                TargetState::Gone => (format!("→ {pane} · gone"), Some(Semantic::Bad), false),
                TargetState::NoHost | TargetState::Clipboard => unreachable!("matched above"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::host::{PaneRecord, SessionRef};
    use crate::engine::{PaneRow, Snapshot, Target, TargetState};

    fn pane(id: &str, agent: &str, status: &str, cwd: &str, title: &str, this: bool) -> PaneRow {
        PaneRow {
            record: PaneRecord {
                pane_id: id.into(),
                agent: Some(agent.into()),
                agent_status: Some(status.into()),
                agent_session: Some(SessionRef {
                    kind: "id".into(),
                    value: "s".into(),
                }),
                cwd: Some(cwd.into()),
                title: Some(title.into()),
                ..PaneRecord::default()
            },
            this_worktree: this,
        }
    }

    fn snapshot_with(panes: Vec<PaneRow>, seq: u64) -> Snapshot {
        let mut s = Snapshot::empty("/r");
        s.panes = Some(std::sync::Arc::new(panes));
        s.panes_seq = seq;
        s
    }

    #[test]
    fn the_chip_reads_each_state_and_host_absence_outranks_target_absence() {
        let mut s = Snapshot::empty("/r");
        s.target_state = TargetState::NoHost;
        assert_eq!(chip(&s), ("→ no host".to_string(), None, true));
        s.target_state = TargetState::Unverified;
        assert_eq!(chip(&s), ("→ no agent".to_string(), None, true));
        s.target = Some(Target::Pane {
            pane: "w4:p2".into(),
            socket: "/s".into(),
            agent: "codex".into(),
            session: None,
            title: String::new(),
        });
        assert_eq!(chip(&s), ("→ codex w4:p2 · ?".to_string(), None, true));
        s.target_state = TargetState::Live("idle".into());
        assert_eq!(
            chip(&s),
            ("→ codex w4:p2".to_string(), Some(Semantic::Accent), false)
        );
        s.target_state = TargetState::Live("done".into());
        assert_eq!(chip(&s).0, "→ codex w4:p2");
        s.target_state = TargetState::Live("working".into());
        assert_eq!(
            chip(&s),
            (
                "→ codex w4:p2 · working".to_string(),
                Some(Semantic::Warn),
                false
            )
        );
        s.target_state = TargetState::Live("blocked".into());
        assert_eq!(
            chip(&s),
            (
                "→ codex w4:p2 · blocked".to_string(),
                Some(Semantic::Warn),
                false
            )
        );
        s.target_state = TargetState::Live("unknown".into());
        assert_eq!(
            chip(&s),
            ("→ codex w4:p2 · unknown".to_string(), None, true)
        );
        s.target_state = TargetState::Restarted("idle".into());
        assert_eq!(
            chip(&s),
            (
                "→ w4:p2 · restarted".to_string(),
                Some(Semantic::Warn),
                false
            )
        );
        s.target_state = TargetState::Left;
        assert_eq!(
            chip(&s),
            ("→ w4:p2 · left".to_string(), Some(Semantic::Bad), false)
        );
        s.target_state = TargetState::Gone;
        assert_eq!(
            chip(&s),
            ("→ w4:p2 · gone".to_string(), Some(Semantic::Bad), false)
        );
        s.target_state = TargetState::NoHost;
        assert_eq!(chip(&s), ("→ no host".to_string(), None, true));
        s.target = Some(Target::Clipboard);
        s.target_state = TargetState::Clipboard;
        assert_eq!(chip(&s), ("→ clipboard".to_string(), None, false));
        // Untrusted text: a title or kind with control characters is sanitised before the chip.
        s.target = Some(Target::Pane {
            pane: "w4:p2".into(),
            socket: "/s".into(),
            agent: "co\u{1b}dex".into(),
            session: None,
            title: String::new(),
        });
        s.target_state = TargetState::Live("idle".into());
        assert!(!chip(&s).0.contains('\u{1b}'));
    }

    #[test]
    fn choices_come_in_three_groups_and_the_filter_matches_every_column() {
        let panes = vec![
            pane(
                "w1:p3",
                "claude",
                "idle",
                "/home/u/repo/src",
                "fix the cart",
                true,
            ),
            pane("w1:p2", "codex", "working", "/home/u/other", "lint", false),
            pane("w2:p1", "kimi", "idle", "/home/u/repo", "", true),
        ];
        let picker = PanePicker::open(1, ReturnTo::Nothing);
        let s = snapshot_with(panes.clone(), 1);
        let choices = picker.choices(&s);
        assert_eq!(choices.len(), 4);
        assert!(matches!(&choices[0], Choice::Pane(r) if r.record.pane_id == "w1:p3"));
        assert!(matches!(&choices[1], Choice::Pane(r) if r.record.pane_id == "w2:p1"));
        assert!(matches!(&choices[2], Choice::Pane(r) if r.record.pane_id == "w1:p2"));
        assert_eq!(choices[3], Choice::Clipboard);
        for (input, expected) in [
            ("codex", "w1:p2"),
            ("p1", "w2:p1"),
            ("working", "w1:p2"),
            ("other", "w1:p2"),
            ("cart", "w1:p3"),
        ] {
            let mut picker = PanePicker::open(1, ReturnTo::Nothing);
            picker.input = input.into();
            let choices = picker.choices(&s);
            assert_eq!(choices.len(), 2, "{input}: {choices:?}");
            assert!(
                matches!(&choices[0], Choice::Pane(r) if r.record.pane_id == expected),
                "{input}"
            );
            assert_eq!(
                choices[1],
                Choice::Clipboard,
                "the clipboard row is never filtered out"
            );
        }
        // The panel: two headings, the rows, the clipboard; the cursor lands on an entry row.
        let panel = picker.panel(&s, WIDTH, 20);
        let plain: Vec<String> = panel
            .rows
            .iter()
            .map(|r| match r {
                Row::Entry { label, value, .. } => format!("E {label} | {value}"),
                Row::Text(t) => format!("T {t}"),
                Row::Note(t) => format!("N {t}"),
                Row::Warn(t) => format!("W {t}"),
                Row::Rule => "R".into(),
            })
            .collect();
        assert!(plain[0].starts_with("T > _"));
        assert_eq!(plain[1], "T this worktree");
        assert!(
            plain[2].starts_with("E claude  w1:p3  idle  ")
                && plain[2].contains("repo/src")
                && plain[2].ends_with("| fix the cart"),
            "{}",
            plain[2]
        );
        assert!(plain[3].starts_with("E kimi  w2:p1  idle  ") && plain[3].contains("repo"));
        assert_eq!(plain[4], "T other panes");
        assert!(plain[5].starts_with("E codex  w1:p2  working  ") && plain[5].contains("other"));
        assert_eq!(plain[6], format!("E {CLIPBOARD_ROW} | "));
        assert_eq!(panel.cursor, Some(2));
        assert_eq!(picker.panel_line(&s, WIDTH, 20, 3), Some(6));
        assert_eq!(panel.title, "Send to");
        assert_eq!(panel.footer, "Enter pick · Esc cancel · type to filter");
    }

    #[test]
    fn an_empty_host_offers_the_clipboard_alone_and_a_failure_names_its_reason() {
        let picker = PanePicker::open(1, ReturnTo::Nothing);
        let s = snapshot_with(Vec::new(), 1);
        let panel = picker.panel(&s, WIDTH, 20);
        assert!(panel
            .rows
            .iter()
            .any(|r| matches!(r, Row::Note(t) if t == NO_AGENT)));
        assert_eq!(picker.choices(&s), vec![Choice::Clipboard]);
        assert_eq!(panel.cursor, Some(2));
        // Wrapped notes move the clipboard hit with its rendered line.
        assert_eq!(picker.panel_line(&s, WIDTH, 20, 0), Some(3));
        assert_eq!(picker.panel_line(&s, 44, 20, 0), Some(4));
        assert_eq!(
            picker.panel_line(&s, 44, 20, 1),
            None,
            "a choice that does not exist has no line"
        );
        let mut s = s;
        s.panes_error = Some("could not list panes: deadline".into());
        let panel = picker.panel(&s, WIDTH, 20);
        assert!(panel
            .rows
            .iter()
            .any(|r| matches!(r, Row::Warn(t) if t == "could not list panes: deadline")));
        assert!(!panel
            .rows
            .iter()
            .any(|r| matches!(r, Row::Note(t) if t == NO_AGENT)));
        // Loading: the token has not been answered yet.
        let mut s = Snapshot::empty("/r");
        s.panes_seq = 0;
        let panel = picker.panel(&s, WIDTH, 20);
        assert_eq!(panel.footer, "loading panes…");
        assert_eq!(picker.choices(&s), vec![Choice::Clipboard]);
    }

    #[test]
    fn a_choice_becomes_the_target_it_names() {
        let row = pane("w1:p3", "claude", "idle", "/r", "fix", true);
        let target = Choice::Pane(row.clone()).target("/run/h.sock");
        assert_eq!(
            target,
            Target::Pane {
                pane: "w1:p3".into(),
                socket: "/run/h.sock".into(),
                agent: "claude".into(),
                session: row.record.agent_session.clone(),
                title: "fix".into()
            }
        );
        assert_eq!(Choice::Clipboard.target("/run/h.sock"), Target::Clipboard);
    }

    #[test]
    fn observe_closes_on_the_answered_pick_and_follows_the_rows() {
        let mut picker = PanePicker::open(3, ReturnTo::Nothing);
        let mut s = snapshot_with(vec![pane("w1:p3", "claude", "idle", "/r", "", true)], 2);
        picker.observe(&s);
        assert_eq!(picker.cursor, 0);
        s.panes_seq = 3;
        picker.observe(&s);
        picker.pending = Some(7);
        s.target_seq += 1;
        s.target_token = None; // a comment's write answered
        picker.observe(&s);
        assert!(!picker.done, "another write's answer is not this pick's");
        s.target_seq += 1;
        s.target_token = Some(3); // an older pick's, had it not been dropped
        picker.observe(&s);
        assert!(!picker.done);
        s.target_seq += 1;
        s.target_token = Some(7);
        picker.observe(&s);
        assert!(picker.done);
    }

    #[test]
    fn loading_hides_old_errors_and_filtering_does_not_claim_the_host_is_empty() {
        let mut s = snapshot_with(vec![pane("w1:p2", "codex", "idle", "/r", "", true)], 1);
        s.panes_error = Some("old failure".into());
        let mut picker = PanePicker::open(2, ReturnTo::Nothing);
        assert!(!picker
            .panel(&s, WIDTH, 20)
            .rows
            .iter()
            .any(|r| matches!(r, Row::Warn(_))));
        s.panes_seq = 2;
        s.panes_error = None;
        picker.input = "no match".into();
        assert_eq!(picker.choices(&s), vec![Choice::Clipboard]);
        assert!(!picker
            .panel(&s, WIDTH, 20)
            .rows
            .iter()
            .any(|r| matches!(r, Row::Note(t) if t == NO_AGENT)));
    }

    #[test]
    fn home_relative_directories_respect_path_components() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(relative_to_home(&home), "~");
        assert_eq!(relative_to_home(&format!("{home}/repo")), "~/repo");
        let sibling = format!("{home}-other/repo");
        assert_eq!(relative_to_home(&sibling), sibling);
    }

    #[test]
    fn a_choice_clipped_by_the_panel_has_no_hit_line() {
        let mut s = snapshot_with(Vec::new(), 1);
        s.panes_error = Some("deadline ".repeat(100));
        let picker = PanePicker::open(1, ReturnTo::Nothing);
        assert_eq!(picker.panel_line(&s, 44, 8, 0), None);
    }
}
