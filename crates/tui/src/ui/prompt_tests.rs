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
fn trailing_newline_paints_a_blank_continuation_row() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.input_buffer = "hello\n".into();
    app.input_cursor = app.input_buffer.len();
    let rows = rendered_rows(&mut app, 40, 10);
    assert!(
        rows.iter().any(|r| r.contains("hello")),
        "trailing newline must still paint the typed line, got {rows:?}"
    );
    assert!(
        input_row_count(&app, 40) >= 2,
        "a trailing newline must reserve a blank wrap row"
    );
}

#[test]
fn home_prompt_centers_and_narrow_drops_chips() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.agent_name = "build".into();
    app.provider_name = "anthropic".into();
    app.model_name = "very-long-model-name-xyz".into();
    app.reasoning_effort = Some("high".into());
    app.approval_mode = whycodes_core::types::ApprovalMode::Manual;
    app.intent_badge = Some("plan".into());
    app.intent_kind = Some("plan".into());
    let rows = rendered_rows(&mut app, 36, 10);
    assert!(rows.iter().any(|r| r.contains("build") || r.contains("╭")));
    let tiny = rendered_rows(&mut app, 5, 4);
    assert_eq!(tiny.len(), 4);
    app.focus = crate::app::FocusPane::Scrollback;
    let home = rendered_rows(&mut app, 60, 8);
    assert!(
        home.iter()
            .any(|r| r.contains("Ask anything") || r.contains("drop images")),
        "unfocused empty home prompt must show the ask hint, got {home:?}"
    );
    app.add_message(crate::app::ChatRole::User, "hi");
    app.focus = crate::app::FocusPane::Scrollback;
    let scrolled = rendered_rows(&mut app, 60, 8);
    assert!(
        scrolled
            .iter()
            .any(|r| r.contains("Tab") || r.contains("j/k") || r.contains("prompt")),
        "scrollback-owned empty prompt must nudge focus, got {scrolled:?}"
    );
    assert_eq!(
        empty_prompt_hint(true, false, false, AppMode::Command, true),
        Some("command…")
    );
    assert!(empty_prompt_hint(false, false, false, AppMode::Normal, true).is_none());
    assert!(empty_prompt_hint(true, true, false, AppMode::Normal, true).is_none());
    assert!(empty_prompt_hint(true, false, true, AppMode::Normal, true).is_none());
}

#[test]
fn busy_empty_prompt_paints_ellipsis_prefix() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.add_message(crate::app::ChatRole::User, "hi");
    app.current_agent_state = crate::app::AgentState::Generating;
    app.input_buffer.clear();
    app.focus = crate::app::FocusPane::Prompt;
    let rows = rendered_rows(&mut app, 40, 8);
    assert!(
        rows.iter().any(|r| r.contains('…') || r.contains("...")),
        "busy empty focused prompt must paint the ellipsis prefix, got {rows:?}"
    );
}

#[test]
fn short_input_in_tall_prompt_pads_continuation_rows() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.add_message(crate::app::ChatRole::User, "hi");
    app.input_buffer = "ok".into();
    app.input_cursor = 2;
    let rows = rendered_rows(&mut app, 40, 14);
    assert!(
        rows.iter().any(|r| r.contains("ok") || r.contains('❯')),
        "typed prompt must paint, got {rows:?}"
    );
    assert!(
        rows.len() >= 8,
        "a tall prompt area must still fill its reserved rows, got {}",
        rows.len()
    );
}

#[test]
fn empty_provider_and_model_paint_placeholders() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.agent_name = "build".into();
    app.provider_name.clear();
    app.model_name.clear();
    let rows = rendered_rows(&mut app, 60, 10);
    assert!(
        rows.iter().any(|r| r.contains("build")),
        "agent name must still paint, got {rows:?}"
    );
    let short = rendered_rows(&mut app, 20, 5);
    assert_eq!(short.len(), 5);
}

#[test]
fn busy_empty_focused_prompt_uses_ellipsis_prefix() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.current_agent_state = AgentState::Generating;
    app.input_buffer.clear();
    app.focus = crate::app::FocusPane::Prompt;
    let rows = rendered_rows(&mut app, 60, 8);
    assert!(
        rows.iter().any(|r| r.contains('…') || r.contains("...")),
        "busy empty prompt must show the ellipsis prefix, got {rows:?}"
    );
}

#[test]
fn slash_command_prompt_paints_the_token() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.input_buffer = "/help me".into();
    app.input_cursor = app.input_buffer.len();
    let rows = rendered_rows(&mut app, 60, 8);
    assert!(
        rows.iter().any(|r| r.contains("/help")),
        "slash command must paint the token, got {rows:?}"
    );
}

#[test]
fn wrapped_prompt_keeps_caret_on_last_rows() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.input_buffer = "word ".repeat(80);
    app.input_cursor = app.input_buffer.len();
    let rows = rendered_rows(&mut app, 24, 12);
    assert!(
        rows.iter().any(|r| r.contains("word")),
        "long wrap must still paint prompt text, got {rows:?}"
    );
    let word_rows: Vec<&String> = rows.iter().filter(|r| r.contains("word")).collect();
    assert!(
        !word_rows.is_empty(),
        "a long wrap must still paint prompt text, got {rows:?}"
    );
    assert!(
        word_rows.iter().any(|r| r.contains("word")
            && !r.contains('❯')
            && !r.contains("{276F}")
            && !r.contains(": ")),
        "caret-at-end viewport must paint continuation rows without repeating ❯, got {rows:?}"
    );
    app.input_cursor = 0;
    let top = rendered_rows(&mut app, 24, 12);
    assert!(
        top.iter()
            .any(|r| r.contains("word") && (r.contains('❯') || r.contains("{276F}"))),
        "caret-at-start must paint the first row with ❯, got {top:?}"
    );
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
fn many_staged_images_collapse_to_summary() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    for i in 0..6 {
        app.pending_images.push(crate::images::PromptImage {
            path: format!("very-long-screenshot-name-{i}.png").into(),
            label: format!("very-long-screenshot-name-{i}.png"),
            media_type: "image/png".into(),
        });
    }
    let rows = rendered_rows(&mut app, 36, 10);
    assert!(
        rows.iter()
            .any(|r| r.contains("images") || r.contains("very-long")),
        "{rows:?}"
    );
}

