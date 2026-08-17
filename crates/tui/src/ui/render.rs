// ── ui/render.rs: home + session shells ────────────────────────────────
//
// Home: header · centered logo + prompt · footer (branch · cwd).
// Session: header · [ main | sidebar? ] · footer; main has side pad,
// messages scroll, prompt at bottom. Turn-status strip while busy;
// viewport metrics for row-based scrollback selection.

use crate::app::{AgentState, TuiApp};
use crate::theme::ThemePalette;
use crate::tokens::layout;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::chat;
use super::dialogs;
use super::file_suggest;
use super::prompt;
use super::sidebar;
use super::slash_suggest;
use super::status;
use super::subagents;
use super::toast;

const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn render(frame: &mut Frame, app: &mut TuiApp) {
    // Via the config rather than `app.theme`, so a palette loaded from a JSON
    // theme file takes precedence over the built-in of the same name.
    let palette = app.config.palette();

    frame.render_widget(
        Block::default().style(Style::default().bg(palette.bg)),
        frame.area(),
    );

    if app.dialogs.is_open() {
        render_shell(frame, app, &palette);
        dialogs::render(frame, app, &palette);
        paint_selection(frame, app);
        return;
    }

    if let crate::app::AppMode::Help = app.mode {
        render_shell(frame, app, &palette);
        dialogs::render_help(frame, app, &palette);
        // Toast above help so "Copied N chars" is visible after a modal select.
        toast::render(
            frame,
            layout::inset_safe(frame.area()),
            app.toasts.visible(),
            &palette,
        );
        paint_selection(frame, app);
        return;
    }

    render_shell(frame, app, &palette);
    // Last, so toasts sit above the chat. Not drawn over a dialog: a modal has
    // the user's attention already, and covering its corner would obscure it.
    toast::render(
        frame,
        layout::inset_safe(frame.area()),
        app.toasts.visible(),
        &palette,
    );
    paint_selection(frame, app);
}

/// Reverse-video a **linear** selection (Grok / native terminal shape).
///
/// Only content cells are painted — trailing pad on short lines is left alone
/// so the highlight matches what `clipboard::text_from_cells` will copy.
/// When a modal is open, ranges are clipped to `dialog_modal_hit` so the
/// background chat is never highlighted.
fn paint_selection(frame: &mut Frame, app: &TuiApp) {
    let Some(sel) = app.mouse_sel else {
        return;
    };

    let clip = app
        .dialog_modal_hit
        .map(crate::clipboard::ClipRect::from_ratatui);

    // Prefer the previous frame's cell snapshot (same grid copy uses). On the
    // first drag frame it may still be empty — fall back to linear geometry.
    let ranges = if app.screen_cells.is_empty() {
        let area = frame.area();
        let row_max = area.width.saturating_sub(1);
        let (top_y, bot_y, top_x, bot_x) = if sel.anchor_y < sel.focus_y {
            (sel.anchor_y, sel.focus_y, sel.anchor_x, sel.focus_x)
        } else if sel.focus_y < sel.anchor_y {
            (sel.focus_y, sel.anchor_y, sel.focus_x, sel.anchor_x)
        } else {
            let lo = sel.anchor_x.min(sel.focus_x);
            let hi = sel.anchor_x.max(sel.focus_x);
            (sel.anchor_y, sel.focus_y, lo, hi)
        };
        let mut r = Vec::new();
        for y in top_y..=bot_y {
            if let Some(c) = clip
                && !c.contains_y(y)
            {
                continue;
            }
            if let Some((xs, xe)) =
                crate::clipboard::linear_cols(y, top_y, bot_y, top_x, bot_x, row_max)
            {
                let (xs, xe) = match clip.and_then(|c| c.col_range()) {
                    Some((cx0, cx1)) => {
                        let xs = xs.max(cx0);
                        let xe = xe.min(cx1);
                        if xs > xe {
                            continue;
                        }
                        (xs, xe)
                    }
                    None => (xs, xe),
                };
                r.push((y, xs, xe));
            }
        }
        r
    } else {
        crate::clipboard::paint_ranges_clipped(
            &app.screen_cells,
            sel.anchor_x,
            sel.anchor_y,
            sel.focus_x,
            sel.focus_y,
            clip,
        )
    };

    let area = frame.area();
    let buf = frame.buffer_mut();
    for (y, xs, xe) in ranges {
        if y < area.y || y >= area.y.saturating_add(area.height) {
            continue;
        }
        for x in xs..=xe {
            if x < area.x || x >= area.x.saturating_add(area.width) {
                continue;
            }
            // `paint_ranges` already clamps to first/last non-pad, so interior
            // spaces (between words) stay inside [xs, xe] and get highlighted.
            let cell = &mut buf[(x, y)];
            let style = cell.style().add_modifier(Modifier::REVERSED);
            cell.set_style(style);
        }
    }
}

