//! Shared list picker dialog (model, session, workspace, …).
//!
//! One component for title + rows + cursor + footer keeps highlight style
//! consistent. Chrome is drawn via [`dialog_frame`]; overflow gets a solid
//! scrollbar on the right edge.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use unicode_width::UnicodeWidthStr;

use super::base::dialog_frame;
use crate::theme::ThemePalette;
use crate::ui::scrollbar::{ScrollbarColors, elevate, paint_scrollbar, scroll_to_selected};

/// Grok picker leaf mark (`diamond_filled`).
const DIAMOND: &str = "◆ ";
/// Selected-row wash — Grok `bg_visual` ≈ +44 on the canvas.
const SELECTED_LIFT: u8 = 44;

/// One row: what to show, and an optional dimmed detail after it.
pub struct SelectItem {
    pub label: String,
    pub detail: Option<String>,
}

impl SelectItem {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            detail: None,
        }
    }

    pub fn with_detail(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            detail: Some(detail.into()),
        }
    }
}

/// Hit-test metadata written during paint for mouse wheel / click handling.
#[derive(Debug, Clone, Copy, Default)]
pub struct SelectPaintInfo {
    pub close_hit: Option<Rect>,
    /// Full modal rect (border inclusive) — selection/copy is clipped here.
    pub modal: Option<Rect>,
    /// Rows of the list (not including the scrollbar gutter).
    pub list_area: Option<Rect>,
    /// Scrollbar track (when the list overflows); drag / click to scroll.
    pub scrollbar_hit: Option<Rect>,
    /// First visible item index.
    pub scroll_start: usize,
    /// How many rows fit in the viewport.
    pub visible: usize,
    /// Total items (for clamping click indices).
    pub total: usize,
}

/// Render a select dialog.
///
/// `empty` is shown in place of the list when there is nothing to choose. A
/// picker with no rows and no explanation looks broken, and the reason is
/// always specific — no providers configured, no sessions yet.
pub fn render_select(
    frame: &mut Frame,
    title: &str,
    items: &[SelectItem],
    selected: usize,
    empty: &str,
    palette: &ThemePalette,
    mouse_pos: Option<(u16, u16)>,
) -> SelectPaintInfo {
    let chrome = dialog_frame(
        frame,
        title,
        &["↑/↓ nav", "Enter select", "Esc close"],
        palette,
        mouse_pos,
    );
    let area = chrome.content;
    if area.width == 0 || area.height == 0 {
        return SelectPaintInfo {
            close_hit: chrome.close_hit,
            modal: Some(chrome.modal),
            ..Default::default()
        };
    }

    let total = items.len();
    let visible = (area.height as usize).max(1);
    let needs_scrollbar = total > visible;
    let list_width = if needs_scrollbar {
        area.width.saturating_sub(1)
    } else {
        area.width
    };
    let list_area = Rect {
        x: area.x,
        y: area.y,
        width: list_width,
        height: area.height,
    };

    let start = scroll_to_selected(selected, total, visible);

    if items.is_empty() {
        let row = Rect {
            x: list_area.x,
            y: list_area.y,
            width: list_area.width,
            height: 1,
        };
        paint_picker_row(
            frame.buffer_mut(),
            row,
            empty,
            None,
            false,
            palette,
            /* dimmed */ true,
        );
    } else {
        for (row_i, (i, item)) in items
            .iter()
            .enumerate()
            .skip(start)
            .take(visible)
            .enumerate()
        {
            let row = Rect {
                x: list_area.x,
                y: list_area.y + row_i as u16,
                width: list_area.width,
                height: 1,
            };
            paint_picker_row(
                frame.buffer_mut(),
                row,
                &item.label,
                item.detail.as_deref(),
                i == selected,
                palette,
                false,
            );
        }
    }

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
            visible,
            start,
            colors.track,
            colors.thumb,
        );
        Some(sb)
    } else {
        None
    };

    SelectPaintInfo {
        close_hit: chrome.close_hit,
        modal: Some(chrome.modal),
        list_area: Some(list_area),
        scrollbar_hit,
        scroll_start: start,
        visible,
        total,
    }
}

