// ── ui/status.rs: Top header + bottom cwd / context bar ────────────────
// Top: upright status square + dual-tone brand · project · shortcuts.
// Bottom: git branch + cwd (click-to-copy) · context used/max (hover → %).

use crate::app::{AgentState, AppMode, FocusPane, TuiApp};
use crate::theme::ThemePalette;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
};
use std::sync::OnceLock;
use unicode_width::UnicodeWidthStr;

const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Filled upright square (not a round bullet). U+25AE BLACK VERTICAL RECTANGLE
/// stands taller in the cell — a solid status chip rather than a flat disk.
const STATUS_SQUARE: &str = "▮";
/// Hollow upright square while waiting on the user.
const STATUS_SQUARE_OPEN: &str = "▯";

/// Grok-style branch glyph (process-lifetime cache).
///
/// Prefer Powerline Nerd Font `` (`U+E0A0`). Without a patched font it tofu,
/// so fall back to platform stock glyphs (`⎇` / Windows `≡`). Override with
/// `WHYCODE_NERD_FONTS=1|0` (same semantics as Grok's `GROK_NERD_FONTS`).
fn branch_icon() -> &'static str {
    static ICON: OnceLock<&'static str> = OnceLock::new();
    ICON.get_or_init(|| {
        let nerd = match std::env::var("WHYCODE_NERD_FONTS").ok().as_deref() {
            Some("0") | Some("false") => false,
            Some(_) => true,
            // Grok assumes Nerd Fonts everywhere except Windows consoles and
            // stock macOS terminals; we skip brand detection and default on
            // for non-Windows hosts (primary whycode target is Linux TUI).
            None => !cfg!(windows),
        };
        if nerd {
            "\u{e0a0}" //  Powerline branch
        } else if cfg!(windows) {
            "\u{2261}" // ≡ CP437-safe
        } else {
            "\u{2387}" // ⎇
        }
    })
}

/// Dual-tone wordmark: bold fg `why` + bold accent `code`.
fn brand_spans(palette: &ThemePalette) -> Vec<Span<'static>> {
    vec![
        Span::styled(
            "why",
            Style::default().fg(palette.fg).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "code",
            Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD),
        ),
    ]
}

fn status_glyph(app: &TuiApp, palette: &ThemePalette) -> Span<'static> {
    match &app.current_agent_state {
        AgentState::Generating | AgentState::Thinking => Span::styled(
            SPINNER_FRAMES[app.spinner_frame % SPINNER_FRAMES.len()].to_string(),
            Style::default().fg(palette.accent),
        ),
        AgentState::WaitingForPermission => Span::styled(
            STATUS_SQUARE_OPEN.to_string(),
            Style::default().fg(palette.warning),
        ),
        AgentState::Error(_) => Span::styled(
            STATUS_SQUARE.to_string(),
            Style::default().fg(palette.error),
        ),
        AgentState::Idle => Span::styled(
            STATUS_SQUARE.to_string(),
            Style::default().fg(palette.success),
        ),
    }
}

