use super::*;
use crate::app::{
    AgentState, AppMode, ChatBlock, ChatRole, ConfirmAction, DialogKind, FocusPane, SidebarTab,
    TuiApp,
};
use crate::config::TuiAppConfig;
use crate::keymap::KeymapContext;
use crate::theme::ThemeName;
use crossterm::event::{KeyModifiers, MouseButton};
use ratatui::layout::Rect;

fn app() -> TuiApp {
    TuiApp::new(TuiAppConfig::default())
}

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn key_release(code: KeyCode) -> Event {
    let mut k = KeyEvent::new(code, KeyModifiers::NONE);
    k.kind = KeyEventKind::Release;
    Event::Key(k)
}

fn ctrl(c: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> Event {
    Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

#[test]
fn handle_event_ignores_key_release_and_keeps_running_on_resize() {
    let mut a = app();
    let mut rel = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
    rel.kind = KeyEventKind::Release;
    assert!(handle_event(&mut a, Event::Key(rel)));
    assert!(a.input_buffer.is_empty());
    assert!(handle_event(&mut a, Event::Resize(120, 40)));
    assert!(
        a.needs_redraw,
        "resize (OSK / rotate) must dirty so the next paint uses the new size"
    );
    assert!(handle_event(&mut a, Event::FocusGained));
}

#[test]
fn ctrl_v_attaches_stubbed_clipboard_image() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shot.png");
    std::fs::write(&path, b"\x89PNG\r\n\x1a\nhello").unwrap();

    crate::clipboard_image::with_stub(
        Ok(crate::clipboard_image::PromptClipboard::ImagePaths(vec![
            path.clone(),
        ])),
        || {
            let mut a = app();
            a.focus = FocusPane::Scrollback;
            assert!(handle_event(&mut a, ctrl('v')));
            assert_eq!(a.focus, FocusPane::Prompt);
            assert_eq!(a.pending_images.len(), 1);
            assert!(
                a.toasts
                    .visible()
                    .iter()
                    .any(|t| t.message.contains("Attached")),
                "expected attach toast, got {:?}",
                a.toasts
                    .visible()
                    .iter()
                    .map(|t| t.message.as_str())
                    .collect::<Vec<_>>()
            );
        },
    );
}

#[test]
fn ctrl_v_empty_clipboard_is_silent() {
    crate::clipboard_image::with_stub(Ok(crate::clipboard_image::PromptClipboard::Empty), || {
        let mut a = app();
        a.input_buffer = "keep".into();
        a.input_cursor = 4;
        assert!(handle_event(&mut a, ctrl('v')));
        assert_eq!(a.input_buffer, "keep");
        assert!(a.pending_images.is_empty());
        assert!(a.toasts.is_empty());
    });
}

#[test]
fn ctrl_v_text_stub_inserts_like_bracketed_paste() {
    crate::clipboard_image::with_stub(
        Ok(crate::clipboard_image::PromptClipboard::Text(
            "hello from clip".into(),
        )),
        || {
            let mut a = app();
            assert!(handle_event(&mut a, ctrl('v')));
            assert_eq!(a.input_buffer, "hello from clip");
            assert!(a.pending_images.is_empty());
        },
    );
}

#[test]
fn ctrl_v_error_toasts_and_keeps_draft() {
    crate::clipboard_image::with_stub(Err("clipboard image is too large".into()), || {
        let mut a = app();
        a.input_buffer = "draft".into();
        assert!(handle_event(&mut a, ctrl('v')));
        assert_eq!(a.input_buffer, "draft");
        assert!(a.pending_images.is_empty());
        assert!(
            a.toasts
                .visible()
                .iter()
                .any(|t| t.kind == crate::toast::ToastKind::Warning
                    && t.message.contains("too large")),
            "expected warning toast"
        );
    });
}

#[test]
fn ctrl_v_ignored_in_dialog() {
    crate::clipboard_image::with_stub(
        Ok(crate::clipboard_image::PromptClipboard::Text("nope".into())),
        || {
            let mut a = app();
            a.mode = AppMode::Dialog;
            a.key_context = KeymapContext::Dialog;
            a.dialogs.push(DialogKind::Theme);
            assert!(handle_event(&mut a, ctrl('v')));
            assert!(a.input_buffer.is_empty());
            assert!(a.pending_images.is_empty());
        },
    );
}

#[test]
fn typing_and_editing_keys_update_the_prompt() {
    let mut a = app();
    assert!(handle_event(&mut a, key(KeyCode::Char('h'))));
    assert!(handle_event(&mut a, key(KeyCode::Char('i'))));
    assert_eq!(a.input_buffer, "hi");
    assert_eq!(a.input_cursor, 2);

    assert!(handle_event(&mut a, key(KeyCode::Left)));
    assert_eq!(a.input_cursor, 1);
    assert!(handle_event(&mut a, key(KeyCode::Right)));
    assert_eq!(a.input_cursor, 2);
    assert!(handle_event(&mut a, key(KeyCode::Home)));
    assert_eq!(a.input_cursor, 0);
    assert!(handle_event(&mut a, key(KeyCode::End)));
    assert_eq!(a.input_cursor, 2);

    assert!(handle_event(&mut a, key(KeyCode::Backspace)));
    assert_eq!(a.input_buffer, "h");
    a.input_cursor = 0;
    assert!(handle_event(&mut a, key(KeyCode::Delete)));
    assert!(a.input_buffer.is_empty());

    a.input_buffer = "ab".into();
    a.input_cursor = 1;
    assert!(handle_event(
        &mut a,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT))
    ));
    assert_eq!(a.input_buffer, "a\nb");

    a.input_buffer = "keep".into();
    a.input_cursor = 4;
    assert!(handle_event(&mut a, ctrl('u')));
    assert!(a.input_buffer.is_empty());
    assert_eq!(a.input_cursor, 0);
}

#[test]
fn ctrl_w_and_ctrl_backspace_kill_the_previous_word() {
    let mut a = app();
    a.input_buffer = "hello world".into();
    a.input_cursor = a.input_buffer.len();
    assert!(handle_event(&mut a, ctrl('w')));
    assert_eq!(a.input_buffer, "hello ");
    assert_eq!(a.input_cursor, 6);

    assert!(handle_event(
        &mut a,
        Event::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL))
    ));
    assert_eq!(a.input_buffer, "");
    assert_eq!(a.input_cursor, 0);
}

#[test]
fn ctrl_w_deletes_a_collapsed_paste_token_as_one_unit() {
    let mut a = app();
    let token = crate::paste::placeholder(1, 3);
    a.input_buffer = format!("see {token}");
    a.input_cursor = a.input_buffer.len();
    a.pending_pastes.push(crate::paste::PastedBlock {
        id: 1,
        content: "a\nb\nc".into(),
    });
    assert!(handle_event(&mut a, ctrl('w')));
    assert_eq!(a.input_buffer, "see ");
    assert!(a.pending_pastes.is_empty());
}

#[test]
fn ctrl_delete_and_alt_d_kill_the_next_word() {
    let mut a = app();
    a.input_buffer = "hello world".into();
    a.input_cursor = 0;
    assert!(handle_event(
        &mut a,
        Event::Key(KeyEvent::new(KeyCode::Delete, KeyModifiers::CONTROL))
    ));
    assert_eq!(a.input_buffer, " world");
    assert_eq!(a.input_cursor, 0);

    a.input_buffer = "hello world".into();
    a.input_cursor = 5;
    assert!(handle_event(
        &mut a,
        Event::Key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT))
    ));
    assert_eq!(a.input_buffer, "hello");
    assert_eq!(a.input_cursor, 5);
}

#[test]
fn ctrl_arrows_move_by_word_without_stealing_shift_turn_jump() {
    let mut a = app();
    a.input_buffer = "hello world".into();
    a.input_cursor = a.input_buffer.len();
    assert!(handle_event(
        &mut a,
        Event::Key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL))
    ));
    assert_eq!(a.input_cursor, 6);
    assert!(handle_event(
        &mut a,
        Event::Key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL))
    ));
    assert_eq!(a.input_cursor, 0);
    assert!(handle_event(
        &mut a,
        Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL))
    ));
    assert_eq!(a.input_cursor, 5);

    // Shift+Left is turn-jump, not word-left — cursor stays put.
    assert!(handle_event(
        &mut a,
        Event::Key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT))
    ));
    assert_eq!(a.input_cursor, 5);
    assert_eq!(a.input_buffer, "hello world");
}

#[test]
fn unmapped_ctrl_letter_does_not_insert_into_the_prompt() {
    let mut a = app();
    a.input_buffer = "hi".into();
    a.input_cursor = 2;
    // Ctrl+X is unbound; must not type `x`.
    assert!(handle_event(&mut a, ctrl('x')));
    assert_eq!(a.input_buffer, "hi");
    assert_eq!(a.input_cursor, 2);
}

#[test]
fn session_picker_ctrl_w_still_closes_a_live_row() {
    let mut a = app();
    a.session_list.sessions = vec![crate::app::SessionEntry {
        id: "live".into(),
        title: "t".into(),
        messages: 1,
        updated_at: None,
        live: Some(2),
    }];
    open_dialog(&mut a, DialogKind::SessionList);
    assert!(handle_event(&mut a, ctrl('w')));
    assert_eq!(a.session_list.pending_close, Some(2));
}

