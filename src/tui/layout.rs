//! Scroll arithmetic after herdr-agent-watcher's layout.rs, with usize content offsets.
//! The original uses u16 and saturates at row 65,535, which a diff exceeds.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineSpan {
    pub start: usize,
    pub height: usize,
}

pub fn clamp_scroll(offset: usize, total_lines: usize, viewport_height: u16) -> usize {
    offset.min(total_lines.saturating_sub(usize::from(viewport_height)))
}

pub fn ensure_visible(offset: usize, span: LineSpan, viewport: u16, total_lines: usize) -> usize {
    let viewport = usize::from(viewport);
    let end = span.start + span.height;
    let next = if span.start < offset {
        span.start
    } else if end > offset + viewport {
        end.saturating_sub(viewport)
    } else {
        offset
    };
    clamp_scroll(next, total_lines, viewport as u16)
}

/// Keeps the row that was at `old_row` on the same viewport line now that it is `new_row`.
pub fn reanchor(
    offset: usize,
    old_row: usize,
    new_row: usize,
    viewport: u16,
    total_lines: usize,
) -> usize {
    let viewport_line = old_row.saturating_sub(offset);
    clamp_scroll(new_row.saturating_sub(viewport_line), total_lines, viewport)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_keeps_the_last_page_reachable_past_u16() {
        assert_eq!(clamp_scroll(150_000, 100_000, 40), 99_960);
        assert_eq!(clamp_scroll(70_000, 100_000, 40), 70_000);
        assert_eq!(clamp_scroll(5, 10, 40), 0);
    }

    #[test]
    fn ensure_visible_scrolls_the_minimum() {
        let span = |start| LineSpan { start, height: 1 };
        assert_eq!(ensure_visible(0, span(90_000), 40, 100_000), 89_961);
        assert_eq!(ensure_visible(89_961, span(89_970), 40, 100_000), 89_961);
        assert_eq!(ensure_visible(89_961, span(10), 40, 100_000), 10);
    }

    #[test]
    fn reanchor_keeps_the_viewport_row() {
        // the cursor sat on viewport row 7 (row 107 with offset 100); it is now row 250
        assert_eq!(reanchor(100, 107, 250, 40, 100_000), 243);
        assert_eq!(reanchor(100, 107, 3, 40, 100_000), 0);
    }
}
