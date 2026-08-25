// ── ui/chat.rs: session message list ───────────────────────────────────
// User: elevated band + ❯ prefix. Assistant: free-flow body + turn footer.
// Home: centered dual-block logo.

use crate::app::{ChatBlock, ChatRole, TuiApp};
use crate::theme::ThemePalette;
use crate::tokens::{HOME_LOGO_CODE, HOME_LOGO_WHY, layout};
use crate::ui::scrollbar::{SCROLLBAR_GUTTER, ScrollbarColors, paint_scrollbar};
use crate::widgets::wrap::wrap_text;
#[cfg(test)]
use ratatui::widgets::Widget;
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
};
use std::sync::Arc;
use unicode_width::UnicodeWidthStr;
use whycode_format::diff::{looks_like_diff, parse_diff_line};
use whycode_format::highlight::{detect_language, highlight_code_spans};

pub fn render(frame: &mut Frame, area: Rect, app: &mut TuiApp, palette: &ThemePalette) {
    if app.messages.is_empty() {
        app.clear_chat_hits();
        render_home(frame, area, app, palette);
        return;
    }
    render_session(frame, area, app, palette);
}

/// Display rows the session view occupies at this width.
///
/// Immutable entry: re-renders every message (tests / one-shot). Prefer
/// [`session_line_count_mut`] on the hot scroll path so heights cache.
pub fn session_line_count(app: &TuiApp, width: u16) -> usize {
    message_row_layout(app, width).1
}

/// Like [`session_line_count`] but fills per-message height caches.
pub fn session_line_count_mut(app: &mut TuiApp, width: u16) -> usize {
    message_row_layout_mut(app, width).1
}

/// Per-message start row (top-origin) and total display rows.
///
/// Used for selection → viewport sync (Grok scrollback entry selection).
pub fn message_row_layout(app: &TuiApp, width: u16) -> (Vec<usize>, usize) {
    let palette = app.config.palette();
    let mut starts = Vec::with_capacity(app.messages.len());
    let mut total = 0;
    for (i, msg) in app.messages.iter().enumerate() {
        starts.push(total);
        // Key is (width, closed) — not global `is_busy()`. A new turn must
        // not evict every finished bubble's height (that froze scroll).
        let closed = message_is_closed(app, i);
        if let Some((w, c, h)) = msg.layout_cache
            && w == width
            && c == closed
        {
            total += h;
            continue;
        }
        if let Some((w, c, ref lines)) = msg.line_cache
            && w == width
            && c == closed
        {
            total += lines.len();
            continue;
        }
        total += render_message(msg, app, &palette, i, width, None, false).len();
    }
    (starts, total)
}

/// Like [`message_row_layout`] but writes height / line caches on miss.
pub fn message_row_layout_mut(app: &mut TuiApp, width: u16) -> (Vec<usize>, usize) {
    let n = app.messages.len();
    let mut starts = Vec::with_capacity(n);
    let mut total = 0;
    for i in 0..n {
        starts.push(total);
        let closed = message_is_closed(app, i);
        let h = if let Some((w, c, h)) = app.messages[i].layout_cache
            && w == width
            && c == closed
        {
            h
        } else if let Some((w, c, ref lines)) = app.messages[i].line_cache
            && w == width
            && c == closed
        {
            let h = lines.len();
            app.messages[i].layout_cache = Some((width, closed, h));
            h
        } else if !closed {
            // Live bubble: prefix (thinking/tools) is small; markdown lives in
            // IncrementalMarkdown.buf so a growing fence is not cloned here.
            refresh_live_markdown(app, i, width);
            let prefix = {
                let palette = app.config.palette();
                render_message_live(app, i, &palette, width, false)
            };
            prefix.len() + live_md_len(app, i)
        } else {
            let lines = {
                let palette = app.config.palette();
                render_message_live(app, i, &palette, width, true)
            };
            let h = lines.len();
            app.messages[i].layout_cache = Some((width, closed, h));
            app.messages[i].line_cache = Some((width, closed, Arc::new(lines)));
            h
        };
        total += h;
    }
    (starts, total)
}

/// Last user prompt whose first row sits above the viewport (Grok sticky header).
fn last_scrolled_past_user(app: &TuiApp, starts: &[usize], view_start: usize) -> Option<usize> {
    // Binary search to the first message at/after the viewport, then walk
    // backward (Grok `partition_point` on `virtual_y`).
    let above = starts.partition_point(|&s| s < view_start);
    (0..above)
        .rev()
        .find(|&i| matches!(app.messages.get(i).map(|m| &m.role), Some(ChatRole::User)))
}

/// Message index range whose rows intersect `[view_start, view_end)`.
///
/// Grok `compute_paint_window`: `starts` is a prefix-sum of heights, so
/// two `partition_point`s replace a linear scan of the whole transcript.
pub fn visible_message_range(
    starts: &[usize],
    total: usize,
    view_start: usize,
    view_end: usize,
) -> std::ops::Range<usize> {
    if starts.is_empty() || view_start >= view_end || view_start >= total {
        return 0..0;
    }
    let end = starts.partition_point(|&s| s < view_end);
    let first_after = starts.partition_point(|&s| s <= view_start);
    let start = first_after.saturating_sub(1);
    start.min(end)..end
}

/// Visible `[start, end)` range for bottom-anchored scroll.
///
/// `scroll_offset` is rows up from the newest line (`0` = stick to bottom).
/// Matches `TuiApp::scroll_offset` / `ensure_selected_visible`.
pub fn visible_range(total: usize, height: usize, scroll_offset: usize) -> (usize, usize) {
    if total == 0 || height == 0 {
        return (0, 0);
    }
    let max_off = total.saturating_sub(height);
    let off = scroll_offset.min(max_off);
    let end = total - off;
    let start = end.saturating_sub(height);
    (start, end)
}

fn render_home(frame: &mut Frame, area: Rect, app: &TuiApp, palette: &ThemePalette) {
    let recents: Vec<&crate::app::SessionEntry> = app
        .session_list
        .sessions
        .iter()
        .filter(|s| s.live.is_none())
        .take(6)
        .collect();
    let mut lines: Vec<Line> = Vec::new();
    // Logo + meta + recent sessions (Grok welcome). Recents sit under the
    // brand so an empty workspace still centers; a history list pushes up.
    let recents_h = if recents.is_empty() {
        0
    } else {
        2 + recents.len() as u16
    };
    let content_h = 4 + 1 + 2 + 2 + recents_h; // logo + gap + meta + hints + list
    let top = area.height.saturating_sub(content_h) / 2;
    for _ in 0..top {
        lines.push(Line::from(""));
    }

    // Center logo horizontally
    let logo_w = HOME_LOGO_WHY[1].chars().count() + 1 + HOME_LOGO_CODE[1].chars().count();
    let left_pad = area
        .width
        .saturating_sub(logo_w as u16 + 2)
        .saturating_div(2) as usize;
    let pad = " ".repeat(left_pad);

    for i in 0..4 {
        lines.push(Line::from(vec![
            Span::raw(pad.clone()),
            Span::styled(
                HOME_LOGO_WHY[i].to_string(),
                Style::default().fg(palette.dim),
            ),
            Span::raw(" "),
            Span::styled(
                HOME_LOGO_CODE[i].to_string(),
                Style::default().fg(palette.fg).add_modifier(Modifier::BOLD),
            ),
        ]));
    }

    lines.push(Line::from(""));
    let agent_color = app
        .config
        .agent_color(&app.agent_name, app.agent_cycle_idx, palette);
    let meta_part = format!(
        "  ·  {}/{}",
        empty_dash(&app.provider_name),
        empty_dash(&app.model_name)
    );
    lines.push(center_line_colored(
        &app.agent_name,
        &meta_part,
        area.width,
        agent_color,
        palette.dim,
        false,
    ));
    lines.push(center_line(
        &app.project_label,
        area.width,
        palette.dim,
        false,
    ));
    lines.push(Line::from(""));
    if recents.is_empty() {
        let gs = "Get started  /connect".to_string();
        lines.push(center_line(&gs, area.width, palette.fg, false));
    } else {
        lines.push(center_line("recent", area.width, palette.dim, false));
        let list_w = (area.width as usize).clamp(24, 72);
        let left = area.width.saturating_sub(list_w as u16) / 2;
        let pad = " ".repeat(left as usize);
        for s in recents {
            let when = s
                .updated_at
                .map(crate::ui::timefmt::format_relative)
                .unwrap_or_default();
            let time_w = UnicodeWidthStr::width(when.as_str());
            let title_budget = list_w.saturating_sub(time_w.saturating_add(2));
            let title = truncate_home_title(&s.title, title_budget);
            let gap = list_w
                .saturating_sub(UnicodeWidthStr::width(title.as_str()))
                .saturating_sub(time_w);
            lines.push(Line::from(vec![
                Span::raw(pad.clone()),
                Span::styled(title, Style::default().fg(palette.fg)),
                Span::raw(" ".repeat(gap)),
                Span::styled(when, Style::default().fg(palette.dim)),
            ]));
        }
        lines.push(Line::from(""));
        lines.push(center_line(
            "/resume  ·  Enter to open",
            area.width,
            palette.dim,
            false,
        ));
    }

    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::default().bg(palette.bg)),
        area,
    );
}

fn render_session(frame: &mut Frame, area: Rect, app: &mut TuiApp, palette: &ThemePalette) {
    // Shell already applies SIDE_PAD — don't pad again (extra spaces end up in mouse selection).
    // When the transcript overflows, reserve a dedicated right-hand gutter so
    // the solid scrollbar never paints over wrapped text.
    let height = area.height as usize;
    let full_width = area.width;
    let (starts_full, total_full) = message_row_layout_mut(app, full_width);
    let mut needs_bar = total_full > height && area.width > SCROLLBAR_GUTTER;
    let mut content_width = if needs_bar {
        full_width.saturating_sub(SCROLLBAR_GUTTER)
    } else {
        full_width
    };
    let (starts, total) = if needs_bar && content_width != full_width {
        let relayout = message_row_layout_mut(app, content_width);
        // Narrower wrap can grow height. If it still overflows, keep the
        // gutter; if it now fits, give the column back to the transcript.
        if relayout.1 > height {
            relayout
        } else {
            needs_bar = false;
            content_width = full_width;
            (starts_full, total_full)
        }
    } else {
        (starts_full, total_full)
    };
    let (view_start, view_end) = visible_range(total, height, app.scroll_offset);

    // Pin messages to the bottom: empty rows sit above the transcript.
    let visible = view_end.saturating_sub(view_start).min(height);
    let pad = height.saturating_sub(visible);
    let buf = frame.buffer_mut();
    let row = ChatRowPaint {
        x: area.x,
        width: content_width,
        bg: palette.bg,
        caret_style: Style::default()
            .fg(palette.accent)
            .add_modifier(Modifier::BOLD),
    };
    let mut y = area.y;
    for _ in 0..pad {
        paint_chat_row(buf, y, &row, None, false);
        y = y.saturating_add(1);
    }

    let paint = visible_message_range(&starts, total, view_start, view_end);
    for i in paint {
        let msg_start = starts[i];

        let selected =
            app.selected_msg == Some(i) && app.focus == crate::app::FocusPane::Scrollback;
        let closed = message_is_closed(app, i);
        let slice_from = view_start.saturating_sub(msg_start);
        let slice_to_excl = view_end.saturating_sub(msg_start);

        // Cheap Arc clone so we can paint by reference without holding
        // `app.messages` borrowed across a possible cache fill.
        let cached = app.messages[i]
            .line_cache
            .as_ref()
            .and_then(|(w, c, lines)| {
                if *w == content_width && *c == closed {
                    Some(Arc::clone(lines))
                } else {
                    None
                }
            });

        if let Some(ref lines) = cached {
            y = paint_message_slice(
                buf,
                y,
                &row,
                lines.as_slice(),
                slice_from,
                slice_to_excl,
                selected,
            );
            continue;
        }

        if closed {
            let rendered = render_message_live(app, i, palette, content_width, true);
            let arc = Arc::new(rendered);
            let h = arc.len();
            app.messages[i].line_cache = Some((content_width, closed, Arc::clone(&arc)));
            app.messages[i].layout_cache = Some((content_width, closed, h));
            y = paint_message_slice(
                buf,
                y,
                &row,
                arc.as_slice(),
                slice_from,
                slice_to_excl,
                selected,
            );
        } else {
            refresh_live_markdown(app, i, content_width);
            let prefix = render_message_live(app, i, palette, content_width, false);
            let md = app.messages[i]
                .stream_md
                .as_ref()
                .map(|s| s.lines())
                .unwrap_or(&[]);
            y = paint_concat_slices(
                buf,
                y,
                &row,
                &prefix,
                md,
                slice_from..slice_to_excl,
                selected,
            );
        }
    }

    // If layout undershot (stale height), blank any leftover rows so scroll
    // cannot leave ghost glyphs from the previous frame.
    let end_y = area.y.saturating_add(area.height);
    while y < end_y {
        paint_chat_row(buf, y, &row, None, false);
        y = y.saturating_add(1);
    }

    // Grok sticky_headers: pin the last user prompt scrolled past the top.
    if pad == 0
        && let Some(idx) = last_scrolled_past_user(app, &starts, view_start)
    {
        let selected =
            app.selected_msg == Some(idx) && app.focus == crate::app::FocusPane::Scrollback;
        let sticky = sticky_user_lines(&app.messages[idx], palette, content_width, selected);
        let mut sy = area.y;
        for line in &sticky {
            if sy >= end_y {
                break;
            }
            paint_chat_row(buf, sy, &row, Some(line), false);
            sy = sy.saturating_add(1);
        }
    }

    let scrollbar_hit = if needs_bar {
        // Use our proportional painter — not ratatui::Scrollbar. Ratatui's
        // thumb math (`position * track / (content-1 + viewport)`) parks the
        // handle around ~70% when `position = view_start` at the last page,
        // so "at bottom" never looked like the bottom. Ours maps
        // top-origin `view_start` with `thumb_pos = view_start * travel / max_off`,
        // matching drag math in `ui/scrollbar` (0 → top, max_off → flush bottom).
        let sb = Rect {
            x: area.x + area.width.saturating_sub(SCROLLBAR_GUTTER),
            y: area.y,
            width: SCROLLBAR_GUTTER,
            height: area.height,
        };
        let colors = ScrollbarColors::from_palette(palette);
        paint_scrollbar(
            frame.buffer_mut(),
            sb,
            total,
            height,
            view_start,
            colors.track,
            colors.thumb,
        );
        Some(sb)
    } else {
        None
    };

    app.apply_chat_paint(area, scrollbar_hit, total);
    // Scroll math / ensure_selected_visible wrap at the text column, not the
    // overlayed bar. Keep the published width in sync with `content_width`.
    if needs_bar {
        app.chat_content_width = content_width;
    }
}

/// Paint chat lines, filling every row so previous-frame glyphs cannot linger.
///
/// History: a pure sparse writer (only non-empty spans) left ghost cells after
/// scroll. Production paint now writes rows directly; this widget remains for
/// unit tests that stamp a buffer without a full session.
#[cfg(test)]
struct SparseLines {
    lines: Vec<Line<'static>>,
    bg: ratatui::style::Color,
}

#[cfg(test)]
impl Widget for SparseLines {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let row = ChatRowPaint {
            x: area.x,
            width: area.width,
            bg: self.bg,
            caret_style: Style::default(),
        };
        for r in 0..area.height {
            let line = self.lines.get(r as usize);
            paint_chat_row(buf, area.y + r, &row, line, false);
        }
    }
}

struct ChatRowPaint {
    x: u16,
    width: u16,
    bg: Color,
    caret_style: Style,
}

fn line_has_content(line: &Line<'_>) -> bool {
    line.spans.iter().any(|s| !s.content.is_empty())
}

