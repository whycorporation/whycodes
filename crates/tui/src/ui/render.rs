// ── ui/render.rs: Main render entry point ──────────────────────────────
// Routes to the correct view based on application mode and overlays.

use ratatui::Frame;
use crate::app::TuiApp;
use crate::theme::ThemePalette;

use super::chat;
use super::dialogs;
use super::prompt;
use super::sidebar;
use super::status;

pub fn render(frame: &mut Frame, app: &TuiApp) {
    let palette = app.theme.palette();

    // If a dialog is open, render it on top.
    if app.dialogs.is_open() {
        render_main_underlay(frame, app, &palette);
        dialogs::render(frame, app, &palette);
        return;
    }

    // Help overlay.
    if let crate::app::AppMode::Help = app.mode {
        render_main_underlay(frame, app, &palette);
        dialogs::render_help(frame, app, &palette);
        return;
    }

    // Normal / Session / Command layout.
    render_main_underlay(frame, app, &palette);
}

fn render_main_underlay(frame: &mut Frame, app: &TuiApp, palette: &ThemePalette) {
    use ratatui::layout::{Constraint, Direction, Layout};

    let area = frame.area();

    // Split: sidebar (if visible) | main content
    let main_chunks = if app.sidebar.visible {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(25),
                Constraint::Percentage(75),
            ])
            .split(area);
        sidebar::render(frame, chunks[0], app, palette);
        chunks[1]
    } else {
        area
    };

    // Main area: chat | prompt | status bar
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),      // chat
            Constraint::Length(3),    // prompt
            Constraint::Length(1),    // status bar
        ])
        .split(main_chunks);

    chat::render(frame, vertical[0], app, palette);
    prompt::render(frame, vertical[1], app, palette);
    status::render(frame, vertical[2], app, palette);
}
