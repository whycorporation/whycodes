//! Sticky todo panel under the header (Grok Build-style).
//!
//! Header is always one row (`▸`/`▾ Todos  done/total` plus a compact
//! progress track). Click the header (or `t` in scrollback) to fold.
//! Finished items stay in the list so they can be reopened after the
//! panel auto-collapses when everything is done.
//!
//! Glyphs and per-status colors follow Grok's `TodoPane`: hollow square,
//! play triangle, check mark, ballot X — icon color separate from text.

use crate::app::TuiApp;
use crate::theme::ThemePalette;
use crate::ui::progress_bar::progress_bar_string;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
};
use unicode_width::UnicodeWidthStr;
use whycodes_core::todo::{TodoItem, TodoStatus, all_terminal, terminal_count};

/// Max item rows (not counting the header or overflow line).
pub const MAX_ITEMS: usize = 8;

/// Compact track in the header (Grok context-bar cells).
const HEADER_BAR_CELLS: u16 = 8;

/// True when the sticky panel should paint only the header row.
pub fn is_collapsed(app: &TuiApp) -> bool {
    app.todos_collapsed
}

/// Rows reserved under the header/subagent strip when todos exist.
pub fn panel_height(app: &TuiApp, body_h: u16) -> u16 {
    if app.todos.is_empty() {
        return 0;
    }
    let max = body_h.saturating_sub(3);
    if max == 0 {
        return 0;
    }
    let want = if is_collapsed(app) {
        1
    } else {
        let shown = app.todos.len().min(MAX_ITEMS);
        let extra = usize::from(app.todos.len() > MAX_ITEMS);
        (1 + shown + extra) as u16
    };
    want.min(max)
}

/// Grok `TodoPane` status glyph (not the ballot-box marks in `TodoStatus::mark`).
pub fn status_glyph(status: TodoStatus) -> &'static str {
    match status {
        TodoStatus::Pending => "\u{25a1}",    // □
        TodoStatus::InProgress => "\u{25b6}", // ▶
        TodoStatus::Completed => "\u{2713}",  // ✓
        TodoStatus::Cancelled => "\u{2717}",  // ✗
    }
}

pub fn icon_style(status: TodoStatus, palette: &ThemePalette) -> Style {
    match status {
        TodoStatus::Pending => Style::default().fg(palette.fg),
        TodoStatus::InProgress => Style::default()
            .fg(palette.warning)
            .add_modifier(Modifier::BOLD),
        TodoStatus::Completed => Style::default().fg(palette.success),
        TodoStatus::Cancelled => Style::default().fg(palette.error),
    }
}

pub fn text_style(status: TodoStatus, palette: &ThemePalette) -> Style {
    match status {
        TodoStatus::Pending => Style::default().fg(palette.fg),
        TodoStatus::InProgress => Style::default().fg(palette.fg).add_modifier(Modifier::BOLD),
        TodoStatus::Completed => Style::default().fg(palette.dim),
        TodoStatus::Cancelled => Style::default()
            .fg(palette.dim)
            .add_modifier(Modifier::CROSSED_OUT),
    }
}

/// Combined style (sidebar / tests that only need one color).
pub fn status_style(status: TodoStatus, palette: &ThemePalette) -> Style {
    text_style(status, palette)
}

pub fn item_line(item: &TodoItem, palette: &ThemePalette) -> Line<'static> {
    item_line_width(item, palette, 80)
}

fn item_line_width(item: &TodoItem, palette: &ThemePalette, width: u16) -> Line<'static> {
    let glyph = status_glyph(item.status);
    // Chevron + space is 2 columns; items use the same indent so the glyph
    // column lines up with the header label (Grok tasks-pane rule).
    let indent = "  ";
    let prefix_cols = indent.width() + glyph.width() + 1;
    let max = (width as usize).saturating_sub(prefix_cols).max(8);
    let content = truncate_chars(first_line(&item.content), max);
    Line::from(vec![
        Span::raw(indent),
        Span::styled(format!("{glyph} "), icon_style(item.status, palette)),
        Span::styled(content, text_style(item.status, palette)),
    ])
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("").trim()
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

