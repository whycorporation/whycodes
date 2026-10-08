use super::*;

#[test]
fn fold_parallel_joins_ok_and_fail() {
    fold_parallel_joins([Ok(false)], false).unwrap();
    assert!(fold_parallel_joins([Ok(true)], false).is_err());
    assert!(fold_parallel_joins([Err("x".into())], true).is_err());
    assert!(fold_parallel_joins([Err("x".into())], false).is_err());
}

#[test]
fn emit_parallel_outcome_covers_formats() {
    let meta = ResultMeta {
        session_id: "s".into(),
        provider: "p".into(),
        model: "m".into(),
        agent: "a".into(),
        usage: Default::default(),
        duration_ms: 1,
        slop: None,
    };
    let wrap = |ev: CiEvent| ev;
    assert!(!emit_parallel_outcome(
        OutputFormat::Text,
        Ok("ok".into()),
        meta.clone(),
        &wrap
    ));
    assert!(!emit_parallel_outcome(
        OutputFormat::Text,
        Ok(String::new()),
        meta.clone(),
        &wrap
    ));
    assert!(!emit_parallel_outcome(
        OutputFormat::Json,
        Ok("j".into()),
        meta.clone(),
        &wrap
    ));
    assert!(!emit_parallel_outcome(
        OutputFormat::StreamJson,
        Ok("s".into()),
        meta.clone(),
        &wrap
    ));
    assert!(emit_parallel_outcome(
        OutputFormat::Text,
        Err("boom".into()),
        meta.clone(),
        &wrap
    ));
    assert!(emit_parallel_outcome(
        OutputFormat::Json,
        Err("jerr".into()),
        meta.clone(),
        &wrap
    ));
    assert!(emit_parallel_outcome(
        OutputFormat::StreamJson,
        Err("cancel me".into()),
        meta.clone(),
        &wrap
    ));
    assert!(emit_parallel_outcome(
        OutputFormat::StreamJson,
        Err("provider down".into()),
        meta,
        &wrap
    ));
}

#[test]
fn helpers_all_prompts_and_fan_out() {
    assert!(all_prompts_empty(&[String::new(), String::new()]));
    assert!(!all_prompts_empty(&["x".into()]));
    assert!(!should_fan_out(&["a".into()]));
    assert!(should_fan_out(&["a".into(), "b".into()]));
    let mapped = map_tui_run_error(anyhow::anyhow!("os error 6"));
    assert!(mapped.to_string().contains("TUI needs a real terminal"));
    let mapped = map_tui_run_error(anyhow::anyhow!("No such device"));
    assert!(mapped.to_string().contains("TUI needs a real terminal"));
    let mapped = map_tui_run_error(anyhow::anyhow!("not a terminal"));
    assert!(mapped.to_string().contains("TUI needs a real terminal"));
    let passthrough = map_tui_run_error(anyhow::anyhow!("bind failed"));
    assert_eq!(passthrough.to_string(), "bind failed");
}

#[test]
fn tui_plain_and_resume_helpers() {
    assert!(!force_plain_mode(false));
    assert!(force_plain_mode(true));
    let prev = std::env::var_os("WHYCODES_PLAIN");
    unsafe { std::env::set_var("WHYCODES_PLAIN", "1") };
    assert!(force_plain_mode(false));
    match prev {
        Some(v) => unsafe { std::env::set_var("WHYCODES_PLAIN", v) },
        None => unsafe { std::env::remove_var("WHYCODES_PLAIN") },
    }

    assert!(!should_use_tui(true, true, true));
    assert!(should_use_tui(false, true, false));
    // `cmd_run` tests run with `cfg!(test)`; a live console must not open TUI.
    assert!(!should_use_tui(false, false, true));
    assert!(!should_use_tui(false, false, false));
    assert!(!should_use_tui_in(true, false, false, true));
    assert!(should_use_tui_in(true, false, true, false));
    assert!(should_use_tui_in(false, false, false, true));
    assert!(!should_use_tui_in(false, true, true, true));

    assert!(is_repl_interactive(None, false));
    assert!(is_repl_interactive(Some(""), false));
    assert!(!is_repl_interactive(Some("hi"), false));
    assert!(!is_repl_interactive(None, true));

    assert_eq!(
        resume_missing_label(whycodes_tui::RESUME_LATEST),
        "none saved yet"
    );
    assert_eq!(resume_missing_label("sess-1"), "sess-1");
}