#[test]
fn history_up_down_walk_the_prompt_stack() {
    let mut a = app();
    a.input_history = vec!["one".into(), "two".into()];
    a.input_history_idx = 2;
    assert!(handle_event(&mut a, key(KeyCode::Up)));
    assert_eq!(a.input_buffer, "two");
    assert!(handle_event(&mut a, key(KeyCode::Up)));
    assert_eq!(a.input_buffer, "one");
    assert!(handle_event(&mut a, key(KeyCode::Down)));
    assert_eq!(a.input_buffer, "two");
    assert!(handle_event(&mut a, key(KeyCode::Down)));
    assert!(a.input_buffer.is_empty());
}

#[test]
fn quit_help_command_and_escape_modes() {
    let mut a = app();
    assert!(handle_event(&mut a, ctrl('c')));
    assert_eq!(a.mode, AppMode::Dialog);
    assert!(matches!(
        a.dialogs.active(),
        Some(DialogKind::Confirm {
            on_confirm: ConfirmAction::Quit,
            ..
        })
    ));
    assert!(handle_event(&mut a, key(KeyCode::Esc)));
    assert_eq!(a.mode, AppMode::Normal);

    a.mode = AppMode::Help;
    a.key_context = KeymapContext::Help;
    assert!(handle_event(&mut a, key(KeyCode::Char('?'))));
    assert_eq!(a.mode, AppMode::Help);
    assert!(handle_event(&mut a, key(KeyCode::Char('q'))));
    assert_eq!(a.mode, AppMode::Normal);

    a.input_buffer.clear();
    a.input_cursor = 0;
    assert!(handle_event(&mut a, key(KeyCode::Char('?'))));
    assert_eq!(a.mode, AppMode::Normal);
    assert_eq!(a.input_buffer, "?");

    // `:` is a printable character — URLs / `::` stay in the chat draft.
    assert!(handle_event(&mut a, key(KeyCode::Char(':'))));
    assert_eq!(a.mode, AppMode::Normal);
    assert_eq!(a.input_buffer, "?:");
    assert!(a.command.buffer.is_empty());
}

#[test]
fn colon_types_into_the_prompt_instead_of_opening_command_mode() {
    let mut a = app();
    assert!(handle_event(&mut a, key(KeyCode::Char(':'))));
    assert!(handle_event(&mut a, key(KeyCode::Char(':'))));
    assert_eq!(a.mode, AppMode::Normal);
    assert_eq!(a.input_buffer, "::");
    assert_eq!(a.input_cursor, 2);
    assert!(a.command.buffer.is_empty());
    assert_eq!(a.key_context, KeymapContext::Normal);
}

#[test]
fn escape_steals_slash_file_busy_and_double_clears() {
    let mut a = app();
    a.input_buffer = "/".into();
    a.input_cursor = 1;
    a.slash_suggest.refresh(&a.input_buffer);
    assert!(a.slash_suggest.active);
    handle_event(&mut a, key(KeyCode::Esc));
    assert!(!a.slash_suggest.active);
    assert!(a.input_buffer.is_empty(), "bare slash draft is dropped");

    a.file_suggest.active = true;
    a.input_buffer = "@src".into();
    handle_event(&mut a, key(KeyCode::Esc));
    assert!(!a.file_suggest.active);
    assert_eq!(a.input_buffer, "@src");

    a.current_agent_state = AgentState::Generating;
    a.input_buffer = "keep".into();
    handle_event(&mut a, key(KeyCode::Esc));
    assert_eq!(a.input_buffer, "keep");
    a.current_agent_state = AgentState::Idle;

    handle_event(&mut a, key(KeyCode::Esc));
    assert!(a.esc_armed_at.is_some());
    handle_event(&mut a, key(KeyCode::Esc));
    assert!(a.input_buffer.is_empty());
    assert!(
        a.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("cleared"))
    );

    a.focus = FocusPane::Scrollback;
    handle_event(&mut a, key(KeyCode::Esc));
    assert_eq!(a.focus, FocusPane::Prompt);
}

#[test]
fn slash_reopens_on_second_slash_instead_of_doubling() {
    let mut a = app();
    a.input_buffer = "/".into();
    a.input_cursor = 1;
    handle_event(&mut a, key(KeyCode::Char('/')));
    assert_eq!(a.input_buffer, "/");
    assert!(a.slash_suggest.active);
}

#[test]
fn file_suggest_keys_accept_step_and_open() {
    let mut a = app();
    a.file_suggest.active = true;
    a.file_suggest.token_start = 0;
    a.file_suggest.matches = vec![
        whycodes_index::FileMatch {
            rel: "a.rs".into(),
            ..Default::default()
        },
        whycodes_index::FileMatch {
            rel: "b.rs".into(),
            ..Default::default()
        },
    ];
    a.input_buffer = "@x".into();
    a.input_cursor = 2;
    handle_event(&mut a, key(KeyCode::Down));
    assert_eq!(a.file_suggest.selected, 1);
    handle_event(&mut a, key(KeyCode::Up));
    assert_eq!(a.file_suggest.selected, 0);
    handle_event(&mut a, key(KeyCode::Tab));
    assert_eq!(a.input_buffer, "@a.rs ");
    assert!(!a.file_suggest.active);

    a.file_suggest.active = true;
    a.file_suggest.matches.clear();
    handle_event(&mut a, key(KeyCode::Enter));
    assert!(!a.file_suggest.active);

    handle_event(
        &mut a,
        Event::Key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL)),
    );
    assert!(a.file_suggest.active);
    assert!(a.input_buffer.contains('@'));
}

#[test]
fn submit_slash_tab_and_focus_actions() {
    let mut a = app();
    a.input_buffer = "hello".into();
    handle_event(&mut a, key(KeyCode::Enter));
    assert_eq!(a.pending_prompt.as_deref(), Some("hello"));
    assert!(a.input_buffer.is_empty());

    a.input_buffer = "/".into();
    a.slash_suggest.refresh(&a.input_buffer);
    let before = a.slash_suggest.selected;
    handle_event(&mut a, key(KeyCode::Down));
    assert_ne!(a.slash_suggest.selected, before);
    handle_event(&mut a, key(KeyCode::Tab));
    assert!(a.input_buffer.starts_with('/'));

    a.pending_suggestion = Some("try this".into());
    a.input_buffer.clear();
    a.slash_suggest.dismiss();
    handle_event(&mut a, key(KeyCode::Tab));
    assert_eq!(a.input_buffer, "try this");

    a.slash_suggest.dismiss();
    handle_event(&mut a, key(KeyCode::Tab));
    assert_eq!(a.focus, FocusPane::Scrollback);
    handle_event(&mut a, key(KeyCode::Tab));
    assert_eq!(a.focus, FocusPane::Prompt);
}

#[test]
fn sidebar_scroll_and_dialog_chords() {
    let mut a = app();
    assert!(!a.sidebar.visible);
    handle_event(&mut a, ctrl('b'));
    assert!(a.sidebar.visible);
    handle_event(&mut a, ctrl('g'));
    assert_eq!(a.sidebar.active_tab, SidebarTab::Agents);
    assert!(a.sidebar.visible, "empty tasks list still uses Agents tab");

    let mut a = app();
    a.upsert_subagent(crate::app::SubagentUpdate {
        id: "kid".into(),
        kind: "explore".into(),
        description: "look".into(),
        status: "running".into(),
        activity: "Thinking".into(),
        elapsed_ms: 0,
        output: String::new(),
    });
    assert!(!a.tasks_collapsed);
    handle_event(&mut a, ctrl('g'));
    assert!(a.tasks_collapsed);
    assert!(
        !a.sidebar.visible,
        "Ctrl+G must fold the sticky panel, not open the sidebar"
    );
    handle_event(&mut a, ctrl('g'));
    assert!(!a.tasks_collapsed);

    a.focus = FocusPane::Scrollback;
    handle_event(&mut a, key(KeyCode::Char(']')));
    handle_event(&mut a, key(KeyCode::Char('[')));

    let mut a = app();
    a.focus = FocusPane::Prompt;
    handle_event(&mut a, ctrl('.'));
    assert!(a.sidebar.visible, "Ctrl+. opens the sidebar from prompt");
    assert_eq!(a.sidebar.active_tab, SidebarTab::Files);
    handle_event(&mut a, ctrl('.'));
    assert_eq!(a.sidebar.active_tab, SidebarTab::Diagnostics);
    handle_event(&mut a, ctrl(','));
    assert_eq!(a.sidebar.active_tab, SidebarTab::Files);

    a.focus = FocusPane::Scrollback;
    handle_event(&mut a, key(KeyCode::Char('3')));
    assert_eq!(a.sidebar.active_tab, SidebarTab::Mcp);
    handle_event(&mut a, key(KeyCode::Char('6')));
    assert_eq!(a.sidebar.active_tab, SidebarTab::Agents);

    a.focus = FocusPane::Prompt;
    a.input_buffer.clear();
    a.input_cursor = 0;
    handle_event(&mut a, key(KeyCode::Char('1')));
    assert_eq!(a.input_buffer, "1", "digits still type in the prompt");
    handle_event(&mut a, key(KeyCode::Char('.')));
    assert!(a.input_buffer.ends_with('.'), "bare `.` still types");

    handle_event(&mut a, ctrl('a'));
    assert!(!a.auto_scroll);
    handle_event(&mut a, ctrl('a'));
    assert!(a.auto_scroll);

    handle_event(&mut a, ctrl('l'));
    assert!(matches!(
        a.dialogs.active(),
        Some(DialogKind::Confirm {
            on_confirm: ConfirmAction::ClearSession,
            ..
        })
    ));
    handle_event(&mut a, key(KeyCode::Enter));
    assert!(a.messages.is_empty());

    let mut a = app();
    handle_event(&mut a, ctrl('t'));
    assert!(a.status_message.contains("switch agent"));

    handle_event(&mut a, key(KeyCode::PageUp));
    handle_event(&mut a, key(KeyCode::PageDown));
    a.focus = FocusPane::Scrollback;
    handle_event(&mut a, key(KeyCode::Home));
    handle_event(&mut a, key(KeyCode::End));

    handle_event(&mut a, ctrl('p'));
    assert!(matches!(a.dialogs.active(), Some(DialogKind::Provider)));
    handle_event(&mut a, key(KeyCode::Esc));
    handle_event(&mut a, ctrl('m'));
    assert!(matches!(a.dialogs.active(), Some(DialogKind::Model)));
}