/// Stamp `lines[from..to]` at `y`. Returns the next free row.
fn paint_message_slice(
    buf: &mut Buffer,
    y: u16,
    row: &ChatRowPaint,
    lines: &[Line<'static>],
    from: usize,
    to_excl: usize,
    selected: bool,
) -> u16 {
    paint_concat_slices(buf, y, row, lines, &[], from..to_excl, selected)
}

/// Paint `a` then `b` as one transcript, clipping to `view`.
fn paint_concat_slices(
    buf: &mut Buffer,
    mut y: u16,
    row: &ChatRowPaint,
    a: &[Line<'static>],
    b: &[Line<'static>],
    view: std::ops::Range<usize>,
    selected: bool,
) -> u16 {
    let total = a.len() + b.len();
    let from = view.start;
    let to = view.end.min(total);
    if from >= to {
        return y;
    }
    let caret_at = if selected {
        a.iter()
            .position(line_has_content)
            .or_else(|| b.iter().position(line_has_content).map(|i| i + a.len()))
    } else {
        None
    };
    for abs in from..to {
        let line = if abs < a.len() {
            &a[abs]
        } else {
            &b[abs - a.len()]
        };
        paint_chat_row(buf, y, row, Some(line), caret_at == Some(abs));
        y = y.saturating_add(1);
    }
    y
}

/// Write one transcript row. Trailing cells are filled so a shorter line
/// after scroll cannot leave leftover glyphs. Band backgrounds (user prompt,
/// diff add/remove) wash the full width.
fn paint_chat_row(
    buf: &mut Buffer,
    y: u16,
    row: &ChatRowPaint,
    line: Option<&Line<'_>>,
    caret: bool,
) {
    if row.width == 0 {
        return;
    }
    let x0 = row.x;
    let end = x0.saturating_add(row.width);
    let clear = Style::default().fg(row.bg).bg(row.bg);
    let mut x = x0;
    let mut band_bg: Option<Color> = None;

    if caret {
        let remaining = end.saturating_sub(x);
        if remaining > 0 {
            buf.set_stringn(x, y, "▌", remaining as usize, row.caret_style);
            x = x.saturating_add(1);
        }
    }

    if let Some(line) = line {
        for span in &line.spans {
            if let Some(span_bg) = span.style.bg {
                band_bg = Some(span_bg);
            }
            if x >= end {
                break;
            }
            let text = span.content.as_ref();
            if text.is_empty() {
                continue;
            }
            let remaining = end - x;
            buf.set_stringn(x, y, text, remaining as usize, span.style);
            let w = text.width().min(remaining as usize) as u16;
            x = x.saturating_add(w);
        }
    }

    while x < end {
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_symbol(" ");
            cell.set_style(clear);
        }
        x = x.saturating_add(1);
    }

    if let Some(band) = band_bg {
        for cx in x0..end {
            if let Some(cell) = buf.cell_mut((cx, y)) {
                if cell.symbol().is_empty() {
                    cell.set_symbol(" ");
                }
                // Only the background — `set_style` can drop RGB token
                // colours on some ratatui versions when reconstructed.
                cell.set_bg(band);
            }
        }
    }
}

/// Closed = not the live streaming tail (and no open thinking).
///
/// Only closed bubbles get a line cache so spinner frames do not re-parse
/// finished markdown. The last assistant message while `app.is_busy()` is open.
fn message_is_closed(app: &TuiApp, index: usize) -> bool {
    let Some(msg) = app.messages.get(index) else {
        return false;
    };
    // Any open thinking block is still streaming.
    for b in &msg.blocks {
        if let ChatBlock::Thinking(t) = b
            && t.is_running()
        {
            return false;
        }
    }
    // While the agent is busy, the last assistant bubble may still grow.
    if app.is_busy() && index + 1 == app.messages.len() && matches!(msg.role, ChatRole::Assistant) {
        return false;
    }
    true
}

fn live_md_width(width: u16) -> usize {
    (width as usize).saturating_sub(4).max(20)
}

fn live_md_len(app: &TuiApp, index: usize) -> usize {
    app.messages
        .get(index)
        .and_then(|m| m.stream_md.as_ref())
        .map(|s| s.lines().len())
        .unwrap_or(0)
}

/// Refresh the live assistant's incremental markdown buffer (no Line clone).
fn refresh_live_markdown(app: &mut TuiApp, index: usize, width: u16) {
    let live = app
        .messages
        .get(index)
        .is_some_and(|m| matches!(m.role, ChatRole::Assistant) && !message_is_closed(app, index));
    if !live {
        return;
    }
    if app.messages[index].stream_md.is_none() {
        app.messages[index].stream_md = Some(crate::md_stream::IncrementalMarkdown::default());
    }
    let palette = app.config.palette();
    let md_width = live_md_width(width);
    let mut stream = app.messages[index].stream_md.take();
    if let Some(inc) = stream.as_mut() {
        inc.render(&app.messages[index].content, &palette, Some(md_width));
        let ts = message_clock(&app.messages[index]);
        stamp_first_content_line(
            inc.lines_mut(),
            Some(&ts),
            Style::default().fg(palette.dim),
            width,
        );
    }
    app.messages[index].stream_md = stream;
}

/// Render message `index`, threading the Grok checkpoint cache on live
/// assistant bubbles so a growing reply does not re-parse frozen blocks.
///
/// `include_live_md`: when false, skip the growing answer (caller paints
/// [`IncrementalMarkdown::lines`] by reference).
fn render_message_live(
    app: &mut TuiApp,
    index: usize,
    palette: &ThemePalette,
    width: u16,
    include_live_md: bool,
) -> Vec<Line<'static>> {
    if app
        .messages
        .get(index)
        .is_some_and(|m| matches!(m.role, ChatRole::Assistant) && !message_is_closed(app, index))
        && app.messages[index].stream_md.is_none()
    {
        app.messages[index].stream_md = Some(crate::md_stream::IncrementalMarkdown::default());
    }
    let mut stream = app.messages[index].stream_md.take();
    let skip_md = !include_live_md && stream.is_some();
    let lines = render_message(
        &app.messages[index],
        app,
        palette,
        index,
        width,
        stream.as_mut(),
        skip_md,
    );
    app.messages[index].stream_md = stream;
    lines
}

fn render_message(
    msg: &crate::app::ChatMessage,
    app: &TuiApp,
    palette: &ThemePalette,
    index: usize,
    width: u16,
    mut stream: Option<&mut crate::md_stream::IncrementalMarkdown>,
    skip_live_md: bool,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    // Blank line between messages
    if index > 0 {
        lines.push(Line::from(""));
    }

    match msg.role {
        ChatRole::User => {
            let selected =
                app.selected_msg == Some(index) && app.focus == crate::app::FocusPane::Scrollback;
            let ts = message_clock(msg);
            lines.extend(user_prompt_lines(
                &msg.content,
                &msg.image_labels,
                Some(&ts),
                palette,
                width,
                selected,
                msg.results_expanded,
            ));
        }
        ChatRole::Assistant => {
            // Turn order (Grok-style, latest answer at the bottom):
            //   thinking → tools → written answer
            // Live text accumulates in `msg.content` and used to be dumped
            // *before* the first tool, so the growing reply sat above the
            // cards and auto-scroll (stick-to-bottom) showed tools, not the
            // answer. Content width for Mermaid: SIDE_PAD + "│ " gutter.
            //
            // Clock on the first answer line, same as the user ❯ row.
            let ts = message_clock(msg);
            let ts_style = Style::default().fg(palette.dim);
            let stamped = std::cell::Cell::new(false);
            let stamp = |chunk: &mut [Line<'static>]| {
                if stamped.get() {
                    return;
                }
                if stamp_first_content_line(chunk, Some(&ts), ts_style, width) {
                    stamped.set(true);
                }
            };
            let md_width = (width as usize).saturating_sub(4).max(20);
            let emit_markdown =
                |lines: &mut Vec<Line<'static>>,
                 text: &str,
                 stream: &mut Option<&mut crate::md_stream::IncrementalMarkdown>| {
                    if skip_live_md || text.is_empty() {
                        return;
                    }
                    let start = lines.len();
                    if let Some(inc) = stream.as_mut() {
                        lines.extend_from_slice(inc.render(text, palette, Some(md_width)));
                    } else {
                        lines.extend(super::markdown::render_with_width(
                            text,
                            palette,
                            Some(md_width),
                        ));
                    }
                    stamp(&mut lines[start..]);
                };

            let paint = ToolPaint {
                is_error: false,
                palette,
                expanded: msg.results_expanded,
                width,
                spin: app.spinner_frame,
            };
            let mut group: Vec<ToolRef<'_>> = Vec::new();
            let flush_group = |lines: &mut Vec<Line<'static>>, group: &mut Vec<ToolRef<'_>>| {
                if group.is_empty() {
                    return;
                }
                lines.extend(paint_tool_run(group, paint));
                group.clear();
            };

            for block in &msg.blocks {
                match block {
                    ChatBlock::Thinking(t) => {
                        flush_group(&mut lines, &mut group);
                        lines.extend(thinking_lines(t, palette, width, app.spinner_frame));
                    }
                    ChatBlock::ToolUse { id, name, input } => {
                        let (result, is_error) = msg
                            .tool_calls
                            .iter()
                            .find(|tc| tc.id == *id)
                            .map(|tc| (tc.result.as_deref(), tc.is_error))
                            .unwrap_or((None, false));
                        let tool = ToolRef {
                            name,
                            input,
                            result,
                            is_error,
                        };
                        if !msg.results_expanded && verb_kind(name).is_some() {
                            group.push(tool);
                        } else {
                            flush_group(&mut lines, &mut group);
                            lines.extend(tool_block(
                                tool.name,
                                tool.input,
                                tool.result,
                                ToolPaint {
                                    is_error: tool.is_error,
                                    ..paint
                                },
                            ));
                        }
                    }
                    ChatBlock::Text(t) if msg.content.is_empty() => {
                        flush_group(&mut lines, &mut group);
                        emit_markdown(&mut lines, t, &mut stream);
                    }
                    ChatBlock::Text(_) | ChatBlock::ToolResult { .. } => {
                        // Live `content` is emitted after every tool. ToolResult
                        // is painted with the matching ToolUse / tool_calls entry.
                    }
                    ChatBlock::Subagent {
                        kind,
                        description,
                        status,
                        activity,
                        elapsed_ms,
                        ..
                    } => {
                        flush_group(&mut lines, &mut group);
                        let bullet = if status == "running" {
                            crate::ui::subagents::SPIN
                                [app.spinner_frame % crate::ui::subagents::SPIN.len()]
                        } else if status == "completed" {
                            "✓"
                        } else {
                            "✗"
                        };
                        let mut line =
                            format!("{bullet} Subagent {status}: \"{description}\" ({kind})");
                        if status == "running" && !activity.is_empty() {
                            line.push_str(" — ");
                            line.push_str(activity);
                        } else if *elapsed_ms > 0 && status != "running" {
                            line.push_str(&format!(" in {:.1}s", *elapsed_ms as f64 / 1000.0));
                        }
                        lines.push(Line::from(Span::styled(
                            line,
                            Style::default().fg(if status == "failed" {
                                palette.error
                            } else if status == "completed" {
                                palette.success
                            } else {
                                palette.accent
                            }),
                        )));
                    }
                }
            }
            for tc in &msg.tool_calls {
                let dup = msg
                    .blocks
                    .iter()
                    .any(|b| matches!(b, ChatBlock::ToolUse { id, .. } if id == &tc.id));
                if dup {
                    continue;
                }
                let tool = ToolRef {
                    name: &tc.name,
                    input: &tc.arguments,
                    result: tc.result.as_deref(),
                    is_error: tc.is_error,
                };
                if !msg.results_expanded && verb_kind(&tc.name).is_some() {
                    group.push(tool);
                } else {
                    flush_group(&mut lines, &mut group);
                    lines.extend(tool_block(
                        tool.name,
                        tool.input,
                        tool.result,
                        ToolPaint {
                            is_error: tool.is_error,
                            ..paint
                        },
                    ));
                }
            }
            flush_group(&mut lines, &mut group);
            // Written answer last — always nearest the prompt / stop row.
            emit_markdown(&mut lines, &msg.content, &mut stream);
            // Turn footer: past tense, muted duration ("Worked for 12s").
            // Provider/model live under the prompt meta row once.
            let is_last = index + 1 == app.messages.len();
            let still_streaming = app.is_busy() && is_last;
            let empty = msg.content.is_empty()
                && msg.blocks.is_empty()
                && msg.tool_calls.is_empty()
                && msg.error.is_none();
            if !empty && !still_streaming {
                let (left, clock) = turn_done_footer(msg, is_last, app);
                let ts_style = Style::default().fg(palette.dim);
                let mut epi = vec![meta_gutter()];
                epi.push(Span::styled(left, ts_style));
                lines.push(Line::from(""));
                lines.push(line_with_right(epi, clock.as_deref(), ts_style, width));
            }
        }
        ChatRole::System => {
            lines.extend(system_callout(&msg.content, palette, width));
        }
        ChatRole::Tool => {
            lines.extend(tool_result(
                &msg.content,
                false,
                palette,
                msg.results_expanded,
                ToolOutHint::Auto,
                width,
            ));
        }
    }

    if let Some(ref err) = msg.error {
        // Quiet callout language (same as system/toasts), not a heavy panel wash.
        lines.push(Line::from(""));
        lines.extend(system_callout(&format!("Error: {err}"), palette, width));
    }

    lines
}

/// Grok accent rail: U+2503 HEAVY VERTICAL (same as `accent_bar()` in Grok pager).
const ACCENT_RAIL: &str = "┃";

/// Render a thinking/reasoning block (Grok Build–style lifecycle).
///
/// Visual match to Grok pager thinking blocks:
/// - Header muted bold (`Thought` / `Thinking…`), detail in plain muted
/// - Body: primary text dimmed (not purple/italic) — reads like quiet terminal
/// - Left `┃` accent column on **every** row while the body is open (Grok paints
///   a full-height accent column beside header + body; none when collapsed)
/// - Running + collapsed → header + live tail (last N lines)
/// - Finished + collapsed → single header line
/// - Expanded → header + full body
fn thinking_lines(
    t: &crate::app::ThinkingBlock,
    palette: &ThemePalette,
    width: u16,
    spin: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    // Grok default: muted bold label, muted detail — never purple italic.
    let label_style = Style::default()
        .fg(palette.dim)
        .add_modifier(Modifier::BOLD);
    let detail_style = Style::default().fg(palette.dim);
    // Body de-emphasis via dim on primary fg (Grok uses bg_blend; dim is the
    // portable stand-in so reasoning still looks like monospaced stream text).
    let body_style = Style::default().fg(palette.fg).add_modifier(Modifier::DIM);
    // Grok: ┃ on the left of Thinking / Thought. Live = thinking-color
    // vertical pulse; finished = dim static.
    let live_rail = t.is_running();
    let rail = AccentRail {
        hot: if live_rail {
            palette.thinking
        } else {
            palette.dim
        },
        rest: palette.dim,
        spin,
        animate: live_rail,
    };
    let rail_at = |row: usize| rail.style(row);
    // Header always has the accent; body only while open (live tail / expand).
    let show_body = t.show_body();
    let content_w = width.saturating_sub(2);

    // Grok: live is `Thinking...` with the timer on the right (`1.4s`).
    // Finished is `Thought for 1.4s` with no chevron on the right.
    let elapsed = t.format_elapsed();
    let header_spans: Vec<Span<'static>> = if t.is_running() {
        vec![Span::styled("Thinking...".to_string(), label_style)]
    } else {
        vec![
            Span::styled("Thought".to_string(), label_style),
            Span::styled(format!(" for {elapsed}"), detail_style),
        ]
    };
    let right = if t.is_running() {
        if elapsed.is_empty() || elapsed == "0.0s" {
            None
        } else {
            Some(elapsed)
        }
    } else {
        None
    };
    let mut rail_row = 0usize;
    let header_line = accent_line(header_spans, true, rail_at(rail_row));
    rail_row += 1;
    if let Some(ref right) = right {
        lines.push(line_with_right(
            header_line.spans,
            Some(right.as_str()),
            detail_style,
            width,
        ));
    } else {
        lines.push(header_line);
    }

    if !show_body {
        return lines;
    }

    let body = t.body_lines();
    if t.is_truncated_live() {
        // Ellipsis row when we dropped earlier reasoning lines.
        lines.push(accent_line(
            vec![Span::styled("…".to_string(), body_style)],
            true,
            rail_at(rail_row),
        ));
        rail_row += 1;
    }
    let wrap_w = content_w.max(8);
    for line in body {
        // Soft-wrap reasoning so long thoughts don't blow the pane (Grok).
        for row in wrap_text(line, wrap_w) {
            let (a, b) = row.byte_range;
            if a >= b {
                continue;
            }
            let slice = line.get(a..b).unwrap_or("").to_string();
            lines.push(accent_line(
                vec![Span::styled(slice, body_style)],
                true,
                rail_at(rail_row),
            ));
            rail_row += 1;
        }
    }
    if t.is_truncated_expanded() {
        lines.push(accent_line(
            vec![Span::styled("…".to_string(), body_style)],
            true,
            rail_at(rail_row),
        ));
    }
    lines
}

