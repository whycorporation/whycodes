// ── ui/prompt.rs: boxed prompt ─────────────────────────────────────────
// Rounded box ╭─╮│╰─╯, ❯ prefix, agent/model/effort/mode on the bottom border.
// No panel fill — sits on the canvas background.
//
// Layout:
//   (blank gap above the box)
//   ╭─────────────────────────╮   top border
//   │ ❯ text…                 │   1..MAX_INPUT_ROWS
//   ╰──── agent · model · Med · auto ─╯   bottom border / info
//   hint (home only)
//
// The input block grows upward as text wraps, capped at MAX_INPUT_ROWS.

use crate::app::{AgentState, AppMode, TuiApp};
use crate::theme::ThemePalette;
use crate::tokens::layout;
use crate::widgets::wrap::wrap_text;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use whycodes_core::types::ApprovalMode;

/// The input block grows from a single line up to this many visual rows.
pub const MAX_INPUT_ROWS: u16 = 8;
/// Gap reserved for the hint row on the home screen (kept even when hidden,
/// so the layout doesn't jump once the first message arrives).
const HINT_GAP: u16 = 1;

/// Blank rows between chat/turn-strip and the top of the box.
const OUTER_TOP_GAP: u16 = 2;
/// Top border row (`╭─╮`).
const VPAD_TOP: u16 = 1;
/// Bottom border / info row (`╰─ model ─╯`).
const INFO_BLOCK: u16 = 1;
/// Extra inner row when one or more images are staged on the prompt.
const ATTACH_ROW: u16 = 1;

/// Inner padding between side borders and content (Grok: left 2, right 1).
const PAD_LEFT: u16 = 2;
const PAD_RIGHT: u16 = 1;
/// `"❯ "` / `": "` / `"> "` — always 2 columns.
const PREFIX_WIDTH: u16 = 2;

/// Columns reserved outside the wrapable text:
/// left pad + prefix + right pad  (side borders sit in the pad/edge cells).
const CHROME_H: u16 = PAD_LEFT + PREFIX_WIDTH + PAD_RIGHT;

/// Rows the input text needs right now (1 for an empty prompt).
pub fn input_row_count(app: &TuiApp, area_width: u16) -> u16 {
    let width = content_width(app, area_width);
    let buf: &str = match app.mode {
        AppMode::Command => &app.command.buffer,
        _ => &app.input_buffer,
    };
    if buf.is_empty() {
        return 1;
    }
    wrap_text(buf, width)
        .len()
        .clamp(1, MAX_INPUT_ROWS as usize) as u16
}

/// Rows used by staged image chips (0 or 1).
pub fn attach_row_count(app: &TuiApp) -> u16 {
    if app.pending_images.is_empty() {
        0
    } else {
        ATTACH_ROW
    }
}

/// Total height of the prompt block (gap + top + text + bottom), plus home hint.
pub fn prompt_height(app: &TuiApp, area_width: u16) -> u16 {
    OUTER_TOP_GAP
        + input_row_count(app, area_width)
        + attach_row_count(app)
        + VPAD_TOP
        + INFO_BLOCK
        + HINT_GAP
}

/// Native terminal caret belongs in the prompt (insert-style blinking bar).
/// Hidden while a modal owns keys or scrollback/todos is focused — otherwise the
/// emulator caret sits on the draft while typing goes elsewhere.
fn prompt_owns_caret(app: &TuiApp) -> bool {
    !app.modal_is_open()
        && app.open_subagent.is_none()
        && (app.focus == crate::app::FocusPane::Prompt || matches!(app.mode, AppMode::Command))
}

/// Inner width available for input text inside a prompt area.
fn content_width(app: &TuiApp, area_width: u16) -> u16 {
    let area_width = if app.messages.is_empty() {
        center_prompt_area(Rect {
            x: 0,
            y: 0,
            width: area_width,
            height: 1,
        })
        .width
    } else {
        area_width
    };
    area_width.saturating_sub(CHROME_H).max(8)
}

