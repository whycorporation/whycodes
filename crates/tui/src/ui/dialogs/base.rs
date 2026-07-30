// ── ui/dialogs/base.rs: Dialog frame utilities ─────────────────────────

use crate::theme::ThemePalette;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph},
};

/// Render a centered dialog frame with title and border.
/// Returns the inner area for body content.
pub fn dialog_frame(
    frame: &mut Frame,
    title: &str,
    palette: &ThemePalette,
    percent_x: u16,
    percent_y: u16,
) -> Rect {
    let area = frame.area();
    let dialog_area = centered_rect(percent_x, percent_y, area);

    // Clear background.
    frame.render_widget(Clear, dialog_area);

    // Render the frame border.
    let block = Block::default()
        .title(format!(" {} ", title))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette.dialog_border))
        .style(Style::default().bg(palette.dialog_bg));

    frame.render_widget(block, dialog_area);

    // Inner area (inside borders).

    Rect {
        x: dialog_area.x + 1,
        y: dialog_area.y + 1,
        width: dialog_area.width.saturating_sub(2),
        height: dialog_area.height.saturating_sub(2),
    }
}

/// Render dialog footer buttons.
pub fn dialog_footer(
    frame: &mut Frame,
    area: Rect,
    buttons: &[&str],
    palette: &ThemePalette,
    active: usize,
) {
    let button_spans: Vec<Span> = buttons
        .iter()
        .enumerate()
        .flat_map(|(i, label)| {
            let style = if i == active {
                Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(palette.dim)
            };
            vec![Span::styled(format!(" {} ", label), style), Span::raw(" ")]
        })
        .collect();

    let p = Paragraph::new(Text::from(Line::from(button_spans)))
        .style(Style::default().bg(palette.dialog_bg));
    frame.render_widget(p, area);
}

/// Create a centered rectangle.
pub fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup[1])[1]
}
