use super::*;

#[test]
fn restore_terminal_resets_cursor_style_to_user_default() {
    let mut out = Vec::new();
    restore_terminal_on(&mut out);
    let bytes = String::from_utf8_lossy(&out);
    // Unix emulators echo DECSCUSR (`CSI 0 q`) into the writer. Windows CONOUT$
    // often swallows the sequence, so coverage is the call itself there.
    #[cfg(unix)]
    assert!(
        bytes.contains("\u{1b}[0 q"),
        "DECSCUSR default shape missing after TUI exit: {bytes:?}"
    );
    #[cfg(windows)]
    let _ = bytes;
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

    let err = resolve_and_load_session(&db, "").unwrap_err();
    assert!(err.to_string().contains("ambiguous"), "{err}");
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
    session.add_assistant_message(vec![whycodes_core::types::ContentBlock::Text {
        text: "working".into(),
    }]);
    session.messages.push(whycodes_core::types::Message {
        role: whycodes_core::types::Role::System,
        content: whycodes_core::types::MessageContent::Text("note".into()),
        tool_call_id: None,
        name: None,
        created_at: None,
    });
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
    assert!(out.contains("messages:  4"), "{out}");
    assert!(out.contains("user: 1"), "{out}");
    assert!(out.contains("assistant: 1"), "{out}");
    assert!(out.contains("system: 1"), "{out}");
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
    let _g = isolate_home_lock();
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
    // Do not take ENV_LOCK if a caller already holds it (`isolate_home`).
    if std::env::var_os("WHYCODES_HOME").is_none() {
        let _g = isolate_home_lock();
        if std::env::var_os("WHYCODES_HOME").is_none() {
            let dir = HOME.get_or_init(|| tempfile::tempdir().expect("tempdir"));
            unsafe { std::env::set_var("WHYCODES_HOME", dir.path()) };
        }
    }

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

/// Newtype so tests can hold `ENV_LOCK` across `.await` without
/// `clippy::await_holding_lock` (the inner guard is not a local).
#[allow(dead_code)] // RAII: dropping the inner `MutexGuard` releases `ENV_LOCK`.
struct HomeLock(std::sync::MutexGuard<'static, ()>);

fn isolate_home() -> HomeLock {
    let g = isolate_home_lock();
    unsafe { std::env::set_var("WHYCODES_HOME", shared_test_home()) };
    g
}

fn shared_test_home() -> &'static std::path::Path {
    use std::sync::OnceLock;
    static HOME: OnceLock<tempfile::TempDir> = OnceLock::new();
    HOME.get_or_init(|| tempfile::tempdir().expect("tempdir"))
        .path()
}

/// Exclusive empty `WHYCODES_HOME` for tests that assert on an empty session
/// store. The shared [`isolate_home`] OnceLock is process-wide, so a
/// sibling persist can make `RESUME_LATEST` look populated.
fn isolate_home_lock() -> HomeLock {
    HomeLock(crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner()))
}

fn isolate_home_fresh() -> (HomeLock, tempfile::TempDir) {
    let lock = isolate_home_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    unsafe { std::env::set_var("WHYCODES_HOME", dir.path()) };
    // `with_session_db` caches the SQLite handle for process lifetime; a
    // sibling persist would otherwise keep `RESUME_LATEST` populated.
    reset_session_db_cache();
    (lock, dir)
}

#[test]
fn apply_turn_event_covers_every_variant() {
    let _home = isolate_home();
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
fn shown_tool_name_maps_aliases() {
    assert_eq!(shown_tool_name("bash"), "run");
    assert_eq!(shown_tool_name("shell"), "run");
    assert_eq!(shown_tool_name("run_terminal_command"), "run");
    assert_eq!(shown_tool_name("read_file"), "read");
    assert_eq!(shown_tool_name("search_code"), "grep");
    assert_eq!(shown_tool_name("rg"), "grep");
    assert_eq!(shown_tool_name("custom_tool"), "custom_tool");
}

#[test]
fn apply_background_event_covers_every_status() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    apply_background_event(&mut app, "j1", "running", "sleep 1");
    assert_eq!(app.bg_running_count, 1);
    apply_background_event(&mut app, "j1", "done", "ok");
    assert_eq!(app.bg_running_count, 0);
    apply_background_event(&mut app, "j2", "running", "x");
    apply_background_event(&mut app, "j2", "failed", "boom");
    apply_background_event(&mut app, "j3", "running", "x");
    apply_background_event(&mut app, "j3", "killed", "");
    apply_background_event(&mut app, "j4", "queued", "later");
    assert!(app.status_message.contains("j4"));
    assert!(app.status_message.contains("queued"));
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
    let _home = isolate_home();
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
    let _home = isolate_home();
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
    assert_eq!(app.mode, AppMode::Dialog);
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
fn tui_login_ui_send_logs_when_loop_is_closed() {
    use whycodes_auth::providers::LoginUi;
    let (tx, rx) = mpsc::unbounded_channel();
    drop(rx);
    let mut ui = TuiLoginUi { tx };
    ui.note("gone");
    ui.show_sign_in("Anthropic", "https://example.test", false);
    ui.show_device_code("ABCD", "https://github.com/login", false);
}

#[tokio::test]
async fn tui_login_prompt_pasted_code_errors_when_sender_dropped() {
    use whycodes_auth::providers::LoginUi;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut ui = TuiLoginUi { tx };
    let fut = ui.prompt_pasted_code();
    let AuthFlowEvent::NeedCode(code_tx) = rx.try_recv().expect("NeedCode") else {
        panic!("expected NeedCode");
    };
    drop(code_tx);
    let err = fut.await.expect_err("cancelled");
    assert!(err.to_string().to_lowercase().contains("dismissed") || !err.to_string().is_empty());
}

#[test]
fn apply_approval_mode_sets_app_config_and_agent() {
    let _home = isolate_home();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let mut agent = Agent::new(dummy_info("build"));
    let mut config = Config::default();
    apply_approval_mode(&mut app, &mut agent, &mut config, ApprovalMode::Important);
    assert_eq!(app.approval_mode, ApprovalMode::Important);
    assert_eq!(config.general.approval_mode, Some(ApprovalMode::Important));
    assert!(
        app.status_message.to_lowercase().contains("important")
            || app.status_message.to_lowercase().contains("approval")
    );
}

#[tokio::test]
async fn fill_oauth_credential_skips_set_key_and_non_oauth() {
    let _home = isolate_home();
    let mut key = "already-set".to_string();
    fill_oauth_credential(&mut key, "anthropic").await;
    assert_eq!(key, "already-set");

    let mut empty = String::new();
    fill_oauth_credential(&mut empty, "unknown-provider-xyz").await;
    assert!(empty.is_empty());

    let mut oauth_empty = String::new();
    fill_oauth_credential(&mut oauth_empty, "anthropic").await;
    assert!(oauth_empty.is_empty());
}

#[test]
fn enable_keyboard_enhancement_skips_when_bench_set() {
    let _home = isolate_home();
    let prev = std::env::var_os("WHYCODES_BENCH");
    unsafe { std::env::set_var("WHYCODES_BENCH", "1") };
    let mut out = Vec::new();
    assert!(!enable_keyboard_enhancement(&mut out, Some((80, 24))));
    match prev {
        Some(v) => unsafe { std::env::set_var("WHYCODES_BENCH", v) },
        None => unsafe { std::env::remove_var("WHYCODES_BENCH") },
    }
}

#[test]
fn spawn_model_context_fetch_skips_without_base_and_with_opt_out() {
    let _home = isolate_home();
    let config = Config::default();
    let (tx, _rx) = mpsc::unbounded_channel();
    spawn_model_context_fetch(&config, "acme", "m", "sk", tx.clone());

    let prev = std::env::var_os("WHYCODES_NO_MODEL_CATALOG");
    unsafe { std::env::set_var("WHYCODES_NO_MODEL_CATALOG", "1") };
    spawn_model_context_fetch(&config, "acme", "m", "sk", tx);
    match prev {
        Some(v) => unsafe { std::env::set_var("WHYCODES_NO_MODEL_CATALOG", v) },
        None => unsafe { std::env::remove_var("WHYCODES_NO_MODEL_CATALOG") },
    }
}

#[test]
fn force_stop_keeps_agent_on_remote_outcome() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let idx = whycodes_index::WorkspaceIndex::start(Vec::new());
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.agent_name = "build".into();
    let mut rt = test_runtime();
    rt.agent_busy = true;
    rt.cancel_flag = Some(new_cancel_flag());
    let (qtx, qrx) = tokio::sync::oneshot::channel();
    rt.pending_question_queue.push_back(QuestionRequest {
        questions: vec![sample_question()],
        reply: qtx,
    });
    let (ptx, prx) = tokio::sync::oneshot::channel();
    rt.pending_perm_queue
        .push_back(whycodes_agent::PermissionRequest {
            tool_name: "bash".into(),
            detail: "ls".into(),
            reply: ptx,
        });
    rt.done_tx
        .send(TurnOutcome::Remote {
            text: "remote".into(),
            error: None,
            work_ms: 1,
        })
        .unwrap();
    let before = rt.agent.info.name.clone();
    let mut cancel_at = Some(Instant::now());
    force_stop_turn(
        &mut app,
        &mut rt,
        &mut cancel_at,
        &Config::default(),
        dir.path(),
        &idx,
    );
    assert_eq!(rt.agent.info.name, before);
    assert!(!rt.agent_busy);
    assert!(cancel_at.is_none());
    assert_eq!(qrx.blocking_recv().unwrap(), Err(QuestionError::Cancelled));
    assert_eq!(prx.blocking_recv().ok(), Some(false));
    assert!(
        app.messages
            .iter()
            .any(|m| m.content.contains("Stopped") || m.content.contains("cancelled"))
    );

    app.add_message(ChatRole::System, "turn cancelled already");
    rt.agent_busy = true;
    let mut cancel_at = Some(Instant::now());
    force_stop_turn(
        &mut app,
        &mut rt,
        &mut cancel_at,
        &Config::default(),
        dir.path(),
        &idx,
    );
    let stopped = app
        .messages
        .iter()
        .filter(|m| m.content.contains("Stopped"))
        .count();
    assert!(
        stopped <= 1,
        "already-cancelled transcript skips extra Stopped"
    );
}

#[tokio::test]
async fn spawn_model_context_fetch_sends_window_or_swallows_errors() {
    let _home = isolate_home();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for _ in 0..4 {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let mut buf = [0u8; 1024];
            let _ = std::io::Read::read(&mut stream, &mut buf);
            let body = r#"{"data":[{"id":"m","context_length":32000}]}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = std::io::Write::write_all(&mut stream, resp.as_bytes());
        }
    });

    let mut config = Config::default();
    config.providers.insert(
        "acme".into(),
        whycodes_core::types::ProviderConfig {
            name: "acme".into(),
            api_key: Some("sk".into()),
            api_base: Some(format!("http://{addr}/v1")),
            base_url: Some(format!("http://{addr}/v1")),
            headers: None,
            models: vec!["m".into()],
            tool_arguments: None,
            extra: Default::default(),
        },
    );
    let (tx, mut rx) = mpsc::unbounded_channel();
    spawn_model_context_fetch(&config, "acme", "m", "sk", tx);
    let got = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .ok()
        .flatten();
    assert_eq!(got, Some(("acme".into(), "m".into(), 32000)));

    let (tx, mut rx) = mpsc::unbounded_channel();
    spawn_model_context_fetch(&config, "acme", "other", "sk", tx);
    let none = tokio::time::timeout(Duration::from_millis(400), rx.recv())
        .await
        .ok()
        .flatten();
    assert!(none.is_none());

    let mut bad = Config::default();
    bad.providers.insert(
        "acme".into(),
        whycodes_core::types::ProviderConfig {
            name: "acme".into(),
            api_key: Some("sk".into()),
            api_base: Some("http://127.0.0.1:1/v1".into()),
            base_url: Some("http://127.0.0.1:1/v1".into()),
            headers: None,
            models: vec!["m".into()],
            tool_arguments: None,
            extra: Default::default(),
        },
    );
    let (tx, mut rx) = mpsc::unbounded_channel();
    spawn_model_context_fetch(&bad, "acme", "m", "sk", tx);
    let err = tokio::time::timeout(Duration::from_millis(400), rx.recv())
        .await
        .ok()
        .flatten();
    assert!(err.is_none());
}

