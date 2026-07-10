// ── ui/prompt.rs: Input prompt ─────────────────────────────────────────

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};
use crate::app::{AppMode, TuiApp};
use crate::theme::ThemePalette;

pub fn render(frame: &mut Frame, area: Rect, app: &TuiApp, palette: &ThemePalette) {
    let prompt_prefix = match app.mode {
        AppMode::Command => ":",
        _ => ">",
    };

    let display_text = match app.mode {
        AppMode::Normal => {
            if app.input_buffer.is_empty() {
                format!("{} ", prompt_prefix)
            } else {
                format!("{} {}", prompt_prefix, app.input_buffer)
            }
        }
        AppMode::Command => {
            format!("{}", app.command.buffer)
        }
        _ => String::new(),
    };

    let mut spans = vec![Span::styled(
        display_text.clone(),
        Style::default().fg(palette.input_fg),
    )];

    // Cursor.
    spans.push(Span::styled(
        "█",
        Style::default().fg(palette.accent),
    ));

    // File autocomplete hint.
    if app.mode == AppMode::Normal && app.input_buffer.contains('/') {
        spans.push(Span::styled(
            "  [Ctrl+Space for file autocomplete]",
            Style::default().fg(palette.dim),
        ));
    }

    let title = match app.mode {
        AppMode::Command => " Command ",
        _ => " Prompt ",
    };

    let input_style = match app.mode {
        AppMode::Command => Style::default().fg(palette.accent),
        _ => Style::default(),
    };

    let paragraph = Paragraph::new(Text::from(Line::from(spans)))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(input_style)
                .style(Style::default().bg(palette.input_bg)),
        )
        .wrap(Wrap { trim: true });

    frame.render_widget(paragraph, area);
}