pub fn render(frame: &mut Frame, area: Rect, app: &mut TuiApp, palette: &ThemePalette) {
    // Center prompt on home (empty messages) with max width cap.
    // Own the leftover columns first: a long paste echo lands at x=0 while
    // the box sits at ~15% — those side gutters stay spaces in both ratatui
    // frames, so skip-diff never overwrites the ghost.
    let area = if app.messages.is_empty() {
        let boxed = center_prompt_area(area);
        if boxed.x > area.x {
            crate::ui::layout::fill_blank(
                frame,
                Rect {
                    x: area.x,
                    y: area.y,
                    width: boxed.x.saturating_sub(area.x),
                    height: area.height,
                },
                palette.bg,
            );
        }
        let right = boxed.x.saturating_add(boxed.width);
        let end = area.x.saturating_add(area.width);
        if right < end {
            crate::ui::layout::fill_blank(
                frame,
                Rect {
                    x: right,
                    y: area.y,
                    width: end.saturating_sub(right),
                    height: area.height,
                },
                palette.bg,
            );
        }
        boxed
    } else {
        area
    };

    if area.height == 0 || area.width < 6 {
        app.agent_hit.set_rect(None);
        app.model_hit.set_rect(None);
        return;
    }

    let busy = !matches!(
        app.current_agent_state,
        AgentState::Idle | AgentState::Error(_)
    );
    let prompt_focused =
        app.focus == crate::app::FocusPane::Prompt || matches!(app.mode, AppMode::Command);

    // Box chrome matches the agent identity (same color as the footer
    // name / header chip). Prefix stays bright when focused, dim idle.
    // No panel fill — canvas bg shows through.
    let agent_color = app
        .config
        .agent_color(&app.agent_name, app.agent_cycle_idx, palette);
    let accent = if prompt_focused {
        agent_color
    } else {
        palette.dim
    };

    let (buf, cursor) = match app.mode {
        AppMode::Command => (&app.command.buffer, app.command.buffer.len()),
        _ => (&app.input_buffer, app.input_cursor),
    };

    // Wrap to the inner text columns (after pad + ❯). Never wider than the
    // paragraph rect so hard-wrapped paste lines cannot paint into/over the
    // right border or past the box.
    let text_w = area.width.saturating_sub(CHROME_H).max(1);
    let rows = if buf.is_empty() {
        Vec::new()
    } else {
        wrap_text(buf, text_w)
    };
    // Defensive: clamp each painted row to text_w display columns.
    let rows = clamp_rows_to_width(buf, rows, text_w);
    let input_rows = rows.len().max(1).min(MAX_INPUT_ROWS as usize) as u16;
    let attach_rows = attach_row_count(app);

    // Cursor row within the full wrap (before viewport clip). Used both for
    // scrolling the visible window and placing the terminal caret.
    let (cursor_row, cursor_col) = cursor_row_col(&rows, buf, cursor);

    // When text wraps past MAX_INPUT_ROWS, keep the caret on-screen by
    // scrolling the viewport (previously only the first N rows were shown,
    // so pasting long text hid the cursor and felt like a flicker/jump).
    let view_start = if rows.len() <= input_rows as usize {
        0usize
    } else {
        let max_start = rows.len() - input_rows as usize;
        cursor_row
            .saturating_sub((input_rows as usize).saturating_sub(1))
            .min(max_start)
    };
    let view_end = (view_start + input_rows as usize).min(rows.len());

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(OUTER_TOP_GAP), // breathing room above the box
            Constraint::Length(VPAD_TOP),      // ╭─╮
            Constraint::Length(attach_rows),   // image chips (0–1)
            Constraint::Length(input_rows),    // text
            Constraint::Length(INFO_BLOCK),    // ╰─ meta ─╯
            Constraint::Min(0),                // home hint
        ])
        .split(area);

    // Breathing-room rows above ╭─╮ are otherwise never written. A large
    // paste echo (or a previously taller box) sits here beside the input.
    crate::ui::layout::fill_blank(frame, chunks[0], palette.bg);

    let border_style = Style::default().fg(agent_color);

    // ── Top: ╭──────────╮ ───────────────────────────────────────────
    let top_border = chunks[1];
    if top_border.height > 0 {
        paint_h_border(frame, top_border, border_style, true);
    }

    // ── Attachment chips: │  🖼 shot.png · 2 images  │ ─────────────
    let attach_area = chunks[2];
    let content_x = area.x.saturating_add(PAD_LEFT);
    if attach_rows > 0 && attach_area.height > 0 {
        paint_attach_row(frame, attach_area, area, app, palette, border_style);
    }

    // ── Text rows: │  ❯ body…  │ ────────────────────────────────────
    // Prefix is always 2 columns. Placeholder only when empty + unfocused
    // (Grok); focused empty keeps a bare caret after ❯.
    let prefix: &str = match app.mode {
        AppMode::Command => ": ",
        _ if busy && app.input_buffer.is_empty() && prompt_focused => "… ",
        _ => "❯ ",
    };
    let empty_hint: Option<&str> =
        if !buf.is_empty() || prompt_focused || !app.pending_images.is_empty() {
            None
        } else {
            match app.mode {
                AppMode::Command => Some("command…"),
                _ if app.messages.is_empty() => Some("Ask anything…  (drop images)"),
                // Scrollback owns focus — nudge how to get back.
                _ => Some("Tab/i/Space → prompt · j/k select"),
            }
        };

    let text_area = chunks[3];
    let mut lines: Vec<Line> = Vec::with_capacity(input_rows as usize);

    let prefix_style = Style::default()
        .fg(if busy && prompt_focused {
            palette.dim
        } else {
            accent
        })
        .add_modifier(Modifier::BOLD);

    match empty_hint {
        Some(text) => {
            lines.push(Line::from(vec![
                Span::styled(prefix.to_string(), prefix_style),
                Span::styled(text.to_string(), Style::default().fg(palette.dim)),
            ]));
        }
        None if rows.is_empty() => {
            lines.push(Line::from(Span::styled(prefix.to_string(), prefix_style)));
        }
        None => {
            // Slash commands (`/help`, `/models …`) render bold + accent so the
            // command token stands out from ordinary prompt text.
            let cmd_end = slash_command_byte_end(buf);
            let cmd_style = Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD);
            let text_style = Style::default().fg(palette.input_fg);
            // Collapsed paste tokens (`[pasted #1 ~ 42 lines]`) use dim reverse
            // so they read as chips, not as typed prose.
            let paste_style = Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::DIM | Modifier::BOLD);
            let paste_ranges = crate::paste::style_ranges(buf);

            for (vis_i, row) in rows[view_start..view_end].iter().enumerate() {
                let abs_i = view_start + vis_i;
                let mut spans = Vec::new();
                // ❯ only on the first logical row of the whole buffer (not the
                // first visible row after scroll).
                if abs_i == 0 {
                    spans.push(Span::styled(prefix.to_string(), prefix_style));
                } else {
                    // Continuation rows align under the text, not under ❯.
                    spans.push(Span::raw(" ".repeat(PREFIX_WIDTH as usize)));
                }
                spans.extend(styled_input_row(
                    buf,
                    row.byte_range.0,
                    row.byte_range.1,
                    cmd_end,
                    cmd_style,
                    text_style,
                    paste_style,
                    &paste_ranges,
                ));
                lines.push(Line::from(spans));
            }
        }
    }
    while lines.len() < input_rows as usize {
        lines.push(Line::from(Span::raw(" ".repeat(PREFIX_WIDTH as usize))));
    }

    // Text content inset by PAD_LEFT (left border painted over edge after).
    let text_rect = Rect {
        x: content_x,
        y: text_area.y,
        width: area.width.saturating_sub(PAD_LEFT + PAD_RIGHT).max(1),
        height: text_area.height,
    };
    // No Paragraph wrap — rows are pre-wrapped to `text_w` and clamped. Letting
    // ratatui re-wrap would add vertical lines that spill past the box height.
    frame.render_widget(Paragraph::new(Text::from(lines)), text_rect);

    // Native caret (blinking bar, set once at TUI start). Overlay / scrollback /
    // todos hide it so it does not sit on the prompt while keys go elsewhere.
    if prompt_owns_caret(app) {
        let vis_row = cursor_row.saturating_sub(view_start);
        if vis_row < input_rows as usize {
            let x = content_x
                .saturating_add(PREFIX_WIDTH)
                .saturating_add(cursor_col as u16)
                .min(text_rect.x + text_rect.width.saturating_sub(1));
            frame.set_cursor_position(Position::new(x, text_area.y + vis_row as u16));
        }
    }

    // Side borders │ on each text row (overwrite left/right edges).
    paint_side_borders(frame, text_area, area, border_style);
    if attach_rows > 0 {
        paint_side_borders(frame, attach_area, area, border_style);
    }

    // ── Bottom: ╰──── agent · provider/model ──╯ ────────────────────
    let bottom = chunks[4];
    if bottom.height > 0 {
        paint_h_border(frame, bottom, border_style, false);
        paint_bottom_meta(frame, bottom, app, palette);
    } else {
        app.agent_hit.set_rect(None);
        app.model_hit.set_rect(None);
        app.effort_hit.set_rect(None);
        app.approval_hit.set_rect(None);
    }

    // Home rotating hint under the box.
    if app.messages.is_empty()
        && app.input_buffer.is_empty()
        && app.pending_images.is_empty()
        && !busy
        && chunks[5].height > 0
    {
        let hint = Line::from(vec![
            Span::raw(" "),
            Span::styled(
                format!("  {}", pick_hint()),
                Style::default().fg(palette.dim),
            ),
        ]);
        frame.render_widget(Paragraph::new(Text::from(hint)), chunks[5]);
    }
}

