// ── ui/sidebar.rs: Sidebar panel ───────────────────────────────────────

use crate::app::{SidebarTab, TuiApp};
use crate::theme::ThemePalette;
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
    let tabs = ["Files", "Diag", "MCP", "Todos"];
    let tab_titles: Vec<Line> = tabs
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let tab_enum = match i {
                0 => SidebarTab::Files,
                1 => SidebarTab::Diagnostics,
                2 => SidebarTab::Mcp,
                3 => SidebarTab::Todos,
                _ => SidebarTab::Files,
            };
            if tab_enum == sidebar.active_tab {
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
    }
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
    let sidebar = &app.sidebar;
    let mut lines: Vec<Line> = Vec::new();

    if sidebar.todos.is_empty() {
        lines.push(Line::from(Span::styled(
            " No TODOs ",
            Style::default().fg(palette.dim),
        )));
    } else {
        for todo in &sidebar.todos {
            lines.push(Line::from(Span::styled(
                format!(" ☐ {}", todo),
                Style::default().fg(palette.fg),
            )));
        }
    }

    let p = Paragraph::new(Text::from(lines));
    frame.render_widget(p, area);
}
