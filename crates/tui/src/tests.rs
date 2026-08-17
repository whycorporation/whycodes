use crate::app::{
    AgentState, AppMode, AuthMethod, ChatBlock, ChatRole, ConfirmAction, DialogKind, DialogManager,
    ProviderDialogMode, ProviderDialogState, SidebarState, SidebarTab, TuiApp,
};
use crate::config::TuiAppConfig;
use crate::theme::ThemeName;
use std::str::FromStr;

fn test_config() -> TuiAppConfig {
    TuiAppConfig::default()
}

// ── Theme Tests ─────────────────────────────────────────────────────

#[test]
fn test_theme_from_str_dark() {
    assert_eq!(ThemeName::from_str("dark").unwrap(), ThemeName::DefaultDark);
    assert_eq!(
        ThemeName::from_str("default-dark").unwrap(),
        ThemeName::DefaultDark
    );
    assert_eq!(
        ThemeName::from_str("default_dark").unwrap(),
        ThemeName::DefaultDark
    );
}

#[test]
fn test_theme_from_str_light() {
    assert_eq!(
        ThemeName::from_str("light").unwrap(),
        ThemeName::DefaultLight
    );
    assert_eq!(
        ThemeName::from_str("default_light").unwrap(),
        ThemeName::DefaultLight
    );
}

#[test]
fn test_theme_from_str_named_themes() {
    assert_eq!(ThemeName::from_str("monokai").unwrap(), ThemeName::Monokai);
    assert_eq!(ThemeName::from_str("nord").unwrap(), ThemeName::Nord);
    assert_eq!(ThemeName::from_str("dracula").unwrap(), ThemeName::Dracula);
    assert_eq!(ThemeName::from_str("gruvbox").unwrap(), ThemeName::Gruvbox);
    assert_eq!(
        ThemeName::from_str("catppuccin-mocha").unwrap(),
        ThemeName::CatppuccinMocha
    );
    assert_eq!(
        ThemeName::from_str("tokyonight").unwrap(),
        ThemeName::TokyoNight
    );
}

#[test]
fn test_theme_from_str_unknown_returns_err() {
    assert!(ThemeName::from_str("nonexistent_theme").is_err());
    assert!(ThemeName::from_str("").is_err());
    // Error should contain the theme name
    let err = ThemeName::from_str("nonexistent_theme").unwrap_err();
    assert!(err.name.contains("nonexistent_theme"));
}

#[test]
fn test_theme_palette_has_distinct_colors() {
    let palette = ThemeName::DefaultDark.palette();
    // bg and fg should be different
    assert_ne!(format!("{:?}", palette.bg), format!("{:?}", palette.fg));
    // error and success should be different
    assert_ne!(
        format!("{:?}", palette.error),
        format!("{:?}", palette.success)
    );
}

#[test]
fn test_agent_color_by_index_cycles() {
    let palette = ThemeName::DefaultDark.palette();
    let color0 = palette.agent_color_by_index(0);
    let color7 = palette.agent_color_by_index(7); // should wrap around
    assert_eq!(format!("{:?}", color0), format!("{:?}", color7));
}

#[test]
fn test_all_themes_produce_palette() {
    let themes = [
        ThemeName::DefaultDark,
        ThemeName::DefaultLight,
        ThemeName::Monokai,
        ThemeName::SolarizedDark,
        ThemeName::SolarizedLight,
        ThemeName::Nord,
        ThemeName::Dracula,
        ThemeName::Gruvbox,
        ThemeName::OneDark,
        ThemeName::CatppuccinMocha,
        ThemeName::CatppuccinLatte,
        ThemeName::TokyoNight,
        ThemeName::TokyoNightStorm,
        ThemeName::TokyoNightLight,
        ThemeName::Kanagawa,
        ThemeName::Everforest,
        ThemeName::RosePine,
        ThemeName::RosePineMoon,
        ThemeName::RosePineDawn,
        ThemeName::AyuDark,
        ThemeName::AyuMirage,
        ThemeName::AyuLight,
        ThemeName::GithubDark,
        ThemeName::GithubLight,
        ThemeName::VscodeDark,
        ThemeName::VscodeLight,
        ThemeName::Zenburn,
        ThemeName::OceanicNext,
        ThemeName::MaterialPalenight,
    ];
    assert_eq!(
        themes.len(),
        ThemeName::ALL.len(),
        "ThemeName::ALL is missing a variant"
    );
    for theme in ThemeName::ALL {
        theme.palette(); // should not panic
    }
}

#[test]
fn syntax_theme_follows_grok_mapping() {
    use whycode_format::highlight::SyntaxTheme;
    assert_eq!(
        ThemeName::DefaultDark.syntax_theme(),
        SyntaxTheme::GrokNight
    );
    assert_eq!(ThemeName::DefaultLight.syntax_theme(), SyntaxTheme::GrokDay);
    assert_eq!(
        ThemeName::TokyoNight.syntax_theme(),
        SyntaxTheme::TokyoNight
    );
    assert_eq!(
        ThemeName::RosePineMoon.syntax_theme(),
        SyntaxTheme::GrokNight
    );
}

#[test]
fn test_theme_name_round_trips_through_from_str() {
    for theme in ThemeName::ALL {
        assert_eq!(
            ThemeName::from_str(theme.name()).unwrap(),
            *theme,
            "canonical name {:?} does not parse back",
            theme.name()
        );
    }
}

// ── App Tests ───────────────────────────────────────────────────────

#[test]
fn test_tui_app_new() {
    let app = TuiApp::new(test_config());
    assert!(app.running);
    assert_eq!(app.mode, AppMode::Normal);
    assert!(app.messages.is_empty());
    assert_eq!(app.agent_name, "build");
    assert_eq!(app.agent_cycle_idx, 0);
    assert!(app.pending_prompt.is_none());
}

#[test]
fn test_chat_role_as_str() {
    assert_eq!(ChatRole::User.as_str(), "user");
    assert_eq!(ChatRole::Assistant.as_str(), "assistant");
    assert_eq!(ChatRole::System.as_str(), "system");
    assert_eq!(ChatRole::Tool.as_str(), "tool");
}

#[test]
fn test_chat_role_display() {
    assert_eq!(format!("{}", ChatRole::User), "user");
    assert_eq!(format!("{}", ChatRole::Assistant), "assistant");
}

#[test]
fn test_add_message() {
    let mut app = TuiApp::new(test_config());
    app.add_message(ChatRole::User, "hello");
    assert_eq!(app.messages.len(), 1);
    assert_eq!(app.messages[0].role, ChatRole::User);
    assert_eq!(app.messages[0].content, "hello");
}

#[test]
fn test_append_to_last() {
    let mut app = TuiApp::new(test_config());
    app.add_message(ChatRole::Assistant, "first");
    app.append_to_last(" second");
    assert_eq!(app.messages.len(), 1);
    assert_eq!(app.messages[0].content, "first second");
}

#[test]
fn test_append_to_last_creates_new_when_needed() {
    let mut app = TuiApp::new(test_config());
    app.append_to_last("new message");
    assert_eq!(app.messages.len(), 1);
    assert_eq!(app.messages[0].role, ChatRole::Assistant);
}

#[test]
fn test_append_thinking() {
    let mut app = TuiApp::new(test_config());
    app.append_thinking("thinking...");
    assert_eq!(app.messages.len(), 1);
    assert_eq!(app.messages[0].role, ChatRole::Assistant);
    assert!(!app.messages[0].blocks.is_empty());
    match &app.messages[0].blocks[0] {
        ChatBlock::Thinking(t) => {
            assert!(t.is_running());
            assert!(t.collapsed);
            assert_eq!(t.text, "thinking...");
        }
        _ => panic!("expected Thinking block"),
    }
}

