// ── ui/dialogs/alert.rs: Alert/info dialog ─────────────────────────────

use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
    Frame,
};
use crate::theme::ThemePalette;

use super::base::dialog_frame;

pub fn render_alert_dialog(frame: &mut Frame, title: &str, message: &str, palette: &ThemePalette) {
    let area = dialog_frame(frame, title, palette, 50, 30);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    // Body.
    let body = Paragraph::new(Text::from(vec![
        Line::from(""),
        Line::from(Span::styled(
            message,
            Style::default().fg(palette.fg),
        )),
        Line::from(""),
    ]));
    frame.render_widget(body, chunks[0]);

    // Footer.
    let footer = Paragraph::new(Line::from(Span::styled(
        "  Press any key to dismiss  ",
        Style::default().fg(palette.dim),
    )));
    frame.render_widget(footer, chunks[1]);
}
