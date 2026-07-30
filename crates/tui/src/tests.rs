use std::str::FromStr;
use crate::theme::ThemeName;
use crate::app::{
    TuiApp, AppMode, ChatRole,
    DialogManager, DialogKind, ConfirmAction,
    SidebarState, SidebarTab, AgentState,
    ProviderDialogState, ProviderDialogMode, AuthMethod,
};
use crate::config::TuiAppConfig;

fn test_config() -> TuiAppConfig {
    TuiAppConfig::default()
}

// ── Theme Tests ─────────────────────────────────────────────────────

#[test]
fn test_theme_from_str_dark() {
    assert_eq!(ThemeName::from_str("dark").unwrap(), ThemeName::DefaultDark);
    assert_eq!(ThemeName::from_str("default-dark").unwrap(), ThemeName::DefaultDark);
    assert_eq!(ThemeName::from_str("default_dark").unwrap(), ThemeName::DefaultDark);
}

#[test]
fn test_theme_from_str_light() {
    assert_eq!(ThemeName::from_str("light").unwrap(), ThemeName::DefaultLight);
    assert_eq!(ThemeName::from_str("default_light").unwrap(), ThemeName::DefaultLight);
}

#[test]
fn test_theme_from_str_named_themes() {
    assert_eq!(ThemeName::from_str("monokai").unwrap(), ThemeName::Monokai);
    assert_eq!(ThemeName::from_str("nord").unwrap(), ThemeName::Nord);
    assert_eq!(ThemeName::from_str("dracula").unwrap(), ThemeName::Dracula);
    assert_eq!(ThemeName::from_str("gruvbox").unwrap(), ThemeName::Gruvbox);
    assert_eq!(ThemeName::from_str("catppuccin-mocha").unwrap(), ThemeName::CatppuccinMocha);
    assert_eq!(ThemeName::from_str("tokyonight").unwrap(), ThemeName::TokyoNight);
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
    assert_ne!(format!("{:?}", palette.error), format!("{:?}", palette.success));
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
        ThemeName::DefaultDark, ThemeName::DefaultLight,
        ThemeName::Monokai, ThemeName::SolarizedDark, ThemeName::SolarizedLight,
        ThemeName::Nord, ThemeName::Dracula, ThemeName::Gruvbox,
        ThemeName::OneDark, ThemeName::CatppuccinMocha, ThemeName::CatppuccinLatte,
        ThemeName::TokyoNight, ThemeName::TokyoNightStorm, ThemeName::TokyoNightLight,
        ThemeName::Kanagawa, ThemeName::Everforest,
        ThemeName::RosePine, ThemeName::RosePineMoon, ThemeName::RosePineDawn,
        ThemeName::AyuDark, ThemeName::AyuMirage, ThemeName::AyuLight,
        ThemeName::GithubDark, ThemeName::GithubLight,
        ThemeName::VscodeDark, ThemeName::VscodeLight,
        ThemeName::Zenburn, ThemeName::OceanicNext, ThemeName::MaterialPalenight,
    ];
    for theme in &themes {
        theme.palette(); // should not panic
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
    app.add_tool_call("tc-1".to_string(), "bash".to_string(), serde_json::json!({}));
    app.add_tool_result("tc-1", "output here", false);
    assert_eq!(app.messages[0].tool_calls[0].result.as_deref(), Some("output here"));
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