#[test]
fn test_thinking_lifecycle_finish_and_elapsed() {
    use crate::app::{ThinkingBlock, format_thinking_elapsed};
    let mut app = TuiApp::new(test_config());
    app.append_thinking("step 1\n");
    app.append_thinking("step 2\n");
    assert_eq!(app.messages[0].blocks.len(), 1);
    match &app.messages[0].blocks[0] {
        ChatBlock::Thinking(t) => assert!(t.is_running()),
        _ => panic!("expected Thinking"),
    }

    app.finish_open_thinking();
    match &app.messages[0].blocks[0] {
        ChatBlock::Thinking(t) => {
            assert!(!t.is_running());
            assert!(t.header_label().starts_with("Thought for "));
            assert!(t.collapsed);
            assert!(!t.show_body());
        }
        _ => panic!("expected Thinking"),
    }

    // Text after thinking starts a separate content path and keeps thinking finished.
    app.append_to_last("answer");
    assert_eq!(app.messages[0].content, "answer");
    assert_eq!(app.messages[0].blocks.len(), 1);

    assert_eq!(format_thinking_elapsed(1400), "1.4s");
    assert_eq!(format_thinking_elapsed(12_000), "12s");
    assert_eq!(format_thinking_elapsed(72_000), "1m12s");
    assert_eq!(crate::app::format_elapsed_ms(450), "0.5s");

    let mut tb = ThinkingBlock::new("a\nb\nc");
    tb.finish();
    tb.collapsed = false;
    assert_eq!(tb.body_lines().len(), 3);
}

#[test]
fn test_turn_timing_stamps_assistant_duration() {
    use crate::app::format_elapsed_ms;
    use std::thread;
    use std::time::Duration;

    let mut app = TuiApp::new(test_config());
    app.add_message(ChatRole::User, "hi");
    app.add_message(ChatRole::Assistant, "");
    app.mark_turn_started();
    assert!(app.turn_elapsed_ms().is_some());
    thread::sleep(Duration::from_millis(20));
    let ms = app.complete_turn_timing().expect("elapsed");
    assert!(ms >= 15, "expected at least ~20ms, got {ms}");
    assert!(app.turn_started_at.is_none());
    assert_eq!(app.messages.last().and_then(|m| m.duration_ms), Some(ms));
    // Human format is non-empty for any positive duration.
    assert!(!format_elapsed_ms(ms).is_empty());
}

#[test]
fn test_complete_turn_timing_ms_uses_reported_work_not_wall() {
    // Title refine (or any post-turn work) must not inflate the footer.
    let mut app = TuiApp::new(test_config());
    app.add_message(ChatRole::User, "hi");
    app.add_message(ChatRole::Assistant, "yo");
    app.mark_turn_started();
    // Pretend wall clock ran long (title refine), but work was 2.5s.
    let stamped = app.complete_turn_timing_ms(2_500);
    assert_eq!(stamped, 2_500);
    assert!(app.turn_started_at.is_none());
    assert_eq!(app.messages.last().and_then(|m| m.duration_ms), Some(2_500));
    assert_eq!(crate::app::format_elapsed_ms(2_500), "2.5s");
}

#[test]
fn test_format_usage_short() {
    use crate::app::format_usage_short;
    use whycode_core::types::Usage;

    let u = Usage {
        input_tokens: 1200,
        output_tokens: 340,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: Some(500),
    };
    let s = format_usage_short(&u);
    assert!(s.contains("in"), "{s}");
    assert!(s.contains("out"), "{s}");
    assert!(s.contains("cached"), "{s}");
}

#[test]
fn test_format_context_usage_and_percent() {
    use crate::app::{
        context_tokens_from_usage, fmt_pct5, format_context_hover, format_context_percent,
        format_context_usage,
    };
    use unicode_width::UnicodeWidthStr;
    use whycode_core::types::Usage;

    assert_eq!(format_context_usage(1_200, 200_000), "1.2k / 200k");
    assert_eq!(format_context_usage(12_400, 128_000), "12.4k / 128k");
    // Grok fmt_pct5: fixed 5 chars.
    assert_eq!(fmt_pct5(0.0), "0.00%");
    assert_eq!(fmt_pct5(1.0), "1.00%");
    assert_eq!(fmt_pct5(42.0), "42.0%");
    assert_eq!(fmt_pct5(100.0), "MAX %");
    assert_eq!(format_context_percent(1_200, 200_000), "0.60%");
    assert_eq!(format_context_percent(200_000, 200_000), "MAX %");

    // Hover width matches default (no layout shift) — Grok invariant.
    for (used, max) in [(1_200u64, 200_000u64), (0, 9), (84_000, 200_000)] {
        let idle = format_context_usage(used, max);
        let hover = format_context_hover(used, max);
        assert_eq!(
            idle.width().max(6),
            hover.width(),
            "idle={idle:?} hover={hover:?}"
        );
    }
    let hover = format_context_hover(84_000, 200_000);
    assert!(hover.contains("42.0%"), "hover={hover:?}");
    assert!(
        hover.contains('█') || hover.contains('░') || hover.contains('▌'),
        "hover={hover:?}"
    );

    let u = Usage {
        input_tokens: 1000,
        output_tokens: 50,
        cache_creation_input_tokens: Some(100),
        cache_read_input_tokens: Some(400),
    };
    // Context fill counts prompt-side tokens, not completion output.
    assert_eq!(context_tokens_from_usage(&u), 1500);
}

/// Sticky hover (Grok HitArea): enter/leave flips flag + dirty; paint must
/// read the sticky flag, not recompute after clearing context_hit.
#[test]
fn context_meter_hover_marks_dirty_on_enter_leave() {
    use crossterm::event::{Event, KeyModifiers, MouseEvent, MouseEventKind};
    use ratatui::layout::Rect;

    let mut app = TuiApp::new(test_config());
    // Simulate a painted footer hit-box (right side of row 40).
    app.context_hit.set_rect(Some(Rect {
        x: 70,
        y: 40,
        width: 12,
        height: 1,
    }));
    app.needs_redraw = false;
    assert!(!app.context_hovered());

    // Move onto the meter → sticky hover + dirty.
    let enter = Event::Mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column: 75,
        row: 40,
        modifiers: KeyModifiers::NONE,
    });
    assert!(crate::input::handle_event(&mut app, enter));
    assert!(app.context_hovered());
    assert!(app.needs_redraw, "enter context meter must mark_dirty");

    // Simulate paint clearing the rect only (sticky hovered must survive).
    app.context_hit.set_rect(None);
    assert!(
        app.context_hovered(),
        "sticky hover must survive paint clearing context_hit.rect"
    );

    // Restore hit for leave hit-test (next frame would set it after paint).
    app.context_hit.set_rect(Some(Rect {
        x: 70,
        y: 40,
        width: 12,
        height: 1,
    }));
    app.needs_redraw = false;
    let stay = Event::Mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column: 78,
        row: 40,
        modifiers: KeyModifiers::NONE,
    });
    assert!(crate::input::handle_event(&mut app, stay));
    assert!(app.context_hovered());
    assert!(
        !app.needs_redraw,
        "move inside meter should not re-dirty every pixel"
    );

    // Leave the meter → sticky clear + dirty so paint restores used/max.
    let leave = Event::Mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column: 10,
        row: 40,
        modifiers: KeyModifiers::NONE,
    });
    assert!(crate::input::handle_event(&mut app, leave));
    assert!(!app.context_hovered());
    assert!(app.needs_redraw, "leave context meter must mark_dirty");
}

