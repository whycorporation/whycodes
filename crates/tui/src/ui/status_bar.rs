//! Right-aligned composable status bar (Grok `AgentStatusBar`).
//!
//! Collect items as styled lines, lay them out right-aligned with dim `│`
//! separators, and return hit-test areas keyed by item id.
//!
//! Paints only the right cluster — leaves the left side of `area` alone so
//! callers can render branch/path first.

use std::collections::HashMap;

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};
use unicode_width::UnicodeWidthStr;

/// Separator between status items (Grok).
pub const SEPARATOR: &str = "│";

struct StatusEntry {
    id: &'static str,
    line: Line<'static>,
    width: u16,
}

/// Builder for a right-aligned status strip.
pub struct StatusBar {
    items: Vec<StatusEntry>,
    sep_style: Style,
}

impl StatusBar {
    pub fn new(sep_style: Style) -> Self {
        Self {
            items: Vec::new(),
            sep_style,
        }
    }

    /// Push a named item (left-to-right within the right-aligned group).
    pub fn push(&mut self, id: &'static str, line: Line<'static>) {
        let width = line.width() as u16;
        self.items.push(StatusEntry { id, line, width });
    }

    /// Total display width including separators between items.
    pub fn total_width(&self) -> u16 {
        if self.items.is_empty() {
            return 0;
        }
        let items: u16 = self.items.iter().map(|e| e.width).sum();
        let seps = (self.items.len() as u16).saturating_sub(1) * 3; // ` │ `
        items + seps
    }

    /// Paint right-aligned into the right edge of `area` (single row).
    /// Returns id → absolute screen rect.
    pub fn render(self, frame: &mut Frame, area: Rect) -> HashMap<&'static str, Rect> {
        let mut areas = HashMap::new();
        if area.height == 0 || area.width == 0 || self.items.is_empty() {
            return areas;
        }

        let total = self.total_width().min(area.width);
        let start_x = area.x.saturating_add(area.width.saturating_sub(total));
        let paint = Rect {
            x: start_x,
            y: area.y,
            width: total,
            height: 1,
        };

        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut x = start_x;
        for (i, entry) in self.items.into_iter().enumerate() {
            if i > 0 {
                let sep = format!(" {SEPARATOR} ");
                let sw = sep.width() as u16;
                spans.push(Span::styled(sep, self.sep_style));
                x = x.saturating_add(sw);
            }
            let w = entry.width;
            for s in entry.line.spans {
                spans.push(s);
            }
            areas.insert(
                entry.id,
                Rect {
                    x,
                    y: area.y,
                    width: w,
                    height: 1,
                },
            );
            x = x.saturating_add(w);
        }

        frame.render_widget(Paragraph::new(Line::from(spans)), paint);
        areas
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn total_width_one_and_two_items() {
        let mut bar = StatusBar::new(Style::default());
        bar.push("a", Line::from("AA"));
        assert_eq!(bar.total_width(), 2);

        let mut bar = StatusBar::new(Style::default());
        bar.push("a", Line::from("AA"));
        bar.push("b", Line::from("BBB"));
        assert_eq!(bar.total_width(), 8);
    }

    #[test]
    fn render_places_context_on_right() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let backend = TestBackend::new(20, 1);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| {
            let mut bar = StatusBar::new(Style::default().fg(Color::DarkGray));
            bar.push("context", Line::from("1.2k / 200k"));
            let areas = bar.render(f, f.area());
            let r = areas.get("context").expect("context hit");
            assert_eq!(r.width, 11);
            assert_eq!(r.x + r.width, 20);
        })
        .unwrap();
    }
}
