// ── ui/chat.rs: session message list ───────────────────────────────────
// User: elevated band + ❯ prefix. Assistant: free-flow body + turn footer.
// Home: centered dual-block logo.

use crate::app::{ChatBlock, ChatRole, TuiApp};
use crate::theme::ThemePalette;
use crate::tokens::{HOME_LOGO_CODE, HOME_LOGO_WHY, layout};
use crate::ui::scrollbar::{ScrollbarColors, paint_scrollbar};
use crate::widgets::wrap::wrap_text;
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Widget},
};
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
    let busy = app.is_busy();
    let mut starts = Vec::with_capacity(app.messages.len());
    let mut total = 0;
    for (i, msg) in app.messages.iter().enumerate() {
        starts.push(total);
        if let Some((w, b, h)) = msg.layout_cache
            && w == width
            && b == busy
        {
            total += h;
            continue;
        }
        if let Some((w, b, ref lines)) = msg.line_cache
            && w == width
            && b == busy
        {
            total += lines.len();
            continue;
        }
        total += render_message(msg, app, &palette, i, width).len();
    }
    (starts, total)
}

/// Like [`message_row_layout`] but writes height / line caches on miss.
pub fn message_row_layout_mut(app: &mut TuiApp, width: u16) -> (Vec<usize>, usize) {
    let busy = app.is_busy();
    let n = app.messages.len();
    let mut starts = Vec::with_capacity(n);
    let mut total = 0;
    for i in 0..n {
        starts.push(total);
        let h = if let Some((w, b, h)) = app.messages[i].layout_cache
            && w == width
            && b == busy
        {
            h
        } else if let Some((w, b, ref lines)) = app.messages[i].line_cache
            && w == width
            && b == busy
        {
            let h = lines.len();
            app.messages[i].layout_cache = Some((width, busy, h));
            h
        } else {
            let lines = {
                let palette = app.config.palette();
                render_message(&app.messages[i], app, &palette, i, width)
            };
            let h = lines.len();
            app.messages[i].layout_cache = Some((width, busy, h));
            if message_is_closed(app, i) {
                app.messages[i].line_cache = Some((width, busy, lines));
            }
            h
        };
        total += h;
    }
    (starts, total)
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
    let mut lines: Vec<Line> = Vec::new();
    // Vertical centering via top spacers
    let content_h = 4 + 1 + 2 + 2; // logo + gap + meta + hints
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
    let agent_color = palette.agent_color_by_index(app.agent_cycle_idx);
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
    // "Get started /connect" like footer welcome
    let gs = "Get started  /connect".to_string();
    lines.push(center_line(&gs, area.width, palette.fg, false));

    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::default().bg(palette.bg)),
        area,
    );
}

