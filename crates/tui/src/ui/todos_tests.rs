use super::*;
use crate::config::TuiAppConfig;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use whycodes_core::todo::{TodoItem, TodoStatus};

fn paint(app: &mut TuiApp, w: u16, h: u16) -> String {
    paint_with_side(app, w, h, 0)
}

fn paint_with_side(app: &mut TuiApp, w: u16, h: u16, side: u16) -> String {
    let palette = app.config.palette();
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).expect("term");
    terminal
        .draw(|f| render_panel(f, f.area(), app, &palette, side))
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
    assert!(text.contains("▾ Todos"), "{text}");
    assert!(text.contains("2/4 50%"), "{text}");
    assert!(text.contains("□ pending one"), "{text}");
    assert!(text.contains("▶ working now"), "{text}");
    assert!(text.contains("✓ done item"), "{text}");
    assert!(text.contains("✗ skipped"), "{text}");
    assert!(
        text.contains('░') || text.contains('█'),
        "progress track: {text}"
    );
    assert!(app.todos_hit.rect.is_some());
    let header = text.lines().next().expect("header");
    assert!(
        header.contains("2/4 50% "),
        "count + percent sit next to the track: {header:?}"
    );
}

#[test]
fn hover_does_not_underline_todos_label() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.replace_todos(vec![item("a", "open", TodoStatus::Pending)]);
    app.todos_hit.hovered = true;
    let palette = app.config.palette();
    let backend = TestBackend::new(40, 2);
    let mut terminal = Terminal::new(backend).expect("term");
    terminal
        .draw(|f| render_panel(f, f.area(), &mut app, &palette, 0))
        .expect("draw");
    let buf = terminal.backend().buffer();
    let mut found = false;
    for x in 0..40u16 {
        let cell = buf.cell((x, 0)).expect("cell");
        if cell.symbol() == "T" {
            found = true;
            assert!(
                !cell.style().add_modifier.contains(Modifier::UNDERLINED),
                "Todos label must not underline on hover"
            );
        }
    }
    assert!(found, "Todos T cell");
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
    assert!(text.contains("▸ Todos"), "{text}");
    assert!(text.contains("2/2 100%"), "{text}");
    assert!(!text.contains(" done"), "{text}");
    assert!(!text.contains('□'), "{text}");
    assert!(!text.contains("✓ one"), "{text}");
    assert!(!text.contains("✗ two"), "{text}");

    app.toggle_todos_panel();
    assert!(!app.todos_collapsed);
    assert_eq!(panel_height(&app, 24), 3);
    let text = paint(&mut app, 40, 4);
    assert!(text.contains("▾ Todos"), "{text}");
    assert!(text.contains("2/2 100%"), "{text}");
    assert!(text.contains("✓ one"), "{text}");
    assert!(text.contains("✗ two"), "{text}");
}

#[test]
fn overflow_scrolls_instead_of_plus_n() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.replace_todos(
        (0..10)
            .map(|i| item(&i.to_string(), &format!("item {i}"), TodoStatus::Pending))
            .collect(),
    );
    assert_eq!(panel_height(&app, 24), 9); // header + 8 items
    let text = paint(&mut app, 40, 12);
    assert!(!text.contains("+2 more"), "{text}");
    assert!(text.contains("item 0"), "{text}");
    assert!(!text.contains("item 9"), "{text}");
    assert!(app.todos_can_scroll());
    assert!(app.todos_scrollbar_hit.rect.is_some());
    assert_eq!(app.todos_viewport_rows, 8);

    assert!(app.scroll_todos(2));
    let text = paint(&mut app, 40, 12);
    assert!(!text.contains("item 0"), "{text}");
    assert!(text.contains("item 9"), "{text}");
    assert_eq!(app.todos_scroll, 2);
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
    assert!(text.contains("▸ Todos"), "{text}");
    assert!(text.contains("0/1 0%"), "{text}");
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

#[test]
fn panel_indents_text_by_side_and_keeps_item_line_unpadded() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.replace_todos(
        (0..10)
            .map(|i| item(&i.to_string(), &format!("item {i}"), TodoStatus::Pending))
            .collect(),
    );
    let text = paint_with_side(&mut app, 60, 12, 2);
    let header = text.lines().next().expect("header");
    assert!(header.starts_with("  ▾"), "side indent: {header:?}");
    let item_row = text.lines().find(|l| l.contains("item 0")).expect("item 0");
    assert!(
        item_row.starts_with("    □"),
        "side + 2 item indent: {item_row:?}"
    );
    assert!(
        !text.contains("+2 more"),
        "overflowing lists scroll instead of +N: {text}"
    );
    assert!(
        app.todos_scrollbar_hit.rect.is_some(),
        "overflowing panel paints a scrollbar"
    );
    // Sidebar helper stays at the original two-space indent.
    let sidebar = item_line(
        &item("z", "sidebar row", TodoStatus::Pending),
        &app.config.palette(),
    );
    let sidebar_s: String = sidebar.spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        sidebar_s.starts_with("  □"),
        "item_line must not take side: {sidebar_s:?}"
    );
}
