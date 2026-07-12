// ── widgets/tool_call.rs: Collapsible tool call widget ─────────────────

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Wrap},
    Frame,
};
use crate::theme::ThemePalette;

/// Render a collapsible tool-call widget.
#[allow(clippy::too_many_arguments)]
pub fn render_tool_call(
    frame: &mut Frame,
    area: Rect,
    name: &str,
    arguments: &str,
    result: Option<&str>,
    is_error: bool,
    collapsed: bool,
    palette: &ThemePalette,
) {
    let mut lines: Vec<Line> = Vec::new();

    let color = if is_error { palette.error } else { palette.tool_msg };

    if collapsed {
        lines.push(Line::from(Span::styled(
            format!(" ▶ {} — {}", name, arguments),
            Style::default().fg(color),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            format!(" ▼ {}", name),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )));

        // Arguments.
        for line in arguments.lines() {
            lines.push(Line::from(Span::styled(
                format!("   args: {}", line),
                Style::default().fg(palette.dim),
            )));
        }

        // Result (if available).
        if let Some(res) = result {
            lines.push(Line::from(Span::styled(
                "   ── result ──",
                Style::default().fg(palette.dim),
            )));
            for line in res.lines().take(10) {
                lines.push(Line::from(Span::styled(
                    format!("   │ {}", line),
                    Style::default().fg(color).add_modifier(Modifier::DIM),
                )));
            }
            if res.lines().count() > 10 {
                lines.push(Line::from(Span::styled(
                    "   │ ... (more lines) ",
                    Style::default().fg(palette.dim),
                )));
            }
        }
    }

    let p = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: true });
    frame.render_widget(p, area);
}