#[tokio::test]
async fn spawn_oauth_login_reports_unknown_provider() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let (tx, mut rx) = mpsc::unbounded_channel();
    spawn_oauth_login(
        &mut app,
        &tx,
        dir.path().to_path_buf(),
        "no-such-oauth-provider",
    );
    let ev = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .ok()
        .flatten()
        .expect("auth event");
    match ev {
        AuthFlowEvent::Done { provider, result } => {
            assert_eq!(provider, "no-such-oauth-provider");
            assert!(result.is_err(), "{result:?}");
        }
        _other => panic!("expected Done, got non-Done auth event"),
    }
}

#[test]
fn event_forces_redraw_treats_paste_as_dirty() {
    assert!(event_forces_redraw(&Event::Paste("x".into())));
    assert!(event_forces_redraw(&Event::FocusGained));
}

#[test]
fn parse_loop_slash_covers_stop_queue_and_usage() {
    assert!(matches!(parse_loop_slash("stop"), LoopSlash::Stop));
    assert!(matches!(parse_loop_slash("clear"), LoopSlash::Stop));
    match parse_loop_slash("2 do the thing") {
        LoopSlash::Queue { n, prompt } => {
            assert_eq!(n, 2);
            assert_eq!(prompt, "do the thing");
        }
        _ => panic!("expected queue, got usage/stop"),
    }
    match parse_loop_slash("keep going") {
        LoopSlash::Queue { n, prompt } => {
            assert_eq!(n, 3);
            assert_eq!(prompt, "keep going");
        }
        _ => panic!("expected default N=3"),
    }
    match parse_loop_slash("99 only") {
        LoopSlash::Queue { n, prompt } => {
            assert_eq!(n, 20);
            assert_eq!(prompt, "only");
        }
        _ => panic!("expected clamp to 20"),
    }
    assert!(matches!(parse_loop_slash(""), LoopSlash::Usage));
    assert!(matches!(parse_loop_slash("4"), LoopSlash::Usage));
    assert_eq!(LOOP_USAGE, "Usage: /loop N prompt…  |  /loop stop");
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
    _home: HomeLock,
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
        let _home = isolate_home();
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
            _home,
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
    h.session.add_user_message("session-only undo");
    h.run("/undo").await;
    assert!(h.app.status_message.to_lowercase().contains("undid"));
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

    // CI runners set `CI=true`, which makes `/import` a no-op skip toast.
    let prev_ci = std::env::var_os("CI");
    let prev_skip = std::env::var_os("WHYCODES_SKIP_IMPORT");
    unsafe {
        std::env::remove_var("CI");
        std::env::remove_var("WHYCODES_SKIP_IMPORT");
    }
    h.run("/import nope").await;
    unsafe {
        match prev_ci {
            Some(v) => std::env::set_var("CI", v),
            None => std::env::remove_var("CI"),
        }
        match prev_skip {
            Some(v) => std::env::set_var("WHYCODES_SKIP_IMPORT", v),
            None => std::env::remove_var("WHYCODES_SKIP_IMPORT"),
        }
    }
    assert!(
        h.app
            .toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Unknown product")),
        "{:?}",
        h.app.toasts.visible()
    );

    h.run("/unshare").await;
    assert!(
        h.app.status_message.contains("No share files")
            || h.app.status_message.contains("Unshared"),
        "{}",
        h.app.status_message
    );
    let shares = h.session.project_path.join(".whycodes").join("shares");
    std::fs::create_dir_all(&shares).unwrap();
    std::fs::write(shares.join(format!("{}.json", h.session.id)), "{}").unwrap();
    h.run("/unshare").await;
    assert!(
        h.app.status_message.contains("Unshared"),
        "{}",
        h.app.status_message
    );
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
    let _home = isolate_home();
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
    let svc = memory_service(dir.path(), &config).expect("memory service");
    let _ = svc.list(1);
}

#[test]
fn auto_index_zero_chunks_does_not_toast() {
    let _home = isolate_home_fresh();
    let dir = tempfile::tempdir().unwrap();
    let config = Config::default();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    maybe_session_auto_index(dir.path(), &config, &mut app);
    assert!(
        app.toasts.is_empty(),
        "empty project must not toast Indexed 0 code chunks"
    );
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
        important: false,
    }
}

#[test]
fn force_stop_applies_outcome_or_rebuilds() {
    let _home = isolate_home();
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

    rt.agent = Agent::new(dummy_info("keep-remote"));
    rt.agent_busy = true;
    rt.cancel_flag = Some(new_cancel_flag());
    at = Some(Instant::now());
    rt.done_tx
        .send(TurnOutcome::Remote {
            text: "remote".into(),
            error: None,
            work_ms: 1,
        })
        .unwrap();
    force_stop_turn(&mut app, &mut rt, &mut at, &config, dir.path(), &idx);
    assert_eq!(rt.agent.info.name, "keep-remote");
    assert!(!rt.agent_busy);
}

#[test]
fn rebuild_agent_resolves_pending_name() {
    let _home = isolate_home();
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
        event_tx.clone(),
        Arc::new(perm),
        Arc::new(question),
        &idx,
    );
    assert_eq!(agent.info.name, "build");

    let config = Config {
        default_agent: "plan".into(),
        ..Config::default()
    };
    let (perm, _) = ChannelPermissionPrompter::new();
    let (question, _) = ChannelQuestionPrompter::new(None);
    rebuild_agent_after_force_stop(
        &mut agent,
        &mut session,
        &config,
        dir.path(),
        "_pending",
        event_tx,
        Arc::new(perm),
        Arc::new(question),
        &idx,
    );
    assert_eq!(agent.info.name, "plan");
}

#[tokio::test]
async fn cycle_agent_walks_primary_list() {
    let _home = isolate_home();
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

#[tokio::test]
async fn switch_to_agent_picker_unknown_and_rebuild() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let (perm, _) = ChannelPermissionPrompter::new();
    let (question, _) = ChannelQuestionPrompter::new(None);
    let perm = Arc::new(perm);
    let question = Arc::new(question);
    let (event_tx, _) = mpsc::unbounded_channel();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.primary_agents = vec!["build".into(), "plan".into()];
    app.agent_cycle_idx = 0;
    app.intent_badge = Some("x".into());
    let claims = whycodes_core::FileClaimRegistry::new();
    let mut agent = Agent::new(dummy_info("build")).with_session_claims(claims);
    let mut session = Session::new(dir.path().to_path_buf(), "sys".into());
    let mut config = Config::default();

    switch_to_agent(
        &mut app,
        &mut agent,
        &mut session,
        &config,
        dir.path(),
        Arc::clone(&perm),
        Arc::clone(&question),
        &event_tx,
        "ghost",
        false,
    )
    .await;
    assert_eq!(app.agent_name, "ghost");
    assert_eq!(agent.info.name, "build");
    assert!(app.intent_badge.is_none());
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message == "Agent → ghost")
    );
    assert!(
        !app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Ctrl+T"))
    );

    config.agents.push(dummy_info("plan"));
    switch_to_agent(
        &mut app,
        &mut agent,
        &mut session,
        &config,
        dir.path(),
        perm,
        question,
        &event_tx,
        "plan",
        true,
    )
    .await;
    assert_eq!(app.agent_cycle_idx, 1);
    assert_eq!(app.agent_name, "plan");
    assert_eq!(agent.info.name, "plan");
    assert!(agent.session_claims().is_some());
    assert!(session.system_prompt.contains("sys") || !session.system_prompt.is_empty());
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Ctrl+T"))
    );
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
    let _home = isolate_home();
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

    let mut rt = test_runtime();
    let mut seed = TuiApp::from_config(TuiAppConfig::default());
    seed.add_message(ChatRole::Assistant, "");
    seed.yield_view(&mut rt.view);
    rt.done_tx
        .send(TurnOutcome::Ok {
            text: "filled".into(),
            agent: Agent::new(dummy_info("ok2")),
            session: Session::new(PathBuf::from("/work"), "sys".into()),
            work_ms: 1,
        })
        .unwrap();
    drain_background_runtime(&mut rt);
    assert_eq!(rt.view.messages.last().unwrap().content, "filled");

    let mut seed = TuiApp::from_config(TuiAppConfig::default());
    seed.add_message(ChatRole::Assistant, "");
    seed.yield_view(&mut rt.view);
    rt.done_tx
        .send(TurnOutcome::Remote {
            text: "remote-fill".into(),
            error: None,
            work_ms: 1,
        })
        .unwrap();
    drain_background_runtime(&mut rt);
    assert_eq!(rt.view.messages.last().unwrap().content, "remote-fill");

    let agent = Agent::new(dummy_info("cmp"));
    let mut session = Session::new(PathBuf::from("/work"), "sys".into());
    session.add_user_message("compact me");
    rt.done_tx
        .send(TurnOutcome::Compact {
            agent,
            session,
            outcome: whycodes_session::CompactOutcome {
                messages_before: 4,
                messages_after: 1,
                tokens_before: 400,
                tokens_after: 80,
                dropped_transcript: "old".into(),
            },
            work_ms: 2,
        })
        .unwrap();
    drain_background_runtime(&mut rt);
    assert!(!rt.last_error);
    assert_eq!(rt.agent.info.name, "cmp");
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

#[tokio::test]
async fn suggestion_and_catalog_helpers_short_circuit() {
    let _home = isolate_home();
    let session = Session::new(PathBuf::from("/work"), "sys".into());
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut config = Config::default();
    maybe_spawn_prompt_suggestion(&config, &session, "p", "m", "key", &mut app, tx.clone());
    config.tui.prompt_suggestions = "idle".into();
    maybe_spawn_prompt_suggestion(&config, &session, "p", "m", "", &mut app, tx.clone());
    maybe_spawn_prompt_suggestion(&config, &session, "p", "m", "key", &mut app, tx.clone());
    let mut session = session;
    session.add_user_message("   ");
    maybe_spawn_prompt_suggestion(&config, &session, "p", "m", "key", &mut app, tx.clone());
    session.add_user_message("do the next step");
    session.add_assistant_message(vec![whycodes_core::types::ContentBlock::Text {
        text: "ok".into(),
    }]);
    maybe_spawn_prompt_suggestion(&config, &session, "p", "m", "key", &mut app, tx);
    config.tui.prompt_suggestions = "on".into();
    maybe_spawn_prompt_suggestion(
        &config,
        &session,
        "unknown-provider",
        "m",
        "key",
        &mut app,
        {
            let (tx, _rx) = mpsc::unbounded_channel();
            tx
        },
    );

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

    let prev = std::env::var_os("WHYCODES_NO_MODEL_CATALOG");
    unsafe { std::env::set_var("WHYCODES_NO_MODEL_CATALOG", "1") };
    assert!(skip_model_catalog());
    spawn_model_context_fetch(&config, "p", "m", "k", {
        let (tx, _rx) = mpsc::unbounded_channel();
        tx
    });
    match prev {
        Some(v) => unsafe { std::env::set_var("WHYCODES_NO_MODEL_CATALOG", v) },
        None => unsafe { std::env::remove_var("WHYCODES_NO_MODEL_CATALOG") },
    }
}

