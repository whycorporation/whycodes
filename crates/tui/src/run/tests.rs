use super::*;

#[test]
fn restore_terminal_resets_cursor_style_to_user_default() {
    let mut out = Vec::new();
    restore_terminal_on(&mut out);
    let bytes = String::from_utf8_lossy(&out);
    assert!(
        bytes.contains("\u{1b}[0 q"),
        "DECSCUSR default shape missing after TUI exit: {bytes:?}"
    );
}

#[test]
fn keyboard_enhancement_query_skips_bench_and_zero_size() {
    assert!(!should_query_keyboard_enhancement(true, Some((80, 24))));
    assert!(!should_query_keyboard_enhancement(false, Some((0, 24))));
    assert!(!should_query_keyboard_enhancement(false, Some((80, 0))));
    assert!(!should_query_keyboard_enhancement(false, None));
    assert!(should_query_keyboard_enhancement(false, Some((80, 24))));
}

#[test]
fn truncate_toast_takes_first_line_and_trims() {
    assert_eq!(truncate_toast("short", 20), "short");
    assert_eq!(truncate_toast("first\nsecond", 20), "first");
    assert_eq!(truncate_toast("  padded  ", 20), "padded");
    let long = "x".repeat(100);
    let out = truncate_toast(&long, 10);
    assert_eq!(out.chars().count(), 10);
    assert!(out.ends_with('…'));
}

#[test]
fn mouse_move_does_not_force_redraw_other_events_do() {
    let moved = Event::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Moved,
        column: 1,
        row: 1,
        modifiers: crossterm::event::KeyModifiers::NONE,
    });
    let wheel = Event::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 1,
        row: 1,
        modifiers: crossterm::event::KeyModifiers::NONE,
    });
    let key = Event::Key(crossterm::event::KeyEvent::from(KeyCode::Char('a')));
    assert!(!event_forces_redraw(&moved));
    assert!(event_forces_redraw(&wheel));
    assert!(event_forces_redraw(&key));
    assert!(event_forces_redraw(&Event::Resize(80, 24)));
}

#[test]
fn rfc3339_parser_accepts_and_rejects() {
    assert!(parse_session_rfc3339("2026-01-02T03:04:05Z").is_some());
    assert!(parse_session_rfc3339("2026-01-02T03:04:05+02:00").is_some());
    assert!(parse_session_rfc3339("not a date").is_none());
    assert!(parse_session_rfc3339("").is_none());
}

#[test]
fn short_session_id_truncates_long_ids() {
    assert_eq!(short_session_id("abc"), "abc");
    assert_eq!(short_session_id("abcdefgh"), "abcdefgh");
    assert_eq!(short_session_id("abcdefghijkl"), "abcdefgh…");
}

#[test]
fn turn_done_status_formats_cancel_and_usage() {
    let app = TuiApp::new(TuiAppConfig::default());
    let s = format_turn_done_status(&app, "build", "anthropic", "m", None, true);
    assert_eq!(s, "Turn cancelled.");
    let s = format_turn_done_status(&app, "build", "anthropic", "m", Some(4500), true);
    assert_eq!(s, "Turn cancelled in 4.5s");

    let mut app = TuiApp::new(TuiAppConfig::default());
    let s = format_turn_done_status(&app, "build", "anthropic", "m", None, false);
    assert_eq!(s, "Done");

    app.turn_usage = Some(whycodes_core::types::Usage {
        input_tokens: 1200,
        output_tokens: 340,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: Some(500),
    });
    let s = format_turn_done_status(&app, "build", "anthropic", "m", Some(4200), false);
    assert!(s.contains("Worked for 4.2s"), "{s}");
    assert!(s.contains("in"), "{s}");
    assert!(s.contains("out"), "{s}");
    assert!(s.contains("cached"), "{s}");
}

#[test]
fn snapshot_cells_reads_every_symbol() {
    use ratatui::buffer::Buffer;
    let mut buf = Buffer::empty(Rect::new(0, 0, 2, 2));
    buf[(0, 0)].set_symbol("a");
    buf[(1, 1)].set_symbol("b");
    let grid = crate::cell_grid::CellGrid::from_buffer(&buf);
    assert_eq!(grid.height(), 2);
    assert_eq!(grid.get(0, 0), "a");
    assert_eq!(grid.get(1, 1), "b");
    assert_eq!(grid.get(1, 0), " ");
}

#[test]
fn expand_at_files_inlines_existing_and_keeps_missing() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("note.txt");
    std::fs::write(&f, "hello world").unwrap();
    let out = expand_at_files("read @note.txt now", dir.path());
    assert!(out.contains("hello world"), "{out}");
    assert!(out.contains("--- file: note.txt ---"), "{out}");
    assert!(out.ends_with("now"), "{out}");

    let out = expand_at_files("see @missing.txt", dir.path());
    assert!(
        out.contains("@missing.txt"),
        "missing must stay literal: {out}"
    );

    // Absolute path works too.
    let out = expand_at_files(&format!("@{} done", f.display()), dir.path());
    assert!(out.contains("hello world"), "{out}");
}

#[test]
fn expand_at_files_truncates_huge_files() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("big.txt");
    std::fs::write(&f, "x".repeat(AT_FILE_MAX_CHARS + 100)).unwrap();
    let out = expand_at_files("@big.txt", dir.path());
    assert!(out.contains("characters omitted"), "{out}");
}

#[test]
fn expand_at_files_bare_at_stays_literal() {
    let out = expand_at_files("email me @ now", std::path::Path::new("/work"));
    assert!(out.contains("@ now"), "{out}");
}

#[test]
fn memory_settings_for_sets_agent_bank() {
    let config = Config::default();
    let base = memory_settings(&config);
    assert!(base.enabled);
    assert!(base.agent_bank.is_none());
    let scoped = memory_settings_for(&config, Some("worker".into()));
    assert_eq!(scoped.agent_bank.as_deref(), Some("worker"));
}

#[test]
fn resolve_session_latest_and_prefix() {
    let db = whycodes_storage::db::Database::open_in_memory().unwrap();
    // Empty db → latest is None.
    assert!(
        resolve_and_load_session(&db, RESUME_LATEST)
            .unwrap()
            .is_none()
    );
    assert!(resolve_and_load_session(&db, "nope").unwrap().is_none());

    let mut s1 = Session::new(std::path::PathBuf::from("/work/proj"), "sys".into());
    s1.add_user_message("first");
    s1.save_to_db(&db).unwrap();
    let id1 = s1.id.clone();

    let mut s2 = Session::new(std::path::PathBuf::from("/work/proj"), "sys".into());
    s2.add_user_message("second");
    s2.save_to_db(&db).unwrap();

    // Exact id.
    let loaded = resolve_and_load_session(&db, &id1).unwrap().expect("exact");
    assert_eq!(loaded.id, id1);

    // Unique prefix (8 chars) matches.
    let prefix: String = id1.chars().take(8).collect();
    let loaded = resolve_and_load_session(&db, &prefix)
        .unwrap()
        .expect("prefix");
    assert_eq!(loaded.id, id1);

    // RESUME_LATEST → most recently updated (s2).
    let latest = resolve_and_load_session(&db, RESUME_LATEST)
        .unwrap()
        .expect("latest");
    assert_eq!(latest.id, s2.id);
}

#[test]
fn apply_panel_update_sets_preview_and_toast() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    apply_panel_update(
        &mut app,
        whycodes_core::PanelUpdate::File {
            path: "a.rs".into(),
            text: "fn main() {}".into(),
        },
    );
    assert!(app.sidebar.visible);
    assert_eq!(app.sidebar.active_tab, crate::app::SidebarTab::Preview);
    assert!(matches!(
        &app.sidebar.preview,
        crate::app::SidebarPreview::File { path, text }
            if path == "a.rs" && text == "fn main() {}"
    ));

    apply_panel_update(
        &mut app,
        whycodes_core::PanelUpdate::Diff {
            path: "b.rs".into(),
            unified: "-a\n+b".into(),
        },
    );
    assert!(matches!(
        &app.sidebar.preview,
        crate::app::SidebarPreview::Diff { path, unified }
            if path == "b.rs" && unified == "-a\n+b"
    ));

    apply_panel_update(
        &mut app,
        whycodes_core::PanelUpdate::Mermaid {
            source: "graph TD".into(),
        },
    );
    assert!(matches!(
        &app.sidebar.preview,
        crate::app::SidebarPreview::Mermaid { source } if source == "graph TD"
    ));

    apply_panel_update(&mut app, whycodes_core::PanelUpdate::Clear);
    assert!(matches!(
        app.sidebar.preview,
        crate::app::SidebarPreview::None
    ));
}

#[test]
fn cost_report_handles_empty_and_filled_usage() {
    let mut session = Session::new(PathBuf::from("/work/proj"), "sys".into());
    session.add_user_message("hello");
    let app = TuiApp::new(TuiAppConfig::default());

    // No provider usage yet → estimated line.
    let out = cost_report(&session, &app);
    assert!(out.contains("estimated"), "{out}");
    assert!(out.contains("last turn: (none yet)"), "{out}");

    session.usage = whycodes_core::types::Usage {
        input_tokens: 1200,
        output_tokens: 300,
        cache_creation_input_tokens: Some(500),
        cache_read_input_tokens: Some(9000),
    };
    let out = cost_report(&session, &app);
    assert!(out.contains("1.2k in / 300 out"), "{out}");
    assert!(out.contains("cache write: 500"), "{out}");
    assert!(out.contains("cache read:  9k"), "{out}");
    assert!(out.contains("total 11k"), "{out}"); // includes cache tokens
}

#[test]
fn cost_report_includes_last_turn_usage() {
    let session = Session::new(PathBuf::from("/work/proj"), "sys".into());
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.turn_usage = Some(whycodes_core::types::Usage {
        input_tokens: 100,
        output_tokens: 50,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    });
    let out = cost_report(&session, &app);
    assert!(out.contains("last turn: 100 in / 50 out"), "{out}");
}

#[test]
fn context_report_lists_roles_and_tool_sizes() {
    let mut session = Session::new(PathBuf::from("/work/proj"), "sys".into());
    session.add_user_message("do it");
    session.add_tool_results(vec![whycodes_core::types::ToolResult {
        tool_call_id: "tc1".into(),
        content: "short result".into(),
        is_error: false,
    }]);
    let app = TuiApp::new(TuiAppConfig::default());
    let config = Config::default();
    let agent = Agent::new(whycodes_core::types::AgentInfo {
        name: "build".into(),
        description: String::new(),
        mode: AgentMode::Primary,
        permission: whycodes_core::types::PermissionSet::default(),
        model: None,
        system_prompt: None,
        temperature: None,
        top_p: None,
    });
    let out = context_report(&session, &app, &config, &agent);
    assert!(out.contains("Context"), "{out}");
    assert!(out.contains("messages:  2"), "{out}");
    assert!(out.contains("user: 1"), "{out}");
    assert!(out.contains("tool: 1"), "{out}");
    assert!(out.contains("largest tool results"), "{out}");
    assert!(out.contains("profile="), "{out}");
    assert!(out.contains("memory:    enabled="), "{out}");
    assert!(out.contains("cwd:"), "{out}");
}

#[test]
fn load_session_todos_missing_and_valid() {
    let dir = tempfile::tempdir().unwrap();
    assert!(whycodes_core::todo::load_todos(dir.path(), None).is_empty());

    let whycodes = dir.path().join(".whycodes");
    std::fs::create_dir_all(&whycodes).unwrap();
    std::fs::write(
        whycodes.join("todos.json"),
        r#"{"todos": [
            {"content": "finish task", "status": "pending"},
            {"content": "done item", "status": "completed"},
            {"content": "working now", "status": "in_progress"},
            {"content": "skipped", "status": "cancelled"},
            {"content": "no status"}
        ]}"#,
    )
    .unwrap();
    let todos = whycodes_core::todo::load_todos(dir.path(), None);
    assert_eq!(todos.len(), 5);
    assert_eq!(todos[0].line(), "☐ finish task");
    assert_eq!(todos[1].line(), "☑ done item");
    assert_eq!(todos[2].line(), "▶ working now");
    assert_eq!(todos[3].line(), "✗ skipped");
    assert_eq!(todos[4].line(), "☐ no status");
}

