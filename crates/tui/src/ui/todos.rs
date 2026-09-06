//! Sticky todo panel under the header (Grok Build-style).
//!
//! Header is always one row (`▸`/`▾ Todos` plus a right-aligned
//! `done/total pct%` track). Click the header (or `t` in scrollback) to fold.
//! Finished items stay in the list so they can be reopened after the
//! panel auto-collapses when everything is done.
//!
//! When the list is longer than [`MAX_ITEMS`], the extra rows scroll in
//! place (wheel over the panel, ↑/↓ after focusing the list). There is no
//! `+N more` overflow line.
//!
//! Glyphs and per-status colors follow Grok's `TodoPane`: hollow square,
//! play triangle, check mark, ballot X — icon color separate from text.

use crate::app::TuiApp;
use crate::theme::ThemePalette;
use crate::ui::progress_bar::progress_bar_string;
use crate::ui::scrollbar::{SCROLLBAR_GAP, SCROLLBAR_GUTTER, ScrollbarColors, paint_scrollbar};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
};
use unicode_width::UnicodeWidthStr;
use whycodes_core::todo::{TodoItem, TodoStatus, all_terminal, terminal_count};

/// Max item rows (not counting the header). Extra items scroll in place.
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
        (1 + shown) as u16
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
    item_line_indented(item, palette, width, 0)
}

fn item_line_indented(
    item: &TodoItem,
    palette: &ThemePalette,
    width: u16,
    side: u16,
) -> Line<'static> {
    let glyph = status_glyph(item.status);
    // Chevron + space is 2 columns; items use the same indent so the glyph
    // column lines up with the header label (Grok tasks-pane rule). Panel
    // path adds `side` so the glyph sits on the shared body column.
    let indent = format!("{}  ", " ".repeat(side as usize));
    let prefix_cols = 2 + glyph.width() + 1;
    let inner = (width as usize).saturating_sub((side as usize).saturating_mul(2));
    let max = inner.saturating_sub(prefix_cols).max(8);
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

fn header_line(app: &TuiApp, palette: &ThemePalette, width: u16, side: u16) -> Line<'static> {
    let total = app.todos.len();
    let done = terminal_count(&app.todos);
    let collapsed = is_collapsed(app);
    let all_done = all_terminal(&app.todos);
    let chevron = if collapsed { "\u{25b8} " } else { "\u{25be} " };
    let mut chevron_style = Style::default().fg(palette.dim);
    let label_style = Style::default().fg(palette.fg).add_modifier(Modifier::BOLD);
    let count_style = if all_done {
        Style::default()
            .fg(palette.success)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(palette.dim)
    };
    // Hover brightens the chevron so the header still reads as clickable,
    // but never underlines — a line under "Todos" fights the compact row.
    if app.todos_hit.hovered {
        chevron_style = chevron_style.fg(palette.fg);
    }

    let mut spans = Vec::new();
    if side > 0 {
        spans.push(Span::raw(" ".repeat(side as usize)));
    }
    spans.push(Span::styled(chevron, chevron_style));
    spans.push(Span::styled("Todos", label_style));

    let used: usize = spans.iter().map(|s| s.content.width()).sum();
    let bar_w = HEADER_BAR_CELLS;
    // Text (and the right-aligned track) stay inside [side, width - side).
    let inner_end = (width as usize).saturating_sub(side as usize);
    if total > 0 && inner_end > used {
        let pct = ((done as f64 / total as f64) * 100.0).round() as u16;
        let stats = format!("{done}/{total} {pct}%");
        let stats_w = stats.width();
        let frac = done as f64 / total as f64;
        let bar_color = if all_done {
            palette.success
        } else if app.todos.iter().any(|t| t.status == TodoStatus::InProgress) {
            palette.warning
        } else {
            palette.accent
        };
        // Prefer `2/4 50% ████░░░░`; drop the track if the row is too tight.
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

pub fn render_panel(
    frame: &mut Frame,
    area: Rect,
    app: &mut TuiApp,
    palette: &ThemePalette,
    side: u16,
) {
    if area.height == 0 || app.todos.is_empty() {
        app.todos_hit.clear();
        app.todos_body_hit.clear();
        app.todos_scrollbar_hit.clear();
        app.todos_viewport_rows = 0;
        return;
    }
    let collapsed = is_collapsed(app) || area.height == 1;

    let header = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    app.todos_hit.set_rect(Some(header));

    let mut lines: Vec<Line> = Vec::new();
    lines.push(header_line(app, palette, area.width, side));

    if collapsed {
        app.todos_body_hit.clear();
        app.todos_scrollbar_hit.clear();
        app.todos_viewport_rows = 0;
        frame.render_widget(
            Paragraph::new(Text::from(lines)).style(Style::default().bg(palette.status_bar_bg)),
            area,
        );
        return;
    }

    let vis = (area.height.saturating_sub(1) as usize).min(MAX_ITEMS);
    let body = Rect {
        x: area.x,
        y: area.y.saturating_add(1),
        width: area.width,
        height: vis as u16,
    };
    app.todos_viewport_rows = vis;
    app.clamp_todos_scroll();

    let overflowing = app.todos.len() > vis;
    let reserved = if overflowing {
        SCROLLBAR_GUTTER.saturating_add(SCROLLBAR_GAP)
    } else {
        0
    };
    let text_w = area.width.saturating_sub(reserved);
    let start = app.todos_scroll;
    let take = vis.min(app.todos.len().saturating_sub(start));
    for item in app.todos.iter().skip(start).take(take) {
        lines.push(item_line_indented(item, palette, text_w, side));
    }

    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::default().bg(palette.status_bar_bg)),
        area,
    );

    if overflowing && body.width > 0 && vis > 0 {
        let colors = ScrollbarColors::from_palette(palette);
        let sb = Rect {
            x: area.x + area.width.saturating_sub(SCROLLBAR_GUTTER),
            y: body.y,
            width: SCROLLBAR_GUTTER,
            height: body.height,
        };
        paint_scrollbar(
            frame.buffer_mut(),
            sb,
            app.todos.len(),
            vis,
            start,
            colors.track,
            colors.thumb,
        );
        app.todos_scrollbar_hit.set_rect(Some(sb));
        app.todos_body_hit.set_rect(Some(Rect {
            x: body.x,
            y: body.y,
            width: body.width.saturating_sub(reserved),
            height: body.height,
        }));
    } else {
        app.todos_scrollbar_hit.clear();
        app.todos_body_hit.set_rect(Some(body));
    }
}

#[cfg(test)]
#[path = "todos_tests.rs"]
mod tests;