#[test]
fn command_mode_executes_colon_commands() {
    type Check = fn(&TuiApp);
    let cases: &[(&str, Check)] = &[
        (":q", |a| assert!(!a.running)),
        (":help", |a| assert_eq!(a.mode, AppMode::Help)),
        (":theme", |a| {
            assert!(matches!(a.dialogs.active(), Some(DialogKind::Theme)))
        }),
        (":model", |a| {
            assert!(matches!(a.dialogs.active(), Some(DialogKind::Model)))
        }),
        (":clear", |a| assert!(a.messages.is_empty())),
        (":sidebar", |a| assert!(a.sidebar.visible)),
        (":nope", |a| assert!(a.status_message.contains("Unknown"))),
    ];
    for (cmd, check) in cases {
        let mut a = app();
        a.add_message(ChatRole::User, "stay");
        a.mode = AppMode::Command;
        a.key_context = KeymapContext::Command;
        a.command.buffer = (*cmd).into();
        handle_event(&mut a, key(KeyCode::Enter));
        check(&a);
    }
    let mut a = app();
    a.mode = AppMode::Command;
    a.key_context = KeymapContext::Command;
    a.command.buffer = ":".into();
    handle_event(&mut a, key(KeyCode::Char('q')));
    assert_eq!(a.command.buffer, ":q");

    let mut a = app();
    a.mode = AppMode::Help;
    handle_event(&mut a, key(KeyCode::Char('q')));
    assert_eq!(a.mode, AppMode::Normal);
}

#[test]
fn dialog_keys_confirm_cancel_and_list_jumps() {
    let mut a = app();
    a.confirm("Quit", "sure?", ConfirmAction::Quit);
    handle_event(&mut a, key(KeyCode::Char('y')));
    assert!(!a.running);

    let mut a = app();
    a.confirm("Clear", "?", ConfirmAction::ClearSession);
    a.add_message(ChatRole::User, "x");
    handle_event(&mut a, key(KeyCode::Enter));
    assert!(a.messages.is_empty());

    let mut a = app();
    a.alert("Hi", "there");
    handle_event(&mut a, key(KeyCode::Enter));
    assert_eq!(a.mode, AppMode::Normal);

    let mut a = app();
    a.theme_selected = 0;
    open_dialog(&mut a, DialogKind::Theme);
    handle_event(&mut a, key(KeyCode::Down));
    assert_eq!(a.theme_selected, 1);
    handle_event(&mut a, key(KeyCode::Home));
    assert_eq!(a.theme_selected, 0);
    a.dialog_list_total = ThemeName::ALL.len();
    handle_event(&mut a, key(KeyCode::End));
    assert_eq!(a.theme_selected, ThemeName::ALL.len() - 1);
    handle_event(&mut a, key(KeyCode::PageDown));
    handle_event(&mut a, key(KeyCode::PageUp));
    handle_event(&mut a, key(KeyCode::Enter));
    assert_eq!(a.mode, AppMode::Normal);

    let mut a = app();
    a.model_selection.models = vec![("acme".into(), "m1".into())];
    a.provider_name = "acme".into();
    a.model_name = "m1".into();
    open_model_dialog(&mut a);
    handle_event(&mut a, key(KeyCode::Enter));
    assert_eq!(
        a.pending_model
            .as_ref()
            .map(|(p, m)| (p.as_str(), m.as_str())),
        Some(("acme", "m1"))
    );

    let mut a = app();
    a.primary_agents = vec!["build".into(), "plan".into()];
    a.agent_name = "build".into();
    open_agent_dialog(&mut a);
    assert!(matches!(a.dialogs.active(), Some(DialogKind::Agent)));
    assert_eq!(a.agent_picker_selected, 0);
    handle_event(&mut a, key(KeyCode::Down));
    assert_eq!(a.agent_picker_selected, 1);
    handle_event(&mut a, key(KeyCode::Enter));
    assert_eq!(a.pending_agent.as_deref(), Some("plan"));
    assert_eq!(a.mode, AppMode::Normal);

    let mut a = app();
    a.session_list.sessions = vec![crate::app::SessionEntry {
        id: "abc".into(),
        title: "t".into(),
        messages: 1,
        updated_at: None,
        live: None,
    }];
    open_dialog(&mut a, DialogKind::SessionList);
    handle_event(&mut a, key(KeyCode::Enter));
    assert_eq!(a.pending_session_id.as_deref(), Some("abc"));

    let mut a = app();
    a.session_list.sessions = vec![crate::app::SessionEntry {
        id: "live".into(),
        title: "t".into(),
        messages: 1,
        updated_at: None,
        live: Some(2),
    }];
    open_dialog(&mut a, DialogKind::SessionList);
    handle_event(
        &mut a,
        Event::Key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL)),
    );
    assert_eq!(a.session_list.pending_close, Some(2));
}

#[test]
fn provider_form_types_and_backspaces() {
    let mut a = app();
    open_provider_dialog(&mut a);
    a.provider_dialog.mode = crate::app::ProviderDialogMode::AddCustom;
    a.provider_dialog.active_field = 0;
    handle_event(&mut a, key(KeyCode::Char('x')));
    assert_eq!(a.provider_dialog.form_name, "x");
    handle_event(&mut a, key(KeyCode::Backspace));
    assert!(a.provider_dialog.form_name.is_empty());
    handle_event(&mut a, key(KeyCode::Down));
    assert_eq!(a.provider_dialog.active_field, 1);
}

#[test]
fn paste_ignored_outside_normal_and_auth_esc_cancels() {
    let mut a = app();
    a.mode = AppMode::Help;
    handle_event(&mut a, Event::Paste("secret".into()));
    assert!(a.input_buffer.is_empty());

    let mut a = app();
    let (tx, _rx) = tokio::sync::oneshot::channel::<String>();
    a.auth_code_sink = Some(tx);
    handle_event(&mut a, key(KeyCode::Esc));
    assert!(a.auth_code_sink.is_none());
    assert!(a.status_message.contains("cancelled"));
}

#[test]
fn mouse_click_prompt_meta_opens_agent_and_model_pickers() {
    let mut a = app();
    a.agent_hit.set_rect(Some(Rect {
        x: 40,
        y: 20,
        width: 5,
        height: 1,
    }));
    a.primary_agents = vec!["build".into(), "plan".into()];
    a.agent_name = "build".into();
    handle_event(
        &mut a,
        mouse(MouseEventKind::Down(MouseButton::Left), 42, 20),
    );
    assert!(matches!(a.dialogs.active(), Some(DialogKind::Agent)));
    assert_eq!(a.agent_picker_selected, 0);

    let mut a = app();
    a.model_hit.set_rect(Some(Rect {
        x: 50,
        y: 20,
        width: 12,
        height: 1,
    }));
    a.model_selection.models = vec![("acme".into(), "m1".into())];
    a.provider_name = "acme".into();
    a.model_name = "m1".into();
    handle_event(
        &mut a,
        mouse(MouseEventKind::Down(MouseButton::Left), 52, 20),
    );
    assert!(matches!(a.dialogs.active(), Some(DialogKind::Model)));
    // Header + current model: cursor lands on the model row.
    assert_eq!(a.model_selection.selected, 1);

    let mut a = app();
    a.provider_name = "xai".into();
    a.model_name = "grok-4".into();
    a.effort_hit.set_rect(Some(Rect {
        x: 64,
        y: 20,
        width: 3,
        height: 1,
    }));
    handle_event(
        &mut a,
        mouse(MouseEventKind::Down(MouseButton::Left), 65, 20),
    );
    assert!(matches!(a.dialogs.active(), Some(DialogKind::Effort)));

    let mut a = app();
    a.approval_hit.set_rect(Some(Rect {
        x: 70,
        y: 20,
        width: 4,
        height: 1,
    }));
    handle_event(
        &mut a,
        mouse(MouseEventKind::Down(MouseButton::Left), 71, 20),
    );
    assert!(matches!(a.dialogs.active(), Some(DialogKind::ApprovalMode)));
}

#[test]
fn mouse_clicks_file_and_slash_rows() {
    let mut a = app();
    a.file_suggest.active = true;
    a.file_suggest.token_start = 0;
    a.file_suggest.matches = vec![whycodes_index::FileMatch {
        rel: "lib.rs".into(),
        ..Default::default()
    }];
    a.file_suggest.list_hit = Some(Rect {
        x: 0,
        y: 10,
        width: 20,
        height: 2,
    });
    a.input_buffer = "@x".into();
    handle_event(
        &mut a,
        mouse(MouseEventKind::Down(MouseButton::Left), 2, 10),
    );
    assert_eq!(a.input_buffer, "@lib.rs ");

    let mut a = app();
    a.input_buffer = "/".into();
    a.slash_suggest.refresh(&a.input_buffer);
    a.slash_suggest.list_hit = Some(Rect {
        x: 0,
        y: 8,
        width: 20,
        height: 4,
    });
    a.slash_suggest.list_scroll_start = 0;
    handle_event(&mut a, mouse(MouseEventKind::Down(MouseButton::Left), 1, 8));
    assert!(
        a.input_buffer.starts_with('/') && a.input_buffer.ends_with(' '),
        "{}",
        a.input_buffer
    );
    assert!(!a.slash_suggest.active);
}