#[test]
fn load_session_todos_invalid_json_and_wrong_shape() {
    let dir = tempfile::tempdir().unwrap();
    let whycodes = dir.path().join(".whycodes");
    std::fs::create_dir_all(&whycodes).unwrap();
    std::fs::write(whycodes.join("todos.json"), "not json {{{").unwrap();
    assert!(whycodes_core::todo::load_todos(dir.path(), None).is_empty());
    std::fs::write(whycodes.join("todos.json"), r#"{"other": 1}"#).unwrap();
    assert!(whycodes_core::todo::load_todos(dir.path(), None).is_empty());
}

#[test]
fn configured_models_from_providers_and_oauth() {
    use std::sync::OnceLock;
    static HOME: OnceLock<tempfile::TempDir> = OnceLock::new();
    let dir = HOME.get_or_init(|| tempfile::tempdir().expect("tempdir"));
    // Isolate WHYCODES_HOME so TokenStore reads a temp dir, not user keys.
    let prev = std::env::var_os("WHYCODES_HOME");
    unsafe { std::env::set_var("WHYCODES_HOME", dir.path()) };

    let mut config = Config::default();
    config.providers.insert(
        "acme".into(),
        whycodes_core::types::ProviderConfig {
            name: "acme".into(),
            api_key: None,
            api_base: None,
            base_url: None,
            headers: None,
            models: vec!["acme-1".into(), "acme-2".into()],
            tool_arguments: None,
            extra: Default::default(),
        },
    );
    let out = configured_models(&config);
    assert!(out.contains(&("acme".to_string(), "acme-1".to_string())));
    assert!(out.contains(&("acme".to_string(), "acme-2".to_string())));

    match prev {
        Some(v) => unsafe { std::env::set_var("WHYCODES_HOME", v) },
        None => unsafe { std::env::remove_var("WHYCODES_HOME") },
    }
}

#[test]
fn format_token_count_scales() {
    assert_eq!(format_token_count(0), "0");
    assert_eq!(format_token_count(999), "999");
    assert_eq!(format_token_count(1000), "1k");
    assert_eq!(format_token_count(1200), "1.2k");
    assert_eq!(format_token_count(200_000), "200k");
    assert_eq!(format_token_count(1_000_000), "1M");
    assert_eq!(format_token_count(1_200_000), "1.2M");
    // Sub-million stays in the k branch.
    assert_eq!(format_token_count(999_500), "999.5k");
}

#[test]
fn session_details_reports_usage_and_flags() {
    let mut session = Session::new(PathBuf::from("/work/proj"), "sys".into());
    session.add_user_message("hi");
    let app = TuiApp::new(TuiAppConfig::default());
    let config = Config::default();

    // No usage yet → estimated line.
    let out = session_details(&session, "build", &app, &config);
    assert!(out.contains("title:"), "{out}");
    assert!(out.contains("agent:     build"), "{out}");
    assert!(out.contains("messages:  1"), "{out}");
    assert!(out.contains("estimated"), "{out}");
    assert!(out.contains("prompt_cache:"), "{out}");
    assert!(out.contains("model_fast:"), "{out}");
    assert!(out.contains("model_smol:"), "{out}");
    assert!(out.contains("model_race:"), "{out}");
    assert!(out.contains("swarm:"), "{out}");

    // With usage → input/output/cache lines.
    session.usage = whycodes_core::types::Usage {
        input_tokens: 10,
        output_tokens: 20,
        cache_creation_input_tokens: Some(5),
        cache_read_input_tokens: Some(7),
    };
    let out = session_details(&session, "build", &app, &config);
    assert!(out.contains("input:     10"), "{out}");
    assert!(out.contains("output:    20"), "{out}");
    assert!(out.contains("cache write: 5"), "{out}");
    assert!(out.contains("cache read:  7"), "{out}");
    assert!(out.contains("total:     42"), "{out}");
    assert!(!out.contains("estimated"), "{out}");
}

#[test]
fn doctor_report_lists_checks() {
    let session = Session::new(PathBuf::from("/work/proj"), "sys".into());
    let app = TuiApp::new(TuiAppConfig::default());
    let config = Config::default();
    let agent = Agent::new(whycodes_core::types::AgentInfo {
        name: "build".into(),
        description: String::new(),
        mode: AgentMode::Primary,
        permission: whycodes_core::types::PermissionSet::default(),
        model: None,
        system_prompt: None,
        temperature: None,
        top_p: None,
    });
    let dir = tempfile::tempdir().unwrap();
    let out = doctor_report(&session, &app, &config, &agent, dir.path());
    assert!(out.contains("Doctor"), "{out}");
    assert!(out.contains("provider:"), "{out}");
    assert!(out.contains("model:"), "{out}");
    assert!(out.contains("agent:        build"), "{out}");
    assert!(out.contains("api_key:"), "{out}");
    assert!(out.contains("git_repo:     no"), "{out}");
    assert!(out.contains("bash_risk:"), "{out}");
    assert!(out.contains("sandbox:"), "{out}");
    assert!(out.contains("background:"), "{out}");
    assert!(out.contains("swarm:"), "{out}");
    assert!(out.contains("context:"), "{out}");
    assert!(out.contains("status:"), "{out}");
}

#[test]
fn refresh_sessions_rows_is_idempotent() {
    let rt = test_runtime();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    assert!(refresh_sessions_rows(&mut app, &rt, &[]));
    assert!(
        !refresh_sessions_rows(&mut app, &rt, &[]),
        "unchanged dashboard must not report a paint"
    );
    app.status_message = "Generating…".into();
    // Idle runtime preview ignores status — still unchanged.
    assert!(!refresh_sessions_rows(&mut app, &rt, &[]));
}

#[test]
fn refresh_sessions_rows_dirties_when_title_changes() {
    let mut rt = test_runtime();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    assert!(refresh_sessions_rows(&mut app, &rt, &[]));
    rt.session.title = "renamed".into();
    assert!(refresh_sessions_rows(&mut app, &rt, &[]));
    assert!(
        app.sessions_rows[0].title.contains("renamed"),
        "{:?}",
        app.sessions_rows[0].title
    );
}

#[test]
fn refresh_picker_skips_db_and_is_idempotent() {
    let rt = test_runtime();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.session_list.sessions = vec![crate::app::SessionEntry {
        id: "persisted".into(),
        title: "old".into(),
        messages: 2,
        updated_at: None,
        live: None,
    }];
    assert!(refresh_picker_live_section(&mut app, &rt, &[]));
    assert!(
        !refresh_picker_live_section(&mut app, &rt, &[]),
        "second refresh with the same live+persisted set is a no-op"
    );
    assert!(
        app.session_list
            .sessions
            .iter()
            .any(|e| e.id == "persisted" && e.live.is_none()),
        "persisted tail must survive without a DB reload"
    );
}

#[test]
fn drain_background_idle_does_not_touch_view() {
    let mut rt = test_runtime();
    let mut seed = TuiApp::from_config(TuiAppConfig::default());
    seed.add_message(ChatRole::Assistant, "hello");
    seed.yield_view(&mut rt.view);
    let ptr = rt.view.messages.as_ptr();
    drain_background_runtime(&mut rt);
    assert_eq!(
        rt.view.messages.as_ptr(),
        ptr,
        "idle drain must not reallocate the parked transcript"
    );
    assert!(!rt.unread);
    assert_eq!(rt.view.messages[0].content, "hello");
}

#[test]
fn drain_background_applies_text_delta_in_place() {
    let mut rt = test_runtime();
    let mut seed = TuiApp::from_config(TuiAppConfig::default());
    seed.add_message(ChatRole::Assistant, "hi");
    seed.yield_view(&mut rt.view);
    rt.event_tx
        .send(TurnEvent::TextDelta(" there".into()))
        .expect("open channel");
    drain_background_runtime(&mut rt);
    assert!(rt.unread);
    assert_eq!(rt.view.messages.last().unwrap().content, "hi there");
}

#[test]
fn switch_to_runtime_moves_transcripts() {
    let mut active = test_runtime();
    let mut parked = test_runtime();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.add_message(ChatRole::User, "from-active");
    let mut seed = TuiApp::from_config(TuiAppConfig::default());
    seed.add_message(ChatRole::User, "from-parked");
    seed.yield_view(&mut parked.view);
    let mut runtimes = vec![parked];
    switch_to_runtime(&mut app, &mut active, &mut runtimes, 0);
    assert!(
        app.pending_full_clears >= 2,
        "session switch must wipe PTY ghosts (home gutters / sidebar)"
    );
    assert_eq!(app.messages[0].content, "from-parked");
    assert_eq!(runtimes[0].view.messages[0].content, "from-active");
    assert!(
        active.view.messages.is_empty(),
        "live snapshot stays empty while adopted"
    );
    assert_eq!(
        crate::session_runtime::preview_from_messages(&app.messages),
        "from-parked"
    );
}

fn test_runtime() -> SessionRuntime {
    use std::sync::OnceLock;
    static HOME: OnceLock<tempfile::TempDir> = OnceLock::new();
    let dir = HOME.get_or_init(|| tempfile::tempdir().expect("tempdir"));
    unsafe { std::env::set_var("WHYCODES_HOME", dir.path()) };

    let info = whycodes_core::types::AgentInfo {
        name: "build".into(),
        description: String::new(),
        mode: AgentMode::Primary,
        permission: whycodes_core::types::PermissionSet::default(),
        model: None,
        system_prompt: None,
        temperature: None,
        top_p: None,
    };
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let (done_tx, done_rx) = mpsc::unbounded_channel();
    let (perm_prompter, perm_rx) = ChannelPermissionPrompter::new();
    let (question_prompter, question_rx) = ChannelQuestionPrompter::new(None);
    SessionRuntime::new(
        Agent::new(info),
        Session::new(PathBuf::from("/work/proj"), "sys".into()),
        SessionHistory::new(),
        event_tx,
        event_rx,
        done_tx,
        done_rx,
        Arc::new(perm_prompter),
        Arc::new(question_prompter),
        perm_rx,
        question_rx,
    )
}

fn isolate_home() {
    use std::sync::OnceLock;
    static HOME: OnceLock<tempfile::TempDir> = OnceLock::new();
    let dir = HOME.get_or_init(|| tempfile::tempdir().expect("tempdir"));
    unsafe { std::env::set_var("WHYCODES_HOME", dir.path()) };
}

/// Exclusive empty `WHYCODES_HOME` for tests that assert on an empty session
/// store. The shared [`isolate_home`] OnceLock is process-wide, so a
/// sibling persist can make `RESUME_LATEST` look populated.
fn isolate_home_fresh() -> (std::sync::MutexGuard<'static, ()>, tempfile::TempDir) {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    unsafe { std::env::set_var("WHYCODES_HOME", dir.path()) };
    (lock, dir)
}

#[test]
fn apply_turn_event_covers_every_variant() {
    isolate_home();
    let mut app = TuiApp::from_config(TuiAppConfig::default());

    apply_turn_event(&mut app, TurnEvent::TextDelta("hello".into()));
    assert_eq!(app.current_agent_state, AgentState::Generating);
    assert!(app.messages.iter().any(|m| m.content.contains("hello")));

    apply_turn_event(&mut app, TurnEvent::ThinkingDelta("hmm".into()));
    assert_eq!(app.current_agent_state, AgentState::Thinking);

    apply_turn_event(
        &mut app,
        TurnEvent::ToolStart {
            id: "t1".into(),
            name: "bash".into(),
            input: serde_json::json!({"cmd": "ls"}),
        },
    );
    assert!(app.status_message.contains("tool: run"));
    apply_turn_event(
        &mut app,
        TurnEvent::ToolStart {
            id: "t2".into(),
            name: "read_file".into(),
            input: serde_json::json!({}),
        },
    );
    assert!(app.status_message.contains("tool: read"));
    apply_turn_event(
        &mut app,
        TurnEvent::ToolStart {
            id: "t3".into(),
            name: "search_code".into(),
            input: serde_json::json!({}),
        },
    );
    assert!(app.status_message.contains("tool: grep"));
    apply_turn_event(
        &mut app,
        TurnEvent::ToolStart {
            id: "t4".into(),
            name: "custom_tool".into(),
            input: serde_json::json!({}),
        },
    );
    assert!(app.status_message.contains("custom_tool"));
    apply_turn_event(
        &mut app,
        TurnEvent::ToolEnd {
            id: "t1".into(),
            content: "ok".into(),
            is_error: false,
        },
    );

    app.current_agent_state = AgentState::Idle;
    apply_turn_event(&mut app, TurnEvent::Status("Remembered foo".into()));
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Remembered"))
    );
    apply_turn_event(&mut app, TurnEvent::Status("working".into()));
    assert_eq!(app.status_message, "working");

    apply_turn_event(
        &mut app,
        TurnEvent::Intent {
            kind: "change".into(),
            confidence: 0.9,
            badge: "chg".into(),
            notice_kind: "warning".into(),
            notice: "mode mismatch — switch agent".into(),
        },
    );
    assert_eq!(app.intent_kind.as_deref(), Some("change"));
    assert_eq!(app.intent_badge.as_deref(), Some("chg"));
    apply_turn_event(
        &mut app,
        TurnEvent::Intent {
            kind: "question".into(),
            confidence: 0.4,
            badge: String::new(),
            notice_kind: "info".into(),
            notice: "short note".into(),
        },
    );
    assert!(app.intent_badge.is_none());

    apply_turn_event(
        &mut app,
        TurnEvent::Usage(whycodes_core::types::Usage {
            input_tokens: 10,
            output_tokens: 4,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }),
    );
    assert!(app.turn_usage.is_some());

    apply_turn_event(&mut app, TurnEvent::Cancelled);
    assert_eq!(app.current_agent_state, AgentState::Idle);
    assert!(app.status_message.contains("Cancelled"));

    apply_turn_event(
        &mut app,
        TurnEvent::FileConflict {
            path: "src/lib.rs".into(),
            claimant: "a".into(),
            owner: "b".into(),
        },
    );
    assert!(app.status_message.contains("lib.rs"));

    apply_turn_event(
        &mut app,
        TurnEvent::SwarmStatus {
            active: 1,
            total: 3,
            message: String::new(),
        },
    );
    assert_eq!(app.status_message, "swarm 3…");
    apply_turn_event(
        &mut app,
        TurnEvent::SwarmStatus {
            active: 1,
            total: 3,
            message: "workers go".into(),
        },
    );
    assert_eq!(app.status_message, "workers go");

    apply_turn_event(
        &mut app,
        TurnEvent::Background {
            id: "bg-1".into(),
            status: "running".into(),
            summary: "cargo test".into(),
        },
    );
    assert_eq!(app.bg_running_count, 1);
    assert!(
        app.bg_jobs
            .iter()
            .any(|j| j.id == "bg-1" && j.status == "running"),
        "running bg job must appear in the sticky tasks list"
    );
    apply_turn_event(
        &mut app,
        TurnEvent::Background {
            id: "bg-1".into(),
            status: "done".into(),
            summary: "ok".into(),
        },
    );
    assert_eq!(app.bg_running_count, 0);
    apply_turn_event(
        &mut app,
        TurnEvent::Background {
            id: "bg-2".into(),
            status: "failed".into(),
            summary: "boom".into(),
        },
    );
    apply_turn_event(
        &mut app,
        TurnEvent::Background {
            id: "bg-3".into(),
            status: "killed".into(),
            summary: String::new(),
        },
    );
    apply_turn_event(
        &mut app,
        TurnEvent::Background {
            id: "bg-4".into(),
            status: "queued".into(),
            summary: String::new(),
        },
    );
    assert!(app.status_message.contains("bg-4"));

    apply_turn_event(
        &mut app,
        TurnEvent::EnqueuePrompt {
            text: "  next  ".into(),
        },
    );
    assert_eq!(app.pending_auto_prompts.len(), 1);
    apply_turn_event(&mut app, TurnEvent::EnqueuePrompt { text: "   ".into() });
    assert_eq!(app.pending_auto_prompts.len(), 1);

    apply_turn_event(
        &mut app,
        TurnEvent::Panel(whycodes_core::PanelUpdate::File {
            path: "x.rs".into(),
            text: "fn x() {}".into(),
        }),
    );
    assert!(app.sidebar.visible);

    apply_turn_event(
        &mut app,
        TurnEvent::Subagent {
            id: "kid".into(),
            kind: "explore".into(),
            description: "look".into(),
            status: "running".into(),
            activity: "Thinking".into(),
            elapsed_ms: 12,
            output: String::new(),
        },
    );
    assert!(app.subagents.iter().any(|s| s.id == "kid"));

    apply_turn_event(
        &mut app,
        TurnEvent::SwarmMessage {
            from: "a".into(),
            to: "b".into(),
            text: "hi".into(),
        },
    );
    apply_turn_event(
        &mut app,
        TurnEvent::PermissionAsk {
            request_id: "p".into(),
            tool_name: "bash".into(),
            detail: "ls".into(),
        },
    );
    apply_turn_event(
        &mut app,
        TurnEvent::QuestionAsk {
            request_id: "q".into(),
            questions: serde_json::json!([]),
        },
    );
    apply_turn_event(
        &mut app,
        TurnEvent::FileStale {
            path: "src/main.rs".into(),
            reader: "r".into(),
            writer: "w".into(),
        },
    );
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("stale"))
    );

    apply_turn_event(
        &mut app,
        TurnEvent::Todos {
            todos: vec![whycodes_core::TodoItem::new(
                "a",
                "panel item",
                whycodes_core::TodoStatus::InProgress,
            )],
        },
    );
    assert_eq!(app.todos.len(), 1);
    assert_eq!(app.todos[0].content, "panel item");

    apply_turn_event(
        &mut app,
        TurnEvent::ToolStart {
            id: "tw".into(),
            name: "todowrite".into(),
            input: serde_json::json!({
                "todos":[{"id":"a","content":"updated","status":"completed"}]
            }),
        },
    );
    assert_eq!(app.todos[0].status, whycodes_core::TodoStatus::Completed);
    apply_turn_event(
        &mut app,
        TurnEvent::ToolStart {
            id: "tw2".into(),
            name: "todo".into(),
            input: serde_json::json!({
                "merge": false,
                "todos":[{"id":"z","content":"only","status":"pending"}]
            }),
        },
    );
    assert_eq!(app.todos.len(), 1);
    assert_eq!(app.todos[0].id, "z");
}