/// One row of staged image labels inside the prompt box.
fn paint_attach_row(
    frame: &mut Frame,
    row: Rect,
    full: Rect,
    app: &TuiApp,
    palette: &ThemePalette,
    border_style: Style,
) {
    let _ = border_style;
    let _ = full;
    let labels: Vec<String> = app
        .pending_images
        .iter()
        .map(|i| format!("🖼 {}", i.label))
        .collect();
    let n = labels.len();
    let joined = labels.join("  ·  ");
    let max_w = row.width.saturating_sub(PAD_LEFT + PAD_RIGHT).max(4) as usize;
    let mut line = if UnicodeWidthStr::width(joined.as_str()) > max_w {
        // Prefer a compact summary when many/long names.
        let summary = if n == 1 {
            labels[0].clone()
        } else {
            format!("🖼 {n} images · Backspace removes last")
        };
        truncate_to_width(&summary, max_w)
    } else {
        joined
    };
    // Hint on the right when there is room.
    let hint = " ⌫ ";
    let hint_w = UnicodeWidthStr::width(hint);
    let line_w = UnicodeWidthStr::width(line.as_str());
    if line_w + hint_w + 1 < max_w {
        let pad = max_w.saturating_sub(line_w + hint_w);
        line = format!("{line}{}{hint}", " ".repeat(pad));
    }

    let content_x = row.x.saturating_add(PAD_LEFT);
    let text_rect = Rect {
        x: content_x,
        y: row.y,
        width: row.width.saturating_sub(PAD_LEFT + PAD_RIGHT).max(1),
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            line,
            Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::DIM),
        ))),
        text_rect,
    );
}

/// Horizontal border row: corners + fill `─`.
fn paint_h_border(frame: &mut Frame, row: Rect, style: Style, top: bool) {
    if row.width == 0 || row.height == 0 {
        return;
    }
    let left = if top { '╭' } else { '╰' };
    let right = if top { '╮' } else { '╯' };
    let line = if row.width == 1 {
        left.to_string()
    } else {
        let mid = "─".repeat(row.width.saturating_sub(2) as usize);
        format!("{left}{mid}{right}")
    };
    frame.render_widget(Paragraph::new(Line::from(Span::styled(line, style))), row);
}

fn paint_side_borders(frame: &mut Frame, text_area: Rect, full: Rect, style: Style) {
    if full.width < 2 || text_area.height == 0 {
        return;
    }
    let buf = frame.buffer_mut();
    let left_x = full.x;
    let right_x = full.x + full.width.saturating_sub(1);
    for y in text_area.y..text_area.y.saturating_add(text_area.height) {
        if let Some(cell) = buf.cell_mut((left_x, y)) {
            cell.set_char('│');
            cell.set_style(style);
        }
        if let Some(cell) = buf.cell_mut((right_x, y)) {
            cell.set_char('│');
            cell.set_style(style);
        }
    }
}

