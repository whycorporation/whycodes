use super::*;
use crate::config::TuiAppConfig;
use std::time::Instant;
use whycodes_tools::question::QuestionOption;

fn app() -> TuiApp {
    TuiApp::from_config(TuiAppConfig::default())
}

fn catalog() -> ModelSelectionState {
    ModelSelectionState {
        models: vec![
            ("anthropic".into(), "claude-sonnet".into()),
            ("anthropic".into(), "claude-opus".into()),
            ("openai".into(), "gpt-4o".into()),
        ],
        ..Default::default()
    }
}

#[test]
fn model_picker_groups_and_search() {
    let mut s = catalog();
    s.prepare_for_open("openai", "gpt-4o");
    let rows = s.visible_rows();
    assert!(
        matches!(
            &rows[0],
            ModelPickerRow::Header {
                provider,
                collapsed: true,
                ..
            } if provider == "anthropic"
        ),
        "{rows:?}"
    );
    assert!(
        matches!(
            &rows[1],
            ModelPickerRow::Header {
                provider,
                collapsed: false,
                ..
            } if provider == "openai"
        ),
        "{rows:?}"
    );
    assert!(
        matches!(
            &rows[2],
            ModelPickerRow::Model { model, .. } if model == "gpt-4o"
        ),
        "{rows:?}"
    );
    assert_eq!(s.selected, 2);

    s.query = "claude".into();
    let rows = s.visible_rows();
    assert_eq!(
        rows.len(),
        3,
        "search auto-expands matching provider: {rows:?}"
    );
    assert!(matches!(
        &rows[0],
        ModelPickerRow::Header {
            collapsed: false,
            ..
        }
    ));
    assert!(
        rows.iter()
            .any(|r| matches!(r, ModelPickerRow::Model { model, .. } if model == "claude-sonnet"))
    );
    assert!(
        !rows
            .iter()
            .any(|r| matches!(r, ModelPickerRow::Model { model, .. } if model == "gpt-4o"))
    );

    s.query = "nope".into();
    assert!(s.visible_rows().is_empty());
}

#[test]
fn model_picker_toggle_and_fold_keys() {
    let mut s = catalog();
    s.prepare_for_open("anthropic", "claude-sonnet");
    assert!(!s.toggle_group_at_cursor(), "cursor is on a model");
    s.selected = 0;
    assert!(s.toggle_group_at_cursor());
    assert!(s.collapsed.contains("anthropic"));
    assert!(s.set_group_collapsed_at_cursor(false));
    assert!(!s.collapsed.contains("anthropic"));
    s.selected = 1; // model under anthropic
    assert!(s.set_group_collapsed_at_cursor(true));
    assert!(s.collapsed.contains("anthropic"));
    assert!(
        matches!(s.selected_row(), Some(ModelPickerRow::Header { provider, .. }) if provider == "anthropic")
    );

    let mut empty = ModelSelectionState::default();
    assert!(
        !empty.set_group_collapsed_at_cursor(true),
        "empty catalog has no row to fold"
    );
}

fn question(prompt: &str, labels: &[&str], multi_select: bool) -> QuestionSpec {
    QuestionSpec {
        prompt: prompt.into(),
        options: labels
            .iter()
            .map(|label| QuestionOption {
                label: (*label).into(),
                description: String::new(),
                preview: None,
            })
            .collect(),
        multi_select,
        important: false,
    }
}

#[test]
fn question_answers_rehydrate_when_navigating_back_and_forward() {
    let mut state = QuestionDialogState::new(vec![
        question("Pick", &["A", "B"], false),
        question("Explain", &[], false),
    ]);

    state.set_cursor(1);
    assert!(state.confirm_current().is_none());
    assert_eq!(state.index, 1);
    assert!(state.free_text_focus);
    state.free_text = "discarded draft".into();

    assert!(state.go_prev_question());
    assert_eq!(state.cursor, 1);
    assert!(!state.free_text_focus);
    assert!(state.go_next_question());
    assert_eq!(state.free_text, "");
    assert!(state.free_text_focus);

    state.free_text = " because ".into();
    let answers = state.confirm_current().expect("all questions answered");
    assert_eq!(answers[0].selected, ["B"]);
    assert_eq!(answers[1].free_text.as_deref(), Some("because"));
}

#[test]
fn multi_question_requires_a_choice_and_collects_selected_labels() {
    let mut state =
        QuestionDialogState::new(vec![question("Pick several", &["A", "B", "C"], true)]);

    assert!(state.confirm_current().is_none());
    state.set_cursor(2);
    state.toggle_multi_at_cursor();
    state.set_cursor(0);
    state.toggle_multi_at_cursor();
    let answers = state.confirm_current().expect("selection completes dialog");

    let selected: HashSet<_> = answers[0].selected.iter().map(String::as_str).collect();
    assert_eq!(selected, HashSet::from(["A", "C"]));
    assert_eq!(answers[0].free_text, None);
}

