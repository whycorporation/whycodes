//! A list dialog with a moving selection.
//!
//! opencode builds every picker — model, session, workspace, account — from one
//! `dialog-select` component rather than writing each from scratch. The same
//! applies here: a picker is a title, a list of rows, a cursor and a footer, and
//! a second copy of that is a second place for the highlight style to drift.
//!
//! Chrome matches Grok `ModalWindow` (via [`dialog_frame`]).
//! When the list overflows the content area, a solid scrollbar is painted on
//! the right edge.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::base::dialog_frame;
use crate::theme::ThemePalette;
use crate::ui::scrollbar::{ScrollbarColors, paint_scrollbar, scroll_to_selected};

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
        &["↑/↓ / wheel", "Enter select", "Esc / [✗]"],
        palette,
        60,
        60,
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

    let mut lines: Vec<Line> = Vec::new();

    if items.is_empty() {
        lines.push(Line::from(Span::styled(
            format!(" {empty}"),
            Style::default().fg(palette.dim),
        )));
    } else {
        for (i, item) in items.iter().enumerate().skip(start).take(visible) {
            let current = i == selected;
            // Grok/fzf: selected row recolors text with accent, no full wash.
            let mut spans = vec![
                Span::styled(
                    if current { " ▸ " } else { "   " }.to_string(),
                    Style::default().fg(palette.accent),
                ),
                Span::styled(
                    item.label.clone(),
                    if current {
                        Style::default()
                            .fg(palette.accent)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(palette.fg)
                    },
                ),
            ];
            if let Some(detail) = &item.detail {
                spans.push(Span::styled(
                    format!("  {detail}"),
                    Style::default().fg(palette.dim),
                ));
            }
            lines.push(Line::from(spans));
        }
    }

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(palette.bg)),
        list_area,
    );

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
