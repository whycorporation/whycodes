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