#[test]
fn turn_stop_click_sets_pending_cancel() {
    use crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::layout::Rect;

    let mut app = TuiApp::new(test_config());
    app.current_agent_state = crate::app::AgentState::Generating;
    app.turn_stop_hit.set_rect(Some(Rect {
        x: 70,
        y: 20,
        width: 6,
        height: 1,
    }));
    let click = Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 72,
        row: 20,
        modifiers: KeyModifiers::NONE,
    });
    assert!(crate::input::handle_event(&mut app, click));
    assert!(app.pending_cancel);
}

#[test]
fn test_thinking_new_block_after_tool() {
    let mut app = TuiApp::new(test_config());
    app.append_thinking("before tool");
    app.add_tool_call("t1".into(), "bash".into(), serde_json::json!({}));
    app.append_thinking("after tool");
    let thinkings: Vec<_> = app.messages[0]
        .blocks
        .iter()
        .filter(|b| matches!(b, ChatBlock::Thinking(_)))
        .collect();
    assert_eq!(thinkings.len(), 2);
    // First block was finished by add_tool_call.
    match &app.messages[0].blocks[0] {
        ChatBlock::Thinking(t) => assert!(!t.is_running()),
        _ => panic!("first should be thinking"),
    }
    match app.messages[0].blocks.last() {
        Some(ChatBlock::Thinking(t)) => {
            assert!(t.is_running());
            assert_eq!(t.text, "after tool");
        }
        _ => panic!("last should be open thinking"),
    }
}

#[test]
fn test_toggle_selected_thinking() {
    let mut app = TuiApp::new(test_config());
    app.append_thinking("line1\nline2");
    app.finish_open_thinking();
    app.selected_msg = Some(0);
    // Seed a layout cache so expand must clear it (Grok expand reliability).
    app.messages[0].layout_cache = Some((80, false, 3));
    assert!(matches!(
        &app.messages[0].blocks[0],
        ChatBlock::Thinking(t) if t.collapsed
    ));
    app.toggle_selected_thinking();
    assert!(matches!(
        &app.messages[0].blocks[0],
        ChatBlock::Thinking(t) if !t.collapsed && t.show_body()
    ));
    assert!(
        app.messages[0].layout_cache.is_none(),
        "expand must invalidate message layout cache"
    );
    assert!(app.needs_redraw, "expand must request a redraw frame");
}

#[test]
fn test_thinking_live_tail_truncates() {
    use crate::app::{THINKING_LIVE_TAIL_LINES, ThinkingBlock};
    let mut text = String::new();
    for i in 0..(THINKING_LIVE_TAIL_LINES + 5) {
        text.push_str(&format!("line {i}\n"));
    }
    let t = ThinkingBlock::new(text);
    assert!(t.is_truncated_live());
    assert_eq!(t.body_lines().len(), THINKING_LIVE_TAIL_LINES);
}

#[test]
fn test_thinking_push_delta_true_deltas() {
    use crate::app::ThinkingBlock;
    let mut t = ThinkingBlock::new("Hello");
    t.push_delta(" world");
    t.push_delta("!");
    assert_eq!(t.text, "Hello world!");
}

#[test]
fn test_thinking_push_delta_full_snapshot_no_dup() {
    // Some gateways resend the full reasoning so far each chunk.
    use crate::app::ThinkingBlock;
    let mut t = ThinkingBlock::new("ab");
    t.push_delta("abc");
    t.push_delta("abcd");
    t.push_delta("abcd"); // exact re-send
    assert_eq!(t.text, "abcd");
}

#[test]
fn test_thinking_ignores_empty_and_whitespace() {
    let mut app = TuiApp::new(test_config());
    app.append_thinking("   \n\t  ");
    assert!(app.messages.is_empty(), "whitespace must not open a block");
    app.append_thinking("real");
    app.append_thinking("");
    app.append_thinking("\n");
    match &app.messages[0].blocks[0] {
        ChatBlock::Thinking(t) => assert_eq!(t.text, "real"),
        _ => panic!("expected Thinking"),
    }
}

#[test]
fn test_thinking_cap_does_not_grow_unbounded() {
    use crate::app::{THINKING_MAX_CHARS, ThinkingBlock};
    let mut t = ThinkingBlock::new("");
    let chunk = "x".repeat(8 * 1024);
    for _ in 0..32 {
        t.push_delta(&chunk);
    }
    assert!(
        t.text.len() <= THINKING_MAX_CHARS + 4,
        "len={} cap={}",
        t.text.len(),
        THINKING_MAX_CHARS
    );
}

#[test]
fn test_thinking_expanded_line_cap() {
    use crate::app::{THINKING_EXPANDED_MAX_LINES, ThinkingBlock};
    let mut body = String::new();
    for i in 0..(THINKING_EXPANDED_MAX_LINES + 50) {
        body.push_str(&format!("L{i}\n"));
    }
    let mut t = ThinkingBlock::new(body);
    t.finish();
    t.collapsed = false;
    assert!(t.is_truncated_expanded());
    assert_eq!(t.body_lines().len(), THINKING_EXPANDED_MAX_LINES);
}

#[test]
fn test_add_tool_call() {
    let mut app = TuiApp::new(test_config());
    app.add_tool_call(
        "tc-1".to_string(),
        "bash".to_string(),
        serde_json::json!({"command": "echo hi"}),
    );
    assert_eq!(app.messages.len(), 1);
    assert_eq!(app.messages[0].tool_calls.len(), 1);
    assert_eq!(app.messages[0].tool_calls[0].name, "bash");
}

#[test]
fn test_add_tool_result() {
    let mut app = TuiApp::new(test_config());
    app.add_tool_call(
        "tc-1".to_string(),
        "bash".to_string(),
        serde_json::json!({}),
    );
    app.add_tool_result("tc-1", "output here", false);
    assert_eq!(
        app.messages[0].tool_calls[0].result.as_deref(),
        Some("output here")
    );
    assert!(!app.messages[0].tool_calls[0].is_error);
}

#[test]
fn test_submit_input() {
    let mut app = TuiApp::new(test_config());
    app.input_buffer = "hello world".to_string();
    app.submit_input();
    assert!(app.pending_prompt.is_some());
    assert_eq!(app.pending_prompt.unwrap(), "hello world");
    assert!(app.input_buffer.is_empty());
    assert_eq!(app.messages.len(), 1);
    assert_eq!(app.messages[0].role, ChatRole::User);
}

#[test]
fn test_submit_input_skips_slash_commands() {
    let mut app = TuiApp::new(test_config());
    app.input_buffer = "/help".to_string();
    app.submit_input();
    assert!(app.pending_prompt.is_none());
    assert!(app.messages.is_empty());
}

#[test]
fn test_submit_input_skips_empty() {
    let mut app = TuiApp::new(test_config());
    app.input_buffer = "   ".to_string();
    app.submit_input();
    assert!(app.pending_prompt.is_none());
}

#[test]
fn test_submit_input_with_images_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shot.png");
    std::fs::write(&path, b"\x89PNG").unwrap();

    let mut app = TuiApp::new(test_config());
    app.attach_image(&path).unwrap();
    assert_eq!(app.pending_images.len(), 1);
    app.submit_input();
    assert_eq!(app.pending_prompt.as_deref(), Some(""));
    assert!(app.pending_images.is_empty());
    assert_eq!(app.pending_submit_images.len(), 1);
    assert_eq!(app.messages.len(), 1);
    assert_eq!(app.messages[0].image_labels, vec!["shot.png".to_string()]);
}

