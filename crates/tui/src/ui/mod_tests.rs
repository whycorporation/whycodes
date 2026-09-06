use crate::app::TuiApp;
use crate::config::TuiAppConfig;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

#[test]
fn render_paints_default_home_shell() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| super::render(f, &mut app)).unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().to_string())
        .collect();
    assert!(!text.trim().is_empty(), "{text}");
}
