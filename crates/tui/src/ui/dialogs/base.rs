// ── ui/dialogs/base.rs: Grok-style modal chrome ─────────────────────────
// Visual model from Grok Build `ModalWindow` (`modal_window.rs`):
//   Clear · square dim border · bold "─ Title ─" · [✗] on top-right ·
//   padded body · centered footer shortcuts (key bold, label dim).
//
// No full-screen scrim — Grok clears only the popup rect.

use crate::theme::ThemePalette;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders},
};
use unicode_width::UnicodeWidthStr;

/// Horizontal padding inside the border (both sides). Wide terminals only.
const H_PAD: u16 = 2;
/// Vertical padding above content (below top border).
const V_PAD: u16 = 1;
/// Footer shortcut rows reserved (Grok Keyboard Shortcuts: 2).
const FOOTER_LINES: u16 = 2;
/// Prefer at least this many columns so titles/footers stay readable.
/// Phone-portrait terminals (~40–60 cols) otherwise get a tiny percentage box.
const MIN_MODAL_WIDTH: u16 = 36;
/// Prefer at least this many rows so body + footer survive short viewports
/// (on-screen keyboard shrinking the PTY).
const MIN_MODAL_HEIGHT: u16 = 10;
/// Grok `ModalSizing.max_width` for the shortcuts / picker family.
const POPUP_MAX_WIDTH: u16 = 80;
/// Grok `ModalSizing.min_width` for the shortcuts / picker family.
const POPUP_MIN_WIDTH: u16 = 44;
/// Grok `ModalSizing.width_pct` (percent).
const POPUP_WIDTH_PCT: u16 = 70;
/// Grok shortcuts `v_margin` (rows dropped from top and bottom).
const POPUP_V_MARGIN: u16 = 4;
/// Confirm/alert: same width, more outer margin so the box stays short.
const COMPACT_V_MARGIN: u16 = 8;

/// Grok compact chrome: drop inner pad when the PTY is a split/phone pane.
fn inner_h_pad(modal_w: u16) -> u16 {
    if modal_w < 48 { 1 } else { H_PAD }
}

fn inner_v_pad(modal_h: u16) -> u16 {
    if modal_h < 14 { 0 } else { V_PAD }
}

/// Areas after painting the modal frame.
pub struct DialogChrome {
    /// Body content (inside padding, above footer).
    pub content: Rect,
    /// Full modal rect (border inclusive).
    pub modal: Rect,
    /// Clickable `[✗]` on the top-right border (if painted).
    pub close_hit: Option<Rect>,
    /// Inner x (border to border, no h_pad) — full-width dividers.
    pub inner_x: u16,
    /// Inner width (border to border, no h_pad).
    pub inner_width: u16,
}

/// Grok Keyboard Shortcuts popup sizing (`shortcuts_help::modal_sizing`).
///
/// Width is `pct` of the outer area, then clamped to `[min, max]` and the
/// terminal. Height is the outer height minus `2 * v_margin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DialogSizing {
    pub width_pct: u16,
    pub max_width: u16,
    pub min_width: u16,
    pub v_margin: u16,
}

impl DialogSizing {
    /// Cheatsheet / picker family: 70% wide, cap 80, floor 44, 4-row margin.
    pub fn popup() -> Self {
        Self {
            width_pct: POPUP_WIDTH_PCT,
            max_width: POPUP_MAX_WIDTH,
            min_width: POPUP_MIN_WIDTH,
            v_margin: POPUP_V_MARGIN,
        }
    }

    /// Same width as [`Self::popup`], shorter box (confirm / alert).
    pub fn compact() -> Self {
        Self {
            width_pct: POPUP_WIDTH_PCT,
            max_width: POPUP_MAX_WIDTH,
            min_width: POPUP_MIN_WIDTH,
            v_margin: COMPACT_V_MARGIN,
        }
    }
}

/// Painted glyph width of the close control (` [✗] `).
pub const CLOSE_GLYPH_W: u16 = 5;

/// Geometry of the top-right close control.
///
/// Hit target runs from the glyph through the right border edge so a click on
/// the corner (not only the exact `✗` cell) still dismisses. Paint uses the
/// leading [`CLOSE_GLYPH_W`] cells only.
///
/// Shared by paint and mouse hit-testing so the glyph and the click target
/// never drift apart.
pub fn close_button_rect(modal: Rect) -> Option<Rect> {
    if modal.width < CLOSE_GLYPH_W + 2 {
        return None;
    }
    // Glyph starts 2 cells inset from the right; hit extends to the border.
    let x0 = modal.x + modal.width.saturating_sub(CLOSE_GLYPH_W + 2);
    let width = (modal.x + modal.width).saturating_sub(x0);
    Some(Rect {
        x: x0,
        y: modal.y,
        width,
        height: 1,
    })
}

