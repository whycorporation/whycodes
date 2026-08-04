//! Draw toasts in the top-right corner.
//!
//! Over the chat rather than beside it: taking layout space for something that
//! is usually absent would move the whole view every time one appeared.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::theme::ThemePalette;
use crate::toast::{Toast, ToastKind};

/// Widest a toast may get, before the terminal's own width is considered.
const MAX_WIDTH: u16 = 48;
/// Gap from the right and top edges.
const MARGIN: u16 = 2;

fn color(kind: ToastKind, palette: &ThemePalette) -> ratatui::style::Color {
    match kind {
        ToastKind::Info => palette.info,
        ToastKind::Success => palette.success,
        ToastKind::Warning => palette.warning,
        ToastKind::Error => palette.error,
    }
}

/// Render `toasts` stacked downward from the top-right of `area`.
pub fn render(frame: &mut Frame, area: Rect, toasts: &[Toast], palette: &ThemePalette) {
    if toasts.is_empty() || area.width < 16 || area.height < 4 {
        return;
    }

    let max_w = area.width.saturating_sub(MARGIN * 2).max(1);
    let width = MAX_WIDTH.min(max_w).max(16).min(area.width);
    let inner = width.saturating_sub(4) as usize;
    let mut top = area.y + MARGIN.min(area.height.saturating_sub(1));

    for toast in toasts {
        let body = wrap(&toast.message, inner, 2);
        let height = body.len() as u16 + 2;
        if top + height > area.y + area.height {
            break;
        }

        let x = area.x + area.width.saturating_sub(width + MARGIN);
        if x < area.x {
            break;
        }
        let rect = Rect {
            x,
            y: top,
            width,
            height,
        };

        let accent = color(toast.kind, palette);
        let lines: Vec<Line> = body
            .into_iter()
            .enumerate()
            .map(|(i, text)| {
                let mut spans = Vec::new();
                if i == 0 {
                    spans.push(Span::styled(
                        format!("{} ", toast.kind.glyph()),
                        Style::default().fg(accent).add_modifier(Modifier::BOLD),
                    ));
                } else {
                    spans.push(Span::raw("  "));
                }
                spans.push(Span::styled(text, Style::default().fg(palette.fg)));
                Line::from(spans)
            })
            .collect();

        // Clear first: this draws over the chat, and without it the text
        // underneath shows through the gaps.
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(accent))
                    .style(Style::default().bg(palette.dialog_bg)),
            ),
            rect,
        );

        top += height;
    }
}

/// Break `text` to `width`, at most `max_lines`, marking truncation.
fn wrap(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        if current.is_empty() {
            current = word.to_string();
        } else if current.chars().count() + 1 + word.chars().count() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            current = word.to_string();
        }
        if lines.len() == max_lines {
            break;
        }
    }
    if lines.len() < max_lines && !current.is_empty() {
        lines.push(current);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }
    // A word longer than the line still has to fit somewhere.
    for line in &mut lines {
        if line.chars().count() > width {
            *line = line
                .chars()
                .take(width.saturating_sub(1))
                .collect::<String>()
                + "…";
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_at_word_boundaries() {
        assert_eq!(wrap("one two three", 7, 3), vec!["one two", "three"]);
    }

    #[test]
    fn stops_at_the_line_limit() {
        let out = wrap("a b c d e f g h i j k l", 3, 2);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn truncates_a_word_too_long_to_fit() {
        let out = wrap("supercalifragilistic", 10, 1);
        assert_eq!(out.len(), 1);
        assert!(out[0].ends_with('…'));
        assert_eq!(out[0].chars().count(), 10);
    }

    #[test]
    fn empty_text_still_produces_a_line() {
        assert_eq!(wrap("", 10, 2), vec![String::new()]);
        assert_eq!(wrap("x", 0, 2), vec![String::new()]);
    }

    #[test]
    fn each_kind_maps_to_its_own_palette_colour() {
        let p = crate::theme::ThemeName::DefaultDark.palette();
        assert_eq!(color(ToastKind::Error, &p), p.error);
        assert_eq!(color(ToastKind::Warning, &p), p.warning);
        assert_eq!(color(ToastKind::Success, &p), p.success);
        assert_eq!(color(ToastKind::Info, &p), p.info);
    }
}