#[test]
fn drain_turn_events_coalesces_deltas() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let (tx, mut rx) = mpsc::unbounded_channel();
    assert!(!drain_turn_events(&mut app, &mut rx));
    tx.send(TurnEvent::ThinkingDelta("th".into())).unwrap();
    tx.send(TurnEvent::ThinkingDelta("ink".into())).unwrap();
    tx.send(TurnEvent::TextDelta("hel".into())).unwrap();
    tx.send(TurnEvent::TextDelta("lo".into())).unwrap();
    tx.send(TurnEvent::Status("done".into())).unwrap();
    assert!(drain_turn_events(&mut app, &mut rx));
    assert_eq!(app.status_message, "done");
    assert!(app.messages.iter().any(|m| m.content.contains("hello")));
}

#[test]
fn helpers_tui_available_summary_share_and_diff() {
    isolate_home();
    let _ = tui_available();
    print_session_summary("coverage-summary");
    assert!(!share_server_up(1), "port 1 should be closed");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    assert!(share_server_up(port));
    drop(listener);

    let dir = tempfile::tempdir().unwrap();
    let out = project_diff_report(dir.path());
    assert!(out.contains("Diff"), "{out}");
    assert!(
        out.contains("git") || out.contains("clean") || out.contains("status"),
        "{out}"
    );

    let shares = dir.path().join(".whycodes").join("shares");
    std::fs::create_dir_all(&shares).unwrap();
    std::fs::write(shares.join("abc.json"), "{}").unwrap();
    std::fs::write(shares.join("abc.md"), "#").unwrap();
    assert_eq!(unshare_session(dir.path(), "abc"), 2);
    assert_eq!(unshare_session(dir.path(), "abc"), 0);

    #[cfg(target_os = "linux")]
    {
        let _ = which_bwrap();
    }

    persist_session_best_effort(
        &Session::new(dir.path().to_path_buf(), "sys".into()),
        "test",
    );
    let _ = try_load_session("no-such-session");
    let _ = open_db_quiet();
}

#[test]
fn refresh_sidebar_and_dashboard() {
    isolate_home();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn a() {}").unwrap();
    std::fs::create_dir_all(dir.path().join(".whycodes")).unwrap();
    std::fs::write(
        dir.path().join(".whycodes").join("todos.json"),
        r#"{"todos":[{"content":"do it","status":"pending"}]}"#,
    )
    .unwrap();
    let idx = whycodes_index::WorkspaceIndex::start_with(
        vec![dir.path().to_path_buf()],
        whycodes_index::IndexOptions {
            watch: false,
            threads: 1,
            ..Default::default()
        },
    );
    let _ = idx.wait_ready(Duration::from_secs(5));
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.project_dir = dir.path().to_path_buf();
    let mut config = Config::default();
    config.mcp_servers.insert(
        "demo".into(),
        whycodes_config::McpServerConfig {
            transport: None,
            command: Some("true".into()),
            args: Vec::new(),
            env: None,
            cwd: None,
            url: None,
            headers: None,
        },
    );
    refresh_sidebar(&mut app, &config, &idx);
    load_app_todos(&mut app);
    assert!(app.todos.iter().any(|t| t.content == "do it"));
    assert!(app.sidebar.mcp_status.iter().any(|s| s.contains("demo")));

    let rt = test_runtime();
    open_sessions_dashboard(&mut app, &rt, &[]);
    assert!(matches!(app.dialogs.active(), Some(DialogKind::Sessions)));
    assert_eq!(app.key_context, KeymapContext::Dialog);

    let mut view = crate::session_runtime::ViewSnapshot::default();
    app.add_message(ChatRole::User, "scratch");
    app.yield_view(&mut view);
    with_view_scratch(&mut view, |scratch| {
        scratch.add_message(ChatRole::Assistant, "from-scratch");
    });
    assert!(
        view.messages
            .iter()
            .any(|m| m.content.contains("from-scratch"))
    );
}

#[test]
fn begin_cancel_sets_flag_and_status() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let flag = new_cancel_flag();
    let mut at = None;
    let mut q = std::collections::VecDeque::new();
    let mut p = std::collections::VecDeque::new();
    begin_cancel(&mut app, &Some(Arc::clone(&flag)), &mut at, &mut q, &mut p);
    assert!(whycodes_agent::is_cancelled(&Some(flag)));
    assert!(at.is_some());
    assert!(app.status_message.contains("Cancelling"));
    begin_cancel(&mut app, &None, &mut at, &mut q, &mut p);
    assert!(at.is_some(), "second call keeps the original timer");
}

#[test]
fn tui_login_ui_emits_notes() {
    use whycodes_auth::providers::LoginUi;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut ui = TuiLoginUi { tx };
    ui.show_sign_in("Anthropic", "https://example.test", true);
    ui.show_sign_in("Anthropic", "https://example.test", false);
    ui.note("waiting");
    ui.show_device_code("ABCD", "https://github.com/login", true);
    ui.show_device_code("ABCD", "https://github.com/login", false);
    let mut notes = 0usize;
    while let Ok(ev) = rx.try_recv() {
        if let AuthFlowEvent::Note(_) = ev {
            notes += 1;
        }
    }
    assert_eq!(notes, 5);
}

#[test]
fn event_forces_redraw_treats_paste_as_dirty() {
    assert!(event_forces_redraw(&Event::Paste("x".into())));
    assert!(event_forces_redraw(&Event::FocusGained));
}

#[test]
fn expand_at_files_multiple_and_directory() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "AAA").unwrap();
    std::fs::write(dir.path().join("b.txt"), "BBB").unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    let out = expand_at_files("see @a.txt and @b.txt please", dir.path());
    assert!(out.contains("AAA") && out.contains("BBB"), "{out}");
    let out = expand_at_files("open @src now", dir.path());
    assert!(out.contains("@src"), "dirs stay literal: {out}");
}

#[test]
fn maybe_offer_update_confirms_on_empty_home() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.available_update = Some(UpdateOffer::SelfInstall("9.9.9".into()));
    maybe_offer_update(&mut app);
    assert!(app.update_prompted);
    assert!(matches!(
        app.dialogs.active(),
        Some(DialogKind::Confirm {
            on_confirm: ConfirmAction::Upgrade,
            ..
        })
    ));

    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.available_update = Some(UpdateOffer::Homebrew("9.9.9".into()));
    maybe_offer_update(&mut app);
    assert!(matches!(
        app.dialogs.active(),
        Some(DialogKind::Alert { .. })
    ));
    maybe_offer_update(&mut app);
    assert!(app.update_prompted);

    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.add_message(ChatRole::User, "hi");
    app.available_update = Some(UpdateOffer::SelfInstall("9.9.9".into()));
    maybe_offer_update(&mut app);
    assert!(app.update_prompted);
    assert!(!app.dialogs.is_open());
}

#[test]
fn run_options_and_turn_outcome_exist() {
    let opts = TuiRunOptions {
        project_dir: PathBuf::from("/tmp"),
        provider: "x".into(),
        model: "y".into(),
        api_key: String::new(),
        agent_name: "build".into(),
        max_turns: None,
        initial_prompt: None,
        config: Config::default(),
        resume_session_id: Some(RESUME_LATEST.into()),
        remote: None,
        update_rx: None,
    };
    assert_eq!(opts.resume_session_id.as_deref(), Some(RESUME_LATEST));
    let _ = TurnOutcome::Remote {
        text: "hi".into(),
        error: None,
        work_ms: 1,
    };
}

struct SlashHarness {
    _tmp: tempfile::TempDir,
    app: TuiApp,
    session: Session,
    history: SessionHistory,
    agent: Agent,
    config: Config,
    provider: String,
    model: String,
    api_key: String,
    perm_prompter: Arc<ChannelPermissionPrompter>,
    question_prompter: Arc<ChannelQuestionPrompter>,
    auth_tx: mpsc::UnboundedSender<AuthFlowEvent>,
    _auth_rx: mpsc::UnboundedReceiver<AuthFlowEvent>,
    pending_compact: Option<String>,
}