#[test]
fn test_paste_image_path_attaches() {
    use crossterm::event::Event;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ui.jpg");
    std::fs::write(&path, b"jpeg").unwrap();

    let mut app = TuiApp::new(test_config());
    let paste = Event::Paste(path.to_string_lossy().to_string());
    assert!(crate::input::handle_event(&mut app, paste));
    assert_eq!(app.pending_images.len(), 1);
    assert!(app.input_buffer.is_empty());
}

#[test]
fn test_long_paste_collapses_to_placeholder() {
    use crossterm::event::Event;

    let mut app = TuiApp::new(test_config());
    let body = "line one\nline two\nline three\nline four";
    assert!(crate::input::handle_event(
        &mut app,
        Event::Paste(body.to_string())
    ));
    assert_eq!(app.pending_pastes.len(), 1);
    assert_eq!(app.pending_pastes[0].content, body);
    assert!(
        app.input_buffer.contains("[pasted #"),
        "buffer should show collapsed token, got {:?}",
        app.input_buffer
    );
    assert!(
        app.input_buffer.contains("lines]"),
        "token should mention lines: {:?}",
        app.input_buffer
    );
    // Prompt stays a single visual row (no multi-line reflow / flicker).
    assert_eq!(crate::ui::prompt::input_row_count(&app, 80), 1);
}

#[test]
fn test_short_paste_stays_inline() {
    use crossterm::event::Event;

    let mut app = TuiApp::new(test_config());
    assert!(crate::input::handle_event(
        &mut app,
        Event::Paste("hello world".into())
    ));
    assert!(app.pending_pastes.is_empty());
    assert_eq!(app.input_buffer, "hello world");
}

#[test]
fn test_submit_expands_collapsed_paste() {
    let mut app = TuiApp::new(test_config());
    let body = "a\nb\nc\nd";
    app.insert_paste_text(body);
    assert!(app.input_buffer.contains("[pasted #"));
    app.submit_input();
    assert_eq!(app.pending_prompt.as_deref(), Some(body));
    // Chat bubble keeps the compact form.
    assert!(
        app.messages
            .last()
            .map(|m| m.content.contains("[pasted #"))
            .unwrap_or(false)
    );
    assert!(app.pending_pastes.is_empty());
    assert!(app.input_buffer.is_empty());
}

#[test]
fn test_backspace_deletes_whole_paste_token() {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    let mut app = TuiApp::new(test_config());
    app.insert_paste_text("one\ntwo\nthree\nfour");
    assert!(!app.pending_pastes.is_empty());
    let token_len = app.input_buffer.len();
    app.input_cursor = token_len;
    let backspace = Event::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    assert!(crate::input::handle_event(&mut app, backspace));
    assert!(
        app.input_buffer.is_empty(),
        "whole token should go, got {:?}",
        app.input_buffer
    );
    assert!(app.pending_pastes.is_empty());
}

#[test]
fn test_backspace_removes_last_image() {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.webp");
    std::fs::write(&path, b"w").unwrap();

    let mut app = TuiApp::new(test_config());
    app.attach_image(&path).unwrap();
    let backspace = Event::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    assert!(crate::input::handle_event(&mut app, backspace));
    assert!(app.pending_images.is_empty());
}

// ── Dialog Manager Tests ────────────────────────────────────────────

#[test]
fn test_dialog_manager_new() {
    let dm = DialogManager::new();
    assert!(!dm.is_open());
    assert!(dm.active().is_none());
}

#[test]
fn test_dialog_manager_push_pop() {
    let mut dm = DialogManager::new();
    dm.push(DialogKind::Help);
    assert!(dm.is_open());
    assert!(dm.active().is_some());

    let popped = dm.pop();
    assert!(popped.is_some());
    assert!(!dm.is_open());
}

#[test]
fn test_dialog_manager_clear() {
    let mut dm = DialogManager::new();
    dm.push(DialogKind::Help);
    dm.push(DialogKind::Theme);
    dm.clear();
    assert!(!dm.is_open());
}

// ── TuiApp Dialog Methods ───────────────────────────────────────────

#[test]
fn test_alert() {
    let mut app = TuiApp::new(test_config());
    app.alert("Title", "Message");
    assert_eq!(app.mode, AppMode::Dialog);
    assert!(app.dialogs.is_open());
}

#[test]
fn test_confirm() {
    let mut app = TuiApp::new(test_config());
    app.confirm("Sure?", "Do it?", ConfirmAction::Quit);
    assert_eq!(app.mode, AppMode::Dialog);
    assert!(app.dialogs.is_open());
}

#[test]
fn test_ask_question_opens_dialog() {
    use crate::app::{AgentState, DialogKind};
    use whycode_tools::question::{QuestionOption, QuestionSpec};

    let mut app = TuiApp::new(test_config());
    app.ask_question(vec![QuestionSpec {
        prompt: "Pick?".into(),
        options: vec![
            QuestionOption {
                label: "A".into(),
                description: "first".into(),
                preview: None,
            },
            QuestionOption {
                label: "B".into(),
                description: String::new(),
                preview: None,
            },
        ],
        multi_select: false,
    }]);
    assert_eq!(app.current_agent_state, AgentState::WaitingForQuestion);
    assert!(matches!(
        app.dialogs.active(),
        Some(DialogKind::Question(_))
    ));
}

#[test]
fn test_question_dialog_confirm_single() {
    use crate::app::QuestionDialogState;
    use whycode_tools::question::{QuestionOption, QuestionSpec};

    let mut st = QuestionDialogState::new(vec![QuestionSpec {
        prompt: "Go?".into(),
        options: vec![
            QuestionOption {
                label: "Yes".into(),
                description: String::new(),
                preview: None,
            },
            QuestionOption {
                label: "No".into(),
                description: String::new(),
                preview: None,
            },
        ],
        multi_select: false,
    }]);
    st.cursor = 0;
    let done = st.confirm_current().expect("should finish one question");
    assert_eq!(done[0].selected, vec!["Yes".to_string()]);
}

#[test]
fn test_question_navigate_prev_next_and_copy() {
    use crate::app::QuestionDialogState;
    use whycode_tools::question::{QuestionOption, QuestionSpec};

    let opt = |label: &str| QuestionOption {
        label: label.into(),
        description: format!("desc {label}"),
        preview: None,
    };
    let mut st = QuestionDialogState::new(vec![
        QuestionSpec {
            prompt: "Backend?".into(),
            options: vec![opt("SQLite"), opt("Postgres")],
            multi_select: false,
        },
        QuestionSpec {
            prompt: "Deploy?".into(),
            options: vec![opt("Local"), opt("Cloud")],
            multi_select: false,
        },
    ]);

    // Cannot skip forward before answering.
    assert!(!st.go_next_question());

    st.cursor = 0; // SQLite
    assert!(st.confirm_current().is_none()); // advances to q2
    assert_eq!(st.index, 1);
    assert_eq!(st.answers[0].as_ref().unwrap().selected, vec!["SQLite"]);

    // Back to first question; rehydrates selection.
    assert!(st.go_prev_question());
    assert_eq!(st.index, 0);
    assert_eq!(st.cursor, 0);

    // Forward again after answer exists.
    assert!(st.go_next_question());
    assert_eq!(st.index, 1);

    let clip = st.clipboard_text();
    assert!(clip.contains("Backend?"), "{clip}");
    assert!(clip.contains("SQLite"), "{clip}");
    assert!(clip.contains("Deploy?"), "{clip}");
    assert!(clip.contains("Answer: SQLite"), "{clip}");

    st.cursor = 1; // Cloud
    let done = st.confirm_current().expect("both answered");
    assert_eq!(done.len(), 2);
    assert_eq!(done[1].selected, vec!["Cloud".to_string()]);
}