fn header_line(app: &TuiApp, palette: &ThemePalette, width: u16) -> Line<'static> {
    let total = app.todos.len();
    let done = terminal_count(&app.todos);
    let collapsed = is_collapsed(app);
    let all_done = all_terminal(&app.todos);
    let chevron = if collapsed { "\u{25b8} " } else { "\u{25be} " };
    let counts = if all_done {
        format!("{done}/{total} done")
    } else {
        format!("{done}/{total}")
    };

    let mut chevron_style = Style::default().fg(palette.dim);
    let mut label_style = Style::default().fg(palette.fg).add_modifier(Modifier::BOLD);
    let mut count_style = if all_done {
        Style::default()
            .fg(palette.success)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(palette.dim)
    };
    if app.todos_hit.hovered {
        chevron_style = chevron_style
            .fg(palette.fg)
            .add_modifier(Modifier::UNDERLINED);
        label_style = label_style.add_modifier(Modifier::UNDERLINED);
        count_style = count_style.add_modifier(Modifier::UNDERLINED);
    }

    let mut spans = vec![
        Span::styled(chevron, chevron_style),
        Span::styled("Todos", label_style),
        Span::styled(format!("  {counts}"), count_style),
    ];

    let used: usize = spans.iter().map(|s| s.content.width()).sum();
    let bar_w = HEADER_BAR_CELLS;
    // "  " + bar
    if total > 0 && width as usize >= used + 2 + bar_w as usize {
        let frac = done as f64 / total as f64;
        let bar = progress_bar_string(bar_w, frac);
        let pad = (width as usize).saturating_sub(used + 1 + bar_w as usize);
        spans.push(Span::raw(" ".repeat(pad.max(1))));
        let bar_color = if all_done {
            palette.success
        } else if app.todos.iter().any(|t| t.status == TodoStatus::InProgress) {
            palette.warning
        } else {
            palette.accent
        };
        spans.push(Span::styled(bar, Style::default().fg(bar_color)));
    }

    Line::from(spans)
}

