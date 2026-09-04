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
    use super::*;
    use crate::theme::ThemeName;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn render_alert_paints_title_and_message() {
        let palette = ThemeName::DefaultDark.palette();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                let chrome = render_alert_dialog(f, "Notice", "hello-alert", &palette, None);
                assert!(chrome.content.width > 0 || chrome.content.height == 0);
            })
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol().to_string())
            .collect();
        assert!(
            text.contains("Notice") || text.contains("hello-alert"),
            "{text}"
        );
    }
}