#[test]
fn test_bottom_rect_docks_to_bottom() {
    use crate::ui::dialogs::bottom_rect;
    use ratatui::layout::Rect;

    let screen = Rect {
        x: 0,
        y: 0,
        width: 100,
        height: 40,
    };
    let r = bottom_rect(80, 50, screen);
    assert_eq!(r.height, 20);
    assert_eq!(r.y, 20, "panel must sit on bottom half");
    assert!(r.x >= 10);
    assert_eq!(r.width, 80);
}

#[test]
fn test_ask_permission() {
    let mut app = TuiApp::new(test_config());
    app.ask_permission("bash", "echo test");
    assert_eq!(app.mode, AppMode::Dialog);
    assert_eq!(app.current_agent_state, AgentState::WaitingForPermission);
}

// ── Sidebar Tests ───────────────────────────────────────────────────

#[test]
fn test_sidebar_default() {
    let sidebar = SidebarState::default();
    assert!(!sidebar.visible);
    assert_eq!(sidebar.active_tab, SidebarTab::Files);
    assert!(sidebar.file_tree.is_empty());
    assert_eq!(sidebar.diagnostics, 0);
}

#[test]
fn sidebar_tabs_cycle() {
    let all = SidebarTab::ALL;
    assert!(all.len() >= 2);
    for (i, tab) in all.iter().copied().enumerate() {
        assert_eq!(tab.next(), all[(i + 1) % all.len()]);
        assert_eq!(tab.prev(), all[(i + all.len() - 1) % all.len()]);
    }
}

#[test]
fn panel_event_opens_preview() {
    let mut app = TuiApp::new(test_config());
    assert!(!app.sidebar.visible);
    crate::run::apply_panel_update(
        &mut app,
        whycode_core::PanelUpdate::File {
            path: "src/main.rs".into(),
            text: "fn main() {}".into(),
        },
    );
    assert!(app.sidebar.visible);
    assert_eq!(app.sidebar.active_tab, SidebarTab::Preview);
    match &app.sidebar.preview {
        crate::app::SidebarPreview::File { path, text } => {
            assert_eq!(path, "src/main.rs");
            assert!(text.contains("main"));
        }
        other => panic!("unexpected {other:?}"),
    }
}

// ── Provider Dialog Tests ───────────────────────────────────────────

#[test]
fn test_provider_dialog_default() {
    let pd = ProviderDialogState::default();
    assert_eq!(pd.mode, ProviderDialogMode::Select);
    assert_eq!(pd.selected, 0);
    assert!(pd.providers.is_empty());
    assert_eq!(pd.active_field, 0);
    assert!(pd.error.is_none());
    assert!(!pd.saved);
    assert_eq!(pd.form_auth_method, AuthMethod::ApiKey);
}

// ── List dialog navigation ──────────────────────────────────────────

#[test]
fn selection_moves_and_wraps_at_both_ends() {
    use crate::app::move_selection;
    assert_eq!(move_selection(0, 3, 1), 1);
    assert_eq!(move_selection(2, 3, 1), 0, "should wrap forward");
    assert_eq!(move_selection(0, 3, -1), 2, "should wrap backward");
    assert_eq!(move_selection(1, 3, -1), 0);
}

#[test]
fn selection_on_an_empty_list_stays_at_zero() {
    use crate::app::move_selection;
    assert_eq!(move_selection(0, 0, 1), 0);
    assert_eq!(move_selection(0, 0, -1), 0);
    assert_eq!(move_selection(5, 0, 1), 0);
}

#[test]
fn a_single_item_list_stays_put() {
    use crate::app::move_selection;
    assert_eq!(move_selection(0, 1, 1), 0);
    assert_eq!(move_selection(0, 1, -1), 0);
}

#[test]
fn opening_a_dialog_switches_mode_and_key_context() {
    use crate::app::DialogKind;
    let mut app = TuiApp::new(test_config());
    crate::input::open_dialog(&mut app, DialogKind::SessionList);

    assert_eq!(app.mode, AppMode::Dialog);
    assert!(app.dialogs.is_open());
    assert!(matches!(
        app.dialogs.active(),
        Some(DialogKind::SessionList)
    ));
}

#[test]
fn the_session_list_starts_empty() {
    let app = TuiApp::new(test_config());
    assert!(app.session_list.sessions.is_empty());
    assert_eq!(app.session_list.selected, 0);
}

#[test]
fn dialog_close_hit_and_list_index_from_paint_meta() {
    use ratatui::layout::Rect;
    let mut app = TuiApp::new(test_config());
    app.apply_select_paint(
        Some(Rect {
            x: 50,
            y: 5,
            width: 5,
            height: 1,
        }),
        Some(Rect {
            x: 10,
            y: 8,
            width: 40,
            height: 10,
        }),
        Some(Rect {
            x: 50,
            y: 8,
            width: 1,
            height: 10,
        }),
        3, // scroll_start
        10,
        25, // total
        Some(Rect {
            x: 8,
            y: 4,
            width: 50,
            height: 16,
        }),
    );
    assert!(app.dialog_scrollbar_contains(50, 12));
    assert!(!app.dialog_scrollbar_contains(49, 12));
    assert!(app.dialog_close_contains(52, 5));
    assert!(!app.dialog_close_contains(52, 6));
    // Row 0 of viewport → absolute index 3
    assert_eq!(app.dialog_list_index_at(12, 8), Some(3));
    // Row 2 → 5
    assert_eq!(app.dialog_list_index_at(12, 10), Some(5));
    // Outside list
    assert_eq!(app.dialog_list_index_at(12, 7), None);
    // Past total
    assert_eq!(app.dialog_list_index_at(12, 8 + 22), None);
}

#[test]
fn session_list_selection_identifies_entry_for_resume() {
    use crate::app::{DialogKind, SessionEntry};
    use crate::input::open_dialog;

    let mut app = TuiApp::new(test_config());
    app.session_list.sessions = vec![
        SessionEntry {
            id: "aaa-111".into(),
            title: "First".into(),
            messages: 2,
            updated_at: None,
            live: None,
        },
        SessionEntry {
            id: "bbb-222".into(),
            title: "Second".into(),
            messages: 5,
            updated_at: None,
            live: None,
        },
    ];
    app.session_list.selected = 1;
    open_dialog(&mut app, DialogKind::SessionList);
    // Mirrors confirm_dialog(SessionList): queue the selected id for the run loop.
    if let Some(entry) = app.session_list.sessions.get(app.session_list.selected) {
        app.pending_session_id = Some(entry.id.clone());
    }
    assert_eq!(app.pending_session_id.as_deref(), Some("bbb-222"));
}