/// Where a modal sits on the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogPlacement {
    /// Classic centered popup.
    Center,
    /// Docked to the bottom edge (Grok ask_user_question style).
    Bottom,
}

/// Paint a Grok Keyboard Shortcuts-style centered modal and return the content rect.
///
/// `shortcuts` are labels like `"Esc cancel"` / `"Enter select"` — the first
/// whitespace-separated token is the key (bold), the rest is the hint (dim),
/// joined with `  |  ` and centered on the footer row (wraps upward).
///
/// `mouse_pos` drives hover styling on the top-right `[✗]` control.
pub fn dialog_frame(
    frame: &mut Frame,
    title: &str,
    shortcuts: &[&str],
    palette: &ThemePalette,
    mouse_pos: Option<(u16, u16)>,
) -> DialogChrome {
    dialog_frame_sized(
        frame,
        title,
        shortcuts,
        palette,
        DialogSizing::popup(),
        mouse_pos,
        DialogPlacement::Center,
    )
}

/// Like [`dialog_frame`] with an explicit [`DialogSizing`].
pub fn dialog_frame_sized(
    frame: &mut Frame,
    title: &str,
    shortcuts: &[&str],
    palette: &ThemePalette,
    sizing: DialogSizing,
    mouse_pos: Option<(u16, u16)>,
    placement: DialogPlacement,
) -> DialogChrome {
    let area = frame.area();
    let dialog_area = match placement {
        DialogPlacement::Center => popup_rect(area, sizing),
        DialogPlacement::Bottom => {
            let mut r = popup_rect(area, sizing);
            r.y = area.y + area.height.saturating_sub(r.height);
            r
        }
    };
    paint_dialog_chrome(frame, title, shortcuts, palette, mouse_pos, dialog_area)
}

/// Like [`dialog_frame`] but with explicit placement (center or bottom dock)
/// and a percentage size (question panel).
#[allow(clippy::too_many_arguments)]
pub fn dialog_frame_placed(
    frame: &mut Frame,
    title: &str,
    shortcuts: &[&str],
    palette: &ThemePalette,
    percent_x: u16,
    percent_y: u16,
    mouse_pos: Option<(u16, u16)>,
    placement: DialogPlacement,
) -> DialogChrome {
    let area = frame.area();
    let dialog_area = match placement {
        DialogPlacement::Center => centered_rect(percent_x, percent_y, area),
        DialogPlacement::Bottom => bottom_rect(percent_x, percent_y, area),
    };
    paint_dialog_chrome(frame, title, shortcuts, palette, mouse_pos, dialog_area)
}

fn empty_chrome(dialog_area: Rect) -> DialogChrome {
    DialogChrome {
        content: dialog_area,
        modal: dialog_area,
        close_hit: None,
        inner_x: dialog_area.x,
        inner_width: dialog_area.width,
    }
}

fn paint_dialog_chrome(
    frame: &mut Frame,
    title: &str,
    shortcuts: &[&str],
    palette: &ThemePalette,
    mouse_pos: Option<(u16, u16)>,
    dialog_area: Rect,
) -> DialogChrome {
    if dialog_area.width < 12 || dialog_area.height < 5 {
        return empty_chrome(dialog_area);
    }

    // Clear drops RGB on Apple Terminal.app (Reset + 38;2 leak). Fill the
    // modal rect with themed spaces so every cell has an explicit bg/fg.
    crate::ui::layout::fill_blank(frame, dialog_area, palette.bg);

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

    let h_pad = inner_h_pad(dialog_area.width);
    let v_pad = inner_v_pad(dialog_area.height);
    let footer_width = inner.width.saturating_sub(h_pad.saturating_mul(2));
    let needed_footer = shortcuts_rows_needed(shortcuts, footer_width);
    let footer_h = if shortcuts.is_empty() {
        0
    } else {
        FOOTER_LINES.max(needed_footer)
    };
    let content = Rect {
        x: inner.x + h_pad,
        y: inner.y + v_pad,
        width: footer_width,
        height: inner.height.saturating_sub(v_pad + footer_h),
    };

    if footer_h > 0 {
        let footer = Rect {
            x: inner.x + h_pad,
            y: inner.y + inner.height.saturating_sub(footer_h),
            width: footer_width,
            height: footer_h,
        };
        paint_footer_shortcuts(frame, footer, shortcuts, palette);
    }

    DialogChrome {
        content,
        modal: dialog_area,
        close_hit,
        inner_x: inner.x,
        inner_width: inner.width,
    }
}