/// Right-aligned ` agent · provider/model · effort · mode ` on the bottom border, with
/// leading/trailing spaces blanking adjacent `─` (Grok chrome caption).
///
/// Agent and model pick up theme / `[tui.agent_colors]` so the caption is
/// not a dim grey smear — same identity color as the header chip.
/// Hover underlines each name; click opens the matching picker.
fn paint_bottom_meta(frame: &mut Frame, row: Rect, app: &mut TuiApp, palette: &ThemePalette) {
    app.agent_hit.set_rect(None);
    app.model_hit.set_rect(None);
    app.effort_hit.set_rect(None);
    app.approval_hit.set_rect(None);
    if row.width < 8 {
        return;
    }
    let provider = if app.provider_name.is_empty() {
        "—"
    } else {
        app.provider_name.as_str()
    };
    let model = if app.model_name.is_empty() {
        "—"
    } else {
        app.model_name.as_str()
    };
    let agent_color = app
        .config
        .agent_color(&app.agent_name, app.agent_cycle_idx, palette);
    let model_color = app.config.model_color(palette);
    let sep_style = Style::default().fg(palette.dim);
    let mut agent_style = Style::default()
        .fg(agent_color)
        .add_modifier(Modifier::BOLD);
    if app.agent_hit.hovered {
        agent_style = agent_style.add_modifier(Modifier::UNDERLINED);
    }
    let mut model_style = Style::default().fg(model_color);
    if app.model_hit.hovered {
        model_style = model_style.add_modifier(Modifier::UNDERLINED);
    }
    let effort = whycodes_llm::ThinkingConfig::resolve_effort(
        &app.provider_name,
        &app.model_name,
        app.reasoning_effort.as_deref(),
    );
    let effort_shown = effort.map(|e| e.label().to_string()).unwrap_or_default();
    let mut effort_style = Style::default().fg(palette.dim);
    if app.effort_hit.hovered {
        effort_style = effort_style.add_modifier(Modifier::UNDERLINED);
    }
    let mode_shown = app.approval_mode.label().to_string();
    // `auto` stays muted; `important` / `manual` are more visible so you notice
    // the session will interrupt.
    let mut mode_style = Style::default().fg(match app.approval_mode {
        ApprovalMode::Auto => palette.dim,
        ApprovalMode::Important => palette.warning,
        ApprovalMode::Manual => palette.fg,
    });
    if app.approval_hit.hovered {
        mode_style = mode_style.add_modifier(Modifier::UNDERLINED);
    }
    let badge_style = match app.intent_kind.as_deref() {
        Some("question") => Style::default()
            .fg(palette.info)
            .add_modifier(Modifier::BOLD),
        Some("plan") => Style::default()
            .fg(palette.accent)
            .add_modifier(Modifier::BOLD),
        Some("change") => Style::default()
            .fg(palette.success)
            .add_modifier(Modifier::BOLD),
        _ => sep_style,
    };

    // Corners + 1-cell inset each side stay pure border.
    let max_w = row.width.saturating_sub(4) as usize;
    let model_label = format!("{provider}/{model}");
    let reserved = 2 // leading/trailing spaces
        + UnicodeWidthStr::width(app.agent_name.as_str())
        + 3 // ` · `
        + app
            .intent_badge
            .as_deref()
            .map(|b| UnicodeWidthStr::width(b) + 3)
            .unwrap_or(0);
    let model_max = max_w.saturating_sub(reserved);
    let model_shown = if UnicodeWidthStr::width(model_label.as_str()) > model_max {
        truncate_to_width(&model_label, model_max)
    } else {
        model_label
    };

    let mut spans = vec![
        Span::styled(" ", sep_style),
        Span::styled(app.agent_name.clone(), agent_style),
    ];
    if let Some(ref badge) = app.intent_badge {
        spans.push(Span::styled(" · ", sep_style));
        spans.push(Span::styled(badge.clone(), badge_style));
    }
    let model_shown_w = UnicodeWidthStr::width(model_shown.as_str()) as u16;
    if !model_shown.is_empty() {
        spans.push(Span::styled(" · ", sep_style));
        spans.push(Span::styled(model_shown, model_style));
    }
    let effort_shown_w = UnicodeWidthStr::width(effort_shown.as_str()) as u16;
    let mode_shown_w = UnicodeWidthStr::width(mode_shown.as_str()) as u16;
    let mut show_effort = !effort_shown.is_empty();
    let mut show_mode = !mode_shown.is_empty();
    if show_effort {
        spans.push(Span::styled(" · ", sep_style));
        spans.push(Span::styled(effort_shown.clone(), effort_style));
    }
    if show_mode {
        spans.push(Span::styled(" · ", sep_style));
        spans.push(Span::styled(mode_shown.clone(), mode_style));
    }
    spans.push(Span::styled(" ", sep_style));

    let span_width = |spans: &[Span<'_>]| -> usize {
        spans
            .iter()
            .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
            .sum()
    };
    let mut label_w = span_width(&spans);
    // Narrow terminals drop chips before collapsing to the agent name:
    // approval first, then effort (issue #45).
    if label_w > max_w && show_mode {
        spans.truncate(spans.len() - 3); // trailing space + ` · ` + mode
        spans.push(Span::styled(" ", sep_style));
        show_mode = false;
        label_w = span_width(&spans);
    }
    if label_w > max_w && show_effort {
        spans.truncate(spans.len() - 3); // trailing space + ` · ` + effort
        spans.push(Span::styled(" ", sep_style));
        show_effort = false;
        label_w = span_width(&spans);
    }
    if label_w == 0 || label_w > max_w {
        // Narrow box: fall back to a single truncated, still-colored agent name.
        let trunc = truncate_to_width(&format!(" {} ", app.agent_name), max_w);
        let w = UnicodeWidthStr::width(trunc.as_str()) as u16;
        if w == 0 {
            return;
        }
        let x = row.x + row.width.saturating_sub(2 + w);
        // Hit the inner name, not the padding spaces.
        let name_w = UnicodeWidthStr::width(app.agent_name.as_str()) as u16;
        let hit_w = name_w.min(w.saturating_sub(2));
        if hit_w > 0 {
            app.agent_hit.set_rect(Some(Rect {
                x: x.saturating_add(1),
                y: row.y,
                width: hit_w,
                height: 1,
            }));
        }
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(trunc, agent_style))),
            Rect {
                x,
                y: row.y,
                width: w,
                height: 1,
            },
        );
        return;
    }
    let label_w = label_w as u16;
    // Right-align ending 2 cells before ╯.
    let x = row.x + row.width.saturating_sub(2 + label_w);
    let meta_rect = Rect {
        x,
        y: row.y,
        width: label_w,
        height: 1,
    };
    // Record clickable spans: skip leading space, then agent, optional badge, model, effort.
    let mut col = x.saturating_add(1);
    let agent_w = UnicodeWidthStr::width(app.agent_name.as_str()) as u16;
    if agent_w > 0 {
        app.agent_hit.set_rect(Some(Rect {
            x: col,
            y: row.y,
            width: agent_w,
            height: 1,
        }));
        col = col.saturating_add(agent_w);
    }
    if let Some(ref badge) = app.intent_badge {
        col = col.saturating_add(3); // ` · `
        col = col.saturating_add(UnicodeWidthStr::width(badge.as_str()) as u16);
    }
    if model_shown_w > 0 {
        col = col.saturating_add(3); // ` · `
        app.model_hit.set_rect(Some(Rect {
            x: col,
            y: row.y,
            width: model_shown_w,
            height: 1,
        }));
        col = col.saturating_add(model_shown_w);
    }
    if show_effort {
        col = col.saturating_add(3); // ` · `
        app.effort_hit.set_rect(Some(Rect {
            x: col,
            y: row.y,
            width: effort_shown_w,
            height: 1,
        }));
        col = col.saturating_add(effort_shown_w);
    }
    if show_mode {
        col = col.saturating_add(3); // ` · `
        app.approval_hit.set_rect(Some(Rect {
            x: col,
            y: row.y,
            width: mode_shown_w,
            height: 1,
        }));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), meta_rect);
}

fn truncate_to_width(s: &str, max_w: usize) -> String {
    if max_w == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0).max(1);
        if w + cw > max_w {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out
}

const HINTS: &[&str] = &[
    "/ for commands",
    "drop or paste image paths into the prompt",
    "tab toggles scrollback focus",
    "ctrl+t cycles agent",
    "esc cancels · double-esc clears",
    "j/k select messages in scrollback",
    "y copies the selected message",
    "/init to create AGENTS.md",
];

fn pick_hint() -> &'static str {
    use std::time::{SystemTime, UNIX_EPOCH};
    let idx = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| (d.as_secs() / 8) as usize)
        .unwrap_or(0)
        % HINTS.len();
    HINTS[idx]
}

fn center_prompt_area(area: Rect) -> Rect {
    // Wide terminals: 70% capped at PROMPT_MAX_WIDTH. Narrow / portrait: fill
    // almost the full width (2-col side breathing room) so the box is not a
    // 70%-of-40 island that wastes the PTY.
    let side = if area.width >= 48 { 4 } else { 2 };
    let usable = area.width.saturating_sub(side);
    let ratio_w = (area.width as f32 * layout::PROMPT_WIDTH_RATIO) as u16;
    let preferred = if area.width < 64 {
        usable
    } else {
        layout::PROMPT_MAX_WIDTH.min(ratio_w.max(layout::PROMPT_MIN_WIDTH))
    };
    let max_w = preferred
        .min(usable)
        .max(layout::PROMPT_MIN_WIDTH.min(usable));
    let x = area.x + (area.width.saturating_sub(max_w)) / 2;
    Rect {
        x,
        y: area.y,
        width: max_w,
        height: area.height,
    }
}

/// Byte end of the leading slash-command token (`/name`), or `None` when the
/// buffer is not a slash command. Includes the leading `/`; stops at the first
/// whitespace (arguments stay in the normal input style).
fn slash_command_byte_end(buf: &str) -> Option<usize> {
    if !buf.starts_with('/') {
        return None;
    }
    Some(buf.find(char::is_whitespace).unwrap_or(buf.len()))
}

