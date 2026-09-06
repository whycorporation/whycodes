use super::*;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

#[test]
fn render_skips_tiny_area_and_paints_shortcuts() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            render(f, Rect::new(0, 0, 10, 4), KeymapContext::Normal, 0);
        })
        .unwrap();

    terminal
        .draw(|f| {
            render(f, Rect::new(0, 0, 80, 24), KeymapContext::Normal, 0);
        })
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().to_string())
        .collect();
    assert!(text.contains("Shortcuts") || text.contains("KEY"), "{text}");
    assert!(text.contains("Esc") || text.contains("close"), "{text}");
}

#[test]
fn render_labels_each_keymap_context() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    for ctx in [
        KeymapContext::Dialog,
        KeymapContext::Command,
        KeymapContext::Help,
        KeymapContext::Session,
    ] {
        terminal
            .draw(|f| render(f, Rect::new(0, 0, 80, 24), ctx, 0))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol().to_string())
            .collect();
        assert!(
            text.contains("Shortcuts") || text.contains("KEY"),
            "{ctx:?}: {text}"
        );
    }
}
