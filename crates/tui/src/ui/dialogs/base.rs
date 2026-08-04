// ── ui/dialogs/base.rs: Grok-style modal chrome ─────────────────────────
// Visual model from Grok Build `ModalWindow` (`modal_window.rs`):
//   Clear · square dim border · bold "─ Title ─" · [✗] on top-right ·
//   padded body · centered footer shortcuts (key bold, label dim).
//
// No full-screen scrim — Grok clears only the popup rect.

use crate::theme::ThemePalette;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use unicode_width::UnicodeWidthStr;

/// Horizontal padding inside the border (both sides).
const H_PAD: u16 = 2;
/// Vertical padding above content (below top border).
const V_PAD: u16 = 1;
/// Footer shortcut row height.
const FOOTER_LINES: u16 = 1;

/// Areas after painting the modal frame.
pub struct DialogChrome {
    /// Body content (inside padding, above footer).
    pub content: Rect,
}

/// Paint a Grok-style centered modal and return the content rect.
///
/// `shortcuts` are labels like `"Esc cancel"` / `"Enter select"` — the first
/// whitespace-separated token is the key (bold), the rest is the hint (dim),
/// joined with `  |  ` and centered on the footer row.
pub fn dialog_frame(
    frame: &mut Frame,
    title: &str,
    shortcuts: &[&str],
    palette: &ThemePalette,
    percent_x: u16,
    percent_y: u16,
) -> DialogChrome {
    let area = frame.area();
    let dialog_area = centered_rect(percent_x, percent_y, area);
    if dialog_area.width < 12 || dialog_area.height < 5 {
        return DialogChrome {
            content: dialog_area,
        };
    }

    // Clear cells under the modal so content behind doesn't bleed through.
    frame.render_widget(Clear, dialog_area);

    // Grok: border = gray_dim on bg_base; title bold primary on same fill.
    let border_style = Style::default().fg(palette.dim).bg(palette.bg);
    let title_style = Style::default()
        .fg(palette.fg)
        .bg(palette.bg)
        .add_modifier(Modifier::BOLD);
    let fill = Style::default().bg(palette.bg).fg(palette.fg);

    let t = title.trim();
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(fill);
    if !t.is_empty() {
        // Decorative ─ around the title blend with the border (Grok).
        block = block.title(Line::from(vec![
            Span::styled("─ ", border_style),
            Span::styled(t.to_string(), title_style),
            Span::styled(" ─", border_style),
        ]));
    }

    let inner = block.inner(dialog_area);
    frame.render_widget(block, dialog_area);

    // Top-right [✗] on the border (visual parity; Esc still closes).
    paint_close_button(frame, dialog_area, palette);

    let footer_h = if shortcuts.is_empty() {
        0
    } else {
        FOOTER_LINES
    };
    let content = Rect {
        x: inner.x + H_PAD,
        y: inner.y + V_PAD,
        width: inner.width.saturating_sub(H_PAD * 2),
        height: inner.height.saturating_sub(V_PAD + footer_h),
    };

    if footer_h > 0 {
        let footer = Rect {
            x: inner.x + H_PAD,
            y: inner.y + inner.height.saturating_sub(footer_h),
            width: inner.width.saturating_sub(H_PAD * 2),
            height: footer_h,
        };
        paint_footer_shortcuts(frame, footer, shortcuts, palette);
    }

    DialogChrome { content }
}

/// Create a centered rectangle as a percentage of `r`.
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

// ── private chrome ─────────────────────────────────────────────────────

fn paint_close_button(frame: &mut Frame, modal: Rect, palette: &ThemePalette) {
    // Five cells: ` [✗] `, inset 2 columns from the right edge.
    let cells = [" ", "[", "✗", "]", " "];
    let w = cells.len() as u16;
    if modal.width < w + 2 {
        return;
    }
    let x0 = modal.x + modal.width.saturating_sub(w + 2);
    let y = modal.y;
    let buf = frame.buffer_mut();
    let style = Style::default().fg(palette.dim).bg(palette.bg);
    for (i, sym) in cells.iter().enumerate() {
        if let Some(cell) = buf.cell_mut((x0 + i as u16, y)) {
            cell.set_symbol(sym);
            cell.set_style(style);
        }
    }
}

fn paint_footer_shortcuts(
    frame: &mut Frame,
    area: Rect,
    shortcuts: &[&str],
    palette: &ThemePalette,
) {
    if area.width == 0 || area.height == 0 || shortcuts.is_empty() {
        return;
    }
    let sep = "  |  ";
    let mut spans: Vec<Span> = Vec::new();
    let mut total_w = 0usize;
    for (i, label) in shortcuts.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(
                sep.to_string(),
                Style::default().fg(palette.dim).bg(palette.bg),
            ));
            total_w += UnicodeWidthStr::width(sep);
        }
        let (key, rest) = split_shortcut_label(label);
        spans.push(Span::styled(
            key.to_string(),
            Style::default()
                .fg(palette.fg)
                .bg(palette.bg)
                .add_modifier(Modifier::BOLD),
        ));
        total_w += UnicodeWidthStr::width(key);
        if !rest.is_empty() {
            spans.push(Span::styled(
                rest.to_string(),
                Style::default().fg(palette.dim).bg(palette.bg),
            ));
            total_w += UnicodeWidthStr::width(rest);
        }
    }
    let pad = (area.width as usize).saturating_sub(total_w) / 2;
    let mut line_spans = vec![Span::styled(
        " ".repeat(pad),
        Style::default().bg(palette.bg),
    )];
    line_spans.extend(spans);
    frame.render_widget(
        Paragraph::new(Line::from(line_spans)).style(Style::default().bg(palette.bg)),
        area,
    );
}

/// Split `"Esc cancel"` → (`"Esc"`, `" cancel"`). Single token → whole as key.
fn split_shortcut_label(label: &str) -> (&str, &str) {
    match label.find(char::is_whitespace) {
        Some(i) => (&label[..i], &label[i..]),
        None => (label, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_shortcut_separates_key_and_label() {
        assert_eq!(split_shortcut_label("Esc cancel"), ("Esc", " cancel"));
        assert_eq!(split_shortcut_label("Enter"), ("Enter", ""));
        assert_eq!(split_shortcut_label("↑/↓ move"), ("↑/↓", " move"));
    }
}
