// ── ui/header.rs: thin top bar (session chrome) ────────────────────────
// OpenCode session route doesn't use a heavy title box; keep a 1-line bar.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
    Frame,
};
use crate::app::TuiApp;
use crate::theme::ThemePalette;

pub fn render(frame: &mut Frame, area: Rect, app: &TuiApp, palette: &ThemePalette) {
    let brand = Span::styled(
        " whycode ",
        Style::default()
            .fg(palette.accent)
            .add_modifier(Modifier::BOLD),
    );
    let agent_color = palette.agent_color_by_index(app.agent_cycle_idx);
    let agent = Span::styled(
        format!(" {} ", app.agent_name),
        Style::default()
            .fg(agent_color)
            .add_modifier(Modifier::BOLD),
    );
    let rest = Span::styled(
        format!(
            " {}/{} ",
            if app.provider_name.is_empty() {
                "—"
            } else {
                app.provider_name.as_str()
            },
            if app.model_name.is_empty() {
                "—"
            } else {
                app.model_name.as_str()
            }
        ),
        Style::default().fg(palette.dim),
    );

    frame.render_widget(
        Paragraph::new(Text::from(Line::from(vec![brand, agent, rest])))
            .style(Style::default().bg(palette.status_bar_bg)),
        area,
    );
}
