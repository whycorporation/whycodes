// ── widgets/tool_call.rs: Collapsible tool call widget ─────────────────

use crate::theme::ThemePalette;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Wrap},
};

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

    let color = if is_error {
        palette.error
    } else {
        palette.tool_msg
    };

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

        // Result (if available). Cap each line so minified JSON cannot flood.
        if let Some(res) = result {
            lines.push(Line::from(Span::styled(
                "   ── result ──",
                Style::default().fg(palette.dim),
            )));
            const MAX_COLS: usize = 96;
            for line in res.lines().take(10) {
                let shown = if line.chars().count() > MAX_COLS {
                    format!(
                        "{}…",
                        line.chars().take(MAX_COLS.saturating_sub(1)).collect::<String>()
                    )
                } else {
                    line.to_string()
                };
                lines.push(Line::from(Span::styled(
                    format!("   │ {shown}"),
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