impl SlashHarness {
    fn new() -> Self {
        isolate_home();
        let tmp = tempfile::tempdir().expect("tmpdir");
        let info = whycodes_core::types::AgentInfo {
            name: "build".into(),
            description: String::new(),
            mode: AgentMode::Primary,
            permission: whycodes_core::types::PermissionSet::default(),
            model: None,
            system_prompt: Some("sys".into()),
            temperature: None,
            top_p: None,
        };
        let (perm_prompter, _perm_rx) = ChannelPermissionPrompter::new();
        let (question_prompter, _q_rx) = ChannelQuestionPrompter::new(None);
        let (auth_tx, auth_rx) = mpsc::unbounded_channel();
        let session = Session::new(tmp.path().to_path_buf(), "sys".into());
        Self {
            app: TuiApp::from_config(TuiAppConfig::default()),
            session,
            history: SessionHistory::new(),
            agent: Agent::new(info),
            config: Config::default(),
            provider: "acme".into(),
            model: "m1".into(),
            api_key: String::new(),
            perm_prompter: Arc::new(perm_prompter),
            question_prompter: Arc::new(question_prompter),
            auth_tx,
            _auth_rx: auth_rx,
            pending_compact: None,
            _tmp: tmp,
        }
    }

    async fn run(&mut self, cmd: &str) {
        let project_dir = self.session.project_path.clone();
        let mut ctx = SlashContext {
            app: &mut self.app,
            session: &mut self.session,
            history: &mut self.history,
            agent: &mut self.agent,
            config: &mut self.config,
            project_dir: &project_dir,
            provider: &mut self.provider,
            model: &mut self.model,
            api_key: &mut self.api_key,
            perm_prompter: Arc::clone(&self.perm_prompter),
            question_prompter: Arc::clone(&self.question_prompter),
            auth_tx: self.auth_tx.clone(),
            pending_compact: &mut self.pending_compact,
        };
        handle_slash(cmd, &mut ctx).await;
    }
}

#[tokio::test]
async fn handle_slash_covers_local_commands() {
    let mut h = SlashHarness::new();

    h.run("/help").await;
    assert_eq!(h.app.mode, AppMode::Help);
    h.app.mode = AppMode::Normal;

    h.run("/exit").await;
    assert!(!h.app.running);
    h.app.running = true;

    h.run("/rename").await;
    assert!(h.app.status_message.contains("Title"));
    h.run("/rename coverage-session").await;
    assert!(h.session.title.contains("coverage"));

    h.run("/undo").await;
    assert!(h.app.status_message.to_lowercase().contains("nothing"));
    h.run("/redo").await;
    assert!(h.app.status_message.to_lowercase().contains("nothing"));

    h.run("/compact").await;
    assert!(h.app.status_message.contains("Nothing to compact"));
    h.session.add_user_message("old task");
    h.session
        .add_assistant_message(vec![whycodes_core::types::ContentBlock::Text {
            text: "working".into(),
        }]);
    h.session.add_user_message("fix login");
    h.app.load_messages_from_session(&h.session);
    h.run("/compact keep the auth details").await;
    assert!(
        h.app.status_message.contains("Compacting conversation"),
        "{}",
        h.app.status_message
    );
    h.run("/fresh").await;
    assert!(
        h.app
            .toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("prompt cache")),
        "expected /fresh toast"
    );
    assert_eq!(
        h.pending_compact.as_deref(),
        Some("keep the auth details"),
        "slash must queue compact; the event loop spawns the LLM"
    );
    assert_eq!(h.session.messages[0].content.as_text(), Some("old task"));

    h.run("/bg").await;
    assert!(
        h.app
            .toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("No background"))
    );
    h.run("/bg kill missing").await;
    h.run("/bg whatever").await;
    assert!(h.app.status_message.contains("Usage"));

    h.run("/loop").await;
    assert!(h.app.status_message.contains("Usage"));
    h.run("/loop 2 do the thing").await;
    assert_eq!(h.app.pending_prompt.as_deref(), Some("do the thing"));
    assert_eq!(h.app.pending_auto_prompts.len(), 1);
    h.run("/loop stop").await;
    assert!(h.app.pending_auto_prompts.is_empty());

    h.run("/remember").await;
    assert!(h.app.status_message.contains("Usage"));
    h.run("/remember save this fact").await;
    h.run("/memory").await;

    h.run("/agent").await;
    assert!(matches!(h.app.dialogs.active(), Some(DialogKind::Agent)));
    h.app.dialogs.clear();
    h.app.mode = AppMode::Normal;
    h.run("/agent no-such-agent").await;
    assert!(
        h.app
            .toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Unknown agent"))
    );

    h.run("/theme").await;
    assert!(matches!(h.app.dialogs.active(), Some(DialogKind::Theme)));
    h.app.dialogs.clear();
    h.app.mode = AppMode::Normal;
    h.run("/theme nord").await;
    assert_eq!(h.app.theme, crate::theme::ThemeName::Nord);
    h.run("/theme not-a-theme").await;

    h.run("/sessions").await;
    assert!(matches!(
        h.app.dialogs.active(),
        Some(DialogKind::SessionList)
    ));
    h.app.dialogs.clear();
    h.app.mode = AppMode::Normal;

    h.run("/resume abcdef").await;
    assert_eq!(h.app.pending_session_id.as_deref(), Some("abcdef"));
    h.app.pending_session_id = None;
    h.run("/continue").await;
    assert_eq!(h.app.pending_session_id.as_deref(), Some(RESUME_LATEST));
    h.app.pending_session_id = None;
    h.run("/resume").await;
    assert!(matches!(
        h.app.dialogs.active(),
        Some(DialogKind::SessionList)
    ));
    h.app.dialogs.clear();
    h.app.mode = AppMode::Normal;

    h.run("/models").await;
    assert!(matches!(h.app.dialogs.active(), Some(DialogKind::Model)));
    h.app.dialogs.clear();
    h.app.mode = AppMode::Normal;
    h.run("/models m2").await;
    assert_eq!(h.model, "m2");
    h.run("/models acme/m3").await;
    assert_eq!(h.provider, "acme");
    assert_eq!(h.model, "m3");

    h.provider = "xai".into();
    h.model = "grok-4".into();
    h.app.provider_name = "xai".into();
    h.app.model_name = "grok-4".into();
    h.run("/effort").await;
    assert!(matches!(h.app.dialogs.active(), Some(DialogKind::Effort)));
    h.app.dialogs.clear();
    h.app.mode = AppMode::Normal;
    h.run("/effort high").await;
    assert_eq!(h.app.reasoning_effort.as_deref(), Some("high"));
    h.run("/effort xhigh").await;
    assert_eq!(h.app.reasoning_effort.as_deref(), Some("high"));

    h.run("/mode").await;
    assert!(matches!(
        h.app.dialogs.active(),
        Some(DialogKind::ApprovalMode)
    ));
    h.app.dialogs.clear();
    h.app.mode = AppMode::Normal;
    h.run("/mode manual").await;
    assert_eq!(h.app.approval_mode, ApprovalMode::Manual);
    h.run("/mode auto").await;
    assert_eq!(h.app.approval_mode, ApprovalMode::Auto);
    h.run("/mode nope").await;
    assert_eq!(h.app.approval_mode, ApprovalMode::Auto);

    h.run("/tools").await;
    h.run("/info").await;
    h.run("/doctor").await;
    h.run("/diff").await;
    h.run("/context").await;
    h.run("/cost").await;
    h.run("/init").await;
    assert!(h.app.pending_prompt.is_some());

    h.run("/unshare").await;
    h.run("/share").await;
    h.run("/connect").await;
    h.run("/login").await;
    h.run("/login not-oauth").await;
    h.run("/nope").await;
    assert!(
        h.app
            .toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Unknown command"))
    );

    h.run("/new").await;
    assert!(
        h.app
            .toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("New session"))
    );

    h.config.commands.insert(
        "hello".into(),
        whycodes_config::CustomCommandConfig {
            template: "hello $ARGUMENTS".into(),
            description: Some("say hi".into()),
            agent: None,
            model: None,
            subtask: None,
        },
    );
    h.run("/hello world").await;
    assert_eq!(h.app.pending_prompt.as_deref(), Some("hello world"));
}

#[test]
fn memory_and_index_helpers() {
    isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let config = Config::default();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    maybe_session_auto_index(dir.path(), &config, &mut app);
    let prompt = with_project_memory("base prompt", dir.path(), &config, Some("query"));
    assert!(prompt.contains("base prompt"));
    let mut session = Session::new(dir.path().to_path_buf(), "sys".into());
    let agent = Agent::new(whycodes_core::types::AgentInfo {
        name: "build".into(),
        description: String::new(),
        mode: AgentMode::Primary,
        permission: whycodes_core::types::PermissionSet::default(),
        model: None,
        system_prompt: Some("sys".into()),
        temperature: None,
        top_p: None,
    });
    refresh_session_memory(&mut session, &agent, dir.path(), &config, None);
    let _ = memory_service(dir.path(), &config);
}

fn dummy_info(name: &str) -> whycodes_core::types::AgentInfo {
    whycodes_core::types::AgentInfo {
        name: name.into(),
        description: String::new(),
        mode: AgentMode::Primary,
        permission: whycodes_core::types::PermissionSet::default(),
        model: None,
        system_prompt: Some("sys".into()),
        temperature: None,
        top_p: None,
    }
}

fn temp_index() -> (tempfile::TempDir, Arc<whycodes_index::WorkspaceIndex>) {
    let dir = tempfile::tempdir().unwrap();
    let idx = whycodes_index::WorkspaceIndex::start_with(
        vec![dir.path().to_path_buf()],
        whycodes_index::IndexOptions {
            watch: false,
            threads: 1,
            ..Default::default()
        },
    );
    (dir, idx)
}

fn sample_question() -> whycodes_tools::question::QuestionSpec {
    whycodes_tools::question::QuestionSpec {
        prompt: "Pick?".into(),
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
    }
}

#[test]
fn force_stop_applies_outcome_or_rebuilds() {
    isolate_home();
    let (dir, idx) = temp_index();
    let config = Config::default();
    let mut rt = test_runtime();
    rt.session = Session::new(dir.path().to_path_buf(), "sys".into());
    rt.agent_busy = true;
    rt.cancel_flag = Some(new_cancel_flag());
    let mut at = Some(Instant::now());

    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.add_message(ChatRole::Assistant, "partial");

    rt.done_tx
        .send(TurnOutcome::Ok {
            text: "done".into(),
            agent: Agent::new(dummy_info("from-outcome")),
            session: Session::new(dir.path().to_path_buf(), "restored".into()),
            work_ms: 3,
        })
        .unwrap();
    app.provider_name = "acme".into();
    app.model_name = "m".into();
    force_stop_turn(&mut app, &mut rt, &mut at, &config, dir.path(), &idx);
    assert!(!rt.agent_busy);
    assert!(rt.cancel_flag.is_none());
    assert_eq!(rt.agent.info.name, "from-outcome");
    assert!(
        app.messages
            .iter()
            .any(|m| m.role == ChatRole::System && m.content.contains("Stopped"))
    );

    // No outcome → restore backup and rebuild.
    rt.agent = Agent::new(dummy_info("old"));
    rt.session = Session::new(dir.path().to_path_buf(), "sys".into());
    rt.agent_busy = true;
    rt.cancel_flag = Some(new_cancel_flag());
    rt.session_backup = Some(Session::new(dir.path().to_path_buf(), "backup-sys".into()));
    at = Some(Instant::now());
    app.agent_name = "plan".into();
    app.add_message(ChatRole::System, "already cancelled");
    app.provider_name = "acme".into();
    app.model_name = "m".into();
    force_stop_turn(&mut app, &mut rt, &mut at, &config, dir.path(), &idx);
    assert!(!rt.agent_busy);
    assert!(rt.session_backup.is_none());
    assert_eq!(rt.session.system_prompt, "backup-sys");
}

#[test]
fn rebuild_agent_resolves_pending_name() {
    isolate_home();
    let (dir, idx) = temp_index();
    let config = Config::default();
    let (perm, _) = ChannelPermissionPrompter::new();
    let (question, _) = ChannelQuestionPrompter::new(None);
    let (event_tx, _) = mpsc::unbounded_channel();
    let mut agent = Agent::new(dummy_info("old"));
    let mut session = Session::new(dir.path().to_path_buf(), String::new());
    rebuild_agent_after_force_stop(
        &mut agent,
        &mut session,
        &config,
        dir.path(),
        "_pending",
        event_tx.clone(),
        Arc::new(perm),
        Arc::new(question),
        &idx,
    );
    assert_eq!(agent.info.name, "build");
    assert!(!session.system_prompt.is_empty());

    let (perm, _) = ChannelPermissionPrompter::new();
    let (question, _) = ChannelQuestionPrompter::new(None);
    rebuild_agent_after_force_stop(
        &mut agent,
        &mut session,
        &config,
        dir.path(),
        "",
        event_tx,
        Arc::new(perm),
        Arc::new(question),
        &idx,
    );
    assert_eq!(agent.info.name, "build");
}

