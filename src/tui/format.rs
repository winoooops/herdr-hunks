use unicode_width::UnicodeWidthStr;

pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Left-aligned padding by DISPLAY CELLS. `format!("{:<11}")` counts chars, so a
/// CJK tool name pads as if it were half its real width and overflows the pane
/// (§2.7). Truncates first, so the result is always exactly `cells` wide.
pub fn pad(s: &str, cells: usize) -> String {
    let mut out = truncate(s, cells);
    out.push_str(&" ".repeat(cells.saturating_sub(width(&out))));
    out
}

/// Truncates by display cells, always ending in `…` (§2.6). A budget below 2
/// yields an empty string: the caller omits the field entirely (§2.7).
pub fn truncate(s: &str, max_cells: usize) -> String {
    if width(s) <= max_cells {
        return s.to_string();
    }
    if max_cells < 2 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in s.chars() {
        let w = UnicodeWidthStr::width(ch.to_string().as_str());
        if used + w > max_cells - 1 {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

/// `…` plus the tail of `text` that fits `max` cells; the whole text when it already fits.
pub fn truncate_left(text: &str, max: usize) -> String {
    if width(text) <= max {
        return text.to_string();
    }
    let mut tail = String::new();
    let mut used = 1;
    for ch in text.chars().rev() {
        let w = width(&ch.to_string());
        if used + w > max {
            break;
        }
        used += w;
        tail.insert(0, ch);
    }
    format!("…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_left_keeps_the_tail_behind_an_ellipsis() {
        assert_eq!(truncate_left("src/values.ts", 8), "…lues.ts");
        assert_eq!(truncate_left("ab", 8), "ab");
        assert_eq!(width(&truncate_left("猫猫猫猫", 5)), 5);
    }

    #[test]
    fn truncate_measures_display_cells_and_ends_in_ellipsis() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 8), "hello w…");
        assert_eq!(truncate("猫猫猫", 4), "猫…");
        assert_eq!(width("猫"), 2);
        assert_eq!(width("a"), 1);
    }

    #[test]
    fn every_glyph_in_the_vocabulary_is_one_cell() {
        for g in [
            "◐", "!", "○", "✕", "✓", "●", "▸", "▾", "›", "…", "█", "▓", "▒", "░", "─", "↵", "·",
        ] {
            assert_eq!(width(g), 1, "glyph {g} must occupy exactly one cell");
        }
    }
}
