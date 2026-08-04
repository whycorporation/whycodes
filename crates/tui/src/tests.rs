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
    use crate::app::{format_thinking_elapsed, ThinkingBlock};
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
        context_tokens_from_usage, format_context_percent, format_context_usage,
    };
    use whycode_core::types::Usage;

    assert_eq!(format_context_usage(1_200, 200_000), "1.2k / 200k");
    assert_eq!(format_context_percent(1_200, 200_000), "1%");
    assert_eq!(format_context_percent(0, 200_000), "0%");
    assert_eq!(format_context_percent(200_000, 200_000), "100%");
    assert_eq!(format_context_usage(12_400, 128_000), "12.4k / 128k");

    let u = Usage {
        input_tokens: 1000,
        output_tokens: 50,
        cache_creation_input_tokens: Some(100),
        cache_read_input_tokens: Some(400),
    };
    // Context fill counts prompt-side tokens, not completion output.
    assert_eq!(context_tokens_from_usage(&u), 1500);
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
    assert!(matches!(
        &app.messages[0].blocks[0],
        ChatBlock::Thinking(t) if t.collapsed
    ));
    app.toggle_selected_thinking();
    assert!(matches!(
        &app.messages[0].blocks[0],
        ChatBlock::Thinking(t) if !t.collapsed && t.show_body()
    ));
}

#[test]
fn test_thinking_live_tail_truncates() {
    use crate::app::{ThinkingBlock, THINKING_LIVE_TAIL_LINES};
    let mut text = String::new();
    for i in 0..(THINKING_LIVE_TAIL_LINES + 5) {
        text.push_str(&format!("line {i}\n"));
    }
    let t = ThinkingBlock::new(text);
    assert!(t.is_truncated_live());
    assert_eq!(t.body_lines().len(), THINKING_LIVE_TAIL_LINES);
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
fn scroll_to_top_and_bottom_match_visible_range() {
    let mut app = TuiApp::new(test_config());
    for i in 0..12 {
        app.add_message(ChatRole::User, format!("message line {i} with enough text to wrap maybe"));
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
