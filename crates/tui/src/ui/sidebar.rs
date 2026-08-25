// ── ui/sidebar.rs: Sidebar panel ───────────────────────────────────────

use crate::app::{SidebarPreview, SidebarTab, TuiApp};
use crate::theme::ThemePalette;
use crate::widgets::diff::{parse_unified_diff, render_unified_diff};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Tabs, Wrap},
};

pub fn render(frame: &mut Frame, area: Rect, app: &TuiApp, palette: &ThemePalette) {
    let sidebar = &app.sidebar;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // tab bar
            Constraint::Min(1),    // content
        ])
        .split(area);

    // Tab bar.
    let tabs = SidebarTab::ALL;
    let tab_titles: Vec<Line> = tabs
        .iter()
        .map(|tab_enum| {
            let name = tab_enum.label();
            if *tab_enum == sidebar.active_tab {
                Line::from(Span::styled(
                    format!(" {} ", name),
                    Style::default()
                        .fg(palette.accent)
                        .add_modifier(Modifier::BOLD),
                ))
            } else {
                Line::from(Span::styled(
                    format!(" {} ", name),
                    Style::default().fg(palette.dim),
                ))
            }
        })
        .collect();

    let tab_widget = Tabs::new(tab_titles)
        .block(Block::default().borders(Borders::NONE))
        .style(Style::default().bg(palette.sidebar_bg));
    frame.render_widget(tab_widget, chunks[0]);

    // Content area.
    let content_area = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(palette.border))
        .style(Style::default().bg(palette.sidebar_bg));
    frame.render_widget(content_area.clone(), chunks[1]);

    let inner = content_area.inner(chunks[1]);

    match sidebar.active_tab {
        SidebarTab::Files => render_files(frame, inner, app, palette),
        SidebarTab::Diagnostics => render_diagnostics(frame, inner, app, palette),
        SidebarTab::Mcp => render_mcp(frame, inner, app, palette),
        SidebarTab::Todos => render_todos(frame, inner, app, palette),
        SidebarTab::Preview => render_preview(frame, inner, app, palette),
        SidebarTab::Agents => render_agents(frame, inner, app, palette),
    }
}

fn render_agents(frame: &mut Frame, area: Rect, app: &TuiApp, palette: &ThemePalette) {
    let lines = super::subagents::agent_lines(app, palette);
    frame.render_widget(
        Paragraph::new(Text::from(lines)).wrap(Wrap { trim: true }),
        area,
    );
}

fn render_files(frame: &mut Frame, area: Rect, app: &TuiApp, palette: &ThemePalette) {
    let sidebar = &app.sidebar;
    let mut lines: Vec<Line> = Vec::new();

    if sidebar.file_tree.is_empty() {
        lines.push(Line::from(Span::styled(
            " No files loaded ",
            Style::default().fg(palette.dim),
        )));
    } else {
        for path in &sidebar.file_tree {
            let icon = if path.ends_with('/') { "📁" } else { "📄" };
            lines.push(Line::from(Span::styled(
                format!(" {} {}", icon, path),
                Style::default().fg(palette.fg),
            )));
        }
    }

    let p = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: true });
    frame.render_widget(p, area);
}

fn render_diagnostics(frame: &mut Frame, area: Rect, app: &TuiApp, palette: &ThemePalette) {
    let sidebar = &app.sidebar;
    let text = if sidebar.diagnostics == 0 {
        vec![
            Line::from(""),
            Line::from(Span::styled(
                " ✓ No issues found ",
                Style::default().fg(palette.success),
            )),
        ]
    } else {
        vec![
            Line::from(""),
            Line::from(Span::styled(
                format!(" {} issues found ", sidebar.diagnostics),
                Style::default().fg(palette.warning),
            )),
        ]
    };

    let p = Paragraph::new(Text::from(text));
    frame.render_widget(p, area);
}

fn render_mcp(frame: &mut Frame, area: Rect, app: &TuiApp, palette: &ThemePalette) {
    let sidebar = &app.sidebar;
    let mut lines: Vec<Line> = Vec::new();

    if sidebar.mcp_status.is_empty() {
        lines.push(Line::from(Span::styled(
            " No MCP servers ",
            Style::default().fg(palette.dim),
        )));
    } else {
        for status in &sidebar.mcp_status {
            lines.push(Line::from(Span::styled(
                status,
                Style::default().fg(palette.fg),
            )));
        }
    }

    let p = Paragraph::new(Text::from(lines));
    frame.render_widget(p, area);
}

fn render_todos(frame: &mut Frame, area: Rect, app: &TuiApp, palette: &ThemePalette) {
    let mut lines: Vec<Line> = Vec::new();

    if app.todos.is_empty() {
        lines.push(Line::from(Span::styled(
            " No TODOs ",
            Style::default().fg(palette.dim),
        )));
    } else {
        for item in &app.todos {
            lines.push(super::todos::item_line(item, palette));
        }
    }

    let p = Paragraph::new(Text::from(lines));
    frame.render_widget(p, area);
}