pub fn render_panel(frame: &mut Frame, area: Rect, app: &mut TuiApp, palette: &ThemePalette) {
    if area.height == 0 || app.todos.is_empty() {
        app.todos_hit.clear();
        return;
    }
    let collapsed = is_collapsed(app) || area.height == 1;

    let mut lines: Vec<Line> = Vec::new();
    lines.push(header_line(app, palette, area.width));

    if !collapsed {
        let mut budget = area.height.saturating_sub(1) as usize;
        let overflow = app.todos.len() > MAX_ITEMS.min(budget);
        if overflow {
            budget = budget.saturating_sub(1);
        }
        let take = budget.min(MAX_ITEMS).min(app.todos.len());
        for item in app.todos.iter().take(take) {
            lines.push(item_line_width(item, palette, area.width));
        }
        let hidden = app.todos.len().saturating_sub(take);
        if hidden > 0 {
            lines.push(Line::from(Span::styled(
                format!("  … +{hidden} more"),
                Style::default().fg(palette.dim),
            )));
        }
    }

    // Header row is the click target (Grok HitArea: set after paint).
    let header = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    app.todos_hit.set_rect(Some(header));

    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::default().bg(palette.status_bar_bg)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TuiAppConfig;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use whycodes_core::todo::{TodoItem, TodoStatus};

    fn paint(app: &mut TuiApp, w: u16, h: u16) -> String {
        let palette = app.config.palette();
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).expect("term");
        terminal
            .draw(|f| render_panel(f, f.area(), app, &palette))
            .expect("draw");
        let buf = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buf.area().height {
            for x in 0..buf.area().width {
                if let Some(c) = buf.cell((x, y)) {
                    out.push_str(c.symbol());
                }
            }
            out.push('\n');
        }
        out
    }

    fn item(id: &str, content: &str, status: TodoStatus) -> TodoItem {
        TodoItem::new(id, content, status)
    }

    #[test]
    fn hidden_when_empty() {
        let mut app = TuiApp::new(TuiAppConfig::default());
        assert_eq!(panel_height(&app, 24), 0);
        let text = paint(&mut app, 40, 2);
        assert!(!text.contains("Todos"), "{text}");
        assert!(app.todos_hit.rect.is_none());
    }

    #[test]
    fn shows_marks_and_counts() {
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.replace_todos(vec![
            item("a", "pending one", TodoStatus::Pending),
            item("b", "working now", TodoStatus::InProgress),
            item("c", "done item", TodoStatus::Completed),
            item("d", "skipped", TodoStatus::Cancelled),
        ]);
        assert!(!app.todos_collapsed);
        assert_eq!(panel_height(&app, 24), 5);
        let text = paint(&mut app, 60, 6);
        assert!(text.contains("▾ Todos  2/4"), "{text}");
        assert!(text.contains("□ pending one"), "{text}");
        assert!(text.contains("▶ working now"), "{text}");
        assert!(text.contains("✓ done item"), "{text}");
        assert!(text.contains("✗ skipped"), "{text}");
        assert!(
            text.contains('░') || text.contains('█'),
            "progress track: {text}"
        );
        assert!(app.todos_hit.rect.is_some());
    }

    #[test]
    fn all_done_auto_collapses_and_can_reopen() {
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.replace_todos(vec![
            item("a", "one", TodoStatus::Completed),
            item("b", "two", TodoStatus::Cancelled),
        ]);
        assert!(app.todos_collapsed);
        assert_eq!(panel_height(&app, 24), 1);
        let text = paint(&mut app, 40, 1);
        assert!(text.contains("▸ Todos  2/2 done"), "{text}");
        assert!(!text.contains('□'), "{text}");
        assert!(!text.contains("✓ one"), "{text}");
        assert!(!text.contains("✗ two"), "{text}");

        app.toggle_todos_panel();
        assert!(!app.todos_collapsed);
        assert_eq!(panel_height(&app, 24), 3);
        let text = paint(&mut app, 40, 4);
        assert!(text.contains("▾ Todos  2/2 done"), "{text}");
        assert!(text.contains("✓ one"), "{text}");
        assert!(text.contains("✗ two"), "{text}");
    }

    #[test]
    fn overflow_line_when_many() {
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.replace_todos(
            (0..10)
                .map(|i| item(&i.to_string(), &format!("item {i}"), TodoStatus::Pending))
                .collect(),
        );
        assert_eq!(panel_height(&app, 24), 10); // header + 8 + overflow
        let text = paint(&mut app, 40, 12);
        assert!(text.contains("+2 more"), "{text}");
        assert!(text.contains("item 0"), "{text}");
        assert!(!text.contains("item 9"), "{text}");
    }

    #[test]
    fn tiny_body_hides_panel() {
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.replace_todos(vec![item("a", "x", TodoStatus::Pending)]);
        assert_eq!(panel_height(&app, 3), 0);
        assert_eq!(panel_height(&app, 0), 0);
    }

    #[test]
    fn user_collapse_hides_items_while_work_remains() {
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.replace_todos(vec![item("a", "still open", TodoStatus::Pending)]);
        app.toggle_todos_panel();
        assert!(app.todos_collapsed);
        assert_eq!(panel_height(&app, 24), 1);
        let text = paint(&mut app, 40, 2);
        assert!(text.contains("▸ Todos  0/1"), "{text}");
        assert!(!text.contains("still open"), "{text}");
    }

    #[test]
    fn new_open_work_unfolds_after_all_done() {
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.replace_todos(vec![item("a", "one", TodoStatus::Completed)]);
        assert!(app.todos_collapsed);
        app.replace_todos(vec![
            item("a", "one", TodoStatus::Completed),
            item("b", "next", TodoStatus::Pending),
        ]);
        assert!(!app.todos_collapsed);
        assert_eq!(panel_height(&app, 24), 3);
    }

    #[test]
    fn user_expand_survives_completed_refresh() {
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.replace_todos(vec![item("a", "one", TodoStatus::Completed)]);
        app.toggle_todos_panel();
        assert!(!app.todos_collapsed);
        app.replace_todos(vec![item("a", "one", TodoStatus::Completed)]);
        assert!(!app.todos_collapsed, "keep the list open while reviewing");
    }

    #[test]
    fn glyphs_match_grok_todo_pane() {
        assert_eq!(status_glyph(TodoStatus::Pending), "□");
        assert_eq!(status_glyph(TodoStatus::InProgress), "▶");
        assert_eq!(status_glyph(TodoStatus::Completed), "✓");
        assert_eq!(status_glyph(TodoStatus::Cancelled), "✗");
    }
}