/// Grok accent column: status color, a 2-row hot band walking down while live.
#[derive(Clone, Copy)]
struct AccentRail {
    hot: Color,
    rest: Color,
    spin: usize,
    animate: bool,
}

impl AccentRail {
    fn style(self, row: usize) -> Style {
        if !self.animate {
            return Style::default().fg(self.hot);
        }
        let period = 4;
        let head = self.spin % period;
        let dist = (row + period - head) % period;
        let fg = match dist {
            0 | 1 => self.hot,
            _ => self.rest,
        };
        Style::default().fg(fg)
    }
}

/// Shared paint knobs for a tool card (keeps `tool_block` under clippy's arg cap).
#[derive(Clone, Copy)]
struct ToolPaint<'a> {
    is_error: bool,
    palette: &'a ThemePalette,
    expanded: bool,
    width: u16,
    spin: usize,
}

/// One scrollback row with optional Grok-style left accent column.
///
/// ```text
/// ┃ content…     ← show_rail
///   content…     ← collapsed / no accent
/// ```
fn accent_line(content: Vec<Span<'static>>, show_rail: bool, rail_style: Style) -> Line<'static> {
    if !show_rail {
        return Line::from(content);
    }
    let mut spans = Vec::with_capacity(content.len() + 2);
    spans.push(Span::styled(ACCENT_RAIL.to_string(), rail_style));
    spans.push(Span::raw(" "));
    spans.extend(content);
    Line::from(spans)
}

/// Shared left gutter for tools / epilogue (one level under free-flow body).
fn meta_gutter() -> Span<'static> {
    Span::raw(" ".repeat(layout::ASSISTANT_PAD as usize))
}

/// Grok `scrollback.blocks.tool.bullet = "diamond"`.
const TOOL_BULLET: &str = "• ";

/// Put `clock` on the first non-empty line. Returns whether it landed.
fn stamp_first_content_line(
    lines: &mut [Line<'static>],
    clock: Option<&str>,
    style: Style,
    width: u16,
) -> bool {
    let Some(clock) = clock.filter(|s| !s.is_empty()) else {
        return false;
    };
    for line in lines.iter_mut() {
        if !line_has_content(line) {
            continue;
        }
        if line.spans.iter().any(|s| s.content.as_ref() == clock) {
            return true;
        }
        let left = std::mem::take(&mut line.spans);
        *line = line_with_right(left, Some(clock), style, width);
        return true;
    }
    false
}

/// Grok session event: `Worked for 12s` (+ optional token usage on last turn).
///
/// Matches pager `SessionEvent::TurnCompleted` wording — past tense, no agent
/// badge, no `▣`. Cancelled turns are separate system messages.
fn turn_done_footer(
    msg: &crate::app::ChatMessage,
    is_last: bool,
    app: &TuiApp,
) -> (String, Option<String>) {
    let mut s = match msg.duration_ms {
        Some(ms) => format!("Worked for {}", crate::app::format_elapsed_ms(ms)),
        None => "Done.".to_string(),
    };
    // Token summary only for the most recent finished turn (session totals live elsewhere).
    if is_last
        && let Some(ref usage) = app.turn_usage
        && (usage.input_tokens > 0 || usage.output_tokens > 0)
    {
        s.push_str(" · ");
        s.push_str(&crate::app::format_usage_short(usage));
    }
    (s, Some(message_clock(msg)))
}

fn message_clock(msg: &crate::app::ChatMessage) -> String {
    crate::ui::timefmt::format_clock(msg.created_at.unwrap_or_else(chrono::Utc::now))
}

/// Grok `prompt_arrow()`: U+276F HEAVY RIGHT-POINTING ANGLE QUOTATION MARK + space.
/// Always 2 columns wide.
const PROMPT_ARROW: &str = "\u{276F} ";
const PROMPT_ARROW_WIDTH: u16 = 2;

/// Compact sticky header: band pad + first ❯ line (Grok `min_lines = 2`).
fn sticky_user_lines(
    msg: &crate::app::ChatMessage,
    palette: &ThemePalette,
    width: u16,
    is_selected: bool,
) -> Vec<Line<'static>> {
    let ts = message_clock(msg);
    let band = palette.prompt_band(is_selected);
    let band_style = Style::default().bg(band);
    let prefix_style = Style::default().fg(palette.user_msg).bg(band);
    let body_style = Style::default().fg(palette.fg).bg(band);
    let ts_style = Style::default().fg(palette.dim).bg(band);
    let first = msg.content.lines().next().unwrap_or("").trim();
    let ts_reserve = UnicodeWidthStr::width(ts.as_str()).saturating_add(2) as u16;
    let first_w = width
        .saturating_sub(PROMPT_ARROW_WIDTH)
        .saturating_sub(ts_reserve)
        .max(4);
    let slice = if first.is_empty() {
        " ".to_string()
    } else {
        hard_truncate_line(first, first_w as usize)
    };
    vec![
        band_pad_line(band_style),
        line_with_right(
            vec![
                Span::styled(PROMPT_ARROW.to_string(), prefix_style),
                Span::styled(slice, body_style),
            ],
            Some(ts.as_str()),
            ts_style,
            width,
        ),
    ]
}

/// Grok pager `UserPromptBlock` collapsed cap (content rows, not vpad).
const USER_COLLAPSED_MAX_LINES: usize = 3;
/// Grok collapsed ellipsis (`" …"` = space + U+2026).
const USER_ELLIPSIS: &str = " \u{2026}";

/// Grok-style user prompt block.
///
/// ```text
/// ┌──────────────── full-width elevated band ────────────────┐
/// │ ❯ first line of the prompt…                     2:32 PM  │
/// │   soft-wrapped continuation                              │
/// └──────────────────────────────────────────────────────────┘
/// ```
///
/// Matches Grok pager `UserPromptBlock`:
/// - prefix `❯ ` in user accent (Grok `accent_user`)
/// - body primary fg on `bg_light` band (canvas + ~16)
/// - vertical pad rows with the same band
/// - long prompts fold to 3 lines + ` …` until expanded
/// - `/command` tokens pick up the skill/accent color
/// - no left `┃` rail (accent is the arrow, not a border)
fn user_prompt_lines(
    content: &str,
    image_labels: &[String],
    timestamp: Option<&str>,
    palette: &ThemePalette,
    width: u16,
    is_selected: bool,
    expanded: bool,
) -> Vec<Line<'static>> {
    // Grok: elevated band = bg_light; selected steps up slightly.
    let band = palette.prompt_band(is_selected);
    let band_style = Style::default().bg(band);
    let prefix_style = Style::default().fg(palette.user_msg).bg(band);
    let body_style = Style::default().fg(palette.fg).bg(band);
    let skill_style = Style::default().fg(palette.accent).bg(band);
    let img_style = Style::default()
        .fg(palette.accent)
        .bg(band)
        .add_modifier(Modifier::DIM);

    let mut body_lines: Vec<Line<'static>> = Vec::new();
    let ts_style = Style::default().fg(palette.dim).bg(band);
    let push_first = |lines: &mut Vec<Line<'static>>, prefix: &str, body: &str, body_st: Style| {
        let mut left = vec![Span::styled(prefix.to_string(), prefix_style)];
        left.extend(prompt_body_spans(body, body_st, skill_style));
        lines.push(line_with_right(left, timestamp, ts_style, width));
    };
    let push_cont = |lines: &mut Vec<Line<'static>>, body: &str| {
        let mut spans = vec![Span::styled("  ".to_string(), prefix_style)];
        spans.extend(prompt_body_spans(body, body_style, skill_style));
        lines.push(Line::from(spans));
    };

    // Image attachment chips (file names from drag-drop / paste).
    if !image_labels.is_empty() {
        let chips = image_labels
            .iter()
            .map(|l| format!("🖼 {l}"))
            .collect::<Vec<_>>()
            .join("  ");
        push_first(&mut body_lines, PROMPT_ARROW, &chips, img_style);
    }

    // Gap before the clock + 1-col gutter so a scrollbar / band edge
    // cannot eat the last digit when you scroll up into history.
    let ts_reserve = timestamp
        .map(|t| UnicodeWidthStr::width(t).saturating_add(2))
        .unwrap_or(0) as u16;
    let first_w = width
        .saturating_sub(PROMPT_ARROW_WIDTH)
        .saturating_sub(ts_reserve)
        .max(4);
    let content_w = width.saturating_sub(PROMPT_ARROW_WIDTH).max(4) as usize;
    let text = content.trim_end_matches('\n');
    // Skip redundant "[Image: …]" body when labels already render chips and
    // content is the synthetic image-only placeholder.
    let skip_body =
        !image_labels.is_empty() && (text.starts_with("[Image:") || text.starts_with("[Images:"));
    if skip_body {
        let mut lines = vec![band_pad_line(band_style)];
        lines.extend(body_lines);
        lines.push(band_pad_line(band_style));
        return lines;
    }

    if text.is_empty() {
        if image_labels.is_empty() {
            push_first(&mut body_lines, PROMPT_ARROW, " ", body_style);
        }
    } else {
        // Soft-wrap per logical line so explicit newlines stay as hard breaks
        // (same shape as Grok wrap_prompt_lines).
        // First visual row is shorter so the clock fits on the right.
        let mut first_visual = image_labels.is_empty();
        for logical in text.split('\n') {
            if logical.is_empty() {
                if first_visual {
                    push_first(&mut body_lines, PROMPT_ARROW, "", body_style);
                    first_visual = false;
                } else {
                    body_lines.push(Line::from(vec![Span::styled(
                        " ".repeat(PROMPT_ARROW_WIDTH as usize),
                        prefix_style,
                    )]));
                }
                continue;
            }
            if first_visual {
                let head = crate::widgets::wrap::wrap_text(logical, first_w);
                let Some(row) = head.first() else {
                    continue;
                };
                let slice = logical[row.byte_range.0..row.byte_range.1].trim_end();
                push_first(&mut body_lines, PROMPT_ARROW, slice, body_style);
                first_visual = false;
                let remain = logical[row.byte_range.1..].trim_start();
                if remain.is_empty() {
                    continue;
                }
                for row in crate::widgets::wrap::wrap_text(remain, content_w as u16) {
                    let slice = remain[row.byte_range.0..row.byte_range.1].trim_end();
                    push_cont(&mut body_lines, slice);
                }
                continue;
            }
            let wrapped = crate::widgets::wrap::wrap_text(logical, content_w as u16);
            if wrapped.is_empty() {
                continue;
            }
            for row in wrapped {
                let slice = logical[row.byte_range.0..row.byte_range.1].trim_end();
                push_cont(&mut body_lines, slice);
            }
        }
        if first_visual {
            push_first(&mut body_lines, PROMPT_ARROW, " ", body_style);
        }
    }

    // Grok default: fold past 3 visual lines; `e`/`l` expands.
    if !expanded && body_lines.len() > USER_COLLAPSED_MAX_LINES {
        body_lines.truncate(USER_COLLAPSED_MAX_LINES);
        if let Some(last) = body_lines.last_mut() {
            append_user_ellipsis(last, body_style, width);
        }
    }

    let mut lines = Vec::with_capacity(body_lines.len() + 2);
    // vpad top / bottom (Grok PromptConfig.vpad = true)
    lines.push(band_pad_line(band_style));
    lines.extend(body_lines);
    lines.push(band_pad_line(band_style));
    lines
}

/// Highlight `/command` tokens the way Grok paints `accent_skill`.
fn prompt_body_spans(text: &str, body: Style, skill: Style) -> Vec<Span<'static>> {
    if text.is_empty() {
        return vec![Span::styled(String::new(), body)];
    }
    let bytes = text.as_bytes();
    let mut spans = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        let at_token = bytes[i] == b'/'
            && (i == 0 || bytes[i - 1].is_ascii_whitespace())
            && i + 1 < bytes.len()
            && bytes[i + 1].is_ascii_alphabetic();
        if !at_token {
            i += 1;
            continue;
        }
        if i > start {
            spans.push(Span::styled(text[start..i].to_string(), body));
        }
        let mut j = i + 2;
        while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'-') {
            j += 1;
        }
        spans.push(Span::styled(text[i..j].to_string(), skill));
        start = j;
        i = j;
    }
    if start < text.len() {
        spans.push(Span::styled(text[start..].to_string(), body));
    }
    if spans.is_empty() {
        spans.push(Span::styled(text.to_string(), body));
    }
    spans
}

fn append_user_ellipsis(line: &mut Line<'static>, style: Style, width: u16) {
    let ew = UnicodeWidthStr::width(USER_ELLIPSIS);
    truncate_spans_to(&mut line.spans, (width as usize).saturating_sub(ew));
    line.spans
        .push(Span::styled(USER_ELLIPSIS.to_string(), style));
}

fn truncate_home_title(title: &str, budget: usize) -> String {
    if budget == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(title) <= budget {
        return title.to_string();
    }
    if budget <= 1 {
        return "…".to_string();
    }
    let mut out = String::new();
    let mut w = 0usize;
    for ch in title.chars() {
        let cw = UnicodeWidthStr::width(ch.to_string().as_str());
        if w + cw + 1 > budget {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}

/// One empty elevated-band row (Grok prompt vertical pad).
fn band_pad_line(band_style: Style) -> Line<'static> {
    // A single space carries the bg so SparseLines can full-width fill the row.
    Line::from(Span::styled(" ".to_string(), band_style))
}

/// Left content + right-aligned clock (`2:32 PM`).
///
/// The last column is left empty so a transcript scrollbar cannot paint
/// over the clock. If the left side is still too wide, it is truncated —
/// the clock is never dropped (the first, often longest, user bubble
/// used to lose it).
fn line_with_right(
    mut left: Vec<Span<'static>>,
    right: Option<&str>,
    right_style: Style,
    width: u16,
) -> Line<'static> {
    let Some(right) = right.filter(|s| !s.is_empty()) else {
        return Line::from(left);
    };
    let rw = UnicodeWidthStr::width(right);
    // 1-col gap before the clock, 1-col gutter after it.
    let align_w = (width as usize).saturating_sub(1);
    let max_left = align_w.saturating_sub(rw.saturating_add(1));
    truncate_spans_to(&mut left, max_left);
    let left_w: usize = left
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum();
    let gap = align_w.saturating_sub(left_w).saturating_sub(rw);
    if gap > 0 {
        let pad_style = left.last().map(|s| s.style).unwrap_or_default();
        left.push(Span::styled(" ".repeat(gap), pad_style));
    }
    left.push(Span::styled(right.to_string(), right_style));
    Line::from(left)
}

/// Hard-cut `spans` so their display width is at most `max_w`.
fn truncate_spans_to(spans: &mut Vec<Span<'static>>, max_w: usize) {
    if max_w == 0 {
        spans.clear();
        return;
    }
    let mut used = 0usize;
    let mut i = 0usize;
    while i < spans.len() {
        let w = UnicodeWidthStr::width(spans[i].content.as_ref());
        if used + w <= max_w {
            used += w;
            i += 1;
            continue;
        }
        let remain = max_w.saturating_sub(used);
        let style = spans[i].style;
        let cut = cut_to_width(spans[i].content.as_ref(), remain);
        spans.truncate(i);
        if !cut.is_empty() {
            spans.push(Span::styled(cut, style));
        }
        return;
    }
}

