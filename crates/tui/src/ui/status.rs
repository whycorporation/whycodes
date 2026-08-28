// ── ui/status.rs: Top header + bottom cwd / context bar ────────────────
// Top: `?` mark + fg wordmark · project · shortcuts.
// Bottom: git branch + cwd (click-to-copy, hover underline) · Grok context bar.

use crate::app::{AgentState, AppMode, FocusPane, TuiApp};
use crate::theme::ThemePalette;
use crate::tokens::HEADER_MARK;
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

const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Filled upright square (not a round bullet). U+25AE BLACK VERTICAL RECTANGLE
/// stands taller in the cell — a solid status chip rather than a flat disk.
const STATUS_SQUARE: &str = "▮";
/// Hollow upright square while waiting on the user.
const STATUS_SQUARE_OPEN: &str = "▯";

/// Grok-style branch glyph (process-lifetime cache).
fn branch_icon() -> &'static str {
    static ICON: OnceLock<&'static str> = OnceLock::new();
    ICON.get_or_init(|| {
        let nerd = match std::env::var("WHYCODES_NERD_FONTS").ok().as_deref() {
            Some("0") | Some("false") => false,
            Some(_) => true,
            None => !cfg!(windows),
        };
        if nerd {
            "\u{e0a0}" //  Powerline branch
        } else if cfg!(windows) {
            "\u{2261}" // ≡
        } else {
            "\u{2387}" // ⎇
        }
    })
}

/// Home-matching wordmark: dim `why` + bold fg `codes` (no accent/blue).
fn brand_wordmark(palette: &ThemePalette) -> Vec<Span<'static>> {
    vec![
        Span::styled("why", Style::default().fg(palette.dim)),
        Span::styled(
            "codes",
            Style::default().fg(palette.fg).add_modifier(Modifier::BOLD),
        ),
    ]
}