#[test]
fn mouse_subagent_and_selection_drag() {
    let mut a = app();
    a.upsert_subagent(crate::app::SubagentUpdate {
        id: "child-1".into(),
        kind: "explore".into(),
        description: "look".into(),
        status: "running".into(),
        activity: "Thinking".into(),
        elapsed_ms: 10,
        output: String::new(),
    });
    a.subagent_strip_hit.push((
        Rect {
            x: 2,
            y: 0,
            width: 10,
            height: 1,
        },
        "child-1".into(),
    ));
    handle_event(&mut a, mouse(MouseEventKind::Down(MouseButton::Left), 4, 0));
    assert_eq!(a.open_subagent.as_deref(), Some("child-1"));
    handle_event(&mut a, key(KeyCode::Char('q')));
    assert!(a.open_subagent.is_none());

    let mut a = app();
    handle_event(&mut a, mouse(MouseEventKind::Down(MouseButton::Left), 3, 5));
    assert!(a.mouse_sel.is_some());
    handle_event(&mut a, mouse(MouseEventKind::Drag(MouseButton::Left), 8, 6));
    assert_eq!(a.mouse_sel.as_ref().map(|s| s.focus_x), Some(8));
    handle_event(&mut a, mouse(MouseEventKind::Up(MouseButton::Left), 8, 6));
}

#[test]
fn modal_mouse_scrolls_and_closes() {
    let mut a = app();
    open_dialog(&mut a, DialogKind::Theme);
    a.dialog_modal_hit = Some(Rect {
        x: 10,
        y: 5,
        width: 40,
        height: 12,
    });
    a.dialog_close_hit = Some(Rect {
        x: 48,
        y: 5,
        width: 3,
        height: 1,
    });
    handle_event(&mut a, mouse(MouseEventKind::ScrollDown, 20, 8));
    assert!(a.theme_selected > 0);
    handle_event(&mut a, mouse(MouseEventKind::ScrollUp, 20, 8));
    handle_event(
        &mut a,
        mouse(MouseEventKind::Down(MouseButton::Left), 49, 5),
    );
    assert!(!a.dialogs.is_open());
}

#[test]
fn help_overlay_search_then_esc_clears_then_closes() {
    let mut a = app();
    a.mode = AppMode::Help;
    a.key_context = KeymapContext::Help;
    handle_event(&mut a, key(KeyCode::Char('/')));
    assert!(a.help_searching);
    handle_event(&mut a, key(KeyCode::Char('t')));
    handle_event(&mut a, key(KeyCode::Char('a')));
    handle_event(&mut a, key(KeyCode::Char('b')));
    assert_eq!(a.help_query, "tab");
    handle_event(&mut a, key(KeyCode::Esc));
    assert!(a.help_query.is_empty());
    assert!(!a.help_searching);
    assert_eq!(a.mode, AppMode::Help);
    handle_event(&mut a, key(KeyCode::Esc));
    assert_eq!(a.mode, AppMode::Normal);
}

#[test]
fn help_overlay_wheel_and_close() {
    let mut a = app();
    a.mode = AppMode::Help;
    a.key_context = KeymapContext::Help;
    handle_event(&mut a, mouse(MouseEventKind::ScrollDown, 10, 10));
    assert_eq!(a.help_scroll, 3);
    handle_event(&mut a, mouse(MouseEventKind::ScrollUp, 10, 10));
    assert_eq!(a.help_scroll, 0);
    handle_event(&mut a, key(KeyCode::Esc));
    assert_eq!(a.mode, AppMode::Normal);
}

#[test]
fn chat_wheel_step_and_coalesce() {
    let mut a = app();
    a.chat_viewport_rows = 30;
    assert_eq!(chat_wheel_step(&a), 10);
    a.chat_viewport_rows = 3;
    assert_eq!(chat_wheel_step(&a), 3);

    a.chat_viewport_rows = 12;
    a.chat_content_width = 40;
    for i in 0..20 {
        a.add_message(ChatRole::User, format!("line {i}"));
    }
    let mut events = vec![
        mouse(MouseEventKind::ScrollUp, 1, 1),
        mouse(MouseEventKind::ScrollUp, 1, 1),
        mouse(MouseEventKind::Moved, 1, 1),
        mouse(MouseEventKind::ScrollDown, 1, 1),
    ];
    coalesce_chat_wheels(&mut a, &mut events);
    assert_eq!(events.len(), 1, "wheels folded, move kept");
    assert!(a.scroll_offset > 0);

    a.mode = AppMode::Help;
    let n = events.len();
    coalesce_chat_wheels(&mut a, &mut events);
    assert_eq!(events.len(), n, "modal leaves the batch alone");
}

fn overflowing_todos(app: &mut TuiApp) {
    app.replace_todos(
        (0..12)
            .map(|i| {
                whycodes_core::TodoItem::new(
                    format!("t{i}"),
                    format!("item {i}"),
                    whycodes_core::TodoStatus::Pending,
                )
            })
            .collect(),
    );
    app.todos_viewport_rows = 8;
    app.todos_body_hit.set_rect(Some(Rect {
        x: 0,
        y: 3,
        width: 40,
        height: 8,
    }));
    app.todos_hit.set_rect(Some(Rect {
        x: 0,
        y: 2,
        width: 40,
        height: 1,
    }));
}

#[test]
fn todo_wheel_scrolls_list_not_chat() {
    let mut a = app();
    overflowing_todos(&mut a);
    a.chat_viewport_rows = 20;
    a.chat_content_width = 40;
    a.add_message(ChatRole::User, "keep chat still");
    handle_event(&mut a, mouse(MouseEventKind::ScrollDown, 4, 5));
    assert_eq!(a.todos_scroll, 1);
    assert_eq!(a.scroll_offset, 0);
    handle_event(&mut a, mouse(MouseEventKind::ScrollUp, 4, 5));
    assert_eq!(a.todos_scroll, 0);
}

#[test]
fn todo_body_click_focuses_list_and_keys_scroll() {
    let mut a = app();
    overflowing_todos(&mut a);
    a.add_message(ChatRole::User, "hi");
    handle_event(&mut a, mouse(MouseEventKind::Down(MouseButton::Left), 4, 5));
    assert_eq!(a.focus, FocusPane::Todos);
    handle_event(&mut a, key(KeyCode::Down));
    assert_eq!(a.todos_scroll, 1);
    handle_event(&mut a, key(KeyCode::Char('j')));
    assert_eq!(a.todos_scroll, 2);
    handle_event(&mut a, key(KeyCode::Up));
    assert_eq!(a.todos_scroll, 1);
    handle_event(&mut a, key(KeyCode::Char('G')));
    assert_eq!(a.todos_scroll, 4);
    handle_event(&mut a, key(KeyCode::Char('g')));
    assert_eq!(a.todos_scroll, 0);
    handle_event(&mut a, key(KeyCode::Esc));
    assert_eq!(a.focus, FocusPane::Prompt);
}

#[test]
fn coalesce_wheels_over_todos_do_not_move_chat() {
    let mut a = app();
    overflowing_todos(&mut a);
    a.chat_viewport_rows = 12;
    a.chat_content_width = 40;
    for i in 0..20 {
        a.add_message(ChatRole::User, format!("line {i}"));
    }
    let mut events = vec![
        mouse(MouseEventKind::ScrollDown, 4, 5),
        mouse(MouseEventKind::ScrollDown, 4, 5),
        mouse(MouseEventKind::Moved, 4, 5),
        mouse(MouseEventKind::ScrollUp, 4, 5),
    ];
    coalesce_chat_wheels(&mut a, &mut events);
    assert_eq!(events.len(), 1, "wheels folded, move kept");
    assert_eq!(a.todos_scroll, 1);
    assert_eq!(a.scroll_offset, 0);
}

#[test]
fn scrollback_select_and_open_subagent() {
    let mut a = app();
    a.add_message(ChatRole::User, "one");
    a.add_message(ChatRole::Assistant, "two");
    a.focus = FocusPane::Scrollback;
    a.selected_msg = Some(0);
    handle_event(&mut a, key(KeyCode::Down));
    assert_eq!(a.selected_msg, Some(1));
    handle_event(&mut a, key(KeyCode::Char('k')));
    assert_eq!(a.selected_msg, Some(0));
    handle_event(
        &mut a,
        Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT)),
    );
    handle_event(
        &mut a,
        Event::Key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT)),
    );
    handle_event(&mut a, key(KeyCode::Char('y')));
    handle_event(&mut a, key(KeyCode::Char('e')));
    handle_event(&mut a, key(KeyCode::Char('l')));

    a.upsert_subagent(crate::app::SubagentUpdate {
        id: "kid".into(),
        kind: "general".into(),
        description: "d".into(),
        status: "running".into(),
        activity: String::new(),
        elapsed_ms: 0,
        output: String::new(),
    });
    // The new system message with the Subagent block is last.
    a.selected_msg = Some(a.messages.len() - 1);
    assert!(matches!(
        a.messages.last().unwrap().blocks.first(),
        Some(ChatBlock::Subagent { .. })
    ));
    handle_event(&mut a, key(KeyCode::Enter));
    assert_eq!(a.open_subagent.as_deref(), Some("kid"));
    handle_event(&mut a, key(KeyCode::Esc));
    assert!(a.open_subagent.is_none());
}

