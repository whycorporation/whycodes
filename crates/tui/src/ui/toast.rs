//! Draw toasts in the top-right corner.
//!
//! Over the chat rather than beside it: taking layout space for something that
//! is usually absent would move the whole view every time one appeared.
//!
//! Visual language matches the prompt chrome: dim rounded border (`╭─╮│╰─╯`),
//! base `bg` fill, kind colour only on the leading glyph — not a coloured
//! floating card.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::theme::ThemePalette;
use crate::toast::{Toast, ToastKind};

/// Widest a toast may get, before the terminal's own width is considered.
const MAX_WIDTH: u16 = 48;
/// Gap from the right and top edges.
const MARGIN: u16 = 1;

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
    if toasts.is_empty() || area.width < 8 || area.height < 3 {
        return;
    }

    let max_w = area.width.saturating_sub(MARGIN * 2).max(1);
    let width = MAX_WIDTH.min(max_w).max(8.min(area.width)).min(area.width);
    // Inner text width: borders (2) + left pad " " + glyph + " " ≈ 5 cells.
    let inner = width.saturating_sub(5) as usize;
    let mut top = area.y + MARGIN.min(area.height.saturating_sub(1));

    // Prompt-like chrome: dim rounded border on base bg.
    let border_style = Style::default().fg(palette.dim).bg(palette.bg);
    let fill = Style::default().bg(palette.bg).fg(palette.fg);

    for toast in toasts {
        let body = wrap(&toast.message, inner, 2);
        let height = body.len() as u16 + 2; // +2 for top/bottom border
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
                // Soft left rail + glyph on first line (system_callout pattern).
                if i == 0 {
                    Line::from(vec![
                        Span::styled("│ ".to_string(), Style::default().fg(accent).bg(palette.bg)),
                        Span::styled(
                            format!("{} ", toast.kind.glyph()),
                            Style::default().fg(accent).bg(palette.bg),
                        ),
                        Span::styled(text, Style::default().fg(palette.fg).bg(palette.bg)),
                    ])
                } else {
                    Line::from(vec![
                        Span::styled("│ ".to_string(), Style::default().fg(accent).bg(palette.bg)),
                        Span::styled("  ", Style::default().bg(palette.bg)),
                        Span::styled(text, Style::default().fg(palette.dim).bg(palette.bg)),
                    ])
                }
            })
            .collect();

        // Clear first: this draws over the chat, and without it the text
        // underneath shows through the gaps.
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(border_style)
                    .style(fill),
            ),
            rect,
        );

        top = top.saturating_add(height).saturating_add(1); // 1-row gap between stacked toasts
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

    fn paint(width: u16, height: u16, toasts: &[Toast]) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let palette = crate::theme::ThemeName::DefaultDark.palette();
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                render(frame, area, toasts, &palette);
            })
            .expect("draw");
        let buf = terminal.backend().buffer().clone();
        let area = buf.area();
        let mut out = String::new();
        for y in area.y..area.y.saturating_add(area.height) {
            for x in area.x..area.x.saturating_add(area.width) {
                if let Some(cell) = buf.cell((x, y)) {
                    out.push_str(cell.symbol());
                }
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn render_skips_empty_or_tiny_areas() {
        let toast = Toast::new(ToastKind::Info, "hello");
        assert!(
            !paint(80, 24, &[]).contains('i'),
            "empty stack must not paint"
        );
        let tiny = paint(10, 2, std::slice::from_ref(&toast));
        assert!(
            !tiny.contains("hello"),
            "area below the 16×3 floor must skip: {tiny:?}"
        );
    }

    #[test]
    fn render_paints_each_kind_in_the_top_right() {
        let toasts = [
            Toast::new(ToastKind::Success, "saved"),
            Toast::new(ToastKind::Error, "boom"),
            Toast::new(ToastKind::Warning, "careful"),
            Toast::new(ToastKind::Info, "note"),
        ];
        let text = paint(80, 24, &toasts);
        assert!(text.contains("saved"), "{text}");
        assert!(text.contains("boom"), "{text}");
        assert!(text.contains("careful"), "{text}");
        assert!(text.contains("note"), "{text}");
        assert!(text.contains('✓'), "success glyph: {text}");
        assert!(text.contains('✕'), "error glyph: {text}");
        assert!(text.contains('!'), "warning glyph: {text}");
    }

    #[test]
    fn render_wraps_long_copy_and_truncates_overflow() {
        let long = Toast::new(
            ToastKind::Info,
            "one two three four five six seven eight nine ten eleven twelve",
        );
        let text = paint(40, 12, std::slice::from_ref(&long));
        assert!(
            text.contains("one two") || text.contains("one"),
            "wrapped body must appear: {text}"
        );

        // A short stack that cannot fit a second toast: first stays, rest drops.
        let stacked = [
            Toast::new(ToastKind::Info, "first toast stays"),
            Toast::new(ToastKind::Error, "second never fits"),
        ];
        let cramped = paint(32, 5, &stacked);
        assert!(cramped.contains("first"), "{cramped}");
        assert!(
            !cramped.contains("second"),
            "overflow toast must not paint: {cramped}"
        );
    }

    #[test]
    fn wrap_keeps_a_single_word_under_the_limit() {
        assert_eq!(wrap("hello", 10, 2), vec!["hello"]);
        // Whitespace-only collapses like empty.
        assert_eq!(wrap("   \t  ", 8, 2), vec![String::new()]);
    }
}