fn cut_to_width(s: &str, max_w: usize) -> String {
    if max_w == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(s) <= max_w {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = UnicodeWidthStr::width(ch.to_string().as_str());
        if w + cw > max_w {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out
}

#[derive(Clone, Copy)]
enum CalloutKind {
    Error,
    Warning,
    Success,
    Info,
}

impl CalloutKind {
    fn from_content(content: &str) -> Self {
        let head = content.lines().next().unwrap_or("").to_ascii_lowercase();
        if head.starts_with("error")
            || head.contains("cannot call")
            || head.contains("failed")
            || head.contains("denied")
        {
            Self::Error
        } else if head.contains("no api key")
            || head.contains("missing api key")
            || head.contains("api key")
            || head.starts_with("⚠")
        {
            Self::Warning
        } else if head.starts_with('✓')
            || head.contains("ready")
            || head.contains("loaded")
            || head.contains("success")
            || head.contains("compacted")
        {
            Self::Success
        } else {
            Self::Info
        }
    }

    fn accent(self, palette: &ThemePalette) -> ratatui::style::Color {
        match self {
            Self::Error => palette.error,
            Self::Warning => palette.warning,
            Self::Success => palette.success,
            Self::Info => palette.info,
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Error => "✕",
            Self::Warning => "!",
            Self::Success => "✓",
            Self::Info => "i",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Error => "Error",
            Self::Warning => "Setup",
            Self::Success => "Ready",
            Self::Info => "Note",
        }
    }
}

/// Compact system notices: little vertical space, quiet in the stream, still
/// readable. Title in normal fg; steps dim. No panel fill / bold / padding rows.
fn system_callout(content: &str, palette: &ThemePalette, width: u16) -> Vec<Line<'static>> {
    let kind = CalloutKind::from_content(content);
    let accent = kind.accent(palette);
    let mut lines = Vec::new();

    let mut body: Vec<&str> = content.lines().collect();
    while body.first().is_some_and(|l| l.trim().is_empty()) {
        body.remove(0);
    }
    while body.last().is_some_and(|l| l.trim().is_empty()) {
        body.pop();
    }
    if body.is_empty() {
        return lines;
    }

    let title_text = {
        let first = body[0].trim();
        let stripped = first
            .strip_prefix("Error:")
            .or_else(|| first.strip_prefix("error:"))
            .or_else(|| first.strip_prefix("Warning:"))
            .or_else(|| first.strip_prefix("Cannot call LLM:"))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(first);
        // Avoid "✓ Ready · ✓ …" when the message already carries a glyph.
        stripped
            .trim_start_matches(['✓', '✕', '!', '⚠', '⏹'])
            .trim()
            .to_string()
    };

    // Title: soft accent rail + glyph, quiet label, readable message.
    lines.push(Line::from(vec![
        Span::styled("│ ".to_string(), Style::default().fg(accent)),
        Span::styled(format!("{} ", kind.glyph()), Style::default().fg(accent)),
        Span::styled(
            format!("{} · ", kind.label()),
            Style::default().fg(palette.dim),
        ),
        Span::styled(title_text, Style::default().fg(palette.fg)),
    ]));

    // Body sits under the title. Wrap so a compact summary (or any long
    // notice) is readable instead of one clipped terminal row.
    let wrap_w = width.saturating_sub(4).max(16);
    for line in body.iter().skip(1) {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let wrapped = crate::widgets::wrap::wrap_text(t, wrap_w);
        if wrapped.is_empty() {
            continue;
        }
        for row in wrapped {
            if row.byte_range.0 >= row.byte_range.1 {
                continue;
            }
            let slice = t[row.byte_range.0..row.byte_range.1].trim_end();
            if slice.is_empty() {
                continue;
            }
            lines.push(Line::from(vec![
                Span::styled("│ ".to_string(), Style::default().fg(accent)),
                Span::styled(format!("  {slice}"), Style::default().fg(palette.dim)),
            ]));
        }
    }

    lines
}

/// How to paint a tool result body (Grok-like: diffs + syntax, not flat dim).
#[derive(Debug, Clone)]
enum ToolOutHint {
    /// Inspect content (and fall back to plain).
    Auto,
    /// Unified / edit-preview diff: green `+`, red `-`, cyan hunks.
    Diff,
    /// Syntax-highlight body with this language token (e.g. `"rust"`).
    Code(Option<String>),
    /// Grep hits: path headers + line nos + match highlight (Grok style).
    Grep { pattern: String },
}

fn tool_out_hint(name: &str, input: &serde_json::Value, result: &str) -> ToolOutHint {
    match name {
        "git_diff" | "apply_patch" => ToolOutHint::Diff,
        "edit" | "write" if looks_like_diff(result) => ToolOutHint::Diff,
        "read" | "read_file" => {
            let path = input
                .get("path")
                .or_else(|| input.get("file_path"))
                .or_else(|| input.get("file"))
                .or_else(|| input.get("target_file"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            ToolOutHint::Code(detect_language(path).map(str::to_string))
        }
        "grep" | "search_code" | "rg" => {
            let pattern = input
                .get("pattern")
                .or_else(|| input.get("query"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            ToolOutHint::Grep { pattern }
        }
        _ if looks_like_diff(result) => ToolOutHint::Diff,
        _ => ToolOutHint::Auto,
    }
}

/// `+adds` / `−dels` counts for collapsed tool headers.
fn diff_stat(content: &str) -> (usize, usize) {
    let mut add = 0usize;
    let mut del = 0usize;
    for line in content.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        match parse_diff_line(line).marker {
            Some('+') => add += 1,
            Some('-') => del += 1,
            _ => {}
        }
    }
    (add, del)
}

/// Grok-style display name for tools (`bash`/`shell` → `run`).
fn tool_display_name(name: &str) -> &str {
    match name {
        "bash" | "shell" | "run_terminal_command" => "run",
        "read_file" => "read",
        "search_code" | "rg" => "grep",
        other => other,
    }
}

/// Grok tool header verb: gerund while the call is open, label when done.
fn tool_header_verb(name: &str, running: bool) -> String {
    match (tool_display_name(name), running) {
        ("read", true) => "Reading".into(),
        ("read", false) => "Read".into(),
        ("run", true) => "Running".into(),
        ("run", false) => "Run".into(),
        ("grep", true) => "Searching".into(),
        ("grep", false) => "Searched".into(),
        ("list" | "list_dir", true) => "Listing".into(),
        ("list" | "list_dir", false) => "Listed".into(),
        ("edit" | "write" | "apply_patch", true) => "Editing".into(),
        ("edit" | "write" | "apply_patch", false) => "Edited".into(),
        ("web_fetch" | "webfetch" | "fetch", true) => "Fetching".into(),
        ("web_fetch" | "webfetch" | "fetch", false) => "Fetched".into(),
        ("web_search", true) => "Searching".into(),
        ("web_search", false) => "Searched".into(),
        (_, true) => "Calling".into(),
        (other, false) => {
            let mut chars = other.chars();
            match chars.next() {
                None => "Called".into(),
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VerbKind {
    File,
    Search,
    Dir,
    WebFetch,
    WebSearch,
    Memory,
}

impl VerbKind {
    fn verb(self, running: bool) -> &'static str {
        let (past, present) = match self {
            Self::File => ("Read", "Reading"),
            Self::Search | Self::WebSearch | Self::Memory => ("Searched", "Searching"),
            Self::Dir => ("Listed", "Listing"),
            Self::WebFetch => ("Fetched", "Fetching"),
        };
        if running { present } else { past }
    }

    fn noun(self, count: usize) -> &'static str {
        let (one, many) = match self {
            Self::File => ("file", "files"),
            Self::Search => ("pattern", "patterns"),
            Self::Dir => ("dir", "dirs"),
            Self::WebFetch | Self::WebSearch => ("website", "websites"),
            Self::Memory => ("memory", "memories"),
        };
        if count == 1 { one } else { many }
    }
}

fn verb_kind(name: &str) -> Option<VerbKind> {
    match tool_display_name(name) {
        "read" => Some(VerbKind::File),
        "grep" | "glob" => Some(VerbKind::Search),
        "list" | "list_dir" => Some(VerbKind::Dir),
        "web_search" => Some(VerbKind::WebSearch),
        "web_fetch" | "webfetch" | "fetch" => Some(VerbKind::WebFetch),
        "memory_search" | "memory" => Some(VerbKind::Memory),
        _ => None,
    }
}

struct ToolRef<'a> {
    name: &'a str,
    input: &'a serde_json::Value,
    result: Option<&'a str>,
    is_error: bool,
}

fn paint_tool_run(run: &[ToolRef<'_>], paint: ToolPaint<'_>) -> Vec<Line<'static>> {
    if run.is_empty() {
        return Vec::new();
    }
    if paint.expanded || run.len() == 1 {
        let mut out = Vec::new();
        for t in run {
            out.extend(tool_block(
                t.name,
                t.input,
                t.result,
                ToolPaint {
                    is_error: t.is_error,
                    ..paint
                },
            ));
        }
        return out;
    }
    vec![verb_group_line(run, paint)]
}

fn verb_group_line(run: &[ToolRef<'_>], paint: ToolPaint<'_>) -> Line<'static> {
    let running = run.iter().any(|t| t.result.is_none());
    let failed = run.iter().filter(|t| t.is_error).count();
    let mut buckets: Vec<(VerbKind, usize)> = Vec::new();
    for t in run {
        let Some(kind) = verb_kind(t.name) else {
            continue;
        };
        if let Some(b) = buckets.iter_mut().find(|b| b.0 == kind) {
            b.1 += 1;
        } else {
            buckets.push((kind, 1));
        }
    }
    let mut text = String::new();
    for (i, (kind, count)) in buckets.iter().enumerate() {
        if i > 0 {
            text.push_str(", ");
        }
        text.push_str(kind.verb(running));
        text.push(' ');
        text.push_str(&count.to_string());
        text.push(' ');
        text.push_str(kind.noun(*count));
    }
    if failed > 0 {
        text.push_str(&format!(" · {failed} failed"));
    }
    let style = if failed > 0 {
        Style::default().fg(paint.palette.error)
    } else {
        Style::default().fg(paint.palette.dim)
    };
    Line::from(vec![
        Span::styled(TOOL_BULLET.to_string(), style),
        Span::styled(text, style),
    ])
}

/// Quiet Grok-style tool chrome: `• Read path`. Expanded execute keeps a ┃.
///
/// Execute (`Run`) keeps a status-colored ┃ on the header (and body when
/// open). The rail pulses down the column while the command is running.
///
/// ```text
/// ┃ Thinking...                                          1.4s
/// ┃ …
///   • Read  path/to/file.rs
/// ┃ • Run  cargo test
/// ┃ ok
/// ┃ Thought for 2.1s
/// ```
fn tool_block(
    name: &str,
    input: &serde_json::Value,
    result: Option<&str>,
    paint: ToolPaint<'_>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let running = result.is_none();
    let execute = is_execute_tool(name);
    let display = tool_header_verb(name, running);
    let is_error = paint.is_error;
    let palette = paint.palette;
    let expanded = paint.expanded;
    let width = paint.width;
    let spin = paint.spin;
    let name_style = if is_error {
        Style::default().fg(palette.error)
    } else {
        Style::default().fg(palette.dim)
    };
    let summary_style = if is_error {
        Style::default().fg(palette.error)
    } else {
        Style::default().fg(palette.fg)
    };
    let detail = Style::default().fg(palette.dim);
    let summary = tool_summary(name, input);
    let rail = AccentRail {
        hot: if is_error {
            palette.error
        } else {
            palette.success
        },
        rest: palette.dim,
        spin,
        animate: running,
    };

    let mut header = Vec::new();
    if expanded && execute {
        header.push(Span::styled(ACCENT_RAIL.to_string(), rail.style(0)));
        header.push(Span::raw(" "));
    }
    header.push(Span::styled(TOOL_BULLET.to_string(), name_style));
    header.push(Span::styled(display.to_string(), name_style));
    if !summary.is_empty() {
        header.push(Span::styled(" ".to_string(), detail));
        header.push(Span::styled(summary, summary_style));
    }

    if expanded && let Some(r) = result {
        if is_error {
            header.push(Span::styled("  ✕".to_string(), name_style));
        } else if looks_like_diff(r) {
            let (a, d) = diff_stat(r);
            if a > 0 || d > 0 {
                header.push(Span::styled("  ".to_string(), detail));
                if a > 0 {
                    header.push(Span::styled(
                        format!("+{a}"),
                        Style::default()
                            .fg(palette.diff_add)
                            .add_modifier(Modifier::BOLD),
                    ));
                }
                if a > 0 && d > 0 {
                    header.push(Span::styled(" ".to_string(), detail));
                }
                if d > 0 {
                    header.push(Span::styled(
                        format!("−{d}"),
                        Style::default()
                            .fg(palette.diff_remove)
                            .add_modifier(Modifier::BOLD),
                    ));
                }
            }
        } else if matches!(name, "grep" | "search_code" | "rg")
            && let Some(n) = grep_match_count(r)
        {
            header.push(Span::styled(
                format!("  {n}"),
                Style::default()
                    .fg(palette.highlight)
                    .add_modifier(Modifier::BOLD),
            ));
            header.push(Span::styled(
                if n == 1 {
                    " match".to_string()
                } else {
                    " matches".to_string()
                },
                detail,
            ));
        }
    }

    lines.push(Line::from(header));

    // Grok muted_collapsed: header only until the user expands (`l`).
    if expanded && let Some(r) = result {
        if is_execute_tool(name) {
            lines.extend(tool_result_head_tail(r, is_error, palette, width, rail));
        } else {
            let hint = tool_out_hint(name, input, r);
            lines.extend(tool_result(r, is_error, palette, true, hint, width));
        }
    }
    lines
}

fn is_execute_tool(name: &str) -> bool {
    matches!(name, "bash" | "shell" | "run_terminal_command" | "run")
}

/// Pull `(N match…)` / hit-line count from a grep tool body for the header chip.
fn grep_match_count(content: &str) -> Option<usize> {
    // Footer from GrepTool: `(12 matches in 3 files; pattern \`foo\`)`
    for line in content.lines().rev() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix('(')
            && let Some(num) = rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse::<usize>().ok())
            && (rest.contains("match") || rest.contains("hit"))
        {
            return Some(num);
        }
    }
    // Fallback: count `path:line:…` hit lines (not context `path:line-…`).
    let n = content
        .lines()
        .filter(|l| parse_grep_hit(l).is_some_and(|h| h.is_match))
        .count();
    if n > 0 { Some(n) } else { None }
}

/// Collapsed = header only; expanded (`l`) = body.
/// Grok execute truncated mode: first 2 + last 3.
const TOOL_RESULT_PREVIEW: usize = 12;
const TOOL_RESULT_DIFF_PREVIEW: usize = 20;
const TOOL_RESULT_EXPANDED: usize = 120;
const EXECUTE_FIRST_LINES: usize = 2;
const EXECUTE_LAST_LINES: usize = 3;

fn tool_result(
    content: &str,
    is_error: bool,
    palette: &ThemePalette,
    expanded: bool,
    hint: ToolOutHint,
    width: u16,
) -> Vec<Line<'static>> {
    if content.is_empty() {
        return Vec::new();
    }

    // Pretty-print minified JSON (webfetch / package.json dumps) so the rail
    // shows structure instead of one long wrapped garbage line.
    let prepared = prettify_tool_result(content);
    let content = prepared.as_str();

    let mode = match &hint {
        ToolOutHint::Diff => ToolOutHint::Diff,
        ToolOutHint::Code(lang) => ToolOutHint::Code(lang.clone()),
        ToolOutHint::Grep { pattern } => ToolOutHint::Grep {
            pattern: pattern.clone(),
        },
        ToolOutHint::Auto => {
            if looks_like_diff(content) {
                ToolOutHint::Diff
            } else if looks_like_json_body(content) {
                ToolOutHint::Code(Some("json".to_string()))
            } else if looks_like_grep_body(content) {
                ToolOutHint::Grep {
                    pattern: String::new(),
                }
            } else if let Some(path) = content
                .lines()
                .next()
                .and_then(|l| l.strip_prefix("# "))
                .filter(|l| !l.is_empty())
            {
                ToolOutHint::Code(detect_language(path).map(str::to_string))
            } else {
                ToolOutHint::Auto
            }
        }
    };

    match mode {
        ToolOutHint::Diff => tool_result_diff(content, is_error, palette, expanded, width),
        ToolOutHint::Code(lang) => {
            tool_result_code(content, is_error, palette, expanded, lang, width)
        }
        ToolOutHint::Grep { pattern } => {
            tool_result_grep(content, is_error, palette, expanded, &pattern, width)
        }
        ToolOutHint::Auto => tool_result_plain(content, is_error, palette, expanded, width),
    }
}

/// True when most non-empty lines look like `path:line:…` / `path:line-…` hits.
fn looks_like_grep_body(s: &str) -> bool {
    let mut hits = 0usize;
    let mut lines = 0usize;
    for line in s.lines().take(40) {
        let t = line.trim();
        if t.is_empty() || t == "--" || t.starts_with('(') || t.starts_with('[') {
            continue;
        }
        lines += 1;
        if parse_grep_hit(t).is_some() {
            hits += 1;
        }
    }
    lines > 0 && hits * 2 >= lines
}

/// Soften tool bodies for display: pretty-print JSON payloads (whole body or
/// trailing after webfetch-style headers). Leaves non-JSON text unchanged.
fn prettify_tool_result(content: &str) -> String {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return content.to_string();
    }

    if let Some(pretty) = try_pretty_json(trimmed) {
        return pretty;
    }

    // `URL: …\nStatus: …\nContent-Type: …\n\n{json}` (webfetch)
    if let Some((head, body)) = content.split_once("\n\n") {
        let body_trim = body.trim();
        if let Some(pretty) = try_pretty_json(body_trim) {
            return format!("{}\n\n{}", head.trim_end(), pretty);
        }
    }

    // Prefix noise then a JSON value (shell / partial dumps).
    if let Some(idx) = find_json_value_start(content) {
        let head = content[..idx].trim_end();
        if let Some(pretty) = try_pretty_json(content[idx..].trim()) {
            if head.is_empty() {
                return pretty;
            }
            return format!("{head}\n{pretty}");
        }
    }

    content.to_string()
}

fn try_pretty_json(s: &str) -> Option<String> {
    let s = s.trim();
    if !(s.starts_with('{') || s.starts_with('[')) {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(s).ok()?;
    let pretty = serde_json::to_string_pretty(&value).ok()?;
    // Only rewrite when it actually helps (minified / very long lines).
    let max_line = s.lines().map(|l| l.chars().count()).max().unwrap_or(0);
    let multi = s.lines().count() > 1;
    if multi && max_line <= 100 && pretty.lines().count() <= s.lines().count() + 2 {
        return None;
    }
    Some(pretty)
}

fn find_json_value_start(s: &str) -> Option<usize> {
    // Single attempt from the first brace — avoids O(n) re-parses on large dumps.
    let i = s.find(['{', '['])?;
    if serde_json::from_str::<serde_json::Value>(s[i..].trim()).is_ok() {
        Some(i)
    } else {
        None
    }
}

/// Pure JSON only (not webfetch headers + body) so syntax highlight stays clean.
fn looks_like_json_body(s: &str) -> bool {
    let t = s.trim();
    if !(t.starts_with('{') || t.starts_with('[')) {
        return false;
    }
    serde_json::from_str::<serde_json::Value>(t).is_ok()
}

/// One terminal row per logical line; long lines get a hard `…` (never wrap a
/// minified dump across the whole preview budget).
fn hard_truncate_line(line: &str, max_cols: usize) -> String {
    let max_cols = max_cols.max(4);
    let count = line.chars().count();
    if count <= max_cols {
        return line.to_string();
    }
    format!(
        "{}…",
        line.chars()
            .take(max_cols.saturating_sub(1))
            .collect::<String>()
    )
}

/// Grok execute truncated body: first `head` lines, `…`, last `tail` lines.
fn tool_result_head_tail(
    content: &str,
    is_error: bool,
    palette: &ThemePalette,
    width: u16,
    rail: AccentRail,
) -> Vec<Line<'static>> {
    let head = EXECUTE_FIRST_LINES;
    let tail = EXECUTE_LAST_LINES;
    let color = if is_error { palette.error } else { palette.dim };
    let style = Style::default().fg(color);
    let text_w = width.saturating_sub(4).max(8) as usize;
    let all: Vec<&str> = content.lines().collect();
    let total = all.len();
    let mut lines = Vec::new();
    let mut row = 1usize; // header already used row 0
    let push = |lines: &mut Vec<Line<'static>>, text: &str, row: usize| {
        lines.push(Line::from(vec![
            meta_gutter(),
            Span::styled(ACCENT_RAIL.to_string(), rail.style(row)),
            Span::raw(" "),
            Span::styled(hard_truncate_line(text, text_w), style),
        ]));
    };
    if total <= head.saturating_add(tail) {
        for line in &all {
            push(&mut lines, line, row);
            row += 1;
        }
        return lines;
    }
    for line in all.iter().take(head) {
        push(&mut lines, line, row);
        row += 1;
    }
    lines.push(Line::from(vec![
        meta_gutter(),
        Span::styled(ACCENT_RAIL.to_string(), rail.style(row)),
        Span::raw(" "),
        Span::styled(
            "…".to_string(),
            Style::default().fg(palette.dim).add_modifier(Modifier::DIM),
        ),
    ]));
    row += 1;
    for line in all.iter().skip(total - tail) {
        push(&mut lines, line, row);
        row += 1;
    }
    lines
}

