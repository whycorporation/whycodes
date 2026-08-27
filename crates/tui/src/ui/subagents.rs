//! Sticky tasks panel under the header (same chrome as [`super::todos`]).
//!
//! Header is always one row (`▸`/`▾ Tasks` plus a right-aligned
//! `done/total pct%` track). Click the header (or Ctrl+G) to fold.
//! Item rows list subagents and background jobs; click a subagent to
//! inspect the framed child transcript.

use crate::app::{BgJobUi, SubagentUi, TuiApp};
use crate::theme::ThemePalette;
use crate::ui::progress_bar::progress_bar_string;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

pub const SPIN: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Max item rows (not counting the header or overflow line).
pub const MAX_ITEMS: usize = 8;

/// Compact track in the header (same cells as the todo panel).
const HEADER_BAR_CELLS: u16 = 8;

/// True when the sticky panel should paint only the header row.
pub fn is_collapsed(app: &TuiApp) -> bool {
    app.tasks_collapsed
}

/// Rows reserved under the header when any task exists.
pub fn panel_height(app: &TuiApp, body_h: u16) -> u16 {
    let total = app.task_count();
    if total == 0 {
        return 0;
    }
    let max = body_h.saturating_sub(3);
    if max == 0 {
        return 0;
    }
    let want = if is_collapsed(app) {
        1
    } else {
        let shown = total.min(MAX_ITEMS);
        let extra = usize::from(total > MAX_ITEMS);
        (1 + shown + extra) as u16
    };
    want.min(max)
}

/// Back-compat alias used by older layout tests.
pub fn strip_height(app: &TuiApp) -> u16 {
    panel_height(app, u16::MAX)
}

fn status_glyph(status: &str, spin: usize) -> &'static str {
    match status {
        "running" => SPIN[spin % SPIN.len()],
        "completed" | "done" => "\u{2713}",              // ✓
        "failed" | "cancelled" | "killed" => "\u{2717}", // ✗
        _ => "\u{25a1}",                                 // □
    }
}

fn icon_style(status: &str, palette: &ThemePalette) -> Style {
    match status {
        "running" => Style::default()
            .fg(palette.warning)
            .add_modifier(Modifier::BOLD),
        "completed" | "done" => Style::default().fg(palette.success),
        "failed" | "cancelled" | "killed" => Style::default().fg(palette.error),
        _ => Style::default().fg(palette.fg),
    }
}

fn text_style(status: &str, palette: &ThemePalette) -> Style {
    match status {
        "running" => Style::default().fg(palette.fg).add_modifier(Modifier::BOLD),
        "completed" | "done" => Style::default().fg(palette.dim),
        "failed" | "cancelled" | "killed" => Style::default()
            .fg(palette.dim)
            .add_modifier(Modifier::CROSSED_OUT),
        _ => Style::default().fg(palette.fg),
    }
}

fn truncate(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        s.to_string()
    } else {
        format!(
            "{}…",
            s.chars().take(max.saturating_sub(1)).collect::<String>()
        )
    }
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("").trim()
}

/// One painted row in the sticky list (subagent or background job).
struct TaskRow {
    id: Option<String>,
    glyph: &'static str,
    status: String,
    label: String,
}

fn collect_rows(app: &TuiApp) -> Vec<TaskRow> {
    let spin = app.spinner_frame;
    let mut rows = Vec::with_capacity(app.task_count());
    for row in &app.subagents {
        rows.push(TaskRow {
            id: Some(row.id.clone()),
            glyph: status_glyph(&row.status, spin),
            status: row.status.clone(),
            label: subagent_label(row),
        });
    }
    for job in &app.bg_jobs {
        rows.push(TaskRow {
            id: None,
            glyph: status_glyph(&job.status, spin),
            status: job.status.clone(),
            label: bg_label(job),
        });
    }
    rows
}

fn subagent_label(row: &SubagentUi) -> String {
    let elapsed = if !row.is_running() && row.elapsed_ms > 0 {
        format!(" {:.1}s", row.elapsed_ms as f64 / 1000.0)
    } else if row.is_running() && !row.activity.is_empty() {
        format!(" — {}", row.activity)
    } else {
        String::new()
    };
    format!("{} · {}{elapsed}", row.kind, first_line(&row.description))
}

fn bg_label(job: &BgJobUi) -> String {
    let elapsed = if job.elapsed_ms > 0 {
        format!(" {:.1}s", job.elapsed_ms as f64 / 1000.0)
    } else {
        String::new()
    };
    format!("bg · {}{elapsed}", first_line(&job.summary))
}

fn item_line_indented(
    row: &TaskRow,
    palette: &ThemePalette,
    width: u16,
    side: u16,
) -> Line<'static> {
    let indent = format!("{}  ", " ".repeat(side as usize));
    let prefix_cols = 2 + row.glyph.width() + 1;
    let inner = (width as usize).saturating_sub((side as usize).saturating_mul(2));
    let max = inner.saturating_sub(prefix_cols).max(8);
    Line::from(vec![
        Span::raw(indent),
        Span::styled(format!("{} ", row.glyph), icon_style(&row.status, palette)),
        Span::styled(truncate(&row.label, max), text_style(&row.status, palette)),
    ])
}