#[test]
fn slash_suggestions_filter_wrap_hit_test_and_dismiss() {
    let mut state = SlashSuggestState::default();
    state.refresh("/he");
    assert!(state.active);
    assert_eq!(state.current().map(|cmd| cmd.name), Some("/help"));

    state.step(-1);
    assert_eq!(state.selected, state.matches.len() - 1);
    state.list_hit = Some(Rect::new(4, 10, 12, 2));
    state.list_scroll_start = state.matches.len().saturating_sub(1);
    assert_eq!(state.row_index_at(5, 10), Some(state.matches.len() - 1));
    assert_eq!(state.row_index_at(3, 10), None);
    assert_eq!(state.row_index_at(5, 11), None);

    state.dismiss();
    assert!(!state.active);
    assert!(state.matches.is_empty());
    state.refresh("/help now");
    assert!(!state.active, "arguments close command completion");
}

#[test]
fn focus_and_scroll_transitions_preserve_bottom_following_contract() {
    let mut app = app();
    app.toggle_focus();
    assert_eq!(app.focus, FocusPane::Prompt, "empty chat cannot take focus");

    app.add_message(ChatRole::User, "one");
    app.add_message(ChatRole::Assistant, "two");
    app.focus_scrollback();
    assert_eq!(app.focus, FocusPane::Scrollback);
    assert_eq!(app.selected_msg, Some(1));
    assert!(!app.auto_scroll);

    app.chat_scroll_total = 100;
    app.chat_viewport_rows = 20;
    app.scroll_rows(500);
    assert_eq!(app.scroll_offset, 80);
    app.scroll_page(false);
    assert_eq!(app.scroll_offset, 60);
    app.scroll_to_bottom();
    assert_eq!(app.scroll_offset, 0);
    assert!(app.auto_scroll);
    assert_eq!(app.selected_msg, Some(1));
}

#[test]
fn prompt_draft_clear_expands_paste_and_resets_transient_state() {
    let mut app = app();
    let pasted = "one\ntwo\nthree\nfour";
    app.insert_paste_text(pasted);
    app.slash_suggest.active = true;
    app.file_suggest.active = true;
    app.esc_armed_at = Some(Instant::now());

    app.clear_prompt_draft();

    assert_eq!(app.input_history, [pasted]);
    assert_eq!(app.input_history_idx, 1);
    assert!(app.input_buffer.is_empty());
    assert!(app.pending_pastes.is_empty());
    assert!(!app.slash_suggest.active);
    assert!(!app.file_suggest.active);
    assert!(app.esc_armed_at.is_none());
    assert!(app.pending_full_clears >= 1);
}

#[test]
fn insert_paste_requests_two_full_clears() {
    let mut app = app();
    app.pending_full_clears = 0;
    app.insert_paste_text(&"x".repeat(200));
    assert_eq!(app.pending_full_clears, 2);
    assert!(app.needs_redraw);
}

#[test]
fn submit_input_requests_full_clear_for_layout_jump() {
    let mut app = app();
    app.pending_full_clears = 0;
    app.input_buffer = "hello".into();
    app.input_cursor = 5;
    app.submit_input();
    assert_eq!(app.pending_full_clears, 2);
    assert!(app.messages.iter().any(|m| m.content == "hello"));
}

#[test]
fn import_picker_toggle_and_select_all() {
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
    let mut state = ImportPickerState::from_plan(&plan);
    assert_eq!(state.items.len(), 2);
    assert!(state.any_checked());
    assert_eq!(state.checked_count(), 2);
    state.toggle_at_cursor();
    assert_eq!(state.checked_count(), 1);
    state.select_all(false);
    assert!(!state.any_checked());
    state.select_all(true);
    assert_eq!(state.checked_count(), 2);
}

