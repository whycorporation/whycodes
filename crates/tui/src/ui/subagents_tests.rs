use super::*;
use crate::app::{BgJobUi, SubagentUpdate, TuiApp};
use crate::config::TuiAppConfig;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use std::time::Instant;

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

fn running(id: &str, kind: &str, desc: &str) -> SubagentUpdate {
    SubagentUpdate {
        id: id.into(),
        kind: kind.into(),
        description: desc.into(),
        status: "running".into(),
        activity: "Thinking".into(),
        elapsed_ms: 0,
        output: String::new(),
    }
}

#[test]
fn hidden_when_empty() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    assert_eq!(panel_height(&app, 24), 0);
    let text = paint(&mut app, 40, 2);
    assert!(!text.contains("Tasks"), "{text}");
    assert!(app.tasks_hit.rect.is_none());
}

#[test]
fn shows_marks_and_counts() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.upsert_subagent(running("task-1", "explore", "scan the crate"));
    app.upsert_subagent(SubagentUpdate {
        id: "task-2".into(),
        kind: "general".into(),
        description: "patch files".into(),
        status: "completed".into(),
        activity: String::new(),
        elapsed_ms: 1400,
        output: "ok".into(),
    });
    app.bg_jobs.push(BgJobUi {
        id: "bg-1".into(),
        summary: "cargo test".into(),
        status: "failed".into(),
        started_at: Instant::now(),
        elapsed_ms: 800,
    });
    assert!(!app.tasks_collapsed);
    assert_eq!(panel_height(&app, 24), 4);
    let text = paint(&mut app, 72, 6);
    assert!(text.contains("▾ Tasks"), "{text}");
    assert!(text.contains("2/3 67%"), "{text}");
    assert!(text.contains("explore · scan the crate"), "{text}");
    assert!(text.contains("general · patch files"), "{text}");
    assert!(text.contains("bg · cargo test"), "{text}");
    assert!(
        text.contains('░') || text.contains('█'),
        "progress track: {text}"
    );
    assert!(app.tasks_hit.rect.is_some());
    assert_eq!(
        app.tasks_row_hits.len(),
        2,
        "only subagent rows are clickable"
    );
}

#[test]
fn hover_does_not_underline_tasks_label() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.upsert_subagent(running("task-1", "explore", "scan"));
    app.tasks_hit.hovered = true;
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
                "Tasks label must not underline on hover"
            );
        }
    }
    assert!(found, "Tasks T cell");
}

#[test]
fn all_done_auto_collapses_and_can_reopen() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.upsert_subagent(running("task-1", "explore", "scan"));
    app.upsert_subagent(SubagentUpdate {
        id: "task-1".into(),
        kind: "explore".into(),
        description: "scan".into(),
        status: "completed".into(),
        activity: String::new(),
        elapsed_ms: 400,
        output: "ok".into(),
    });
    assert!(app.tasks_collapsed);
    assert_eq!(panel_height(&app, 24), 1);
    let text = paint(&mut app, 40, 1);
    assert!(text.contains("▸ Tasks"), "{text}");
    assert!(text.contains("1/1 100%"), "{text}");
    assert!(!text.contains("scan"), "{text}");

    app.toggle_tasks_pane();
    assert!(!app.tasks_collapsed);
    assert_eq!(panel_height(&app, 24), 2);
    let text = paint(&mut app, 40, 3);
    assert!(text.contains("▾ Tasks"), "{text}");
    assert!(text.contains("explore · scan"), "{text}");
}

#[test]
fn overflow_line_when_many() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    for i in 0..10 {
        app.upsert_subagent(running(&format!("t-{i}"), "explore", &format!("item {i}")));
    }
    assert_eq!(panel_height(&app, 24), 10); // header + 8 + overflow
    let text = paint(&mut app, 40, 12);
    assert!(text.contains("+2 more"), "{text}");
    assert!(text.contains("item 0"), "{text}");
    assert!(!text.contains("item 9"), "{text}");
}