fn tool_result_plain(
    content: &str,
    is_error: bool,
    palette: &ThemePalette,
    expanded: bool,
    width: u16,
) -> Vec<Line<'static>> {
    let color = if is_error { palette.error } else { palette.dim };
    let style = Style::default().fg(color);
    let rail = Style::default().fg(palette.dim);
    let budget = if expanded {
        TOOL_RESULT_EXPANDED
    } else {
        TOOL_RESULT_PREVIEW
    };
    // Gutter: ASSISTANT_PAD + "┃ " = 4 cols.
    let text_w = width.saturating_sub(4).max(8) as usize;
    let all_lines: Vec<&str> = content.lines().collect();
    let total = all_lines.len();
    let mut lines = Vec::new();
    let mut line_was_cut = false;
    for line in all_lines.iter().take(budget) {
        if line.chars().count() > text_w {
            line_was_cut = true;
        }
        let shown = hard_truncate_line(line, text_w);
        lines.push(Line::from(vec![
            meta_gutter(),
            Span::styled("┃ ".to_string(), rail),
            Span::styled(shown, style),
        ]));
    }
    if total > budget {
        lines.push(Line::from(vec![
            meta_gutter(),
            Span::styled("┃ ".to_string(), rail),
            Span::styled(
                "…".to_string(),
                Style::default().fg(palette.dim).add_modifier(Modifier::DIM),
            ),
        ]));
    } else if line_was_cut {
        lines.push(Line::from(vec![
            meta_gutter(),
            Span::styled("┃ ".to_string(), rail),
            Span::styled(
                "… long lines truncated".to_string(),
                Style::default().fg(palette.dim).add_modifier(Modifier::DIM),
            ),
        ]));
    }
    lines
}

fn tool_result_diff(
    content: &str,
    is_error: bool,
    palette: &ThemePalette,
    expanded: bool,
    width: u16,
) -> Vec<Line<'static>> {
    let budget = if expanded {
        TOOL_RESULT_EXPANDED
    } else {
        TOOL_RESULT_DIFF_PREVIEW
    };
    let total = content.lines().count();
    // Gutter "  " + "┃ " = 4 cols; body budget for hard-truncate.
    let text_w = width.saturating_sub(4).max(8) as usize;
    // Strong green/red band (Grok): full line fg + visible wash bg.
    let add_bg = palette.diff_line_bg(palette.diff_add);
    let rem_bg = palette.diff_line_bg(palette.diff_remove);

    let mut lines = Vec::new();
    for line in content.lines().take(budget) {
        let kind = diff_line_kind(line, is_error);
        let (rail_color, body_color, line_bg) = match kind {
            DiffPaint::Error => (palette.error, palette.error, None),
            DiffPaint::FileHeader => (palette.dim, palette.fg, None),
            DiffPaint::Hunk => (palette.diff_hunk, palette.diff_hunk, None),
            DiffPaint::Add => (palette.diff_add, palette.diff_add, Some(add_bg)),
            DiffPaint::Remove => (palette.diff_remove, palette.diff_remove, Some(rem_bg)),
            DiffPaint::Meta => (palette.dim, palette.fg, None),
            DiffPaint::Context => (palette.dim, palette.dim, None),
        };

        let paint = |fg: Color, bold: bool| {
            let mut s = Style::default().fg(fg);
            if let Some(bg) = line_bg {
                s = s.bg(bg);
            }
            if bold {
                s = s.add_modifier(Modifier::BOLD);
            }
            s
        };

        // Left meta pad must carry the wash so SparseLines full-row fill
        // starts from a painted cell (Grok full-width green/red band).
        let gutter = if line_bg.is_some() {
            Span::styled(
                " ".repeat(layout::ASSISTANT_PAD as usize),
                paint(body_color, false),
            )
        } else {
            meta_gutter()
        };

        let mut spans = vec![
            gutter,
            Span::styled("┃ ".to_string(), paint(rail_color, false)),
        ];

        // Grok-style: line number + marker + body all green/red.
        // Format: `  12|-body` or bare `+body`.
        let parts = parse_diff_line(line);
        if let Some(m) = parts.marker {
            // Leading pad (spaces before digits) shares the wash bg.
            if !parts.line_no_pad.is_empty() {
                spans.push(Span::styled(
                    parts.line_no_pad.to_string(),
                    paint(body_color, false),
                ));
            }
            if let Some(no) = parts.line_no {
                spans.push(Span::styled(no.to_string(), paint(body_color, true)));
                // Separator between number and +/- (present in edit/write previews).
                spans.push(Span::styled("|".to_string(), paint(body_color, false)));
            }
            spans.push(Span::styled(m.to_string(), paint(body_color, true)));
            let used = parts.line_no_pad.chars().count()
                + parts.line_no.map(|s| s.chars().count() + 1).unwrap_or(0)
                + 1;
            let body_budget = text_w.saturating_sub(used).max(1);
            let body_shown = hard_truncate_line(parts.body, body_budget);
            spans.push(Span::styled(body_shown, paint(body_color, false)));
        } else {
            let shown = hard_truncate_line(line, text_w);
            spans.push(Span::styled(shown, paint(body_color, false)));
        }

        // Trailing space carries bg so SparseLines always sees a band even
        // on empty +/− bodies and can stretch the wash full-width.
        if line_bg.is_some() {
            spans.push(Span::styled(" ".to_string(), paint(body_color, false)));
        }

        lines.push(Line::from(spans));
    }
    if total > budget {
        lines.push(Line::from(vec![
            meta_gutter(),
            Span::styled("┃ ".to_string(), Style::default().fg(palette.dim)),
            Span::styled(
                "…".to_string(),
                Style::default().fg(palette.dim).add_modifier(Modifier::DIM),
            ),
        ]));
    }
    lines
}

#[derive(Clone, Copy)]
enum DiffPaint {
    Error,
    FileHeader,
    Hunk,
    Add,
    Remove,
    Meta,
    Context,
}

fn diff_line_kind(line: &str, is_error: bool) -> DiffPaint {
    if is_error {
        return DiffPaint::Error;
    }
    if line.starts_with("+++") || line.starts_with("---") {
        return DiffPaint::FileHeader;
    }
    if line.starts_with("@@") || line.starts_with("diff --git") {
        return DiffPaint::Hunk;
    }
    if line.starts_with("Edited ")
        || line.starts_with("Wrote ")
        || line.starts_with('…')
        || line.starts_with("(empty")
    {
        return DiffPaint::Meta;
    }
    match parse_diff_line(line).marker {
        Some('+') => DiffPaint::Add,
        Some('-') => DiffPaint::Remove,
        _ => DiffPaint::Context,
    }
}

/// Syntax-highlight tool output (read previews). Line-numbered `read` rows
/// keep the gutter dim and highlight only the code after `|`.
///
/// Grok layout:
/// ```text
/// read · path/to/file.rs
/// ┃     1|fn main() {
/// ┃     2|    …
/// ```
/// Meta banners (`# path`, `# lines 1–N`) stay quiet dim; code is syntax-coloured.
fn tool_result_code(
    content: &str,
    is_error: bool,
    palette: &ThemePalette,
    expanded: bool,
    language: Option<String>,
    width: u16,
) -> Vec<Line<'static>> {
    if is_error {
        return tool_result_plain(content, true, palette, expanded, width);
    }

    let budget = if expanded {
        TOOL_RESULT_EXPANDED
    } else {
        TOOL_RESULT_PREVIEW
    };
    // Skip redundant `# path` banner (path already lives in the tool header).
    // Keep `# lines 1–N of M` as a quiet dim meta row when present.
    let all: Vec<&str> = content
        .lines()
        .filter(|l| {
            let t = l.trim();
            // Drop pure path banner: `# crates/foo/bar.rs`
            if let Some(rest) = t.strip_prefix("# ") {
                // Keep range/size meta (`# lines 1–40 of 200  |  4.2 KB`)
                if rest.starts_with("lines ") {
                    return true;
                }
                // Drop path-only banner.
                return false;
            }
            true
        })
        .collect();
    let total = all.len();
    let slice = &all[..total.min(budget)];
    let rail = Style::default().fg(palette.dim);
    // Gutter "  " + "┃ " + typical "   12|" ≈ 4+7 = 11 cols reserved.
    let text_w = width.saturating_sub(11).max(8) as usize;

    let mut code_body = String::new();
    // (is_code, left_gutter_or_meta, code)
    let mut meta: Vec<(bool, String, String)> = Vec::with_capacity(slice.len());
    for line in slice {
        if let Some((gutter, code)) = split_read_line(line) {
            meta.push((true, gutter, code.to_string()));
            code_body.push_str(code);
            code_body.push('\n');
        } else {
            meta.push((false, (*line).to_string(), String::new()));
        }
    }

    let highlighted = if language.is_some() && meta.iter().any(|(is_code, _, _)| *is_code) {
        Some(highlight_code_spans(
            code_body.trim_end_matches('\n'),
            language.as_deref(),
        ))
    } else if language.is_some() && !meta.iter().any(|(is_code, _, _)| *is_code) {
        Some(highlight_code_spans(&slice.join("\n"), language.as_deref()))
    } else {
        None
    };

    let mut lines = Vec::new();
    let mut code_idx = 0usize;
    let mut line_was_cut = false;
    match highlighted {
        Some(hl) if meta.iter().any(|(is_code, _, _)| *is_code) => {
            for (is_code, left, _) in &meta {
                if *is_code {
                    let mut spans = vec![
                        meta_gutter(),
                        Span::styled("┃ ".to_string(), rail),
                        // Grok: dim right-aligned line no + `|` separator.
                        Span::styled(
                            left.clone(),
                            Style::default().fg(palette.dim).add_modifier(Modifier::DIM),
                        ),
                    ];
                    if let Some(row) = hl.get(code_idx) {
                        let mut used = 0usize;
                        for ((r, g, b), text) in row.iter() {
                            let t = text.trim_end_matches('\n');
                            let n = t.chars().count();
                            if used >= text_w {
                                line_was_cut = true;
                                break;
                            }
                            if used + n > text_w {
                                let take = text_w.saturating_sub(used);
                                let shown = hard_truncate_line(t, take);
                                spans.push(Span::styled(
                                    shown,
                                    Style::default().fg(Color::Rgb(*r, *g, *b)),
                                ));
                                line_was_cut = true;
                                break;
                            }
                            spans.push(Span::styled(
                                t.to_string(),
                                Style::default().fg(Color::Rgb(*r, *g, *b)),
                            ));
                            used += n;
                        }
                    }
                    code_idx += 1;
                    lines.push(Line::from(spans));
                } else {
                    // Quiet meta (`# lines …`, truncation notes).
                    let shown = hard_truncate_line(left, text_w.saturating_add(7));
                    lines.push(Line::from(vec![
                        meta_gutter(),
                        Span::styled("┃ ".to_string(), rail),
                        Span::styled(
                            shown,
                            Style::default().fg(palette.dim).add_modifier(Modifier::DIM),
                        ),
                    ]));
                }
            }
        }
        Some(hl) => {
            for row in hl.iter().take(budget) {
                let mut spans = vec![meta_gutter(), Span::styled("┃ ".to_string(), rail)];
                let mut used = 0usize;
                for ((r, g, b), text) in row.iter() {
                    let t = text.trim_end_matches('\n');
                    let n = t.chars().count();
                    if used >= text_w {
                        line_was_cut = true;
                        break;
                    }
                    if used + n > text_w {
                        spans.push(Span::styled(
                            hard_truncate_line(t, text_w.saturating_sub(used)),
                            Style::default().fg(Color::Rgb(*r, *g, *b)),
                        ));
                        line_was_cut = true;
                        break;
                    }
                    spans.push(Span::styled(
                        t.to_string(),
                        Style::default().fg(Color::Rgb(*r, *g, *b)),
                    ));
                    used += n;
                }
                lines.push(Line::from(spans));
            }
        }
        None => {
            // Still paint line-numbered read rows Grok-style even without a grammar.
            if meta.iter().any(|(is_code, _, _)| *is_code) {
                for (is_code, left, code) in &meta {
                    if *is_code {
                        let shown = hard_truncate_line(code, text_w);
                        if code.chars().count() > text_w {
                            line_was_cut = true;
                        }
                        lines.push(Line::from(vec![
                            meta_gutter(),
                            Span::styled("┃ ".to_string(), rail),
                            Span::styled(
                                left.clone(),
                                Style::default().fg(palette.dim).add_modifier(Modifier::DIM),
                            ),
                            Span::styled(shown, Style::default().fg(palette.fg)),
                        ]));
                    } else {
                        lines.push(Line::from(vec![
                            meta_gutter(),
                            Span::styled("┃ ".to_string(), rail),
                            Span::styled(
                                hard_truncate_line(left, text_w.saturating_add(7)),
                                Style::default().fg(palette.dim).add_modifier(Modifier::DIM),
                            ),
                        ]));
                    }
                }
            } else {
                return tool_result_plain(content, is_error, palette, expanded, width);
            }
        }
    }

    if total > budget {
        lines.push(Line::from(vec![
            meta_gutter(),
            Span::styled("┃ ".to_string(), rail),
            Span::styled(
                "…".to_string(),
                Style::default().fg(palette.dim).add_modifier(Modifier::DIM),
            ),
        ]));
    } else if line_was_cut {
        lines.push(Line::from(vec![
            meta_gutter(),
            Span::styled("┃ ".to_string(), rail),
            Span::styled(
                "… long lines truncated".to_string(),
                Style::default().fg(palette.dim).add_modifier(Modifier::DIM),
            ),
        ]));
    }
    lines
}