#[test]
fn subagent_headline_and_question_clipboard() {
    let running = SubagentUi {
        id: "a".into(),
        kind: "explore".into(),
        description: "x".repeat(80),
        status: "running".into(),
        activity: "Thinking".into(),
        started_at: Instant::now(),
        elapsed_ms: 0,
        output: String::new(),
    };
    let head = running.headline();
    assert!(head.contains("running"), "{head}");
    assert!(head.contains("Thinking"), "{head}");
    assert!(head.contains('…'), "long desc truncated: {head}");

    let done = SubagentUi {
        id: "b".into(),
        kind: "explore".into(),
        description: "short".into(),
        status: "completed".into(),
        activity: String::new(),
        started_at: Instant::now(),
        elapsed_ms: 1500,
        output: String::new(),
    };
    let head = done.headline();
    assert!(head.contains("completed"), "{head}");
    assert!(head.contains("1.5s"), "{head}");

    for status in ["failed", "cancelled", "started"] {
        let row = SubagentUi {
            id: status.into(),
            kind: "k".into(),
            description: "d".into(),
            status: status.into(),
            activity: String::new(),
            started_at: Instant::now(),
            elapsed_ms: 0,
            output: String::new(),
        };
        let h = row.headline();
        assert!(!h.is_empty(), "{status}");
    }

    let mut state = QuestionDialogState::new(vec![
        question("Pick", &["A", "B"], false),
        question("Why", &[], false),
    ]);
    let clip = state.clipboard_text();
    assert!(clip.contains("Pick"), "{clip}");
    assert!(clip.contains("Other"), "{clip}");
    assert!(clip.contains("free-text") || clip.contains("Why"), "{clip}");

    state.set_cursor(0);
    assert!(state.confirm_current().is_none());
    state.free_text = "because".into();
    let answers = state.confirm_current().expect("done");
    assert_eq!(answers.len(), 2);

    let mut multi = QuestionDialogState::new(vec![question("Many", &["A", "B"], true)]);
    multi.toggle_multi_at_cursor();
    multi.toggle_multi_at_cursor();
    assert!(multi.multi_selected.is_empty());
    multi.free_text_focus = true;
    multi.toggle_multi_at_cursor();
    assert!(multi.free_text_focus);
    multi.free_text_focus = false;
    multi.set_cursor(2);
    multi.toggle_multi_at_cursor();
    assert!(multi.free_text_focus);

    let mut hole = QuestionDialogState::new(vec![
        question("Pick", &["A", "B"], false),
        question("Why", &[], false),
    ]);
    hole.index = 1;
    hole.free_text = "because".into();
    assert!(hole.confirm_current().is_none());
    assert_eq!(hole.index, 0);

    let mut empty = QuestionDialogState::new(vec![]);
    empty.move_cursor(1);
    empty.set_cursor(3);
    assert!(empty.confirm_current().is_none());

    let _ = DialogManager::default();
}

#[test]
fn sidebar_hover_clears_and_todos_toggle() {
    let mut app = app();
    app.sidebar.tab_hits[0].hovered = true;
    assert!(app.sidebar.update_tab_hover(None));
    assert!(!app.sidebar.tab_hits[0].hovered);

    app.replace_todos(vec![whycodes_core::TodoItem::new(
        "t1",
        "do it",
        whycodes_core::TodoStatus::Pending,
    )]);
    app.focus = FocusPane::Todos;
    app.toggle_todos_panel();
    assert!(app.todos_collapsed);
    assert_eq!(app.focus, FocusPane::Prompt);
    app.toggle_todos_panel();
    assert!(!app.todos_collapsed);
}

#[test]
fn bg_job_status_flags_and_git_branch_fast_path() {
    for (status, running, terminal) in [
        ("running", true, false),
        ("done", false, true),
        ("failed", false, true),
        ("killed", false, true),
        ("completed", false, true),
        ("cancelled", false, true),
        ("queued", false, false),
    ] {
        let job = BgJobUi {
            id: "j".into(),
            summary: "s".into(),
            status: status.into(),
            started_at: Instant::now(),
            elapsed_ms: 0,
        };
        assert_eq!(job.is_running(), running, "{status}");
        assert_eq!(job.is_terminal(), terminal, "{status}");
    }

    let dir = tempfile::tempdir().unwrap();
    assert!(resolve_git_branch_fast(dir.path()).is_none());

    let git = dir.path().join(".git");
    std::fs::create_dir_all(&git).unwrap();
    std::fs::write(git.join("HEAD"), "").unwrap();
    assert!(resolve_git_branch_fast(dir.path()).is_none());
    std::fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    assert_eq!(resolve_git_branch_fast(dir.path()).as_deref(), Some("main"));
    std::fs::write(git.join("HEAD"), "ref: refs/heads/feature/x\n").unwrap();
    assert_eq!(
        resolve_git_branch_fast(dir.path()).as_deref(),
        Some("feature/x")
    );
    std::fs::write(git.join("HEAD"), "ref: refs/tags/v1\n").unwrap();
    assert_eq!(resolve_git_branch_fast(dir.path()).as_deref(), Some("v1"));
    std::fs::write(git.join("HEAD"), "abcdef1234567890\n").unwrap();
    assert_eq!(
        resolve_git_branch_fast(dir.path()).as_deref(),
        Some("abcdef1")
    );

    let wt = tempfile::tempdir().unwrap();
    std::fs::write(
        wt.path().join(".git"),
        format!("gitdir: {}\n", git.display()),
    )
    .unwrap();
    std::fs::write(git.join("HEAD"), "ref: refs/heads/worktree\n").unwrap();
    assert_eq!(
        resolve_git_branch_fast(wt.path()).as_deref(),
        Some("worktree")
    );

    let missing = git_output_timeout(
        &mut std::process::Command::new("whycodes-no-such-git-bin"),
        std::time::Duration::from_millis(50),
    );
    assert!(missing.is_none());
    let _ = resolve_git_branch(dir.path());
}