pub fn render(frame: &mut Frame, area: Rect, app: &TuiApp, palette: &ThemePalette) {
    if area.height == 0 || area.width < 8 {
        return;
    }

    let glyph = status_glyph(app, palette);
    let brand = brand_spans(palette);

    // Prefer the session title in the header when it is more specific than the
    // bare project folder (auto-title / rename). Fall back to project basename.
    let title_raw = app.session_title.trim();
    let dir_raw = app.project_label.trim();
    let label_src = if !title_raw.is_empty() && title_raw != dir_raw {
        title_raw
    } else if !dir_raw.is_empty() && !dir_raw.eq_ignore_ascii_case("whycode") && dir_raw != "." {
        dir_raw
    } else {
        ""
    };
    let dir = if !label_src.is_empty() {
        truncate_start(
            label_src,
            ((area.width / 4).max(10)).min(area.width / 3) as usize,
        )
    } else {
        String::new()
    };

    let no_key = app.status_message.contains("no API key")
        || app.status_message.contains("/connect")
        || (app.provider_name.is_empty() && app.messages.is_empty());

    let right: Vec<Span<'_>> = if no_key && matches!(app.current_agent_state, AgentState::Idle) {
        vec![
            Span::styled(
                String::from("Get started "),
                Style::default().fg(palette.fg),
            ),
            Span::styled(String::from("/connect"), Style::default().fg(palette.dim)),
            Span::raw(" "),
        ]
    } else {
        shortcuts_spans(app, palette)
    };

    // Layout: ▮  whycode [· dir] …… shortcuts
    let mut left: Vec<Span<'_>> = vec![glyph, Span::raw("  ")];
    left.extend(brand);
    if !dir.is_empty() {
        left.push(Span::styled(
            format!("  ·  {dir}"),
            Style::default().fg(palette.dim),
        ));
    }

    let left_w: usize = left.iter().map(|s| s.content.as_ref().width()).sum();
    let right_w: usize = right.iter().map(|s| s.content.as_ref().width()).sum();
    let mid = area
        .width
        .saturating_sub(left_w as u16)
        .saturating_sub(right_w as u16)
        .saturating_sub(1) as usize;

    let mut spans: Vec<Span<'_>> = left;
    spans.push(Span::raw(" ".repeat(mid)));
    spans.extend(right);

    frame.render_widget(
        Paragraph::new(Text::from(Line::from(spans))).style(Style::default().bg(palette.bg)),
        area,
    );
}

/// Grok-style context-aware shortcuts bar (right side of status header).
fn shortcuts_spans(app: &TuiApp, palette: &ThemePalette) -> Vec<Span<'static>> {
    let mut parts: Vec<(&str, &str)> = Vec::new();

    match app.mode {
        AppMode::Help => {
            parts.push(("q/esc", "close"));
        }
        AppMode::Command => {
            parts.push(("enter", "run"));
            parts.push(("esc", "back"));
        }
        AppMode::Dialog => {
            parts.push(("y/n", "confirm"));
            parts.push(("esc", "close"));
        }
        AppMode::Normal | AppMode::Session => {
            if app.is_busy() {
                parts.push(("esc", "cancel"));
                parts.push(("tab", "focus"));
            } else {
                match app.focus {
                    FocusPane::Prompt => {
                        parts.push(("enter", "send"));
                        parts.push(("tab", "scrollback"));
                        parts.push(("^t", "agent"));
                        parts.push(("?", "help"));
                    }
                    FocusPane::Scrollback => {
                        parts.push(("j/k", "select"));
                        parts.push(("y", "copy"));
                        parts.push(("e", "fold"));
                        parts.push(("tab", "prompt"));
                    }
                }
            }
        }
    }

    let mut spans = Vec::new();
    for (i, (key, label)) in parts.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(
                " · ".to_string(),
                Style::default().fg(palette.dim),
            ));
        }
        spans.push(Span::styled(
            key.to_string(),
            Style::default().fg(palette.fg).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {label}"),
            Style::default().fg(palette.dim),
        ));
    }
    spans.push(Span::raw(" "));
    spans
}

fn truncate_start(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        s.to_string()
    } else {
        format!("…{}", s.chars().skip(n - max + 1).collect::<String>())
    }
}

