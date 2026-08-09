// ── ui/dialogs/help.rs: Help / keybinding cheatsheet ──────────────────
// Grok-style: ModalWindow chrome + section headers + key/desc columns.
// Scrolls when content exceeds the body; solid scrollbar on the right.
// Same modal chrome hits as every other popup (selection clip + [✗]).

use crate::app::TuiApp;
use crate::theme::ThemePalette;
use crate::ui::scrollbar::{ScrollbarColors, paint_scrollbar};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Wrap},
};

use super::base::dialog_frame;

const KEY_COL: usize = 16;

/// Paint help and register the same modal hit boxes as list/confirm dialogs.
pub fn render_help_overlay(frame: &mut Frame, app: &mut TuiApp, palette: &ThemePalette) {
    let chrome = dialog_frame(
        frame,
        "Help",
        &["↑/↓ scroll", "Esc/?/q close"],
        palette,
        65,
        75,
        app.mouse_pos,
    );
    let area = chrome.content;
    if area.width == 0 || area.height == 0 {
        app.apply_modal_chrome(chrome.close_hit, chrome.modal, None);
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
        section("Sessions"),
        row("Ctrl+O", "Live session dashboard"),
        row("Ctrl+N", "New session"),
        row("Ctrl+Tab", "Switch session (recent)"),
        row("Ctrl+PgUp/PgDn", "Cycle sessions"),
        row("Ctrl+W", "Close live session (in /sessions)"),
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
        row("/login", "Sign in (OAuth)"),
        row(":theme", "Change theme"),
        row(":q", "Quit"),
    ];

    // Scroll: skip leading lines per help_scroll, keep footer-safe window.
    let total = help_text.len();
    let max_rows = area.height as usize;
    let max_scroll = total.saturating_sub(max_rows);
    let start = app.help_scroll.min(max_scroll);
    // Keep stored scroll in range after resize.
    if app.help_scroll > max_scroll {
        app.help_scroll = max_scroll;
    }
    let needs_scrollbar = total > max_rows;
    let list_width = if needs_scrollbar {
        area.width.saturating_sub(1)
    } else {
        area.width
    };
    let visible: Vec<Line> = help_text.into_iter().skip(start).take(max_rows).collect();

    let list_area = Rect {
        x: area.x,
        y: area.y,
        width: list_width,
        height: area.height,
    };
    let p = Paragraph::new(Text::from(visible))
        .wrap(Wrap { trim: false })
        .style(Style::default().bg(palette.bg));
    frame.render_widget(p, list_area);

    let scrollbar_hit = if needs_scrollbar {
        let colors = ScrollbarColors::from_palette(palette);
        let sb = Rect {
            x: area.x + area.width.saturating_sub(1),
            y: area.y,
            width: 1,
            height: area.height,
        };
        paint_scrollbar(
            frame.buffer_mut(),
            sb,
            total,
            max_rows,
            start,
            colors.track,
            colors.thumb,
        );
        Some(sb)
    } else {
        None
    };

    // Same hit contract as SessionList / Theme / Permission.
    app.apply_modal_chrome(chrome.close_hit, chrome.modal, scrollbar_hit);
    app.dialog_list_hit = Some(list_area);
    app.dialog_list_scroll_start = start;
    app.dialog_list_visible = max_rows;
    app.dialog_list_total = total;
}
