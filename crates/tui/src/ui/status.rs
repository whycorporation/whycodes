// ── ui/status.rs: OpenCode Footer ──────────────────────────────────────
// session/footer.tsx:
//   <box justifyContent="space-between">
//     <text muted>{directory}</text>
//     <box> permissions / LSP / MCP / /status  OR  Get started /connect
//   </box>

use crate::app::{AgentState, AppMode, TuiApp};
use crate::theme::ThemePalette;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
};

pub fn render(frame: &mut Frame, area: Rect, app: &TuiApp, palette: &ThemePalette) {
    let dir = truncate_start(&app.project_label, (area.width / 3).max(12) as usize);

    let left = Span::styled(format!(" {dir}"), Style::default().fg(palette.dim));

    // Right side — OpenCode welcome or status
    let no_key = app.status_message.contains("no API key")
        || app.status_message.contains("/connect")
        || (app.provider_name.is_empty() && app.messages.is_empty());

    let right: Vec<Span<'_>> = if no_key && matches!(app.current_agent_state, AgentState::Idle) {
        vec![
            Span::styled(
                String::from("Get started "),
                Style::default().fg(palette.fg),
            ),
            Span::styled(String::from("/connect"), Style::default().fg(palette.dim)),
            Span::raw(" "),
        ]
    } else {
        let state = match &app.current_agent_state {
            AgentState::Idle => {
                Span::styled(String::from("ready"), Style::default().fg(palette.success))
            }
            AgentState::Generating => Span::styled(
                String::from("generating"),
                Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            AgentState::Thinking => Span::styled(
                String::from("thinking"),
                Style::default().fg(palette.thinking),
            ),
            AgentState::WaitingForPermission => Span::styled(
                String::from("△ permission"),
                Style::default().fg(palette.warning),
            ),
            AgentState::Error(_) => {
                Span::styled(String::from("error"), Style::default().fg(palette.error))
            }
        };
        let hint = match app.mode {
            AppMode::Normal => "  /status",
            AppMode::Command => "  esc",
            AppMode::Dialog => "  y/n",
            AppMode::Help => "  q",
            AppMode::Session => "",
        };
        vec![
            state,
            Span::styled(String::from(hint), Style::default().fg(palette.dim)),
            Span::raw(" "),
        ]
    };

    // space-between: pad middle with spaces
    let left_w = dir.chars().count() + 1;
    let right_w: usize = right.iter().map(|s| s.content.chars().count()).sum();
    let mid = area
        .width
        .saturating_sub((left_w + right_w) as u16)
        .saturating_sub(1) as usize;

    let mut spans: Vec<Span<'_>> = vec![left, Span::raw(" ".repeat(mid))];
    spans.extend(right);

    frame.render_widget(
        Paragraph::new(Text::from(Line::from(spans))).style(Style::default().bg(palette.bg)),
        area,
    );
}

fn truncate_start(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        s.to_string()
    } else {
        format!("…{}", s.chars().skip(n - max + 1).collect::<String>())
    }
}