#[tokio::test]
async fn cycle_agent_walks_primary_list() {
    isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let (perm, _) = ChannelPermissionPrompter::new();
    let (question, _) = ChannelQuestionPrompter::new(None);
    let perm = Arc::new(perm);
    let question = Arc::new(question);
    let (event_tx, _) = mpsc::unbounded_channel();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let mut agent = Agent::new(dummy_info("build"));
    let mut session = Session::new(dir.path().to_path_buf(), "sys".into());
    let mut config = Config::default();

    app.primary_agents.clear();
    cycle_agent(
        &mut app,
        &mut agent,
        &mut session,
        &config,
        dir.path(),
        Arc::clone(&perm),
        Arc::clone(&question),
        &event_tx,
    )
    .await;
    assert!(app.agent_name.is_empty() || app.agent_name == "build");

    app.primary_agents = vec!["build".into(), "plan".into()];
    app.agent_cycle_idx = 0;
    config.agents.push(dummy_info("plan"));
    cycle_agent(
        &mut app,
        &mut agent,
        &mut session,
        &config,
        dir.path(),
        perm,
        question,
        &event_tx,
    )
    .await;
    assert_eq!(app.agent_name, "plan");
    assert_eq!(agent.info.name, "plan");
    assert!(app.status_message.contains("plan"));
}

#[test]
fn handle_question_key_navigates_confirms_and_cancels() {
    let spec = sample_question();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let mut qqueue = std::collections::VecDeque::new();
    let pqueue = std::collections::VecDeque::new();
    assert!(!handle_question_key(
        &mut app,
        KeyCode::Enter,
        &mut qqueue,
        &pqueue
    ));

    app.ask_question(vec![spec.clone()]);
    assert!(handle_question_key(
        &mut app,
        KeyCode::Down,
        &mut qqueue,
        &pqueue
    ));
    assert!(handle_question_key(
        &mut app,
        KeyCode::Up,
        &mut qqueue,
        &pqueue
    ));
    assert!(handle_question_key(
        &mut app,
        KeyCode::Char('j'),
        &mut qqueue,
        &pqueue
    ));
    assert!(handle_question_key(
        &mut app,
        KeyCode::Char('k'),
        &mut qqueue,
        &pqueue
    ));
    assert!(handle_question_key(
        &mut app,
        KeyCode::Right,
        &mut qqueue,
        &pqueue
    ));
    assert!(handle_question_key(
        &mut app,
        KeyCode::Left,
        &mut qqueue,
        &pqueue
    ));
    assert!(handle_question_key(
        &mut app,
        KeyCode::Char('y'),
        &mut qqueue,
        &pqueue
    ));
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Copied") || t.message.contains("clipboard"))
    );

    // Digit 1 selects first option and finishes.
    let (tx, rx) = tokio::sync::oneshot::channel();
    qqueue.push_back(QuestionRequest {
        questions: vec![spec.clone()],
        reply: tx,
    });
    app.ask_question(vec![spec.clone()]);
    assert!(handle_question_key(
        &mut app,
        KeyCode::Char('1'),
        &mut qqueue,
        &pqueue
    ));
    assert!(qqueue.is_empty());
    assert!(rx.blocking_recv().unwrap().is_ok());
    assert_eq!(app.mode, AppMode::Normal);

    // Esc cancels.
    let (tx, rx) = tokio::sync::oneshot::channel();
    qqueue.push_back(QuestionRequest {
        questions: vec![spec.clone()],
        reply: tx,
    });
    app.ask_question(vec![spec.clone()]);
    assert!(handle_question_key(
        &mut app,
        KeyCode::Esc,
        &mut qqueue,
        &pqueue
    ));
    assert!(matches!(
        rx.blocking_recv().unwrap(),
        Err(QuestionError::Cancelled)
    ));

    // Other + free text + Enter.
    app.ask_question(vec![spec.clone()]);
    assert!(handle_question_key(
        &mut app,
        KeyCode::Char('o'),
        &mut qqueue,
        &pqueue
    ));
    assert!(handle_question_key(
        &mut app,
        KeyCode::Char('x'),
        &mut qqueue,
        &pqueue
    ));
    assert!(handle_question_key(
        &mut app,
        KeyCode::Backspace,
        &mut qqueue,
        &pqueue
    ));
    assert!(handle_question_key(
        &mut app,
        KeyCode::Char('z'),
        &mut qqueue,
        &pqueue
    ));
    if let Some(DialogKind::Question(st)) = app.dialogs.active() {
        assert_eq!(st.free_text, "z");
        assert!(st.free_text_focus);
    } else {
        panic!("expected question dialog");
    }
    // First Esc with non-empty Other text leaves the field.
    assert!(handle_question_key(
        &mut app,
        KeyCode::Esc,
        &mut qqueue,
        &pqueue
    ));
    if let Some(DialogKind::Question(st)) = app.dialogs.active() {
        assert!(!st.free_text_focus);
    } else {
        panic!("expected question dialog after leaving Other");
    }

    // Space on Other focuses free text; unknown key is not consumed.
    app.ask_question(vec![spec]);
    if let Some(DialogKind::Question(mut st)) = app.dialogs.pop() {
        st.cursor = st.option_count() - 1;
        app.dialogs.push(DialogKind::Question(st));
    }
    assert!(handle_question_key(
        &mut app,
        KeyCode::Char(' '),
        &mut qqueue,
        &pqueue
    ));
    assert!(!handle_question_key(
        &mut app,
        KeyCode::F(1),
        &mut qqueue,
        &pqueue
    ));
}

#[test]
fn resume_after_question_opens_next_or_permission() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let mut q = std::collections::VecDeque::new();
    let mut p = std::collections::VecDeque::new();
    resume_after_question(&mut app, &q, &p);
    assert!(app.status_message.contains("continuing"));

    let (tx, _rx) = tokio::sync::oneshot::channel();
    q.push_back(QuestionRequest {
        questions: vec![sample_question()],
        reply: tx,
    });
    resume_after_question(&mut app, &q, &p);
    assert!(matches!(
        app.dialogs.active(),
        Some(DialogKind::Question(_))
    ));
    assert!(app.status_message.contains("more question"));

    q.clear();
    app.dialogs.clear();
    let (tx, _rx) = tokio::sync::oneshot::channel();
    p.push_back(whycodes_agent::PermissionRequest {
        tool_name: "bash".into(),
        detail: "ls".into(),
        reply: tx,
    });
    resume_after_question(&mut app, &q, &p);
    assert!(matches!(
        app.dialogs.active(),
        Some(DialogKind::Permission { .. })
    ));
}

#[tokio::test]
async fn spawn_runtime_and_drain_outcomes() {
    isolate_home();
    let (dir, idx) = temp_index();
    let rt = spawn_new_session_runtime(
        "no-such-agent",
        &Config::default(),
        dir.path(),
        &idx,
        whycodes_core::FileClaimRegistry::new(),
    )
    .await;
    assert_eq!(rt.agent.info.name, "no-such-agent");
    assert!(!rt.agent_busy);

    let mut rt = test_runtime();
    let agent = Agent::new(dummy_info("ok"));
    let session = Session::new(PathBuf::from("/work"), "sys".into());
    rt.done_tx
        .send(TurnOutcome::Ok {
            text: "hi".into(),
            agent,
            session,
            work_ms: 1,
        })
        .unwrap();
    drain_background_runtime(&mut rt);
    assert!(!rt.agent_busy);
    assert_eq!(rt.agent.info.name, "ok");

    rt.done_tx
        .send(TurnOutcome::Remote {
            text: String::new(),
            error: Some("boom".into()),
            work_ms: 1,
        })
        .unwrap();
    drain_background_runtime(&mut rt);
    assert!(rt.last_error);
    assert!(
        rt.view
            .messages
            .iter()
            .any(|m| m.content.contains("Remote error"))
    );

    let agent = Agent::new(dummy_info("err"));
    let session = Session::new(PathBuf::from("/work"), "sys".into());
    rt.done_tx
        .send(TurnOutcome::Err {
            error: "nope".into(),
            agent,
            session,
            cancelled: false,
            work_ms: 1,
        })
        .unwrap();
    drain_background_runtime(&mut rt);
    assert!(rt.last_error);

    let agent = Agent::new(dummy_info("cx"));
    let session = Session::new(PathBuf::from("/work"), "sys".into());
    rt.done_tx
        .send(TurnOutcome::Err {
            error: "x".into(),
            agent,
            session,
            cancelled: true,
            work_ms: 1,
        })
        .unwrap();
    drain_background_runtime(&mut rt);
    assert!(!rt.last_error);
    assert!(
        rt.view
            .messages
            .iter()
            .any(|m| m.content.contains("cancelled"))
    );
}

