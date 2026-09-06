// ── ui/header.rs: thin top bar (session chrome) ────────────────────────

use crate::app::TuiApp;
use crate::theme::ThemePalette;
use crate::tokens::HEADER_MARK;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
};

pub fn render(frame: &mut Frame, area: Rect, app: &TuiApp, palette: &ThemePalette) {
    // Tiny `?` glued to the wordmark: bold fg `why` + dim `codes`.
    let agent_color = app
        .config
        .agent_color(&app.agent_name, app.agent_cycle_idx, palette);
    let mut spans = vec![
        Span::styled(
            HEADER_MARK,
            Style::default().fg(palette.fg).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "why",
            Style::default().fg(palette.fg).add_modifier(Modifier::BOLD),
        ),
        Span::styled("codes ", Style::default().fg(palette.dim)),
        Span::styled(
            format!("v{} ", env!("CARGO_PKG_VERSION")),
            Style::default().fg(palette.dim),
        ),
        Span::styled(
            format!(" {} ", app.agent_name),
            Style::default()
                .fg(agent_color)
                .add_modifier(Modifier::BOLD),
        ),
    ];
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
        Style::default().fg(app.config.model_color(palette)),
    ));

    frame.render_widget(
        Paragraph::new(Text::from(Line::from(spans)))
            .style(Style::default().bg(palette.status_bar_bg)),
        area,
    );
}

#[cfg(test)]
#[path = "header_tests.rs"]
mod tests;
