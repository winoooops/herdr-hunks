//! One binding table for input routing and the key sheet.
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::tui::dialog::{Panel, Row};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    LineDown,
    LineUp,
    HalfPageDown,
    HalfPageUp,
    HunkPrev,
    HunkNext,
    FileNext,
    FilePrev,
    SideDeletions,
    SideAdditions,
    ToggleView,
    ToggleFiles,
    PinFiles,
    Refresh,
    ToggleScope,
    First,
    Last,
    ScrollLeft,
    ScrollRight,
    ToggleMouse,
    Help,
    Quit,
}

pub struct Binding {
    pub key: &'static str,
    pub label: &'static str,
    pub action: KeyAction,
    pub vimeflow: Option<&'static str>,
}

pub const RESERVED: &[&str] = &[
    "s", "d", "D", "i", "I", "u", "U", "x", "v", "y", "Y", "@", "c", "/",
];

pub const KEYS: &[Binding] = &[
    Binding {
        key: "j",
        label: "next row",
        action: KeyAction::LineDown,
        vimeflow: Some("diff-line-next"),
    },
    Binding {
        key: "k",
        label: "previous row",
        action: KeyAction::LineUp,
        vimeflow: Some("diff-line-previous"),
    },
    Binding {
        key: "ctrl+d",
        label: "half page down",
        action: KeyAction::HalfPageDown,
        vimeflow: Some("diff-scroll-page-down"),
    },
    Binding {
        key: "ctrl+u",
        label: "half page up",
        action: KeyAction::HalfPageUp,
        vimeflow: Some("diff-scroll-page-up"),
    },
    Binding {
        key: "[",
        label: "previous hunk",
        action: KeyAction::HunkPrev,
        vimeflow: Some("diff-hunk-previous"),
    },
    Binding {
        key: "]",
        label: "next hunk",
        action: KeyAction::HunkNext,
        vimeflow: Some("diff-hunk-next"),
    },
    Binding {
        key: "n",
        label: "next file",
        action: KeyAction::FileNext,
        vimeflow: Some("diff-file-next"),
    },
    Binding {
        key: "p",
        label: "previous file",
        action: KeyAction::FilePrev,
        vimeflow: Some("diff-file-previous"),
    },
    Binding {
        key: "h",
        label: "deletions side",
        action: KeyAction::SideDeletions,
        vimeflow: Some("diff-side-deletions"),
    },
    Binding {
        key: "l",
        label: "additions side",
        action: KeyAction::SideAdditions,
        vimeflow: Some("diff-side-additions"),
    },
    Binding {
        key: "t",
        label: "unified / split",
        action: KeyAction::ToggleView,
        vimeflow: Some("diff-view-toggle"),
    },
    Binding {
        key: "e",
        label: "toggle files panel",
        action: KeyAction::ToggleFiles,
        vimeflow: Some("diff-files-toggle"),
    },
    Binding {
        key: "E",
        label: "pin files panel",
        action: KeyAction::PinFiles,
        vimeflow: Some("diff-files-pin"),
    },
    Binding {
        key: "r",
        label: "refresh",
        action: KeyAction::Refresh,
        vimeflow: Some("diff-refresh"),
    },
    Binding {
        key: "b",
        label: "switch scope",
        action: KeyAction::ToggleScope,
        vimeflow: None,
    },
    Binding {
        key: "g",
        label: "first row",
        action: KeyAction::First,
        vimeflow: None,
    },
    Binding {
        key: "G",
        label: "last row",
        action: KeyAction::Last,
        vimeflow: None,
    },
    Binding {
        key: "H",
        label: "scroll left",
        action: KeyAction::ScrollLeft,
        vimeflow: None,
    },
    Binding {
        key: "L",
        label: "scroll right",
        action: KeyAction::ScrollRight,
        vimeflow: None,
    },
    Binding {
        key: "m",
        label: "toggle mouse capture",
        action: KeyAction::ToggleMouse,
        vimeflow: None,
    },
    Binding {
        key: "?",
        label: "this key sheet",
        action: KeyAction::Help,
        vimeflow: None,
    },
    Binding {
        key: "q",
        label: "quit",
        action: KeyAction::Quit,
        vimeflow: None,
    },
];

