// ── ui/layout.rs: Layout helpers ───────────────────────────────────────

use ratatui::layout::{Constraint, Rect};

/// Calculate the number of lines that fit in a rect.
pub fn lines_in_rect(rect: Rect) -> usize {
    rect.height as usize
}

/// Calculate max visible scroll offset.
pub fn max_scroll(item_count: usize, visible_lines: usize) -> usize {
    item_count.saturating_sub(visible_lines)
}