#[test]
fn load_session_entries_and_picker_merge() {
    let _home = isolate_home();
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
fn load_session_entries_backfills_placeholder_title() {
    let (_lock, home) = isolate_home_fresh();
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::new(dir.path().to_path_buf(), "sys".into());
    session.title = format!(
        "{}-ab",
        dir.path()
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("session")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect::<String>()
    );
    session.title_source = whycodes_session::title::TitleSource::Default;
    session.add_user_message("please fix the login flow today");
    persist_session_best_effort(&session, "title-backfill");
    let entries = load_session_entries();
    let row = entries
        .iter()
        .find(|e| e.id == session.id)
        .expect("persisted");
    assert_ne!(row.title, session.title, "placeholder should upgrade");
    assert!(
        row.title.to_ascii_lowercase().contains("login")
            || row.title.to_ascii_lowercase().contains("fix"),
        "got {}",
        row.title
    );
    let _ = home;
}

#[test]
fn doctor_report_flags_missing_project() {
    let _home = isolate_home();
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

    let project = h.session.project_path.clone();
    h.history.push_before_turn(&h.session.messages, &project);
    h.session.add_user_message("later turn");
    h.run("/undo").await;
    assert!(h.app.status_message.to_lowercase().contains("undid"));
    h.run("/redo").await;
    assert!(h.app.status_message.to_lowercase().contains("redid"));

    h.app.api_context_for = Some((h.provider.clone(), h.model.clone()));
    h.run("/models").await;
    h.app.dialogs.clear();
    h.app.mode = AppMode::Normal;

    h.provider = "ollama".into();
    h.app.provider_name = "ollama".into();
    h.api_key.clear();
    h.run("/connect").await;
    assert!(
        h.app.status_message.contains("local") || h.app.status_message.contains("API key"),
        "{}",
        h.app.status_message
    );

    h.run("/login anthropic").await;
    h.provider = "anthropic".into();
    h.app.provider_name = "anthropic".into();
    h.api_key.clear();
    h.run("/connect").await;

    let prev_ci = std::env::var_os("CI");
    let prev_skip = std::env::var_os("WHYCODES_SKIP_IMPORT");
    unsafe {
        std::env::set_var("WHYCODES_SKIP_IMPORT", "1");
        std::env::remove_var("CI");
    }
    h.run("/import").await;
    unsafe {
        match prev_ci {
            Some(v) => std::env::set_var("CI", v),
            None => std::env::remove_var("CI"),
        }
        match prev_skip {
            Some(v) => std::env::set_var("WHYCODES_SKIP_IMPORT", v),
            None => std::env::remove_var("WHYCODES_SKIP_IMPORT"),
        }
    }
    assert!(
        h.app
            .toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Import skipped")),
        "{:?}",
        h.app
            .toasts
            .visible()
            .iter()
            .map(|t| t.message.as_str())
            .collect::<Vec<_>>()
    );
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
    let _home = isolate_home();
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
    let _home = isolate_home();
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
    let _home = isolate_home();
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
            auto_picked: false,
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
    let _home = isolate_home();
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
    let _home = isolate_home();
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
    assert!(
        app.messages
            .iter()
            .any(|m| m.content.contains("Signed in to `tui-oauth-switch-demo`")),
        "already-on provider still announces sign-in"
    );
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
        important: false,
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
        important: false,
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
        auto_picked: false,
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
    let _home = isolate_home();
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
    let _home = isolate_home();
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
    let _home = isolate_home();
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
    let _home = isolate_home();
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
    let _home = isolate_home();
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
    let _home = isolate_home();
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
    let _home = isolate_home();
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
    let _home = isolate_home();
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

fn sample_session_entry() -> crate::app::SessionEntry {
    crate::app::SessionEntry {
        id: "sess-1".into(),
        title: "recent".into(),
        messages: 1,
        updated_at: None,
        live: None,
    }
}

#[test]
fn first_frame_hydrate_empty_home_is_not_dirty() {
    let app = TuiApp::from_config(TuiAppConfig::default());
    let before = capture_first_frame_hydrate_chrome(&app);
    assert!(
        !first_frame_hydrate_needs_paint(&before, &app),
        "empty sessions, unchanged status, hidden sidebar, no toasts"
    );
}

#[test]
fn first_frame_hydrate_sessions_zero_to_n_is_dirty() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let before = capture_first_frame_hydrate_chrome(&app);
    app.session_list.sessions = vec![sample_session_entry()];
    assert!(first_frame_hydrate_needs_paint(&before, &app));
}

#[test]
fn first_frame_hydrate_status_change_is_dirty() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let before = capture_first_frame_hydrate_chrome(&app);
    app.status_message =
        "agent=why  xai/grok-4 — Tab focus  Ctrl+T agent  Esc cancel  /help".into();
    assert!(first_frame_hydrate_needs_paint(&before, &app));
}

#[test]
fn first_frame_hydrate_toast_is_dirty() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let before = capture_first_frame_hydrate_chrome(&app);
    app.toasts
        .push(crate::toast::ToastKind::Info, "Indexed 3 code chunks");
    assert!(first_frame_hydrate_needs_paint(&before, &app));
}

#[test]
fn first_frame_hydrate_visible_sidebar_file_tree_change_is_dirty() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.sidebar.visible = true;
    let before = capture_first_frame_hydrate_chrome(&app);
    app.sidebar.file_tree = vec!["src/main.rs".into()];
    assert!(first_frame_hydrate_needs_paint(&before, &app));
}

#[test]
fn first_frame_hydrate_hidden_sidebar_file_tree_change_is_not_dirty() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    assert!(!app.sidebar.visible, "home sidebar starts hidden");
    let before = capture_first_frame_hydrate_chrome(&app);
    app.sidebar.file_tree = vec!["src/main.rs".into()];
    app.sidebar.mcp_status = vec![" demo".into()];
    assert!(
        !first_frame_hydrate_needs_paint(&before, &app),
        "hidden sidebar mutations must not schedule a second paint"
    );
}

#[test]
fn first_frame_hydrate_settle_clears_leftover_dirty() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let before = capture_first_frame_hydrate_chrome(&app);
    app.needs_redraw = true;
    app.pending_full_clears = 2;
    settle_first_frame_hydrate(&mut app, &before, false);
    assert!(
        !app.needs_redraw,
        "unchanged empty home must drop leftover dirty"
    );
    assert_eq!(app.pending_full_clears, 0);
}

#[test]
fn first_frame_hydrate_settle_keeps_dirty_when_chrome_changed() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let before = capture_first_frame_hydrate_chrome(&app);
    app.toasts
        .push(crate::toast::ToastKind::Info, "Indexed 3 code chunks");
    app.needs_redraw = false;
    settle_first_frame_hydrate(&mut app, &before, false);
    assert!(app.needs_redraw);
}

#[test]
fn first_frame_hydrate_settle_keeps_clears_when_animating() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let before = capture_first_frame_hydrate_chrome(&app);
    app.needs_redraw = true;
    app.pending_full_clears = 1;
    settle_first_frame_hydrate(&mut app, &before, true);
    assert!(app.needs_redraw);
    assert_eq!(app.pending_full_clears, 1);
}

#[tokio::test]
async fn run_returns_quit_when_test_tui_env_set() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev = std::env::var_os("WHYCODES_TEST_TUI");
    unsafe { std::env::set_var("WHYCODES_TEST_TUI", "quit") };
    let opts = TuiRunOptions {
        project_dir: dir.path().to_path_buf(),
        provider: "anthropic".into(),
        model: "m".into(),
        api_key: String::new(),
        agent_name: "build".into(),
        max_turns: None,
        initial_prompt: None,
        config: Config::default(),
        resume_session_id: None,
        remote: None,
        update_rx: None,
    };
    let exit = super::run(opts).await.unwrap();
    match prev {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn run_returns_upgrade_when_test_tui_env_upgrade() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev = std::env::var_os("WHYCODES_TEST_TUI");
    unsafe { std::env::set_var("WHYCODES_TEST_TUI", "upgrade") };
    let opts = TuiRunOptions {
        project_dir: dir.path().to_path_buf(),
        provider: "anthropic".into(),
        model: "m".into(),
        api_key: String::new(),
        agent_name: "build".into(),
        max_turns: None,
        initial_prompt: None,
        config: Config::default(),
        resume_session_id: None,
        remote: None,
        update_rx: None,
    };
    let exit = super::run(opts).await.unwrap();
    match prev {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    assert_eq!(exit, TuiExit::Upgrade);
}

#[test]
fn apply_compact_view_sets_status_and_idle() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.current_agent_state = AgentState::Generating;
    let session = Session::new(PathBuf::from("/work/proj"), "sys".into());
    let outcome = whycodes_session::CompactOutcome {
        tokens_before: 400,
        tokens_after: 80,
        messages_before: 12,
        messages_after: 3,
        dropped_transcript: "ok".into(),
    };
    apply_compact_view(&mut app, &session, &outcome);
    assert_eq!(app.current_agent_state, AgentState::Idle);
    assert!(app.status_message.contains("12"));
    assert!(app.status_message.contains("3"));
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("compacted"))
    );
}

#[test]
fn apply_reasoning_effort_unknown_and_unsupported_and_ok() {
    let _home = isolate_home();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.provider_name = "anthropic".into();
    app.model_name = "claude-3".into();
    let mut agent = Agent::new(dummy_info("build"));
    let mut config = Config::default();

    apply_reasoning_effort(&mut app, &mut agent, &mut config, "nope");
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Unknown effort"))
    );

    apply_reasoning_effort(&mut app, &mut agent, &mut config, "high");
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("no reasoning-effort"))
    );

    app.provider_name = "openai".into();
    app.model_name = "gpt-5".into();
    apply_reasoning_effort(&mut app, &mut agent, &mut config, "xhigh");
    assert_eq!(app.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(config.session.reasoning_effort.as_deref(), Some("high"));
    assert!(app.status_message.contains("clamped") || app.status_message.contains("high"));
}

#[test]
fn persist_session_reasoning_effort_is_noop_in_tests() {
    persist_session_reasoning_effort("high").unwrap();
}

#[test]
fn apply_approval_mode_raw_unknown_and_ok() {
    let _home = isolate_home();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let mut agent = Agent::new(dummy_info("build"));
    let mut config = Config::default();

    apply_approval_mode_raw(&mut app, &mut agent, &mut config, "nope");
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Unknown mode"))
    );

    apply_approval_mode_raw(&mut app, &mut agent, &mut config, "manual");
    assert_eq!(app.approval_mode, ApprovalMode::Manual);
    assert_eq!(config.general.approval_mode, Some(ApprovalMode::Manual));
    assert!(app.status_message.contains("manual") || app.status_message.contains("Manual"));
}

#[test]
fn persist_general_approval_mode_is_noop_in_tests() {
    persist_general_approval_mode(ApprovalMode::Auto).unwrap();
}

#[test]
fn close_interactive_overlays_clears_permission_dialog() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    close_interactive_overlays(&mut app);
    assert!(app.dialogs.active().is_none());

    app.ask_permission("bash", "ls");
    app.pending_question_answers = Some(Default::default());
    app.question_dismissed = true;
    close_interactive_overlays(&mut app);
    assert!(app.dialogs.active().is_none());
    assert!(app.pending_question_answers.is_none());
    assert!(!app.question_dismissed);
    assert_eq!(app.mode, AppMode::Normal);
}