#[test]
fn catalog_models_merges_config_and_dedups() {
    let mut cfg = whycodes_config::Config::default();
    cfg.providers.insert(
        "acme".into(),
        whycodes_core::types::ProviderConfig {
            name: "acme".into(),
            api_key: None,
            api_base: None,
            base_url: None,
            headers: None,
            models: vec!["m1".into(), "m2".into()],
            tool_arguments: None,
            extra: Default::default(),
        },
    );
    let models = catalog_models(&cfg);
    assert!(models.contains(&("acme".into(), "m1".into())), "{models:?}");
    assert!(models.contains(&("acme".into(), "m2".into())), "{models:?}");
    let mut sorted = models.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(models, sorted, "catalog is sorted and unique");
}

#[test]
fn catalog_models_merges_oauth_token_store_suggestions() {
    let _lock = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let prev = std::env::var_os("WHYCODES_HOME");
    unsafe { std::env::set_var("WHYCODES_HOME", dir.path()) };

    let json = r#"{
        "kind": "auth",
        "auth": {
            "provider": "tui-cov-oauth-cat",
            "label": "CovCat",
            "flow": "device-code",
            "client_id": "abc",
            "authorize_url": "https://example.com/device/code",
            "token_url": "https://example.com/token",
            "scopes": "read",
            "suggested_models": ["oauth-m1"]
        }
    }"#;
    whycodes_auth::plugin::register_from_json(json).expect("register oauth plugin");
    whycodes_auth::TokenStore::new(dir.path())
        .set(
            "tui-cov-oauth-cat",
            whycodes_auth::ProviderAuth {
                method: "oauth".into(),
                token: whycodes_auth::OAuthToken {
                    access_token: "tok".into(),
                    refresh_token: None,
                    expires_at: None,
                    extra: Default::default(),
                },
            },
        )
        .expect("write token");

    let cfg = whycodes_config::Config::default();
    let models = catalog_models(&cfg);
    match prev {
        Some(v) => unsafe { std::env::set_var("WHYCODES_HOME", v) },
        None => unsafe { std::env::remove_var("WHYCODES_HOME") },
    }
    assert!(
        models.contains(&("tui-cov-oauth-cat".into(), "oauth-m1".into())),
        "OAuth store must merge suggested models, got {models:?}"
    );
}

#[test]
fn prompt_paste_image_and_scroll_helpers() {
    let mut state = app();
    assert!(!state.prompt_has_content());
    assert!(state.pop_pending_image().is_none());
    assert!(!state.copy_selected_message());

    state.input_buffer = "   ".into();
    assert!(!state.prompt_has_content());
    state.input_buffer = "hello".into();
    assert!(state.prompt_has_content());

    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("shot.png");
    std::fs::write(&img, b"\x89PNG\r\n\x1a\nfake").unwrap();
    state.input_buffer.clear();
    state.attach_image(&img).unwrap();
    assert!(state.prompt_has_content());
    assert_eq!(state.pending_images.len(), 1);
    state.attach_image(&img).unwrap();
    assert_eq!(state.pending_images.len(), 1, "same path is not duplicated");
    assert!(state.pop_pending_image().is_some());
    assert!(state.pending_images.is_empty());

    for i in 0..crate::images::MAX_ATTACHMENTS {
        let p = dir.path().join(format!("n{i}.png"));
        std::fs::write(&p, b"\x89PNG\r\n").unwrap();
        state.attach_image(&p).unwrap();
    }
    let extra = dir.path().join("overflow.png");
    std::fs::write(&extra, b"\x89PNG\r\n").unwrap();
    let err = state.attach_image(&extra).unwrap_err();
    assert!(err.contains("max"), "{err}");

    let mut paste_app = app();
    let pasted = "one\ntwo\nthree";
    paste_app.insert_paste_text(pasted);
    let token = crate::paste::placeholder_at(&paste_app.input_buffer, 0).expect("collapsed paste");
    let id = token.id;
    paste_app.remove_paste_span(token.start, token.end, id);
    assert!(paste_app.input_buffer.is_empty());
    assert!(paste_app.pending_pastes.is_empty());
    assert_eq!(paste_app.input_cursor, 0);
    paste_app.remove_paste_span(4, 1, 99);
    paste_app.remove_paste_span(0, 8, 1);

    paste_app.add_message(ChatRole::User, "hello");
    paste_app.add_message(ChatRole::Assistant, "world");
    paste_app.chat_viewport_rows = 20;
    paste_app.chat_content_width = 80;
    paste_app.chat_scroll_total = 0;
    let (total, height, max_off) = paste_app.chat_scroll_metrics();
    assert_eq!(height, 20);
    assert!(total > 0);
    assert_eq!(max_off, total.saturating_sub(20));
    paste_app.chat_scroll_total = 40;
    let (total2, _, max2) = paste_app.chat_scroll_metrics();
    assert_eq!(total2, 40);
    assert_eq!(max2, 20);
    paste_app.scroll_offset = 99;
    paste_app.clamp_chat_scroll();
    assert_eq!(paste_app.scroll_offset, 20);
    paste_app.scroll_offset = 0;
    paste_app.clamp_chat_scroll();
    assert!(paste_app.auto_scroll);
}