#[test]
fn slash_info_cost_doctor_and_resume_helpers() {
    assert_eq!(session_token_label(true, 42, 0, 0, 0), "Tokens≈42 (est)");
    assert_eq!(
        session_token_label(false, 0, 10, 3, 13),
        "Tokens: 10 in / 3 out / 13 total"
    );
    assert_eq!(
        session_cost_line(true, 7, 0, 0, 0, None),
        "  session: ~7 tokens (estimated)"
    );
    assert_eq!(
        session_cost_line(false, 0, 2, 4, 6, None),
        "  session: 2 in / 4 out · total 6"
    );
    assert!(
        session_cost_line(false, 0, 2, 4, 6, Some("interactive"))
            .contains("credential: interactive")
    );
    assert_eq!(doctor_api_key_status(true, true), "not required");
    assert_eq!(doctor_api_key_status(true, false), "set");
    assert_eq!(doctor_api_key_status(false, true), "MISSING");
    match resume_slash_want("/resume", "abc") {
        ResumeSlash::Id(id) => assert_eq!(id, "abc"),
        ResumeSlash::List => panic!("expected id"),
    }
    match resume_slash_want("/continue", "") {
        ResumeSlash::Id(id) => assert_eq!(id, whycodes_tui::RESUME_LATEST),
        ResumeSlash::List => panic!("expected latest"),
    }
    assert!(matches!(
        resume_slash_want("/resume", ""),
        ResumeSlash::List
    ));
    match parse_models_slash("acme/m") {
        ModelsSlash::ProviderModel(p, m) => {
            assert_eq!(p, "acme");
            assert_eq!(m, "m");
        }
        ModelsSlash::ModelOnly(_) => panic!("expected provider/model"),
    }
    match parse_models_slash("solo") {
        ModelsSlash::ModelOnly(m) => assert_eq!(m, "solo"),
        ModelsSlash::ProviderModel(_, _) => panic!("expected model-only"),
    }
    assert!(thinking_display_label(true).contains("ON"));
    assert!(thinking_display_label(false).contains("OFF"));
    assert!(matches!(parse_effort_slash(""), EffortSlash::Show));
    assert!(matches!(
        parse_effort_slash("high"),
        EffortSlash::Set(whycodes_llm::ReasoningEffort::High)
    ));
    assert!(matches!(parse_effort_slash("nope"), EffortSlash::Unknown));
    assert_eq!(masked_api_key_prefix("abcdefghij"), "abcdefgh");
    assert_eq!(masked_api_key_prefix("ab"), "ab");
}

