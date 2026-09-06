// ── ui/layout.rs: Layout helpers ───────────────────────────────────────

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

/// Calculate the number of lines that fit in a rect.
pub fn lines_in_rect(rect: Rect) -> usize {
    rect.height as usize
}

/// Write spaces + `bg` across `area` so leftover glyphs cannot sit in
/// unpainted gaps (bracketed-paste echo, shrinking prompt).
///
/// `Block::style(bg)` only sets the cell style — it does not replace the
/// symbol — so a previous widget (or a stain in tests) would otherwise show
/// through breathing-room rows.
pub fn fill_blank(frame: &mut Frame, area: Rect, bg: Color) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let buf = frame.buffer_mut();
    let style = Style::default().fg(bg).bg(bg);
    let x_end = area.x.saturating_add(area.width);
    let y_end = area.y.saturating_add(area.height);
    for y in area.y..y_end {
        for x in area.x..x_end {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_symbol(" ");
                cell.set_style(style);
            }
        }
    }
}

/// Calculate max visible scroll offset.
pub fn max_scroll(item_count: usize, visible_lines: usize) -> usize {
    item_count.saturating_sub(visible_lines)
}

#[cfg(test)]
#[path = "layout_tests.rs"]
mod tests;