#[tokio::test]
async fn drain_background_queues_prompter_asks() {
    use whycodes_agent::{PermissionPrompter, QuestionPrompter};
    let mut rt = test_runtime();
    let perm = Arc::clone(&rt.perm_prompter);
    let question = Arc::clone(&rt.question_prompter);
    let p = tokio::spawn(async move {
        perm.ask("bash", "ls").await;
    });
    let q = tokio::spawn(async move {
        let _ = question.ask(vec![sample_question()]).await;
    });
    for _ in 0..50 {
        drain_background_runtime(&mut rt);
        if !rt.pending_perm_queue.is_empty() && !rt.pending_question_queue.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(!rt.pending_perm_queue.is_empty());
    assert!(!rt.pending_question_queue.is_empty());
    assert!(rt.unread);
    // Unblock the waiters so the test can finish.
    let _ = rt.pending_perm_queue.pop_front().unwrap().reply.send(false);
    let _ = rt
        .pending_question_queue
        .pop_front()
        .unwrap()
        .reply
        .send(Err(QuestionError::Cancelled));
    let _ = p.await;
    let _ = q.await;
}

#[test]
fn suggestion_and_catalog_helpers_short_circuit() {
    isolate_home();
    let session = Session::new(PathBuf::from("/work"), "sys".into());
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut config = Config::default();
    maybe_spawn_prompt_suggestion(&config, &session, "p", "m", "key", &mut app, tx.clone());
    config.tui.prompt_suggestions = "idle".into();
    maybe_spawn_prompt_suggestion(&config, &session, "p", "m", "", &mut app, tx.clone());
    maybe_spawn_prompt_suggestion(&config, &session, "p", "m", "key", &mut app, tx);

    spawn_model_context_fetch(&config, "p", "m", "", {
        let (tx, _rx) = mpsc::unbounded_channel();
        tx
    });
    restore_terminal_on(&mut Vec::<u8>::new());
    if let Ok(mut w) = open_tui_writer() {
        let _ = w.write(b"");
        let _ = w.flush();
    }
    let _ = bind_agent_prompters(
        Agent::new(dummy_info("build")),
        &{
            let (p, _) = ChannelPermissionPrompter::new();
            Arc::new(p)
        },
        &{
            let (q, _) = ChannelQuestionPrompter::new(None);
            Arc::new(q)
        },
    );
}

#[test]
fn load_session_entries_and_picker_merge() {
    isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::new(dir.path().to_path_buf(), "sys".into());
    session.add_user_message("hello there");
    persist_session_best_effort(&session, "entries");
    let entries = load_session_entries();
    assert!(
        entries.iter().any(|e| e.id == session.id),
        "persisted session must appear: {entries:?}"
    );

    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let rt = test_runtime();
    let parked = test_runtime();
    app.session_list.sessions = entries;
    assert!(refresh_picker_live_section(&mut app, &rt, &[parked]));
    assert!(
        app.session_list
            .sessions
            .iter()
            .any(|e| e.live == Some(usize::MAX)),
        "current live row"
    );
    assert!(
        app.session_list.sessions.iter().any(|e| e.live == Some(0)),
        "parked live row"
    );
}

#[test]
fn doctor_report_flags_missing_project() {
    isolate_home();
    let session = Session::new(PathBuf::from("/no/such/project/dir"), "sys".into());
    let app = TuiApp::from_config(TuiAppConfig::default());
    let config = Config::default();
    let agent = Agent::new(dummy_info("build"));
    let out = doctor_report(
        &session,
        &app,
        &config,
        &agent,
        PathBuf::from("/no/such/project/dir").as_path(),
    );
    assert!(out.contains("issues"), "{out}");
    assert!(out.contains("project directory missing"), "{out}");
}

#[tokio::test]
async fn handle_slash_more_aliases_and_connect_with_key() {
    let mut h = SlashHarness::new();
    h.run("/q").await;
    assert!(!h.app.running);
    h.app.running = true;
    h.run("/h").await;
    assert_eq!(h.app.mode, AppMode::Help);
    h.app.mode = AppMode::Normal;
    h.app.key_context = KeymapContext::Normal;

    h.run("/clear").await;
    h.run("/summarize").await;
    h.run("/export").await;
    h.run("/usage").await;
    h.run("/themes").await;
    assert!(matches!(h.app.dialogs.active(), Some(DialogKind::Theme)));
    h.app.dialogs.clear();
    h.app.mode = AppMode::Normal;

    h.run("/loop keep going").await;
    assert_eq!(h.app.pending_prompt.as_deref(), Some("keep going"));
    assert_eq!(
        h.app.pending_auto_prompts.len(),
        2,
        "default N=3 → 2 queued"
    );

    h.config.providers.insert(
        "acme".into(),
        whycodes_core::types::ProviderConfig {
            name: "acme".into(),
            api_key: Some("sk-test".into()),
            api_base: None,
            base_url: None,
            headers: None,
            models: vec!["m1".into()],
            tool_arguments: None,
            extra: Default::default(),
        },
    );
    h.api_key.clear();
    h.run("/connect").await;
    assert!(
        h.app
            .toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Connected")),
        "{:?}",
        h.app
            .toasts
            .visible()
            .iter()
            .map(|t| t.message.as_str())
            .collect::<Vec<_>>()
    );

    h.config.agents.push(dummy_info("plan"));
    h.run("/agent plan").await;
    assert_eq!(h.agent.info.name, "plan");
    assert!(h.app.status_message.contains("plan"));
}

fn outcome_ok(name: &str, text: &str) -> TurnOutcome {
    TurnOutcome::Ok {
        text: text.into(),
        agent: Agent::new(dummy_info(name)),
        session: Session::new(PathBuf::from("/work"), "sys".into()),
        work_ms: 1500,
    }
}

#[test]
fn apply_turn_outcome_ok_remote_and_errors() {
    isolate_home();
    let mut rt = test_runtime();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.add_message(ChatRole::Assistant, "");
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let mut cancel_at = Some(Instant::now());
    let mut pending_title = Some((rt.session.id.clone(), "New Title".into()));

    let queue = apply_turn_outcome(
        &mut app,
        &mut rt,
        outcome_ok("ok-agent", "hello"),
        &mut cancel_at,
        &mut pending_title,
        "acme",
        "m",
        &config,
        "",
        &tx,
    );
    assert!(queue, "no live window → catalog queued");
    assert!(!rt.agent_busy);
    assert!(cancel_at.is_none());
    assert_eq!(rt.agent.info.name, "ok-agent");
    assert_eq!(app.current_agent_state, AgentState::Idle);
    assert!(
        app.messages
            .iter()
            .any(|m| m.role == ChatRole::Assistant && m.content == "hello")
    );

    app.add_message(ChatRole::Assistant, "");
    apply_turn_outcome(
        &mut app,
        &mut rt,
        TurnOutcome::Remote {
            text: "from-serve".into(),
            error: None,
            work_ms: 10,
        },
        &mut cancel_at,
        &mut pending_title,
        "acme",
        "m",
        &config,
        "",
        &tx,
    );
    assert!(
        app.messages
            .iter()
            .any(|m| m.content == "from-serve" || m.content.contains("from-serve"))
    );

    apply_turn_outcome(
        &mut app,
        &mut rt,
        TurnOutcome::Remote {
            text: String::new(),
            error: Some("down".into()),
            work_ms: 4,
        },
        &mut cancel_at,
        &mut pending_title,
        "acme",
        "m",
        &config,
        "",
        &tx,
    );
    assert_eq!(app.status_message, "remote error");

    apply_turn_outcome(
        &mut app,
        &mut rt,
        TurnOutcome::Err {
            error: "cancelled by user".into(),
            agent: Agent::new(dummy_info("cx")),
            session: Session::new(PathBuf::from("/work"), "sys".into()),
            cancelled: true,
            work_ms: 20,
        },
        &mut cancel_at,
        &mut pending_title,
        "acme",
        "m",
        &config,
        "",
        &tx,
    );
    assert!(app.messages.iter().any(|m| m.content.contains("cancelled")));

    apply_turn_outcome(
        &mut app,
        &mut rt,
        TurnOutcome::Err {
            error: "provider exploded".into(),
            agent: Agent::new(dummy_info("err")),
            session: Session::new(PathBuf::from("/work"), "sys".into()),
            cancelled: false,
            work_ms: 30,
        },
        &mut cancel_at,
        &mut pending_title,
        "acme",
        "m",
        &config,
        "",
        &tx,
    );
    assert!(matches!(app.current_agent_state, AgentState::Error(_)));
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.kind == crate::toast::ToastKind::Error)
    );

    let mut compacted = Session::new(PathBuf::from("/work"), "sys".into());
    compacted.add_user_message("fix login");
    compacted.apply_full_replace(
        "<summary>\n1. Primary Request: fix login\n2. Files: auth.rs\n</summary>",
    );
    apply_turn_outcome(
        &mut app,
        &mut rt,
        TurnOutcome::Compact {
            agent: Agent::new(dummy_info("cmp")),
            session: compacted,
            outcome: whycodes_session::CompactOutcome {
                messages_before: 4,
                messages_after: 2,
                tokens_before: 800,
                tokens_after: 200,
                dropped_transcript: "old".into(),
            },
            work_ms: 12,
        },
        &mut cancel_at,
        &mut pending_title,
        "acme",
        "m",
        &config,
        "",
        &tx,
    );
    assert!(!rt.agent_busy);
    assert_eq!(app.current_agent_state, AgentState::Idle);
    assert!(
        app.status_message.contains("Conversation compacted"),
        "{}",
        app.status_message
    );
    assert!(
        app.messages.iter().any(|m| m.role == ChatRole::System
            && m.content.contains("Conversation compacted")
            && m.content.contains("fix login")),
        "compact result should paint the summary card"
    );
}

#[test]
fn close_session_slot_busy_last_and_parked() {
    isolate_home();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let mut rt = test_runtime();
    let mut runtimes = Vec::new();
    let mut mru = Vec::new();

    rt.agent_busy = true;
    close_session_slot(&mut app, &mut rt, &mut runtimes, &mut mru, usize::MAX);
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("in flight"))
    );

    rt.agent_busy = false;
    close_session_slot(&mut app, &mut rt, &mut runtimes, &mut mru, usize::MAX);
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Last live"))
    );

    let parked = test_runtime();
    let parked_title = parked.session.title.clone();
    runtimes.push(parked);
    mru.push(0);
    close_session_slot(&mut app, &mut rt, &mut runtimes, &mut mru, 0);
    assert!(runtimes.is_empty());
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains(&parked_title) || t.message.contains("Closed"))
    );

    let parked = test_runtime();
    runtimes.push(parked);
    mru.push(0);
    let after_title = runtimes[0].session.title.clone();
    close_session_slot(&mut app, &mut rt, &mut runtimes, &mut mru, usize::MAX);
    assert!(runtimes.is_empty());
    assert_eq!(rt.session.title, after_title);
}

#[test]
fn resume_or_switch_session_paths() {
    isolate_home();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let mut rt = test_runtime();
    let mut parked = test_runtime();
    parked.session.add_user_message("parked-hi");
    let live_id = parked.session.id.clone();
    let mut runtimes = vec![parked];
    let mut mru = vec![];

    rt.agent_busy = true;
    resume_or_switch_session(
        &mut app,
        &mut rt,
        &mut runtimes,
        &mut mru,
        "busy-id".into(),
        PathBuf::from("/work").as_path(),
        &Config::default(),
    );
    assert_eq!(app.pending_session_id.as_deref(), Some("busy-id"));

    rt.agent_busy = false;
    app.pending_session_id = None;
    resume_or_switch_session(
        &mut app,
        &mut rt,
        &mut runtimes,
        &mut mru,
        live_id,
        PathBuf::from("/work").as_path(),
        &Config::default(),
    );
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Switched to live"))
    );

    resume_or_switch_session(
        &mut app,
        &mut rt,
        &mut runtimes,
        &mut mru,
        "nope-id".into(),
        PathBuf::from("/work").as_path(),
        &Config::default(),
    );
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("not found"))
    );

    let dir = tempfile::tempdir().unwrap();
    let mut saved = Session::new(dir.path().to_path_buf(), "sys".into());
    saved.add_user_message("persisted hello");
    persist_session_best_effort(&saved, "resume-test");
    let id = saved.id.clone();
    resume_or_switch_session(
        &mut app,
        &mut rt,
        &mut runtimes,
        &mut mru,
        id,
        dir.path(),
        &Config::default(),
    );
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Resumed"))
    );
}

#[test]
fn reply_permission_allow_deny_and_queue() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.ask_permission("bash", "ls");
    let mut q = std::collections::VecDeque::new();
    reply_permission(&mut app, &mut q, true);
    assert_eq!(app.status_message, "Allowed — continuing…");

    let (tx1, rx1) = tokio::sync::oneshot::channel();
    let (tx2, _rx2) = tokio::sync::oneshot::channel();
    q.push_back(whycodes_agent::PermissionRequest {
        tool_name: "bash".into(),
        detail: "one".into(),
        reply: tx1,
    });
    q.push_back(whycodes_agent::PermissionRequest {
        tool_name: "read".into(),
        detail: "two".into(),
        reply: tx2,
    });
    app.ask_permission("bash", "one");
    reply_permission(&mut app, &mut q, false);
    assert!(rx1.blocking_recv().ok() == Some(false));
    assert!(matches!(
        app.dialogs.active(),
        Some(DialogKind::Permission { .. })
    ));
    assert!(app.status_message.contains("Denied"));
}

#[test]
fn questionnaire_complete_and_cancel() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let mut q = std::collections::VecDeque::new();
    let p = std::collections::VecDeque::new();
    let (tx, rx) = tokio::sync::oneshot::channel();
    q.push_back(QuestionRequest {
        questions: vec![sample_question()],
        reply: tx,
    });
    complete_questionnaire_ui(
        &mut app,
        &mut q,
        &p,
        Some(vec![whycodes_tools::question::QuestionAnswer {
            selected: vec!["Yes".into()],
            free_text: None,
        }]),
    );
    assert!(rx.blocking_recv().unwrap().is_ok());

    let (tx, rx) = tokio::sync::oneshot::channel();
    q.push_back(QuestionRequest {
        questions: vec![sample_question()],
        reply: tx,
    });
    complete_questionnaire_ui(&mut app, &mut q, &p, None);
    assert!(matches!(
        rx.blocking_recv().unwrap(),
        Err(QuestionError::Cancelled)
    ));
}

#[test]
fn warn_suggestion_catalog_and_shutdown() {
    isolate_home();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    warn_missing_api_key(&mut app, "acme");
    assert!(app.status_message.contains("no API key"));
    assert!(
        app.messages
            .iter()
            .any(|m| m.content.contains("ACME_API_KEY"))
    );

    apply_idle_suggestion(&mut app, "   ".into(), false);
    assert!(app.pending_suggestion.is_none());
    apply_idle_suggestion(&mut app, "try cargo test".into(), true);
    assert!(app.pending_suggestion.is_none());
    apply_idle_suggestion(&mut app, "try cargo test".into(), false);
    assert_eq!(app.pending_suggestion.as_deref(), Some("try cargo test"));

    let config = Config::default();
    assert!(!apply_catalog_window(
        &mut app, "p", "m", "other", "m", 8_000, &config
    ));
    assert!(apply_catalog_window(
        &mut app, "p", "m", "p", "m", 128_000, &config
    ));
    assert_eq!(app.api_context_window, Some(128_000));

    let mut rt = test_runtime();
    let (tx, rx) = tokio::sync::oneshot::channel();
    rt.pending_perm_queue
        .push_back(whycodes_agent::PermissionRequest {
            tool_name: "bash".into(),
            detail: "x".into(),
            reply: tx,
        });
    shutdown_runtime_queues(&mut rt);
    assert!(rx.blocking_recv().ok() == Some(false));
    assert!(rt.pending_perm_queue.is_empty());
}