#[test]
fn load_messages_from_session_restores_user_and_assistant() {
    use crate::app::{ChatRole, chat_messages_from_session};
    use whycode_core::types::ContentBlock;
    use whycode_session::session::Session;

    let mut session = Session::new(std::path::PathBuf::from("/proj"), "sys".into());
    session.add_user_message("hello");
    session.add_assistant_message(vec![
        ContentBlock::Text {
            text: "hi there".into(),
        },
        ContentBlock::ToolUse {
            id: "t1".into(),
            name: "read".into(),
            input: serde_json::json!({"path": "a.rs"}),
        },
    ]);
    session.add_tool_results(vec![whycode_core::types::ToolResult {
        tool_call_id: "t1".into(),
        content: "fn main() {}".into(),
        is_error: false,
    }]);

    let msgs = chat_messages_from_session(&session);
    assert_eq!(msgs.len(), 2, "tool result should fold into assistant");
    assert_eq!(msgs[0].role, ChatRole::User);
    assert_eq!(msgs[0].content, "hello");
    assert!(
        msgs[0].created_at.is_some(),
        "resumed user bubble should keep the session timestamp"
    );
    assert_eq!(msgs[1].role, ChatRole::Assistant);
    assert!(msgs[1].content.contains("hi there"));
    assert_eq!(msgs[1].tool_calls.len(), 1);
    assert_eq!(msgs[1].tool_calls[0].name, "read");
    assert_eq!(
        msgs[1].tool_calls[0].result.as_deref(),
        Some("fn main() {}")
    );

    let mut app = TuiApp::new(test_config());
    app.load_messages_from_session(&session);
    assert_eq!(app.messages.len(), 2);
    assert_eq!(app.session_title, session.title);
}

#[test]
fn load_messages_uses_transcript_estimate_not_session_usage() {
    use whycode_core::types::Usage;
    use whycode_session::session::Session;

    let mut session = Session::new(std::path::PathBuf::from("/proj"), "sys".into());
    session.add_user_message("hello world, this is a short prompt");
    // Cumulative billed usage across many turns — must NOT drive the context meter.
    session.add_usage(&Usage {
        input_tokens: 500_000,
        output_tokens: 80_000,
        cache_creation_input_tokens: Some(10_000),
        cache_read_input_tokens: Some(200_000),
    });

    let mut app = TuiApp::new(test_config());
    app.load_messages_from_session(&session);

    let estimate = session.token_count() as u64;
    assert_eq!(
        app.context_used, estimate,
        "resume must use transcript size, not session.usage totals"
    );
    assert!(
        app.context_used < 10_000,
        "estimate should be tiny for a short session, got {}",
        app.context_used
    );
    assert!(
        app.turn_usage.is_none(),
        "last-turn usage is unknown after resume"
    );
}

#[test]
fn context_tokens_from_usage_is_prompt_side_only() {
    use crate::app::context_tokens_from_usage;
    use whycode_core::types::Usage;

    // Anthropic-style: cache fields additive with input.
    let anthropic = Usage {
        input_tokens: 100,
        output_tokens: 50,
        cache_creation_input_tokens: Some(20),
        cache_read_input_tokens: Some(400),
    };
    assert_eq!(context_tokens_from_usage(&anthropic), 520);

    // OpenAI-style after our mapping: only prompt_tokens (no cache fields).
    let openai = Usage {
        input_tokens: 1500,
        output_tokens: 12,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    };
    assert_eq!(context_tokens_from_usage(&openai), 1500);
}

// ── Slash Suggest Tests ─────────────────────────────────────────────

#[test]
fn slash_opens_the_popup_and_lists_every_command() {
    use crate::app::BUILTIN_SLASH_COMMANDS;
    let mut app = TuiApp::new(test_config());
    app.input_buffer = "/".to_string();
    app.slash_suggest.refresh(&app.input_buffer);
    assert!(app.slash_suggest.active);
    assert_eq!(
        app.slash_suggest.matches.len(),
        BUILTIN_SLASH_COMMANDS.len()
    );
}

#[test]
fn typing_narrows_the_match_list() {
    let mut app = TuiApp::new(test_config());
    app.input_buffer = "/mo".to_string();
    app.slash_suggest.refresh(&app.input_buffer);
    let names: Vec<&str> = app
        .slash_suggest
        .matches
        .iter()
        .map(|&i| crate::app::BUILTIN_SLASH_COMMANDS[i].name)
        .collect();
    assert!(names.contains(&"/models"));
    assert!(!names.contains(&"/help"));
}

#[test]
fn a_space_or_no_leading_slash_closes_the_popup() {
    let mut app = TuiApp::new(test_config());
    app.input_buffer = "/models anthropic".to_string();
    app.slash_suggest.refresh(&app.input_buffer);
    assert!(!app.slash_suggest.active);

    app.input_buffer = "hello".to_string();
    app.slash_suggest.refresh(&app.input_buffer);
    assert!(!app.slash_suggest.active);
}

#[test]
fn up_and_down_keys_move_the_selection_in_visual_order() {
    use crate::app::BUILTIN_SLASH_COMMANDS;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut app = TuiApp::new(test_config());
    app.input_buffer = "/".to_string();
    app.slash_suggest.refresh(&app.input_buffer);
    assert!(app.slash_suggest.active);
    assert_eq!(app.slash_suggest.selected, 0);

    let down = || KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
    let up = || KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);

    crate::input::handle_event(&mut app, crossterm::event::Event::Key(down()));
    assert_eq!(
        app.slash_suggest.selected, 1,
        "Down must move down the list"
    );
    crate::input::handle_event(&mut app, crossterm::event::Event::Key(down()));
    assert_eq!(app.slash_suggest.selected, 2);
    crate::input::handle_event(&mut app, crossterm::event::Event::Key(up()));
    assert_eq!(app.slash_suggest.selected, 1, "Up must move back up");

    app.slash_suggest.selected = 0;
    crate::input::handle_event(&mut app, crossterm::event::Event::Key(up()));
    assert_eq!(
        app.slash_suggest.selected,
        BUILTIN_SLASH_COMMANDS.len() - 1,
        "Up at the top wraps to the bottom"
    );
    crate::input::handle_event(&mut app, crossterm::event::Event::Key(down()));
    assert_eq!(
        app.slash_suggest.selected, 0,
        "Down at the bottom wraps to the top"
    );
}

#[test]
fn step_wraps_around_in_both_directions() {
    use crate::app::SlashSuggestState;
    let mut s = SlashSuggestState {
        active: true,
        matches: vec![0, 1, 2],
        selected: 0,
        ..Default::default()
    };
    s.step(-1);
    assert_eq!(s.selected, 2);
    s.step(1);
    assert_eq!(s.selected, 0);
}

#[test]
fn current_returns_the_highlighted_command() {
    use crate::app::{BUILTIN_SLASH_COMMANDS, SlashSuggestState};
    let mut s = SlashSuggestState {
        active: true,
        matches: vec![],
        selected: 0,
        ..Default::default()
    };
    assert!(s.current().is_none());
    s.matches = BUILTIN_SLASH_COMMANDS
        .iter()
        .enumerate()
        .filter(|(_, c)| c.name.starts_with("/mod"))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(s.current().map(|c| c.name), Some("/models"));
}

#[test]
fn dismiss_clears_popup_state_so_a_new_slash_reopens_clean() {
    let mut app = TuiApp::new(test_config());
    app.input_buffer = "/mo".to_string();
    app.slash_suggest.refresh(&app.input_buffer);
    assert!(app.slash_suggest.active);
    assert!(app.slash_suggest.selected > 0 || !app.slash_suggest.matches.is_empty());

    app.slash_suggest.dismiss();
    assert!(!app.slash_suggest.active);
    assert_eq!(app.slash_suggest.selected, 0);

    app.input_buffer = "/he".to_string();
    app.slash_suggest.refresh(&app.input_buffer);
    assert!(app.slash_suggest.active);
    let names: Vec<&str> = app
        .slash_suggest
        .matches
        .iter()
        .map(|&i| crate::app::BUILTIN_SLASH_COMMANDS[i].name)
        .collect();
    assert_eq!(names, vec!["/help"]);
}

