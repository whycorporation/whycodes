// ── ui/render.rs: OpenCode home + session shells ───────────────────────
//
// home.tsx:
//   header (status · shortcuts)
//   [grow] logo [gap] prompt(maxW)
//   footer ( branch · cwd, click-to-copy)
//
// session/index.tsx:
//   header (status · shortcuts)
//   row [ main(pad 2) | sidebar? ]
//   main: scroll messages | prompt
//   footer ( branch · cwd)
//
// Grok Build additions: turn-status strip above the prompt while busy,
// viewport metrics for row-based scrollback selection.

use crate::app::{AgentState, TuiApp};
use crate::opencode_tokens::layout as oc;
use crate::theme::ThemePalette;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::chat;
use super::dialogs;
use super::prompt;
use super::sidebar;
use super::slash_suggest;
use super::status;
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
        paint_selection(frame, app);
        return;
    }

    render_shell(frame, app, &palette);
    // Last, so toasts sit above the chat. Not drawn over a dialog: a modal has
    // the user's attention already, and covering its corner would obscure it.
    toast::render(
        frame,
        oc::inset_safe(frame.area()),
        app.toasts.visible(),
        &palette,
    );
    paint_selection(frame, app);
}

/// Reverse-video a **linear** selection (Grok / native terminal shape).
///
/// Only content cells are painted — trailing pad on short lines is left alone
/// so the highlight matches what `clipboard::text_from_cells` will copy.
fn paint_selection(frame: &mut Frame, app: &TuiApp) {
    let Some(sel) = app.mouse_sel else {
        return;
    };

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
            if let Some((xs, xe)) =
                crate::clipboard::linear_cols(y, top_y, bot_y, top_x, bot_x, row_max)
            {
                r.push((y, xs, xe));
            }
        }
        r
    } else {
        crate::clipboard::paint_ranges(
            &app.screen_cells,
            sel.anchor_x,
            sel.anchor_y,
            sel.focus_x,
            sel.focus_y,
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
    let area = oc::inset_safe(frame.area());

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
    let body = outer[1];
    status::render_footer(frame, outer[2], app, palette);

    if app.messages.is_empty() {
        render_home(frame, body, app, palette);
    } else {
        render_session(frame, body, app, palette);
    }
}

/// home.tsx vertical stack — logo area grows, prompt fixed, no header chrome
fn render_home(frame: &mut Frame, area: Rect, app: &mut TuiApp, palette: &ThemePalette) {
    let content = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: area.height.saturating_sub(oc::BOTTOM_PAD),
    };
    let turn_h = turn_status_height(app);
    let prompt_h = prompt::prompt_height(app, content.width).min(content.height / 2);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),
            Constraint::Length(turn_h),
            Constraint::Length(prompt_h),
        ])
        .split(content);

    app.chat_viewport_rows = chunks[0].height;
    app.chat_content_width = chunks[0].width;

    chat::render(frame, chunks[0], app, palette);
    if turn_h > 0 {
        render_turn_status(frame, chunks[1], app, palette);
    }
    prompt::render(frame, chunks[2], app, palette);
    slash_suggest::render(frame, chunks[2], app, palette);
}

/// session: optional sidebar + padded main column
fn render_session(frame: &mut Frame, area: Rect, app: &mut TuiApp, palette: &ThemePalette) {
    let main = if app.sidebar.visible && area.width >= 36 {
        let w = oc::SIDEBAR_WIDTH.min(area.width / 3).max(24);
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(20), Constraint::Length(w)])
            .split(area);
        // OpenCode: main left, sidebar right
        sidebar::render(frame, chunks[1], app, palette);
        chunks[0]
    } else {
        area
    };

    let inset = Rect {
        x: main.x.saturating_add(oc::SIDE_PAD),
        y: main.y,
        width: main.width.saturating_sub(oc::SIDE_PAD * 2),
        height: main.height.saturating_sub(oc::BOTTOM_PAD),
    };

    let turn_h = turn_status_height(app);
    let prompt_h = prompt::prompt_height(app, inset.width).min(inset.height / 2);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),           // scroll messages
            Constraint::Length(turn_h),   // Grok turn status (busy only)
            Constraint::Length(prompt_h), // boxed prompt (╭ text ╰ meta) + hint
        ])
        .split(inset);

    app.chat_viewport_rows = chunks[0].height;
    app.chat_content_width = chunks[0].width;

    chat::render(frame, chunks[0], app, palette);
    if turn_h > 0 {
        render_turn_status(frame, chunks[1], app, palette);
    }
    prompt::render(frame, chunks[2], app, palette);
    slash_suggest::render(frame, chunks[2], app, palette);
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
        AgentState::WaitingForPermission => palette.warning,
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
}