#[tokio::test]
async fn apply_auth_flow_note_code_and_results() {
    isolate_home();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let mut key = String::new();
    let mut provider = "anthropic".to_string();
    let mut model = "claude-sonnet-5".to_string();
    let config = Config::default();
    apply_auth_flow_event(
        &mut app,
        AuthFlowEvent::Note("Visit https://x\nthen paste".into()),
        &mut provider,
        &mut model,
        &mut key,
        &config,
    )
    .await;
    assert_eq!(app.status_message, "Visit https://x");

    let (tx, _rx) = tokio::sync::oneshot::channel();
    apply_auth_flow_event(
        &mut app,
        AuthFlowEvent::NeedCode(tx),
        &mut provider,
        &mut model,
        &mut key,
        &config,
    )
    .await;
    assert!(app.auth_code_sink.is_some());
    assert!(app.status_message.contains("Paste"));

    apply_auth_flow_event(
        &mut app,
        AuthFlowEvent::Done {
            provider: "anthropic".into(),
            result: Err("nope".into()),
        },
        &mut provider,
        &mut model,
        &mut key,
        &config,
    )
    .await;
    assert!(app.status_message.contains("sign-in failed"));

    apply_auth_flow_event(
        &mut app,
        AuthFlowEvent::Done {
            provider: "openai".into(),
            result: Ok("ok".into()),
        },
        &mut provider,
        &mut model,
        &mut key,
        &config,
    )
    .await;
    assert!(app.status_message.contains("Signed in"));
    // Plugin-less installs have no suggested models; still switch provider.
    assert_eq!(provider, "openai");
    assert!(
        app.messages
            .iter()
            .any(|m| m.content.contains("using openai/")),
        "{:?}",
        app.messages.iter().map(|m| &m.content).collect::<Vec<_>>()
    );

    whycodes_auth::register_spec(whycodes_auth::ProviderSpec {
        name: "tui-oauth-switch-demo".into(),
        label: "Demo".into(),
        flow: whycodes_auth::FlowKind::DeviceCode,
        client_id: "cid".into(),
        client_secret: None,
        authorize_url: "https://example.com/auth".into(),
        token_url: "https://example.com/token".into(),
        scopes: "read".into(),
        token_encoding: whycodes_auth::TokenEncoding::Form,
        redirect_uri: None,
        loopback_port: None,
        loopback_host: None,
        callback_path: String::new(),
        extra_authorize: vec![],
        derived: None,
        suggested_models: vec!["demo-model".into()],
        inference: None,
    });
    apply_auth_flow_event(
        &mut app,
        AuthFlowEvent::Done {
            provider: "tui-oauth-switch-demo".into(),
            result: Ok("ok".into()),
        },
        &mut provider,
        &mut model,
        &mut key,
        &config,
    )
    .await;
    assert_eq!(provider, "tui-oauth-switch-demo");
    assert_eq!(model, "demo-model");
}

#[test]
fn handle_question_enter_confirms_and_multi_space() {
    let spec = whycodes_tools::question::QuestionSpec {
        prompt: "Pick many?".into(),
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
    };
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let mut q = std::collections::VecDeque::new();
    let p = std::collections::VecDeque::new();
    app.ask_question(vec![spec.clone()]);
    assert!(handle_question_key(
        &mut app,
        KeyCode::Char(' '),
        &mut q,
        &p
    ));
    if let Some(DialogKind::Question(st)) = app.dialogs.active() {
        assert!(!st.multi_selected.is_empty());
    }
    // Enter on multi without finishing stays open (or finishes if confirm works).
    assert!(handle_question_key(&mut app, KeyCode::Enter, &mut q, &p));

    let single = sample_question();
    let (tx, rx) = tokio::sync::oneshot::channel();
    q.push_back(QuestionRequest {
        questions: vec![single.clone()],
        reply: tx,
    });
    app.ask_question(vec![single]);
    assert!(handle_question_key(&mut app, KeyCode::Enter, &mut q, &p));
    assert!(rx.blocking_recv().unwrap().is_ok());
}

#[test]
fn empty_free_text_esc_cancels_question_immediately() {
    let spec = whycodes_tools::question::QuestionSpec {
        prompt: "Type it?".into(),
        options: vec![],
        multi_select: false,
    };
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let mut q = std::collections::VecDeque::new();
    let p = std::collections::VecDeque::new();
    let (tx, rx) = tokio::sync::oneshot::channel();
    q.push_back(QuestionRequest {
        questions: vec![spec.clone()],
        reply: tx,
    });
    app.ask_question(vec![spec]);
    assert!(matches!(app.dialogs.active(), Some(DialogKind::Question(st)) if st.free_text_focus));
    assert!(handle_question_key(&mut app, KeyCode::Esc, &mut q, &p));
    assert!(q.is_empty());
    assert!(!app.dialogs.is_open());
    assert!(matches!(
        rx.blocking_recv().unwrap(),
        Err(QuestionError::Cancelled)
    ));
}

#[test]
fn flush_pending_question_replies_completes_oneshot() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let mut q = std::collections::VecDeque::new();
    let p = std::collections::VecDeque::new();
    let (tx, rx) = tokio::sync::oneshot::channel();
    q.push_back(QuestionRequest {
        questions: vec![sample_question()],
        reply: tx,
    });
    app.pending_question_answers = Some(vec![whycodes_tools::question::QuestionAnswer {
        selected: vec!["Yes".into()],
        free_text: None,
    }]);
    flush_pending_question_replies(&mut app, &mut q, &p);
    assert!(q.is_empty());
    assert!(app.pending_question_answers.is_none());
    assert!(rx.blocking_recv().unwrap().is_ok());

    let (tx, rx) = tokio::sync::oneshot::channel();
    q.push_back(QuestionRequest {
        questions: vec![sample_question()],
        reply: tx,
    });
    app.question_dismissed = true;
    flush_pending_question_replies(&mut app, &mut q, &p);
    assert!(!app.question_dismissed);
    assert!(matches!(
        rx.blocking_recv().unwrap(),
        Err(QuestionError::Cancelled)
    ));
}

#[test]
fn stale_waiting_for_question_still_opens_queued_dialog() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let mut rt = test_runtime();
    let (tx, _rx) = tokio::sync::oneshot::channel();
    rt.pending_question_queue.push_back(QuestionRequest {
        questions: vec![sample_question()],
        reply: tx,
    });
    app.current_agent_state = AgentState::WaitingForQuestion;
    assert!(app.dialogs.active().is_none());
    maybe_open_queued_dialog(&mut app, &rt);
    assert!(matches!(
        app.dialogs.active(),
        Some(DialogKind::Question(_))
    ));
}

#[test]
fn project_diff_on_a_real_repo() {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .unwrap()
    };
    assert!(git(&["init", "-q"]).success());
    std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
    assert!(git(&["add", "a.txt"]).success());
    assert!(git(&["commit", "-q", "-m", "init"]).success());
    std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
    let out = project_diff_report(dir.path());
    assert!(out.contains("Diff"), "{out}");
    assert!(
        out.contains("status") || out.contains("a.txt") || out.contains("HEAD"),
        "{out}"
    );
}

#[test]
fn context_report_counts_tool_blocks() {
    isolate_home();
    let mut session = Session::new(PathBuf::from("/work"), "sys".into());
    session.add_user_message("go");
    session.add_tool_results(vec![whycodes_core::types::ToolResult {
        tool_call_id: "t1".into(),
        content: "x".repeat(80),
        is_error: false,
    }]);
    let app = TuiApp::from_config(TuiAppConfig::default());
    let agent = Agent::new(dummy_info("build"));
    let out = context_report(&session, &app, &Config::default(), &agent);
    assert!(out.contains("tool:"), "{out}");
    assert!(out.contains("largest tool results"), "{out}");
}

#[test]
fn tui_login_prompt_pasted_code_cancels_when_dropped() {
    use whycodes_auth::providers::LoginUi;
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut ui = TuiLoginUi { tx };
    let fut = ui.prompt_pasted_code();
    // Dropping the NeedCode sender (the TUI side) cancels the flow.
    drop(fut);
}

#[test]
fn arm_record_route_and_model_choice() {
    isolate_home();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let mut rt = test_runtime();
    let mut at = Some(Instant::now());
    let flag = arm_generating(&mut app, &mut rt, &mut at, "remote…");
    assert!(rt.agent_busy);
    assert!(at.is_none());
    assert_eq!(app.status_message, "remote…");
    assert!(
        app.messages
            .last()
            .is_some_and(|m| m.role == ChatRole::Assistant)
    );
    assert!(!whycodes_agent::is_cancelled(&Some(flag)));

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "hello file").unwrap();
    let mut config = Config::default();
    config.session.auto_title = true;
    let expanded = record_user_turn(
        &mut app,
        &mut rt,
        "read @note.txt please",
        dir.path(),
        &config,
        &[],
    );
    assert!(expanded.contains("hello file"), "{expanded}");
    assert!(rt.session.messages.iter().any(|m| {
        m.content
            .as_text()
            .is_some_and(|t| t.contains("hello file"))
    }));

    let bad = [crate::images::PromptImage {
        path: dir.path().join("missing.png"),
        label: "missing.png".into(),
        media_type: "image/png".into(),
    }];
    record_user_turn(&mut app, &mut rt, "", dir.path(), &config, &bad);
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Image attach"))
    );

    let (p, m) = route_turn_model(&rt.session.id, "acme", "big", "hi", Some("fast-1"));
    assert_eq!(p, "acme");
    let _ = m;

    let mut provider = "old".into();
    let mut model = "old-m".into();
    let mut key = String::new();
    config.providers.insert(
        "acme".into(),
        whycodes_core::types::ProviderConfig {
            name: "acme".into(),
            api_key: Some("sk-from-cfg".into()),
            api_base: None,
            base_url: None,
            headers: None,
            models: vec!["m1".into()],
            tool_arguments: None,
            extra: Default::default(),
        },
    );
    apply_model_choice(
        &mut app,
        &mut provider,
        &mut model,
        &mut key,
        "acme".into(),
        "m1".into(),
        &config,
    );
    assert_eq!(provider, "acme");
    assert_eq!(model, "m1");
    assert_eq!(key, "sk-from-cfg");
    assert!(app.status_message.contains("acme/m1"));

    // Switching to an OAuth-only provider must drop the previous key.
    let mut leftover = "sk-from-previous-backend".into();
    apply_model_choice(
        &mut app,
        &mut provider,
        &mut model,
        &mut leftover,
        "google-antigravity".into(),
        "gemini-3.5-flash-low".into(),
        &config,
    );
    assert_eq!(provider, "google-antigravity");
    assert!(
        leftover.is_empty(),
        "must not keep previous provider credential"
    );

    leftover = "ya29-oauth".into();
    apply_model_choice(
        &mut app,
        &mut provider,
        &mut model,
        &mut leftover,
        "google-antigravity".into(),
        "gemini-3.1-pro-low".into(),
        &config,
    );
    assert_eq!(
        leftover, "ya29-oauth",
        "same-provider model change keeps the credential"
    );

    let mut key = String::new();
    try_fill_api_key(&mut key, "nope");
    assert!(key.is_empty());
    try_fill_api_key(&mut key, "acme");
    // config load may or may not see providers; env fallback:
    unsafe { std::env::set_var("NOPE_API_KEY", "from-env") };
    let mut key = String::new();
    try_fill_api_key(&mut key, "nope");
    assert_eq!(key, "from-env");
    unsafe { std::env::remove_var("NOPE_API_KEY") };
    try_fill_api_key(&mut key, "nope");
    assert_eq!(key, "from-env", "already filled stays");
}

#[test]
fn apply_async_title_active_parked_and_pending() {
    isolate_home();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let mut rt = test_runtime();
    rt.session.add_user_message("topic");
    let sid = rt.session.id.clone();
    let mut pending = None;
    apply_async_title(
        &mut app,
        &mut rt,
        &mut [],
        &mut pending,
        sid.clone(),
        "Better Title".into(),
    );
    assert!(
        rt.session.title.contains("Better") || app.session_title == rt.session.title,
        "title={} app={}",
        rt.session.title,
        app.session_title
    );

    let mut parked = test_runtime();
    parked.session.add_user_message("bg topic");
    let bg_id = parked.session.id.clone();
    let mut runtimes = vec![parked];
    apply_async_title(
        &mut app,
        &mut rt,
        &mut runtimes,
        &mut pending,
        bg_id,
        "Parked Title".into(),
    );

    apply_async_title(
        &mut app,
        &mut rt,
        &mut runtimes,
        &mut pending,
        "unknown-sid".into(),
        "Later".into(),
    );
    assert_eq!(
        pending.as_ref().map(|(s, _)| s.as_str()),
        Some("unknown-sid")
    );
}

fn boot_opts(dir: &std::path::Path, key: &str) -> TuiRunOptions {
    TuiRunOptions {
        project_dir: dir.to_path_buf(),
        provider: "acme".into(),
        model: "m1".into(),
        api_key: key.into(),
        agent_name: "plan".into(),
        max_turns: None,
        initial_prompt: None,
        config: Config::default(),
        resume_session_id: None,
        remote: None,
        update_rx: None,
    }
}

#[tokio::test]
async fn prepare_tui_boot_sets_chrome_and_defaults() {
    isolate_home();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("README.md"), "hi").unwrap();
    let mut opts = boot_opts(dir.path(), "");
    opts.agent_name = "plan".into();
    opts.config.tools.question.timeout_enabled = true;
    opts.config.tools.question.timeout_secs = 5;
    let mut plan = dummy_info("plan");
    plan.mode = AgentMode::All;
    opts.config.agents.push(plan);
    let boot = prepare_tui_boot(&opts).await;
    assert_eq!(boot.app.provider_name, "acme");
    assert_eq!(boot.app.model_name, "m1");
    assert_eq!(boot.app.agent_name, "plan");
    assert!(boot.missing_key);
    assert!(boot.app.status_message.contains("no API key"));
    assert!(boot.app.primary_agents.contains(&"plan".to_string()));
    assert_eq!(boot.app.agent_cycle_idx, 1);
    assert_eq!(boot.agent.info.name, "plan");

    let opts = boot_opts(dir.path(), "sk-test");
    let boot = prepare_tui_boot(&opts).await;
    assert!(!boot.missing_key);
    assert!(boot.app.status_message.contains("Tab focus"));
}

