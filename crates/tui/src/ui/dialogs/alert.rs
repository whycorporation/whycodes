// ── ui/dialogs/alert.rs: Alert/info dialog ─────────────────────────────
// Grok-style ModalWindow chrome + dismiss footer.

use crate::theme::ThemePalette;
use ratatui::{
    Frame,
    style::Style,
    text::{Line, Span, Text},
    widgets::{Paragraph, Wrap},
};

use super::base::{DialogChrome, dialog_frame};

pub fn render_alert_dialog(
    frame: &mut Frame,
    title: &str,
    message: &str,
    palette: &ThemePalette,
) -> DialogChrome {
    let chrome = dialog_frame(frame, title, &["any-key / [✗]"], palette, 50, 30);
    let area = chrome.content;
    if area.width == 0 || area.height == 0 {
        return chrome;
    }

    let body = Paragraph::new(Text::from(vec![
        Line::from(""),
        Line::from(Span::styled(message, Style::default().fg(palette.fg))),
    ]))
    .wrap(Wrap { trim: true })
    .style(Style::default().bg(palette.bg));
    frame.render_widget(body, area);
    chrome
}