fn status_glyph(app: &TuiApp, palette: &ThemePalette) -> Span<'static> {
    match &app.current_agent_state {
        AgentState::Generating | AgentState::Thinking => Span::styled(
            SPINNER_FRAMES[app.spinner_frame % SPINNER_FRAMES.len()].to_string(),
            Style::default().fg(palette.accent),
        ),
        AgentState::WaitingForPermission | AgentState::WaitingForQuestion => Span::styled(
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

    let title_raw = app.session_title.trim();
    let dir_raw = app.project_label.trim();
    let label_src = if !title_raw.is_empty() && title_raw != dir_raw {
        title_raw
    } else if !dir_raw.is_empty() && !dir_raw.eq_ignore_ascii_case("whycodes") && dir_raw != "." {
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

    let mut left: Vec<Span<'_>> = vec![
        Span::styled(
            HEADER_MARK,
            Style::default().fg(palette.fg).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        glyph,
        Span::raw("  "),
    ];
    left.extend(brand_wordmark(palette));
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
                        parts.push(("/help", "help"));
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

/// Bottom chrome: branch + cwd (left) · Grok context meter (right, via StatusBar).
///
/// Default: `1.2k / 200k` · hover: `████░ 42.0%` (same width). Sticky hover on
/// `app.context_hit` — never recompute after clearing rect mid-paint.
pub fn render_footer(frame: &mut Frame, area: Rect, app: &mut TuiApp, palette: &ThemePalette) {
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

    let path_full = app.project_dir.display().to_string();
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

/// Urgency color from fill percent (Grok context bar gradient).
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
mod tests {
    use super::*;
    use crate::app::{AgentState, AppMode, FocusPane, TuiApp};
    use crate::config::TuiAppConfig;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn cfg() -> TuiAppConfig {
        TuiAppConfig::default()
    }

    /// Render `f` into a fresh terminal and return the painted buffer text.
    fn paint<F>(width: u16, height: u16, f: F) -> String
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
        out
    }

    /// App configured with a provider so the "Get started" branch is off.
    fn app_ready() -> TuiApp {
        let mut app = TuiApp::new(cfg());
        app.provider_name = "anthropic".into();
        app
    }

    #[test]
    fn glyph_matches_agent_state() {
        let palette = TuiApp::new(cfg()).config.palette();

        let mut app = TuiApp::new(cfg());
        app.current_agent_state = AgentState::Idle;
        let glyph = status_glyph(&app, &palette);
        assert_eq!(glyph.content.as_ref(), STATUS_SQUARE);

        app.current_agent_state = AgentState::Error("boom".into());
        let glyph = status_glyph(&app, &palette);
        assert_eq!(glyph.content.as_ref(), STATUS_SQUARE);

        app.current_agent_state = AgentState::WaitingForPermission;
        let glyph = status_glyph(&app, &palette);
        assert_eq!(glyph.content.as_ref(), STATUS_SQUARE_OPEN);

        app.current_agent_state = AgentState::WaitingForQuestion;
        let glyph = status_glyph(&app, &palette);
        assert_eq!(glyph.content.as_ref(), STATUS_SQUARE_OPEN);

        app.current_agent_state = AgentState::Generating;
        app.spinner_frame = 3;
        let glyph = status_glyph(&app, &palette);
        assert_eq!(glyph.content.as_ref(), SPINNER_FRAMES[3]);

        app.current_agent_state = AgentState::Thinking;
        app.spinner_frame = 7;
        let glyph = status_glyph(&app, &palette);
        assert_eq!(glyph.content.as_ref(), SPINNER_FRAMES[7]);
    }

    #[test]
    fn paints_whycodes_wordmark_as_one_word() {
        let app = TuiApp::new(cfg());
        let palette = app.config.palette();
        let text = paint(120, crate::tokens::layout::HEADER_H, |f| {
            render(f, f.area(), &app, &palette)
        });
        assert!(text.contains('?'), "{text}");
        assert!(text.contains("whycodes"), "{text}");
        assert!(!text.contains("why codes"), "{text}");
    }

    #[test]
    fn shows_get_started_when_no_key_and_idle() {
        // Default app: empty provider, empty messages, Idle → /connect hint.
        let app = TuiApp::new(cfg());
        let palette = app.config.palette();
        let text = paint(120, crate::tokens::layout::HEADER_H, |f| {
            render(f, f.area(), &app, &palette)
        });
        assert!(text.contains("Get started"), "{text}");
        assert!(text.contains("/connect"), "{text}");
    }

    #[test]
    fn hides_get_started_when_configured() {
        let app = app_ready();
        let palette = app.config.palette();
        let text = paint(120, crate::tokens::layout::HEADER_H, |f| {
            render(f, f.area(), &app, &palette)
        });
        assert!(!text.contains("Get started"), "{text}");
        assert!(text.contains("enter send"), "{text}");
    }

    #[test]
    fn shortcuts_help_mode() {
        let mut app = app_ready();
        app.mode = AppMode::Help;
        let palette = app.config.palette();
        let text = paint(120, crate::tokens::layout::HEADER_H, |f| {
            render(f, f.area(), &app, &palette)
        });
        assert!(text.contains("q/esc"), "{text}");
        assert!(text.contains("close"), "{text}");
    }

    #[test]
    fn shortcuts_command_mode() {
        let mut app = app_ready();
        app.mode = AppMode::Command;
        let palette = app.config.palette();
        let text = paint(120, crate::tokens::layout::HEADER_H, |f| {
            render(f, f.area(), &app, &palette)
        });
        assert!(text.contains("enter"), "{text}");
        assert!(text.contains("run"), "{text}");
        assert!(text.contains("esc"), "{text}");
        assert!(text.contains("back"), "{text}");
    }

    #[test]
    fn shortcuts_dialog_mode() {
        let mut app = app_ready();
        app.mode = AppMode::Dialog;
        let palette = app.config.palette();
        let text = paint(120, crate::tokens::layout::HEADER_H, |f| {
            render(f, f.area(), &app, &palette)
        });
        assert!(text.contains("y/n"), "{text}");
        assert!(text.contains("confirm"), "{text}");
    }

    #[test]
    fn shortcuts_normal_prompt_focus() {
        let app = app_ready();
        let palette = app.config.palette();
        let text = paint(120, crate::tokens::layout::HEADER_H, |f| {
            render(f, f.area(), &app, &palette)
        });
        assert!(text.contains("enter send"), "{text}");
        assert!(text.contains("tab scrollback"), "{text}");
        assert!(text.contains("^t agent"), "{text}");
        assert!(text.contains("/help help"), "{text}");
    }

    #[test]
    fn shortcuts_normal_scrollback_focus() {
        let mut app = app_ready();
        app.focus = FocusPane::Scrollback;
        let palette = app.config.palette();
        let text = paint(120, crate::tokens::layout::HEADER_H, |f| {
            render(f, f.area(), &app, &palette)
        });
        assert!(text.contains("j/k select"), "{text}");
        assert!(text.contains("y copy"), "{text}");
        assert!(text.contains("e fold"), "{text}");
        assert!(text.contains("tab prompt"), "{text}");
    }

    #[test]
    fn shortcuts_busy_shows_cancel_and_focus() {
        let mut app = app_ready();
        app.current_agent_state = AgentState::Generating;
        let palette = app.config.palette();
        let text = paint(120, crate::tokens::layout::HEADER_H, |f| {
            render(f, f.area(), &app, &palette)
        });
        assert!(text.contains("esc cancel"), "{text}");
        assert!(text.contains("tab focus"), "{text}");
        assert!(!text.contains("enter send"), "{text}");
    }

    #[test]
    fn truncate_start_keeps_short_and_ellipsizes_long() {
        assert_eq!(truncate_start("short", 20), "short");
        assert_eq!(truncate_start("", 5), "");
        assert_eq!(truncate_start("abcdef", 3), "…ef");
        assert_eq!(truncate_start("abcdef", 6), "abcdef");
        // Multibyte characters count as chars, not bytes; the ellipsis
        // counts toward the max budget.
        assert_eq!(truncate_start("çok uzun başlık", 6), "…aşlık");
    }

    #[test]
    fn tiny_area_skips_render() {
        let app = app_ready();
        let palette = app.config.palette();
        let text = paint(4, 1, |f| render(f, f.area(), &app, &palette));
        assert!(text.trim().is_empty(), "{text}");
    }

    // ── footer (branch / cwd / context meter) ────────────────────────────

    fn footer_app() -> TuiApp {
        let mut app = app_ready();
        app.project_dir = std::path::PathBuf::from("/tmp/proj");
        app.git_branch = Some("main".into());
        app
    }

    fn paint_footer(app: &mut TuiApp, width: u16) -> String {
        let palette = app.config.palette();
        paint(width, 1, |f| render_footer(f, f.area(), app, &palette))
    }

    #[test]
    fn footer_paints_branch_path_and_context() {
        let mut app = footer_app();
        let text = paint_footer(&mut app, 120);
        assert!(text.contains("main"), "{text}");
        assert!(text.contains("/tmp/proj"), "{text}");
        assert!(text.contains("0 / 200k"), "{text}");
        // Hit rects recorded for mouse handling.
        assert!(app.cwd_hit.rect.is_some(), "cwd hit recorded");
        assert!(app.context_hit.rect.is_some(), "context hit recorded");
    }

    #[test]
    fn footer_detached_head_without_branch_name() {
        let mut app = footer_app();
        app.git_branch = Some(String::new());
        let text = paint_footer(&mut app, 120);
        assert!(text.contains("detached"), "{text}");
    }

    #[test]
    fn footer_shows_bg_job_count() {
        let mut app = footer_app();
        app.bg_running_count = 2;
        let text = paint_footer(&mut app, 120);
        assert!(text.contains("bg:2"), "{text}");
    }

    #[test]
    fn footer_hovered_context_shows_percent() {
        let mut app = footer_app();
        app.context_used = 100_000;
        app.context_hit.hovered = true;
        let text = paint_footer(&mut app, 120);
        assert!(text.contains("50.0%"), "{text}");
        // Hover swaps the token string for a progress bar + percent, keeping
        // the same total width (5-bar + gap + 5-char percent).
        assert!(text.contains("██▌░░"), "{text}");
    }

    #[test]
    fn footer_hovered_cwd_underlines_path() {
        let mut app = footer_app();
        app.cwd_hit.hovered = true;
        let text = paint_footer(&mut app, 120);
        assert!(text.contains("/tmp/proj"), "{text}");
        assert!(app.cwd_hit.rect.is_some(), "cwd hit recorded");
    }

    #[test]
    fn footer_tiny_area_skips() {
        let mut app = footer_app();
        let palette = app.config.palette();
        let text = paint(3, 1, |f| render_footer(f, f.area(), &mut app, &palette));
        assert!(text.trim().is_empty(), "{text}");
    }

    #[test]
    fn context_meter_color_scales_with_fill() {
        let palette = TuiApp::new(cfg()).config.palette();
        // 0% → dim-ish start of gradient; >100% → far end (no panic on overflow).
        let _low = context_meter_color(0.0, &palette);
        let _high = context_meter_color(150.0, &palette);
        let _mid = context_meter_color(50.0, &palette);
    }
}