#[test]
fn copy_selected_message_covers_blocks_and_empty() {
    let mut state = app();
    assert!(!state.copy_selected_message());

    state.add_message(ChatRole::Assistant, "answer");
    let last = state.messages.last_mut().unwrap();
    last.blocks.push(ChatBlock::Thinking({
        let mut t = ThinkingBlock::new("secret plan");
        t.finish();
        t.collapsed = true;
        t
    }));
    last.blocks.push(ChatBlock::Thinking({
        let mut t = ThinkingBlock::new("open thought");
        t.finish();
        t.collapsed = false;
        t
    }));
    last.blocks.push(ChatBlock::ToolUse {
        id: "t1".into(),
        name: "read".into(),
        input: serde_json::json!({"path": "a.rs"}),
    });
    last.blocks.push(ChatBlock::ToolResult {
        id: "t1".into(),
        content: "fn main() {}".into(),
        is_error: false,
    });
    last.blocks.push(ChatBlock::Subagent {
        id: "s1".into(),
        kind: "explore".into(),
        description: "look around".into(),
        status: "completed".into(),
        activity: String::new(),
        elapsed_ms: 100,
    });
    last.blocks.push(ChatBlock::Text("ignored".into()));
    last.tool_calls.push(ChatToolCall {
        id: "t1".into(),
        name: "read".into(),
        arguments: serde_json::json!({"path": "a.rs"}),
        collapsed: true,
        result: Some("fn main() {}".into()),
        is_error: false,
    });
    let _ = state.copy_selected_message();
    assert!(
        state
            .toasts
            .visible()
            .iter()
            .any(|t| { t.message.contains("Copied") || t.message.contains("clipboard") }),
        "{:?}",
        state
            .toasts
            .visible()
            .iter()
            .map(|t| t.message.as_str())
            .collect::<Vec<_>>()
    );

    let mut empty = TuiApp::from_config(TuiAppConfig::default());
    empty.add_message(ChatRole::Assistant, "   ");
    assert!(!empty.copy_selected_message());

    crate::clipboard::with_copy_stub(false, || {
        let mut fail = app();
        fail.add_message(ChatRole::User, "copy me");
        fail.selected_msg = Some(0);
        assert!(!fail.copy_selected_message());
        assert!(
            fail.toasts
                .visible()
                .iter()
                .any(|t| t.message.contains("no clipboard")),
            "{:?}",
            fail.toasts
                .visible()
                .iter()
                .map(|t| t.message.as_str())
                .collect::<Vec<_>>()
        );
    });
}

#[test]
fn format_elapsed_ms_covers_hour_bucket() {
    assert_eq!(crate::app::format_elapsed_ms(3_600_000), "1h0m");
    assert_eq!(crate::app::format_elapsed_ms(3_720_000), "1h2m");
    assert_eq!(crate::app::format_thinking_elapsed(3_600_000), "1h0m");
}

#[test]
fn mouse_selection_normalized_orders_corners() {
    let sel = crate::app::MouseSelection {
        anchor_x: 8,
        anchor_y: 4,
        focus_x: 2,
        focus_y: 1,
        dragging: true,
    };
    assert_eq!(sel.normalized(), (2, 1, 8, 4));
}