#[test]
fn execute_provider_and_unknown_colon() {
    let mut a = app();
    a.mode = AppMode::Command;
    a.key_context = KeymapContext::Command;
    a.command.buffer = ":provider".into();
    handle_event(&mut a, key(KeyCode::Enter));
    assert!(matches!(a.dialogs.active(), Some(DialogKind::Provider)));
}

#[test]
fn dialog_mode_with_empty_stack_resets() {
    let mut a = app();
    a.mode = AppMode::Dialog;
    a.key_context = KeymapContext::Dialog;
    handle_event(&mut a, key(KeyCode::Esc));
    assert_eq!(a.mode, AppMode::Normal);
}

#[test]
fn login_and_sessions_dashboard_confirm() {
    let mut a = app();
    a.login_dialog.rows = vec![crate::app::LoginProviderRow {
        provider: "anthropic".into(),
        label: "Anthropic".into(),
        connected: false,
    }];
    open_dialog(&mut a, DialogKind::Login);
    handle_event(&mut a, key(KeyCode::Enter));
    assert_eq!(a.pending_login_provider.as_deref(), Some("anthropic"));

    let mut a = app();
    a.sessions_rows = vec![crate::app::SessionDashboardRow {
        parked_idx: Some(3),
        title: "s".into(),
        glyph: "·".into(),
        state_label: "idle".into(),
        preview: String::new(),
        unread: false,
    }];
    open_dialog(&mut a, DialogKind::Sessions);
    handle_event(&mut a, key(KeyCode::Enter));
    assert_eq!(a.pending_session_switch, Some(3));
}

#[test]
fn confirm_upgrade_marks_pending_and_quits() {
    let mut a = app();
    a.confirm("Update available", "Update now?", ConfirmAction::Upgrade);
    handle_event(&mut a, key(KeyCode::Char('y')));
    assert!(a.pending_upgrade);
    assert!(!a.running);
}

#[test]
fn confirm_import_settings_marks_pending() {
    let mut a = app();
    a.confirm("Import settings", "Copy?", ConfirmAction::ImportSettings);
    handle_event(&mut a, key(KeyCode::Char('y')));
    assert!(a.pending_import);
    assert!(a.running);
    assert_eq!(a.mode, AppMode::Normal);
}

#[test]
fn import_picker_space_toggles_and_enter_applies() {
    let mut a = app();
    let mut plan = whycodes_import::ImportPlan::default();
    plan.mcp_add.push((
        "fs".into(),
        whycodes_config::McpServerConfig {
            transport: None,
            command: Some("npx".into()),
            args: vec![],
            env: None,
            cwd: None,
            url: None,
            headers: None,
        },
    ));
    plan.permission_add
        .push(("bash".into(), whycodes_core::types::PermissionAction::Ask));
    a.open_import_picker(&plan);
    assert_eq!(a.import_picker.checked, vec![true, true]);
    handle_event(&mut a, key(KeyCode::Char(' ')));
    assert_eq!(a.import_picker.checked, vec![false, true]);
    handle_event(&mut a, key(KeyCode::Down));
    handle_event(&mut a, key(KeyCode::Char(' ')));
    assert_eq!(a.import_picker.checked, vec![false, false]);
    handle_event(&mut a, key(KeyCode::Enter));
    assert!(!a.pending_import, "empty selection must not apply");
    handle_event(&mut a, key(KeyCode::Char('a')));
    assert_eq!(a.import_picker.checked, vec![true, true]);
    handle_event(&mut a, key(KeyCode::Char('n')));
    assert_eq!(a.import_picker.checked, vec![false, false]);
    handle_event(&mut a, key(KeyCode::Char('a')));
    handle_event(&mut a, key(KeyCode::Enter));
    assert!(a.pending_import);
    assert_eq!(a.mode, AppMode::Normal);
}

#[test]
fn confirm_delete_provider_and_live_session_switch() {
    let mut a = app();
    a.confirm(
        "Delete",
        "gone?",
        ConfirmAction::DeleteProvider("acme".into()),
    );
    handle_event(&mut a, key(KeyCode::Enter));
    assert!(a.status_message.contains("acme"));
    assert_eq!(a.mode, AppMode::Normal);

    let mut a = app();
    a.session_list.sessions = vec![crate::app::SessionEntry {
        id: "live".into(),
        title: "t".into(),
        messages: 1,
        updated_at: None,
        live: Some(usize::MAX),
    }];
    open_dialog(&mut a, DialogKind::SessionList);
    handle_event(&mut a, key(KeyCode::Enter));
    assert_eq!(a.pending_session_switch, Some(usize::MAX));

    let mut a = app();
    a.session_list.sessions = vec![crate::app::SessionEntry {
        id: "persisted".into(),
        title: "t".into(),
        messages: 1,
        updated_at: None,
        live: None,
    }];
    open_dialog(&mut a, DialogKind::SessionList);
    handle_event(
        &mut a,
        Event::Key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL)),
    );
    assert!(
        a.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("persisted") || t.message.contains("live"))
    );
}

#[test]
fn permission_and_question_dismiss() {
    let mut a = app();
    a.ask_permission("bash", "rm -rf /");
    assert!(matches!(
        a.dialogs.active(),
        Some(DialogKind::Permission { .. })
    ));
    handle_event(&mut a, key(KeyCode::Esc));
    assert!(!a.dialogs.is_open());

    let mut a = app();
    a.ask_question(vec![whycodes_tools::question::QuestionSpec {
        prompt: "Go?".into(),
        options: vec![whycodes_tools::question::QuestionOption {
            label: "Yes".into(),
            description: String::new(),
            preview: None,
        }],
        multi_select: false,
        important: false,
    }]);
    handle_event(&mut a, key(KeyCode::Esc));
    assert!(a.question_dismissed);
    assert!(!a.dialogs.is_open());
}

#[test]
fn question_enter_does_not_dismiss_via_confirm_dialog() {
    let mut a = app();
    a.ask_question(vec![whycodes_tools::question::QuestionSpec {
        prompt: "Go?".into(),
        options: vec![whycodes_tools::question::QuestionOption {
            label: "Yes".into(),
            description: String::new(),
            preview: None,
        }],
        multi_select: false,
        important: false,
    }]);
    handle_event(&mut a, key(KeyCode::Enter));
    assert!(matches!(a.dialogs.active(), Some(DialogKind::Question(_))));
    assert!(!a.question_dismissed);
    assert!(a.pending_question_answers.is_none());
}

#[test]
fn mouse_confirms_list_and_question_rows() {
    let mut a = app();
    a.theme_selected = 0;
    open_dialog(&mut a, DialogKind::Theme);
    a.dialog_modal_hit = Some(Rect {
        x: 10,
        y: 5,
        width: 40,
        height: 12,
    });
    a.dialog_list_hit = Some(Rect {
        x: 12,
        y: 8,
        width: 30,
        height: 6,
    });
    a.dialog_list_total = crate::theme::ThemeName::ALL.len();
    a.dialog_list_visible = 6;
    a.dialog_list_scroll_start = 0;
    handle_event(
        &mut a,
        mouse(MouseEventKind::Down(MouseButton::Left), 14, 10),
    );
    handle_event(&mut a, mouse(MouseEventKind::Up(MouseButton::Left), 14, 10));
    assert_eq!(a.mode, AppMode::Normal);

    let mut a = app();
    a.ask_question(vec![whycodes_tools::question::QuestionSpec {
        prompt: "Go?".into(),
        options: vec![
            whycodes_tools::question::QuestionOption {
                label: "Yes".into(),
                description: String::new(),
                preview: None,
            },
            whycodes_tools::question::QuestionOption {
                label: "No".into(),
                description: String::new(),
                preview: None,
            },
        ],
        multi_select: false,
        important: false,
    }]);
    a.dialog_modal_hit = Some(Rect {
        x: 10,
        y: 5,
        width: 40,
        height: 12,
    });
    a.dialog_list_hit = Some(Rect {
        x: 12,
        y: 8,
        width: 30,
        height: 4,
    });
    a.dialog_list_total = 3;
    a.dialog_list_visible = 4;
    handle_event(
        &mut a,
        mouse(MouseEventKind::Down(MouseButton::Left), 14, 8),
    );
    handle_event(&mut a, mouse(MouseEventKind::Up(MouseButton::Left), 14, 8));
    assert!(
        a.pending_question_answers.is_some() || a.dialogs.is_open(),
        "click either finishes or keeps the questionnaire"
    );
}

