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

    #[test]
    fn fill_blank_overwrites_glyphs() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::style::Style;
        use ratatui::widgets::Paragraph;

        let backend = TestBackend::new(8, 2);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                f.render_widget(
                    Paragraph::new("ABCDEFGH\nIJKLMNOP").style(Style::default()),
                    f.area(),
                );
                fill_blank(f, Rect::new(0, 0, 8, 2), Color::Black);
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        for y in 0..2u16 {
            for x in 0..8u16 {
                assert_eq!(
                    buf[(x, y)].symbol(),
                    " ",
                    "stain leftover at ({x},{y}): {:?}",
                    buf[(x, y)].symbol()
                );
            }
        }
    }
}
