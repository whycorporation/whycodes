use super::*;
use crate::app::{SidebarPreview, SidebarTab, TuiApp};
use crate::config::TuiAppConfig;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn cfg() -> TuiAppConfig {
    TuiAppConfig::default()
}

fn app_with_tab(tab: SidebarTab) -> TuiApp {
    let mut app = TuiApp::new(cfg());
    app.sidebar.active_tab = tab;
    app
}

/// Render `f` into a fresh terminal and return the painted buffer text.
fn paint<F>(width: u16, height: u16, f: F) -> String
where
    F: FnOnce(&mut ratatui::Frame),
{
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(f).expect("draw");
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
fn agents_tab_lists_subagents() {
    let mut app = TuiApp::new(cfg());
    app.sidebar.active_tab = SidebarTab::Agents;
    app.upsert_subagent(crate::app::SubagentUpdate {
        id: "t1".into(),
        kind: "explore".into(),
        description: "look around".into(),
        status: "running".into(),
        activity: "Thinking".into(),
        elapsed_ms: 0,
        output: String::new(),
    });
    let palette = app.config.palette();
    let text = paint(60, 12, |f| render(f, f.area(), &mut app, &palette));
    assert!(text.contains("Agents"), "{text}");
    assert!(text.contains("explore"), "{text}");
    assert!(text.contains("look around"), "{text}");
}

#[test]
fn paints_all_tab_labels() {
    let mut app = TuiApp::new(cfg());
    let palette = app.config.palette();
    let text = paint(60, 12, |f| render(f, f.area(), &mut app, &palette));
    for tab in SidebarTab::ALL {
        assert!(
            text.contains(tab.label()),
            "tab label {:?} missing: {text}",
            tab.label()
        );
    }
    assert!(
        text.contains(TAB_SEP_PADDED) || text.contains(TAB_SEP_TIGHT),
        "tab strip must use status-bar separators: {text}"
    );
}

#[test]
fn narrow_rail_falls_back_to_letter_tabs() {
    let mut app = TuiApp::new(cfg());
    let palette = app.config.palette();
    let text = paint(24, 8, |f| render(f, f.area(), &mut app, &palette));
    assert!(text.contains("F"), "{text}");
    assert!(text.contains("A"), "{text}");
    assert!(
        app.sidebar.tab_hits.iter().all(|h| h.rect.is_some()),
        "letter pack still paints six click targets"
    );
}

#[test]
fn files_tab_empty_and_populated() {
    // Empty tree → hint.
    let mut app = app_with_tab(SidebarTab::Files);
    let palette = app.config.palette();
    let text = paint(60, 12, |f| render(f, f.area(), &mut app, &palette));
    assert!(text.contains("No files loaded"), "{text}");

    // Populated tree → folder/file icons.
    let mut app = app_with_tab(SidebarTab::Files);
    app.sidebar.file_tree = vec!["src/".into(), "src/main.rs".into()];
    let palette = app.config.palette();
    let text = paint(60, 12, |f| render(f, f.area(), &mut app, &palette));
    assert!(text.contains("📁"), "{text}");
    assert!(text.contains("📄"), "{text}");
    assert!(text.contains("src/"), "{text}");
    assert!(text.contains("src/main.rs"), "{text}");
}

#[test]
fn diagnostics_tab_clean_and_counted() {
    // Zero diagnostics → success line.
    let mut app = app_with_tab(SidebarTab::Diagnostics);
    let palette = app.config.palette();
    let text = paint(60, 12, |f| render(f, f.area(), &mut app, &palette));
    assert!(text.contains("No issues found"), "{text}");

    // Counted diagnostics → warning line.
    let mut app = app_with_tab(SidebarTab::Diagnostics);
    app.sidebar.diagnostics = 3;
    let palette = app.config.palette();
    let text = paint(60, 12, |f| render(f, f.area(), &mut app, &palette));
    assert!(text.contains("3 issues found"), "{text}");
}

#[test]
fn mcp_tab_empty_and_statuses() {
    let mut app = app_with_tab(SidebarTab::Mcp);
    let palette = app.config.palette();
    let text = paint(60, 12, |f| render(f, f.area(), &mut app, &palette));
    assert!(text.contains("No MCP servers"), "{text}");

    let mut app = app_with_tab(SidebarTab::Mcp);
    app.sidebar.mcp_status = vec!["connected: alpha".into(), "error: beta".into()];
    let palette = app.config.palette();
    let text = paint(60, 12, |f| render(f, f.area(), &mut app, &palette));
    assert!(text.contains("connected: alpha"), "{text}");
    assert!(text.contains("error: beta"), "{text}");
}

#[test]
fn todos_tab_empty_and_items() {
    let mut app = app_with_tab(SidebarTab::Todos);
    let palette = app.config.palette();
    let text = paint(60, 12, |f| render(f, f.area(), &mut app, &palette));
    assert!(text.contains("No TODOs"), "{text}");

    let mut app = app_with_tab(SidebarTab::Todos);
    app.todos = vec![
        whycodes_core::TodoItem::new("a", "first", whycodes_core::TodoStatus::Pending),
        whycodes_core::TodoItem::new("b", "done", whycodes_core::TodoStatus::Completed),
    ];
    let palette = app.config.palette();
    let text = paint(60, 12, |f| render(f, f.area(), &mut app, &palette));
    assert!(text.contains("□ first"), "{text}");
    assert!(text.contains("✓ done"), "{text}");
}

#[test]
fn preview_none_shows_hint() {
    let mut app = app_with_tab(SidebarTab::Preview);
    let palette = app.config.palette();
    let text = paint(60, 12, |f| render(f, f.area(), &mut app, &palette));
    assert!(text.contains("No preview"), "{text}");
    assert!(text.contains("show_file"), "{text}");
}

#[test]
fn preview_file_shows_path_and_lines() {
    let mut app = app_with_tab(SidebarTab::Preview);
    app.sidebar.preview = SidebarPreview::File {
        path: "src/main.rs".into(),
        text: "fn main() {\n    println!(\"hi\");\n}".into(),
    };
    let palette = app.config.palette();
    let text = paint(60, 20, |f| render(f, f.area(), &mut app, &palette));
    assert!(text.contains("src/main.rs"), "{text}");
    assert!(text.contains("fn main() {"), "{text}");
    assert!(text.contains("println!"), "{text}");
}

#[test]
fn preview_diff_parses_unified() {
    let mut app = app_with_tab(SidebarTab::Preview);
    app.sidebar.preview = SidebarPreview::Diff {
        path: "a.rs".into(),
        unified: "--- a.rs\n+++ b.rs\n@@ -1,2 +1,2 @@\n-old line\n+new line\n same".into(),
    };
    let palette = app.config.palette();
    let text = paint(60, 20, |f| render(f, f.area(), &mut app, &palette));
    assert!(text.contains("Diff"), "{text}");
    assert!(text.contains("-old line"), "{text}");
    assert!(text.contains("+new line"), "{text}");
    assert!(text.contains("same"), "{text}");
}

#[test]
fn preview_mermaid_renders_or_falls_back() {
    let mut app = app_with_tab(SidebarTab::Preview);
    app.sidebar.preview = SidebarPreview::Mermaid {
        source: "A --> B".into(),
    };
    let palette = app.config.palette();
    let text = paint(60, 20, |f| render(f, f.area(), &mut app, &palette));
    // Header always painted; without the `mermaid` feature the raw source
    // is shown, otherwise the rendered diagram — both contain the label.
    assert!(text.contains("mermaid"), "{text}");
    assert!(
        text.contains("A") && text.contains("B"),
        "diagram content visible: {text}"
    );
}

#[test]
fn tab_strip_sets_hit_rects_and_hover_brightens() {
    use ratatui::style::Modifier;

    let mut app = TuiApp::new(cfg());
    let palette = app.config.palette();
    let _ = paint(60, 8, |f| render(f, f.area(), &mut app, &palette));
    assert!(
        app.sidebar.tab_hits.iter().all(|h| h.rect.is_some()),
        "every tab must have a click target"
    );

    let diag_rect = app.sidebar.tab_hits[1].rect.expect("Diag hit");
    app.sidebar.tab_hits[1].hovered = true;
    let backend = TestBackend::new(60, 8);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|f| render(f, f.area(), &mut app, &palette))
        .expect("draw");
    let buf = terminal.backend().buffer();
    let cell = buf.cell((diag_rect.x + 1, diag_rect.y)).expect("tab cell");
    assert_eq!(
        cell.style().fg,
        Some(palette.fg),
        "hovered inactive tab uses fg"
    );
    assert!(
        !cell.style().add_modifier.contains(Modifier::UNDERLINED),
        "tab hover must not underline"
    );
}
