//! A list dialog with a moving selection.
//!
//! opencode builds every picker — model, session, workspace, account — from one
//! `dialog-select` component rather than writing each from scratch. The same
//! applies here: a picker is a title, a list of rows, a cursor and a footer, and
//! a second copy of that is a second place for the highlight style to drift.
//!
//! Chrome matches Grok `ModalWindow` (via [`dialog_frame`]).

use ratatui::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::base::dialog_frame;
use crate::theme::ThemePalette;

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
) {
    let chrome = dialog_frame(
        frame,
        title,
        &["↑/↓ move", "Enter select", "Esc cancel"],
        palette,
        60,
        60,
    );
    let area = chrome.content;
    if area.width == 0 || area.height == 0 {
        return;
    }

    // Keep the cursor on screen for lists longer than the dialog.
    let rows = (area.height as usize).max(1);
    let start = selected
        .saturating_sub(rows.saturating_sub(1))
        .min(items.len().saturating_sub(rows.min(items.len())));

    let mut lines: Vec<Line> = Vec::new();

    if items.is_empty() {
        lines.push(Line::from(Span::styled(
            format!(" {empty}"),
            Style::default().fg(palette.dim),
        )));
    } else {
        for (i, item) in items.iter().enumerate().skip(start).take(rows) {
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

        if items.len() > rows {
            lines.push(Line::from(Span::styled(
                format!("   … {} of {}", selected + 1, items.len()),
                Style::default().fg(palette.dim),
            )));
        }
    }

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(palette.bg)),
        area,
    );
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
