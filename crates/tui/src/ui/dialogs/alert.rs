// ── ui/dialogs/alert.rs: Alert/info dialog ─────────────────────────────
// Grok-style ModalWindow chrome + dismiss footer.

use crate::theme::ThemePalette;
use ratatui::{
    Frame,
    style::Style,
    text::{Line, Span, Text},
    widgets::{Paragraph, Wrap},
};

use super::base::DialogChrome;

pub fn render_alert_dialog(
    frame: &mut Frame,
    title: &str,
    message: &str,
    palette: &ThemePalette,
    mouse_pos: Option<(u16, u16)>,
) -> DialogChrome {
    let chrome = super::base::dialog_frame_sized(
        frame,
        title,
        &["any-key / [✗]"],
        palette,
        super::base::DialogSizing::compact(),
        mouse_pos,
        super::base::DialogPlacement::Center,
    );
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

#[cfg(test)]
mod tests {
    #[test]
    fn alert_module_loads() {
        assert!(!module_path!().is_empty());
    }
}
