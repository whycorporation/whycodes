// ── ui/sidebar.rs: Sidebar panel ───────────────────────────────────────
// Tab strip matches header / status-bar chrome: one row, dim `│`
// separators, bold-fg active label (no ratatui Tabs widget, no peach fill).

use crate::app::{SidebarPreview, SidebarTab, TuiApp};
use crate::theme::ThemePalette;
use crate::widgets::diff::{parse_unified_diff, render_unified_diff};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

/// Separator between tab labels (same glyph as the status bar).
const TAB_SEP_PADDED: &str = " │ ";
const TAB_SEP_TIGHT: &str = "│";

#[derive(Clone, Copy)]
struct TabPack {
    labels: [&'static str; 6],
    /// `true` → ` {label} ` so single-letter packs stay clickable.
    padded: bool,
    sep: &'static str,
}

fn tab_packs() -> [TabPack; 4] {
    [
        TabPack {
            labels: ["Files", "Diag", "MCP", "Todos", "View", "Agents"],
            padded: false,
            sep: TAB_SEP_PADDED,
        },
        TabPack {
            labels: ["Files", "Diag", "MCP", "Todos", "View", "Agents"],
            padded: false,
            sep: TAB_SEP_TIGHT,
        },
        TabPack {
            labels: ["File", "Diag", "MCP", "Todo", "View", "Agt"],
            padded: false,
            sep: TAB_SEP_TIGHT,
        },
        TabPack {
            labels: ["F", "D", "M", "T", "V", "A"],
            padded: true,
            sep: TAB_SEP_TIGHT,
        },
    ]
}

fn pack_width(pack: &TabPack) -> u16 {
    let labels: u16 = pack
        .labels
        .iter()
        .map(|s| s.width() as u16 + if pack.padded { 2 } else { 0 })
        .sum();
    let seps = (pack.labels.len() as u16).saturating_sub(1) * pack.sep.width() as u16;
    labels + seps
}

fn choose_pack(width: u16) -> TabPack {
    tab_packs()
        .into_iter()
        .find(|p| pack_width(p) <= width)
        .unwrap_or(tab_packs()[3])
}

fn render_tab_strip(frame: &mut Frame, area: Rect, app: &mut TuiApp, palette: &ThemePalette) {
    if area.width == 0 || area.height == 0 {
        app.sidebar.clear_tab_hits();
        return;
    }

    // Snapshot sticky hover before replacing rects (HitArea::clear would
    // drop the flag; paint must read last frame's hover).
    let prev_hover: [bool; SidebarTab::ALL.len()] =
        std::array::from_fn(|i| app.sidebar.tab_hits[i].hovered);

    let pack = choose_pack(area.width);
    let sep_style = Style::default().fg(palette.dim);
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut col = area.x;
    let end = area.x.saturating_add(area.width);
    let mut hits = [crate::hit_area::HitArea::default(); SidebarTab::ALL.len()];

    for (i, tab) in SidebarTab::ALL.iter().copied().enumerate() {
        if col >= end {
            break;
        }
        if i > 0 {
            let sep_w = pack.sep.width() as u16;
            if col.saturating_add(sep_w) > end {
                break;
            }
            spans.push(Span::styled(pack.sep, sep_style));
            col = col.saturating_add(sep_w);
        }

        let label = pack.labels[i];
        let cell = if pack.padded {
            format!(" {label} ")
        } else {
            label.to_string()
        };
        let w = cell.width() as u16;
        if col.saturating_add(w) > end {
            break;
        }

        let active = tab == app.sidebar.active_tab;
        let hovered = prev_hover[i];
        let style = if active {
            Style::default().fg(palette.fg).add_modifier(Modifier::BOLD)
        } else if hovered {
            // Hover brightens like the todo/tasks chevron — never underline
            // (a line under the compact strip fights the row).
            Style::default().fg(palette.fg)
        } else {
            Style::default().fg(palette.dim)
        };

        spans.push(Span::styled(cell, style));
        hits[i].set_rect(Some(Rect {
            x: col,
            y: area.y,
            width: w,
            height: 1,
        }));
        hits[i].hovered = hovered;
        col = col.saturating_add(w);
    }

    app.sidebar.tab_hits = hits;

    frame.render_widget(
        Paragraph::new(Text::from(Line::from(spans)))
            .style(Style::default().bg(palette.status_bar_bg)),
        area,
    );
}

pub fn render(frame: &mut Frame, area: Rect, app: &mut TuiApp, palette: &ThemePalette) {
    if area.width == 0 || area.height == 0 {
        app.sidebar.clear_tab_hits();
        return;
    }

    super::layout::fill_blank(frame, area, palette.sidebar_bg);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // tab strip
            Constraint::Min(1),    // content
        ])
        .split(area);

    render_tab_strip(frame, chunks[0], app, palette);

    let inner = Rect {
        x: chunks[1].x.saturating_add(1),
        y: chunks[1].y,
        width: chunks[1].width.saturating_sub(2),
        height: chunks[1].height,
    };
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    match app.sidebar.active_tab {
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
                whycodes_format::mermaid::render_mermaid(source, Some(area.width as usize))
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
#[path = "sidebar_tests.rs"]
mod tests;