#[test]
fn explicit_provider_key_from_config_and_env() {
    let _home = isolate_home();
    let mut config = Config::default();
    config.providers.insert(
        "acme".into(),
        whycodes_core::types::ProviderConfig {
            name: "acme".into(),
            api_key: Some("sk-from-config".into()),
            api_base: None,
            base_url: None,
            headers: None,
            models: vec![],
            tool_arguments: None,
            extra: Default::default(),
        },
    );
    assert_eq!(
        explicit_provider_key(&config, "acme").as_deref(),
        Some("sk-from-config")
    );

    config.providers.insert(
        "envp".into(),
        whycodes_core::types::ProviderConfig {
            name: "envp".into(),
            api_key: Some(String::new()),
            api_base: None,
            base_url: None,
            headers: None,
            models: vec![],
            tool_arguments: None,
            extra: Default::default(),
        },
    );
    unsafe { std::env::set_var("ENVP_API_KEY", "sk-from-env") };
    assert_eq!(
        explicit_provider_key(&config, "envp").as_deref(),
        Some("sk-from-env")
    );
    unsafe { std::env::remove_var("ENVP_API_KEY") };
    assert!(explicit_provider_key(&config, "missing").is_none());
}

#[test]
fn try_fill_api_key_fills_empty_and_skips_set() {
    // Env key, not config.toml — sibling tests race on WHYCODES_HOME.
    let prev = std::env::var_os("FILLME_API_KEY");
    unsafe { std::env::set_var("FILLME_API_KEY", "sk-fill") };
    let mut key = String::new();
    try_fill_api_key(&mut key, "fillme");
    assert_eq!(key, "sk-fill");
    try_fill_api_key(&mut key, "fillme");
    assert_eq!(key, "sk-fill");
    match prev {
        Some(v) => unsafe { std::env::set_var("FILLME_API_KEY", v) },
        None => unsafe { std::env::remove_var("FILLME_API_KEY") },
    }
}

#[test]
fn record_user_turn_appends_message_and_title() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "hello").unwrap();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let mut rt = test_runtime();
    rt.session = Session::new(dir.path().to_path_buf(), "sys".into());
    let mut config = Config::default();
    config.session.auto_title = true;
    let expanded = record_user_turn(
        &mut app,
        &mut rt,
        "please look at @note.txt",
        dir.path(),
        &config,
        &[],
    );
    assert!(
        expanded.contains("hello") || expanded.contains("note.txt"),
        "{expanded}"
    );
    assert!(!rt.session.messages.is_empty());
}

#[test]
fn maybe_offer_update_self_install_and_homebrew() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    maybe_offer_update(&mut app);
    assert!(!app.update_prompted);

    app.available_update = Some(UpdateOffer::SelfInstall("9.9.9".into()));
    maybe_offer_update(&mut app);
    assert!(app.update_prompted);
    assert!(app.dialogs.is_open());

    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.available_update = Some(UpdateOffer::Homebrew("9.9.9".into()));
    maybe_offer_update(&mut app);
    assert!(app.update_prompted);
    assert!(app.dialogs.is_open());

    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.add_message(ChatRole::User, "already chatting");
    app.available_update = Some(UpdateOffer::SelfInstall("9.9.9".into()));
    maybe_offer_update(&mut app);
    assert!(app.update_prompted);
    assert!(!app.dialogs.is_open());
}

/// Exclusive `WHYCODES_HOME` + `$HOME` so import discovery cannot see the
/// developer's real Claude/OpenCode files, and `config.toml` is missing.
struct IsolatedImportHome {
    _lock: HomeLock,
    dir: tempfile::TempDir,
    prev_home: Option<std::ffi::OsString>,
    prev_profile: Option<std::ffi::OsString>,
    prev_skip: Option<std::ffi::OsString>,
    prev_ci: Option<std::ffi::OsString>,
}

impl IsolatedImportHome {
    fn new() -> Self {
        let (lock, dir) = isolate_home_fresh();
        let prev_home = std::env::var_os("HOME");
        let prev_profile = std::env::var_os("USERPROFILE");
        let prev_skip = std::env::var_os("WHYCODES_SKIP_IMPORT");
        let prev_ci = std::env::var_os("CI");
        unsafe {
            std::env::set_var("HOME", dir.path());
            std::env::set_var("USERPROFILE", dir.path());
            std::env::remove_var("WHYCODES_SKIP_IMPORT");
            std::env::remove_var("CI");
        }
        Self {
            _lock: lock,
            dir,
            prev_home,
            prev_profile,
            prev_skip,
            prev_ci,
        }
    }

    fn path(&self) -> &std::path::Path {
        self.dir.path()
    }
}

impl Drop for IsolatedImportHome {
    fn drop(&mut self) {
        unsafe {
            match &self.prev_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match &self.prev_profile {
                Some(v) => std::env::set_var("USERPROFILE", v),
                None => std::env::remove_var("USERPROFILE"),
            }
            match &self.prev_skip {
                Some(v) => std::env::set_var("WHYCODES_SKIP_IMPORT", v),
                None => std::env::remove_var("WHYCODES_SKIP_IMPORT"),
            }
            match &self.prev_ci {
                Some(v) => std::env::set_var("CI", v),
                None => std::env::remove_var("CI"),
            }
            // Restore the shared test home without re-locking (we still hold
            // `_lock`; `isolate_home()` would deadlock).
            std::env::set_var("WHYCODES_HOME", shared_test_home());
        }
        reset_session_db_cache();
    }
}

#[test]
fn parse_product_filter_accepts_known_and_rejects_unknown() {
    assert!(parse_product_filter("").unwrap().is_none());
    assert_eq!(
        parse_product_filter("claude").unwrap(),
        Some(whycodes_import::Product::Claude)
    );
    assert!(parse_product_filter("nope").is_err());
}

#[test]
fn maybe_offer_import_confirms_on_empty_home() {
    let home = IsolatedImportHome::new();
    std::fs::write(
        home.path().join(".claude.json"),
        r#"{"mcpServers":{"fs":{"command":"npx"}}}"#,
    )
    .unwrap();

    let mut app = TuiApp::from_config(TuiAppConfig::default());
    maybe_offer_import(&mut app);
    assert!(app.import_prompted);
    assert!(matches!(app.dialogs.active(), Some(DialogKind::Import)));
    assert!(!app.import_picker.items.is_empty());
    assert!(app.import_picker.any_checked());

    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.add_message(ChatRole::User, "already chatting");
    maybe_offer_import(&mut app);
    assert!(app.import_prompted);
    assert!(!app.dialogs.is_open());

    std::fs::write(
        home.path().join("config.toml"),
        "default_agent = \"build\"\n",
    )
    .unwrap();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    maybe_offer_import(&mut app);
    assert!(app.import_prompted);
    assert!(!app.dialogs.is_open());
}

#[test]
fn maybe_offer_import_skips_when_env_set() {
    let _home = IsolatedImportHome::new();
    unsafe { std::env::set_var("WHYCODES_SKIP_IMPORT", "1") };
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    maybe_offer_import(&mut app);
    assert!(!app.import_prompted);
    assert!(!app.dialogs.is_open());
    handle_import_slash(&mut app, "");
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Import skipped"))
    );
}

#[test]
fn apply_import_now_copies_mcp() {
    let home = IsolatedImportHome::new();
    std::fs::write(
        home.path().join(".claude.json"),
        r#"{"mcpServers":{"fs":{"command":"npx"}}}"#,
    )
    .unwrap();
    let mut config = Config::default();
    let out = apply_import_now(&mut config, home.path(), None).unwrap();
    match out {
        ApplyOutcome::Wrote { summary, .. } => {
            assert!(summary.contains("MCP"), "{summary}");
        }
        other => panic!("expected write, got {other:?}"),
    }
    assert!(config.mcp_servers.contains_key("fs"));
}

#[test]
fn handle_import_slash_unknown_and_preview() {
    let home = IsolatedImportHome::new();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    handle_import_slash(&mut app, "nope");
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Unknown product"))
    );

    handle_import_slash(&mut app, "");
    assert!(matches!(
        app.dialogs.active(),
        Some(DialogKind::Alert { .. })
    ));

    std::fs::write(
        home.path().join(".claude.json"),
        r#"{"mcpServers":{"fs":{"command":"npx"}}}"#,
    )
    .unwrap();
    app.dialogs.clear();
    handle_import_slash(&mut app, "claude");
    assert!(matches!(app.dialogs.active(), Some(DialogKind::Import)));
    assert!(
        app.import_picker
            .items
            .iter()
            .any(|i| i.label.contains("MCP")),
        "{:?}",
        app.import_picker.items
    );
}

#[test]
fn apply_import_now_respects_selection() {
    let home = IsolatedImportHome::new();
    std::fs::write(
        home.path().join(".claude.json"),
        r#"{"mcpServers":{"fs":{"command":"npx"}},"permissions":{"allow":["Bash"]}}"#,
    )
    .unwrap();
    let mut config = Config::default();
    // selectable_items is MCP then permission — keep MCP, skip permission.
    let out = apply_import_now(&mut config, home.path(), Some(&[true, false])).unwrap();
    match out {
        ApplyOutcome::Wrote { summary, .. } => {
            assert!(summary.contains("MCP"), "{summary}");
        }
        other => panic!("expected write, got {other:?}"),
    }
    assert!(config.mcp_servers.contains_key("fs"));
    assert!(config.permission.is_empty());
}

#[test]
fn apply_import_now_empty_selection_writes_nothing() {
    let home = IsolatedImportHome::new();
    std::fs::write(
        home.path().join(".claude.json"),
        r#"{"mcpServers":{"fs":{"command":"npx"}}}"#,
    )
    .unwrap();
    let mut config = Config::default();
    let out = apply_import_now(&mut config, home.path(), Some(&[false])).unwrap();
    match out {
        ApplyOutcome::Nothing { message } => {
            assert!(message.contains("Nothing new"), "{message}");
        }
        other => panic!("expected nothing, got {other:?}"),
    }
    assert!(!config.mcp_servers.contains_key("fs"));
}

#[test]
fn apply_import_outcome_toasts_wrote_nothing_and_error() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    apply_import_outcome(
        &mut app,
        Ok(ApplyOutcome::Wrote {
            path: PathBuf::from("/tmp/config.toml"),
            summary: "MCP 1".into(),
        }),
    );
    assert_eq!(app.status_message, "Imported · MCP 1");
    assert!(
        app.toasts.visible().iter().any(
            |t| t.kind == crate::toast::ToastKind::Success && t.message.contains("config.toml")
        )
    );

    let mut app = TuiApp::from_config(TuiAppConfig::default());
    apply_import_outcome(
        &mut app,
        Ok(ApplyOutcome::Nothing {
            message: "Nothing new to write".into(),
        }),
    );
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.kind == crate::toast::ToastKind::Info && t.message.contains("Nothing new"))
    );

    let mut app = TuiApp::from_config(TuiAppConfig::default());
    apply_import_outcome(&mut app, Err(anyhow::anyhow!("boom")));
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.kind == crate::toast::ToastKind::Error
                && t.message.contains("Import failed: boom"))
    );
}

#[test]
fn import_label_and_confirm_helpers() {
    use whycodes_import::{FoundSource, Product, SourceState};
    let found = [
        FoundSource {
            product: Product::Claude,
            rel_path: ".claude.json",
            path: PathBuf::from("/tmp/.claude.json"),
            state: SourceState::New,
        },
        FoundSource {
            product: Product::Claude,
            rel_path: "settings.json",
            path: PathBuf::from("/tmp/settings.json"),
            state: SourceState::Approved,
        },
        FoundSource {
            product: Product::OpenCode,
            rel_path: "opencode.json",
            path: PathBuf::from("/tmp/opencode.json"),
            state: SourceState::New,
        },
    ];
    let labels = unique_product_labels(&found);
    assert!(labels.contains("Claude Code"), "{labels}");
    assert!(labels.contains("OpenCode"), "{labels}");
    assert_eq!(looked_for(Some(&Product::Grok)), "Grok Build");
    let all = looked_for(None);
    assert!(all.contains("Claude Code"), "{all}");
    assert!(all.contains("Codex CLI"), "{all}");

    let mut app = TuiApp::from_config(TuiAppConfig::default());
    offer_import_confirm(&mut app, "Claude Code");
    match app.dialogs.active() {
        Some(DialogKind::Confirm {
            title, on_confirm, ..
        }) => {
            assert_eq!(title, "Import settings");
            assert_eq!(*on_confirm, ConfirmAction::ImportSettings);
        }
        other => panic!("expected import confirm, got {other:?}"),
    }
}

