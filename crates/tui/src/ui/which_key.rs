// ── which_key.rs: Keyboard shortcut popup (like OpenCode's which-key.tsx) ─
// Activated by pressing '?' or Ctrl+H. Shows context-appropriate shortcuts.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::keymap::{self, KeyBinding, KeymapContext};

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
        "KEY", "DESCRIPTION",
        cw = col1_width,
        cd = col2_width
    );
    lines.push(Line::from(Span::styled(
        header,
        Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
    )));
    let sep = format!(
        "  {:-<cw$} {:-<cd$}",
        "", "",
        cw = col1_width,
        cd = col2_width
    );
    lines.push(Line::from(Span::raw(sep)));

    // Filter by scroll
    let max_visible = (popup_area.height as usize).saturating_sub(6);
    let start = scroll.min(bindings.len().saturating_sub(max_visible));
    let visible: Vec<&KeyBinding> = bindings.iter().skip(start).take(max_visible).collect();

    if start > 0 {
        lines.push(Line::from(Span::styled(
            format!("  {} more above...", start),
            Style::default().fg(Color::DarkGray),
        )));
    }

    for binding in &visible {
        let key_style = Style::default()
            .fg(Color::Rgb(255, 200, 100))
            .add_modifier(Modifier::BOLD);
        let desc_style = Style::default().fg(Color::Rgb(210, 210, 220));

        let mut spans = vec![
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
        ];
        lines.push(Line::from(spans));
    }

    let remaining = bindings.len().saturating_sub(start + visible.len());
    if remaining > 0 {
        lines.push(Line::from(Span::styled(
            format!("  {} more below...", remaining),
            Style::default().fg(Color::DarkGray),
        )));
    }

    // Footer
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  up/down scroll  |  Esc / ? / Ctrl+H to close  ",
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
}