#[test]
fn tiny_body_hides_panel() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.upsert_subagent(running("task-1", "explore", "scan"));
    assert_eq!(panel_height(&app, 3), 0);
    assert_eq!(panel_height(&app, 0), 0);
}

#[test]
fn user_collapse_hides_items_while_work_remains() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.upsert_subagent(running("task-1", "explore", "still open"));
    app.toggle_tasks_pane();
    assert!(app.tasks_collapsed);
    assert_eq!(panel_height(&app, 24), 1);
    let text = paint(&mut app, 40, 2);
    assert!(text.contains("▸ Tasks"), "{text}");
    assert!(text.contains("0/1 0%"), "{text}");
    assert!(!text.contains("still open"), "{text}");
}

#[test]
fn panel_indents_text_by_side() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.upsert_subagent(running("task-1", "explore", "scan the crate"));
    let text = paint_with_side(&mut app, 60, 4, 2);
    let header = text.lines().next().expect("header");
    assert!(header.starts_with("  ▾"), "side indent: {header:?}");
    let item_row = text
        .lines()
        .find(|l| l.contains("scan the crate"))
        .expect("item");
    assert!(
        item_row.starts_with("    "),
        "side + 2 item indent: {item_row:?}"
    );
}

#[test]
fn strip_empty_when_no_children() {
    let app = TuiApp::new(TuiAppConfig::default());
    assert_eq!(strip_height(&app), 0);
}

#[test]
fn strip_shows_running_count_and_kind() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.upsert_subagent(running("task-1", "explore", "scan the crate"));
    assert_eq!(strip_height(&app), 2);
    assert_eq!(app.running_subagent_count(), 1);
    let text = paint(&mut app, 80, 3);
    assert!(text.contains("Tasks"), "{text}");
    assert!(text.contains("explore"), "{text}");
    assert!(text.contains("scan the crate"), "{text}");
    assert!(!text.contains("Ctrl+G"), "{text}");
}

#[test]
fn strip_indents_text_by_side_and_keeps_full_width_hit() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.upsert_subagent(running("task-1", "explore", "scan the crate"));
    let palette = app.config.palette();
    let backend = TestBackend::new(80, 3);
    let mut terminal = Terminal::new(backend).expect("term");
    terminal
        .draw(|f| render_panel(f, f.area(), &mut app, &palette, 2))
        .expect("draw");
    let buf = terminal.backend().buffer().clone();
    let mut row = String::new();
    for x in 0..buf.area().width {
        if let Some(c) = buf.cell((x, 0)) {
            row.push_str(c.symbol());
        }
    }
    assert!(row.starts_with("  "), "side indent: {row:?}");
    assert!(row.contains("Tasks"), "{row}");
    let hit = app.tasks_hit.rect.expect("header hit");
    assert_eq!(hit.x, 0);
    assert_eq!(hit.width, 80, "hit stays full width");
    let (row_hit, id) = app.tasks_row_hits.first().expect("row hit");
    assert_eq!(id, "task-1");
    assert_eq!(row_hit.width, 80);
}

#[test]
fn upsert_updates_same_id() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.upsert_subagent(crate::app::SubagentUpdate {
        id: "w-0".into(),
        kind: "general".into(),
        description: "do it".into(),
        status: "running".into(),
        activity: "Thinking".into(),
        elapsed_ms: 0,
        output: String::new(),
    });
    app.upsert_subagent(crate::app::SubagentUpdate {
        id: "w-0".into(),
        kind: "general".into(),
        description: "do it".into(),
        status: "completed".into(),
        activity: String::new(),
        elapsed_ms: 1400,
        output: "done".into(),
    });
    assert_eq!(app.subagents.len(), 1);
    assert!(!app.subagents[0].is_running());
    assert_eq!(app.subagents[0].output, "done");
    assert_eq!(app.running_subagent_count(), 0);
    assert!(app.tasks_collapsed);
}