#[test]
fn login_picker_enter_sets_pending_provider() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = TuiApp::new(test_config());
    app.login_dialog.rows = vec![
        crate::app::LoginProviderRow {
            provider: "anthropic".to_string(),
            label: "Anthropic (Claude Pro/Max)".to_string(),
            connected: true,
        },
        crate::app::LoginProviderRow {
            provider: "openai".to_string(),
            label: "OpenAI (ChatGPT Plus/Pro)".to_string(),
            connected: false,
        },
    ];
    crate::input::open_dialog(&mut app, DialogKind::Login);
    crate::input::handle_event(
        &mut app,
        crossterm::event::Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
    );
    crate::input::handle_event(
        &mut app,
        crossterm::event::Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    assert_eq!(app.pending_login_provider.as_deref(), Some("openai"));
    assert!(app.dialogs.active().is_none());
}

#[test]
fn double_slash_still_lists_every_command() {
    use crate::app::BUILTIN_SLASH_COMMANDS;
    let mut app = TuiApp::new(test_config());
    // User typed `/`, then another `/` (Esc left a lone slash, or accidental).
    app.input_buffer = "//".to_string();
    app.slash_suggest.refresh(&app.input_buffer);
    assert!(app.slash_suggest.active, "bare // must reopen the menu");
    assert_eq!(
        app.slash_suggest.matches.len(),
        BUILTIN_SLASH_COMMANDS.len()
    );
}

#[test]
fn second_slash_key_after_lone_slash_reopens_menu() {
    use crate::app::BUILTIN_SLASH_COMMANDS;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut app = TuiApp::new(test_config());
    // First `/` opens the popup.
    crate::input::handle_event(
        &mut app,
        crossterm::event::Event::Key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE)),
    );
    assert_eq!(app.input_buffer, "/");
    assert!(app.slash_suggest.active);
    assert_eq!(
        app.slash_suggest.matches.len(),
        BUILTIN_SLASH_COMMANDS.len()
    );

    // Esc dismisses and clears the bare `/` draft.
    crate::input::handle_event(
        &mut app,
        crossterm::event::Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
    );
    assert!(!app.slash_suggest.active);
    assert!(
        app.input_buffer.is_empty(),
        "Esc on bare / should clear the draft"
    );

    // Second `/` opens a clean menu again.
    crate::input::handle_event(
        &mut app,
        crossterm::event::Event::Key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE)),
    );
    assert_eq!(app.input_buffer, "/");
    assert!(app.slash_suggest.active);
    assert_eq!(
        app.slash_suggest.matches.len(),
        BUILTIN_SLASH_COMMANDS.len()
    );
}

#[test]
fn second_slash_while_draft_is_slash_does_not_become_double() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut app = TuiApp::new(test_config());
    app.input_buffer = "/".to_string();
    app.input_cursor = 1;
    // Simulate dismissed menu with `/` still present (legacy path).
    app.slash_suggest.dismiss();

    crate::input::handle_event(
        &mut app,
        crossterm::event::Event::Key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE)),
    );
    assert_eq!(
        app.input_buffer, "/",
        "second / must not turn the draft into //"
    );
    assert!(app.slash_suggest.active);
}

// ── Grok focus model (Prompt vs Scrollback) ─────────────────────────

#[test]
fn default_focus_is_prompt() {
    use crate::app::FocusPane;
    let app = TuiApp::new(test_config());
    assert_eq!(app.focus, FocusPane::Prompt);
    assert!(app.selected_msg.is_none());
}

#[test]
fn tab_toggles_focus_to_scrollback_when_messages_exist() {
    use crate::app::FocusPane;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    let mut app = TuiApp::new(test_config());
    app.add_message(ChatRole::User, "hello");
    app.add_message(ChatRole::Assistant, "world");

    let tab = || Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    crate::input::handle_event(&mut app, tab());
    assert_eq!(app.focus, FocusPane::Scrollback);
    assert_eq!(app.selected_msg, Some(1), "selects last message");

    crate::input::handle_event(&mut app, tab());
    assert_eq!(app.focus, FocusPane::Prompt);
    assert!(app.selected_msg.is_none());
}

#[test]
fn typing_j_inserts_into_prompt_not_scroll() {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    let mut app = TuiApp::new(test_config());
    app.add_message(ChatRole::User, "hi");
    // Focus is Prompt by default — j must type, not select.
    crate::input::handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)),
    );
    assert_eq!(app.input_buffer, "j");
    assert_eq!(app.scroll_offset, 0);
}

#[test]
fn scrollback_j_k_move_selection() {
    use crate::app::FocusPane;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    let mut app = TuiApp::new(test_config());
    app.add_message(ChatRole::User, "one");
    app.add_message(ChatRole::Assistant, "two");
    app.add_message(ChatRole::User, "three");
    app.focus_scrollback();
    assert_eq!(app.selected_msg, Some(2));

    crate::input::handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE)),
    );
    assert_eq!(app.focus, FocusPane::Scrollback);
    assert_eq!(app.selected_msg, Some(1));

    crate::input::handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)),
    );
    assert_eq!(app.selected_msg, Some(2));
}

#[test]
fn letter_while_scrollback_focused_autofocuses_prompt() {
    use crate::app::FocusPane;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    let mut app = TuiApp::new(test_config());
    app.add_message(ChatRole::User, "hi");
    app.focus_scrollback();
    assert_eq!(app.focus, FocusPane::Scrollback);

    // Unbound letter (not j/k/y/e/h/l/g/i/…) auto-focuses prompt and inserts.
    crate::input::handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)),
    );
    assert_eq!(app.focus, FocusPane::Prompt);
    assert_eq!(app.input_buffer, "a");
}

#[test]
fn double_esc_clears_draft() {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    let mut app = TuiApp::new(test_config());
    app.input_buffer = "draft text".into();
    app.input_cursor = app.input_buffer.len();

    let esc = || Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    crate::input::handle_event(&mut app, esc());
    assert_eq!(app.input_buffer, "draft text", "first Esc arms clear");
    assert!(app.esc_armed_at.is_some());

    crate::input::handle_event(&mut app, esc());
    assert!(app.input_buffer.is_empty(), "second Esc clears");
    assert!(app.esc_armed_at.is_none());
}

#[test]
fn jump_user_turn_finds_previous_user_message() {
    let mut app = TuiApp::new(test_config());
    app.add_message(ChatRole::User, "u1");
    app.add_message(ChatRole::Assistant, "a1");
    app.add_message(ChatRole::User, "u2");
    app.add_message(ChatRole::Assistant, "a2");
    app.focus_scrollback();
    assert_eq!(app.selected_msg, Some(3));
    app.jump_user_turn(false);
    assert_eq!(app.selected_msg, Some(2));
    app.jump_user_turn(false);
    assert_eq!(app.selected_msg, Some(0));
}

#[test]
fn scroll_rows_uses_display_height_not_message_count() {
    let mut app = TuiApp::new(test_config());
    // A long wrapped user message is many rows but one message.
    let long = "word ".repeat(80);
    app.add_message(ChatRole::User, long);
    app.chat_content_width = 40;
    app.chat_viewport_rows = 5;
    let total = crate::ui::chat::session_line_count(&app, 40);
    assert!(total > 8, "expected wrapped rows, got {total}");

    let max_off = total.saturating_sub(5);
    app.scroll_rows(3);
    assert_eq!(app.scroll_offset, 3.min(max_off));
    assert!(!app.auto_scroll);
    app.scroll_to_bottom();
    assert_eq!(app.scroll_offset, 0);
    assert!(app.auto_scroll);
}

