use super::*;
use crate::config::TuiAppConfig;
use crate::theme::ThemeName;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

#[test]
fn render_help_overlay_paints_shortcuts() {
    let palette = ThemeName::DefaultDark.palette();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render_help_overlay(f, &mut app, &palette))
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().to_string())
        .collect();
    assert!(
        text.contains("Keyboard") || text.contains("Shortcuts") || text.contains("/help"),
        "{text}"
    );
}