fn render_shell(frame: &mut Frame, app: &mut TuiApp, palette: &ThemePalette) {
    let area = layout::inset_safe(frame.area());

    // Top: status · shortcuts. Bottom: git branch + cwd (click-to-copy).
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);

    status::render(frame, outer[0], app, palette);
    // One blank row under the header so the transcript / home logo / sidebar
    // cannot paint into that chrome (safe-area + header stay empty of chat).
    let body = layout::below_header(outer[1]);
    status::render_footer(frame, outer[2], app, palette);

    let strip_h = subagents::strip_height(app);
    let (strip, body) = if strip_h > 0 {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(strip_h), Constraint::Min(3)])
            .split(body);
        (Some(chunks[0]), chunks[1])
    } else {
        (None, body)
    };
    if let Some(area) = strip {
        subagents::render_strip(frame, area, app, palette);
    }

    if app.messages.is_empty() {
        render_home(frame, body, app, palette);
    } else {
        render_session(frame, body, app, palette);
    }

    if app.open_subagent.is_some() {
        let frame_area = inset_centered(outer[1], 4, 2);
        subagents::render_frame(frame, frame_area, app, palette);
    }
}

fn inset_centered(area: Rect, pad_x: u16, pad_y: u16) -> Rect {
    Rect {
        x: area.x.saturating_add(pad_x),
        y: area.y.saturating_add(pad_y),
        width: area.width.saturating_sub(pad_x.saturating_mul(2)),
        height: area.height.saturating_sub(pad_y.saturating_mul(2)),
    }
}

/// Home vertical stack: logo area grows, prompt fixed, no header chrome
fn render_home(frame: &mut Frame, area: Rect, app: &mut TuiApp, palette: &ThemePalette) {
    let content = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: area.height.saturating_sub(layout::BOTTOM_PAD),
    };
    let turn_h = turn_status_height(app);
    // Prefer the height the prompt actually needs. Leave a few rows for the
    // logo; do not force height/2 (that clipped the box on long pastes).
    let needed = prompt::prompt_height(app, content.width);
    let prompt_h = needed
        .min(content.height.saturating_sub(5))
        .clamp(1, content.height.max(1));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(layout::CHAT_GAP),
            Constraint::Length(turn_h),
            Constraint::Length(prompt_h),
        ])
        .split(content);

    app.chat_viewport_rows = chunks[0].height;
    app.chat_content_width = chunks[0].width;

    chat::render(frame, chunks[0], app, palette);
    if turn_h > 0 {
        render_turn_status(frame, chunks[2], app, palette);
    }
    prompt::render(frame, chunks[3], app, palette);
    slash_suggest::render(frame, chunks[3], app, palette);
    file_suggest::render(frame, chunks[3], app, palette);
}

/// session: optional sidebar + padded main column
fn render_session(frame: &mut Frame, area: Rect, app: &mut TuiApp, palette: &ThemePalette) {
    let main = if app.sidebar.visible && area.width >= 36 {
        let w = layout::SIDEBAR_WIDTH.min(area.width / 3).max(24);
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(20), Constraint::Length(w)])
            .split(area);
        // Main left, sidebar right
        sidebar::render(frame, chunks[1], app, palette);
        chunks[0]
    } else {
        area
    };

    let inset = Rect {
        x: main.x.saturating_add(layout::SIDE_PAD),
        y: main.y,
        width: main.width.saturating_sub(layout::SIDE_PAD * 2),
        height: main.height.saturating_sub(layout::BOTTOM_PAD),
    };

    let turn_h = turn_status_height(app);
    let needed = prompt::prompt_height(app, inset.width);
    let prompt_h = needed
        .min(inset.height.saturating_sub(3))
        .clamp(1, inset.height.max(1));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),                   // scroll messages
            Constraint::Length(layout::CHAT_GAP), // safezone above stop / prompt
            Constraint::Length(turn_h),           // Grok turn status (busy only)
            Constraint::Length(prompt_h),         // boxed prompt (╭ text ╰ meta) + hint
        ])
        .split(inset);

    app.chat_viewport_rows = chunks[0].height;
    app.chat_content_width = chunks[0].width;

    chat::render(frame, chunks[0], app, palette);
    if turn_h > 0 {
        render_turn_status(frame, chunks[2], app, palette);
    }
    prompt::render(frame, chunks[3], app, palette);
    slash_suggest::render(frame, chunks[3], app, palette);
    file_suggest::render(frame, chunks[3], app, palette);
}

fn turn_status_height(app: &mut TuiApp) -> u16 {
    if app.is_busy() {
        1
    } else {
        // Control not painted — drop sticky hover + rect.
        app.turn_stop_hit.clear();
        0
    }
}

