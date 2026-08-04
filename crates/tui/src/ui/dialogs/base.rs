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
    /// Full modal rect (border inclusive).
    pub modal: Rect,
    /// Clickable `[✗]` on the top-right border (if painted).
    pub close_hit: Option<Rect>,
}

/// Geometry of the top-right close control (` [✗] `, 5 cells).
///
/// Shared by paint and mouse hit-testing so the glyph and the click target
/// never drift apart.
pub fn close_button_rect(modal: Rect) -> Option<Rect> {
    const W: u16 = 5; // ` [✗] `
    if modal.width < W + 2 {
        return None;
    }
    let x0 = modal.x + modal.width.saturating_sub(W + 2);
    Some(Rect {
        x: x0,
        y: modal.y,
        width: W,
        height: 1,
    })
}

/// Paint a Grok-style centered modal and return the content rect.
///
/// `shortcuts` are labels like `"Esc cancel"` / `"Enter select"` — the first
/// whitespace-separated token is the key (bold), the rest is the hint (dim),
/// joined with `  |  ` and centered on the footer row.
///
/// `mouse_pos` drives hover styling on the top-right `[✗]` control.
pub fn dialog_frame(
    frame: &mut Frame,
    title: &str,
    shortcuts: &[&str],
    palette: &ThemePalette,
    percent_x: u16,
    percent_y: u16,
    mouse_pos: Option<(u16, u16)>,
) -> DialogChrome {
    let area = frame.area();
    let dialog_area = centered_rect(percent_x, percent_y, area);
    if dialog_area.width < 12 || dialog_area.height < 5 {
        return DialogChrome {
            content: dialog_area,
            modal: dialog_area,
            close_hit: None,
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

    // Top-right [✗] — painted and hit-testable (click = Esc).
    let close_hit = close_button_rect(dialog_area);
    let close_hovered = match (close_hit, mouse_pos) {
        (Some(hit), Some((c, r))) => {
            c >= hit.x && c < hit.x.saturating_add(hit.width) && r == hit.y
        }
        _ => false,
    };
    paint_close_button(frame, dialog_area, palette, close_hovered);

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

    DialogChrome {
        content,
        modal: dialog_area,
        close_hit,
    }
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

fn paint_close_button(
    frame: &mut Frame,
    modal: Rect,
    palette: &ThemePalette,
    hovered: bool,
) {
    let Some(hit) = close_button_rect(modal) else {
        return;
    };
    // Five cells: ` [✗] ` (must match `close_button_rect` width).
    let cells = [" ", "[", "✗", "]", " "];
    debug_assert_eq!(cells.len() as u16, hit.width);
    let buf = frame.buffer_mut();
    // Idle: dim chrome. Hover: error red so the control reads as "close".
    let fg = if hovered { palette.error } else { palette.dim };
    let style = Style::default()
        .fg(fg)
        .bg(palette.bg)
        .add_modifier(if hovered {
            Modifier::BOLD
        } else {
            Modifier::empty()
        });
    for (i, sym) in cells.iter().enumerate() {
        if let Some(cell) = buf.cell_mut((hit.x + i as u16, hit.y)) {
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

    #[test]
    fn close_button_sits_on_top_right_of_modal() {
        let modal = Rect {
            x: 10,
            y: 5,
            width: 40,
            height: 20,
        };
        let hit = close_button_rect(modal).expect("wide enough");
        assert_eq!(hit.y, modal.y);
        assert_eq!(hit.width, 5);
        assert_eq!(hit.height, 1);
        // Inset 2 from the right edge of the modal.
        assert_eq!(hit.x + hit.width + 2, modal.x + modal.width);
    }
}