#[test]
fn chat_scrollbar_hit_and_mouse_wheel_scroll_transcript() {
    use crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::layout::Rect;

    let mut app = TuiApp::new(test_config());
    for i in 0..20 {
        app.add_message(ChatRole::User, format!("line {i} with enough text"));
        app.add_message(ChatRole::Assistant, format!("reply {i}"));
    }
    app.chat_content_width = 40;
    app.chat_viewport_rows = 8;
    let total = crate::ui::chat::session_line_count(&app, 40);
    assert!(total > 8, "need overflow, total={total}");

    // Publish paint meta as if a frame just drew a scrollbar.
    let area = Rect {
        x: 2,
        y: 1,
        width: 40,
        height: 8,
    };
    let bar = Rect {
        x: 40,
        y: 1,
        width: 2,
        height: 8,
    };
    app.apply_chat_paint(area, Some(bar), total);
    assert!(app.chat_scrollbar_contains(40, 3));
    assert!(app.chat_area_contains(10, 4));
    assert!(!app.chat_area_contains(0, 0));

    // Wheel up → older messages (increase bottom-anchored offset).
    let wheel_up = Event::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 10,
        row: 4,
        modifiers: KeyModifiers::NONE,
    });
    assert!(crate::input::handle_event(&mut app, wheel_up));
    assert!(app.scroll_offset >= 3, "offset={}", app.scroll_offset);
    assert!(!app.auto_scroll);

    // Click the track near the top → jump toward oldest (large offset).
    let down = Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 40,
        row: 1,
        modifiers: KeyModifiers::NONE,
    });
    assert!(crate::input::handle_event(&mut app, down));
    let max_off = total.saturating_sub(8);
    assert!(
        app.scroll_offset >= max_off.saturating_sub(2),
        "expected near top, offset={} max={max_off}",
        app.scroll_offset
    );
    assert!(app.chat_scrollbar_grab.is_some());

    let up = Event::Mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: 40,
        row: 1,
        modifiers: KeyModifiers::NONE,
    });
    assert!(crate::input::handle_event(&mut app, up));
    assert!(app.chat_scrollbar_grab.is_none());
}

#[test]
fn scroll_to_top_and_bottom_match_visible_range() {
    let mut app = TuiApp::new(test_config());
    for i in 0..12 {
        app.add_message(
            ChatRole::User,
            format!("message line {i} with enough text to wrap maybe"),
        );
        app.add_message(ChatRole::Assistant, format!("reply {i}"));
    }
    app.chat_content_width = 40;
    app.chat_viewport_rows = 8;
    let total = crate::ui::chat::session_line_count(&app, 40);
    let height = 8usize;
    assert!(total > height, "need overflow for this test, total={total}");

    app.scroll_to_bottom();
    let (s, e) = crate::ui::chat::visible_range(total, height, app.scroll_offset);
    assert_eq!(e, total, "bottom should end at last row");
    assert_eq!(e - s, height);

    app.scroll_to_top();
    let (s, e) = crate::ui::chat::visible_range(total, height, app.scroll_offset);
    assert_eq!(s, 0, "top should start at first row");
    assert_eq!(e - s, height);
    assert_eq!(app.scroll_offset, total - height);
}

// ── Multi-line prompt wrapping ──────────────────────────────────────

#[test]
fn input_row_count_grows_with_wrapped_text() {
    let mut app = TuiApp::new(test_config());
    // Seed a message so the session layout (full width) is used.
    app.add_message(ChatRole::User, String::from("hi"));
    let width = 60u16;

    assert_eq!(crate::ui::prompt::input_row_count(&app, width), 1);

    app.input_buffer = "word ".repeat(40).trim().to_string();
    let rows = crate::ui::prompt::input_row_count(&app, width);
    assert!(
        rows > 1,
        "long text must wrap onto multiple rows, got {rows}"
    );
    assert!(rows <= crate::ui::prompt::MAX_INPUT_ROWS);
}

#[test]
fn input_row_count_respects_max_cap() {
    let mut app = TuiApp::new(test_config());
    app.add_message(ChatRole::User, String::from("hi"));
    app.input_buffer = "x".repeat(100 * 80);
    let rows = crate::ui::prompt::input_row_count(&app, 80);
    assert_eq!(rows, crate::ui::prompt::MAX_INPUT_ROWS);
}

#[test]
fn newline_in_input_forces_an_extra_row() {
    let mut app = TuiApp::new(test_config());
    app.add_message(ChatRole::User, String::from("hi"));
    app.input_buffer = "ab\ncd".to_string();
    let rows = crate::ui::prompt::input_row_count(&app, 60);
    assert_eq!(rows, 2);
}

// ── Chat message panel wrapping ─────────────────────────────────────

#[test]
fn user_message_panel_wraps_long_text() {
    let mut app = TuiApp::new(test_config());
    let long = "lorem ipsum dolor sit amet ".repeat(20);
    app.add_message(ChatRole::User, long.clone());

    let narrow = crate::ui::chat::session_line_count(&app, 60);
    assert!(
        narrow > 4,
        "a long user message must occupy several rows, got {narrow}"
    );
}

#[test]
fn user_message_preserves_newlines_in_panel() {
    let mut app = TuiApp::new(test_config());
    app.add_message(ChatRole::User, String::from("first\nsecond\nthird"));

    // 3 content rows (no top/bottom bubble pad — those polluted mouse selection).
    let rows = crate::ui::chat::session_line_count(&app, 80);
    assert!(rows >= 3, "expected at least 3 rows, got {rows}");
}

#[test]
fn git_output_timeout_kills_a_sleeping_child() {
    let start = std::time::Instant::now();
    let out = crate::app::git_output_timeout(
        std::process::Command::new("sleep").arg("2"),
        std::time::Duration::from_millis(80),
    );
    assert!(out.is_none(), "sleep must not outlast the cap");
    assert!(
        start.elapsed() < std::time::Duration::from_millis(800),
        "timeout must not wait for the full sleep"
    );
}

#[test]
fn coalesce_resizes_keeps_only_the_last() {
    use crossterm::event::Event;
    let mut events = vec![
        Event::Resize(80, 24),
        Event::Key(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Char('a'),
        )),
        Event::Resize(100, 30),
        Event::Resize(120, 40),
    ];
    crate::input::coalesce_resizes(&mut events);
    let resizes: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::Resize(_, _)))
        .collect();
    assert_eq!(resizes.len(), 1);
    assert!(matches!(resizes[0], Event::Resize(120, 40)));
    assert_eq!(events.len(), 2, "key + last resize");
}

#[test]
fn adopt_yield_view_moves_transcript_without_clone() {
    let mut app = TuiApp::from_config(test_config());
    app.add_message(ChatRole::Assistant, String::from("body"));
    let ptr = app.messages.as_ptr();
    let mut view = crate::session_runtime::ViewSnapshot::default();
    app.yield_view(&mut view);
    assert!(app.messages.is_empty());
    assert_eq!(view.messages.as_ptr(), ptr);
    assert_eq!(view.messages[0].content, "body");

    app.adopt_view(&mut view);
    assert!(view.messages.is_empty());
    assert_eq!(app.messages.as_ptr(), ptr);
    assert_eq!(app.messages[0].content, "body");
}