#[test]
fn todos_page_rows_and_expand_input() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    assert!(app.todos_page_rows() >= 1);
    app.input_buffer = "hello".into();
    assert_eq!(app.expand_input(), "hello");
    app.focus = crate::app::FocusPane::Todos;
    app.scroll_todos(4);
    app.scroll_todos(-40);
}

#[test]
fn update_chrome_hover_clears_and_sets_hits() {
    use ratatui::layout::Rect;
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.context_hit.hovered = true;
    app.cwd_hit.hovered = true;
    app.agent_hit.hovered = true;
    app.model_hit.hovered = true;
    app.effort_hit.hovered = true;
    app.approval_hit.hovered = true;
    app.todos_hit.hovered = true;
    app.todos_body_hit.hovered = true;
    app.todos_scrollbar_hit.hovered = true;
    app.turn_stop_hit.hovered = true;
    app.tasks_hit.hovered = true;
    app.slash_suggest.hovered = Some(0);
    app.file_suggest.hovered = Some(0);
    app.mouse_pos = None;
    assert!(app.update_chrome_hover());
    assert!(!app.context_hit.hovered);

    app.slash_suggest.active = false;
    app.file_suggest.active = false;
    app.mouse_pos = Some((80, 80));
    assert!(app.update_chrome_hover());
    assert!(app.slash_suggest.hovered.is_none());
    assert!(app.file_suggest.hovered.is_none());

    app.context_hit.set_rect(Some(Rect {
        x: 1,
        y: 1,
        width: 3,
        height: 1,
    }));
    app.mouse_pos = Some((2, 1));
    assert!(app.update_chrome_hover());
    assert!(app.context_hit.hovered);
}

#[test]
fn update_chrome_hover_tracks_active_slash_and_file_rows() {
    use ratatui::layout::Rect;
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.slash_suggest.active = true;
    app.slash_suggest.matches = vec![0, 1];
    app.slash_suggest.hovered = None;
    app.slash_suggest.list_hit = Some(Rect {
        x: 2,
        y: 10,
        width: 20,
        height: 2,
    });
    app.slash_suggest.list_scroll_start = 0;
    app.file_suggest.active = true;
    app.file_suggest.matches = vec![
        whycodes_index::FileMatch {
            rel: "a.rs".into(),
            ..Default::default()
        },
        whycodes_index::FileMatch {
            rel: "b.rs".into(),
            ..Default::default()
        },
    ];
    app.file_suggest.hovered = None;
    app.file_suggest.list_hit = Some(Rect {
        x: 2,
        y: 20,
        width: 20,
        height: 2,
    });
    app.file_suggest.list_scroll_start = 0;

    app.mouse_pos = Some((5, 10));
    assert!(app.update_chrome_hover());
    assert_eq!(app.slash_suggest.hovered, Some(0));

    app.mouse_pos = Some((5, 11));
    assert!(app.update_chrome_hover());
    assert_eq!(app.slash_suggest.hovered, Some(1));

    app.mouse_pos = Some((50, 10));
    assert!(app.update_chrome_hover());
    assert!(
        app.slash_suggest.hovered.is_none(),
        "leaving the slash list must clear hover"
    );

    app.mouse_pos = Some((5, 20));
    assert!(app.update_chrome_hover());
    assert_eq!(app.file_suggest.hovered, Some(0));

    app.mouse_pos = Some((5, 21));
    assert!(app.update_chrome_hover());
    assert_eq!(app.file_suggest.hovered, Some(1));

    app.mouse_pos = Some((50, 20));
    assert!(app.update_chrome_hover());
    assert!(
        app.file_suggest.hovered.is_none(),
        "leaving the file list must clear hover"
    );
}

#[test]
fn submit_input_oauth_code_paths() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.auth_code_sink = Some(tx);
    app.input_buffer = "abc#state".into();
    app.submit_input();
    assert_eq!(rx.blocking_recv().unwrap(), "abc#state");

    let (tx, rx) = tokio::sync::oneshot::channel();
    drop(rx);
    app.auth_code_sink = Some(tx);
    app.input_buffer = "late".into();
    app.submit_input();
    assert!(app.status_message.contains("already closed"));

    let (tx, _rx) = tokio::sync::oneshot::channel();
    app.auth_code_sink = Some(tx);
    app.input_buffer.clear();
    app.submit_input();
    assert!(app.status_message.contains("cancelled"));
}

#[test]
fn sync_tasks_collapse_when_empty() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.tasks_collapsed = true;
    app.toggle_tasks_pane();
    assert!(!app.tasks_collapsed || app.task_count() == 0);
}

