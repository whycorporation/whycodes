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
#[path = "subagents_tests.rs"]
mod tests;