fn header_line(app: &TuiApp, palette: &ThemePalette, width: u16, side: u16) -> Line<'static> {
    let total = app.task_count();
    let done = app.task_terminal_count();
    let collapsed = is_collapsed(app);
    let all_done = app.all_tasks_terminal();
    let chevron = if collapsed {
        "\u{25b8} " // ▸
    } else {
        "\u{25be} " // ▾
    };
    let mut chevron_style = Style::default().fg(palette.dim);
    let label_style = Style::default().fg(palette.fg).add_modifier(Modifier::BOLD);
    let count_style = if all_done {
        Style::default()
            .fg(palette.success)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(palette.dim)
    };
    if app.tasks_hit.hovered {
        chevron_style = chevron_style.fg(palette.fg);
    }

    let mut spans = Vec::new();
    if side > 0 {
        spans.push(Span::raw(" ".repeat(side as usize)));
    }
    spans.push(Span::styled(chevron, chevron_style));
    spans.push(Span::styled("Tasks", label_style));

    let used: usize = spans.iter().map(|s| s.content.width()).sum();
    let bar_w = HEADER_BAR_CELLS;
    let inner_end = (width as usize).saturating_sub(side as usize);
    if total > 0 && inner_end > used {
        let pct = ((done as f64 / total as f64) * 100.0).round() as u16;
        let stats = format!("{done}/{total} {pct}%");
        let stats_w = stats.width();
        let frac = done as f64 / total as f64;
        let bar_color = if all_done {
            palette.success
        } else if app.running_task_count() > 0 {
            palette.warning
        } else {
            palette.accent
        };
        let with_bar = 1 + stats_w + 1 + bar_w as usize;
        let stats_only = 1 + stats_w;
        if inner_end >= used + with_bar {
            let pad = inner_end.saturating_sub(used + stats_w + 1 + bar_w as usize);
            spans.push(Span::raw(" ".repeat(pad.max(1))));
            spans.push(Span::styled(stats, count_style));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                progress_bar_string(bar_w, frac),
                Style::default().fg(bar_color),
            ));
        } else if inner_end >= used + stats_only {
            let pad = inner_end.saturating_sub(used + stats_w);
            spans.push(Span::raw(" ".repeat(pad.max(1))));
            spans.push(Span::styled(stats, count_style));
        }
    }

    Line::from(spans)
}

/// Paint the sticky tasks panel. Header click folds; item click inspects.
pub fn render_panel(
    frame: &mut Frame,
    area: Rect,
    app: &mut TuiApp,
    palette: &ThemePalette,
    side: u16,
) {
    app.tasks_row_hits.clear();
    if area.height == 0 || app.task_count() == 0 {
        app.tasks_hit.clear();
        return;
    }
    let collapsed = is_collapsed(app) || area.height == 1;
    let rows = collect_rows(app);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(header_line(app, palette, area.width, side));

    if !collapsed {
        let mut budget = area.height.saturating_sub(1) as usize;
        let overflow = rows.len() > MAX_ITEMS.min(budget);
        if overflow {
            budget = budget.saturating_sub(1);
        }
        let take = budget.min(MAX_ITEMS).min(rows.len());
        for (i, row) in rows.iter().take(take).enumerate() {
            lines.push(item_line_indented(row, palette, area.width, side));
            if let Some(id) = row.id.as_ref() {
                app.tasks_row_hits.push((
                    Rect {
                        x: area.x,
                        y: area.y.saturating_add(1 + i as u16),
                        width: area.width,
                        height: 1,
                    },
                    id.clone(),
                ));
            }
        }
        let hidden = rows.len().saturating_sub(take);
        if hidden > 0 {
            let indent = format!("{}  ", " ".repeat(side as usize));
            lines.push(Line::from(Span::styled(
                format!("{indent}… +{hidden} more"),
                Style::default().fg(palette.dim),
            )));
        }
    }

    let header = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    app.tasks_hit.set_rect(Some(header));

    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::default().bg(palette.status_bar_bg)),
        area,
    );
}

/// Sidebar Agents tab — same glyphs as the sticky panel, compact list.
pub fn agent_lines(app: &TuiApp, palette: &ThemePalette) -> Vec<Line<'static>> {
    if app.task_count() == 0 {
        return vec![Line::from(Span::styled(
            " No tasks ",
            Style::default().fg(palette.dim),
        ))];
    }
    let mut lines = Vec::new();
    for row in collect_rows(app) {
        lines.push(item_line_indented(&row, palette, 40, 0));
    }
    lines.push(Line::from(Span::styled(
        " Enter / click a row to inspect ",
        Style::default().fg(palette.dim),
    )));
    lines
}

/// Fullscreen framed child transcript (Grok subagent view).
pub fn render_frame(frame: &mut Frame, area: Rect, app: &TuiApp, palette: &ThemePalette) {
    let Some(id) = app.open_subagent.as_deref() else {
        return;
    };
    let Some(row) = app.subagents.iter().find(|s| s.id == id) else {
        return;
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette.accent))
        .title(format!(
            " {} {} · {}  [q]",
            status_glyph(&row.status, app.spinner_frame),
            row.kind,
            truncate(&row.description, 40)
        ))
        .style(Style::default().bg(palette.bg));
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);

    let mut body = row.headline();
    if !row.output.is_empty() {
        body.push_str("\n\n");
        body.push_str(&row.output);
    } else if row.is_running() {
        body.push_str("\n\n(running — output appears when the child finishes)");
    }
    frame.render_widget(
        Paragraph::new(Text::from(body))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(palette.fg)),
        inner,
    );
}

#[cfg(test)]
mod tests {
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
}