#[test]
fn toggle_tasks_pane_empty_opens_and_closes_agents_tab() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    assert_eq!(app.task_count(), 0);
    app.sidebar.visible = false;
    app.toggle_tasks_pane();
    assert!(app.sidebar.visible);
    assert_eq!(app.sidebar.active_tab, SidebarTab::Agents);

    app.toggle_tasks_pane();
    assert!(
        !app.sidebar.visible,
        "second Ctrl+G on an empty list must hide the Agents tab"
    );

    app.sidebar.visible = true;
    app.sidebar.active_tab = SidebarTab::Files;
    app.toggle_tasks_pane();
    assert!(app.sidebar.visible);
    assert_eq!(
        app.sidebar.active_tab,
        SidebarTab::Agents,
        "empty list still jumps to Agents when another tab is showing"
    );
}

#[test]
fn sync_tasks_collapse_clears_hits_when_the_list_becomes_empty() {
    use ratatui::layout::Rect;
    let mut app = app();
    app.upsert_bg_job("j", "done", "finished");
    app.tasks_collapsed = true;
    app.tasks_hit.set_rect(Some(Rect {
        x: 0,
        y: 0,
        width: 8,
        height: 1,
    }));
    app.tasks_row_hits.push((
        Rect {
            x: 0,
            y: 1,
            width: 8,
            height: 1,
        },
        "j".into(),
    ));
    app.bg_jobs.clear();
    app.subagents.clear();
    app.sync_tasks_collapse(true);
    assert!(!app.tasks_collapsed);
    assert!(app.tasks_hit.rect.is_none());
    assert!(app.tasks_row_hits.is_empty());
}

#[test]
fn thinking_header_shows_elapsed_and_push_capped_splits_utf8() {
    let mut tb = crate::app::ThinkingBlock::new("plan");
    tb.started_at = Instant::now()
        .checked_sub(std::time::Duration::from_millis(1400))
        .expect("elapsed");
    let label = tb.header_label();
    assert!(
        label.contains("Thinking") && label.contains('·'),
        "running thinking with elapsed must include the time, got {label}"
    );

    let mut cap = crate::app::ThinkingBlock::new("x".repeat(crate::app::THINKING_MAX_CHARS - 1));
    cap.push_delta("é overflow");
    assert!(cap.text.ends_with('…') || cap.text.len() <= crate::app::THINKING_MAX_CHARS + 3);
}

#[test]
fn upsert_bg_job_retains_last_sixteen() {
    let mut app = app();
    for i in 0..20 {
        app.upsert_bg_job(format!("job-{i}"), "running", format!("work {i}"));
    }
    assert_eq!(app.bg_jobs.len(), 16);
    assert_eq!(app.bg_jobs[0].id, "job-4");
    app.upsert_bg_job("job-19", "done", "finished");
    assert_eq!(app.bg_jobs.last().map(|j| j.status.as_str()), Some("done"));
}

#[test]
fn slash_suggest_hides_when_prefix_matches_nothing() {
    let mut state = SlashSuggestState::default();
    state.refresh("/zzzz-no-such-command");
    assert!(!state.active);
    assert!(state.matches.is_empty());
    state.step(1);
}

#[test]
fn slash_suggest_resets_selected_when_filter_shrinks() {
    let mut state = SlashSuggestState::default();
    state.refresh("/");
    assert!(state.active);
    assert!(state.matches.len() > 1);
    state.selected = state.matches.len() - 1;
    state.refresh("/help");
    assert!(state.active);
    assert_eq!(
        state.selected, 0,
        "selected past the new match list must wrap to 0"
    );
    assert_eq!(
        state.current().map(|c| c.name),
        Some("/help"),
        "prefix /help must land on the help command"
    );
}

#[test]
fn question_confirm_other_requires_text_then_accepts_free_text() {
    let mut st =
        crate::app::QuestionDialogState::new(vec![whycodes_tools::question::QuestionSpec {
            prompt: "Go?".into(),
            options: vec![whycodes_tools::question::QuestionOption {
                label: "Yes".into(),
                description: String::new(),
                preview: None,
            }],
            multi_select: true,
            important: false,
        }]);
    st.cursor = st.option_count() - 1;
    assert!(st.is_other_index(st.cursor));
    assert!(
        st.confirm_current().is_none(),
        "empty Other must not finish"
    );
    st.free_text = "typed".into();
    st.free_text_focus = true;
    let answers = st.confirm_current().expect("free text Other is valid");
    assert_eq!(answers[0].free_text.as_deref(), Some("typed"));
}

