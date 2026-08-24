//! Sticky todo panel under the header (Grok Build-style).
//!
//! Header is always one row (`▸`/`▾ Todos  done/total`). Click the header
//! (or `t` in scrollback) to fold. Finished items stay in the list so they
//! can be reopened after the panel auto-collapses when everything is done.

use crate::app::TuiApp;
use crate::theme::ThemePalette;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
};
use whycode_core::todo::{TodoItem, TodoStatus, all_terminal, terminal_count};

/// Max item rows (not counting the header or overflow line).
pub const MAX_ITEMS: usize = 8;

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

pub fn status_style(status: TodoStatus, palette: &ThemePalette) -> Style {
    match status {
        TodoStatus::Pending => Style::default().fg(palette.fg),
        TodoStatus::InProgress => Style::default()
            .fg(palette.accent)
            .add_modifier(Modifier::BOLD),
        TodoStatus::Completed => Style::default()
            .fg(palette.success)
            .add_modifier(Modifier::DIM),
        TodoStatus::Cancelled => Style::default().fg(palette.dim),
    }
}

pub fn item_line(item: &TodoItem, palette: &ThemePalette) -> Line<'static> {
    Line::from(Span::styled(
        format!("   {} {}", item.status.mark(), item.content),
        status_style(item.status, palette),
    ))
}

fn header_line(app: &TuiApp, palette: &ThemePalette) -> Line<'static> {
    let total = app.todos.len();
    let done = terminal_count(&app.todos);
    let collapsed = is_collapsed(app);
    let all_done = all_terminal(&app.todos);
    let chevron = if collapsed { "▸" } else { "▾" };
    let counts = if all_done {
        format!("{done}/{total} done")
    } else {
        format!("{done}/{total}")
    };
    let color = if all_done {
        palette.success
    } else {
        palette.accent
    };
    let mut style = Style::default().fg(color).add_modifier(Modifier::BOLD);
    if app.todos_hit.hovered {
        style = style.add_modifier(Modifier::REVERSED);
    }
    Line::from(Span::styled(format!(" {chevron} Todos  {counts}"), style))
}

pub fn render_panel(frame: &mut Frame, area: Rect, app: &mut TuiApp, palette: &ThemePalette) {
    if area.height == 0 || app.todos.is_empty() {
        app.todos_hit.clear();
        return;
    }
    let collapsed = is_collapsed(app) || area.height == 1;

    let mut lines: Vec<Line> = Vec::new();
    lines.push(header_line(app, palette));

    if !collapsed {
        let mut budget = area.height.saturating_sub(1) as usize;
        let overflow = app.todos.len() > MAX_ITEMS.min(budget);
        if overflow {
            budget = budget.saturating_sub(1);
        }
        let take = budget.min(MAX_ITEMS).min(app.todos.len());
        for item in app.todos.iter().take(take) {
            lines.push(item_line(item, palette));
        }
        let hidden = app.todos.len().saturating_sub(take);
        if hidden > 0 {
            lines.push(Line::from(Span::styled(
                format!("   … +{hidden} more"),
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
    use whycode_core::todo::{TodoItem, TodoStatus};

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
        assert!(text.contains("☐ pending one"), "{text}");
        assert!(text.contains("▶ working now"), "{text}");
        assert!(text.contains("☑ done item"), "{text}");
        assert!(text.contains("✗ skipped"), "{text}");
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
        assert!(!text.contains("☐"), "{text}");
        assert!(!text.contains("☑ one"), "{text}");
        assert!(!text.contains("✗ two"), "{text}");

        app.toggle_todos_panel();
        assert!(!app.todos_collapsed);
        assert_eq!(panel_height(&app, 24), 3);
        let text = paint(&mut app, 40, 4);
        assert!(text.contains("▾ Todos  2/2 done"), "{text}");
        assert!(text.contains("☑ one"), "{text}");
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
}
