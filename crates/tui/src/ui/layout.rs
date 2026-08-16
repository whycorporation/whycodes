// ── ui/layout.rs: Layout helpers ───────────────────────────────────────

use ratatui::layout::Rect;

/// Calculate the number of lines that fit in a rect.
pub fn lines_in_rect(rect: Rect) -> usize {
    rect.height as usize
}

/// Calculate max visible scroll offset.
pub fn max_scroll(item_count: usize, visible_lines: usize) -> usize {
    item_count.saturating_sub(visible_lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_in_rect_matches_height() {
        assert_eq!(lines_in_rect(Rect::new(0, 0, 80, 24)), 24);
        assert_eq!(lines_in_rect(Rect::new(0, 0, 80, 1)), 1);
        assert_eq!(lines_in_rect(Rect::default()), 0);
    }

    #[test]
    fn max_scroll_clamps_at_zero() {
        // More items than visible lines → scrollable range.
        assert_eq!(max_scroll(100, 24), 76);
        // Exactly fitting → no scroll.
        assert_eq!(max_scroll(24, 24), 0);
        // Fewer items than lines → no scroll (saturating, no underflow).
        assert_eq!(max_scroll(10, 24), 0);
        assert_eq!(max_scroll(0, 24), 0);
    }
}
