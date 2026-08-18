// ── which_key.rs: Keyboard shortcut popup ──────────────────────────────
// Activated by `/help`. Shows context-appropriate shortcuts.
// When the binding list overflows, a solid scrollbar is painted on the right.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::keymap::{self, KeyBinding, KeymapContext};
use crate::ui::scrollbar::paint_scrollbar;

/// Render the which-key popup overlaid on the main area.
pub fn render(frame: &mut Frame, area: Rect, context: KeymapContext, scroll: usize) {
    let bindings = keymap::bindings_for_context(context);

    let col1_width: usize = 22;
    let col2_width: usize = 38;
    let total_width: u16 = (col1_width + col2_width + 4) as u16;
    let num_rows = bindings.len();
    if area.height < 6 || area.width < 20 {
        return;
    }
    let max_height = (area.height as usize).saturating_sub(4).max(6);
    let total_height: u16 = ((num_rows + 5).min(max_height).max(6)) as u16;

    let perc_y = ((total_height + 2) as u16 * 100 / area.height.max(1)).max(1);
    let popup_area = crate::widgets::centered_rect(total_width, perc_y, area);

    // Clear background
    frame.render_widget(Clear, popup_area);

    let context_label = match context {
        KeymapContext::Normal => "Normal Mode",
        KeymapContext::Dialog => "Dialog Mode",
        KeymapContext::Command => "Command Mode",
        KeymapContext::Help => "Help Mode",
        KeymapContext::Session => "Session List",
    };

    let mut lines: Vec<Line> = Vec::new();

    // Header
    lines.push(Line::from(Span::styled(
        format!("  {} Shortcuts  ", context_label),
        Style::default()
            .fg(Color::Rgb(100, 149, 237))
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    // Column headers
    let header = format!(
        "  {:<cw$} {:<cd$}",
        "KEY",
        "DESCRIPTION",
        cw = col1_width,
        cd = col2_width
    );
    lines.push(Line::from(Span::styled(
        header,
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )));
    let sep = format!(
        "  {:-<cw$} {:-<cd$}",
        "",
        "",
        cw = col1_width,
        cd = col2_width
    );
    lines.push(Line::from(Span::raw(sep)));

    // Binding rows occupy the middle; chrome is header(4) + footer(2).
    const CHROME_ROWS: usize = 6;
    let max_visible = (popup_area.height as usize).saturating_sub(CHROME_ROWS).max(1);
    let total_bindings = bindings.len();
    let needs_scrollbar = total_bindings > max_visible;
    let start = scroll.min(total_bindings.saturating_sub(max_visible));
    let visible: Vec<&KeyBinding> = bindings.iter().skip(start).take(max_visible).collect();

    for binding in &visible {
        let key_style = Style::default()
            .fg(Color::Rgb(255, 200, 100))
            .add_modifier(Modifier::BOLD);
        let desc_style = Style::default().fg(Color::Rgb(210, 210, 220));

        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                format!("{:<cw$}", binding.key, cw = col1_width),
                key_style,
            ),
            Span::styled(" ", Style::default()),
            Span::styled(
                format!("{:<cd$}", binding.description, cd = col2_width),
                desc_style,
            ),
        ]));
    }

    // Footer
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  up/down scroll  |  Esc / q to close  ",
        Style::default().fg(Color::DarkGray),
    )));

    let block = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(100, 149, 237)))
                .style(Style::default().bg(Color::Rgb(18, 18, 28))),
        )
        .wrap(Wrap { trim: true });

    frame.render_widget(block, popup_area);

    if needs_scrollbar && popup_area.height > 4 && popup_area.width > 2 {
        // Bar next to the binding rows only (below header, above footer).
        let header_inner = 4u16; // title + blank + KEY + sep
        let footer_inner = 2u16; // blank + hint
        let bar_y = popup_area.y + 1 + header_inner;
        let bar_h = popup_area
            .height
            .saturating_sub(2 + header_inner + footer_inner);
        if bar_h > 0 {
            let sb = Rect {
                x: popup_area.x + popup_area.width.saturating_sub(2),
                y: bar_y,
                width: 1,
                height: bar_h,
            };
            paint_scrollbar(
                frame.buffer_mut(),
                sb,
                total_bindings,
                max_visible,
                start,
                Color::Rgb(28, 28, 38),
                Color::Rgb(100, 100, 120),
            );
        }
    }
}