/// Grok Keyboard Shortcuts / picker row: `◆ label` left, optional detail
/// right-aligned. Selected row is a `bg_visual` wash + bold primary text —
/// no `▸` caret (selection is the wash).
pub fn paint_picker_row(
    buf: &mut Buffer,
    row: Rect,
    label: &str,
    detail: Option<&str>,
    selected: bool,
    palette: &ThemePalette,
    dimmed: bool,
) {
    if row.width == 0 || row.height == 0 {
        return;
    }
    let bg = if selected {
        elevate(palette.bg, SELECTED_LIFT)
    } else {
        palette.bg
    };
    let fill = Style::default().bg(bg);
    for x in row.x..row.x.saturating_add(row.width) {
        if let Some(cell) = buf.cell_mut((x, row.y)) {
            cell.set_symbol(" ");
            cell.set_style(fill);
        }
    }

    let label_style = if dimmed {
        Style::default().fg(palette.dim).bg(bg)
    } else if selected {
        Style::default()
            .fg(palette.fg)
            .bg(bg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(palette.fg).bg(bg)
    };
    let mark_style = Style::default().fg(palette.dim).bg(bg);
    let detail_style = Style::default().fg(palette.dim).bg(bg);

    let mark_w = UnicodeWidthStr::width(DIAMOND) as u16;
    buf.set_stringn(
        row.x,
        row.y,
        DIAMOND,
        row.width as usize,
        if dimmed { label_style } else { mark_style },
    );

    let label_x = row.x.saturating_add(mark_w);
    let trailing = 1u16;
    let after_label = row
        .x
        .saturating_add(row.width)
        .saturating_sub(trailing)
        .saturating_sub(label_x);
    let detail_text = detail.unwrap_or("");
    let detail_w = if detail_text.is_empty() {
        0u16
    } else {
        let cap = (after_label / 2).max(1);
        (UnicodeWidthStr::width(detail_text) as u16).min(cap)
    };
    let gap = if detail_w > 0 { 2u16 } else { 0 };
    let label_budget = after_label.saturating_sub(detail_w + gap) as usize;
    buf.set_stringn(label_x, row.y, label, label_budget, label_style);

    if detail_w > 0 {
        let right_x = row
            .x
            .saturating_add(row.width)
            .saturating_sub(detail_w + trailing);
        buf.set_stringn(right_x, row.y, detail_text, detail_w as usize, detail_style);
    }
}

/// Grok section header: `── Label ──` in dim, not selectable.
pub fn paint_section_header(buf: &mut Buffer, row: Rect, label: &str, palette: &ThemePalette) {
    if row.width == 0 || row.height == 0 {
        return;
    }
    let style = Style::default().fg(palette.dim).bg(palette.bg);
    for x in row.x..row.x.saturating_add(row.width) {
        if let Some(cell) = buf.cell_mut((x, row.y)) {
            cell.set_symbol(" ");
            cell.set_style(style);
        }
    }
    let text = format!("── {label} ──");
    buf.set_stringn(row.x, row.y, &text, row.width as usize, style);
}

/// Grok picker search bar (` / to search` / ` search: query` + inverse cursor).
pub fn paint_search_bar(
    frame: &mut Frame,
    area: Rect,
    query: &str,
    searching: bool,
    palette: &ThemePalette,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let row = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    let style = Style::default().fg(palette.dim).bg(palette.bg);
    let buf = frame.buffer_mut();
    for x in row.x..row.x.saturating_add(row.width) {
        if let Some(cell) = buf.cell_mut((x, row.y)) {
            cell.set_symbol(" ");
            cell.set_style(style);
        }
    }
    if searching || !query.is_empty() {
        let text = format!(" search: {query}");
        buf.set_stringn(row.x, row.y, &text, row.width as usize, style);
        let cursor_col = (text.chars().count() as u16).min(row.width.saturating_sub(1));
        if let Some(cell) = buf.cell_mut((row.x + cursor_col, row.y)) {
            cell.set_style(Style::default().fg(palette.bg).bg(palette.fg));
        }
    } else {
        buf.set_stringn(row.x, row.y, " / to search", row.width as usize, style);
    }
}

/// Collapsible provider header: `▾ name (N)` expanded, `▸ name (N)` collapsed.
pub fn paint_group_header(
    buf: &mut Buffer,
    row: Rect,
    name: &str,
    count: usize,
    collapsed: bool,
    selected: bool,
    palette: &ThemePalette,
) {
    let chevron = if collapsed { "▸" } else { "▾" };
    let label = format!("{chevron} {name} ({count})");
    paint_picker_row(buf, row, &label, None, selected, palette, false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_item_can_carry_a_detail() {
        let plain = SelectItem::new("a");
        assert_eq!(plain.label, "a");
        assert!(plain.detail.is_none());

        let detailed = SelectItem::with_detail("a", "b");
        assert_eq!(detailed.detail.as_deref(), Some("b"));
    }
}
