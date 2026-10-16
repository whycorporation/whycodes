// ── ui/status.rs: top location bar + bottom shortcuts bar ──────────────
// Top: git branch + cwd (click-to-copy, hover underline) on the left; goal,
// background jobs and the context meter on the right.
// Bottom: context-aware `Key:label  │  Key:label` shortcuts.

use crate::app::{AgentState, AppMode, FocusPane, TuiApp};
use crate::theme::ThemePalette;
use crate::ui::status_bar::StatusBar;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
};
use std::sync::OnceLock;
use unicode_width::UnicodeWidthStr;

/// Branch glyph (process-lifetime cache).
fn branch_icon() -> &'static str {
    static ICON: OnceLock<&'static str> = OnceLock::new();
    ICON.get_or_init(|| {
        branch_icon_from(
            std::env::var("WHYCODES_NERD_FONTS").ok().as_deref(),
            cfg!(windows),
        )
    })
}

/// Host detection split out so Linux CI can drive the Windows glyph.
fn branch_icon_from(nerd_env: Option<&str>, is_windows: bool) -> &'static str {
    let nerd = match nerd_env {
        Some("0") | Some("false") => false,
        Some(_) => true,
        None => !is_windows,
    };
    if nerd {
        "\u{e0a0}" //  Powerline branch
    } else if is_windows {
        "\u{2261}" // ≡
    } else {
        "\u{2387}" // ⎇
    }
}

pub fn render(frame: &mut Frame, area: Rect, app: &mut TuiApp, palette: &ThemePalette) {
    render_location(frame, area, app, palette);
}

/// Bottom chrome: context-aware shortcuts, or how to get started when no
/// provider key is set up yet.
pub fn render_footer(frame: &mut Frame, area: Rect, app: &TuiApp, palette: &ThemePalette) {
    if area.height == 0 || area.width < 8 {
        return;
    }
    let no_key = app.status_message.contains("no API key")
        || app.status_message.contains("/connect")
        || (app.provider_name.is_empty() && app.messages.is_empty());
    let spans: Vec<Span<'_>> = if no_key && matches!(app.current_agent_state, AgentState::Idle) {
        vec![
            Span::styled(
                String::from("Get started "),
                Style::default().fg(palette.fg),
            ),
            Span::styled(String::from("/connect"), Style::default().fg(palette.dim)),
        ]
    } else {
        shortcuts_spans(app, palette)
    };
    frame.render_widget(
        Paragraph::new(Text::from(Line::from(spans))).style(Style::default().bg(palette.bg)),
        area,
    );
}

/// Context-aware shortcuts: `Key:label` pairs, `  │  ` apart.
fn shortcuts_spans(app: &TuiApp, palette: &ThemePalette) -> Vec<Span<'static>> {
    let mut parts: Vec<(&str, &str)> = Vec::new();

    match app.mode {
        AppMode::Help => {
            parts.push(("q/Esc", "close"));
        }
        AppMode::Command => {
            parts.push(("Enter", "run"));
            parts.push(("Esc", "back"));
        }
        AppMode::Dialog => {
            parts.push(("y/n", "confirm"));
            parts.push(("Esc", "close"));
        }
        AppMode::Normal | AppMode::Session => {
            if app.is_busy() {
                if app.queued_turn_count() > 0 {
                    parts.push(("Enter", "queue"));
                }
                parts.push(("Esc", "cancel"));
                parts.push(("Tab", "focus"));
            } else {
                match app.focus {
                    FocusPane::Prompt => {
                        parts.push(("Enter", "send"));
                        parts.push(("Tab", "scrollback"));
                        parts.push(("Ctrl+T", "agent"));
                        parts.push(("/", "commands"));
                    }
                    FocusPane::Scrollback => {
                        parts.push(("j/k", "select"));
                        parts.push(("y", "copy"));
                        parts.push(("e", "fold"));
                        parts.push(("Tab", "prompt"));
                    }
                    FocusPane::Todos => {
                        parts.push(("j/k", "scroll"));
                        parts.push(("t", "fold"));
                        parts.push(("Tab", "prompt"));
                    }
                }
            }
        }
    }

    let mut spans = Vec::new();
    for (i, (key, label)) in parts.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(
                "  │  ".to_string(),
                Style::default().fg(palette.dim),
            ));
        }
        spans.push(Span::styled(
            key.to_string(),
            Style::default().fg(palette.fg).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(":{label}"),
            Style::default().fg(palette.dim),
        ));
    }
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