/// Grok-style grep body: path headers, dim line nos, match highlight on hit text.
///
/// ```text
/// grep · pattern  3 matches
/// ┃ crates/tui/src/ui/chat.rs
/// ┃    12│ matching line with pattern
/// ┃    40│ another hit
/// ┃ crates/tools/src/file/grep.rs
/// ┃    34│ "grep"
/// ```
fn tool_result_grep(
    content: &str,
    is_error: bool,
    palette: &ThemePalette,
    expanded: bool,
    pattern: &str,
    width: u16,
) -> Vec<Line<'static>> {
    if is_error {
        return tool_result_plain(content, true, palette, expanded, width);
    }
    if content.trim().is_empty() || content.trim() == "No matches found." {
        return tool_result_plain(content, false, palette, expanded, width);
    }

    let budget = if expanded {
        TOOL_RESULT_EXPANDED
    } else {
        TOOL_RESULT_PREVIEW
    };
    let rail = Style::default().fg(palette.dim);
    let path_style = Style::default()
        .fg(palette.info)
        .add_modifier(Modifier::BOLD);
    let line_no_style = Style::default().fg(palette.dim).add_modifier(Modifier::DIM);
    let body_style = Style::default().fg(palette.fg);
    let ctx_style = Style::default().fg(palette.dim);
    let hit_style = Style::default()
        .fg(palette.highlight)
        .add_modifier(Modifier::BOLD);
    let meta_style = Style::default().fg(palette.dim).add_modifier(Modifier::DIM);
    // Gutter + rail + "  123│ " ≈ 4 + 7
    let text_w = width.saturating_sub(12).max(8) as usize;

    let re = compile_grep_highlighter(pattern);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut last_path: Option<String> = None;
    let mut logical = 0usize; // rows that count against budget
    let mut truncated = false;
    let all_lines: Vec<&str> = content.lines().collect();
    let total_hits = all_lines
        .iter()
        .filter(|l| parse_grep_hit(l).is_some())
        .count();

    for raw in &all_lines {
        let t = raw.trim_end();
        if t.is_empty() {
            continue;
        }
        // Footer / separator — always dim, don't steal budget from hits.
        if t == "--" {
            if logical >= budget {
                truncated = true;
                break;
            }
            lines.push(Line::from(vec![
                meta_gutter(),
                Span::styled("┃ ".to_string(), rail),
                Span::styled("──".to_string(), meta_style),
            ]));
            logical += 1;
            continue;
        }
        if t.starts_with('(') || t.starts_with('[') || t.starts_with("No matches") {
            // Stat footer — show once after the body, outside budget pressure.
            continue;
        }

        if let Some(hit) = parse_grep_hit(t) {
            // Path header when file changes (Grok groups by file).
            if last_path.as_deref() != Some(hit.path) {
                if logical >= budget {
                    truncated = true;
                    break;
                }
                let path_shown = hard_truncate_line(hit.path, text_w.saturating_add(6));
                lines.push(Line::from(vec![
                    meta_gutter(),
                    Span::styled("┃ ".to_string(), rail),
                    Span::styled(path_shown, path_style),
                ]));
                last_path = Some(hit.path.to_string());
                logical += 1;
            }

            if logical >= budget {
                truncated = true;
                break;
            }

            // Line number gutter: right-align in 5 cols like Grok read.
            let no = format!("{:>5}", hit.lineno);
            let mark = if hit.is_match { '│' } else { '┆' };
            let style = if hit.is_match { body_style } else { ctx_style };
            let mut spans = vec![
                meta_gutter(),
                Span::styled("┃ ".to_string(), rail),
                Span::styled(no, line_no_style),
                Span::styled(mark.to_string(), line_no_style),
            ];
            let body = hard_truncate_line(hit.content, text_w);
            if hit.is_match && re.is_some() {
                spans.extend(paint_grep_match(&body, re.as_ref(), style, hit_style));
            } else if hit.is_match && !pattern.is_empty() {
                spans.extend(paint_grep_literal(&body, pattern, style, hit_style));
            } else {
                spans.push(Span::styled(body, style));
            }
            lines.push(Line::from(spans));
            logical += 1;
        } else {
            if logical >= budget {
                truncated = true;
                break;
            }
            lines.push(Line::from(vec![
                meta_gutter(),
                Span::styled("┃ ".to_string(), rail),
                Span::styled(hard_truncate_line(t, text_w.saturating_add(6)), meta_style),
            ]));
            logical += 1;
        }
    }

    // Quiet footer with total (mirrors tool header chip when collapsed).
    if let Some(footer) = all_lines.iter().rev().find(|l| {
        let t = l.trim();
        t.starts_with('(') || t.starts_with('[')
    }) {
        lines.push(Line::from(vec![
            meta_gutter(),
            Span::styled("┃ ".to_string(), rail),
            Span::styled(footer.trim().to_string(), meta_style),
        ]));
    }

    if truncated {
        let remain = total_hits.saturating_sub(
            lines
                .iter()
                .filter(|l| {
                    l.spans.iter().any(|s| {
                        let c = s.content.as_ref();
                        c.contains('│') || c.contains('┆')
                    })
                })
                .count(),
        );
        // remain is approximate; prefer generic "more lines".
        let _ = remain;
        lines.push(Line::from(vec![
            meta_gutter(),
            Span::styled("┃ ".to_string(), rail),
            Span::styled("…".to_string(), meta_style),
        ]));
    }

    lines
}

/// One parsed `path:line:content` / `path:line-content` hit.
struct GrepHit<'a> {
    path: &'a str,
    lineno: &'a str,
    is_match: bool,
    content: &'a str,
}

/// Parse `path:lineno:content` or context `path:lineno-content`.
fn parse_grep_hit(line: &str) -> Option<GrepHit<'_>> {
    // Scan for `:digits:` or `:digits-` — path may contain `:` (rare / Windows).
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b':' {
            let rest = &line[i + 1..];
            let digit_end = rest.bytes().take_while(|c| c.is_ascii_digit()).count();
            if digit_end > 0 {
                let after_digits = &rest[digit_end..];
                let mark = after_digits.chars().next()?;
                if mark == ':' || mark == '-' {
                    let path = &line[..i];
                    if path.is_empty() {
                        i += 1;
                        continue;
                    }
                    let lineno = &rest[..digit_end];
                    let content = &after_digits[mark.len_utf8()..];
                    return Some(GrepHit {
                        path,
                        lineno,
                        is_match: mark == ':',
                        content,
                    });
                }
            }
        }
        i += 1;
    }
    None
}

fn compile_grep_highlighter(pattern: &str) -> Option<regex::Regex> {
    let p = pattern.trim();
    if p.is_empty() {
        return None;
    }
    regex::RegexBuilder::new(p)
        .size_limit(1 << 16)
        .dfa_size_limit(1 << 16)
        .case_insensitive(false)
        .build()
        .ok()
}

fn paint_grep_match(
    content: &str,
    re: Option<&regex::Regex>,
    base: Style,
    hit: Style,
) -> Vec<Span<'static>> {
    let Some(re) = re else {
        return vec![Span::styled(content.to_string(), base)];
    };
    let mut spans = Vec::new();
    let mut last = 0usize;
    for m in re.find_iter(content) {
        if m.start() > last {
            spans.push(Span::styled(content[last..m.start()].to_string(), base));
        }
        spans.push(Span::styled(content[m.start()..m.end()].to_string(), hit));
        last = m.end();
    }
    if last < content.len() {
        spans.push(Span::styled(content[last..].to_string(), base));
    }
    if spans.is_empty() {
        spans.push(Span::styled(content.to_string(), base));
    }
    spans
}

fn paint_grep_literal(content: &str, pattern: &str, base: Style, hit: Style) -> Vec<Span<'static>> {
    if pattern.is_empty() {
        return vec![Span::styled(content.to_string(), base)];
    }
    let mut spans = Vec::new();
    let mut rest = content;
    while let Some(idx) = rest.find(pattern) {
        if idx > 0 {
            spans.push(Span::styled(rest[..idx].to_string(), base));
        }
        spans.push(Span::styled(pattern.to_string(), hit));
        rest = &rest[idx + pattern.len()..];
    }
    if !rest.is_empty() || spans.is_empty() {
        spans.push(Span::styled(rest.to_string(), base));
    }
    spans
}

/// `read` tool lines look like `   12|contents` (6-wide line no + `|`).
/// Also accepts Grok-style `   12→contents` and `   12│contents`.
fn split_read_line(line: &str) -> Option<(String, &str)> {
    for (sep, sep_len) in [('|', 1usize), ('│', '│'.len_utf8()), ('→', '→'.len_utf8())] {
        if let Some(pipe) = line.find(sep) {
            let (left, right) = line.split_at(pipe);
            if !left.is_empty() && left.chars().all(|c| c.is_ascii_digit() || c == ' ') {
                // Normalise display separator to `|` so gutters stay uniform.
                return Some((format!("{left}|"), &right[sep_len..]));
            }
        }
    }
    None
}