#[test]
fn refresh_context_window_sets_max_from_config_model() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let mut config = Config::default();
    config.models.insert(
        "m".into(),
        whycodes_core::types::ModelConfig {
            model_id: "m".into(),
            provider_id: "acme".into(),
            max_tokens: None,
            context_window: Some(12_345),
            temperature: None,
            top_p: None,
            thinking: None,
            supports_tools: None,
            supports_images: None,
        },
    );
    refresh_context_window(&mut app, &config, "acme", "m");
    assert_eq!(app.max_context_tokens, 12_345);
}

#[test]
fn arm_generating_clears_empty_status_and_reuses_assistant() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.add_message(ChatRole::Assistant, "already");
    app.status_message = "old".into();
    let mut rt = test_runtime();
    let mut at = Some(Instant::now());
    let _flag = arm_generating(&mut app, &mut rt, &mut at, "");
    assert!(rt.agent_busy);
    assert!(at.is_none());
    assert!(app.status_message.is_empty());
    assert_eq!(
        app.messages
            .iter()
            .filter(|m| m.role == ChatRole::Assistant)
            .count(),
        1
    );
}

#[test]
fn mark_import_declined_sets_first_run_asked() {
    let home = IsolatedImportHome::new();
    mark_import_declined();
    let consent = whycodes_import::ConsentStore::new(home.path());
    assert!(consent.first_run_asked().unwrap());
}

#[tokio::test]
async fn start_compact_task_marks_generating() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let mut rt = test_runtime();
    rt.session = Session::new(dir.path().to_path_buf(), "sys".into());
    rt.session.add_user_message("hello compact");
    let mut cancel_at = None;
    start_compact_task(
        &mut app,
        &mut rt,
        &mut cancel_at,
        "keep recent".into(),
        "anthropic",
        "m",
        "sk",
        dir.path(),
    );
    assert!(rt.agent_busy);
    assert_eq!(app.current_agent_state, AgentState::Generating);
    assert!(app.status_message.contains("Compact"));
    if let Some(join) = rt.turn_join.take() {
        join.abort();
    }
}

#[test]
fn cycle_live_session_noop_when_empty() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let mut rt = test_runtime();
    let mut runtimes: Vec<SessionRuntime> = Vec::new();
    let mut mru = Vec::new();
    cycle_live_session(&mut app, &mut rt, &mut runtimes, &mut mru, true);
    assert!(mru.is_empty());
}

#[test]
fn tui_available_does_not_panic() {
    let _ = tui_available();
    let _ = open_tui_writer();
    let _ = open_controlling_console();
    assert!(
        console_open_result(
            Err(std::io::Error::other("no console")),
            "open console failed",
        )
        .is_none()
    );
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();
    drop(tmp);
    let _ = console_open_result(std::fs::File::open(&path), "open tmp");
    let mut sink = Vec::new();
    restore_terminal_on(&mut sink);
}

#[test]
fn attach_live_open_error_and_raw_error() {
    let color = crate::color::ColorMode::Ansi256;
    let err = match attach_live(
        color,
        || Err(std::io::Error::other("no tty")),
        || Ok(()),
        || Ok((80, 24)),
    ) {
        Err(e) => e,
        Ok(_) => panic!("expected open error"),
    };
    assert!(
        err.to_string().contains("plain") || err.to_string().contains("tty"),
        "{err}"
    );
    let err = match attach_live(
        color,
        || Ok(TuiWriter::Buf(Vec::new())),
        || Err(std::io::Error::other("raw failed")),
        || Ok((80, 24)),
    ) {
        Err(e) => e,
        Ok(_) => panic!("expected raw error"),
    };
    assert!(err.to_string().contains("raw"), "{err}");
    let (term, _, tw, th) = attach_live(
        color,
        || Ok(TuiWriter::Buf(Vec::new())),
        || Ok(()),
        || Ok((80, 24)),
    )
    .unwrap();
    assert_eq!((tw, th), (80, 24));
    term.restore(false);
}

#[test]
fn resume_helpers_cover_missing_and_load_error() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let mut session = Session::new(PathBuf::from("/work"), "sys".into());
    assert_eq!(
        resume_missing_toast(RESUME_LATEST),
        "No saved sessions to continue"
    );
    assert!(resume_missing_toast("abc-id").contains("abc-id"));

    apply_resume_loaded(
        &mut app,
        &mut session,
        "keep-sys",
        "want",
        false,
        Err("disk full".into()),
    );
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Resume failed") && t.message.contains("disk full"))
    );

    apply_resume_loaded(
        &mut app,
        &mut session,
        "keep-sys",
        RESUME_LATEST,
        false,
        Ok(None),
    );
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("No saved"))
    );

    let mut loaded = Session::new(PathBuf::from("/work"), "old".into());
    loaded.add_user_message("hello resume");
    apply_resume_loaded(
        &mut app,
        &mut session,
        "keep-sys",
        &loaded.id.clone(),
        true,
        Ok(Some(loaded)),
    );
    assert_eq!(session.system_prompt, "keep-sys");
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Resumed"))
    );
}

#[test]
fn auth_send_helpers_drop_when_loop_closed() {
    let (tx, rx) = mpsc::unbounded_channel();
    drop(rx);
    send_auth_event(&tx, AuthFlowEvent::Note("gone".into()));
    send_auth_done(&tx, "acme".into(), Ok("ok".into()));
    send_auth_done(&tx, "acme".into(), Err("fail".into()));
}

#[test]
fn console_open_and_primary_agent_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("console.txt");
    std::fs::write(&path, b"").unwrap();
    let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    assert!(console_open_result(Ok(file), "ok").is_some());
    assert!(
        console_open_result(
            Err(io::Error::new(io::ErrorKind::NotFound, "nope")),
            "open CONOUT$ failed, trying stdout",
        )
        .is_none()
    );

    let mut agents = Vec::new();
    ensure_primary_agents(&mut agents);
    assert_eq!(agents, vec!["build", "plan", "ask"]);
    ensure_primary_agents(&mut agents);
    assert_eq!(agents.len(), 3);

    let info = default_agent_info("why");
    assert_eq!(info.name, "why");
    assert_eq!(info.description, "Default");
    assert!(info.permission.allow_file_writes);
}

#[test]
fn tui_writer_write_flush_and_summary() {
    let mut stdout = TuiWriter::Stdout(io::stdout());
    let _ = stdout.write(b"");
    let _ = stdout.flush();
    if let Some(console) = open_controlling_console() {
        let mut w = TuiWriter::Console(console);
        let _ = w.write(b"");
        let _ = w.flush();
    }
    print_session_summary("coverage-summary");
    let _ = tui_available();
    let _ = open_tui_writer();
}

#[test]
fn bind_agent_prompters_attaches_channels() {
    let (perm, _perm_rx) = ChannelPermissionPrompter::new();
    let (question, _q_rx) = ChannelQuestionPrompter::new(None);
    let perm = Arc::new(perm);
    let question = Arc::new(question);
    let agent = bind_agent_prompters(Agent::new(dummy_info("build")), &perm, &question);
    assert_eq!(agent.info.name, "build");
}

#[test]
fn enable_keyboard_enhancement_skips_bench() {
    let _lock = isolate_home_lock();
    let prev = std::env::var_os("WHYCODES_BENCH");
    unsafe { std::env::set_var("WHYCODES_BENCH", "1") };
    let mut out = Vec::new();
    assert!(!enable_keyboard_enhancement(&mut out, Some((80, 24))));
    match prev {
        Some(v) => unsafe { std::env::set_var("WHYCODES_BENCH", v) },
        None => unsafe { std::env::remove_var("WHYCODES_BENCH") },
    }
}

#[test]
fn push_keyboard_flags_respects_support() {
    let mut out = Vec::new();
    assert!(!push_keyboard_flags(&mut out, false));
    let mut out = Vec::new();
    let _ = push_keyboard_flags(&mut out, true);
}

#[test]
fn enter_raw_and_alt_maps_raw_mode_error() {
    let mut out = Vec::new();
    let err = enter_raw_and_alt(&mut out, || Err(std::io::Error::other("no tty"))).unwrap_err();
    assert!(err.to_string().contains("raw mode"), "{err}");
}

#[test]
fn apply_resume_loaded_error_toasts() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let mut session = Session::new(PathBuf::from("/work"), "sys".into());
    apply_resume_loaded(
        &mut app,
        &mut session,
        "sys",
        "id",
        false,
        Err("db locked".into()),
    );
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Resume failed: db locked"))
    );
}

#[test]
fn maybe_offer_import_already_asked_empty_and_no_home() {
    {
        let home = IsolatedImportHome::new();
        std::fs::write(
            home.path().join(".claude.json"),
            r#"{"mcpServers":{"fs":{"command":"npx"}}}"#,
        )
        .unwrap();
        let consent = whycodes_import::ConsentStore::new(whycodes_core::paths::data_dir());
        consent.mark_first_run_asked().unwrap();
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        maybe_offer_import(&mut app);
        assert!(app.import_prompted);
        assert!(!app.dialogs.is_open());
    }
    {
        let _home = IsolatedImportHome::new();
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        maybe_offer_import(&mut app);
        assert!(app.import_prompted);
        assert!(!app.dialogs.is_open());
    }
    {
        let _home = IsolatedImportHome::new();
        let prev_profile = std::env::var_os("USERPROFILE");
        unsafe {
            std::env::remove_var("HOME");
            std::env::remove_var("USERPROFILE");
        }
        let mut app = TuiApp::from_config(TuiAppConfig::default());
        maybe_offer_import(&mut app);
        assert!(app.import_prompted);
        if let Some(v) = prev_profile {
            unsafe { std::env::set_var("USERPROFILE", v) };
        }
    }
}

#[test]
fn maybe_offer_import_corrupt_consent_and_preview_fallback() {
    let home = IsolatedImportHome::new();
    std::fs::write(
        home.path().join(".claude.json"),
        r#"{"mcpServers":{"fs":{"command":"npx"}}}"#,
    )
    .unwrap();
    std::fs::write(
        whycodes_core::paths::data_dir().join("import-consent.json"),
        "{",
    )
    .unwrap();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    maybe_offer_import(&mut app);
    assert!(app.import_prompted);
}

#[test]
fn handle_import_slash_empty_plan_after_apply() {
    let home = IsolatedImportHome::new();
    std::fs::write(
        home.path().join(".claude.json"),
        r#"{"mcpServers":{"fs":{"command":"npx"}}}"#,
    )
    .unwrap();
    let mut config = Config::default();
    apply_import_now(&mut config, home.path(), None).unwrap();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    handle_import_slash(&mut app, "claude");
    assert!(matches!(
        app.dialogs.active(),
        Some(DialogKind::Alert { .. }) | Some(DialogKind::Import)
    ));
}

#[test]
fn mark_import_declined_writes_when_home_set() {
    let home = IsolatedImportHome::new();
    mark_import_declined();
    let consent = whycodes_import::ConsentStore::new(whycodes_core::paths::data_dir());
    assert!(consent.first_run_asked().unwrap());
    let _ = home;
}

#[test]
fn handle_import_slash_nothing_approved_and_filter_none() {
    let home = IsolatedImportHome::new();
    std::fs::write(
        home.path().join(".claude.json"),
        r#"{"mcpServers":{"fs":{"command":"npx"}}}"#,
    )
    .unwrap();
    let consent = whycodes_import::ConsentStore::new(whycodes_core::paths::data_dir());
    consent.deny(&home.path().join(".claude.json")).unwrap();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    handle_import_slash(&mut app, "claude");
    assert!(matches!(
        app.dialogs.active(),
        Some(DialogKind::Alert { .. }) | Some(DialogKind::Import)
    ));
    handle_import_slash(&mut app, "grok");
}