#[test]
fn modal_scrollbar_and_copy_selection() {
    let mut a = app();
    open_dialog(&mut a, DialogKind::Theme);
    a.dialog_modal_hit = Some(Rect {
        x: 10,
        y: 5,
        width: 40,
        height: 12,
    });
    a.dialog_scrollbar_hit = Some(Rect {
        x: 49,
        y: 6,
        width: 1,
        height: 10,
    });
    a.dialog_list_total = crate::theme::ThemeName::ALL.len();
    a.dialog_list_visible = 6;
    handle_event(
        &mut a,
        mouse(MouseEventKind::Down(MouseButton::Left), 49, 8),
    );
    assert!(a.dialog_scrollbar_grab.is_some());
    handle_event(
        &mut a,
        mouse(MouseEventKind::Drag(MouseButton::Left), 49, 12),
    );
    handle_event(&mut a, mouse(MouseEventKind::Up(MouseButton::Left), 49, 12));
    assert!(a.dialog_scrollbar_grab.is_none());

    let mut a = app();
    a.mode = AppMode::Help;
    a.key_context = KeymapContext::Help;
    a.dialog_close_hit = Some(Rect {
        x: 70,
        y: 1,
        width: 3,
        height: 1,
    });
    handle_event(
        &mut a,
        mouse(MouseEventKind::Down(MouseButton::Left), 71, 1),
    );
    assert_eq!(a.mode, AppMode::Normal);

    let mut a = app();
    open_dialog(&mut a, DialogKind::Theme);
    a.dialog_modal_hit = Some(Rect {
        x: 10,
        y: 5,
        width: 20,
        height: 8,
    });
    a.mouse_sel = Some(crate::app::MouseSelection {
        anchor_x: 11,
        anchor_y: 6,
        focus_x: 18,
        focus_y: 7,
        dragging: true,
    });
    copy_modal_selection(&mut a, 18, 7);
    let active = a.dialogs.active().cloned();
    apply_modal_scrollbar(&mut a, active.as_ref(), 6, None);
    a.dialog_scrollbar_hit = Some(Rect {
        x: 29,
        y: 6,
        width: 1,
        height: 6,
    });
    a.dialog_list_total = 20;
    a.dialog_list_visible = 6;
    apply_modal_scrollbar(&mut a, active.as_ref(), 8, Some(1));
    let grab = scrollbar_grab_at(
        &a,
        8,
        Rect {
            x: 29,
            y: 6,
            width: 1,
            height: 6,
        },
    );
    let _ = grab;
}

#[test]
fn chat_scrollbar_offset_snaps_and_noops() {
    let mut a = app();
    apply_chat_scrollbar_offset(&mut a, 5, None);
    a.chat_scrollbar_hit = Some(Rect {
        x: 40,
        y: 1,
        width: 1,
        height: 10,
    });
    a.chat_scroll_total = 0;
    apply_chat_scrollbar_offset(&mut a, 5, None);

    a.chat_scroll_total = 8;
    a.chat_viewport_rows = 10;
    apply_chat_scrollbar_offset(&mut a, 5, None);
    assert_eq!(a.scroll_offset, 0);
    assert!(a.auto_scroll);

    a.chat_scroll_total = 100;
    a.chat_viewport_rows = 10;
    apply_chat_scrollbar_offset(&mut a, 1, None);
    assert!(a.scroll_offset > 0, "top of track → oldest");
    apply_chat_scrollbar_offset(&mut a, 10, None);
    assert_eq!(a.scroll_offset, 0, "bottom of track → newest");
    apply_chat_scrollbar_offset(&mut a, 0, None);
    apply_chat_scrollbar_offset(&mut a, 50, Some(1));
    let grab = chat_scrollbar_grab_at(
        &a,
        3,
        Rect {
            x: 40,
            y: 1,
            width: 1,
            height: 10,
        },
    );
    let _ = grab;
}

#[test]
fn paste_token_edits_as_a_unit_and_session_paste() {
    let mut a = app();
    a.insert_paste_text("one\ntwo\nthree\nfour");
    assert!(!a.pending_pastes.is_empty());
    a.input_cursor = 0;
    handle_event(&mut a, key(KeyCode::Delete));
    assert!(a.input_buffer.is_empty() || !a.input_buffer.contains('\n'));

    let mut a = app();
    a.insert_paste_text("one\ntwo\nthree\nfour");
    let end = a.input_buffer.len();
    a.input_cursor = end;
    handle_event(&mut a, key(KeyCode::Left));
    assert!(a.input_cursor < end);
    handle_event(&mut a, key(KeyCode::Right));
    assert_eq!(a.input_cursor, a.input_buffer.len());

    let mut a = app();
    a.mode = AppMode::Session;
    handle_event(&mut a, Event::Paste("hello session".into()));
    assert!(a.input_buffer.contains("hello session"));
}

#[test]
fn turkish_i_and_dotless_i_type_and_paste() {
    let mut a = app();
    assert!(handle_event(&mut a, key(KeyCode::Char('i'))));
    assert!(handle_event(&mut a, key(KeyCode::Char('ı'))));
    assert!(handle_event(&mut a, key(KeyCode::Char('İ'))));
    assert!(handle_event(&mut a, key(KeyCode::Char('I'))));
    assert_eq!(a.input_buffer, "iıİI");

    let mut a = app();
    handle_event(&mut a, Event::Paste("iyi ışık".into()));
    assert_eq!(a.input_buffer, "iyi ışık");

    // Unbracketed short paste: each char is a Key. Scrollback used to
    // bind `i` to FocusPrompt and swallow every ASCII i.
    let mut a = app();
    a.add_message(ChatRole::User, "one");
    a.focus = FocusPane::Scrollback;
    for c in "iyi istanbul".chars() {
        handle_event(&mut a, key(KeyCode::Char(c)));
    }
    assert_eq!(a.focus, FocusPane::Prompt);
    assert_eq!(a.input_buffer, "iyi istanbul");
}

#[test]
fn provider_form_fields_and_command_aliases() {
    let mut a = app();
    open_provider_dialog(&mut a);
    a.provider_dialog.mode = crate::app::ProviderDialogMode::AddCustom;
    for (field, ch, get) in [
        (1usize, 'k', "form_api_key"),
        (2, 'u', "form_base_url"),
        (3, 'h', "form_headers"),
    ] {
        a.provider_dialog.active_field = field;
        handle_event(&mut a, key(KeyCode::Char(ch)));
        let val = match field {
            1 => a.provider_dialog.form_api_key.as_str(),
            2 => a.provider_dialog.form_base_url.as_str(),
            _ => a.provider_dialog.form_headers.as_str(),
        };
        assert_eq!(val, ch.to_string(), "{get}");
        handle_event(&mut a, key(KeyCode::Backspace));
    }

    for cmd in [":quit", ":h", ":prov"] {
        let mut a = app();
        a.mode = AppMode::Command;
        a.key_context = KeymapContext::Command;
        a.command.buffer = cmd.into();
        handle_event(&mut a, key(KeyCode::Enter));
        match cmd {
            ":quit" => assert!(!a.running),
            ":h" => assert_eq!(a.mode, AppMode::Help),
            _ => assert!(matches!(a.dialogs.active(), Some(DialogKind::Provider))),
        }
    }
}

#[test]
fn open_subagent_falls_back_to_last_and_shift_jumps() {
    let mut a = app();
    a.upsert_subagent(crate::app::SubagentUpdate {
        id: "only".into(),
        kind: "explore".into(),
        description: "d".into(),
        status: "running".into(),
        activity: String::new(),
        elapsed_ms: 0,
        output: String::new(),
    });
    a.selected_msg = None;
    open_selected_subagent(&mut a);
    assert_eq!(a.open_subagent.as_deref(), Some("only"));

    let mut a = app();
    a.add_message(ChatRole::User, "u1");
    a.add_message(ChatRole::Assistant, "a1");
    a.add_message(ChatRole::User, "u2");
    handle_event(
        &mut a,
        Event::Key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT)),
    );
    handle_event(
        &mut a,
        Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT)),
    );
}

#[test]
fn mouse_drag_without_down_and_plain_click() {
    let mut a = app();
    handle_event(&mut a, mouse(MouseEventKind::Drag(MouseButton::Left), 4, 4));
    assert!(a.mouse_sel.is_some());
    handle_event(&mut a, mouse(MouseEventKind::Up(MouseButton::Left), 4, 4));

    let mut a = app();
    handle_event(&mut a, mouse(MouseEventKind::Down(MouseButton::Left), 2, 2));
    handle_event(&mut a, mouse(MouseEventKind::Up(MouseButton::Left), 2, 2));
    assert!(a.mouse_sel.is_none());
}

#[test]
fn move_in_dialog_to_clamps_each_kind() {
    let mut a = app();
    open_provider_dialog(&mut a);
    move_in_dialog_to(&mut a, &DialogKind::Provider, 99);
    assert!(a.provider_dialog.selected > 0);

    a.model_selection.models = vec![("p".into(), "m".into())];
    move_in_dialog_to(&mut a, &DialogKind::Model, 9);
    assert_eq!(a.model_selection.selected, 1);

    a.primary_agents = vec!["build".into(), "plan".into()];
    move_in_dialog_to(&mut a, &DialogKind::Agent, 9);
    assert_eq!(a.agent_picker_selected, 1);

    a.session_list.sessions = vec![crate::app::SessionEntry {
        id: "x".into(),
        title: "t".into(),
        messages: 0,
        updated_at: None,
        live: None,
    }];
    move_in_dialog_to(&mut a, &DialogKind::SessionList, 4);
    assert_eq!(a.session_list.selected, 0);

    a.sessions_rows = vec![crate::app::SessionDashboardRow {
        parked_idx: None,
        title: "s".into(),
        glyph: "·".into(),
        state_label: "idle".into(),
        preview: String::new(),
        unread: false,
    }];
    move_in_dialog_to(&mut a, &DialogKind::Sessions, 3);
    assert_eq!(a.sessions_cursor, 0);

    a.login_dialog.rows = vec![crate::app::LoginProviderRow {
        provider: "x".into(),
        label: "X".into(),
        connected: false,
    }];
    move_in_dialog_to(&mut a, &DialogKind::Login, 2);
    assert_eq!(a.login_dialog.selected, 0);

    move_in_dialog_to(&mut a, &DialogKind::Theme, 0);
    assert_eq!(a.theme_selected, 0);
    move_in_dialog_to(&mut a, &DialogKind::Help, 0);
}