/// Grok `compute_modal_dims`: width = pct then `[min, max]` clamp; height
/// is the outer height minus `2 * v_margin`.
pub fn popup_rect(r: Rect, sizing: DialogSizing) -> Rect {
    if r.width == 0 || r.height == 0 {
        return Rect {
            x: r.x,
            y: r.y,
            width: 0,
            height: 0,
        };
    }
    let preferred = ((r.width as u32 * sizing.width_pct as u32) / 100) as u16;
    let w = preferred
        .min(sizing.max_width)
        .max(sizing.min_width)
        .min(r.width);
    let h = r
        .height
        .saturating_sub(sizing.v_margin.saturating_mul(2))
        .max(MIN_MODAL_HEIGHT.min(r.height))
        .min(r.height);
    Rect {
        x: r.x + r.width.saturating_sub(w) / 2,
        y: r.y + r.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    }
}

/// Create a centered rectangle as a percentage of `r`.
///
/// Grok Build (`agent_view/render.rs`): `max(percent of outer, min).min(outer)`.
/// On a split/phone PTY the percentage is a floor, not a cap, so 50% of 40
/// cols becomes 36 instead of a 20-col sliver.
pub fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let w = crate::tokens::layout::popup_dim(r.width, percent_x, MIN_MODAL_WIDTH);
    let h = crate::tokens::layout::popup_dim(r.height, percent_y, MIN_MODAL_HEIGHT);
    Rect {
        x: r.x + r.width.saturating_sub(w) / 2,
        y: r.y + r.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    }
}

/// Bottom-docked rectangle: full height fraction near the bottom edge,
/// horizontally centered by `percent_x`.
///
/// Used by the questionnaire panel so options sit above the prompt like
/// Grok's `ask_user_question` UI.
pub fn bottom_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let w = crate::tokens::layout::popup_dim(r.width, percent_x, MIN_MODAL_WIDTH);
    let h = crate::tokens::layout::popup_dim(r.height, percent_y, MIN_MODAL_HEIGHT.max(8));
    Rect {
        x: r.x + r.width.saturating_sub(w) / 2,
        y: r.y + r.height.saturating_sub(h),
        width: w,
        height: h,
    }
}

// ── private chrome ─────────────────────────────────────────────────────

fn paint_close_button(frame: &mut Frame, modal: Rect, palette: &ThemePalette, hovered: bool) {
    let Some(hit) = close_button_rect(modal) else {
        return;
    };
    // Painted glyph only; hit rect may extend past this to the border edge.
    let cells = [" ", "[", "✗", "]", " "];
    debug_assert_eq!(cells.len() as u16, CLOSE_GLYPH_W);
    let buf = frame.buffer_mut();
    // Grok: idle dim; hover brightens the mark (not error-red).
    let fg = if hovered { palette.fg } else { palette.dim };
    let style = Style::default()
        .fg(fg)
        .bg(palette.bg)
        .add_modifier(if hovered {
            Modifier::BOLD
        } else {
            Modifier::empty()
        });
    for (i, sym) in cells.iter().enumerate() {
        let x = hit.x + i as u16;
        if x >= hit.x.saturating_add(hit.width) {
            break;
        }
        if let Some(cell) = buf.cell_mut((x, hit.y)) {
            cell.set_symbol(sym);
            cell.set_style(style);
        }
    }
}