/// Bottom chrome: ` branch /path` (left, click-to-copy) and Grok-style
/// context meter on the right.
///
/// Default always shows fill: `1.2k / 200k · 1%` (percent does not depend on
/// mouse motion — many hosts never send `Moved`). Hover/click focuses the
/// chip to a bold `%` only (same swap idea as Grok’s context ring).
pub fn render_footer(frame: &mut Frame, area: Rect, app: &mut TuiApp, palette: &ThemePalette) {
    app.cwd_hit = None;
    app.context_hit = None;
    if area.height == 0 || area.width < 4 {
        return;
    }

    // ── Right: context window meter (Grok top-right style, bottom-right here) ──
    let max = app.max_context_tokens.max(1);
    let used = app.context_used;
    let pct = app.context_percent();
    let usage_s = crate::app::format_context_usage(used, max);
    let pct_s = crate::app::format_context_percent(used, max);
    // Idle label always includes % so the value is visible without hover.
    let idle_label = format!("{usage_s} · {pct_s}");
    // Reserve the wider form so hover → `%` never shrinks the hit box.
    let right_w = idle_label.width().max(pct_s.width()) as u16;
    let hovered = app.context_hovered();
    let right_label = if hovered {
        let pad = right_w.saturating_sub(pct_s.width() as u16) as usize;
        format!("{}{pct_s}", " ".repeat(pad))
    } else {
        let pad = right_w.saturating_sub(idle_label.width() as u16) as usize;
        format!("{}{idle_label}", " ".repeat(pad))
    };
    let mut right_style = Style::default().fg(context_meter_color(pct, palette));
    if hovered {
        right_style = right_style.add_modifier(Modifier::BOLD);
    }
    // Leave at least one space gap between path and meter.
    let right_reserve = right_w.saturating_add(1).min(area.width);

    let path_full = app.project_dir.display().to_string();
    let mut spans: Vec<Span<'_>> = Vec::new();
    let mut path_start_cols: u16 = 0;

    // Grok session status bar: single dim primary span for `{icon} {branch}`.
    // No bold on the branch name — same weight as the rest of the chrome.
    if let Some(ref branch) = app.git_branch {
        let branch_disp = truncate_start(branch, 24);
        let icon = branch_icon();
        let git_text = if branch_disp.is_empty() {
            format!("{icon} detached")
        } else {
            format!("{icon} {branch_disp}")
        };
        let git_style = Style::default().fg(palette.fg).add_modifier(Modifier::DIM);
        let sep = Span::styled(" ", Style::default());
        path_start_cols = (git_text.width() + sep.content.as_ref().width()) as u16;
        spans.push(Span::styled(git_text, git_style));
        spans.push(sep);
    }

    let path_budget = area
        .width
        .saturating_sub(path_start_cols)
        .saturating_sub(right_reserve)
        .max(4) as usize;
    let path_disp = truncate_start(&path_full, path_budget);
    let path_w = path_disp.width() as u16;

    spans.push(Span::styled(path_disp, Style::default().fg(palette.dim)));

    // Absolute screen coords of the path text for click-to-copy.
    if path_w > 0 {
        app.cwd_hit = Some(Rect {
            x: area.x.saturating_add(path_start_cols),
            y: area.y,
            width: path_w.min(area.width.saturating_sub(path_start_cols)),
            height: 1,
        });
    }

    // Pad between path and right-aligned meter.
    let left_w = path_start_cols.saturating_add(path_w);
    let gap = area.width.saturating_sub(left_w).saturating_sub(right_w);
    if gap > 0 {
        spans.push(Span::raw(" ".repeat(gap as usize)));
    }
    spans.push(Span::styled(right_label, right_style));

    if right_w > 0 && area.width >= right_w {
        // Hit the whole right cluster (and one cell of gap when present) so
        // easy hover; y is the footer row inside the safe inset (not the
        // terminal’s very last row when SAFE_BOTTOM > 0).
        let hit_w = right_w.saturating_add(if gap > 0 { 1 } else { 0 }).min(area.width);
        app.context_hit = Some(Rect {
            x: area.x.saturating_add(area.width.saturating_sub(hit_w)),
            y: area.y,
            width: hit_w,
            height: 1,
        });
    }

    frame.render_widget(
        Paragraph::new(Text::from(Line::from(spans))).style(Style::default().bg(palette.bg)),
        area,
    );
}

/// Calm dim until pressure builds — matches Grok / OpenCode cue colors.
fn context_meter_color(pct: u64, palette: &ThemePalette) -> ratatui::style::Color {
    if pct >= 90 {
        palette.error
    } else if pct >= 75 {
        palette.warning
    } else {
        palette.dim
    }
}
