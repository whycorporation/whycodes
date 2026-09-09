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
        std::process::Command::new("whycodes-no-such-git-bin"),
        std::time::Duration::from_millis(50),
    );
    assert!(missing.is_none());
    let _ = resolve_git_branch(dir.path());
}
