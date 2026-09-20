//! Portable styled output. No ratatui, no terminal: the view decides what a
//! span *means* and the shell maps that to colours (§2.4a, §2.5).

/// Structural weight. Resolution is theme-dependent (§2.5): under `inherit`
/// `Body`/`Emphasis` are the terminal's default foreground (`Emphasis` adds
/// bold, not a colour); under `lumon` they are that theme's explicit RGB.
/// `Label` and `Rule` are dim and reserved for decoration that may vanish.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Role {
    #[default]
    Body,
    Emphasis,
    Label,
    Rule,
}

/// Advisory colour applied over a role. A terminal cell has one foreground, so
/// this replaces the role's colour rather than layering over it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Semantic {
    Good,
    Warn,
    Bad,
    Accent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub role: Role,
    pub semantic: Option<Semantic>,
    /// Agent-mark override only (§2.5); `rgb` when the terminal reports
    /// truecolor, `ansi` (0–15) otherwise. Both are carried so the view stays
    /// free of capability detection.
    pub rgb: Option<(u8, u8, u8)>,
    pub ansi: Option<u8>,
    /// Reverse video, used for the selected card header and selected trace row.
    pub reverse: bool,
}

impl Style {
    pub fn role(role: Role) -> Self {
        Self {
            role,
            ..Self::default()
        }
    }

    pub fn semantic(role: Role, semantic: Semantic) -> Self {
        Self {
            role,
            semantic: Some(semantic),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

impl Span {
    pub fn new(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }

    pub fn body(text: impl Into<String>) -> Self {
        Self::new(text, Style::role(Role::Body))
    }

    pub fn label(text: impl Into<String>) -> Self {
        Self::new(text, Style::role(Role::Label))
    }

    pub fn emphasis(text: impl Into<String>) -> Self {
        Self::new(text, Style::role(Role::Emphasis))
    }
}

pub type Line = Vec<Span>;