fn shortcuts_rows_needed(shortcuts: &[&str], width: u16) -> u16 {
    if width == 0 || shortcuts.is_empty() {
        return 0;
    }
    let avail = width as usize;
    let sep_w = UnicodeWidthStr::width("  |  ");
    let mut rows = 1u16;
    let mut cur = 0usize;
    for label in shortcuts {
        let label_w = UnicodeWidthStr::width(*label);
        let needed = if cur == 0 {
            label_w
        } else {
            cur + sep_w + label_w
        };
        if needed > avail && cur > 0 {
            rows += 1;
            cur = label_w;
        } else {
            cur = needed;
        }
    }
    rows
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
    let sep_w = UnicodeWidthStr::width(sep);
    let avail = area.width as usize;

    // Grok: greedy wrap, then bottom-align so a single row sits on the last line.
    let mut rows: Vec<Vec<usize>> = vec![vec![]];
    let mut cur = 0usize;
    for (idx, label) in shortcuts.iter().enumerate() {
        let label_w = UnicodeWidthStr::width(*label);
        let needed = if rows.last().is_some_and(|r| r.is_empty()) {
            label_w
        } else {
            cur + sep_w + label_w
        };
        if needed > avail && rows.last().is_some_and(|r| !r.is_empty()) {
            rows.push(vec![idx]);
            cur = label_w;
        } else if let Some(row) = rows.last_mut() {
            row.push(idx);
            cur = needed;
        }
    }
    rows.truncate(area.height as usize);

    let num_rows = rows.len() as u16;
    let buf = frame.buffer_mut();
    for (row_idx, indices) in rows.iter().enumerate() {
        let y = area.y + area.height - num_rows + row_idx as u16;
        let row_total: usize = indices
            .iter()
            .map(|&i| UnicodeWidthStr::width(shortcuts[i]))
            .sum::<usize>()
            + sep_w * indices.len().saturating_sub(1);
        let mut x = if row_total > avail {
            area.x
        } else {
            area.x + (area.width.saturating_sub(row_total as u16)) / 2
        };
        for (local, &i) in indices.iter().enumerate() {
            let (key, rest) = split_shortcut_label(shortcuts[i]);
            buf.set_stringn(
                x,
                y,
                key,
                area.x.saturating_add(area.width).saturating_sub(x) as usize,
                Style::default()
                    .fg(palette.fg)
                    .bg(palette.bg)
                    .add_modifier(Modifier::BOLD),
            );
            x = x.saturating_add(UnicodeWidthStr::width(key) as u16);
            if !rest.is_empty() {
                buf.set_stringn(
                    x,
                    y,
                    rest,
                    area.x.saturating_add(area.width).saturating_sub(x) as usize,
                    Style::default().fg(palette.dim).bg(palette.bg),
                );
                x = x.saturating_add(UnicodeWidthStr::width(rest) as u16);
            }
            if local + 1 < indices.len() {
                buf.set_stringn(
                    x,
                    y,
                    sep,
                    area.x.saturating_add(area.width).saturating_sub(x) as usize,
                    Style::default().fg(palette.dim).bg(palette.bg),
                );
                x = x.saturating_add(sep_w as u16);
            }
        }
    }
}

/// Full-width `─` hairline (Grok picker divider under the search bar).
pub fn paint_divider(frame: &mut Frame, x: u16, y: u16, width: u16, palette: &ThemePalette) {
    let style = Style::default().fg(palette.dim).bg(palette.bg);
    let buf = frame.buffer_mut();
    for i in 0..width {
        if let Some(cell) = buf.cell_mut((x + i, y)) {
            cell.set_char('\u{2500}');
            cell.set_style(style);
        }
    }
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
    fn popup_rect_matches_grok_shortcuts_sizing() {
        // 80-col: 70% is 56, inside [44, 80].
        let r = popup_rect(Rect::new(0, 0, 80, 40), DialogSizing::popup());
        assert_eq!(r.width, 56);
        assert_eq!(r.height, 32); // 40 - 2*4
        assert_eq!(r.x, 12);
        assert_eq!(r.y, 4);

        // Wide terminal: 70% of 120 is 84, capped at 80.
        let wide = popup_rect(Rect::new(0, 0, 120, 40), DialogSizing::popup());
        assert_eq!(wide.width, 80);
        assert_eq!(wide.x, 20);

        // Phone: min_width 44 exceeds 40 → fill the PTY.
        let phone = popup_rect(Rect::new(0, 0, 40, 24), DialogSizing::popup());
        assert_eq!(phone.width, 40);
        assert_eq!(phone.height, 16);
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
        assert_eq!(hit.height, 1);
        // Glyph starts 2 inset; hit extends to the right border edge.
        assert_eq!(hit.x, modal.x + modal.width - CLOSE_GLYPH_W - 2);
        assert_eq!(hit.x + hit.width, modal.x + modal.width);
        assert!(hit.width >= CLOSE_GLYPH_W);
    }
}
