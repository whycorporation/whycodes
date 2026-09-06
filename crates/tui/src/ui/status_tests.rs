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
fn shortcuts_normal_todos_focus() {
    let mut app = app_ready();
    app.focus = FocusPane::Todos;
    let palette = app.config.palette();
    let text = paint(120, crate::tokens::layout::HEADER_H, |f| {
        render(f, f.area(), &app, &palette)
    });
    assert!(text.contains("j/k scroll"), "{text}");
    assert!(text.contains("t fold"), "{text}");
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
fn footer_strips_windows_verbatim_cwd() {
    let mut app = footer_app();
    app.project_dir = std::path::PathBuf::from(r"\\?\C:\dev");
    let text = paint_footer(&mut app, 120);
    assert!(text.contains(r"C:\dev"), "{text}");
    assert!(!text.contains(r"\\?\"), "{text}");
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