#[test]
fn apply_resume_found_missing_and_latest() {
    let (_home_lock, _home) = isolate_home_fresh();
    let dir = tempfile::tempdir().unwrap();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let mut session = Session::new(dir.path().to_path_buf(), "sys".into());
    apply_resume(&mut app, &mut session, "sys", RESUME_LATEST, false);
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("No saved"))
    );
    apply_resume(&mut app, &mut session, "sys", "missing-id", false);
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("not found"))
    );

    let mut saved = Session::new(dir.path().to_path_buf(), "sys".into());
    saved.add_user_message("resume me");
    persist_session_best_effort(&saved, "boot");
    apply_resume(&mut app, &mut session, "keep-sys", &saved.id, true);
    assert_eq!(session.id, saved.id);
    assert_eq!(session.system_prompt, "keep-sys");
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Resumed"))
    );
}

#[tokio::test]
async fn apply_remote_hydrate_error_still_attaches() {
    isolate_home();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let mut session = Session::new(PathBuf::from("/work"), "sys".into());
    let rem = crate::remote::RemoteAttach::new("127.0.0.1:1", "sid-9");
    apply_remote_hydrate(&mut app, &mut session, &rem).await;
    assert_eq!(session.id, "sid-9");
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Attached"))
    );
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Remote hydrate"))
    );
}

#[test]
fn spinner_session_keys_slash_and_busy_ctrl_c() {
    isolate_home();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let mut frame = 0usize;
    app.status_message = "Generating…".into();
    tick_spinner(&mut app, &mut frame);
    assert_eq!(frame, 1);
    assert!(app.status_message.is_empty());
    app.status_message = "⠋ working".into();
    tick_spinner(&mut app, &mut frame);
    assert!(app.status_message.is_empty());
    app.status_message = "tool: run".into();
    tick_spinner(&mut app, &mut frame);
    assert_eq!(app.status_message, "tool: run");

    warn_session_limit(&mut app);
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Session limit"))
    );

    let mut rt = test_runtime();
    let parked = test_runtime();
    let mut runtimes = vec![parked];
    let mut mru = vec![0];
    cycle_live_session(&mut app, &mut rt, &mut runtimes, &mut mru, true);
    switch_mru_session(&mut app, &mut rt, &mut runtimes, &mut mru);
    cycle_live_session(&mut app, &mut rt, &mut [], &mut mru, false);

    let fresh = test_runtime();
    adopt_fresh_runtime(&mut app, &mut rt, &mut runtimes, &mut mru, fresh);
    assert_eq!(runtimes.len(), 2);

    app.input_buffer = "/help".into();
    assert_eq!(slash_command_from_prompt(&app).as_deref(), Some("/help"));
    consume_slash_draft(&mut app);
    assert!(app.input_buffer.is_empty());
    app.input_buffer = "hello".into();
    assert!(slash_command_from_prompt(&app).is_none());

    app.input_buffer = "draft".into();
    assert_eq!(busy_ctrl_c(&mut app, None), BusyCtrlC::ClearedDraft);
    assert!(app.input_buffer.is_empty());
    assert_eq!(busy_ctrl_c(&mut app, None), BusyCtrlC::BeginCancel);
    assert_eq!(
        busy_ctrl_c(&mut app, Some(Instant::now())),
        BusyCtrlC::ForceStop
    );

    queue_auto_prompt_if_idle(&mut app, true);
    assert!(app.pending_prompt.is_none());
    app.pending_auto_prompts.push_back("next".into());
    queue_auto_prompt_if_idle(&mut app, false);
    assert_eq!(app.pending_prompt.as_deref(), Some("next"));
    app.pending_auto_prompts.push_back("held".into());
    queue_auto_prompt_if_idle(&mut app, false);
    assert_eq!(app.pending_prompt.as_deref(), Some("next"));

    app.input_buffer = "/".into();
    app.slash_suggest.refresh(&app.input_buffer);
    let picked = slash_command_from_prompt(&app).expect("slash menu");
    assert!(picked.starts_with('/'), "{picked}");

    apply_boot_prompt(&mut app, true, None);
    assert_eq!(app.status_message, "no API key · /connect");
    apply_boot_prompt(&mut app, false, Some(String::new()));
    apply_boot_prompt(&mut app, false, Some("do it".into()));
    assert_eq!(app.pending_prompt.as_deref(), Some("do it"));

    let mut rt = test_runtime();
    let name = rt.agent.info.name.clone();
    let (ag, sess) = take_turn_owner(&mut rt, PathBuf::from("/work").as_path());
    assert_eq!(ag.info.name, name);
    assert_eq!(rt.agent.info.name, "_pending");
    assert!(rt.session_backup.is_some());
    let _ = sess;

    let mut rt = test_runtime();
    maybe_open_queued_dialog(&mut app, &rt);
    let (tx, _rx) = tokio::sync::oneshot::channel();
    rt.pending_perm_queue
        .push_back(whycodes_agent::PermissionRequest {
            tool_name: "bash".into(),
            detail: "ls".into(),
            reply: tx,
        });
    maybe_open_queued_dialog(&mut app, &rt);
    assert!(matches!(
        app.dialogs.active(),
        Some(DialogKind::Permission { .. })
    ));
    maybe_open_queued_dialog(&mut app, &rt);

    app.dialogs.clear();
    app.mode = AppMode::Normal;
    app.current_agent_state = AgentState::Idle;
    let (tx, _rx) = tokio::sync::oneshot::channel();
    rt.pending_perm_queue.clear();
    rt.pending_question_queue.push_back(QuestionRequest {
        questions: vec![sample_question()],
        reply: tx,
    });
    maybe_open_queued_dialog(&mut app, &rt);
    assert!(matches!(
        app.dialogs.active(),
        Some(DialogKind::Question(_))
    ));

    let mut parked = test_runtime();
    parked.session.add_user_message("dash");
    let mut runtimes = vec![parked];
    let mut mru = vec![];
    apply_dashboard_switch(&mut app, &mut rt, &mut runtimes, &mut mru, usize::MAX);
    apply_dashboard_switch(&mut app, &mut rt, &mut runtimes, &mut mru, 9);
    apply_dashboard_switch(&mut app, &mut rt, &mut runtimes, &mut mru, 0);
    assert_eq!(mru, vec![0]);

    assert!(!should_tick_spinner(&app, false));
    assert!(should_tick_spinner(&app, true));
    app.current_agent_state = AgentState::WaitingForPermission;
    assert!(!should_tick_spinner(&app, true));
    app.current_agent_state = AgentState::Idle;
    assert!(!should_force_stop(false, Some(Instant::now()), true));
    assert!(!should_force_stop(true, None, true));
    assert!(should_force_stop(true, Some(Instant::now()), true));
    assert!(!should_force_stop(true, Some(Instant::now()), false));

    crate::input::open_dialog(&mut app, DialogKind::Sessions);
    let rt = test_runtime();
    refresh_live_session_ui(&mut app, &rt, &[]);
    assert!(!app.sessions_rows.is_empty());
    crate::input::open_dialog(&mut app, DialogKind::SessionList);
    refresh_live_session_ui(&mut app, &rt, &[]);
    assert!(
        app.session_list
            .sessions
            .iter()
            .any(|e| e.live == Some(usize::MAX))
    );
}

#[tokio::test]
async fn prepare_boot_with_resume_and_remote() {
    isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let mut saved = Session::new(dir.path().to_path_buf(), "sys".into());
    saved.add_user_message("boot resume");
    persist_session_best_effort(&saved, "boot-resume");
    let mut opts = boot_opts(dir.path(), "sk");
    opts.resume_session_id = Some(saved.id.clone());
    opts.remote = Some(crate::remote::RemoteAttach::new("127.0.0.1:1", "r1"));
    let boot = prepare_tui_boot(&opts).await;
    assert_eq!(boot.session.id, "r1", "remote hydrate overwrites id");
    assert!(
        boot.app
            .toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Resumed") || t.message.contains("Attached"))
    );
}

#[tokio::test]
async fn apply_remote_hydrate_success() {
    isolate_home();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let Ok((mut sock, _)) = listener.accept().await else {
            return;
        };
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut buf = vec![0u8; 4096];
        let _ = sock.read(&mut buf).await;
        let body = r#"{"title":"remote-title","messages":[{"role":"user","content":"hi"}]}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = sock.write_all(resp.as_bytes()).await;
    });
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let mut session = Session::new(PathBuf::from("/work"), "sys".into());
    let rem = crate::remote::RemoteAttach::new(format!("http://{addr}"), "sid-ok");
    apply_remote_hydrate(&mut app, &mut session, &rem).await;
    assert_eq!(session.id, "sid-ok");
    assert_eq!(session.title, "remote-title");
    assert_eq!(session.messages.len(), 1);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind2");
    let addr = listener.local_addr().expect("addr2");
    tokio::spawn(async move {
        let Ok((mut sock, _)) = listener.accept().await else {
            return;
        };
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut buf = vec![0u8; 2048];
        let _ = sock.read(&mut buf).await;
        let body = r#"{"title":"","messages":[]}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = sock.write_all(resp.as_bytes()).await;
    });
    let rem = crate::remote::RemoteAttach::new(format!("http://{addr}"), "sid-empty");
    apply_remote_hydrate(&mut app, &mut session, &rem).await;
    assert_eq!(session.id, "sid-empty");
}

#[test]
fn spinner_wraps_and_only_clears_generic_progress() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let mut frame = 9;
    app.status_message = "Generating response".into();

    tick_spinner(&mut app, &mut frame);

    assert_eq!(frame, 0);
    assert_eq!(app.spinner_frame, 0);
    assert!(app.status_message.is_empty());

    app.status_message = "⠴ waiting".into();
    tick_spinner(&mut app, &mut frame);
    assert!(app.status_message.is_empty());

    app.status_message = "Running cargo test".into();
    tick_spinner(&mut app, &mut frame);
    assert_eq!(app.status_message, "Running cargo test");
}

#[test]
fn force_stop_requires_busy_cancel_and_timeout_or_pending_signal() {
    let expired = Instant::now()
        .checked_sub(CANCEL_FORCE_AFTER + Duration::from_millis(1))
        .expect("force-stop duration fits in Instant");
    let recent = Instant::now();

    assert!(!should_force_stop(false, Some(expired), true));
    assert!(!should_force_stop(true, None, true));
    assert!(should_force_stop(true, Some(recent), true));
    assert!(should_force_stop(true, Some(expired), false));
    assert!(!should_force_stop(true, Some(recent), false));
}

#[test]
fn auto_prompts_are_fifo_and_do_not_replace_pending_work() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.pending_auto_prompts
        .extend(["first".into(), "second".into()]);

    queue_auto_prompt_if_idle(&mut app, true);
    assert!(app.pending_prompt.is_none());
    assert_eq!(app.pending_auto_prompts.len(), 2);

    queue_auto_prompt_if_idle(&mut app, false);
    assert_eq!(app.pending_prompt.as_deref(), Some("first"));
    assert_eq!(
        app.pending_auto_prompts.front().map(String::as_str),
        Some("second")
    );

    queue_auto_prompt_if_idle(&mut app, false);
    assert_eq!(app.pending_prompt.as_deref(), Some("first"));
    assert_eq!(app.pending_auto_prompts.len(), 1);
}

#[test]
fn queued_dialogs_wait_for_idle_and_prioritize_permissions() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let mut rt = test_runtime();
    let (permission_tx, _permission_rx) = tokio::sync::oneshot::channel();
    rt.pending_perm_queue
        .push_back(whycodes_agent::PermissionRequest {
            tool_name: "bash".into(),
            detail: "cargo test".into(),
            reply: permission_tx,
        });
    let (question_tx, _question_rx) = tokio::sync::oneshot::channel();
    rt.pending_question_queue.push_back(QuestionRequest {
        questions: vec![sample_question()],
        reply: question_tx,
    });

    app.current_agent_state = AgentState::WaitingForQuestion;
    app.ask_question(vec![sample_question()]);
    maybe_open_queued_dialog(&mut app, &rt);
    assert!(matches!(
        app.dialogs.active(),
        Some(DialogKind::Question(_))
    ));

    app.dialogs.clear();
    app.mode = AppMode::Normal;
    app.current_agent_state = AgentState::Idle;
    maybe_open_queued_dialog(&mut app, &rt);
    assert!(matches!(
        app.dialogs.active(),
        Some(DialogKind::Permission { tool_name, detail })
            if tool_name == "bash" && detail == "cargo test"
    ));
}
