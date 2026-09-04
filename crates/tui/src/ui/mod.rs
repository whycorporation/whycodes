// ── ui/mod.rs: UI module root ──────────────────────────────────────────
// Declares sub-modules and re-exports the render entry point.

pub mod chat;
pub mod dialogs;
pub mod file_suggest;
pub mod header;
pub mod layout;
pub mod markdown;
pub mod progress_bar;
pub mod prompt;
pub mod render;
pub mod scrollbar;
pub mod sidebar;
pub mod slash_suggest;
pub mod status;
pub mod status_bar;
pub mod subagents;
pub mod timefmt;
pub mod toast;
pub mod todos;

pub use render::render;

#[cfg(test)]
mod tests {
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
}
