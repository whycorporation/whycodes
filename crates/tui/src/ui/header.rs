// ── ui/header.rs: thin top bar (session chrome) ────────────────────────
// OpenCode session route doesn't use a heavy title box; keep a 1-line bar.

use crate::app::TuiApp;
use crate::theme::ThemePalette;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
};

pub fn render(frame: &mut Frame, area: Rect, app: &TuiApp, palette: &ThemePalette) {
    // Dual-tone wordmark matches the top status bar brand treatment.
    let brand_why = Span::styled(
        " why",
        Style::default().fg(palette.fg).add_modifier(Modifier::BOLD),
    );
    let brand_code = Span::styled(
        "code ",
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
    let mut spans = vec![brand_why, brand_code, agent];
    if let Some(ref badge) = app.intent_badge {
        let badge_color = match app.intent_kind.as_deref() {
            Some("question") => palette.info,
            Some("plan") => palette.accent,
            Some("change") => palette.success,
            _ => palette.dim,
        };
        spans.push(Span::styled(
            format!("[{badge}]"),
            Style::default()
                .fg(badge_color)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(
        format!(
            "{}/{} ",
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
    ));

    frame.render_widget(
        Paragraph::new(Text::from(Line::from(spans)))
            .style(Style::default().bg(palette.status_bar_bg)),
        area,
    );
}