fn render_session(frame: &mut Frame, area: Rect, app: &mut TuiApp, palette: &ThemePalette) {
    // Shell already applies SIDE_PAD — don't pad again (extra spaces end up in mouse selection).
    // Keep wrap width == area.width so it matches `app.chat_content_width`
    // (scroll math / ensure_selected_visible). Scrollbar is painted over the
    // rightmost column when content overflows.
    let height = area.height as usize;
    let content_width = area.width;

    // Virtualize paint: height-cache all messages, but only `render_message`
    // those that intersect the viewport. Long transcripts no longer re-parse
    // every finished bubble on every spinner frame.
    let (starts, total) = message_row_layout_mut(app, content_width);
    let (view_start, view_end) = visible_range(total, height, app.scroll_offset);
    let needs_bar = total > height && area.width > 1;

    let mut lines: Vec<Line> = Vec::with_capacity(height.saturating_add(8));
    let n = app.messages.len();
    let busy = app.is_busy();
    for i in 0..n {
        let msg_start = starts[i];
        let msg_end = if i + 1 < n { starts[i + 1] } else { total };
        // Skip bubbles wholly above or below the viewport.
        if msg_end <= view_start || msg_start >= view_end {
            continue;
        }

        let selected =
            app.selected_msg == Some(i) && app.focus == crate::app::FocusPane::Scrollback;

        // Closed messages: reuse cached lines (no markdown re-parse per paint).
        // Selection mutates the first content row (caret / user highlight), so
        // only the unselected path may serve the cache.
        let mut msg_lines = if !selected
            && let Some((w, b, ref cached)) = app.messages[i].line_cache
            && w == content_width
            && b == busy
        {
            cached.clone()
        } else {
            let rendered = render_message(&app.messages[i], app, palette, i, content_width);
            if !selected && message_is_closed(app, i) {
                app.messages[i].line_cache = Some((content_width, busy, rendered.clone()));
                app.messages[i].layout_cache = Some((content_width, busy, rendered.len()));
            }
            rendered
        };
        if selected {
            // Grok: selected entry gets a left caret on its first content row.
            let caret = Span::styled(
                "▌".to_string(),
                Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD),
            );
            if let Some(line) = msg_lines.iter_mut().find(|l| !l.spans.is_empty()) {
                let mut spans = vec![caret];
                spans.append(&mut line.spans);
                line.spans = spans;
            }
        }

        // Slice the bubble to the viewport (partial top/bottom visibility).
        let slice_from = view_start.saturating_sub(msg_start);
        let slice_to = (view_end.saturating_sub(msg_start)).min(msg_lines.len());
        if slice_from < slice_to {
            lines.extend(msg_lines.drain(slice_from..slice_to));
        }
    }

    // Pin messages to the bottom (chat-style). Empty space sits above the
    // transcript so a downward drag from a bubble doesn't vacuum up a page of
    // blank rows into the clipboard.
    if lines.len() < height {
        let pad = height - lines.len();
        let mut padded = Vec::with_capacity(height);
        padded.resize_with(pad, || Line::from(""));
        padded.append(&mut lines);
        lines = padded;
    }

    // Clear-then-paint: ratatui diffs against the previous buffer, so a sparse
    // writer that only touches non-empty cells leaves *ghost glyphs* after
    // scroll (old rows still visible / garbled). Wipe the viewport first, then
    // stamp content. Clipboard trims trailing pad spaces on copy.
    frame.render_widget(
        SparseLines {
            lines,
            bg: palette.bg,
        },
        area,
    );

    let scrollbar_hit = if needs_bar {
        // Use our proportional painter — not ratatui::Scrollbar. Ratatui's
        // thumb math (`position * track / (content-1 + viewport)`) parks the
        // handle around ~70% when `position = view_start` at the last page,
        // so "at bottom" never looked like the bottom. Ours maps
        // top-origin `view_start` with `thumb_pos = view_start * travel / max_off`,
        // matching drag math in `ui/scrollbar` (0 → top, max_off → flush bottom).
        let sb = Rect {
            x: area.x + area.width.saturating_sub(1),
            y: area.y,
            width: 1,
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
        // Widen the hit column slightly so the 1-cell track is easier to grab.
        let hit_w = 2u16.min(area.width);
        Some(Rect {
            x: area.x + area.width.saturating_sub(hit_w),
            y: area.y,
            width: hit_w,
            height: area.height,
        })
    } else {
        None
    };

    app.apply_chat_paint(area, scrollbar_hit, total);
}

/// Paint chat lines after wiping the viewport.
///
/// History: a pure sparse writer (only non-empty spans) left previous-frame
/// glyphs in place after scroll — the transcript looked frozen or garbled
/// because shorter/empty rows never overwrote the old cells. We now blank the
/// area to `bg` first, then stamp content. Trailing spaces are still pad for
/// the clipboard path (`text_from_cells` trims them).
struct SparseLines {
    lines: Vec<Line<'static>>,
    bg: ratatui::style::Color,
}