fn tool_summary(name: &str, input: &serde_json::Value) -> String {
    let s = match name {
        // Grok: `grep · pattern` (path is secondary when present).
        "grep" | "search_code" | "rg" => {
            let pattern = input
                .get("pattern")
                .or_else(|| input.get("query"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let path = input.get("path").and_then(|v| v.as_str()).unwrap_or("");
            if !pattern.is_empty() && !path.is_empty() && path != "." {
                format!("{pattern} · {path}")
            } else if !pattern.is_empty() {
                pattern.to_string()
            } else {
                path.to_string()
            }
        }
        // Grok: `read · path` (never a random other field).
        "read" | "read_file" | "write" | "edit" => input
            .get("path")
            .or_else(|| input.get("file_path"))
            .or_else(|| input.get("file"))
            .or_else(|| input.get("target_file"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "bash" | "shell" | "run_terminal_command" => input
            .get("description")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| input.get("command").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string(),
        "glob" => input
            .get("pattern")
            .or_else(|| input.get("glob"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        _ => input
            .get("command")
            .or_else(|| input.get("path"))
            .or_else(|| input.get("file_path"))
            .or_else(|| input.get("file"))
            .or_else(|| input.get("pattern"))
            .or_else(|| input.get("glob"))
            .or_else(|| input.get("query"))
            .or_else(|| input.get("goal"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    };
    let s = if s.is_empty() {
        let raw = input.to_string();
        if raw == "{}" || raw == "null" {
            String::new()
        } else {
            // Byte cap: never slice mid-codepoint (`ö`, CJK, emoji).
            ellipsize_bytes(&raw, 56)
        }
    } else {
        s
    };
    if s.chars().count() > 64 {
        format!("{}…", s.chars().take(63).collect::<String>())
    } else {
        s
    }
}

/// Truncate `s` to at most `max_bytes`, backing up to a char boundary.
fn ellipsize_bytes(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    format!("{}…", &s[..s.floor_char_boundary(max_bytes)])
}

fn center_line(text: &str, width: u16, color: ratatui::style::Color, bold: bool) -> Line<'static> {
    let w = text.chars().count() as u16;
    let pad = width.saturating_sub(w) / 2;
    let mut style = Style::default().fg(color);
    if bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    Line::from(vec![
        Span::raw(" ".repeat(pad as usize)),
        Span::styled(text.to_string(), style),
    ])
}

/// Centered line with two color segments: first part in `color1`, rest in `color2`.
fn center_line_colored(
    text1: &str,
    text2: &str,
    width: u16,
    color1: ratatui::style::Color,
    color2: ratatui::style::Color,
    bold: bool,
) -> Line<'static> {
    let total_w = (text1.chars().count() + text2.chars().count()) as u16;
    let pad = width.saturating_sub(total_w) / 2;
    let mut style1 = Style::default().fg(color1);
    let mut style2 = Style::default().fg(color2);
    if bold {
        style1 = style1.add_modifier(Modifier::BOLD);
        style2 = style2.add_modifier(Modifier::BOLD);
    }
    Line::from(vec![
        Span::raw(" ".repeat(pad as usize)),
        Span::styled(text1.to_string(), style1),
        Span::styled(text2.to_string(), style2),
    ])
}

fn empty_dash(s: &str) -> &str {
    if s.is_empty() { "—" } else { s }
}

#[cfg(test)]
mod tests {
    use super::{
        SparseLines, ToolOutHint, ToolPaint, ellipsize_bytes, hard_truncate_line,
        message_row_layout_mut, parse_grep_hit, prettify_tool_result, split_read_line, tool_block,
        tool_display_name, tool_result, tool_summary, visible_message_range,
    };
    use crate::app::{ChatRole, TuiApp};
    use crate::config::TuiAppConfig;
    use crate::theme::ThemeName;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::Widget;
    use serde_json::json;

    #[test]
    fn prettify_minified_json_becomes_multiline() {
        let mini =
            r#"{"exports":{"./config":"./config.js","./schema":"./schema.js"},"license":"MIT"}"#;
        let out = prettify_tool_result(mini);
        assert!(
            out.lines().count() > 3,
            "expected pretty multi-line JSON, got {out:?}"
        );
        assert!(out.contains("\"exports\""));
        assert!(out.contains("\"license\""));
    }

    #[test]
    fn prettify_webfetch_envelope_keeps_headers() {
        let raw = "URL: https://registry.npmjs.org/nuxt/latest\nStatus: 200\nContent-Type: application/json\n\n\
                   {\"name\":\"nuxt\",\"version\":\"3.0.0\",\"exports\":{\"./x\":\"./y.js\"}}";
        let out = prettify_tool_result(raw);
        assert!(out.contains("URL: https://registry.npmjs.org/nuxt/latest"));
        assert!(out.contains("Status: 200"));
        assert!(
            out.lines().count() > 6,
            "headers + pretty body should be multi-line, got:\n{out}"
        );
        assert!(out.contains("\"name\""));
    }

    #[test]
    fn hard_truncate_line_caps_display_width() {
        let s = hard_truncate_line(&"x".repeat(100), 20);
        assert_eq!(s.chars().count(), 20);
        assert!(s.ends_with('…'));
    }

    #[test]
    fn tool_result_minified_json_stays_within_preview_budget() {
        // One giant minified line used to wrap into the entire preview and look
        // like the assistant answer (┃ dump filling the transcript).
        let mini = format!(
            r#"{{"exports":{{"./entry":"{}","license":"MIT","_npmUser":{{"name":"GitHub"}}}}}}"#,
            "z".repeat(800)
        );
        let palette = ThemeName::DefaultDark.palette();
        let lines = tool_result(&mini, false, &palette, false, ToolOutHint::Auto, 80);
        // Preview budget 12 + optional "more lines" footer.
        assert!(
            lines.len() <= 14,
            "minified tool dump must not explode into {} rows",
            lines.len()
        );
        assert!(!lines.is_empty());
        // Rail still present on body rows.
        let body = lines
            .iter()
            .filter(|l| l.spans.iter().any(|s| s.content.as_ref().contains('┃')))
            .count();
        assert!(body >= 1);
    }

    #[test]
    fn tool_result_diff_paints_add_remove_colours() {
        let palette = ThemeName::DefaultDark.palette();
        let body = "Edited src/main.rs\n\n  12|-old\n  12|+new\n";
        let lines = tool_result(body, false, &palette, false, ToolOutHint::Diff, 80);
        let add = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.as_ref() == "+")
            .expect("+ marker");
        let rem = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.as_ref() == "-")
            .expect("- marker");
        assert_eq!(add.style.fg, Some(palette.diff_add));
        assert_eq!(rem.style.fg, Some(palette.diff_remove));
        // Visible wash background on add/remove rows.
        assert!(add.style.bg.is_some());
        assert!(rem.style.bg.is_some());
        // Full line body stays green/red (not syntax-overwritten).
        let add_body = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.as_ref() == "new")
            .expect("+ body");
        let rem_body = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.as_ref() == "old")
            .expect("- body");
        assert_eq!(add_body.style.fg, Some(palette.diff_add));
        assert_eq!(rem_body.style.fg, Some(palette.diff_remove));
        // Left line numbers also green/red.
        let line_nos: Vec<_> = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| s.content.as_ref() == "12")
            .collect();
        assert_eq!(line_nos.len(), 2, "expected two line-number spans");
        assert!(
            line_nos
                .iter()
                .any(|s| s.style.fg == Some(palette.diff_add))
        );
        assert!(
            line_nos
                .iter()
                .any(|s| s.style.fg == Some(palette.diff_remove))
        );
    }

    #[test]
    fn parse_grep_hit_match_and_context() {
        let m = parse_grep_hit("crates/tui/src/ui/chat.rs:901:enum ToolOutHint {").unwrap();
        assert_eq!(m.path, "crates/tui/src/ui/chat.rs");
        assert_eq!(m.lineno, "901");
        assert!(m.is_match);
        assert_eq!(m.content, "enum ToolOutHint {");

        let c = parse_grep_hit("src/foo.rs:12-// context line").unwrap();
        assert!(!c.is_match);
        assert_eq!(c.lineno, "12");
        assert_eq!(c.content, "// context line");
    }

    #[test]
    fn split_read_line_accepts_pipe_and_arrow() {
        let (g, code) = split_read_line("    12|fn main()").unwrap();
        assert_eq!(g, "    12|");
        assert_eq!(code, "fn main()");
        let (g2, code2) = split_read_line("     1→use foo;").unwrap();
        assert_eq!(g2, "     1|");
        assert_eq!(code2, "use foo;");
    }

    #[test]
    fn tool_summary_grep_prefers_pattern() {
        let s = tool_summary(
            "grep",
            &json!({"pattern": "ToolOutHint", "path": "crates/tui"}),
        );
        assert!(s.starts_with("ToolOutHint"), "got {s}");
        assert!(s.contains("crates/tui"), "got {s}");
    }

    #[test]
    fn tool_summary_read_is_path() {
        let s = tool_summary("read", &json!({"path": "src/main.rs", "offset": 1}));
        assert_eq!(s, "src/main.rs");
    }

    #[test]
    fn ellipsize_bytes_backs_up_from_mid_codepoint() {
        // 55 ASCII + `ö` (bytes 55..57) — `&s[..56]` panics.
        let s = format!("{}ö{}", "a".repeat(55), "b".repeat(10));
        assert!(!s.is_char_boundary(56));
        let out = ellipsize_bytes(&s, 56);
        assert!(out.ends_with('…'));
        assert_eq!(out.trim_end_matches('…'), "a".repeat(55));
    }

    #[test]
    fn tool_summary_json_fallback_does_not_panic_on_utf8() {
        // Unknown tool, no known string fields → JSON dump, then 56-byte cap.
        // `{"n":"` is 6 bytes + 49 ASCII = 55, then `ö` straddles offset 56.
        let s = tool_summary(
            "custom",
            &json!({"n": format!("{}ö{}", "a".repeat(49), "b".repeat(20))}),
        );
        assert!(s.ends_with('…'));
        assert!(s.is_char_boundary(s.len()));
    }

    #[test]
    fn tool_result_grep_groups_by_path_and_highlights() {
        let palette = ThemeName::DefaultDark.palette();
        let body = "\
crates/tui/src/ui/chat.rs:901:enum ToolOutHint {
crates/tui/src/ui/chat.rs:910:fn tool_out_hint()
crates/tools/src/file/grep.rs:34:        \"grep\"

(3 matches in 2 files; pattern `ToolOutHint`)";
        let lines = tool_result(
            body,
            false,
            &palette,
            false,
            ToolOutHint::Grep {
                pattern: "ToolOutHint".into(),
            },
            100,
        );
        // Path headers should appear (info-coloured).
        let path_spans: Vec<_> = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| {
                s.content.as_ref().contains("chat.rs") || s.content.as_ref().contains("grep.rs")
            })
            .collect();
        assert!(
            path_spans.iter().any(|s| s.style.fg == Some(palette.info)),
            "expected path headers in info colour"
        );
        // Match text highlighted.
        let hit = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.as_ref() == "ToolOutHint")
            .expect("highlighted match");
        assert_eq!(hit.style.fg, Some(palette.highlight));
        // Line numbers present.
        assert!(
            lines
                .iter()
                .flat_map(|l| l.spans.iter())
                .any(|s| s.content.as_ref().contains('9') && s.style.fg == Some(palette.dim)),
            "expected dim line numbers"
        );
    }

    #[test]
    fn tool_result_read_skips_path_banner_keeps_line_nos() {
        let palette = ThemeName::DefaultDark.palette();
        let body = "\
# crates/tui/src/ui/chat.rs
# lines 1–3 of 3  |  120 B
     1|fn main() {
     2|    println!(\"hi\");
     3|}
";
        let lines = tool_result(
            body,
            false,
            &palette,
            false,
            ToolOutHint::Code(Some("rust".into())),
            100,
        );
        // Path banner must not reappear as a body row.
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>()
            .join("");
        assert!(
            !joined.contains("# crates/tui"),
            "path banner should be stripped, got:\n{joined}"
        );
        // Line-number gutters still painted.
        assert!(
            lines
                .iter()
                .flat_map(|l| l.spans.iter())
                .any(|s| s.content.as_ref().contains("1|") || s.content.as_ref() == "     1|"),
            "expected line-number gutter"
        );
    }

    #[test]
    fn tool_block_grep_header_shows_match_chip() {
        let palette = ThemeName::DefaultDark.palette();
        let body = "src/a.rs:1:foo\nsrc/b.rs:2:foo\n\n(2 matches in 2 files; pattern `foo`)";
        let lines = tool_block(
            "grep",
            &json!({"pattern": "foo"}),
            Some(body),
            ToolPaint {
                is_error: false,
                palette: &palette,
                expanded: false,
                width: 100,
                spin: 0,
            },
        );
        let header: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(header.contains('•'), "Grok bullet, got {header}");
        assert!(header.contains("Searched"), "got {header}");
        assert!(header.contains("foo"), "got {header}");
    }

    #[test]
    fn tool_display_name_maps_bash_to_run() {
        assert_eq!(tool_display_name("bash"), "run");
        assert_eq!(tool_display_name("shell"), "run");
        assert_eq!(tool_display_name("read"), "read");
        assert_eq!(tool_display_name("grep"), "grep");
    }

    #[test]
    fn tool_block_run_shows_display_name_and_command() {
        let palette = ThemeName::DefaultDark.palette();
        let lines = tool_block(
            "bash",
            &json!({"command": "cargo test -p whycode-tui"}),
            Some("ok\n"),
            ToolPaint {
                is_error: false,
                palette: &palette,
                expanded: false,
                width: 100,
                spin: 0,
            },
        );
        let header: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            !header.contains('┃'),
            "collapsed Run has no accent rail, got {header}"
        );
        assert!(header.contains('•'), "Grok bullet, got {header}");
        assert!(header.contains("Run"), "got {header}");
        assert!(!header.contains("bash"), "got {header}");
        assert!(header.contains("cargo test"), "got {header}");
    }

    #[test]
    fn collapsed_tool_is_header_only() {
        use crate::theme::ThemeName;
        let palette = ThemeName::DefaultDark.palette();
        let body = (0..20)
            .map(|i| format!("line {i} of noisy tool output"))
            .collect::<Vec<_>>()
            .join("\n");
        let lines = tool_block(
            "bash",
            &json!({"command": "cargo test"}),
            Some(&body),
            ToolPaint {
                is_error: false,
                palette: &palette,
                expanded: false,
                width: 80,
                spin: 0,
            },
        );
        assert_eq!(
            lines.len(),
            1,
            "Grok collapsed tool is a one-liner: {lines:?}"
        );
        let header: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(header.contains('•'), "{header}");
        assert!(
            !header.contains("line 5"),
            "body must not leak into the header: {header}"
        );
        assert!(
            !header.contains('›') && !header.contains('>'),
            "collapsed tool must not trail a chevron: {header}"
        );
    }

    #[test]
    fn collapsed_tools_group_into_one_verb_line() {
        use crate::app::{ChatRole, TuiApp};
        use crate::config::TuiAppConfig;
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.add_message(ChatRole::User, "look");
        app.add_message(ChatRole::Assistant, "");
        app.add_tool_call("t1".into(), "grep".into(), json!({"pattern": "a"}));
        app.add_tool_result("t1", "(1 match)", false);
        app.add_tool_call("t2".into(), "grep".into(), json!({"pattern": "b"}));
        app.add_tool_result("t2", "(1 match)", false);
        app.add_tool_call("t3".into(), "grep".into(), json!({"pattern": "c"}));
        app.add_tool_result("t3", "(1 match)", false);
        app.add_tool_call("t4".into(), "list".into(), json!({"path": "."}));
        app.add_tool_result("t4", "ok", false);
        app.add_tool_call("t5".into(), "read".into(), json!({"path": "CLAUDE.md"}));
        app.add_tool_result("t5", "ok", false);
        let palette = app.config.palette();
        let lines = super::render_message(&app.messages[1], &app, &palette, 1, 80, None, false);
        let texts: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        let joined = texts.join("\n");
        assert!(
            joined.contains("Searched 3 patterns, Listed 1 dir, Read 1 file"),
            "Grok verb-group line, got {texts:?}"
        );
    }

    #[test]
    fn execute_expanded_keeps_head_and_tail() {
        use crate::theme::ThemeName;
        let palette = ThemeName::DefaultDark.palette();
        let body = (0..10)
            .map(|i| format!("exec-line-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let lines = tool_block(
            "bash",
            &json!({"command": "seq"}),
            Some(&body),
            ToolPaint {
                is_error: false,
                palette: &palette,
                expanded: true,
                width: 80,
                spin: 0,
            },
        );
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("exec-line-0"), "{text}");
        assert!(text.contains("exec-line-1"), "{text}");
        assert!(text.contains("exec-line-9"), "{text}");
        assert!(text.contains('…'), "{text}");
        assert!(
            !text.contains("exec-line-4"),
            "middle dump must stay hidden: {text}"
        );
    }

    #[test]
    fn last_scrolled_past_user_picks_the_latest_above_the_view() {
        use crate::app::{ChatRole, TuiApp};
        use crate::config::TuiAppConfig;
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.add_message(ChatRole::User, "first");
        app.add_message(ChatRole::Assistant, "a");
        app.add_message(ChatRole::User, "second");
        app.add_message(ChatRole::Assistant, "b");
        let starts = vec![0, 5, 10, 20];
        assert_eq!(super::last_scrolled_past_user(&app, &starts, 12), Some(2));
        assert_eq!(super::last_scrolled_past_user(&app, &starts, 5), Some(0));
        assert_eq!(super::last_scrolled_past_user(&app, &starts, 0), None);
    }

    #[test]
    fn live_thinking_rail_moves_with_the_spinner() {
        use crate::app::ThinkingBlock;
        use crate::theme::ThemeName;
        let palette = ThemeName::DefaultDark.palette();
        let t = ThinkingBlock::new("one\ntwo\nthree\nfour");
        let a = super::thinking_lines(&t, &palette, 40, 0);
        let b = super::thinking_lines(&t, &palette, 40, 3);
        let rail_fg = |lines: &[ratatui::text::Line]| {
            lines
                .iter()
                .filter_map(|l| {
                    l.spans
                        .iter()
                        .find(|s| s.content.as_ref() == "┃")
                        .and_then(|s| s.style.fg)
                })
                .collect::<Vec<_>>()
        };
        assert_ne!(
            rail_fg(&a),
            rail_fg(&b),
            "live thinking rail must wave across frames"
        );
    }

    #[test]
    fn running_execute_rail_pulses_and_uses_success() {
        use crate::theme::ThemeName;
        let palette = ThemeName::DefaultDark.palette();
        let paint = |spin| ToolPaint {
            is_error: false,
            palette: &palette,
            expanded: true,
            width: 60,
            spin,
        };
        let a = tool_block("bash", &json!({"command": "sleep 1"}), None, paint(0));
        let b = tool_block("bash", &json!({"command": "sleep 1"}), None, paint(2));
        let rail = |lines: &[ratatui::text::Line]| {
            lines[0]
                .spans
                .iter()
                .find(|s| s.content.as_ref() == "┃")
                .and_then(|s| s.style.fg)
        };
        let fa = rail(&a).expect("running Run paints a rail");
        let fb = rail(&b).expect("running Run paints a rail");
        assert!(
            fa == palette.success || fa == palette.dim,
            "run rail is success or dim rest, got {fa:?}"
        );
        assert_ne!(fa, fb, "running Run rail must blink across frames");
    }

    #[test]
    fn failed_execute_rail_is_error_red() {
        use crate::theme::ThemeName;
        let palette = ThemeName::DefaultDark.palette();
        let lines = tool_block(
            "bash",
            &json!({"command": "false"}),
            Some("exit 1"),
            ToolPaint {
                is_error: true,
                palette: &palette,
                expanded: true,
                width: 60,
                spin: 0,
            },
        );
        let fg = lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "┃")
            .and_then(|s| s.style.fg);
        assert_eq!(fg, Some(palette.error), "failed Run rail is error red");
    }

    #[test]
    fn sparse_lines_full_width_diff_band() {
        // Grok paints the whole row green/red — not only the text width.
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        let add_bg = Color::Rgb(20, 80, 40);
        SparseLines {
            lines: vec![Line::from(vec![
                Span::styled(
                    "+new".to_string(),
                    Style::default().fg(Color::Green).bg(add_bg),
                ),
                Span::styled(" ".to_string(), Style::default().bg(add_bg)),
            ])],
            bg: Color::Black,
        }
        .render(area, &mut buf);

        // Far-right cell (beyond text) must share the band bg.
        let cell = buf.cell((19, 0)).expect("right edge");
        assert_eq!(
            cell.style().bg,
            Some(add_bg),
            "diff wash must fill full row width"
        );
        // Left cell too.
        let left = buf.cell((0, 0)).expect("left edge");
        assert_eq!(left.style().bg, Some(add_bg));
    }

    #[test]
    fn sparse_lines_clears_previous_glyphs_before_paint() {
        let area = Rect::new(0, 0, 12, 3);
        let mut buf = Buffer::empty(area);
        for y in 0..3 {
            buf.set_stringn(0, y, "OLDCONTENT!!", 12, Style::default());
        }

        SparseLines {
            lines: vec![
                Line::from(Span::raw("new")),
                Line::from(""),
                Line::from(Span::raw("ok")),
            ],
            bg: Color::Black,
        }
        .render(area, &mut buf);

        assert_eq!(buf.cell((0, 0)).map(|c| c.symbol()), Some("n"));
        assert_eq!(buf.cell((3, 0)).map(|c| c.symbol()), Some(" "));
        assert_eq!(buf.cell((0, 1)).map(|c| c.symbol()), Some(" "));
        assert_eq!(buf.cell((5, 1)).map(|c| c.symbol()), Some(" "));
        assert_eq!(buf.cell((0, 2)).map(|c| c.symbol()), Some("o"));
    }

    #[test]
    fn selected_concat_slice_marks_first_content_across_halves() {
        let area = Rect::new(0, 0, 8, 2);
        let mut buf = Buffer::empty(area);
        let row = super::ChatRowPaint {
            x: 0,
            width: area.width,
            bg: Color::Black,
            caret_style: Style::default().fg(Color::Yellow),
        };
        let prefix = [Line::from(""), Line::from(Span::raw("prefix"))];
        let body = [Line::from(Span::raw("body"))];

        let next = super::paint_concat_slices(&mut buf, 0, &row, &prefix, &body, 1..3, true);

        assert_eq!(next, 2);
        assert_eq!(buf.cell((0, 0)).map(|c| c.symbol()), Some("▌"));
        assert_eq!(buf.cell((1, 0)).map(|c| c.symbol()), Some("p"));
        assert_eq!(buf.cell((0, 1)).map(|c| c.symbol()), Some("b"));
        assert_eq!(
            super::paint_concat_slices(&mut buf, next, &row, &prefix, &body, 3..3, true),
            next
        );
    }

    #[test]
    fn width_helpers_respect_unicode_and_tiny_budgets() {
        use unicode_width::UnicodeWidthStr;

        assert_eq!(super::truncate_home_title("session", 0), "");
        assert_eq!(super::truncate_home_title("session", 1), "…");
        assert_eq!(super::truncate_home_title("界面 title", 5), "界面…");
        assert_eq!(
            UnicodeWidthStr::width(super::cut_to_width("a界b", 3).as_str()),
            3
        );

        let style = Style::default().fg(Color::Cyan);
        let mut spans = vec![Span::styled("a界".to_string(), style), Span::raw("tail")];
        super::truncate_spans_to(&mut spans, 2);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content.as_ref(), "a");
        assert_eq!(spans[0].style, style);
    }

    #[test]
    fn image_only_prompt_renders_one_chip_between_band_padding() {
        let palette = ThemeName::DefaultDark.palette();
        let labels = vec!["diagram.png".to_string()];
        let lines = super::user_prompt_lines(
            "[Image: diagram.png]",
            &labels,
            Some("2:32 PM"),
            &palette,
            40,
            false,
            false,
        );

        assert_eq!(lines.len(), 3);
        assert_eq!(line_text(&lines[0]), " ");
        assert_eq!(line_text(&lines[2]), " ");
        let chip = line_text(&lines[1]);
        assert!(chip.contains("🖼 diagram.png"), "{chip:?}");
        assert!(chip.contains("2:32 PM"), "{chip:?}");
        assert!(!chip.contains("[Image:"), "{chip:?}");
    }

    #[test]
    fn visible_message_range_is_binary_search() {
        // starts = prefix sums: msg0@0 h=3, msg1@3 h=5, msg2@8 h=2, total=10
        let starts = [0usize, 3, 8];
        let total = 10;
        assert_eq!(visible_message_range(&starts, total, 0, 3), 0..1);
        assert_eq!(visible_message_range(&starts, total, 2, 4), 0..2);
        assert_eq!(visible_message_range(&starts, total, 3, 8), 1..2);
        assert_eq!(visible_message_range(&starts, total, 7, 10), 1..3);
        assert_eq!(visible_message_range(&starts, total, 8, 10), 2..3);
        assert!(visible_message_range(&starts, total, 10, 12).is_empty());
        assert!(visible_message_range(&[], 0, 0, 5).is_empty());
    }

    #[test]
    fn layout_height_cache_hits_on_second_pass() {
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.add_message(ChatRole::User, "hello");
        app.add_message(ChatRole::Assistant, "world");
        let width = 80u16;
        let (starts1, total1) = message_row_layout_mut(&mut app, width);
        assert!(app.messages.iter().all(|m| m.layout_cache.is_some()));
        let (starts2, total2) = message_row_layout_mut(&mut app, width);
        assert_eq!(starts1, starts2);
        assert_eq!(total1, total2);
        assert!(
            app.messages
                .iter()
                .all(|m| matches!(m.layout_cache, Some((w, _, _)) if w == width))
        );
        app.append_to_last(" more");
        assert!(app.messages.last().unwrap().layout_cache.is_none());
    }

    #[test]
    fn closed_message_cache_survives_agent_busy_flip() {
        use crate::app::AgentState;
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.add_message(ChatRole::User, "hello");
        app.add_message(ChatRole::Assistant, "# hi\n\n```rs\nfn main() {}\n```");
        let width = 80u16;
        let _ = message_row_layout_mut(&mut app, width);
        assert!(
            app.messages[0].line_cache.is_some(),
            "user bubble must cache after first layout"
        );
        let user_lines = app.messages[0].line_cache.as_ref().unwrap().2.len();
        assert!(
            app.messages[1].line_cache.is_some(),
            "idle assistant must cache before the busy flip"
        );

        app.current_agent_state = AgentState::Generating;
        let _ = message_row_layout_mut(&mut app, width);
        assert!(
            app.messages[0].line_cache.is_some(),
            "finished user bubble must keep its cache when a turn starts"
        );
        assert_eq!(
            app.messages[0].line_cache.as_ref().unwrap().2.len(),
            user_lines
        );
    }

    #[test]
    fn user_prompt_puts_clock_on_the_first_content_line() {
        use crate::theme::ThemeName;
        let palette = ThemeName::DefaultDark.palette();
        let lines =
            super::user_prompt_lines("hello", &[], Some("2:32 PM"), &palette, 40, false, false);
        let first = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains('\u{276F}')))
            .expect("prompt row");
        let text: String = first.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("hello"), "got {text:?}");
        assert!(
            text.contains("2:32 PM"),
            "clock must sit on the ❯ row, got {text:?}"
        );
        let hello_at = text.find("hello").unwrap();
        let clock_at = text.find("2:32 PM").unwrap();
        assert!(
            clock_at > hello_at,
            "clock must be to the right of the text"
        );
    }

    #[test]
    fn assistant_answer_renders_after_tools() {
        use crate::app::{ChatRole, TuiApp};
        use crate::config::TuiAppConfig;
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.add_message(ChatRole::User, "look");
        app.add_message(ChatRole::Assistant, "AFTER_TOOLS_ANSWER");
        app.add_tool_call(
            "t1".into(),
            "read".into(),
            serde_json::json!({ "path": "a.rs" }),
        );
        app.add_tool_result("t1", "ok", false);
        let palette = app.config.palette();
        let lines = super::render_message(&app.messages[1], &app, &palette, 1, 80, None, false);
        let texts: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        let tool_at = texts
            .iter()
            .position(|t| t.contains('•') || t.contains("Read"));
        let answer_at = texts.iter().position(|t| t.contains("AFTER_TOOLS_ANSWER"));
        assert!(tool_at.is_some(), "tool card missing: {texts:?}");
        assert!(answer_at.is_some(), "answer missing: {texts:?}");
        assert!(
            tool_at.unwrap() < answer_at.unwrap(),
            "answer must sit below tools, got {texts:?}"
        );
    }

    #[test]
    fn assistant_reply_puts_clock_on_the_first_content_line() {
        use crate::app::{ChatRole, TuiApp};
        use crate::config::TuiAppConfig;
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.add_message(ChatRole::User, "hi");
        app.add_message(ChatRole::Assistant, "hello from the agent");
        app.messages[1].duration_ms = Some(1200);
        let palette = app.config.palette();
        let lines = super::render_message(&app.messages[1], &app, &palette, 1, 80, None, false);
        let answer = lines
            .iter()
            .find(|l| {
                l.spans
                    .iter()
                    .any(|s| s.content.contains("hello from the agent"))
            })
            .expect("assistant body");
        let text: String = answer.spans.iter().map(|s| s.content.as_ref()).collect();
        let hello_at = text.find("hello").expect("answer text");
        let colon = text.rfind(':').expect("h:mm AM/PM clock on the reply");
        assert!(
            colon > hello_at,
            "Grok puts the clock on the right of the agent line, got {text:?}"
        );
        assert!(
            text.contains("AM") || text.contains("PM"),
            "Grok bubble clock is 12-hour, got {text:?}"
        );
        let footer = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains("Worked for")))
            .expect("turn footer");
        let foot: String = footer.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            foot.contains(':'),
            "Worked for line also carries the clock, got {foot:?}"
        );
    }

    #[test]
    fn live_thinking_puts_elapsed_on_the_right() {
        use crate::app::ThinkingBlock;
        use crate::theme::ThemeName;
        let palette = ThemeName::DefaultDark.palette();
        let t = ThinkingBlock::new("one\ntwo\nthree");
        let lines = super::thinking_lines(&t, &palette, 60, 0);
        let header: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            header.contains("Thinking..."),
            "Grok live header is Thinking..., got {header:?}"
        );
        assert!(
            !header.contains('·'),
            "elapsed sits on the right, not after a mid-dot: {header:?}"
        );
        assert!(
            header.contains('s') || header.contains("Thinking..."),
            "got {header:?}"
        );
    }

    #[test]
    fn collapsed_thinking_has_no_trailing_chevron() {
        use crate::app::ThinkingBlock;
        use crate::theme::ThemeName;
        let palette = ThemeName::DefaultDark.palette();
        let mut t = ThinkingBlock::new("secret plan");
        t.finish();
        t.collapsed = true;
        let lines = super::thinking_lines(&t, &palette, 60, 0);
        let header: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(header.contains("Thought"), "got {header:?}");
        assert!(
            header.contains('┃'),
            "thinking keeps a left accent, got {header:?}"
        );
        assert!(
            !header.contains('›') && !header.contains('>'),
            "folded rows must not trail a chevron, got {header:?}"
        );
        assert!(
            !header.contains("(e expand)"),
            "legacy hint must not remain, got {header:?}"
        );
    }

    #[test]
    fn first_user_bubble_keeps_clock_on_the_right() {
        use crate::app::{ChatRole, TuiApp};
        use crate::config::TuiAppConfig;
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.add_message(ChatRole::User, "hello from history");
        let palette = app.config.palette();
        let lines = super::render_message(&app.messages[0], &app, &palette, 0, 80, None, false);
        let row = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains('\u{276F}')))
            .expect("first bubble ❯ row");
        let text: String = row.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("hello from history"), "got {text:?}");
        let hello_at = text.find("hello").expect("prompt text");
        let colon = text.rfind(':').expect("h:mm AM/PM clock");
        assert!(
            colon > hello_at,
            "clock must sit to the right, got {text:?}"
        );
        assert!(
            text.contains("AM") || text.contains("PM"),
            "Grok bubble clock is 12-hour, got {text:?}"
        );
    }

    #[test]
    fn long_first_prompt_still_keeps_the_clock() {
        use crate::theme::ThemeName;
        let palette = ThemeName::DefaultDark.palette();
        let prompt = "please look at the first message bubble in chat history \
and tell me if the clock should be on the right side of it like grok does \
with a short 12-hour stamp";
        let lines =
            super::user_prompt_lines(prompt, &[], Some("2:32 PM"), &palette, 72, false, false);
        let first = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains('\u{276F}')))
            .expect("prompt row");
        let text: String = first.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            text.contains("2:32 PM"),
            "long first bubble must keep the clock, got {text:?}"
        );
    }

    #[test]
    fn right_align_keeps_clock_at_the_row_end() {
        let line = super::line_with_right(
            vec![Span::raw("❯ hi".to_string())],
            Some("14:32"),
            Style::default(),
            20,
        );
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        // Last column is a gutter (scrollbar); clock sits just left of it.
        assert_eq!(unicode_width::UnicodeWidthStr::width(text.as_str()), 19);
        assert!(text.ends_with("14:32"), "got {text:?}");
    }

    #[test]
    fn right_align_never_drops_the_clock() {
        let line = super::line_with_right(
            vec![Span::raw(format!("❯ {}", "x".repeat(40)))],
            Some("14:32"),
            Style::default(),
            20,
        );
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            text.contains("14:32"),
            "clock must survive an oversized left side, got {text:?}"
        );
        assert!(
            unicode_width::UnicodeWidthStr::width(text.as_str()) <= 20,
            "row must still fit, got {text:?}"
        );
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn long_user_prompt_collapses_to_three_lines() {
        use crate::theme::ThemeName;
        let palette = ThemeName::DefaultDark.palette();
        let prompt = "one\ntwo\nthree\nfour\nfive";
        let folded = super::user_prompt_lines(prompt, &[], None, &palette, 40, false, false);
        let texts: Vec<String> = folded.iter().map(line_text).collect();
        let content: Vec<&String> = texts.iter().filter(|t| !t.trim().is_empty()).collect();
        assert_eq!(content.len(), 3, "Grok folds to 3 content rows: {texts:?}");
        assert!(
            content.last().is_some_and(|t| t.contains('\u{2026}')),
            "collapsed last row carries …, got {texts:?}"
        );
        assert!(
            !texts
                .iter()
                .any(|t| t.contains("four") || t.contains("five")),
            "hidden tail must not paint: {texts:?}"
        );

        let open = super::user_prompt_lines(prompt, &[], None, &palette, 40, false, true);
        let open_text: Vec<String> = open.iter().map(line_text).collect();
        assert!(
            open_text.iter().any(|t| t.contains("four")),
            "expanded shows the tail: {open_text:?}"
        );
        assert!(
            !open_text.iter().any(|t| t.contains('\u{2026}')),
            "expanded has no ellipsis: {open_text:?}"
        );
    }

    #[test]
    fn slash_command_token_uses_accent() {
        use crate::theme::ThemeName;
        let palette = ThemeName::DefaultDark.palette();
        let lines = super::user_prompt_lines("/help please", &[], None, &palette, 40, false, false);
        let row = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains("/help")))
            .expect("prompt row");
        let token = row
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "/help")
            .expect("/help span");
        assert_eq!(
            token.style.fg,
            Some(palette.accent),
            "Grok paints /command in the skill accent"
        );
        let rest = row
            .spans
            .iter()
            .find(|s| s.content.contains("please"))
            .expect("args span");
        assert_eq!(rest.style.fg, Some(palette.fg));
    }

    #[test]
    fn visible_range_clamps_bottom_anchored_scroll() {
        assert_eq!(super::visible_range(0, 10, 0), (0, 0));
        assert_eq!(super::visible_range(10, 0, 0), (0, 0));
        assert_eq!(super::visible_range(100, 20, 0), (80, 100));
        assert_eq!(super::visible_range(100, 20, 30), (50, 70));
        assert_eq!(super::visible_range(10, 20, usize::MAX), (0, 10));
    }

    #[test]
    fn json_and_grep_detection_reject_malformed_inputs() {
        assert_eq!(super::prettify_tool_result(""), "");
        assert_eq!(super::prettify_tool_result("not json"), "not json");
        assert!(super::find_json_value_start("prefix {\"ok\":true}").is_some());
        assert!(super::find_json_value_start("prefix {bad").is_none());
        assert!(super::looks_like_json_body("[1,2]"));
        assert!(!super::looks_like_json_body("[bad"));
        assert!(super::looks_like_grep_body(
            "src/a.rs:1:hit\nsrc/a.rs:2-context"
        ));
        assert!(!super::looks_like_grep_body("ordinary\ntext"));
        assert!(super::parse_grep_hit(":12:no path").is_none());
        assert!(super::parse_grep_hit("file:no:number").is_none());
        assert!(super::split_read_line("not a numbered row").is_none());
    }
}

#[cfg(test)]
#[path = "chat_scroll_tests.rs"]
mod scroll_tests;