/// Ensure no wrapped row paints wider than `text_w` (hard safety for paste).
fn clamp_rows_to_width(
    buf: &str,
    rows: Vec<crate::widgets::wrap::WrappedRow>,
    text_w: u16,
) -> Vec<crate::widgets::wrap::WrappedRow> {
    let max_w = text_w.max(1) as usize;
    rows.into_iter()
        .map(|row| {
            let (start, end) = row.byte_range;
            if start >= end || end > buf.len() {
                return row;
            }
            let mut w = 0usize;
            let mut cut = end;
            for (off, ch) in buf[start..end].char_indices() {
                let cw = ch.width().unwrap_or(0).max(1);
                if w + cw > max_w {
                    cut = start + off;
                    break;
                }
                w += cw;
            }
            crate::widgets::wrap::WrappedRow {
                byte_range: (start, cut),
                width: w as u16,
            }
        })
        .collect()
}

/// Row index + display column of `cursor` within wrapped `rows`.
fn cursor_row_col(
    rows: &[crate::widgets::wrap::WrappedRow],
    buf: &str,
    cursor: usize,
) -> (usize, usize) {
    if rows.is_empty() {
        return (0, 0);
    }
    let mut row_idx = rows.len().saturating_sub(1);
    let mut col = 0usize;
    for (i, row) in rows.iter().enumerate() {
        if cursor >= row.byte_range.0 && cursor <= row.byte_range.1 {
            row_idx = i;
            let slice_end = cursor.min(row.byte_range.1);
            if slice_end > row.byte_range.0 {
                col = buf[row.byte_range.0..slice_end]
                    .chars()
                    .map(|c| c.width().unwrap_or(0).max(1))
                    .sum();
            }
            break;
        }
    }
    (row_idx, col)
}

/// Split one wrapped row into styled spans so slash-command tokens and
/// collapsed paste chips stay highlighted even when a wrap cuts mid-token.
#[allow(clippy::too_many_arguments)]
fn styled_input_row(
    buf: &str,
    start: usize,
    end: usize,
    cmd_end: Option<usize>,
    cmd_style: Style,
    text_style: Style,
    paste_style: Style,
    paste_ranges: &[(usize, usize)],
) -> Vec<Span<'static>> {
    if start >= end {
        return Vec::new();
    }
    // Collect cut points: row edges, command end, paste range edges.
    let mut cuts = vec![start, end];
    if let Some(c) = cmd_end
        && c > start
        && c < end
    {
        cuts.push(c);
    }
    for &(ps, pe) in paste_ranges {
        if ps > start && ps < end {
            cuts.push(ps);
        }
        if pe > start && pe < end {
            cuts.push(pe);
        }
    }
    cuts.sort_unstable();
    cuts.dedup();

    let mut spans = Vec::new();
    for w in cuts.windows(2) {
        let a = w[0];
        let b = w[1];
        if a >= b {
            continue;
        }
        let mid = a;
        let in_paste = paste_ranges.iter().any(|&(ps, pe)| mid >= ps && mid < pe);
        let in_cmd = cmd_end.is_some_and(|c| mid < c);
        let style = if in_paste {
            paste_style
        } else if in_cmd {
            cmd_style
        } else {
            text_style
        };
        spans.push(Span::styled(buf[a..b].to_string(), style));
    }
    spans
}

#[cfg(test)]
mod wrap_tests {
    use super::*;

    fn row_texts(buf: &str, width: u16) -> Vec<String> {
        wrap_text(buf, width)
            .iter()
            .map(|r| buf[r.byte_range.0..r.byte_range.1].to_string())
            .collect()
    }

    #[test]
    fn short_text_stays_on_one_row() {
        assert_eq!(row_texts("hello", 20), vec!["hello"]);
    }

    #[test]
    fn wraps_at_word_boundaries() {
        // "bbb ccc" fits on one 7-col row, so the only break is after "aaa".
        assert_eq!(row_texts("aaa bbb ccc", 7), vec!["aaa", "bbb ccc"]);
        // Narrower: each word gets its own row.
        assert_eq!(row_texts("aaa bbb ccc", 4), vec!["aaa", "bbb", "ccc"]);
    }

    #[test]
    fn long_word_is_hard_split() {
        let rows = row_texts("abcdefghij", 4);
        assert_eq!(rows, vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn explicit_newline_breaks_row() {
        let rows = row_texts("ab\ncd", 10);
        assert_eq!(rows, vec!["ab", "cd"]);
    }

    #[test]
    fn multibyte_chars_count_by_display_width() {
        // 'ş' is 1 column; CJK '世' is 2.
        let rows = row_texts("ş世a", 3);
        assert_eq!(rows, vec!["ş世", "a"]);
    }

    #[test]
    fn every_byte_is_covered_exactly_once() {
        let buf = "the quick brown fox jumps over the lazy dog";
        let rows = wrap_text(buf, 10);
        let mut covered = String::new();
        let mut prev_end = 0;
        for r in &rows {
            assert_eq!(r.byte_range.0, prev_end, "rows must be contiguous");
            assert!(r.width as usize <= 10);
            covered.push_str(&buf[r.byte_range.0..r.byte_range.1]);
            prev_end = r.byte_range.1;
            // Skip the single whitespace consumed by the wrap boundary.
            if prev_end < buf.len()
                && buf.as_bytes()[prev_end].is_ascii_whitespace()
                && buf.as_bytes()[prev_end] != b'\n'
            {
                covered.push(buf.as_bytes()[prev_end] as char);
                prev_end += 1;
            }
        }
        assert_eq!(covered, buf);
    }

    #[test]
    fn prompt_height_includes_box_chrome() {
        // gap + top + 1 text + bottom + hint
        const {
            assert!(OUTER_TOP_GAP + VPAD_TOP + 1 + INFO_BLOCK + HINT_GAP >= 5);
        }
    }

    #[test]
    fn center_prompt_uses_almost_full_width_on_phone() {
        let area = Rect::new(0, 0, 40, 6);
        let boxed = center_prompt_area(area);
        assert!(
            boxed.width >= 36,
            "portrait prompt.width={} should not sit at the old 40-col floor",
            boxed.width
        );
        assert!(boxed.x + boxed.width <= area.width);
    }

    #[test]
    fn slash_command_byte_end_covers_token_only() {
        assert_eq!(slash_command_byte_end("/help"), Some(5));
        assert_eq!(slash_command_byte_end("/models foo"), Some(7));
        assert_eq!(slash_command_byte_end("/"), Some(1));
        assert_eq!(slash_command_byte_end("hello"), None);
        assert_eq!(slash_command_byte_end(""), None);
    }

    #[test]
    fn styled_input_row_splits_command_and_args() {
        use ratatui::style::{Color, Modifier};
        let cmd = Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD);
        let text = Style::default().fg(Color::White);
        let paste = Style::default().fg(Color::Yellow);
        let buf = "/help me";
        let spans = styled_input_row(
            buf,
            0,
            buf.len(),
            slash_command_byte_end(buf),
            cmd,
            text,
            paste,
            &[],
        );
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].content.as_ref(), "/help");
        assert_eq!(spans[1].content.as_ref(), " me");
        assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn styled_input_row_highlights_paste_token() {
        use ratatui::style::Color;
        let cmd = Style::default();
        let text = Style::default().fg(Color::White);
        let paste = Style::default().fg(Color::Yellow);
        let token = crate::paste::placeholder(1, 5);
        let buf = format!("see {token} ok");
        let ranges = crate::paste::style_ranges(&buf);
        let spans = styled_input_row(&buf, 0, buf.len(), None, cmd, text, paste, &ranges);
        assert!(
            spans.iter().any(|s| s.content.as_ref() == token),
            "paste token should be its own span: {spans:?}"
        );
        let token_span = spans.iter().find(|s| s.content.as_ref() == token).unwrap();
        assert_eq!(token_span.style.fg, Some(Color::Yellow));
    }
}