impl Widget for SparseLines {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let clear = Style::default().fg(self.bg).bg(self.bg);
        // Full wipe — every cell becomes a space with the chat background so
        // scroll/resize cannot leave ghost characters from the last frame.
        for y in area.y..area.y.saturating_add(area.height) {
            for x in area.x..area.x.saturating_add(area.width) {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_symbol(" ");
                    cell.set_style(clear);
                }
            }
        }

        let max = self.lines.len().min(area.height as usize);
        for (row, line) in self.lines.iter().take(max).enumerate() {
            let y = area.y + row as u16;
            let mut x = area.x;
            let end = area.x.saturating_add(area.width);
            let mut band_bg: Option<ratatui::style::Color> = None;
            for span in &line.spans {
                if let Some(bg) = span.style.bg {
                    band_bg = Some(bg);
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
            // Full-width elevated band (Grok user prompt + diff add/remove rows).
            // Walk the entire row — not just trailing cells after content — so
            // meta gutters / empty left pad share the green/red wash.
            if let Some(bg) = band_bg {
                for cx in area.x..end {
                    if let Some(cell) = buf.cell_mut((cx, y)) {
                        if cell.symbol().is_empty() {
                            cell.set_symbol(" ");
                        }
                        let mut st = cell.style();
                        st.bg = Some(bg);
                        cell.set_style(st);
                    }
                }
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
    if app.is_busy()
        && index + 1 == app.messages.len()
        && matches!(msg.role, ChatRole::Assistant)
    {
        return false;
    }
    true
}

fn render_message(
    msg: &crate::app::ChatMessage,
    app: &TuiApp,
    palette: &ThemePalette,
    index: usize,
    width: u16,
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
            lines.extend(user_prompt_lines(
                &msg.content,
                &msg.image_labels,
                palette,
                width,
                selected,
            ));
        }
        ChatRole::Assistant => {
            // Chronological layout: stream thinking first, emit `content` before
            // the first non-thinking block (or at the end if only thinking/text).
            // That way "Thought for Xs" sits above the answer, not below it.
            // Content width for Mermaid compaction: leave room for SIDE_PAD and
            // the diagram gutter ("│ ").
            let md_width = (width as usize).saturating_sub(4).max(20);
            let mut content_emitted = msg.content.is_empty();
            let emit_content = |lines: &mut Vec<Line<'static>>| {
                if !msg.content.is_empty() {
                    lines.extend(super::markdown::render_with_width(
                        &msg.content,
                        palette,
                        Some(md_width),
                    ));
                }
            };
            for block in &msg.blocks {
                match block {
                    ChatBlock::Text(t) if msg.content.is_empty() => {
                        lines.extend(super::markdown::render_with_width(
                            t,
                            palette,
                            Some(md_width),
                        ));
                        content_emitted = true;
                    }
                    ChatBlock::Text(_) => {}
                    ChatBlock::Thinking(t) => {
                        lines.extend(thinking_lines(t, palette, width));
                    }
                    ChatBlock::ToolUse { id, name, input } => {
                        if !content_emitted {
                            emit_content(&mut lines);
                            content_emitted = true;
                        }
                        // Prefer the live tool_calls result when this id already finished.
                        let (result, is_error) = msg
                            .tool_calls
                            .iter()
                            .find(|tc| tc.id == *id)
                            .map(|tc| (tc.result.as_deref(), tc.is_error))
                            .unwrap_or((None, false));
                        lines.extend(tool_block(
                            name,
                            input,
                            result,
                            is_error,
                            palette,
                            msg.results_expanded,
                            width,
                        ));
                    }
                    ChatBlock::ToolResult { .. } => {
                        // Painted with the matching ToolUse / tool_calls entry.
                    }
                }
            }
            if !content_emitted {
                emit_content(&mut lines);
            }
            for tc in &msg.tool_calls {
                let dup = msg
                    .blocks
                    .iter()
                    .any(|b| matches!(b, ChatBlock::ToolUse { id, .. } if id == &tc.id));
                if dup {
                    // Already painted as ToolUse (+ result).
                } else {
                    lines.extend(tool_block(
                        &tc.name,
                        &tc.arguments,
                        tc.result.as_deref(),
                        tc.is_error,
                        palette,
                        msg.results_expanded,
                        width,
                    ));
                }
            }
            // Turn footer: past tense, muted duration ("Worked for 12s").
            // Provider/model live under the prompt meta row once.
            let is_last = index + 1 == app.messages.len();
            let still_streaming = app.is_busy() && is_last;
            let empty = msg.content.is_empty()
                && msg.blocks.is_empty()
                && msg.tool_calls.is_empty()
                && msg.error.is_none();
            if !empty && !still_streaming {
                let footer = turn_done_footer(msg, is_last, app);
                let mut epi = vec![meta_gutter()];
                epi.push(Span::styled(footer, Style::default().fg(palette.dim)));
                lines.push(Line::from(""));
                lines.push(Line::from(epi));
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
/// - Finished + collapsed → single header line (+ expand hint)
/// - Expanded → header + full body
fn thinking_lines(
    t: &crate::app::ThinkingBlock,
    palette: &ThemePalette,
    width: u16,
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
    // Grok thinking accent = gray_dim; soft quiet rail, not purple.
    let rail_style = Style::default().fg(palette.dim);
    // Full-height accent only while body is visible (Grok: no accent collapsed).
    let show_rail = t.show_body();
    // Accent col (1) + pad (1) — matches Grok HorizontalLayout::ACCENT + pad.
    let rail_cols: u16 = if show_rail { 2 } else { 0 };
    let content_w = width.saturating_sub(rail_cols);

    // Header: Grok always labels reasoning as "Thinking" while live, and
    // "Thought for Xs" when finished. Live timer matches the busy strip
    // (`thinking 1.4s`) so the user sees that thinking is actually happening.
    let mut header_spans: Vec<Span<'static>> = if t.is_running() {
        let elapsed = t.format_elapsed();
        if elapsed.is_empty() || elapsed == "0.0s" {
            vec![Span::styled("Thinking…".to_string(), label_style)]
        } else {
            vec![
                Span::styled("Thinking".to_string(), label_style),
                Span::styled(format!(" · {elapsed}"), detail_style),
            ]
        }
    } else {
        vec![
            Span::styled("Thought".to_string(), label_style),
            Span::styled(format!(" for {}", t.format_elapsed()), detail_style),
        ]
    };

    // Expand affordance only when collapsed and finished (something to open).
    if !t.is_running() && t.collapsed {
        let hint = "  (e expand)";
        let used: usize = header_spans
            .iter()
            .map(|s| s.content.as_ref().chars().count())
            .sum();
        if used + hint.chars().count() < content_w as usize {
            header_spans.push(Span::styled(hint.to_string(), detail_style));
        }
    }

    lines.push(accent_line(header_spans, show_rail, rail_style));

    if !show_rail {
        return lines;
    }

    let body = t.body_lines();
    if t.is_truncated_live() {
        // Ellipsis row when we dropped earlier reasoning lines.
        lines.push(accent_line(
            vec![Span::styled("…".to_string(), body_style)],
            true,
            rail_style,
        ));
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
                rail_style,
            ));
        }
    }
    if t.is_truncated_expanded() {
        lines.push(accent_line(
            vec![Span::styled(
                "… expanded view truncated for speed  ·  (e collapse)".to_string(),
                body_style,
            )],
            true,
            rail_style,
        ));
    }
    lines
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

/// Grok session event: `Worked for 12s` (+ optional token usage on last turn).
///
/// Matches pager `SessionEvent::TurnCompleted` wording — past tense, no agent
/// badge, no `▣`. Cancelled turns are separate system messages.
fn turn_done_footer(msg: &crate::app::ChatMessage, is_last: bool, app: &TuiApp) -> String {
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
    s
}

/// Grok `prompt_arrow()`: U+276F HEAVY RIGHT-POINTING ANGLE QUOTATION MARK + space.
/// Always 2 columns wide.
const PROMPT_ARROW: &str = "\u{276F} ";
const PROMPT_ARROW_WIDTH: u16 = 2;

/// Grok-style user prompt block.
///
/// ```text
/// ┌──────────────── full-width elevated band ────────────────┐
/// │ ❯ first line of the prompt…                              │
/// │   soft-wrapped continuation                              │
/// └──────────────────────────────────────────────────────────┘
/// ```
///
/// Matches Grok pager `UserPromptBlock`:
/// - prefix `❯ ` in user accent (Grok `accent_user`)
/// - body primary fg on `bg_light` band (`status_bar_bg` / panel step)
/// - vertical pad rows with the same band
/// - no left `┃` rail (accent is the arrow, not a border)
fn user_prompt_lines(
    content: &str,
    image_labels: &[String],
    palette: &ThemePalette,
    width: u16,
    is_selected: bool,
) -> Vec<Line<'static>> {
    // Grok: elevated band = bg_light; selected steps up slightly in native mode.
    let band = if is_selected {
        palette.input_bg
    } else {
        palette.status_bar_bg
    };
    let band_style = Style::default().bg(band);
    let prefix_style = Style::default().fg(palette.user_msg).bg(band);
    let body_style = Style::default().fg(palette.fg).bg(band);
    let img_style = Style::default()
        .fg(palette.accent)
        .bg(band)
        .add_modifier(Modifier::DIM);

    let mut lines = Vec::new();
    // vpad top (Grok PromptConfig.vpad = true)
    lines.push(band_pad_line(band_style));

    // Image attachment chips (file names from drag-drop / paste).
    if !image_labels.is_empty() {
        let chips = image_labels
            .iter()
            .map(|l| format!("🖼 {l}"))
            .collect::<Vec<_>>()
            .join("  ");
        lines.push(Line::from(vec![
            Span::styled(PROMPT_ARROW.to_string(), prefix_style),
            Span::styled(chips, img_style),
        ]));
    }

    let content_w = width.saturating_sub(PROMPT_ARROW_WIDTH).max(4) as usize;
    // When we already showed image chips with the arrow, indent body lines.
    let text = content.trim_end_matches('\n');
    // Skip redundant "[Image: …]" body when labels already render chips and
    // content is the synthetic image-only placeholder.
    let skip_body =
        !image_labels.is_empty() && (text.starts_with("[Image:") || text.starts_with("[Images:"));
    if skip_body {
        lines.push(band_pad_line(band_style));
        return lines;
    }

    if text.is_empty() {
        if image_labels.is_empty() {
            lines.push(Line::from(vec![
                Span::styled(PROMPT_ARROW.to_string(), prefix_style),
                Span::styled(" ".to_string(), body_style),
            ]));
        }
    } else {
        // Soft-wrap per logical line so explicit newlines stay as hard breaks
        // (same shape as Grok wrap_prompt_lines).
        // If chips already used the ❯ line, body lines are indented continuations.
        let mut first_visual = image_labels.is_empty();
        for logical in text.split('\n') {
            if logical.is_empty() {
                // Empty logical line: show arrow / indent only.
                if first_visual {
                    lines.push(Line::from(vec![Span::styled(
                        PROMPT_ARROW.to_string(),
                        prefix_style,
                    )]));
                    first_visual = false;
                } else {
                    lines.push(Line::from(vec![Span::styled(
                        " ".repeat(PROMPT_ARROW_WIDTH as usize),
                        prefix_style,
                    )]));
                }
                continue;
            }
            let wrapped = crate::widgets::wrap::wrap_text(logical, content_w as u16);
            if wrapped.is_empty() {
                continue;
            }
            for (wrap_i, row) in wrapped.iter().enumerate() {
                let slice = logical[row.byte_range.0..row.byte_range.1].trim_end();
                let is_block_first = first_visual && wrap_i == 0;
                let prefix = if is_block_first {
                    PROMPT_ARROW
                } else {
                    // Continuation indent = prefix width (Grok: "  ").
                    "  "
                };
                lines.push(Line::from(vec![
                    Span::styled(prefix.to_string(), prefix_style),
                    Span::styled(slice.to_string(), body_style),
                ]));
                first_visual = false;
            }
        }
        if first_visual {
            // content was only newlines / empty after split edge case
            lines.push(Line::from(vec![
                Span::styled(PROMPT_ARROW.to_string(), prefix_style),
                Span::styled(" ".to_string(), body_style),
            ]));
        }
    }

    // vpad bottom
    lines.push(band_pad_line(band_style));
    lines
}