fn render_preview(frame: &mut Frame, area: Rect, app: &TuiApp, palette: &ThemePalette) {
    match &app.sidebar.preview {
        SidebarPreview::None => {
            let p = Paragraph::new(Text::from(vec![
                Line::from(""),
                Line::from(Span::styled(
                    " No preview ",
                    Style::default().fg(palette.dim),
                )),
                Line::from(Span::styled(
                    " Agent: panel show_file / show_diff / show_mermaid ",
                    Style::default().fg(palette.dim),
                )),
            ]));
            frame.render_widget(p, area);
        }
        SidebarPreview::File { path, text } => {
            let mut lines = vec![Line::from(Span::styled(
                format!(" {path} "),
                Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD),
            ))];
            for raw in text.lines().take(200) {
                lines.push(Line::from(Span::styled(
                    format!(" {raw}"),
                    Style::default().fg(palette.fg),
                )));
            }
            frame.render_widget(
                Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
                area,
            );
        }
        SidebarPreview::Diff { unified, .. } => {
            let parsed = parse_unified_diff(unified);
            render_unified_diff(frame, area, &parsed, palette);
        }
        SidebarPreview::Mermaid { source } => {
            let rendered =
                whycode_format::mermaid::render_mermaid(source, Some(area.width as usize))
                    .unwrap_or_else(|_| {
                        std::sync::Arc::new(source.lines().map(str::to_string).collect())
                    });
            let mut lines = vec![Line::from(Span::styled(
                " mermaid ",
                Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD),
            ))];
            for raw in rendered.iter().take(200) {
                lines.push(Line::from(Span::styled(
                    format!(" {raw}"),
                    Style::default().fg(palette.fg),
                )));
            }
            frame.render_widget(
                Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
                area,
            );
        }
    }
}

#[cfg(test)]
mod tests {
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
        let text = paint(60, 12, |f| render(f, f.area(), &app, &palette));
        assert!(text.contains("Agents"), "{text}");
        assert!(text.contains("explore"), "{text}");
        assert!(text.contains("look around"), "{text}");
    }

    #[test]
    fn paints_all_tab_labels() {
        let app = TuiApp::new(cfg());
        let palette = app.config.palette();
        let text = paint(60, 12, |f| render(f, f.area(), &app, &palette));
        for tab in SidebarTab::ALL {
            assert!(
                text.contains(tab.label()),
                "tab label {:?} missing: {text}",
                tab.label()
            );
        }
    }

    #[test]
    fn files_tab_empty_and_populated() {
        // Empty tree → hint.
        let app = app_with_tab(SidebarTab::Files);
        let palette = app.config.palette();
        let text = paint(60, 12, |f| render(f, f.area(), &app, &palette));
        assert!(text.contains("No files loaded"), "{text}");

        // Populated tree → folder/file icons.
        let mut app = app_with_tab(SidebarTab::Files);
        app.sidebar.file_tree = vec!["src/".into(), "src/main.rs".into()];
        let palette = app.config.palette();
        let text = paint(60, 12, |f| render(f, f.area(), &app, &palette));
        assert!(text.contains("📁"), "{text}");
        assert!(text.contains("📄"), "{text}");
        assert!(text.contains("src/"), "{text}");
        assert!(text.contains("src/main.rs"), "{text}");
    }

    #[test]
    fn diagnostics_tab_clean_and_counted() {
        // Zero diagnostics → success line.
        let app = app_with_tab(SidebarTab::Diagnostics);
        let palette = app.config.palette();
        let text = paint(60, 12, |f| render(f, f.area(), &app, &palette));
        assert!(text.contains("No issues found"), "{text}");

        // Counted diagnostics → warning line.
        let mut app = app_with_tab(SidebarTab::Diagnostics);
        app.sidebar.diagnostics = 3;
        let palette = app.config.palette();
        let text = paint(60, 12, |f| render(f, f.area(), &app, &palette));
        assert!(text.contains("3 issues found"), "{text}");
    }

    #[test]
    fn mcp_tab_empty_and_statuses() {
        let app = app_with_tab(SidebarTab::Mcp);
        let palette = app.config.palette();
        let text = paint(60, 12, |f| render(f, f.area(), &app, &palette));
        assert!(text.contains("No MCP servers"), "{text}");

        let mut app = app_with_tab(SidebarTab::Mcp);
        app.sidebar.mcp_status = vec!["connected: alpha".into(), "error: beta".into()];
        let palette = app.config.palette();
        let text = paint(60, 12, |f| render(f, f.area(), &app, &palette));
        assert!(text.contains("connected: alpha"), "{text}");
        assert!(text.contains("error: beta"), "{text}");
    }

    #[test]
    fn todos_tab_empty_and_items() {
        let app = app_with_tab(SidebarTab::Todos);
        let palette = app.config.palette();
        let text = paint(60, 12, |f| render(f, f.area(), &app, &palette));
        assert!(text.contains("No TODOs"), "{text}");

        let mut app = app_with_tab(SidebarTab::Todos);
        app.todos = vec![
            whycode_core::TodoItem::new("a", "first", whycode_core::TodoStatus::Pending),
            whycode_core::TodoItem::new("b", "done", whycode_core::TodoStatus::Completed),
        ];
        let palette = app.config.palette();
        let text = paint(60, 12, |f| render(f, f.area(), &app, &palette));
        assert!(text.contains("□ first"), "{text}");
        assert!(text.contains("✓ done"), "{text}");
    }

    #[test]
    fn preview_none_shows_hint() {
        let app = app_with_tab(SidebarTab::Preview);
        let palette = app.config.palette();
        let text = paint(60, 12, |f| render(f, f.area(), &app, &palette));
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
        let text = paint(60, 20, |f| render(f, f.area(), &app, &palette));
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
        let text = paint(60, 20, |f| render(f, f.area(), &app, &palette));
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
        let text = paint(60, 20, |f| render(f, f.area(), &app, &palette));
        // Header always painted; without the `mermaid` feature the raw source
        // is shown, otherwise the rendered diagram — both contain the label.
        assert!(text.contains("mermaid"), "{text}");
        assert!(
            text.contains("A") && text.contains("B"),
            "diagram content visible: {text}"
        );
    }
}