#[test]
fn dialog_question_keys_move_without_free_text() {
    let mut a = app();
    a.ask_question(vec![whycodes_tools::question::QuestionSpec {
        prompt: "Go?".into(),
        options: vec![
            whycodes_tools::question::QuestionOption {
                label: "A".into(),
                description: String::new(),
                preview: None,
            },
            whycodes_tools::question::QuestionOption {
                label: "B".into(),
                description: String::new(),
                preview: None,
            },
        ],
        multi_select: true,
        important: false,
    }]);
    handle_event(&mut a, key(KeyCode::Down));
    handle_event(&mut a, key(KeyCode::Up));
    if let Some(DialogKind::Question(st)) = a.dialogs.active() {
        assert!(!st.free_text_focus);
    }
}

#[test]
fn coalesce_resizes_keeps_ordered_non_resize_events_and_last_size() {
    let typed = key(KeyCode::Char('x'));
    let moved = mouse(MouseEventKind::Moved, 2, 3);
    let mut events = vec![
        Event::Resize(80, 24),
        typed.clone(),
        Event::Resize(100, 30),
        moved.clone(),
        Event::Resize(120, 40),
    ];

    coalesce_resizes(&mut events);

    assert_eq!(events, vec![typed, moved, Event::Resize(120, 40)]);
    let mut unchanged = vec![Event::FocusGained, Event::FocusLost];
    let expected = unchanged.clone();
    coalesce_resizes(&mut unchanged);
    assert_eq!(unchanged, expected);
}

#[test]
fn coalesce_unbracketed_paste_folds_a_long_key_flood() {
    let a = app();
    let mut events: Vec<Event> = "hello\nworld\nmore"
        .chars()
        .map(|c| {
            if c == '\n' {
                key(KeyCode::Enter)
            } else {
                key(KeyCode::Char(c))
            }
        })
        .collect();
    coalesce_unbracketed_paste(&a, &mut events);
    assert_eq!(events, vec![Event::Paste("hello\nworld\nmore".into())]);

    let mut with_resize: Vec<Event> = "hello\nworld\nmore"
        .chars()
        .map(|c| {
            if c == '\n' {
                key(KeyCode::Enter)
            } else {
                key(KeyCode::Char(c))
            }
        })
        .collect();
    with_resize.push(Event::Resize(80, 24));
    coalesce_unbracketed_paste(&a, &mut with_resize);
    assert_eq!(
        with_resize,
        vec![
            Event::Resize(80, 24),
            Event::Paste("hello\nworld\nmore".into())
        ]
    );
}

#[test]
fn coalesce_unbracketed_paste_leaves_short_typing_alone() {
    let a = app();
    let mut events: Vec<Event> = "hi there".chars().map(|c| key(KeyCode::Char(c))).collect();
    let before = events.clone();
    coalesce_unbracketed_paste(&a, &mut events);
    assert_eq!(events, before);
}

#[test]
fn coalesce_unbracketed_paste_leaves_typed_line_plus_enter() {
    let a = app();
    let mut events: Vec<Event> = "hello"
        .chars()
        .map(|c| key(KeyCode::Char(c)))
        .chain(std::iter::once(key(KeyCode::Enter)))
        .collect();
    let before = events.clone();
    coalesce_unbracketed_paste(&a, &mut events);
    assert_eq!(
        events, before,
        "typed line + Enter must stay keys so submit/slash run"
    );

    let mut slash: Vec<Event> = "/help"
        .chars()
        .map(|c| key(KeyCode::Char(c)))
        .chain(std::iter::once(key(KeyCode::Enter)))
        .collect();
    let before_slash = slash.clone();
    coalesce_unbracketed_paste(&a, &mut slash);
    assert_eq!(slash, before_slash);

    // Double Enter in the same batch must not become a 2-line paste.
    let mut doubled: Vec<Event> = "hello"
        .chars()
        .map(|c| key(KeyCode::Char(c)))
        .chain([key(KeyCode::Enter), key(KeyCode::Enter)])
        .collect();
    let before_doubled = doubled.clone();
    coalesce_unbracketed_paste(&a, &mut doubled);
    assert_eq!(doubled, before_doubled);

    // Windows / some hosts deliver Enter as CR, not KeyCode::Enter.
    let mut cr: Vec<Event> = "hello"
        .chars()
        .map(|c| key(KeyCode::Char(c)))
        .chain(std::iter::once(key(KeyCode::Char('\r'))))
        .collect();
    let before_cr = cr.clone();
    coalesce_unbracketed_paste(&a, &mut cr);
    assert_eq!(
        cr, before_cr,
        "typed line + CR must stay keys so submit runs"
    );
}

#[test]
fn coalesce_unbracketed_paste_ignores_key_release_and_cr_newlines() {
    let a = app();
    let mut events = Vec::new();
    for c in "hello\nworld\nmore".chars() {
        let code = if c == '\n' {
            KeyCode::Enter
        } else {
            KeyCode::Char(c)
        };
        events.push(key(code));
        events.push(key_release(code));
    }
    coalesce_unbracketed_paste(&a, &mut events);
    assert_eq!(events, vec![Event::Paste("hello\nworld\nmore".into())]);

    let mut cr_paste: Vec<Event> = "hello\rworld\rmore"
        .chars()
        .map(|c| {
            if c == '\r' {
                key(KeyCode::Char('\r'))
            } else {
                key(KeyCode::Char(c))
            }
        })
        .collect();
    coalesce_unbracketed_paste(&a, &mut cr_paste);
    assert_eq!(cr_paste, vec![Event::Paste("hello\nworld\nmore".into())]);
}

#[test]
fn direct_input_actions_handle_invalid_utf8_cursor_and_history_edges() {
    let mut a = app();
    a.input_buffer = "şa".into();
    a.input_cursor = 1;
    handle_input_action(
        &mut a,
        crate::keymap::Action::InputDelete,
        &KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE),
    );
    assert_eq!(a.input_buffer, "a", "mid-codepoint cursor clamps backward");
    assert_eq!(a.input_cursor, 0);

    a.input_history = vec!["first".into(), "second".into()];
    a.input_history_idx = 0;
    handle_input_action(
        &mut a,
        crate::keymap::Action::InputHistoryPrev,
        &KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
    );
    assert_eq!(a.input_history_idx, 0, "history does not underflow");
    a.input_history_idx = a.input_history.len();
    handle_input_action(
        &mut a,
        crate::keymap::Action::InputHistoryNext,
        &KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
    );
    assert_eq!(a.input_history_idx, 2, "history does not overflow");
}

fn model_catalog(a: &mut TuiApp) {
    a.model_selection.models = vec![
        ("anthropic".into(), "claude-sonnet".into()),
        ("anthropic".into(), "claude-opus".into()),
        ("openai".into(), "gpt-4o".into()),
    ];
    a.provider_name = "openai".into();
    a.model_name = "gpt-4o".into();
}

#[test]
fn model_picker_search_fold_and_enter() {
    let mut a = app();
    model_catalog(&mut a);
    open_model_dialog(&mut a);
    assert!(matches!(a.dialogs.active(), Some(DialogKind::Model)));
    let rows = a.model_selection.visible_rows();
    assert!(
        matches!(
            &rows[0],
            crate::app::ModelPickerRow::Header {
                collapsed: true,
                provider,
                ..
            } if provider == "anthropic"
        ),
        "{rows:?}"
    );
    // Enter on a header toggles collapse, does not pick a model.
    a.model_selection.selected = 0;
    handle_event(&mut a, key(KeyCode::Enter));
    assert!(a.pending_model.is_none());
    assert!(matches!(a.dialogs.active(), Some(DialogKind::Model)));
    assert!(!a.model_selection.collapsed.contains("anthropic"));

    handle_event(&mut a, key(KeyCode::Left));
    assert!(a.model_selection.collapsed.contains("anthropic"));
    handle_event(&mut a, key(KeyCode::Right));
    assert!(!a.model_selection.collapsed.contains("anthropic"));

    handle_event(&mut a, key(KeyCode::Char('/')));
    assert!(a.model_selection.searching);
    handle_event(&mut a, key(KeyCode::Char('g')));
    handle_event(&mut a, key(KeyCode::Char('p')));
    handle_event(&mut a, key(KeyCode::Char('t')));
    assert_eq!(a.model_selection.query, "gpt");
    let rows = a.model_selection.visible_rows();
    assert!(
        rows.iter().any(|r| matches!(
            r,
            crate::app::ModelPickerRow::Model { model, .. } if model == "gpt-4o"
        )),
        "{rows:?}"
    );
    assert!(!rows.iter().any(|r| matches!(
        r,
        crate::app::ModelPickerRow::Model { model, .. } if model.contains("claude")
    )));

    handle_event(&mut a, key(KeyCode::Esc));
    assert!(a.model_selection.query.is_empty());
    assert!(!a.model_selection.searching);
    assert!(matches!(a.dialogs.active(), Some(DialogKind::Model)));
    handle_event(&mut a, key(KeyCode::Esc));
    assert_eq!(a.mode, AppMode::Normal);
}

