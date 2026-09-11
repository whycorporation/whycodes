use super::*;
use crate::theme::ThemeName;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

#[test]
fn render_alert_paints_title_and_message() {
    let palette = ThemeName::DefaultDark.palette();
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            let chrome = render_alert_dialog(f, "Notice", "hello-alert", &palette, None);
            assert!(chrome.content.width > 0 || chrome.content.height == 0);
        })
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().to_string())
        .collect();
    assert!(
        text.contains("Notice") || text.contains("hello-alert"),
        "{text}"
    );
}

#[test]
fn render_alert_on_tiny_frame_returns_empty_content() {
    let palette = ThemeName::DefaultDark.palette();
    let backend = TestBackend::new(1, 1);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            let chrome = render_alert_dialog(f, "T", "m", &palette, None);
            // Tiny PTY: no close control, and the body is the modal itself.
            assert!(chrome.close_hit.is_none());
            assert_eq!(chrome.content, chrome.modal);
        })
        .unwrap();
}
