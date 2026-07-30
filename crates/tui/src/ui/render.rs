// ── ui/render.rs: OpenCode home + session shells ───────────────────────
//
// home.tsx:
//   [grow] logo [gap] prompt(maxW) [grow] footer
//
// session/index.tsx:
//   row [ main(pad 2) | sidebar? ]
//   main: scroll messages | prompt | (footer is outside in some hosts;
//         we keep footer as bottom strip for path + status)

use crate::app::TuiApp;
use crate::opencode_tokens::layout as oc;
use crate::theme::ThemePalette;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::Block;

use super::chat;
use super::dialogs;
use super::prompt;
use super::sidebar;
use super::status;

pub fn render(frame: &mut Frame, app: &TuiApp) {
    let palette = app.theme.palette();

    frame.render_widget(
        Block::default().style(Style::default().bg(palette.bg)),
        frame.area(),
    );

    if app.dialogs.is_open() {
        render_shell(frame, app, &palette);
        dialogs::render(frame, app, &palette);
        return;
    }

    if let crate::app::AppMode::Help = app.mode {
        render_shell(frame, app, &palette);
        dialogs::render_help(frame, app, &palette);
        return;
    }

    render_shell(frame, app, &palette);
}

fn render_shell(frame: &mut Frame, app: &TuiApp, palette: &ThemePalette) {
    let area = frame.area();

    // Outer: content + footer (footer always full-width, OpenCode home_footer)
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(area);

    let body = outer[0];
    status::render(frame, outer[1], app, palette);

    if app.messages.is_empty() {
        render_home(frame, body, app, palette);
    } else {
        render_session(frame, body, app, palette);
    }
}

/// home.tsx vertical stack — logo area grows, prompt fixed, no header chrome
fn render_home(frame: &mut Frame, area: Rect, app: &TuiApp, palette: &ThemePalette) {
    // [messages/logo grow] [prompt 3]  — prompt slightly taller for meta row
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),
            Constraint::Length(3), // input + agent/model meta
        ])
        .split(area);

    chat::render(frame, chunks[0], app, palette);
    prompt::render(frame, chunks[1], app, palette);
}

/// session: optional sidebar + padded main column
fn render_session(frame: &mut Frame, area: Rect, app: &TuiApp, palette: &ThemePalette) {
    let main = if app.sidebar.visible {
        let w = oc::SIDEBAR_WIDTH.min(area.width / 3).max(24);
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(20), Constraint::Length(w)])
            .split(area);
        // OpenCode: main left, sidebar right
        sidebar::render(frame, chunks[1], app, palette);
        chunks[0]
    } else {
        area
    };

    // paddingLeft/Right = 2 via inset
    let inset = Rect {
        x: main.x.saturating_add(oc::SIDE_PAD),
        y: main.y,
        width: main.width.saturating_sub(oc::SIDE_PAD * 2),
        height: main.height.saturating_sub(1), // paddingBottom=1
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),    // scroll messages
            Constraint::Length(3), // prompt + model meta
        ])
        .split(inset);

    chat::render(frame, chunks[0], app, palette);
    prompt::render(frame, chunks[1], app, palette);
}
