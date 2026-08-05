// ── ui/chat.rs: session message list ───────────────────────────────────
// User prompt: Grok-style elevated band + ❯ prefix (not OpenCode ┃ panel)
// Assistant: free parts + Grok turn footer ("Worked for 12s")
// Home: centered dual-block logo

use crate::app::{ChatBlock, ChatRole, TuiApp};
use crate::opencode_tokens::{LOGO_WHY, LOGO_WHY_CODE, layout as oc};
use crate::theme::ThemePalette;
use crate::ui::scrollbar::{ScrollbarColors, paint_scrollbar};
use crate::widgets::wrap::{wrap_plain, wrap_text};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Widget},
};
use unicode_width::UnicodeWidthStr;
use whycode_format::diff::looks_like_diff;
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
        total += render_message(msg, app, &palette, i, width).len();
    }
    (starts, total)
}

/// Like [`message_row_layout`] but writes height caches on miss.
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
        } else {
            let h = {
                let palette = app.config.palette();
                render_message(&app.messages[i], app, &palette, i, width).len()
            };
            app.messages[i].layout_cache = Some((width, busy, h));
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
    // Vertical centering like home.tsx flexGrow spacers
    let content_h = 4 + 1 + 2 + 2; // logo + gap + meta + hints
    let top = area.height.saturating_sub(content_h) / 2;
    for _ in 0..top {
        lines.push(Line::from(""));
    }

    // Center logo horizontally
    let logo_w = LOGO_WHY[1].chars().count() + 1 + LOGO_WHY_CODE[1].chars().count();
    let left_pad = area
        .width
        .saturating_sub(logo_w as u16 + 2)
        .saturating_div(2) as usize;
    let pad = " ".repeat(left_pad);

    for i in 0..4 {
        lines.push(Line::from(vec![
            Span::raw(pad.clone()),
            Span::styled(LOGO_WHY[i].to_string(), Style::default().fg(palette.dim)),
            Span::raw(" "),
            Span::styled(
                LOGO_WHY_CODE[i].to_string(),
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
    for i in 0..n {
        let msg_start = starts[i];
        let msg_end = if i + 1 < n { starts[i + 1] } else { total };
        // Skip bubbles wholly above or below the viewport.
        if msg_end <= view_start || msg_start >= view_end {
            continue;
        }

        let selected =
            app.selected_msg == Some(i) && app.focus == crate::app::FocusPane::Scrollback;
        let mut msg_lines = render_message(&app.messages[i], app, palette, i, content_width);
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
            // Full-width elevated band (Grok user prompt / Light block bg).
            if let Some(bg) = band_bg {
                while x < end {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        if cell.symbol() == " " || cell.symbol().is_empty() {
                            cell.set_symbol(" ");
                            cell.set_style(Style::default().bg(bg));
                        } else {
                            let mut st = cell.style();
                            st.bg = Some(bg);
                            cell.set_style(st);
                        }
                    }
                    x = x.saturating_add(1);
                }
            }
        }
    }
}

fn render_message(
    msg: &crate::app::ChatMessage,
    app: &TuiApp,
    palette: &ThemePalette,
    index: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    // marginTop=1 between messages (OpenCode)
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
            // Grok-style turn footer (session event marker), not OpenCode's
            // "▣ agent · 12s" badge — past tense, muted, no agent name clutter.
            //
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
                epi.push(Span::styled(
                    footer,
                    Style::default().fg(palette.dim),
                ));
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
        lines.extend(system_callout(
            &format!("Error: {err}"),
            palette,
            width,
        ));
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

    // Header: split "Thought" / " for Xs" like Grok.
    let mut header_spans: Vec<Span<'static>> = if t.is_running() {
        vec![Span::styled("Thinking…".to_string(), label_style)]
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
    Span::raw(" ".repeat(oc::ASSISTANT_PAD as usize))
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
}

fn tool_out_hint(name: &str, input: &serde_json::Value, result: &str) -> ToolOutHint {
    match name {
        "git_diff" | "apply_patch" => ToolOutHint::Diff,
        "edit" if looks_like_diff(result) => ToolOutHint::Diff,
        "read" => {
            let path = input
                .get("path")
                .or_else(|| input.get("file_path"))
                .or_else(|| input.get("file"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            ToolOutHint::Code(detect_language(path).map(str::to_string))
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
        if line.starts_with('+') {
            add += 1;
        } else if line.starts_with('-') {
            del += 1;
        }
    }
    (add, del)
}

/// Quiet Grok-style tool chrome: muted name · summary, heavy ┃ body when open.
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
    let summary = tool_summary(input);

    let mut header = vec![
        meta_gutter(),
        Span::styled(name.to_string(), name_style),
    ];
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
                header.push(Span::styled(
                    format!("  +{a} −{d}"),
                    Style::default().fg(palette.dim),
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
        // Still running — quiet live marker.
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

    let mode = match &hint {
        ToolOutHint::Diff => ToolOutHint::Diff,
        ToolOutHint::Code(lang) => ToolOutHint::Code(lang.clone()),
        ToolOutHint::Auto => {
            if looks_like_diff(content) {
                ToolOutHint::Diff
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
        ToolOutHint::Auto => tool_result_plain(content, is_error, palette, expanded, width),
    }
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
    let text_w = width.saturating_sub(4).max(8);
    let all_lines: Vec<&str> = content.lines().collect();
    let total = all_lines.len();
    let mut lines = Vec::new();
    let mut visual = 0usize;
    for line in all_lines.iter().take(budget) {
        for row in wrap_plain(line, text_w, style) {
            if visual >= budget {
                break;
            }
            let mut spans = vec![
                meta_gutter(),
                Span::styled("┃ ".to_string(), rail),
            ];
            spans.extend(row.spans);
            lines.push(Line::from(spans));
            visual += 1;
        }
        if visual >= budget {
            break;
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
    let text_w = width.saturating_sub(4).max(8) as usize;
    let mut lines = Vec::new();
    for line in content.lines().take(budget) {
        let (rail_color, body_color) = if is_error {
            (palette.error, palette.error)
        } else if line.starts_with("+++") || line.starts_with("---") {
            (palette.dim, palette.fg)
        } else if line.starts_with("@@") || line.starts_with("diff --git") {
            (palette.diff_hunk, palette.diff_hunk)
        } else if line.starts_with('+') {
            (palette.diff_add, palette.diff_add)
        } else if line.starts_with('-') {
            (palette.diff_remove, palette.diff_remove)
        } else if line.starts_with("Edited ") || line.starts_with('…') {
            (palette.dim, palette.fg)
        } else {
            (palette.dim, palette.dim)
        };
        let shown = if line.chars().count() > text_w {
            format!(
                "{}…",
                line.chars().take(text_w.saturating_sub(1)).collect::<String>()
            )
        } else {
            line.to_string()
        };
        lines.push(Line::from(vec![
            meta_gutter(),
            Span::styled("┃ ".to_string(), Style::default().fg(rail_color)),
            Span::styled(shown, Style::default().fg(body_color)),
        ]));
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

/// Syntax-highlight tool output (read previews). Line-numbered `read` rows
/// keep the gutter dim and highlight only the code after `|`.
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
    let all: Vec<&str> = content.lines().collect();
    let total = all.len();
    let slice = &all[..total.min(budget)];
    let rail = Style::default().fg(palette.dim);

    let mut code_body = String::new();
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
        Some(highlight_code_spans(
            &slice.join("\n"),
            language.as_deref(),
        ))
    } else {
        None
    };

    let mut lines = Vec::new();
    let mut code_idx = 0usize;
    match highlighted {
        Some(hl) if meta.iter().any(|(is_code, _, _)| *is_code) => {
            for (is_code, left, _) in &meta {
                if *is_code {
                    let mut spans = vec![
                        meta_gutter(),
                        Span::styled("┃ ".to_string(), rail),
                        Span::styled(left.clone(), Style::default().fg(palette.dim)),
                    ];
                    if let Some(row) = hl.get(code_idx) {
                        for ((r, g, b), text) in row.iter() {
                            spans.push(Span::styled(
                                text.trim_end_matches('\n').to_string(),
                                Style::default().fg(Color::Rgb(*r, *g, *b)),
                            ));
                        }
                    }
                    code_idx += 1;
                    lines.push(Line::from(spans));
                } else {
                    lines.push(Line::from(vec![
                        meta_gutter(),
                        Span::styled("┃ ".to_string(), rail),
                        Span::styled(left.clone(), Style::default().fg(palette.dim)),
                    ]));
                }
            }
        }
        Some(hl) => {
            for row in hl.iter().take(budget) {
                let mut spans = vec![meta_gutter(), Span::styled("┃ ".to_string(), rail)];
                for ((r, g, b), text) in row.iter() {
                    spans.push(Span::styled(
                        text.trim_end_matches('\n').to_string(),
                        Style::default().fg(Color::Rgb(*r, *g, *b)),
                    ));
                }
                lines.push(Line::from(spans));
            }
        }
        None => return tool_result_plain(content, is_error, palette, expanded, width),
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
    }
    lines
}

/// `read` tool lines look like `   12|contents` (6-wide line no + `|`).
fn split_read_line(line: &str) -> Option<(String, &str)> {
    let pipe = line.find('|')?;
    let (left, right) = line.split_at(pipe);
    if left.is_empty() || !left.chars().all(|c| c.is_ascii_digit() || c == ' ') {
        return None;
    }
    Some((format!("{left}|"), right.trim_start_matches('|')))
}

fn tool_summary(input: &serde_json::Value) -> String {
    let s = input
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
        .to_string();
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
    use super::{SparseLines, message_row_layout_mut};
    use crate::app::{ChatRole, TuiApp};
    use crate::config::TuiAppConfig;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::Widget;

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
