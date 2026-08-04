// ── ui/dialogs/help.rs: Help / keybinding cheatsheet ──────────────────
// Grok-style: ModalWindow chrome + section headers + key/desc columns.

use crate::app::TuiApp;
use crate::theme::ThemePalette;
use ratatui::{
    Frame,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Wrap},
};

use super::base::dialog_frame;

const KEY_COL: usize = 16;

pub fn render_help_overlay(frame: &mut Frame, app: &TuiApp, palette: &ThemePalette) {
    let chrome = dialog_frame(
        frame,
        "Help",
        &["↑/↓ scroll", "Esc/?/q close"],
        palette,
        65,
        75,
    );
    let area = chrome.content;
    if area.width == 0 || area.height == 0 {
        return;
    }

    let section = |label: &str| {
        Line::from(Span::styled(
            label.to_string(),
            Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD),
        ))
    };

    let row = |key: &str, desc: &str| {
        let key_pad = format!("{key:<width$}", width = KEY_COL);
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                key_pad,
                Style::default().fg(palette.fg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(desc.to_string(), Style::default().fg(palette.dim)),
        ])
    };

    let help_text = vec![
        section("General"),
        row("?", "Toggle this help"),
        row("Tab", "Focus prompt ↔ scrollback"),
        row("Ctrl+T", "Cycle primary agent"),
        row(":", "Enter command mode"),
        row("Esc", "Cancel turn · double-Esc clear draft"),
        row("Ctrl+C", "Clear draft / quit"),
        Line::from(""),
        section("Scrollback"),
        row("j/k · ↑/↓", "Select message"),
        row("Ctrl+↑/↓", "Scroll transcript"),
        row("PgUp/PgDn", "Page scroll"),
        row("g / G", "Top / bottom"),
        row("Shift+←/→", "Prev / next user turn"),
        row("y", "Copy selected message"),
        row("e / h", "Toggle thinking fold"),
        row("l", "Toggle tool results"),
        row("Space / i", "Focus prompt"),
        Line::from(""),
        section("Prompt"),
        row("Enter", "Send message"),
        row("Ctrl+Space", "File autocomplete"),
        row("Ctrl+U", "Clear input"),
        row("↑/↓", "History (empty prompt)"),
        Line::from(""),
        section("Dialogs & commands"),
        row("Ctrl+P", "Provider setup"),
        row("Ctrl+M", "Model selection"),
        row("Ctrl+B", "Toggle sidebar"),
        row("Ctrl+A", "Toggle auto-scroll"),
        row("Ctrl+L", "Clear session"),
        row("/help", "This screen"),
        row("/connect", "Provider help"),
        row(":theme", "Change theme"),
        row(":q", "Quit"),
    ];

    // Scroll: skip leading lines per help_scroll, keep footer-safe window.
    let max_rows = area.height as usize;
    let max_scroll = help_text.len().saturating_sub(max_rows);
    let start = app.help_scroll.min(max_scroll);
    let visible: Vec<Line> = help_text.into_iter().skip(start).take(max_rows).collect();

    let p = Paragraph::new(Text::from(visible))
        .wrap(Wrap { trim: false })
        .style(Style::default().bg(palette.bg));
    frame.render_widget(p, area);
}