/// One empty elevated-band row (Grok prompt vertical pad).
fn band_pad_line(band_style: Style) -> Line<'static> {
    // A single space carries the bg so SparseLines can full-width fill the row.
    Line::from(Span::styled(" ".to_string(), band_style))
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
fn system_callout(content: &str, palette: &ThemePalette, _width: u16) -> Vec<Line<'static>> {
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

    for line in body.iter().skip(1) {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        // Steps / detail sit one level quieter under the title.
        lines.push(Line::from(vec![
            Span::styled("│ ".to_string(), Style::default().fg(accent)),
            Span::styled(format!("  {t}"), Style::default().fg(palette.dim)),
        ]));
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

/// Quiet Grok-style tool chrome: muted name · summary, heavy ┃ body when open.
///
/// Matches Grok pager tool cards interleaved with thinking:
/// ```text
/// Thinking…
/// ┃ …
///   read · path/to/file.rs
///   ┃     1|code
///   run · cargo test
///   ┃ ok
/// Thought for 2.1s
/// ```
fn tool_block(
    name: &str,
    input: &serde_json::Value,
    result: Option<&str>,
    is_error: bool,
    palette: &ThemePalette,
    expanded: bool,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let display = tool_display_name(name);
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

    let mut header = vec![meta_gutter(), Span::styled(display.to_string(), name_style)];
    if !summary.is_empty() {
        header.push(Span::styled(" · ".to_string(), detail));
        header.push(Span::styled(summary, summary_style));
    }

    if let Some(r) = result {
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
        } else if matches!(name, "grep" | "search_code" | "rg") {
            // Grok: match count sits on the header next to the pattern.
            if let Some(n) = grep_match_count(r) {
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
        } else if matches!(name, "bash" | "shell" | "run_terminal_command") {
            // run: short exit/line chip when finished (quiet, Grok-like).
            let n = r.lines().filter(|l| !l.trim().is_empty()).count();
            if n > 0 {
                header.push(Span::styled(
                    format!("  {n}"),
                    Style::default().fg(palette.dim),
                ));
                header.push(Span::styled(
                    if n == 1 {
                        " line".to_string()
                    } else {
                        " lines".to_string()
                    },
                    detail,
                ));
            }
        } else if matches!(name, "read" | "read_file") {
            // read: line count chip when we have numbered body.
            let n = r.lines().filter(|l| split_read_line(l).is_some()).count();
            if n > 0 {
                header.push(Span::styled(
                    format!("  {n}"),
                    Style::default().fg(palette.dim),
                ));
                header.push(Span::styled(
                    if n == 1 {
                        " line".to_string()
                    } else {
                        " lines".to_string()
                    },
                    detail,
                ));
            }
        }
        if !expanded {
            let n = r.lines().count();
            let limit = if looks_like_diff(r) {
                TOOL_RESULT_DIFF_PREVIEW
            } else {
                TOOL_RESULT_PREVIEW
            };
            if n > limit {
                header.push(Span::styled("  (l expand)".to_string(), detail));
            }
        }
    } else {
        // Still running — quiet live marker (Grok in-flight tool).
        header.push(Span::styled("  …".to_string(), detail));
    }

    lines.push(Line::from(header));

    // Always show a short body when a result exists; `l` expands the budget.
    if let Some(r) = result {
        let hint = tool_out_hint(name, input, r);
        lines.extend(tool_result(r, is_error, palette, expanded, hint, width));
    }
    lines
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

/// Collapsed = short preview; expanded (`l`) = long tail.
const TOOL_RESULT_PREVIEW: usize = 12;
const TOOL_RESULT_DIFF_PREVIEW: usize = 20;
const TOOL_RESULT_EXPANDED: usize = 120;

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
                format!("… {} more lines  ·  (l expand)", total - budget),
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
                format!("… {} more lines  ·  (l expand)", total - budget),
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
                format!("… {} more lines  ·  (l expand)", total - budget),
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
            Span::styled("… more matches  ·  (l expand)".to_string(), meta_style),
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
            .get("command")
            .and_then(|v| v.as_str())
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
        } else if raw.len() > 56 {
            format!("{}…", &raw[..56])
        } else {
            raw
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
        SparseLines, ToolOutHint, hard_truncate_line, message_row_layout_mut, parse_grep_hit,
        prettify_tool_result, split_read_line, tool_block, tool_display_name, tool_result,
        tool_summary,
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
            false,
            &palette,
            false,
            100,
        );
        let header: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(header.contains("grep"), "got {header}");
        assert!(header.contains("foo"), "got {header}");
        assert!(header.contains("2"), "got {header}");
        assert!(header.contains("match"), "got {header}");
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
            false,
            &palette,
            false,
            100,
        );
        let header: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(header.contains("run"), "got {header}");
        assert!(!header.contains("bash"), "got {header}");
        assert!(header.contains("cargo test"), "got {header}");
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
}

#[cfg(test)]
#[path = "chat_scroll_tests.rs"]
mod scroll_tests;