#[test]
fn slash_printer_helpers_cover_repl_status_lines() {
    assert!(unknown_slash_line("/nope").contains("/nope"));
    assert!(new_session_line("hello").contains("hello"));
    assert!(rename_usage_line("t", "manual").contains("usage"));
    assert!(renamed_line("n").contains("n"));
    assert!(undid_turn_line(3).contains("3"));
    assert!(redid_turn_line(4).contains("4"));
    let compact = compact_ok_line(10, 2, 100, 20);
    assert!(compact.contains("10"));
    assert!(compact.contains("2"));
    let ctx = context_report_lines(2, 40, 8000, "on", "core");
    assert!(ctx.iter().any(|l| l.contains("messages: 2")));
    assert!(ctx.iter().any(|l| l.contains("llm=on")));
    let doctor = doctor_report_lines("p", "m", "/proj", "set", "bwrap", true, "core");
    assert!(doctor.iter().any(|l| l.contains("provider: p")));
    assert!(doctor.iter().any(|l| l.contains("network=true")));
    assert!(resumed_line("title", "abcdefghij", 7).contains("title"));
    assert!(switched_model_line("acme", "m").contains("acme"));
    assert!(model_set_line("solo").contains("solo"));
    assert!(effort_unknown_line("nope").contains("nope"));
    assert!(effort_no_levels_line().contains("no reasoning-effort"));
    assert!(switched_agent_line("plan").contains("plan"));
    assert!(api_key_loaded_line("xai", "abcdefgh").contains("xai"));
    let missing = connect_missing_key_lines("xai", true);
    assert!(missing.iter().any(|l| l.contains("auth login xai")));
    let missing = connect_missing_key_lines("acme", false);
    assert!(missing.iter().all(|l| !l.contains("auth login")));
    assert!(login_connected_label(true).contains("connected"));
    assert!(login_connected_label(false).contains("not connected"));
    assert!(oauth_unavailable_line("acme", "anthropic").contains("acme"));
    assert!(themes_set_hint("nord").contains("nord"));
    assert!(tools_list_header(3).contains("3"));
    assert!(remembered_line("abcdefghij", "note").contains("note"));
    assert!(repl_memory_status(true, 2, "/tmp/m.md").contains("entries=2"));
    assert!(nothing_to_undo_line().contains("Nothing to undo"));
    assert!(nothing_to_redo_line().contains("Nothing to redo"));
    assert!(init_wrote_line("AGENTS.md").contains("AGENTS.md"));
    assert!(init_failed_line("boom").contains("boom"));
    assert!(session_exported_line("/tmp/s.md").contains("/tmp/s.md"));
    assert!(export_failed_line("io").contains("io"));
    assert!(skip_prompt_cache_line().contains("prompt cache"));
    assert!(nothing_to_compact_line().contains("Nothing to compact"));
    assert!(compacting_line().contains("Compacting"));
    assert_eq!(bang_usage_line(), "Usage: ! <shell command>");
    assert!(bang_echo_line("ls").contains("ls"));
    assert!(custom_command_line("review").contains("review"));
    assert!(git_unavailable_line("enoent").contains("enoent"));
    assert!(git_status_failed_line("fatal").contains("fatal"));
}

/// Bare `whycodes` paints the home screen through `cmd_run_fast_tui`. That
/// path used to hard-code `update_rx: None` and the update modal vanished
/// (v0.6.4). Keep the spawn inside this function's source.
#[test]
fn fast_tui_source_spawns_the_update_check() {
    let src = include_str!("run.rs");
    let start = src
        .find("pub(crate) fn cmd_run_fast_tui")
        .expect("cmd_run_fast_tui");
    let rest = &src[start..];
    let end = rest[1..]
        .find("\npub(crate) ")
        .map(|i| i + 1)
        .unwrap_or(rest.len());
    let fn_src = &rest[..end];
    assert!(
        fn_src.contains("spawn_update_check_if"),
        "cmd_run_fast_tui must spawn the GitHub update check"
    );
    assert!(
        fn_src.contains("should_auto_update_fast_path"),
        "cmd_run_fast_tui must honor CI / WHYCODES_NO_AUTO_UPDATE / WHYCODES_BENCH"
    );
    assert!(
        fn_src.contains("update_rx,"),
        "cmd_run_fast_tui must pass the spawned receiver into TuiRunOptions"
    );
    let assigned_none = fn_src.lines().any(|line| {
        let t = line.trim();
        t.starts_with("update_rx:") && t.contains("None")
    });
    assert!(
        !assigned_none,
        "cmd_run_fast_tui must not assign update_rx: None (home-screen confirm never appears)"
    );
}