/// Busy strip (Grok turn_status): spinner + activity · detail …… 1m20s ⇣12k [stop]
///
/// `[stop]` is mouse-clickable (sticky hit → cancel). Esc still cancels from the
/// header shortcuts / key handler.
fn render_turn_status(frame: &mut Frame, area: Rect, app: &mut TuiApp, palette: &ThemePalette) {
    app.turn_stop_hit.set_rect(None);
    if area.height == 0 {
        return;
    }
    let spin = SPINNER[app.spinner_frame % SPINNER.len()];
    let thinking_elapsed = if matches!(app.current_agent_state, AgentState::Thinking) {
        app.messages
            .last()
            .and_then(|m| {
                m.blocks.iter().rev().find_map(|b| match b {
                    crate::app::ChatBlock::Thinking(t) if t.is_running() => {
                        Some(t.format_elapsed())
                    }
                    _ => None,
                })
            })
            .unwrap_or_default()
    } else {
        String::new()
    };
    let turn_elapsed = app
        .turn_elapsed_ms()
        .map(crate::app::format_elapsed_ms)
        .unwrap_or_default();
    let color = match &app.current_agent_state {
        AgentState::Thinking => palette.thinking,
        AgentState::WaitingForPermission | AgentState::WaitingForQuestion => palette.warning,
        AgentState::Generating => palette.accent,
        AgentState::Error(_) => palette.error,
        AgentState::Idle => palette.success,
    };
    let label = match &app.current_agent_state {
        AgentState::Thinking if !thinking_elapsed.is_empty() => {
            format!("thinking {thinking_elapsed}")
        }
        AgentState::Thinking => "thinking".into(),
        AgentState::WaitingForPermission => "waiting for permission".into(),
        AgentState::WaitingForQuestion => "waiting for answer".into(),
        AgentState::Generating if !turn_elapsed.is_empty() => {
            format!("generating {turn_elapsed}")
        }
        AgentState::Generating => "generating".into(),
        AgentState::Error(_) => "error".into(),
        AgentState::Idle => "ready".into(),
    };
    let detail = turn_status_detail(&app.status_message, &label);

    // Right cluster: optional ⇣tokens + [stop]
    let tokens_s = app.turn_usage.as_ref().map(|u| {
        let n = u.input_tokens + u.output_tokens;
        format!("⇣{}", crate::app::format_token_count(n))
    });
    let stop_label = "[stop]";
    let stop_hovered = app.turn_stop_hit.hovered;
    let stop_style = if stop_hovered {
        Style::default()
            .fg(palette.error)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    } else {
        Style::default().fg(palette.error)
    };

    let mut right_w: u16 = 0;
    if let Some(ref t) = tokens_s {
        right_w = right_w.saturating_add(t.width() as u16).saturating_add(1);
    }
    right_w = right_w.saturating_add(stop_label.width() as u16);

    let left_budget = area.width.saturating_sub(right_w.saturating_add(1)) as usize;
    let mut left_spans = vec![
        Span::styled(
            format!("{spin} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            label.clone(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
    ];
    let mut used = 2 + label.width(); // spin + space + label (approx)
    if !detail.is_empty() {
        let d = format!(" · {detail}");
        let room = left_budget.saturating_sub(used);
        if room > 3 {
            let d = if d.width() > room {
                let keep: String = d.chars().take(room.saturating_sub(1)).collect();
                format!("{keep}…")
            } else {
                d
            };
            used += d.width();
            left_spans.push(Span::styled(d, Style::default().fg(palette.dim)));
        }
    }

    let gap = area
        .width
        .saturating_sub(used as u16)
        .saturating_sub(right_w);
    let mut spans = left_spans;
    if gap > 0 {
        spans.push(Span::raw(" ".repeat(gap as usize)));
    }
    if let Some(ref t) = tokens_s {
        spans.push(Span::styled(
            format!("{t} "),
            Style::default().fg(palette.dim),
        ));
    }
    let stop_x = area
        .x
        .saturating_add(area.width.saturating_sub(stop_label.width() as u16));
    spans.push(Span::styled(stop_label.to_string(), stop_style));

    app.turn_stop_hit.set_rect(Some(Rect {
        x: stop_x,
        y: area.y,
        width: stop_label.width() as u16,
        height: 1,
    }));

    frame.render_widget(
        Paragraph::new(Text::from(Line::from(spans))).style(Style::default().bg(palette.bg)),
        area,
    );
}

/// Clean agent/status text for the busy strip — drop spinner chrome, cancel
/// hints, and anything that only restates the state label.
fn turn_status_detail(status: &str, label: &str) -> String {
    let cleaned = strip_status_chrome(status);
    if cleaned.is_empty() {
        return String::new();
    }
    let lower = cleaned.to_ascii_lowercase();
    // Generic "Generating…" is already the strip label.
    if lower == "generating"
        || lower == "generating…"
        || lower == "generating..."
        || lower.starts_with("generating…")
        || lower.starts_with("generating...")
    {
        return String::new();
    }
    // Avoid "thinking 1.4s · thinking…" style doubles.
    let label_head = label
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if !label_head.is_empty()
        && lower.starts_with(&label_head)
        && lower.len() <= label_head.len() + 4
    {
        return String::new();
    }
    // "Running tool `foo`…" → "tool: foo"
    let display = if let Some(rest) = cleaned.strip_prefix("Running tool ") {
        let name: String = rest
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == '.')
            .collect();
        if name.is_empty() {
            cleaned
        } else {
            format!("tool: {name}")
        }
    } else {
        cleaned
    };
    truncate_mid(&display, 40)
}

fn strip_status_chrome(s: &str) -> String {
    let mut out = s.trim().to_string();
    // Leading braille spinner frames from older status writers.
    const SPIN: &str = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏";
    while out
        .chars()
        .next()
        .map(|c| SPIN.contains(c))
        .unwrap_or(false)
    {
        out = out
            .chars()
            .skip(1)
            .collect::<String>()
            .trim_start()
            .to_string();
    }
    for noise in ["[Esc cancel]", "[esc cancel]", "Esc cancel", "esc cancel"] {
        out = out.replace(noise, "");
    }
    // Collapse leftover whitespace / separators.
    let parts: Vec<&str> = out.split_whitespace().collect();
    parts
        .join(" ")
        .trim_matches(|c: char| c == '·' || c.is_whitespace())
        .to_string()
}

fn truncate_mid(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        s.to_string()
    } else {
        let keep = max.saturating_sub(1);
        format!("{}…", s.chars().take(keep).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{AgentState, TuiApp};
    use crate::config::TuiAppConfig;

    #[test]
    fn strip_status_chrome_drops_cancel_and_spinner() {
        assert_eq!(
            strip_status_chrome("⠋ Generating…  [Esc cancel]"),
            "Generating…"
        );
        assert_eq!(
            strip_status_chrome("LLM request (step 1)…  [Esc cancel]"),
            "LLM request (step 1)…"
        );
        assert_eq!(strip_status_chrome("tool: bash  Esc cancel"), "tool: bash");
    }

    #[test]
    fn turn_status_detail_skips_generic_generating() {
        assert_eq!(turn_status_detail("Generating…", "generating 1.2s"), "");
        assert_eq!(
            turn_status_detail("LLM request (step 1)…  [Esc cancel]", "generating 1.2s"),
            "LLM request (step 1)…"
        );
        assert_eq!(
            turn_status_detail("Running tool `read`…", "generating 3s"),
            "tool: read"
        );
    }

    #[test]
    fn turn_status_detail_handles_generating_variants_and_empty() {
        // All "generating…" spellings collapse to nothing.
        assert_eq!(turn_status_detail("Generating", "generating 1.2s"), "");
        assert_eq!(turn_status_detail("Generating...", "generating"), "");
        assert_eq!(turn_status_detail("Generating… 3 steps", "generating"), "");
        // Empty / chrome-only status → no detail.
        assert_eq!(turn_status_detail("", "generating"), "");
        assert_eq!(turn_status_detail("  [Esc cancel]  ", "generating"), "");
    }

    #[test]
    fn turn_status_detail_dedupes_label_head() {
        // "thinking · thinking…" style double is dropped.
        assert_eq!(turn_status_detail("thinking…", "thinking 1.4s"), "");
        assert_eq!(turn_status_detail("thinking", "thinking 1.4s"), "");
        // A longer detail that merely starts with the label survives.
        assert_eq!(
            turn_status_detail("thinking about next step", "thinking 1.4s"),
            "thinking about next step"
        );
    }

    #[test]
    fn turn_status_detail_cleans_tool_names() {
        // Tool name keeps only safe identifier chars.
        assert_eq!(
            turn_status_detail("Running tool `git status`…", "generating 3s"),
            "tool: gitstatus"
        );
        assert_eq!(
            turn_status_detail("Running tool `read`…", "generating 3s"),
            "tool: read"
        );
        // Backtick content with nothing usable → original text kept.
        assert_eq!(
            turn_status_detail("Running tool ``…", "generating 3s"),
            "Running tool ``…"
        );
    }

    #[test]
    fn turn_status_detail_truncates_mid() {
        let long = "a".repeat(100);
        let detail = turn_status_detail(&long, "generating");
        assert!(detail.ends_with('…'), "{detail}");
        assert_eq!(detail.chars().count(), 40, "{detail}");
    }

    #[test]
    fn strip_status_chrome_extra_cases() {
        // Multiple leading spinners + noise + separators.
        assert_eq!(
            strip_status_chrome("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏⠋⠋  hello · world  ·"),
            "hello · world"
        );
        assert_eq!(strip_status_chrome("  plain text  "), "plain text");
        assert_eq!(strip_status_chrome("Esc cancel · ready"), "ready");
        assert_eq!(strip_status_chrome(""), "");
    }

    #[test]
    fn truncate_mid_keeps_short_and_ellipsizes_long() {
        assert_eq!(truncate_mid("short", 40), "short");
        assert_eq!(truncate_mid("", 5), "");
        assert_eq!(truncate_mid("abcdef", 3), "ab…");
        assert_eq!(truncate_mid("abcdef", 6), "abcdef");
        // Multibyte chars counted, not bytes (keep = max - 1).
        assert_eq!(truncate_mid("çok uzun bir başlık", 5), "çok …");
    }

    #[test]
    fn turn_status_height_tracks_busy_and_clears_stop_hit() {
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.turn_stop_hit.set_rect(Some(Rect::new(10, 10, 6, 1)));
        assert_eq!(turn_status_height(&mut app), 0);
        assert!(app.turn_stop_hit.rect.is_none(), "idle clears the stop hit");

        app.current_agent_state = AgentState::Generating;
        assert_eq!(turn_status_height(&mut app), 1);
        app.current_agent_state = AgentState::Thinking;
        assert_eq!(turn_status_height(&mut app), 1);
    }
}

#[cfg(test)]
mod paint_tests {
    use super::*;
    use crate::app::{ChatBlock, ThinkingBlock, TuiApp};
    use crate::config::TuiAppConfig;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;

    fn app() -> TuiApp {
        TuiApp::new(TuiAppConfig::default())
    }

    fn paint<F>(width: u16, height: u16, f: F) -> (ratatui::buffer::Buffer, String)
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
        (buf, out)
    }

    #[test]
    fn turn_status_generating_shows_spinner_label_tokens_and_stop() {
        let mut a = app();
        a.current_agent_state = AgentState::Generating;
        a.spinner_frame = 2;
        a.status_message = "Running tool `read`…".into();
        a.turn_usage = Some(whycode_core::types::Usage {
            input_tokens: 1_200,
            output_tokens: 80,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        });
        let palette = a.config.palette();
        let (buf, text) = paint(100, 1, |f| {
            render_turn_status(f, f.area(), &mut a, &palette);
        });
        assert!(text.contains("generating"), "{text}");
        assert!(text.contains("tool: read"), "{text}");
        assert!(text.contains("⇣1.3k"), "tokens shown: {text}");
        assert!(text.contains("[stop]"), "{text}");
        // [stop] right-aligned with its hit rect recorded.
        let rect = a.turn_stop_hit.rect.expect("stop hit rect");
        assert_eq!(rect.y, 0);
        assert_eq!(rect.x + rect.width, 100, "stop sits at the right edge");
        // Spinner glyph painted.
        let _ = buf;
    }

    #[test]
    fn turn_status_thinking_uses_running_block_elapsed() {
        let mut a = app();
        let mut tb = ThinkingBlock::new("reasoning");
        tb.collapsed = false;
        a.messages.push(chat_message_with_thinking(tb));
        a.current_agent_state = AgentState::Thinking;
        a.spinner_frame = 4;
        let palette = a.config.palette();
        let (_buf, text) = paint(100, 1, |f| {
            render_turn_status(f, f.area(), &mut a, &palette);
        });
        assert!(text.contains("thinking"), "{text}");
        assert!(text.contains("0.0s") || text.contains("thinking"), "{text}");
    }

    #[test]
    fn turn_status_waiting_and_error_labels() {
        let palette = app().config.palette();

        let mut a = app();
        a.current_agent_state = AgentState::WaitingForPermission;
        let (_buf, text) = paint(100, 1, |f| {
            render_turn_status(f, f.area(), &mut a, &palette);
        });
        assert!(text.contains("waiting for permission"), "{text}");

        let mut a = app();
        a.current_agent_state = AgentState::WaitingForQuestion;
        let (_buf, text) = paint(100, 1, |f| {
            render_turn_status(f, f.area(), &mut a, &palette);
        });
        assert!(text.contains("waiting for answer"), "{text}");

        let mut a = app();
        a.current_agent_state = AgentState::Error("boom".into());
        let (_buf, text) = paint(100, 1, |f| {
            render_turn_status(f, f.area(), &mut a, &palette);
        });
        assert!(text.contains("error"), "{text}");

        let mut a = app();
        a.current_agent_state = AgentState::Idle;
        let (_buf, text) = paint(100, 1, |f| {
            render_turn_status(f, f.area(), &mut a, &palette);
        });
        assert!(text.contains("ready"), "{text}");
    }

    #[test]
    fn turn_status_truncates_long_detail_and_stops() {
        let mut a = app();
        a.current_agent_state = AgentState::Generating;
        a.status_message = format!("long status {}", "x".repeat(120));
        let palette = a.config.palette();
        let (_buf, text) = paint(60, 1, |f| {
            render_turn_status(f, f.area(), &mut a, &palette);
        });
        // Detail truncated to fit the left budget, [stop] still visible.
        assert!(text.contains("…"), "{text}");
        assert!(text.contains("[stop]"), "{text}");
    }

    #[test]
    fn turn_status_hovered_stop_underlines() {
        let mut a = app();
        a.current_agent_state = AgentState::Generating;
        a.turn_stop_hit.hovered = true;
        let palette = a.config.palette();
        let (buf, _text) = paint(100, 1, |f| {
            render_turn_status(f, f.area(), &mut a, &palette);
        });
        // Find the [stop] cell and check it is underlined when hovered.
        let found = (0..100u16).any(|x| {
            let cell = buf.cell((x, 0)).unwrap();
            cell.symbol() == "[" && cell.style().add_modifier.contains(Modifier::UNDERLINED)
        });
        assert!(found, "hovered stop must be underlined");
    }

    fn chat_message_with_thinking(tb: ThinkingBlock) -> crate::app::ChatMessage {
        crate::app::ChatMessage {
            role: crate::app::ChatRole::Assistant,
            content: String::new(),
            blocks: vec![ChatBlock::Thinking(tb)],
            results_expanded: false,
            tool_calls: vec![],
            error: None,
            duration_ms: None,
            image_labels: vec![],
            created_at: None,
            layout_cache: None,
            line_cache: None,
        }
    }

    #[test]
    fn paint_selection_reverses_selected_cells() {
        let mut a = app();
        // Selection across the first five columns of row 0.
        a.mouse_sel = Some(crate::app::MouseSelection {
            anchor_x: 0,
            anchor_y: 0,
            focus_x: 4,
            focus_y: 0,
            dragging: true,
        });
        let (buf, _) = paint(20, 3, |f| {
            // Paint some text first, then apply the selection.
            f.render_widget(
                ratatui::widgets::Paragraph::new("hello world"),
                Rect::new(0, 0, 20, 3),
            );
            paint_selection(f, &a);
        });
        for x in 0..=4u16 {
            assert!(
                buf.cell((x, 0))
                    .unwrap()
                    .style()
                    .add_modifier
                    .contains(Modifier::REVERSED),
                "col {x} must be reversed"
            );
        }
        // Outside the selection → untouched.
        assert!(
            !buf.cell((6, 0))
                .unwrap()
                .style()
                .add_modifier
                .contains(Modifier::REVERSED),
            "col 6 outside selection"
        );
    }

    #[test]
    fn paint_selection_multi_row_and_clip() {
        let mut a = app();
        a.mouse_sel = Some(crate::app::MouseSelection {
            anchor_x: 0,
            anchor_y: 0,
            focus_x: 3,
            focus_y: 1,
            dragging: true,
        });
        // Modal clip covers only row 0..0 (row 1 excluded from selection).
        a.dialog_modal_hit = Some(Rect::new(0, 0, 20, 1));
        let (buf, _) = paint(20, 3, |f| {
            f.render_widget(
                ratatui::widgets::Paragraph::new("abcd\nefgh"),
                Rect::new(0, 0, 20, 3),
            );
            paint_selection(f, &a);
        });
        assert!(
            buf.cell((1, 0))
                .unwrap()
                .style()
                .add_modifier
                .contains(Modifier::REVERSED),
            "row 0 inside clip"
        );
        assert!(
            !buf.cell((1, 1))
                .unwrap()
                .style()
                .add_modifier
                .contains(Modifier::REVERSED),
            "row 1 clipped out"
        );
    }

    #[test]
    fn paint_selection_uses_screen_cells_when_available() {
        let mut a = app();
        a.mouse_sel = Some(crate::app::MouseSelection {
            anchor_x: 0,
            anchor_y: 0,
            focus_x: 2,
            focus_y: 0,
            dragging: true,
        });
        // Non-empty snapshot → clipboard paint_ranges_clipped path.
        a.screen_cells = vec![vec!["a".into(), "b".into(), "c".into(), "d".into()]];
        let (buf, _) = paint(20, 3, |f| {
            f.render_widget(
                ratatui::widgets::Paragraph::new("hello world"),
                Rect::new(0, 0, 20, 3),
            );
            paint_selection(f, &a);
        });
        assert!(
            buf.cell((0, 0))
                .unwrap()
                .style()
                .add_modifier
                .contains(Modifier::REVERSED),
            "screen-cells path paints selection"
        );
    }

    #[test]
    fn paint_selection_none_is_noop() {
        let mut a = app();
        a.mouse_sel = None;
        let (buf, _) = paint(20, 3, |f| {
            f.render_widget(
                ratatui::widgets::Paragraph::new("hello"),
                Rect::new(0, 0, 20, 3),
            );
            paint_selection(f, &a);
        });
        assert!(
            !buf.cell((1, 0))
                .unwrap()
                .style()
                .add_modifier
                .contains(Modifier::REVERSED),
            "no selection → nothing reversed"
        );
    }

    fn row_text(buf: &ratatui::buffer::Buffer, y: u16) -> String {
        let area = buf.area();
        let mut out = String::new();
        for x in area.x..area.x.saturating_add(area.width) {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out
    }

    fn session_with_overflow() -> TuiApp {
        let mut a = app();
        a.provider_name = "anthropic".into();
        a.model_name = "claude".into();
        a.project_label = "whycode".into();
        // Unique markers so we can see which row they land on.
        a.add_message(
            crate::app::ChatRole::User,
            "SAFEAREA_TOP_MARKER unique user prompt",
        );
        a.add_message(
            crate::app::ChatRole::Assistant,
            "SAFEAREA_ASSIST unique assistant reply that wraps a bit more text",
        );
        for i in 0..12 {
            a.add_message(crate::app::ChatRole::User, format!("later user turn {i}"));
            a.add_message(
                crate::app::ChatRole::Assistant,
                format!("later assistant turn {i} filler words for height"),
            );
        }
        a
    }

    fn paint_full_shell(app: &mut TuiApp, w: u16, h: u16) -> (ratatui::buffer::Buffer, String) {
        paint(w, h, |f| {
            crate::ui::render(f, app);
        })
    }

    #[test]
    fn session_chat_stays_below_header_and_safe_area() {
        let mut a = session_with_overflow();
        let (buf, _text) = paint_full_shell(&mut a, 100, 24);

        let safe_top = row_text(&buf, 0);
        let header = row_text(&buf, layout::SAFE_TOP);

        // Terminal edge (SAFE_TOP) is empty of chrome and of chat.
        assert!(
            !safe_top.contains("why"),
            "safe-area row must not hold the header: {safe_top:?}"
        );
        assert!(
            !safe_top.contains("SAFEAREA_TOP_MARKER"),
            "safe-area row must not hold chat: {safe_top:?}"
        );

        // Status header sits on the first inset row and is not overwritten.
        assert!(header.contains("why"), "header brand missing: {header:?}");
        assert!(header.contains("code"), "header brand missing: {header:?}");
        assert!(
            !header.contains("SAFEAREA_TOP_MARKER"),
            "chat spilled into the header: {header:?}"
        );
        assert!(
            !header.contains('\u{276F}'),
            "user-prompt arrow spilled into the header: {header:?}"
        );

        // TOP_PAD blank rows between header and transcript.
        for dy in 1..=layout::TOP_PAD {
            let gap = row_text(&buf, layout::SAFE_TOP + dy);
            assert!(
                !gap.contains("SAFEAREA_TOP_MARKER"),
                "chat spilled into header gap row {dy}: {gap:?}"
            );
            assert!(
                !gap.contains('\u{276F}'),
                "user-prompt arrow spilled into header gap row {dy}: {gap:?}"
            );
        }

        let chat = a.chat_area.expect("session publishes a chat hit rect");
        assert!(
            chat.y >= layout::SAFE_TOP + 1 + layout::TOP_PAD,
            "chat.y={chat:?} must sit below header + TOP_PAD"
        );
        assert!(
            chat.y > layout::SAFE_TOP,
            "chat must not overlap the header row"
        );
    }

    #[test]
    fn session_scrolled_to_top_still_clears_header() {
        let mut a = session_with_overflow();
        // First paint publishes viewport metrics; then pin to oldest lines.
        let (_buf, _) = paint_full_shell(&mut a, 100, 24);
        a.scroll_offset = usize::MAX;
        a.clamp_chat_scroll();
        let (buf, _) = paint_full_shell(&mut a, 100, 24);

        let header = row_text(&buf, layout::SAFE_TOP);
        assert!(
            header.contains("why"),
            "header lost after scroll: {header:?}"
        );
        assert!(
            !header.contains("SAFEAREA_TOP_MARKER"),
            "scrolled chat spilled into the header: {header:?}"
        );
        assert!(
            !header.contains('\u{276F}'),
            "scrolled user band spilled into the header: {header:?}"
        );

        for dy in 1..=layout::TOP_PAD {
            let gap = row_text(&buf, layout::SAFE_TOP + dy);
            assert!(
                !gap.contains("SAFEAREA_TOP_MARKER"),
                "scrolled chat spilled into header gap row {dy}: {gap:?}"
            );
        }

        // Oldest user prompt is visible somewhere below the gap, not above it.
        let mut found = false;
        for y in (layout::SAFE_TOP + 1 + layout::TOP_PAD)..buf.area().height {
            if row_text(&buf, y).contains("SAFEAREA_TOP_MARKER") {
                found = true;
                break;
            }
        }
        assert!(found, "expected oldest user prompt in the chat body");
    }

    #[test]
    fn first_user_bubble_shows_clock_on_the_right() {
        let mut a = app();
        a.add_message(crate::app::ChatRole::User, "FIRST_BUBBLE_MARKER hello");
        let (buf, _) = paint_full_shell(&mut a, 100, 24);
        let mut found = None;
        for y in 0..buf.area().height {
            let row = row_text(&buf, y);
            if row.contains("FIRST_BUBBLE_MARKER") {
                found = Some(row);
                break;
            }
        }
        let row = found.expect("first user bubble should be painted");
        assert!(
            row.contains('\u{276F}'),
            "first bubble must be the ❯ prompt row: {row:?}"
        );
        let marker_at = row.find("FIRST_BUBBLE_MARKER").expect("marker");
        let clock_at = row.rfind(':').expect("HH:MM clock on the first bubble");
        assert!(
            clock_at > marker_at,
            "clock must sit to the right of the first bubble, got {row:?}"
        );
    }

    #[test]
    fn assistant_reply_shows_clock_on_the_right() {
        let mut a = app();
        a.add_message(crate::app::ChatRole::User, "ask");
        a.add_message(
            crate::app::ChatRole::Assistant,
            "AGENT_REPLY_MARKER the answer",
        );
        a.messages.last_mut().unwrap().duration_ms = Some(2100);
        let (buf, _) = paint_full_shell(&mut a, 100, 24);
        let mut found = None;
        for y in 0..buf.area().height {
            let row = row_text(&buf, y);
            if row.contains("AGENT_REPLY_MARKER") {
                found = Some(row);
                break;
            }
        }
        let row = found.expect("agent reply should be painted");
        let marker_at = row.find("AGENT_REPLY_MARKER").expect("marker");
        let clock_at = row.rfind(':').expect("HH:MM clock on the agent reply");
        assert!(
            clock_at > marker_at,
            "clock must sit to the right of the reply, got {row:?}"
        );
    }

    #[test]
    fn fenced_rust_keeps_token_colours_on_the_shell() {
        let mut a = app();
        a.add_message(crate::app::ChatRole::User, "show code");
        a.add_message(
            crate::app::ChatRole::Assistant,
            "```rust\nfn main() { let x = \"hi\"; }\n```",
        );
        let (buf, _) = paint_full_shell(&mut a, 100, 24);
        let mut fgs = std::collections::BTreeSet::new();
        for y in 0..buf.area().height {
            let row = row_text(&buf, y);
            if !row.contains("fn") && !row.contains("let") && !row.contains("hi") {
                continue;
            }
            for x in 0..buf.area().width {
                if let Some(cell) = buf.cell((x, y)) {
                    let sym = cell.symbol();
                    if matches!(sym, "f" | "n" | "l" | "e" | "t" | "h" | "i" | "\"")
                        && let Some(Color::Rgb(r, g, b)) = cell.style().fg
                    {
                        fgs.insert((r, g, b));
                    }
                }
            }
        }
        assert!(
            fgs.len() >= 2,
            "painted rust fence must keep token colours, got {fgs:?}"
        );
    }

    #[test]
    fn session_leaves_gap_between_chat_and_stop() {
        let mut a = session_with_overflow();
        a.current_agent_state = AgentState::Generating;
        let (buf, _) = paint_full_shell(&mut a, 100, 24);

        let chat = a.chat_area.expect("session publishes a chat hit rect");
        let stop = a.turn_stop_hit.rect.expect("busy turn must publish [stop]");
        assert!(
            stop.y
                >= chat
                    .y
                    .saturating_add(chat.height)
                    .saturating_add(layout::CHAT_GAP),
            "chat.bottom={} stop.y={} CHAT_GAP={}",
            chat.y.saturating_add(chat.height),
            stop.y,
            layout::CHAT_GAP
        );
        // The reserved gap row is empty of transcript glyphs.
        let gap_y = chat.y.saturating_add(chat.height);
        let gap = row_text(&buf, gap_y);
        assert!(
            !gap.contains('\u{276F}') && !gap.contains("[stop]"),
            "safezone row must be empty of chat/stop: {gap:?}"
        );
    }

    #[test]
    fn session_chat_has_side_margin() {
        let mut a = session_with_overflow();
        let (_buf, _) = paint_full_shell(&mut a, 100, 24);
        let chat = a.chat_area.expect("session publishes a chat hit rect");
        let expected_x = layout::SAFE_LEFT + layout::SIDE_PAD;
        assert_eq!(
            chat.x, expected_x,
            "bubble column must sit SIDE_PAD inside the safe area"
        );
    }

    #[test]
    fn overflow_session_pins_a_user_prompt_at_the_chat_top() {
        let mut a = session_with_overflow();
        let (buf, _) = paint_full_shell(&mut a, 100, 24);
        let chat = a.chat_area.expect("session publishes a chat hit rect");
        let mut top = String::new();
        for dy in 0..3u16 {
            top.push_str(&row_text(&buf, chat.y + dy));
        }
        assert!(
            top.contains('\u{276F}'),
            "sticky header must be a user ❯ band: {top:?}"
        );
        // Absolute clock on the pinned prompt (same as a live user bubble).
        assert!(
            top.contains(':'),
            "sticky header must keep the clock: {top:?}"
        );
    }
}