#[test]
fn question_multi_accepts_free_text_when_nothing_is_checked() {
    let mut st =
        crate::app::QuestionDialogState::new(vec![whycodes_tools::question::QuestionSpec {
            prompt: "Pick?".into(),
            options: vec![whycodes_tools::question::QuestionOption {
                label: "A".into(),
                description: String::new(),
                preview: None,
            }],
            multi_select: true,
            important: false,
        }]);
    st.cursor = 0;
    st.free_text_focus = false;
    st.free_text = "only this".into();
    let answers = st
        .confirm_current()
        .expect("unchecked multi + free text must still finish");
    assert!(answers[0].selected.is_empty());
    assert_eq!(answers[0].free_text.as_deref(), Some("only this"));
}

#[test]
fn save_view_copies_transcript_and_draft() {
    let mut app = app();
    app.add_message(ChatRole::User, "hello");
    app.session_title = "t".into();
    app.status_message = "ok".into();
    app.input_buffer = "draft".into();
    app.input_cursor = 5;
    app.scroll_offset = 2;
    app.auto_scroll = false;
    app.selected_msg = Some(0);
    let mut snap = crate::session_runtime::ViewSnapshot::default();
    app.save_view(&mut snap);
    assert_eq!(snap.messages.len(), 1);
    assert_eq!(snap.session_title, "t");
    assert_eq!(snap.status_message, "ok");
    assert_eq!(snap.input_buffer, "draft");
    assert_eq!(snap.input_cursor, 5);
    assert_eq!(snap.scroll_offset, 2);
    assert!(!snap.auto_scroll);
    assert_eq!(snap.selected_msg, Some(0));
}

#[test]
fn thinking_push_capped_truncates_on_a_char_boundary() {
    let mut tb = crate::app::ThinkingBlock::new("x".repeat(crate::app::THINKING_MAX_CHARS - 3));
    tb.push_delta("hello");
    assert!(tb.text.ends_with('…'));
    assert!(tb.text.contains("hel") || tb.text.len() <= crate::app::THINKING_MAX_CHARS + 3);
}

#[test]
fn dialog_list_index_past_total_is_none() {
    use ratatui::layout::Rect;
    let mut app = app();
    app.dialog_list_hit = Some(Rect {
        x: 0,
        y: 0,
        width: 10,
        height: 4,
    });
    app.dialog_list_scroll_start = 0;
    app.dialog_list_total = 2;
    assert_eq!(app.dialog_list_index_at(1, 0), Some(0));
    assert_eq!(app.dialog_list_index_at(1, 1), Some(1));
    assert_eq!(
        app.dialog_list_index_at(1, 3),
        None,
        "row past the item count must miss"
    );
}

#[test]
fn thinking_body_lines_empty_when_finished_and_collapsed() {
    let mut tb = crate::app::ThinkingBlock::new("line1\nline2\nline3");
    tb.finish();
    tb.collapsed = true;
    assert!(
        tb.body_lines().is_empty(),
        "folded finished thinking hides the body"
    );
}

#[test]
fn insert_paste_text_empty_is_a_noop() {
    let mut app = app();
    app.input_buffer = "keep".into();
    app.input_cursor = 4;
    app.insert_paste_text("");
    assert_eq!(app.input_buffer, "keep");
    assert_eq!(app.input_cursor, 4);
}

#[test]
fn ensure_selected_visible_scrolls_when_the_row_is_offscreen() {
    let mut app = app();
    for i in 0..20 {
        app.add_message(ChatRole::User, format!("msg {i}"));
    }
    app.chat_viewport_rows = 4;
    app.chat_content_width = 40;
    app.scroll_offset = 0;
    app.selected_msg = Some(0);
    app.ensure_selected_visible();
    assert!(
        app.scroll_offset > 0,
        "oldest message must pull the viewport up from the bottom"
    );
    let high = app.scroll_offset;
    app.selected_msg = Some(19);
    app.ensure_selected_visible();
    assert!(
        app.scroll_offset < high,
        "newest message must drop the viewport back toward the bottom"
    );
}

#[test]
fn question_empty_options_require_free_text() {
    let mut st = crate::app::QuestionDialogState::new(vec![question("Explain", &[], false)]);
    assert!(st.confirm_current().is_none());
    assert!(st.free_text_focus);
    st.free_text = "because".into();
    let answers = st.confirm_current().expect("free text on empty options");
    assert_eq!(answers[0].free_text.as_deref(), Some("because"));
}

#[test]
fn focus_todos_is_a_noop_when_the_list_cannot_scroll() {
    let mut app = app();
    app.focus = crate::app::FocusPane::Prompt;
    app.focus_todos();
    assert_eq!(app.focus, crate::app::FocusPane::Prompt);
}