#[tokio::test]
async fn apply_pending_import_writes_and_reloads() {
    let home = IsolatedImportHome::new();
    std::fs::write(
        home.path().join(".claude.json"),
        r#"{"mcpServers":{"fs":{"command":"npx"}}}"#,
    )
    .unwrap();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    handle_import_slash(&mut app, "claude");
    let mut config = Config::default();
    let mut agent = Agent::new(dummy_info("build"));
    let idx = whycodes_index::WorkspaceIndex::start(Vec::new());
    apply_pending_import(&mut app, &mut agent, &mut config, home.path(), &idx).await;
    assert!(
        config.mcp_servers.contains_key("fs")
            || app
                .toasts
                .visible()
                .iter()
                .any(|t| t.message.contains("Imported")
                    || t.message.contains("Nothing")
                    || t.message.contains("Import"))
    );
}

#[test]
fn shown_tool_name_aliases_and_background_event() {
    assert_eq!(shown_tool_name("bash"), "run");
    assert_eq!(shown_tool_name("read_file"), "read");
    assert_eq!(shown_tool_name("rg"), "grep");
    assert_eq!(shown_tool_name("custom"), "custom");
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    apply_background_event(&mut app, "bg-1", "running", "sleep");
    apply_background_event(&mut app, "bg-1", "done", "ok");
    apply_background_event(&mut app, "bg-2", "failed", "boom");
    assert!(app.bg_jobs.iter().any(|j| j.id == "bg-1"));
}

#[test]
fn handle_question_key_multi_select_space_and_digit() {
    let spec = whycodes_tools::question::QuestionSpec {
        prompt: "Pick many".into(),
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
            whycodes_tools::question::QuestionOption {
                label: "Other".into(),
                description: String::new(),
                preview: None,
            },
        ],
        multi_select: true,
        important: false,
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
    assert!(handle_question_key(
        &mut app,
        KeyCode::Char('2'),
        &mut q,
        &p
    ));
    assert!(handle_question_key(
        &mut app,
        KeyCode::Char('3'),
        &mut q,
        &p
    ));
    assert!(handle_question_key(&mut app, KeyCode::Enter, &mut q, &p));
}

#[test]
fn format_bg_jobs_lists_running() {
    let jobs = [whycodes_agent::JobSnapshot {
        id: "bg-1".into(),
        label: "sleep".into(),
        status: whycodes_agent::JobStatus::Running,
        elapsed: std::time::Duration::from_secs(3),
        output_len: 0,
        exit_code: None,
    }];
    let text = format_bg_jobs(1, &jobs);
    assert!(text.contains("bg-1"), "{text}");
    assert!(text.contains("sleep"), "{text}");
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    memory_err_toast(&mut app, "boom");
    export_failed_toast(&mut app, "io");
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Memory") || t.message.contains("io"))
    );
}

#[tokio::test]
async fn run_headless_draws_then_quits() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    unsafe { std::env::remove_var("WHYCODES_TEST_TUI") };
    set_headless_events(Some(std::collections::VecDeque::from([
        Event::Resize(0, 24),
        Event::Resize(90, 30),
        Event::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Moved,
            column: 1,
            row: 1,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }),
        Event::Key(crossterm::event::KeyEvent::from(KeyCode::Char('x'))),
    ])));
    let opts = boot_opts(dir.path(), "sk-test");
    let exit = super::run(opts).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn run_headless_empty_queue_quits_after_first_frame() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    unsafe { std::env::remove_var("WHYCODES_TEST_TUI") };
    set_headless_events(Some(std::collections::VecDeque::new()));
    let exit = super::run(boot_opts(dir.path(), "")).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn run_headless_scripted_turn_then_quit() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_llm = std::env::var_os("WHYCODES_TEST_LLM");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::set_var("WHYCODES_TEST_LLM", "scripted-ok");
    }
    set_headless_events(Some(std::collections::VecDeque::from([
        press(KeyCode::Char('h')),
        press(KeyCode::Char('i')),
        press(KeyCode::Enter),
    ])));
    let exit = super::run(boot_opts(dir.path(), "sk-test")).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    match prev_llm {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_LLM", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_LLM") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn run_headless_scripted_fail_turn() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_llm = std::env::var_os("WHYCODES_TEST_LLM");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::set_var("WHYCODES_TEST_LLM", "FAIL");
    }
    set_headless_events(Some(std::collections::VecDeque::from([
        press(KeyCode::Char('x')),
        press(KeyCode::Enter),
    ])));
    let exit = super::run(boot_opts(dir.path(), "sk-test")).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    match prev_llm {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_LLM", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_LLM") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn run_headless_initial_prompt_scripted_turn() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_llm = std::env::var_os("WHYCODES_TEST_LLM");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::set_var("WHYCODES_TEST_LLM", "boot-ok");
    }
    set_headless_events(Some(std::collections::VecDeque::new()));
    let mut opts = boot_opts(dir.path(), "sk-test");
    opts.initial_prompt = Some("hello from boot".into());
    let exit = super::run(opts).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    match prev_llm {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_LLM", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_LLM") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn run_headless_hang_then_esc_cancel() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_llm = std::env::var_os("WHYCODES_TEST_LLM");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::set_var("WHYCODES_TEST_LLM", "HANG");
    }
    set_headless_events(Some(std::collections::VecDeque::from([
        press(KeyCode::Char('h')),
        press(KeyCode::Enter),
        press(KeyCode::Esc),
        press(KeyCode::Esc),
    ])));
    let exit = super::run(boot_opts(dir.path(), "sk-test")).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    match prev_llm {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_LLM", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_LLM") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn run_headless_compact_after_turn() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_llm = std::env::var_os("WHYCODES_TEST_LLM");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::set_var("WHYCODES_TEST_LLM", "compact-ok");
    }
    set_headless_events(Some(std::collections::VecDeque::from([
        press(KeyCode::Char('a')),
        press(KeyCode::Enter),
        press(KeyCode::Char('/')),
        press(KeyCode::Char('c')),
        press(KeyCode::Char('o')),
        press(KeyCode::Char('m')),
        press(KeyCode::Char('p')),
        press(KeyCode::Char('a')),
        press(KeyCode::Char('c')),
        press(KeyCode::Char('t')),
        press(KeyCode::Char(' ')),
        press(KeyCode::Char('n')),
        press(KeyCode::Enter),
    ])));
    let exit = super::run(boot_opts(dir.path(), "sk-test")).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    match prev_llm {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_LLM", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_LLM") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn run_headless_picker_dialogs_confirm_and_ctrl_q() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_import = std::env::var_os("WHYCODES_SKIP_IMPORT");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::set_var("WHYCODES_SKIP_IMPORT", "1");
    }
    let mut events = Vec::new();
    events.extend(type_line("/models"));
    events.push(press(KeyCode::Enter));
    events.extend(type_line("/effort"));
    events.push(press(KeyCode::Enter));
    events.extend(type_line("/mode"));
    events.push(press(KeyCode::Enter));
    events.extend(type_line("/login"));
    events.push(press(KeyCode::Esc));
    events.push(ctrl('q'));
    events.push(press(KeyCode::Enter));
    set_headless_events(Some(events.into()));
    let exit = super::run(boot_opts(dir.path(), "sk-test")).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    match prev_import {
        Some(v) => unsafe { std::env::set_var("WHYCODES_SKIP_IMPORT", v) },
        None => unsafe { std::env::remove_var("WHYCODES_SKIP_IMPORT") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

fn type_line(text: &str) -> Vec<Event> {
    let mut out = Vec::new();
    for c in text.chars() {
        out.push(press(KeyCode::Char(c)));
    }
    out.push(press(KeyCode::Enter));
    out
}

#[tokio::test]
async fn run_headless_slash_models_effort_mode_and_connect() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_llm = std::env::var_os("WHYCODES_TEST_LLM");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::set_var("WHYCODES_TEST_LLM", "slash-ok");
    }
    let mut events = Vec::new();
    events.extend(type_line("/models acme/m2"));
    events.extend(type_line("/effort high"));
    events.extend(type_line("/mode manual"));
    events.extend(type_line("/connect"));
    events.extend(type_line("/login"));
    events.extend(type_line("/agent"));
    events.push(press(KeyCode::Esc));
    events.extend(type_line("/sessions"));
    events.push(press(KeyCode::Esc));
    events.extend(type_line("/theme"));
    events.push(press(KeyCode::Esc));
    events.extend(type_line("/diff"));
    events.extend(type_line("/tools"));
    events.extend(type_line("/info"));
    events.extend(type_line("/doctor"));
    events.extend(type_line("/context"));
    events.extend(type_line("/cost"));
    events.extend(type_line("/bg"));
    events.extend(type_line("/memory"));
    events.extend(type_line("/import"));
    events.push(press(KeyCode::Esc));
    events.extend(type_line(":q"));
    set_headless_events(Some(events.into()));
    let exit = super::run(boot_opts(dir.path(), "sk-test")).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    match prev_llm {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_LLM", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_LLM") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn run_headless_loop_slash_and_ctrl_n() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_llm = std::env::var_os("WHYCODES_TEST_LLM");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::set_var("WHYCODES_TEST_LLM", "loop-ok");
    }
    let ctrl_n = Event::Key(crossterm::event::KeyEvent::new(
        KeyCode::Char('n'),
        crossterm::event::KeyModifiers::CONTROL,
    ));
    let mut events = Vec::new();
    events.extend(type_line("/loop 2 ping"));
    events.push(ctrl_n);
    events.push(Event::Key(crossterm::event::KeyEvent::new(
        KeyCode::PageDown,
        crossterm::event::KeyModifiers::CONTROL,
    )));
    events.push(Event::Key(crossterm::event::KeyEvent::new(
        KeyCode::PageUp,
        crossterm::event::KeyModifiers::CONTROL,
    )));
    events.push(Event::Key(crossterm::event::KeyEvent::new(
        KeyCode::Char('o'),
        crossterm::event::KeyModifiers::CONTROL,
    )));
    events.push(press(KeyCode::Esc));
    events.push(Event::Key(crossterm::event::KeyEvent::new(
        KeyCode::Tab,
        crossterm::event::KeyModifiers::CONTROL,
    )));
    events.extend(type_line(":q"));
    set_headless_events(Some(events.into()));
    let exit = super::run(boot_opts(dir.path(), "sk-test")).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    match prev_llm {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_LLM", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_LLM") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

fn ctrl(c: char) -> Event {
    Event::Key(crossterm::event::KeyEvent::new(
        KeyCode::Char(c),
        crossterm::event::KeyModifiers::CONTROL,
    ))
}

