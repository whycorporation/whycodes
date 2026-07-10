// ── ui/status.rs: Status bar ───────────────────────────────────────────

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
    Frame,
};
use crate::app::{AppMode, TuiApp};
use crate::theme::ThemePalette;

pub fn render(frame: &mut Frame, area: Rect, app: &TuiApp, palette: &ThemePalette) {
    let mode_str = match app.mode {
        AppMode::Normal => "NORMAL",
        AppMode::Session => "SESSION",
        AppMode::Command => "COMMAND",
        AppMode::Dialog => "DIALOG",
        AppMode::Help => "HELP",
    };

    let agent_str = match app.current_agent_state {
        crate::app::AgentState::Idle => "⚪ Idle",
        crate::app::AgentState::Generating => "🟢 Generating",
        crate::app::AgentState::Thinking => "💭 Thinking",
        crate::app::AgentState::WaitingForPermission => "⏳ Waiting...",
        crate::app::AgentState::Error(_) => "🔴 Error",
    };

    let shortcuts = match app.mode {
        AppMode::Normal =>
            "? help | Ctrl+P provider | Ctrl+B sidebar | Ctrl+C quit",
        _ => "",
    };

    let text = format!(
        " {} │ {} │ {} │ Msgs: {} │ {}",
        app.status_message,
        mode_str,
        agent_str,
        app.messages.len(),
        shortcuts,
    );

    let status = Paragraph::new(Text::from(Line::from(Span::styled(
        text,
        Style::default().fg(palette.status_bar_fg).add_modifier(Modifier::BOLD),
    ))))
    .style(Style::default().bg(palette.status_bar_bg));

    frame.render_widget(status, area);
}
