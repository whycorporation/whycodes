// ── ui/dialogs/help.rs: Help / keybinding cheatsheet ──────────────────

use crate::app::TuiApp;
use crate::theme::ThemePalette;
use ratatui::{
    Frame,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Wrap},
};

use super::base::dialog_frame;

pub fn render_help_overlay(frame: &mut Frame, _app: &TuiApp, palette: &ThemePalette) {
    let area = dialog_frame(frame, " Help ", palette, 65, 75);

    let help_text = vec![
        // ── General ──
        Line::from(Span::styled(
            " General ",
            Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("  ?           Toggle this help screen"),
        Line::from("  :           Enter command mode"),
        Line::from("  Ctrl+C, q   Quit"),
        Line::from("  Esc         Exit current mode"),
        Line::from(""),
        // ── Commands ──
        Line::from(Span::styled(
            " Commands ",
            Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("  :q, :quit    Quit whycode"),
        Line::from("  :h, :help    Show help"),
        Line::from("  :provider    Select / add provider"),
        Line::from("  :model       Select model"),
        Line::from("  :theme       Change theme"),
        Line::from("  :clear       Clear session"),
        Line::from("  :sidebar     Toggle sidebar"),
        Line::from(""),
        // ── Navigation ──
        Line::from(Span::styled(
            " Navigation ",
            Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("  ↑/k, ↓/j    Scroll messages"),
        Line::from("  PgUp/PgDown   Scroll page"),
        Line::from("  Home/End      Jump to top/bottom"),
        Line::from("  Ctrl+A      Toggle auto-scroll"),
        Line::from(""),
        // ── Dialogs ──
        Line::from(Span::styled(
            " Dialogs ",
            Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("  Ctrl+P      Open provider dialog"),
        Line::from("  Ctrl+M      Open model dialog"),
        Line::from("  Ctrl+B      Toggle sidebar"),
        Line::from("  Tab/↑↓      Navigate fields"),
        Line::from("  Enter / y   Confirm"),
        Line::from("  Esc / q / n Cancel"),
        Line::from(""),
        // ── Input ──
        Line::from(Span::styled(
            " Input ",
            Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("  Ctrl+Space  File autocomplete"),
        Line::from("  Ctrl+U      Clear input"),
        Line::from("  ↑/↓          History navigation"),
        Line::from(""),
        Line::from(Span::styled(
            " Press Esc, q, or ? to close help ",
            Style::default().fg(palette.accent),
        )),
    ];

    let p = Paragraph::new(Text::from(help_text)).wrap(Wrap { trim: true });
    frame.render_widget(p, area);
}