pub fn lookup(key: &KeyEvent) -> Option<KeyAction> {
    if key.kind == KeyEventKind::Release
        || !(key.modifiers - KeyModifiers::SHIFT - KeyModifiers::CONTROL).is_empty()
    {
        return None;
    }
    let KeyCode::Char(ch) = key.code else {
        return if key.modifiers.is_empty() {
            match key.code {
                KeyCode::Down => Some(KeyAction::LineDown),
                KeyCode::Up => Some(KeyAction::LineUp),
                KeyCode::Left => Some(KeyAction::SideDeletions),
                KeyCode::Right => Some(KeyAction::SideAdditions),
                KeyCode::PageDown => Some(KeyAction::HalfPageDown),
                KeyCode::PageUp => Some(KeyAction::HalfPageUp),
                KeyCode::Home => Some(KeyAction::First),
                KeyCode::End => Some(KeyAction::Last),
                _ => None,
            }
        } else {
            None
        };
    };
    let name = if key.modifiers.contains(KeyModifiers::CONTROL) {
        format!("ctrl+{ch}")
    } else {
        ch.to_string()
    };
    KEYS.iter()
        .find(|binding| binding.key == name)
        .map(|binding| binding.action)
}

pub fn help_panel(popup: bool) -> Panel {
    let mut panel = Panel {
        title: "Keys".into(),
        rows: KEYS
            .iter()
            .map(|binding| Row::Entry {
                label: binding.key.into(),
                value: binding.label.into(),
                enabled: false,
            })
            .collect(),
        footer: "esc closes".into(),
        cursor: None,
        offset: 0,
    };
    if popup {
        panel.rows.push(Row::Entry {
            label: "esc".into(),
            value: "close".into(),
            enabled: false,
        });
    }
    panel
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_aliases_require_no_modifiers_and_do_not_extend_the_sheet() {
        for (code, action) in [
            (KeyCode::Down, KeyAction::LineDown),
            (KeyCode::Up, KeyAction::LineUp),
            (KeyCode::Left, KeyAction::SideDeletions),
            (KeyCode::Right, KeyAction::SideAdditions),
            (KeyCode::PageDown, KeyAction::HalfPageDown),
            (KeyCode::PageUp, KeyAction::HalfPageUp),
            (KeyCode::Home, KeyAction::First),
            (KeyCode::End, KeyAction::Last),
        ] {
            assert_eq!(
                lookup(&KeyEvent::new(code, KeyModifiers::NONE)),
                Some(action)
            );
            for modifiers in [
                KeyModifiers::SHIFT,
                KeyModifiers::CONTROL,
                KeyModifiers::ALT,
                KeyModifiers::SUPER,
            ] {
                assert_eq!(lookup(&KeyEvent::new(code, modifiers)), None);
            }
        }
        assert_eq!(KEYS.len(), 22);
        assert_eq!(help_panel(false).rows.len(), 22);
    }

    #[test]
    fn modifiers_preserve_uppercase_and_keep_unbound_combinations_inert() {
        assert_eq!(
            lookup(&KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE)),
            Some(KeyAction::ToggleScope)
        );
        assert_eq!(
            lookup(&KeyEvent::new(KeyCode::Char('E'), KeyModifiers::SHIFT)),
            Some(KeyAction::PinFiles)
        );
        assert_eq!(
            lookup(&KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE)),
            Some(KeyAction::ToggleFiles)
        );
        assert_eq!(
            lookup(&KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)),
            Some(KeyAction::HalfPageDown)
        );
        assert_eq!(
            lookup(&KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE)),
            None
        );
        for modifiers in [
            KeyModifiers::ALT,
            KeyModifiers::SUPER,
            KeyModifiers::HYPER,
            KeyModifiers::META,
            KeyModifiers::CONTROL,
        ] {
            assert_eq!(lookup(&KeyEvent::new(KeyCode::Char('q'), modifiers)), None);
        }
        assert_eq!(
            lookup(&KeyEvent::new_with_kind(
                KeyCode::Char('q'),
                KeyModifiers::NONE,
                KeyEventKind::Release
            )),
            None
        );
        assert_eq!(
            lookup(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            None
        );
    }
}