#[test]
fn finish_fast_tui_bench_maps_paint_errors_and_both_exits() {
    finish_fast_tui_bench(|| Ok(Some(whycodes_tui::TuiExit::Quit))).unwrap();
    finish_fast_tui_bench(|| Ok(Some(whycodes_tui::TuiExit::Upgrade))).unwrap();
    finish_fast_tui_bench(|| Ok(None)).unwrap();
    let err = finish_fast_tui_bench(|| Err(anyhow::anyhow!("not a terminal"))).unwrap_err();
    assert!(err.to_string().contains("whycodes --plain"));
    let other = finish_fast_tui_bench(|| Err(anyhow::anyhow!("disk full"))).unwrap_err();
    assert!(other.to_string().contains("disk full"));
}

#[test]
fn reload_config_after_import_keeps_current_on_error() {
    let dir = std::env::temp_dir();
    let mut config = Config::default();
    assert!(reload_config_after_import(&mut config, &dir, true, |_| Ok(
        Config::default()
    )));
    assert!(!config.memory.enabled);
    assert!(!reload_config_after_import(
        &mut config,
        &dir,
        false,
        |_| Err(anyhow::anyhow!("toml"))
    ));
}

/// `WHYCODES_TEST_TUI` returns before a terminal opens, so the clap-free
/// entry can run in CI. `WHYCODES_NO_AUTO_UPDATE` keeps the GitHub check
/// from spawning a network task on this path.
#[test]
fn fast_tui_quits_and_upgrades_under_the_test_stub() {
    let home = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("WHYCODES_HOME");
    let prev_stub = std::env::var_os("WHYCODES_TEST_TUI");
    let prev_skip = std::env::var_os("WHYCODES_TEST_SKIP_UPGRADE");
    let prev_no = std::env::var_os("WHYCODES_NO_AUTO_UPDATE");
    let prev_bench = std::env::var_os("WHYCODES_BENCH");
    let prev_ci = std::env::var_os("CI");
    unsafe {
        std::env::set_var("WHYCODES_HOME", home.path());
        std::env::remove_var("WHYCODES_BENCH");
        std::env::set_var("WHYCODES_NO_AUTO_UPDATE", "1");
        std::env::set_var("WHYCODES_TEST_SKIP_UPGRADE", "1");
        std::env::set_var("WHYCODES_TEST_TUI", "quit");
    }
    let dir = std::env::temp_dir();
    cmd_run_fast_tui(dir.clone()).expect("quit stub");
    unsafe { std::env::set_var("WHYCODES_TEST_TUI", "upgrade") };
    cmd_run_fast_tui(dir.clone()).expect("upgrade stub");
    // A file where the data dir should be makes `logging::init` return Err
    // unless an earlier test already installed the process-wide subscriber.
    let blocked = home.path().join("not-a-dir");
    std::fs::write(&blocked, b"x").expect("block data dir");
    unsafe { std::env::set_var("WHYCODES_HOME", &blocked) };
    let logged = cmd_run_fast_tui(dir);
    assert!(
        logged.is_ok() || logged.unwrap_err().to_string().contains("runtime"),
        "stub still returns after a logging init failure"
    );
    unsafe {
        match prev_home {
            Some(v) => std::env::set_var("WHYCODES_HOME", v),
            None => std::env::remove_var("WHYCODES_HOME"),
        }
        match prev_stub {
            Some(v) => std::env::set_var("WHYCODES_TEST_TUI", v),
            None => std::env::remove_var("WHYCODES_TEST_TUI"),
        }
        match prev_skip {
            Some(v) => std::env::set_var("WHYCODES_TEST_SKIP_UPGRADE", v),
            None => std::env::remove_var("WHYCODES_TEST_SKIP_UPGRADE"),
        }
        match prev_no {
            Some(v) => std::env::set_var("WHYCODES_NO_AUTO_UPDATE", v),
            None => std::env::remove_var("WHYCODES_NO_AUTO_UPDATE"),
        }
        match prev_bench {
            Some(v) => std::env::set_var("WHYCODES_BENCH", v),
            None => std::env::remove_var("WHYCODES_BENCH"),
        }
        match prev_ci {
            Some(v) => std::env::set_var("CI", v),
            None => std::env::remove_var("CI"),
        }
    }
}