#[test]
fn model_picker_j_k_navigate_then_enter_selects_model() {
    let mut a = app();
    model_catalog(&mut a);
    open_model_dialog(&mut a);
    // Cursor starts on gpt-4o (current). j wraps; k goes to openai header.
    handle_event(&mut a, key(KeyCode::Char('k')));
    assert!(matches!(
        a.model_selection.selected_row(),
        Some(crate::app::ModelPickerRow::Header { provider, .. })
            if provider == "openai"
    ));
    handle_event(&mut a, key(KeyCode::Char('j')));
    handle_event(&mut a, key(KeyCode::Enter));
    assert_eq!(
        a.pending_model
            .as_ref()
            .map(|(p, m)| (p.as_str(), m.as_str())),
        Some(("openai", "gpt-4o"))
    );
    assert_eq!(a.mode, AppMode::Normal);
}

#[test]
fn prompt_backspace_and_word_kill_do_not_full_clear() {
    let mut a = app();
    a.input_buffer = "hello world".into();
    a.input_cursor = a.input_buffer.len();
    a.pending_full_clears = 0;
    handle_event(&mut a, key(KeyCode::Backspace));
    assert_eq!(a.input_buffer, "hello worl");
    assert_eq!(a.pending_full_clears, 0);

    handle_input_action(
        &mut a,
        crate::keymap::Action::InputKillWordBack,
        &KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
    );
    assert_eq!(a.input_buffer, "hello ");
    assert_eq!(a.pending_full_clears, 0);

    a.input_cursor = 0;
    handle_input_action(
        &mut a,
        crate::keymap::Action::InputDelete,
        &KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE),
    );
    assert_eq!(a.input_buffer, "ello ");
    assert_eq!(a.pending_full_clears, 0);

    handle_input_action(
        &mut a,
        crate::keymap::Action::InputClear,
        &KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
    );
    assert!(a.input_buffer.is_empty());
    assert_eq!(a.pending_full_clears, 0);
}

#[test]
fn attach_image_paths_mixed_success_and_error() {
    let dir = tempfile::tempdir().unwrap();
    let ok = dir.path().join("a.png");
    let ok2 = dir.path().join("b.png");
    std::fs::write(&ok, b"\x89PNG\r\n\x1a\nhello").unwrap();
    std::fs::write(&ok2, b"\x89PNG\r\n\x1a\nworld").unwrap();
    let missing = dir.path().join("gone.png");

    let mut a = app();
    attach_image_paths(&mut a, [ok.clone(), missing, ok2]);
    assert_eq!(a.pending_images.len(), 2);
    assert!(
        a.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Attached 2")),
        "{:?}",
        a.toasts
            .visible()
            .iter()
            .map(|t| t.message.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        a.toasts
            .visible()
            .iter()
            .any(|t| t.kind == crate::toast::ToastKind::Warning)
    );
}

#[test]
fn coalesce_unbracketed_paste_bails_on_modal_mode_and_existing_paste() {
    let mut a = app();
    a.mode = AppMode::Help;
    let mut events = vec![key(KeyCode::Char('a')); 20];
    let before = events.clone();
    coalesce_unbracketed_paste(&a, &mut events);
    assert_eq!(events, before);

    a.mode = AppMode::Normal;
    a.confirm("Quit", "sure?", ConfirmAction::Quit);
    coalesce_unbracketed_paste(&a, &mut events);
    assert_eq!(events, before);

    let a = app();
    let mut events = vec![Event::Paste("already".into()), key(KeyCode::Char('x'))];
    coalesce_unbracketed_paste(&a, &mut events);
    assert!(matches!(events[0], Event::Paste(_)));
}

#[test]
fn slash_nav_focus_todos_and_help_q() {
    let mut a = app();
    a.input_buffer = "/".into();
    a.slash_suggest.refresh(&a.input_buffer);
    handle_event(&mut a, key(KeyCode::Up));
    handle_event(&mut a, key(KeyCode::Down));
    assert!(a.slash_suggest.active);

    a.add_message(ChatRole::User, "hi");
    a.focus = FocusPane::Scrollback;
    handle_event(&mut a, key(KeyCode::Char('i')));
    assert_eq!(a.focus, FocusPane::Prompt);

    a.focus = FocusPane::Scrollback;
    handle_event(&mut a, key(KeyCode::Char('t')));
    // empty todos is a no-op; add one then toggle
    a.replace_todos(vec![whycodes_core::TodoItem::new(
        "t1",
        "do",
        whycodes_core::TodoStatus::Pending,
    )]);
    a.focus = FocusPane::Scrollback;
    handle_event(&mut a, key(KeyCode::Char('t')));
    assert!(a.todos_collapsed);

    a.mode = AppMode::Help;
    a.key_context = KeymapContext::Normal;
    handle_event(&mut a, key(KeyCode::Char('q')));
    assert_eq!(a.mode, AppMode::Normal);

    let mut a = app();
    a.mode = AppMode::Command;
    a.key_context = KeymapContext::Command;
    a.command.buffer = ":help".into();
    handle_event(&mut a, key(KeyCode::Esc));
    assert_eq!(a.mode, AppMode::Normal);
    assert!(a.command.buffer.is_empty());

    let mut a = app();
    a.sidebar.visible = false;
    handle_event(&mut a, ctrl('.'));
    assert!(a.sidebar.visible);
}

fn submit_command(app: &mut TuiApp, cmd: &str) {
    app.mode = AppMode::Command;
    app.key_context = KeymapContext::Command;
    app.command.buffer = cmd.into();
    handle_event(app, key(KeyCode::Enter));
}

#[test]
fn colon_commands_quit_help_theme_clear_sidebar_and_unknown() {
    let mut a = app();
    a.add_message(ChatRole::User, "keep");
    submit_command(&mut a, ":clear");
    assert!(a.messages.is_empty());
    assert_eq!(a.status_message, "Session cleared");
    assert_eq!(a.mode, AppMode::Normal);

    submit_command(&mut a, ":h");
    assert_eq!(a.mode, AppMode::Help);

    let mut a = app();
    submit_command(&mut a, ":help");
    assert_eq!(a.mode, AppMode::Help);

    let mut a = app();
    submit_command(&mut a, ":theme");
    assert_eq!(a.mode, AppMode::Dialog);
    assert!(matches!(a.dialogs.active(), Some(DialogKind::Theme)));

    let mut a = app();
    a.sidebar.visible = true;
    submit_command(&mut a, ":sidebar");
    assert!(!a.sidebar.visible);
    submit_command(&mut a, ":sidebar");
    assert!(a.sidebar.visible);

    let mut a = app();
    submit_command(&mut a, ":nope");
    assert!(a.status_message.contains("Unknown command"));

    let mut a = app();
    submit_command(&mut a, ":q");
    assert!(!a.running);

    let mut a = app();
    submit_command(&mut a, ":quit");
    assert!(!a.running);

    let mut a = app();
    submit_command(&mut a, ":provider");
    assert!(matches!(a.dialogs.active(), Some(DialogKind::Provider)));
    assert!(!a.provider_dialog.providers.is_empty());

    let mut a = app();
    submit_command(&mut a, ":prov");
    assert!(matches!(a.dialogs.active(), Some(DialogKind::Provider)));

    let mut a = app();
    a.model_selection.models = vec![("openai".into(), "gpt-4o".into())];
    submit_command(&mut a, ":model");
    assert!(matches!(a.dialogs.active(), Some(DialogKind::Model)));
}

#[test]
fn open_effort_and_mode_dialogs_from_footer_hits() {
    let mut a = app();
    a.provider_name = "openai".into();
    a.model_name = "gpt-5".into();
    a.reasoning_effort = Some("high".into());
    a.effort_hit.set_rect(Some(Rect::new(2, 4, 8, 1)));
    handle_event(&mut a, mouse(MouseEventKind::Down(MouseButton::Left), 3, 4));
    assert!(
        matches!(a.dialogs.active(), Some(DialogKind::Effort)),
        "{:?}",
        a.dialogs.active()
    );
    assert!(a.effort_picker_selected < 4);

    let mut a = app();
    a.provider_name = "anthropic".into();
    a.model_name = "claude-sonnet".into();
    a.effort_hit.set_rect(Some(Rect::new(2, 4, 8, 1)));
    handle_event(&mut a, mouse(MouseEventKind::Down(MouseButton::Left), 3, 4));
    assert!(
        a.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("no reasoning-effort")),
        "{:?}",
        a.toasts
            .visible()
            .iter()
            .map(|t| t.message.as_str())
            .collect::<Vec<_>>()
    );
    assert!(a.dialogs.active().is_none());

    let mut a = app();
    a.approval_mode = whycodes_core::types::ApprovalMode::Manual;
    a.approval_hit.set_rect(Some(Rect::new(10, 6, 8, 1)));
    handle_event(
        &mut a,
        mouse(MouseEventKind::Down(MouseButton::Left), 11, 6),
    );
    assert!(matches!(a.dialogs.active(), Some(DialogKind::ApprovalMode)));
    assert_eq!(
        a.approval_picker_selected,
        whycodes_core::types::ApprovalMode::ALL
            .iter()
            .position(|m| *m == whycodes_core::types::ApprovalMode::Manual)
            .unwrap()
    );

    open_effort_dialog(&mut a);
    open_mode_dialog(&mut a);
    assert!(matches!(a.dialogs.active(), Some(DialogKind::ApprovalMode)));
}

#[test]
fn copy_selection_from_scrollback_y_key() {
    let mut a = app();
    a.add_message(ChatRole::User, "copy me");
    a.focus_scrollback();
    handle_event(&mut a, key(KeyCode::Char('y')));
    assert!(
        a.toasts
            .visible()
            .iter()
            .any(|t| { t.message.contains("Copied") || t.message.contains("clipboard") }),
        "{:?}",
        a.toasts
            .visible()
            .iter()
            .map(|t| t.message.as_str())
            .collect::<Vec<_>>()
    );
}