#[cfg(test)]
mod overflow_render_tests {
    use super::*;
    use crate::app::{AppMode, TuiApp};
    use crate::config::TuiAppConfig;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn rendered_rows(app: &mut TuiApp, width: u16, height: u16) -> Vec<String> {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let palette = app.config.palette();
        terminal
            .draw(|frame| render(frame, frame.area(), app, &palette))
            .expect("draw prompt");
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn command_mode_renders_command_buffer_with_colon_prefix() {
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.mode = AppMode::Command;
        app.input_buffer = "ignored prompt".into();
        app.command.buffer = "provider anthropic".into();

        let rows = rendered_rows(&mut app, 60, 8);
        let input = rows
            .iter()
            .find(|row| row.contains("provider anthropic"))
            .expect("command input row");
        assert!(input.contains(": provider anthropic"), "{input:?}");
        assert!(rows.iter().all(|row| !row.contains("ignored prompt")));
    }

    #[test]
    fn staged_images_render_inside_an_extra_box_row() {
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.pending_images.push(crate::images::PromptImage {
            path: "shot.png".into(),
            label: "shot.png".into(),
            media_type: "image/png".into(),
        });

        let rows = rendered_rows(&mut app, 60, 9);
        let attachment = rows
            .iter()
            .find(|row| row.contains("shot.png"))
            .expect("attachment row");
        assert!(attachment.contains('│'), "{attachment:?}");
        assert!(attachment.contains('⌫'), "{attachment:?}");
        assert_eq!(attach_row_count(&app), 1);
    }

    #[test]
    fn long_paste_stays_inside_box_edges() {
        let backend = TestBackend::new(80, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = TuiApp::new(TuiAppConfig::default());
        // Under collapse threshold but long enough to hard-wrap many rows.
        let body = "x".repeat(crate::paste::COLLAPSE_MIN_CHARS - 1);
        app.insert_paste_text(&body);
        assert!(
            app.pending_pastes.is_empty(),
            "expected inline paste under threshold"
        );

        let palette = app.config.palette();
        terminal
            .draw(|f| {
                let area = f.area();
                render(
                    f,
                    Rect {
                        x: 0,
                        y: 10,
                        width: area.width,
                        height: 20,
                    },
                    &mut app,
                    &palette,
                );
            })
            .unwrap();

        let buf = terminal.backend().buffer();
        let mut top_y = None;
        let mut left_x = None;
        let mut right_x = None;
        for y in 0..30u16 {
            for x in 0..80u16 {
                let cell = &buf[(x, y)];
                if cell.symbol() == "╭" {
                    top_y = Some(y);
                    left_x = Some(x);
                }
                if cell.symbol() == "╮" {
                    right_x = Some(x);
                }
            }
        }
        let top_y = top_y.expect("top-left corner ╭");
        let left_x = left_x.unwrap();
        let right_x = right_x.expect("top-right ╮");

        for y in top_y..top_y.saturating_add(12) {
            for x in right_x.saturating_add(1)..80 {
                assert_ne!(buf[(x, y)].symbol(), "x", "leak right ({x},{y})");
            }
            for x in 0..left_x {
                assert_ne!(buf[(x, y)].symbol(), "x", "leak left ({x},{y})");
            }
        }
    }

    #[test]
    fn bottom_meta_uses_agent_and_model_colors() {
        let backend = TestBackend::new(80, 16);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.agent_name = "build".into();
        app.provider_name = "anthropic".into();
        app.model_name = "sonnet".into();
        app.focus = crate::app::FocusPane::Prompt;
        let palette = app.config.palette();

        terminal
            .draw(|f| {
                let area = f.area();
                render(
                    f,
                    Rect {
                        x: 0,
                        y: 8,
                        width: area.width,
                        height: 8,
                    },
                    &mut app,
                    &palette,
                );
            })
            .unwrap();

        let buf = terminal.backend().buffer();
        let mut row = String::new();
        let mut agent_fg = None;
        let mut model_fg = None;
        for y in 0..16u16 {
            let mut line = String::new();
            for x in 0..80u16 {
                line.push_str(buf[(x, y)].symbol());
            }
            if let Some(col) = cell_seq_col(buf, y, 80, "build") {
                agent_fg = Some(buf[(col, y)].fg);
                row = line;
            }
            if let Some(col) = cell_seq_col(buf, y, 80, "sonnet") {
                model_fg = Some(buf[(col, y)].fg);
            }
        }
        assert!(row.contains("build"), "footer should show agent: {row}");
        assert!(row.contains("sonnet"), "footer should show model: {row}");
        assert_eq!(agent_fg, Some(palette.success), "agent uses success: {row}");
        assert_eq!(model_fg, Some(palette.info), "model uses info: {row}");
        assert_ne!(
            agent_fg,
            Some(palette.dim),
            "agent must not be the dim caption color"
        );
        assert!(
            app.agent_hit.rect.is_some(),
            "agent name must be a click target"
        );
        assert!(
            app.model_hit.rect.is_some(),
            "model name must be a click target"
        );
        assert!(
            row.contains("auto"),
            "footer should show approval mode: {row}"
        );
        assert!(
            app.approval_hit.rect.is_some(),
            "approval mode must be a click target"
        );
    }

    fn draw_prompt_footer(
        app: &mut TuiApp,
        width: u16,
        height: u16,
    ) -> (
        ratatui::Terminal<ratatui::backend::TestBackend>,
        crate::theme::ThemePalette,
    ) {
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let palette = app.config.palette();
        terminal
            .draw(|f| {
                let area = f.area();
                render(
                    f,
                    Rect {
                        x: 0,
                        y: height.saturating_sub(8).min(area.height.saturating_sub(1)),
                        width: area.width,
                        height: 8.min(area.height),
                    },
                    app,
                    &palette,
                );
            })
            .unwrap();
        (terminal, palette)
    }

    fn footer_line_containing(
        buf: &ratatui::buffer::Buffer,
        width: u16,
        height: u16,
        needle: &str,
    ) -> String {
        for y in 0..height {
            let mut line = String::new();
            for x in 0..width {
                line.push_str(buf[(x, y)].symbol());
            }
            if line.contains(needle) {
                return line;
            }
        }
        String::new()
    }

    #[test]
    fn bottom_meta_approval_chip_is_dim_in_auto_and_brighter_otherwise() {
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.agent_name = "build".into();
        app.provider_name = "anthropic".into();
        app.model_name = "sonnet".into();
        app.approval_mode = ApprovalMode::Auto;
        let (terminal, palette) = draw_prompt_footer(&mut app, 80, 16);
        let buf = terminal.backend().buffer();
        let auto_col = (0..16u16).find_map(|y| cell_seq_col(buf, y, 80, "auto"));
        let auto_fg = auto_col.map(|col| {
            let y = (0..16u16)
                .find(|y| cell_seq_col(buf, *y, 80, "auto").is_some())
                .unwrap();
            buf[(col, y)].fg
        });
        assert_eq!(auto_fg, Some(palette.dim), "auto stays muted");

        app.approval_mode = ApprovalMode::Important;
        let (terminal, palette) = draw_prompt_footer(&mut app, 80, 16);
        let buf = terminal.backend().buffer();
        let y = (0..16u16)
            .find(|y| cell_seq_col(buf, *y, 80, "important").is_some())
            .expect("important chip");
        let col = cell_seq_col(buf, y, 80, "important").unwrap();
        assert_eq!(buf[(col, y)].fg, palette.warning);

        app.approval_mode = ApprovalMode::Manual;
        let (terminal, palette) = draw_prompt_footer(&mut app, 80, 16);
        let buf = terminal.backend().buffer();
        let y = (0..16u16)
            .find(|y| cell_seq_col(buf, *y, 80, "manual").is_some())
            .expect("manual chip");
        let col = cell_seq_col(buf, y, 80, "manual").unwrap();
        assert_eq!(buf[(col, y)].fg, palette.fg);
    }

    #[test]
    fn bottom_meta_drops_approval_before_effort_when_narrow() {
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.agent_name = "build".into();
        app.provider_name = "xai".into();
        app.model_name = "grok-4".into();
        app.approval_mode = ApprovalMode::Auto;
        // ` build · xai/grok-4 · Med · auto ` is 33 cols; max_w = width - 4.
        // width 32 → max_w 28: drop auto, keep Med.
        let (terminal, _) = draw_prompt_footer(&mut app, 32, 16);
        let buf = terminal.backend().buffer();
        let row = footer_line_containing(buf, 32, 16, "build");
        assert!(row.contains("build"), "{row}");
        assert!(
            row.contains("Med"),
            "effort stays after dropping mode: {row}"
        );
        assert!(!row.contains("auto"), "approval is first to drop: {row}");
        assert!(app.effort_hit.rect.is_some(), "effort remains clickable");
        assert!(
            app.approval_hit.rect.is_none(),
            "dropped approval has no hit"
        );
    }

    #[test]
    fn bottom_meta_hover_underlines_agent_and_model() {
        let backend = TestBackend::new(80, 16);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.agent_name = "build".into();
        app.provider_name = "anthropic".into();
        app.model_name = "sonnet".into();
        app.agent_hit.hovered = true;
        app.model_hit.hovered = true;
        let palette = app.config.palette();

        terminal
            .draw(|f| {
                let area = f.area();
                render(
                    f,
                    Rect {
                        x: 0,
                        y: 8,
                        width: area.width,
                        height: 8,
                    },
                    &mut app,
                    &palette,
                );
            })
            .unwrap();

        let buf = terminal.backend().buffer();
        let mut agent_mod = None;
        let mut model_mod = None;
        for y in 0..16u16 {
            if let Some(col) = cell_seq_col(buf, y, 80, "build") {
                agent_mod = Some(buf[(col, y)].modifier);
            }
            if let Some(col) = cell_seq_col(buf, y, 80, "sonnet") {
                model_mod = Some(buf[(col, y)].modifier);
            }
        }
        assert!(
            agent_mod.is_some_and(|m| m.contains(Modifier::UNDERLINED)),
            "hovered agent should be underlined: {agent_mod:?}"
        );
        assert!(
            model_mod.is_some_and(|m| m.contains(Modifier::UNDERLINED)),
            "hovered model should be underlined: {model_mod:?}"
        );
    }

    #[test]
    fn prompt_box_border_uses_agent_color() {
        let backend = TestBackend::new(80, 16);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.agent_name = "build".into();
        app.focus = crate::app::FocusPane::Prompt;
        let palette = app.config.palette();
        let expected = app
            .config
            .agent_color(&app.agent_name, app.agent_cycle_idx, &palette);

        terminal
            .draw(|f| {
                let area = f.area();
                render(
                    f,
                    Rect {
                        x: 0,
                        y: 8,
                        width: area.width,
                        height: 8,
                    },
                    &mut app,
                    &palette,
                );
            })
            .expect("draw prompt");

        let buf = terminal.backend().buffer();
        let mut corner_fg = None;
        let mut side_fg = None;
        for y in 0..16u16 {
            for x in 0..80u16 {
                match buf[(x, y)].symbol() {
                    "╭" | "╮" | "╰" | "╯" => corner_fg = Some(buf[(x, y)].fg),
                    "│" => side_fg = Some(buf[(x, y)].fg),
                    _ => {}
                }
            }
        }
        assert_eq!(
            corner_fg,
            Some(expected),
            "box corners should use the agent color"
        );
        assert_eq!(
            side_fg,
            Some(expected),
            "box side borders should use the agent color"
        );
    }

    fn cell_seq_col(
        buf: &ratatui::buffer::Buffer,
        y: u16,
        width: u16,
        needle: &str,
    ) -> Option<u16> {
        let chars: Vec<char> = needle.chars().collect();
        if chars.is_empty() {
            return None;
        }
        let n = chars.len() as u16;
        for x in 0..=width.saturating_sub(n) {
            if (0..n).all(|i| buf[(x + i, y)].symbol() == chars[i as usize].to_string()) {
                return Some(x);
            }
        }
        None
    }

    #[test]
    fn agent_color_spec_overrides_named_default() {
        let mut cfg = TuiAppConfig::default();
        cfg.agent_color_specs
            .insert("build".into(), "#ff00aa".into());
        cfg.agent_color_specs
            .insert("model".into(), "warning".into());
        let palette = cfg.palette();
        assert_eq!(
            cfg.agent_color("build", 0, &palette),
            ratatui::style::Color::Rgb(0xff, 0x00, 0xaa)
        );
        assert_eq!(cfg.model_color(&palette), palette.warning);
        assert_eq!(cfg.agent_color("plan", 1, &palette), palette.accent);
    }

    #[test]
    fn collapsed_long_paste_is_single_row() {
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.insert_paste_text(&"y".repeat(500));
        assert_eq!(app.pending_pastes.len(), 1);
        assert_eq!(input_row_count(&app, 80), 1);
    }

    #[test]
    fn hard_wrap_rows_never_exceed_text_width() {
        let area_w = 50u16;
        let text_w = area_w.saturating_sub(CHROME_H).max(1);
        let buf = "Z".repeat(300);
        for row in wrap_text(&buf, text_w) {
            let slice = &buf[row.byte_range.0..row.byte_range.1];
            let w: usize = slice
                .chars()
                .map(|c| {
                    unicode_width::UnicodeWidthChar::width(c)
                        .unwrap_or(0)
                        .max(1)
                })
                .sum();
            assert!(
                w <= text_w as usize,
                "row width {w} > text_w {text_w}: {slice:?}"
            );
            // full line with prefix must fit in area - PAD_LEFT - PAD_RIGHT
            let full = PREFIX_WIDTH as usize + w;
            let rect_w = (area_w - PAD_LEFT - PAD_RIGHT) as usize;
            assert!(full <= rect_w, "prefix+text {full} > rect {rect_w}");
        }
    }

    #[test]
    fn outer_gap_clears_stale_paste_glyphs() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = TuiApp::new(TuiAppConfig::default());
        // Session layout (not the centered home box) — leftover paste sat
        // in the 2-row gap above ╭─╮ after a long paste + continue.
        app.add_message(crate::app::ChatRole::User, "hi");
        let area = Rect {
            x: 0,
            y: 8,
            width: 80,
            height: 10,
        };
        let palette = app.config.palette();
        terminal
            .draw(|f| {
                let stain: Vec<Line> = (0..20).map(|_| Line::from("W".repeat(80))).collect();
                f.render_widget(Paragraph::new(Text::from(stain)), f.area());
                render(f, area, &mut app, &palette);
            })
            .unwrap();

        let buf = terminal.backend().buffer();
        let mut top_y = None;
        for y in 0..20u16 {
            for x in 0..80u16 {
                if buf[(x, y)].symbol() == "╭" {
                    top_y = Some(y);
                    break;
                }
            }
        }
        let top_y = top_y.expect("prompt ╭");
        assert_eq!(
            top_y,
            area.y + OUTER_TOP_GAP,
            "box should sit under the gap"
        );
        for y in area.y..top_y {
            for x in area.x..area.x.saturating_add(area.width) {
                assert_ne!(buf[(x, y)].symbol(), "W", "paste leftover in gap ({x},{y})");
            }
        }
    }

    fn backend_cursor_visible(terminal: &mut Terminal<TestBackend>) -> bool {
        format!("{:?}", terminal.backend()).contains("cursor: true")
    }

    #[test]
    fn focused_prompt_places_native_caret_after_prefix() {
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.focus = crate::app::FocusPane::Prompt;
        app.input_buffer = "hi".into();
        app.input_cursor = 1; // after 'h'
        assert!(prompt_owns_caret(&app));
        let backend = TestBackend::new(60, 8);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let palette = app.config.palette();
        terminal
            .draw(|frame| render(frame, frame.area(), &mut app, &palette))
            .expect("draw prompt");
        assert!(
            backend_cursor_visible(&mut terminal),
            "focused prompt must show the native caret"
        );
        let pos = terminal
            .get_cursor_position()
            .expect("focused prompt must place a native caret");
        let buffer = terminal.backend().buffer();
        let (row_i, col) = (0..8u16)
            .find_map(|y| cell_seq_col(buffer, y, 60, "❯ h").map(|x| (y, x)))
            .expect("input row");
        // `❯ ` is 2 cols; caret sits on the char after the prefix + cursor_col.
        assert_eq!(pos.y, row_i);
        assert_eq!(pos.x, col + PREFIX_WIDTH + 1);
    }

    #[test]
    fn unfocused_prompt_hides_native_caret() {
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.focus = crate::app::FocusPane::Scrollback;
        app.add_message(crate::app::ChatRole::User, "hi");
        app.input_buffer = "draft".into();
        app.input_cursor = app.input_buffer.len();
        assert!(!prompt_owns_caret(&app));
        let backend = TestBackend::new(60, 8);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let palette = app.config.palette();
        terminal
            .draw(|frame| render(frame, frame.area(), &mut app, &palette))
            .expect("draw prompt");
        assert!(
            !backend_cursor_visible(&mut terminal),
            "scrollback focus must hide the prompt caret"
        );
    }

    #[test]
    fn modal_hides_prompt_caret() {
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.focus = crate::app::FocusPane::Prompt;
        app.mode = AppMode::Help;
        assert!(
            !prompt_owns_caret(&app),
            "help overlay must not leave a caret on the draft"
        );
    }

    #[test]
    fn home_side_gutters_clear_stale_paste_glyphs() {
        use crate::config::TuiAppConfig;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let backend = TestBackend::new(100, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = TuiApp::new(TuiAppConfig::default());
        assert!(app.messages.is_empty(), "home layout");
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 12,
        };
        let palette = app.config.palette();
        terminal
            .draw(|f| {
                let stain: Vec<Line> = (0..12).map(|_| Line::from("W".repeat(100))).collect();
                f.render_widget(Paragraph::new(Text::from(stain)), f.area());
                render(f, area, &mut app, &palette);
            })
            .unwrap();

        let buf = terminal.backend().buffer();
        let mut box_x = None;
        let mut box_y = None;
        for y in 0..12u16 {
            for x in 0..100u16 {
                if buf[(x, y)].symbol() == "╭" {
                    box_x = Some(x);
                    box_y = Some(y);
                    break;
                }
            }
        }
        let box_x = box_x.expect("prompt ╭");
        let box_y = box_y.expect("prompt ╭");
        assert!(box_x > 0, "home prompt should be inset, box_x={box_x}");
        let mut box_right = None;
        for x in (0..100u16).rev() {
            if buf[(x, box_y)].symbol() == "╮" {
                box_right = Some(x);
                break;
            }
        }
        let box_right = box_right.expect("prompt ╮");
        for y in box_y..box_y.saturating_add(3) {
            for x in 0..box_x {
                assert_ne!(
                    buf[(x, y)].symbol(),
                    "W",
                    "paste leftover left of prompt ({x},{y})"
                );
            }
            for x in box_right.saturating_add(1)..100 {
                assert_ne!(
                    buf[(x, y)].symbol(),
                    "W",
                    "paste leftover right of prompt ({x},{y})"
                );
            }
        }
    }
}
