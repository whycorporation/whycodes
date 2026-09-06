use super::*;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

#[test]
fn tick_advances_progress_status_and_completes() {
    let mut s = StartupScreen::new(100);
    assert_eq!(s.spinner(), SPINNER_FRAMES[0]);
    assert!(!s.tick(10));
    assert!(s.progress > 0.0);
    assert!(s.status.contains("Loading configuration"));
    assert!(!s.tick(40));
    assert!(s.status.contains("Connecting") || s.status.contains("Preparing"));
    assert!(s.tick(100));
    assert!(s.done);
    assert_eq!(s.progress, 1.0);
    assert_eq!(s.status, "Ready!");
    assert_eq!(s.spinner(), SPINNER_FRAMES[s.spinner_frame]);
}

#[test]
fn render_paints_branding_and_status() {
    let mut s = StartupScreen::new(1000);
    s.provider_info = "anthropic".into();
    s.project_path = "/work/proj".into();
    s.tick(200);
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render(f, Rect::new(0, 0, 80, 24), &s))
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().to_string())
        .collect();
    assert!(text.contains("whycodes"), "{text}");
    assert!(text.contains("anthropic"), "{text}");
    assert!(text.contains("/work/proj"), "{text}");
}