#[test]
fn tiny_area_skips_paint() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    let rows = rendered_rows(&mut app, 4, 3);
    assert_eq!(rows.len(), 3);
    assert!(app.agent_hit.rect.is_none());
    // Width is enough to enter paint; two rows are consumed by the
    // outer gap so the bottom-meta Length chunk is assigned 0.
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.agent_name = "build".into();
    let short = rendered_rows(&mut app, 40, 2);
    assert_eq!(short.len(), 2);
    assert!(
        app.agent_hit.rect.is_none()
            && app.model_hit.rect.is_none()
            && app.effort_hit.rect.is_none()
            && app.approval_hit.rect.is_none(),
        "a zero-height footer must clear picker hits"
    );
}

#[test]
fn long_wrap_keeps_caret_on_last_visible_row() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.input_buffer = "word ".repeat(80);
    app.input_cursor = app.input_buffer.len();
    let rows = rendered_rows(&mut app, 40, 14);
    assert!(rows.iter().any(|r| r.contains("word")));
}

#[test]
fn intent_badge_question_and_change_paint() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.agent_name = "build".into();
    app.provider_name = "xai".into();
    app.model_name = "grok-4.6".into();
    app.intent_badge = Some("ask".into());
    app.intent_kind = Some("question".into());
    let _ = rendered_rows(&mut app, 80, 10);
    app.intent_kind = Some("change".into());
    app.intent_badge = Some("edit".into());
    let _ = rendered_rows(&mut app, 80, 10);
    app.effort_hit.hovered = true;
    app.approval_hit.hovered = true;
    let _ = rendered_rows(&mut app, 80, 10);
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

    // Narrower still: drop effort after approval (issue #45).
    let (terminal, _) = draw_prompt_footer(&mut app, 22, 16);
    let buf = terminal.backend().buffer();
    let row = footer_line_containing(buf, 22, 16, "build");
    assert!(row.contains("build"), "{row}");
    assert!(
        !row.contains("Med") && !row.contains("auto"),
        "effort drops after approval on a tighter footer: {row}"
    );
    assert!(app.effort_hit.rect.is_none(), "dropped effort has no hit");
}

#[test]
fn bottom_meta_falls_back_to_truncated_agent_on_tiny_width() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.agent_name = "build".into();
    app.provider_name = "anthropic".into();
    app.model_name = "claude-sonnet-4-5".into();
    app.intent_badge = Some("plan".into());
    app.intent_kind = Some("plan".into());
    app.reasoning_effort = Some("high".into());
    app.approval_mode = ApprovalMode::Manual;
    let rows = rendered_rows(&mut app, 16, 8);
    assert!(
        rows.iter()
            .any(|r| r.contains("build") || r.contains("bui")),
        "tiny footer must keep a truncated agent name, got {rows:?}"
    );
    let too_short = rendered_rows(&mut app, 6, 6);
    assert_eq!(too_short.len(), 6);
    assert!(app.agent_hit.rect.is_none());
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

fn cell_seq_col(buf: &ratatui::buffer::Buffer, y: u16, width: u16, needle: &str) -> Option<u16> {
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
fn command_mode_input_row_count_uses_the_command_buffer() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.mode = AppMode::Command;
    app.input_buffer = "ignored".into();
    app.command.buffer.clear();
    assert_eq!(input_row_count(&app, 80), 1);
    app.command.buffer = "word ".repeat(40);
    assert!(
        input_row_count(&app, 24) > 1,
        "a long :command draft must wrap"
    );
    assert!(prompt_height(&app, 24) > prompt_height(&TuiApp::new(TuiAppConfig::default()), 24));
    // Past MAX_INPUT_ROWS the box still paints MAX rows (padded blanks).
    app.mode = AppMode::Normal;
    app.input_buffer = "word ".repeat(200);
    app.input_cursor = 0;
    let tall = rendered_rows(&mut app, 24, 16);
    assert!(
        tall.iter().any(|r| r.contains("word") || r.contains("❯")),
        "over-wrapped prompt must still paint the first visible rows, got {tall:?}"
    );
}

#[test]
fn single_long_image_label_truncates_instead_of_summary() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.pending_images.push(crate::images::PromptImage {
        path: "very-long-screenshot-name-that-will-not-fit.png".into(),
        label: "very-long-screenshot-name-that-will-not-fit.png".into(),
        media_type: "image/png".into(),
    });
    let rows = rendered_rows(&mut app, 28, 9);
    assert!(
        rows.iter()
            .any(|r| r.contains("very-long") || r.contains("screenshot") || r.contains('…')),
        "a single long chip must truncate the label, not switch to N images, got {rows:?}"
    );
    assert!(
        rows.iter().all(|r| !r.contains("images")),
        "n==1 must not use the multi-image summary, got {rows:?}"
    );
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