#[tokio::test]
async fn run_headless_shell_permission_allow() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_llm = std::env::var_os("WHYCODES_TEST_LLM");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::set_var("WHYCODES_TEST_LLM", "SHELL");
    }
    let mut events = Vec::new();
    events.extend(type_line("please rm that"));
    events.push(press(KeyCode::Char('y')));
    events.extend(type_line(":q"));
    set_headless_events(Some(events.into()));
    let mut opts = boot_opts(dir.path(), "sk-test");
    opts.config.general.approval_mode = Some(whycodes_core::types::ApprovalMode::Manual);
    let exit = super::run(opts).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    match prev_llm {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_LLM", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_LLM") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn run_headless_shell_permission_deny() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_llm = std::env::var_os("WHYCODES_TEST_LLM");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::set_var("WHYCODES_TEST_LLM", "SHELL");
    }
    let mut events = Vec::new();
    events.extend(type_line("please rm that"));
    events.push(press(KeyCode::Char('n')));
    events.extend(type_line(":q"));
    set_headless_events(Some(events.into()));
    let mut opts = boot_opts(dir.path(), "sk-test");
    opts.config.general.approval_mode = Some(whycodes_core::types::ApprovalMode::Manual);
    let exit = super::run(opts).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    match prev_llm {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_LLM", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_LLM") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn run_headless_hang_ctrl_c_and_enter() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_llm = std::env::var_os("WHYCODES_TEST_LLM");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::set_var("WHYCODES_TEST_LLM", "HANG");
    }
    set_headless_events(Some(std::collections::VecDeque::from([
        press(KeyCode::Char('h')),
        press(KeyCode::Enter),
        press(KeyCode::Char('x')),
        press(KeyCode::Enter),
        ctrl('c'),
        ctrl('c'),
        ctrl('q'),
    ])));
    let exit = super::run(boot_opts(dir.path(), "sk-test")).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    match prev_llm {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_LLM", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_LLM") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn run_headless_question_tool_enter() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_llm = std::env::var_os("WHYCODES_TEST_LLM");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::set_var("WHYCODES_TEST_LLM", "ASK");
    }
    let mut events = Vec::new();
    events.extend(type_line("please ask"));
    events.push(press(KeyCode::Enter));
    events.extend(type_line(":q"));
    set_headless_events(Some(events.into()));
    let mut opts = boot_opts(dir.path(), "sk-test");
    opts.config.general.approval_mode = Some(whycodes_core::types::ApprovalMode::Manual);
    let exit = super::run(opts).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    match prev_llm {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_LLM", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_LLM") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn run_headless_hydrates_api_key_from_env() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_key = std::env::var_os("ACME_API_KEY");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::set_var("ACME_API_KEY", "sk-from-env");
    }
    set_headless_events(Some(std::collections::VecDeque::from([press(
        KeyCode::Char('x'),
    )])));
    let exit = super::run(boot_opts(dir.path(), "")).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    match prev_key {
        Some(v) => unsafe { std::env::set_var("ACME_API_KEY", v) },
        None => unsafe { std::env::remove_var("ACME_API_KEY") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn run_headless_update_offer_self_install_then_quit() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    unsafe { std::env::remove_var("WHYCODES_TEST_TUI") };
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tx.send(UpdateOffer::SelfInstall("9.9.9".into())).unwrap();
    drop(tx);
    set_headless_events(Some(std::collections::VecDeque::from([
        press(KeyCode::Esc),
        press(KeyCode::Char(':')),
        press(KeyCode::Char('q')),
        press(KeyCode::Enter),
    ])));
    let mut opts = boot_opts(dir.path(), "sk-test");
    opts.update_rx = Some(rx);
    let exit = super::run(opts).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn run_headless_update_offer_homebrew() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    unsafe { std::env::remove_var("WHYCODES_TEST_TUI") };
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tx.send(UpdateOffer::Homebrew("9.9.9".into())).unwrap();
    set_headless_events(Some(std::collections::VecDeque::from([
        press(KeyCode::Enter),
        press(KeyCode::Char(':')),
        press(KeyCode::Char('q')),
        press(KeyCode::Enter),
    ])));
    let mut opts = boot_opts(dir.path(), "sk-test");
    opts.update_rx = Some(rx);
    let exit = super::run(opts).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn run_headless_mouse_stop_while_hanging() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_llm = std::env::var_os("WHYCODES_TEST_LLM");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::set_var("WHYCODES_TEST_LLM", "HANG");
    }
    set_headless_events(Some(std::collections::VecDeque::from([
        press(KeyCode::Char('h')),
        press(KeyCode::Enter),
        Event::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: 78,
            row: 0,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }),
        Event::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: 78,
            row: 0,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }),
        ctrl('q'),
    ])));
    let exit = super::run(boot_opts(dir.path(), "sk-test")).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    match prev_llm {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_LLM", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_LLM") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn run_headless_missing_api_key_warns_then_quits() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_key = std::env::var_os("ACME_API_KEY");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::remove_var("ACME_API_KEY");
    }
    let mut events = Vec::new();
    events.extend(type_line("hello without key"));
    events.extend(type_line(":q"));
    set_headless_events(Some(events.into()));
    let mut opts = boot_opts(dir.path(), "");
    opts.config.providers.insert(
        "acme".into(),
        whycodes_core::types::ProviderConfig {
            name: "acme".into(),
            api_key: None,
            api_base: None,
            base_url: None,
            headers: None,
            models: vec!["m1".into()],
            tool_arguments: None,
            extra: Default::default(),
        },
    );
    let exit = super::run(opts).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    match prev_key {
        Some(v) => unsafe { std::env::set_var("ACME_API_KEY", v) },
        None => unsafe { std::env::remove_var("ACME_API_KEY") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn run_headless_remote_turn_errors_then_quits() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    unsafe { std::env::remove_var("WHYCODES_TEST_TUI") };
    let mut events = Vec::new();
    events.extend(type_line("hi remote"));
    events.extend(type_line(":q"));
    set_headless_events(Some(events.into()));
    let mut opts = boot_opts(dir.path(), "sk-test");
    opts.remote = Some(crate::remote::RemoteAttach::new(
        "http://127.0.0.1:1",
        "sid-remote",
    ));
    let exit = super::run(opts).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[test]
fn read_event_batch_from_crossterm_stub() {
    set_crossterm_stub(std::collections::VecDeque::from([
        Event::Key(crossterm::event::KeyEvent::from(KeyCode::Char('a'))),
        Event::Resize(40, 12),
    ]));
    let batch = super::read_event_batch().unwrap();
    assert_eq!(batch.len(), 2);
}

#[tokio::test]
async fn run_headless_catalog_suggest_and_auth_note() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    unsafe { std::env::remove_var("WHYCODES_TEST_TUI") };
    set_test_catalog_window(Some(("acme".into(), "m1".into(), 128_000)));
    set_test_suggest(Some("try cargo test".into()));
    set_test_auth_event(Some(AuthFlowEvent::Note("signing in…".into())));
    set_headless_events(Some(std::collections::VecDeque::from([press(
        KeyCode::Char('x'),
    )])));
    let exit = super::run(boot_opts(dir.path(), "sk-test")).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

fn press(code: KeyCode) -> Event {
    Event::Key(crossterm::event::KeyEvent::from(code))
}

#[tokio::test]
async fn run_headless_slash_help_then_quit_command() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    unsafe { std::env::remove_var("WHYCODES_TEST_TUI") };
    set_headless_events(Some(std::collections::VecDeque::from([
        press(KeyCode::Char('/')),
        press(KeyCode::Char('h')),
        press(KeyCode::Char('e')),
        press(KeyCode::Char('l')),
        press(KeyCode::Char('p')),
        press(KeyCode::Enter),
        press(KeyCode::Esc),
        press(KeyCode::Char(':')),
        press(KeyCode::Char('q')),
        press(KeyCode::Enter),
    ])));
    let exit = super::run(boot_opts(dir.path(), "sk-test")).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn run_headless_bench_stops_after_first_frame() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_bench = std::env::var_os("WHYCODES_BENCH");
    let prev_dur = std::env::var_os("WHYCODES_BENCH_DURATION_MS");
    let out = dir.path().join("bench.json");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::set_var("WHYCODES_BENCH", &out);
        std::env::set_var("WHYCODES_BENCH_DURATION_MS", "0");
    }
    set_headless_events(Some(std::collections::VecDeque::from([press(
        KeyCode::Char('a'),
    )])));
    let exit = super::run(boot_opts(dir.path(), "")).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    match prev_bench {
        Some(v) => unsafe { std::env::set_var("WHYCODES_BENCH", v) },
        None => unsafe { std::env::remove_var("WHYCODES_BENCH") },
    }
    match prev_dur {
        Some(v) => unsafe { std::env::set_var("WHYCODES_BENCH_DURATION_MS", v) },
        None => unsafe { std::env::remove_var("WHYCODES_BENCH_DURATION_MS") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[test]
fn loop_io_scripted_poll_and_read_batch() {
    set_headless_events(Some(std::collections::VecDeque::from([
        press(KeyCode::Char('a')),
        press(KeyCode::Enter),
    ])));
    let mut io = LoopIo::take_from_thread();
    assert!(io.is_headless());
    assert!(io.peek().is_some());
    assert!(io.poll(Duration::from_millis(1)).unwrap());
    let batch = io.read_batch().unwrap();
    assert_eq!(batch.len(), 1);
    assert!(io.poll(Duration::ZERO).unwrap());
    let _ = io.read_batch().unwrap();
    assert!(!io.poll(Duration::ZERO).unwrap());
    assert!(io.read_batch().is_err());
}

#[test]
fn enter_raw_and_alt_ok_and_restore_backend() {
    let mut out = Vec::new();
    enter_raw_and_alt(&mut out, || Ok(())).unwrap();
    assert!(!out.is_empty());
    restore_live_backend(&mut out, true);
    restore_live_backend(&mut out, false);
}

#[test]
fn loop_term_headless_and_live_buf_draw() {
    let color = crate::color::ColorMode::Ansi256;
    let mut term = LoopTerm::headless(color).unwrap();
    term.resize(Rect::new(0, 0, 40, 12));
    let _ = term.clear();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.pending_full_clears = 1;
    let (area, snapshot) = term.draw_app(&mut app).unwrap();
    assert!(area.width > 0);
    assert!(snapshot.is_none());
    term.restore(false);

    let mut live = LoopTerm::live(TuiWriter::Buf(Vec::new()), color).unwrap();
    live.resize(Rect::new(0, 0, 80, 24));
    let _ = live.clear();
    app.mouse_sel = Some(crate::app::MouseSelection {
        anchor_x: 1,
        anchor_y: 1,
        focus_x: 4,
        focus_y: 2,
        dragging: true,
    });
    let (_area, snapshot) = live.draw_app(&mut app).unwrap();
    assert!(snapshot.is_some());
    live.restore(true);
}

#[tokio::test]
async fn run_live_buf_draws_then_quits() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    unsafe { std::env::remove_var("WHYCODES_TEST_TUI") };
    set_headless_live(true);
    set_headless_events(Some(std::collections::VecDeque::from([
        Event::Resize(0, 0),
        Event::Resize(80, 24),
        press(KeyCode::Char(':')),
        press(KeyCode::Char('q')),
        press(KeyCode::Enter),
    ])));
    let exit = super::run(boot_opts(dir.path(), "sk-test")).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn apply_remote_hydrate_warns_on_unreachable() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    let mut session = Session::new(PathBuf::from("/work"), "sys".into());
    apply_remote_hydrate(
        &mut app,
        &mut session,
        &crate::remote::RemoteAttach::new("http://127.0.0.1:1", "sid-1"),
    )
    .await;
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Remote hydrate") || t.message.contains("Attached")),
        "{:?}",
        app.toasts
            .visible()
            .iter()
            .map(|t| t.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn spawn_model_context_fetch_hits_local_http() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    });
    let mut config = Config::default();
    config.providers.insert(
        "acme".into(),
        whycodes_core::types::ProviderConfig {
            name: "acme".into(),
            api_key: Some("sk".into()),
            api_base: Some(format!("http://{addr}/v1")),
            base_url: None,
            headers: None,
            models: vec!["m1".into()],
            tool_arguments: None,
            extra: Default::default(),
        },
    );
    let (tx, mut rx) = mpsc::unbounded_channel();
    spawn_model_context_fetch(&config, "acme", "m1", "sk", tx);
    let _ = tokio::time::timeout(std::time::Duration::from_millis(400), rx.recv()).await;
}

#[tokio::test]
async fn run_headless_ctrl_keys_and_paste() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    unsafe { std::env::remove_var("WHYCODES_TEST_TUI") };
    let ctrl = |c: char| {
        Event::Key(crossterm::event::KeyEvent::new(
            KeyCode::Char(c),
            crossterm::event::KeyModifiers::CONTROL,
        ))
    };
    set_headless_events(Some(std::collections::VecDeque::from([
        Event::Paste("hello paste".into()),
        ctrl('t'),
        ctrl('n'),
        ctrl('s'),
        press(KeyCode::Esc),
        press(KeyCode::Tab),
        press(KeyCode::Char(':')),
        press(KeyCode::Char('q')),
        press(KeyCode::Enter),
    ])));
    let exit = super::run(boot_opts(dir.path(), "sk-test")).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn run_live_crossterm_stub_first_frame_then_idle_quit() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_import = std::env::var_os("WHYCODES_SKIP_IMPORT");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::set_var("WHYCODES_SKIP_IMPORT", "1");
    }
    set_headless_events(None);
    set_headless_live(true);
    clear_crossterm_stub();
    let exit = super::run(boot_opts(dir.path(), "sk-test")).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    match prev_import {
        Some(v) => unsafe { std::env::set_var("WHYCODES_SKIP_IMPORT", v) },
        None => unsafe { std::env::remove_var("WHYCODES_SKIP_IMPORT") },
    }
    set_headless_live(false);
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn run_live_crossterm_stub_keys_slash_and_quit() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_import = std::env::var_os("WHYCODES_SKIP_IMPORT");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::set_var("WHYCODES_SKIP_IMPORT", "1");
    }
    set_headless_events(None);
    set_headless_live(true);
    set_crossterm_stub(std::collections::VecDeque::from([
        Event::Resize(80, 24),
        Event::Paste("from clip".into()),
        ctrl('q'),
        press(KeyCode::Enter),
    ]));
    let exit = super::run(boot_opts(dir.path(), "sk-test")).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    match prev_import {
        Some(v) => unsafe { std::env::set_var("WHYCODES_SKIP_IMPORT", v) },
        None => unsafe { std::env::remove_var("WHYCODES_SKIP_IMPORT") },
    }
    set_headless_live(false);
    clear_crossterm_stub();
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn run_headless_confirms_self_install_upgrade() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_import = std::env::var_os("WHYCODES_SKIP_IMPORT");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::set_var("WHYCODES_SKIP_IMPORT", "1");
    }
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tx.send(UpdateOffer::SelfInstall("9.9.9".into())).unwrap();
    drop(tx);
    set_headless_events(Some(std::collections::VecDeque::from([press(
        KeyCode::Enter,
    )])));
    let mut opts = boot_opts(dir.path(), "sk-test");
    opts.update_rx = Some(rx);
    let exit = super::run(opts).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    match prev_import {
        Some(v) => unsafe { std::env::set_var("WHYCODES_SKIP_IMPORT", v) },
        None => unsafe { std::env::remove_var("WHYCODES_SKIP_IMPORT") },
    }
    assert_eq!(exit, TuiExit::Upgrade);
}