/// Location bar: branch + cwd (left) · goal, background jobs and the context
/// meter (right, via StatusBar).
///
/// Default: `1.2k / 200k` · hover: `████░ 42.0%` (same width). Sticky hover on
/// `app.context_hit` — never recompute after clearing rect mid-paint.
fn render_location(frame: &mut Frame, area: Rect, app: &mut TuiApp, palette: &ThemePalette) {
    // Clear hit rects only (keep sticky hovered flags).
    app.cwd_hit.set_rect(None);
    app.context_hit.set_rect(None);
    if area.height == 0 || area.width < 4 {
        return;
    }

    let max = app.max_context_tokens.max(1);
    let used = app.context_used;
    let pct_f = crate::app::context_usage_pct(used, max);
    let hovered = app.context_hit.hovered;

    let idle_label = crate::app::format_context_usage(used, max);
    let hover_label = crate::app::format_context_hover(used, max);
    // Same-width invariant for hover swap.
    let right_w = idle_label.width().max(hover_label.width()) as u16;
    let ctx_text = if hovered {
        let pad = right_w.saturating_sub(hover_label.width() as u16) as usize;
        format!("{}{hover_label}", " ".repeat(pad))
    } else {
        let pad = right_w.saturating_sub(idle_label.width() as u16) as usize;
        format!("{idle_label}{}", " ".repeat(pad))
    };
    let ctx_color = context_meter_color(pct_f, palette);
    let ctx_line = Line::from(Span::styled(ctx_text, Style::default().fg(ctx_color)));

    // Reserve right cluster (+ gap) so path truncation doesn't collide.
    let right_reserve = right_w.saturating_add(1).min(area.width);

    let path_full = whycodes_core::display_path(&app.project_dir);
    let mut spans: Vec<Span<'_>> = Vec::new();
    let mut path_start_cols: u16 = 0;

    if let Some(ref branch) = app.git_branch {
        let branch_disp = truncate_start(branch, 24);
        let icon = branch_icon();
        let git_text = if branch_disp.is_empty() {
            format!("{icon} detached")
        } else {
            format!("{icon} {branch_disp}")
        };
        let git_style = Style::default().fg(palette.dim);
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

    let mut path_style = Style::default().fg(palette.dim);
    if app.cwd_hit.hovered {
        path_style = path_style.add_modifier(Modifier::UNDERLINED).fg(palette.fg);
    }
    spans.push(Span::styled(path_disp, path_style));

    if path_w > 0 {
        app.cwd_hit.set_rect(Some(Rect {
            x: area.x.saturating_add(path_start_cols),
            y: area.y,
            width: path_w.min(area.width.saturating_sub(path_start_cols)),
            height: 1,
        }));
    }

    // Left cluster only (StatusBar paints the right side).
    let left_w = path_start_cols.saturating_add(path_w);
    let left_area = Rect {
        x: area.x,
        y: area.y,
        width: left_w.min(area.width),
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Text::from(Line::from(spans))).style(Style::default().bg(palette.bg)),
        left_area,
    );

    // Right: composable StatusBar (bg jobs + context meter).
    let mut bar = StatusBar::new(Style::default().fg(palette.dim).bg(palette.bg));
    if let Some(chip) = app.goal_chip {
        bar.push(
            "goal",
            Line::from(Span::styled(
                chip,
                Style::default().fg(palette.accent).bg(palette.bg),
            )),
        );
    }
    if app.bg_running_count > 0 {
        bar.push(
            "bg",
            Line::from(Span::styled(
                format!("bg:{}", app.bg_running_count),
                Style::default().fg(palette.warning).bg(palette.bg),
            )),
        );
    }
    bar.push("context", ctx_line);
    let areas = bar.render(frame, area);
    if let Some(r) = areas.get("context").copied() {
        app.context_hit.set_rect(Some(r));
    }
}

/// Urgency color from fill percent (context bar gradient).
fn context_meter_color(pct: f64, palette: &ThemePalette) -> ratatui::style::Color {
    use crate::ui::progress_bar::{color_to_rgb, context_urgency_rgb};
    let (r, g, b) = context_urgency_rgb(
        pct,
        color_to_rgb(palette.dim),
        color_to_rgb(palette.accent),
        color_to_rgb(palette.warning),
        color_to_rgb(palette.error),
    );
    ratatui::style::Color::Rgb(r, g, b)
}

#[cfg(test)]
#[path = "status_tests.rs"]
mod tests;