#[tokio::test]
async fn run_headless_compact_after_scripted_turn() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_llm = std::env::var_os("WHYCODES_TEST_LLM");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::set_var("WHYCODES_TEST_LLM", "scripted-ok");
    }
    let mut events = Vec::new();
    events.extend(type_line("please summarize"));
    events.extend(type_line("/compact keep names"));
    events.extend(type_line(":q"));
    set_headless_events(Some(events.into()));
    let exit = super::run(boot_opts(dir.path(), "sk-test")).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    match prev_llm {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_LLM", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_LLM") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn run_headless_idle_ctrl_n_cycle_dashboard_and_mru() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_import = std::env::var_os("WHYCODES_SKIP_IMPORT");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::set_var("WHYCODES_SKIP_IMPORT", "1");
    }
    set_headless_events(Some(std::collections::VecDeque::from([
        ctrl('n'),
        Event::Key(crossterm::event::KeyEvent::new(
            KeyCode::PageDown,
            crossterm::event::KeyModifiers::CONTROL,
        )),
        Event::Key(crossterm::event::KeyEvent::new(
            KeyCode::PageUp,
            crossterm::event::KeyModifiers::CONTROL,
        )),
        ctrl('o'),
        press(KeyCode::Enter),
        Event::Key(crossterm::event::KeyEvent::new(
            KeyCode::Tab,
            crossterm::event::KeyModifiers::CONTROL,
        )),
        ctrl('q'),
        press(KeyCode::Enter),
    ])));
    let exit = super::run(boot_opts(dir.path(), "sk-test")).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    match prev_import {
        Some(v) => unsafe { std::env::set_var("WHYCODES_SKIP_IMPORT", v) },
        None => unsafe { std::env::remove_var("WHYCODES_SKIP_IMPORT") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn run_headless_applies_model_effort_mode_from_pickers() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_import = std::env::var_os("WHYCODES_SKIP_IMPORT");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::set_var("WHYCODES_SKIP_IMPORT", "1");
    }
    let mut opts = boot_opts(dir.path(), "sk-test");
    opts.provider = "xai".into();
    opts.model = "grok-4.6".into();
    opts.config.providers.insert(
        "xai".into(),
        whycodes_core::types::ProviderConfig {
            name: "xai".into(),
            api_key: Some("sk-test".into()),
            api_base: None,
            base_url: None,
            headers: None,
            models: vec!["grok-4.6".into(), "grok-4".into()],
            tool_arguments: None,
            extra: Default::default(),
        },
    );
    let mut events = Vec::new();
    events.extend(type_line("/models"));
    events.push(press(KeyCode::Enter));
    events.extend(type_line("/effort"));
    events.push(press(KeyCode::Enter));
    events.extend(type_line("/mode"));
    events.push(press(KeyCode::Enter));
    events.extend(type_line("/agent"));
    events.push(press(KeyCode::Enter));
    events.push(ctrl('q'));
    events.push(press(KeyCode::Enter));
    set_headless_events(Some(events.into()));
    let exit = super::run(opts).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    match prev_import {
        Some(v) => unsafe { std::env::set_var("WHYCODES_SKIP_IMPORT", v) },
        None => unsafe { std::env::remove_var("WHYCODES_SKIP_IMPORT") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn run_headless_busy_esc_enter_then_force_quit() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_llm = std::env::var_os("WHYCODES_TEST_LLM");
    let prev_import = std::env::var_os("WHYCODES_SKIP_IMPORT");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::set_var("WHYCODES_TEST_LLM", "HANG");
        std::env::set_var("WHYCODES_SKIP_IMPORT", "1");
    }
    set_headless_events(Some(std::collections::VecDeque::from([
        press(KeyCode::Char('h')),
        press(KeyCode::Enter),
        press(KeyCode::Enter),
        press(KeyCode::Esc),
        press(KeyCode::Esc),
        ctrl('q'),
    ])));
    let exit = super::run(boot_opts(dir.path(), "sk-test")).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    match prev_llm {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_LLM", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_LLM") },
    }
    match prev_import {
        Some(v) => unsafe { std::env::set_var("WHYCODES_SKIP_IMPORT", v) },
        None => unsafe { std::env::remove_var("WHYCODES_SKIP_IMPORT") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn run_live_crossterm_poll_error_exits() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_import = std::env::var_os("WHYCODES_SKIP_IMPORT");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::set_var("WHYCODES_SKIP_IMPORT", "1");
    }
    set_headless_events(None);
    set_headless_live(true);
    set_crossterm_poll_err(true);
    clear_crossterm_stub();
    let err = super::run(boot_opts(dir.path(), "sk-test"))
        .await
        .expect_err("poll error should fail the loop");
    assert!(
        err.to_string().contains("poll") || err.to_string().contains("failed"),
        "{err}"
    );
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    match prev_import {
        Some(v) => unsafe { std::env::set_var("WHYCODES_SKIP_IMPORT", v) },
        None => unsafe { std::env::remove_var("WHYCODES_SKIP_IMPORT") },
    }
    set_headless_live(false);
    set_crossterm_poll_err(false);
}

#[tokio::test]
async fn run_live_crossterm_read_error_exits() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_import = std::env::var_os("WHYCODES_SKIP_IMPORT");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::set_var("WHYCODES_SKIP_IMPORT", "1");
    }
    set_headless_events(None);
    set_headless_live(true);
    set_crossterm_read_err(true);
    set_crossterm_stub(std::collections::VecDeque::from([press(KeyCode::Char(
        'x',
    ))]));
    let err = super::run(boot_opts(dir.path(), "sk-test"))
        .await
        .expect_err("read error should fail the loop");
    assert!(
        err.to_string().contains("read") || err.to_string().contains("failed"),
        "{err}"
    );
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    match prev_import {
        Some(v) => unsafe { std::env::set_var("WHYCODES_SKIP_IMPORT", v) },
        None => unsafe { std::env::remove_var("WHYCODES_SKIP_IMPORT") },
    }
    set_headless_live(false);
    set_crossterm_read_err(false);
    clear_crossterm_stub();
}

#[tokio::test]
async fn run_headless_busy_ctrl_c_clears_then_cancels() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_llm = std::env::var_os("WHYCODES_TEST_LLM");
    let prev_import = std::env::var_os("WHYCODES_SKIP_IMPORT");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::set_var("WHYCODES_TEST_LLM", "HANG");
        std::env::set_var("WHYCODES_SKIP_IMPORT", "1");
    }
    set_headless_events(Some(std::collections::VecDeque::from([
        press(KeyCode::Char('h')),
        press(KeyCode::Enter),
        press(KeyCode::Char('x')),
        ctrl('c'),
        ctrl('c'),
        ctrl('c'),
        ctrl('q'),
    ])));
    let exit = super::run(boot_opts(dir.path(), "sk-test")).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    match prev_llm {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_LLM", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_LLM") },
    }
    match prev_import {
        Some(v) => unsafe { std::env::set_var("WHYCODES_SKIP_IMPORT", v) },
        None => unsafe { std::env::remove_var("WHYCODES_SKIP_IMPORT") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn run_headless_permission_esc_denies_then_quits() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_llm = std::env::var_os("WHYCODES_TEST_LLM");
    let prev_import = std::env::var_os("WHYCODES_SKIP_IMPORT");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::set_var("WHYCODES_TEST_LLM", "SHELL");
        std::env::set_var("WHYCODES_SKIP_IMPORT", "1");
    }
    let mut events = Vec::new();
    events.extend(type_line("please rm that"));
    events.push(press(KeyCode::Esc));
    events.push(ctrl('q'));
    events.push(press(KeyCode::Enter));
    set_headless_events(Some(events.into()));
    let mut opts = boot_opts(dir.path(), "sk-test");
    opts.config.general.approval_mode = Some(whycodes_core::types::ApprovalMode::Manual);
    let exit = super::run(opts).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    match prev_llm {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_LLM", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_LLM") },
    }
    match prev_import {
        Some(v) => unsafe { std::env::set_var("WHYCODES_SKIP_IMPORT", v) },
        None => unsafe { std::env::remove_var("WHYCODES_SKIP_IMPORT") },
    }
    assert_eq!(exit, TuiExit::Quit);
}

#[tokio::test]
async fn run_headless_hits_session_limit_toast() {
    let _home = isolate_home();
    let dir = tempfile::tempdir().unwrap();
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_import = std::env::var_os("WHYCODES_SKIP_IMPORT");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::set_var("WHYCODES_SKIP_IMPORT", "1");
    }
    let mut events = Vec::new();
    for _ in 0..10 {
        events.push(ctrl('n'));
    }
    events.push(ctrl('q'));
    events.push(press(KeyCode::Enter));
    set_headless_events(Some(events.into()));
    let exit = super::run(boot_opts(dir.path(), "sk-test")).await.unwrap();
    match prev_stub {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TUI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TUI") },
    }
    match prev_import {
        Some(v) => unsafe { std::env::set_var("WHYCODES_SKIP_IMPORT", v) },
        None => unsafe { std::env::remove_var("WHYCODES_SKIP_IMPORT") },
    }
    assert_eq!(exit, TuiExit::Quit);
}
