use super::*;

#[test]
fn print_helpers_do_not_panic() {
    let src = FoundSource {
        product: Product::Claude,
        rel_path: ".claude.json",
        path: std::path::PathBuf::from("/tmp/x"),
        state: SourceState::New,
    };
    print_found(&[src]);
    print_found(&[FoundSource {
        product: Product::Claude,
        rel_path: ".claude.json",
        path: std::path::PathBuf::from("/tmp/a"),
        state: SourceState::Approved,
    }]);
    print_found(&[FoundSource {
        product: Product::Grok,
        rel_path: ".grok/config.toml",
        path: std::path::PathBuf::from("/tmp/g"),
        state: SourceState::Denied,
    }]);
    print_found(&[FoundSource {
        product: Product::Codex,
        rel_path: ".codex/config.toml",
        path: std::path::PathBuf::from("/tmp/c"),
        state: SourceState::Symlink,
    }]);
    print_extracted(&[whycodes_import::Extracted {
        product: Product::OpenCode,
        path: std::path::PathBuf::from("/tmp/o"),
        mcp: Vec::new(),
        permission: Default::default(),
        hooks: Vec::new(),
        skipped: vec!["event SessionStart".into()],
    }]);
    let mut plan = ImportPlan::default();
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
    plan.mcp_skip
        .push(("git".into(), "already have `git`".into()));
    plan.permission_add
        .push(("bash".into(), whycodes_core::types::PermissionAction::Ask));
    plan.permission_skip
        .push(("read".into(), "already have `read`".into()));
    plan.hooks_add.push(whycodes_config::HookConfig {
        event: whycodes_config::HookEvent::PreTool,
        tool_match: "bash".into(),
        command: "echo hi".into(),
        block_on_failure: true,
        timeout_secs: 30,
    });
    plan.hooks_skip.push("pre_tool echo hi".into());
    plan.warnings.push("ignored".into());
    print_plan(&plan);
    assert!(!maybe_first_run_import(false).unwrap());
}

#[test]
fn prompt_item_selection_keeps_and_skips() {
    let mut plan = ImportPlan::default();
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
    let _guard = crate::cmd::helpers::lock_env();
    crate::cmd::helpers::install_test_repl_lines(["y", "n"]);
    prompt_item_selection(&mut plan).unwrap();
    crate::cmd::helpers::clear_test_repl_lines();
    assert_eq!(plan.mcp_add.len(), 1);
    assert!(plan.permission_add.is_empty());
}

#[test]
fn prompt_item_selection_empty_plan_is_ok() {
    let mut plan = ImportPlan::default();
    prompt_item_selection(&mut plan).unwrap();
    assert!(plan.is_empty());
}

#[test]
fn first_run_skips_when_forced_ci_or_skip_env() {
    let _iso = crate::cmd::helpers::IsolatedHome::new();
    let prev_ci = std::env::var_os("CI");
    let prev_force = std::env::var_os("WHYCODES_FORCE_CI");
    unsafe {
        std::env::set_var("CI", "1");
        std::env::set_var("WHYCODES_FORCE_CI", "1");
    }
    let skipped = maybe_first_run_import(true).unwrap();
    match prev_ci {
        Some(v) => unsafe { std::env::set_var("CI", v) },
        None => unsafe { std::env::remove_var("CI") },
    }
    match prev_force {
        Some(v) => unsafe { std::env::set_var("WHYCODES_FORCE_CI", v) },
        None => unsafe { std::env::remove_var("WHYCODES_FORCE_CI") },
    }
    assert!(!skipped);

    let prev_skip = std::env::var_os("WHYCODES_SKIP_IMPORT");
    unsafe { std::env::set_var("WHYCODES_SKIP_IMPORT", "1") };
    let skipped = maybe_first_run_import(true).unwrap();
    match prev_skip {
        Some(v) => unsafe { std::env::set_var("WHYCODES_SKIP_IMPORT", v) },
        None => unsafe { std::env::remove_var("WHYCODES_SKIP_IMPORT") },
    }
    assert!(!skipped);
}

#[test]
fn prompt_item_selection_empty_line_keeps() {
    let mut plan = ImportPlan::default();
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
    let _guard = crate::cmd::helpers::lock_env();
    crate::cmd::helpers::install_test_repl_lines([""]);
    prompt_item_selection(&mut plan).unwrap();
    crate::cmd::helpers::clear_test_repl_lines();
    assert_eq!(plan.mcp_add.len(), 1);
}

fn sample_mcp() -> whycodes_config::McpServerConfig {
    whycodes_config::McpServerConfig {
        transport: None,
        command: Some("npx".into()),
        args: vec![],
        env: None,
        cwd: None,
        url: None,
        headers: None,
    }
}

fn foreign_home() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::write(
        home.join(".claude.json"),
        r#"{"mcpServers":{"fs":{"command":"npx","args":["-y","@mcp/fs"]}}}"#,
    )
    .unwrap();
    (dir, home)
}

#[tokio::test]
async fn cmd_import_unknown_product_errors() {
    let _home = crate::cmd::helpers::IsolatedHome::new();
    let err = cmd_import(&ImportArgs {
        from: Some("not-a-product".into()),
        dry_run: true,
        yes: true,
        force: false,
    })
    .await
    .unwrap_err();
    assert!(err.to_string().contains("not-a-product"));
}

#[tokio::test]
async fn cmd_import_yes_dry_run_finds_nothing_when_home_empty() {
    let _iso = crate::cmd::helpers::IsolatedHome::new();
    let dir = tempfile::tempdir().unwrap();
    let prev = std::env::var_os("HOME");
    let prev_profile = std::env::var_os("USERPROFILE");
    unsafe {
        std::env::set_var("HOME", dir.path());
        std::env::set_var("USERPROFILE", dir.path());
    }
    cmd_import(&ImportArgs {
        from: None,
        dry_run: true,
        yes: true,
        force: false,
    })
    .await
    .unwrap();
    unsafe {
        match prev {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match prev_profile {
            Some(v) => std::env::set_var("USERPROFILE", v),
            None => std::env::remove_var("USERPROFILE"),
        }
    }
}

#[test]
fn run_import_filters_product_and_dry_runs() {
    let _iso = crate::cmd::helpers::IsolatedHome::new();
    let (_dir, home) = foreign_home();
    let data = tempfile::tempdir().unwrap();
    let consent = ConsentStore::new(data.path());
    run_import(&consent, &home, Some("grok"), true, true, false, false).unwrap();
    run_import(&consent, &home, Some("claude"), true, true, false, false).unwrap();
    run_import(&consent, &home, None, true, true, true, false).unwrap();
}

#[test]
fn run_import_prompts_approve_and_deny_then_skips_empty_selection() {
    let _iso = crate::cmd::helpers::IsolatedHome::new();
    let (_dir, home) = foreign_home();
    let data = tempfile::tempdir().unwrap();
    let consent = ConsentStore::new(data.path());
    crate::cmd::helpers::install_test_repl_lines(["n"]);
    run_import(&consent, &home, None, false, false, false, false).unwrap();
    crate::cmd::helpers::clear_test_repl_lines();

    let data = tempfile::tempdir().unwrap();
    let consent = ConsentStore::new(data.path());
    crate::cmd::helpers::install_test_repl_lines(["y", "n"]);
    run_import(&consent, &home, None, false, false, false, false).unwrap();
    crate::cmd::helpers::clear_test_repl_lines();
}

#[test]
fn run_import_already_asked_writes_and_second_pass_is_empty() {
    let _iso = crate::cmd::helpers::IsolatedHome::new();
    let (_dir, home) = foreign_home();
    let data = tempfile::tempdir().unwrap();
    let consent = ConsentStore::new(data.path());
    run_import(&consent, &home, None, false, false, false, true).unwrap();
    run_import(&consent, &home, None, false, true, false, false).unwrap();
}

#[test]
fn first_run_marks_asked_when_home_missing_or_only_symlinks() {
    let _iso = crate::cmd::helpers::IsolatedHome::new();
    let prev_home = std::env::var_os("HOME");
    let prev_profile = std::env::var_os("USERPROFILE");
    unsafe {
        std::env::set_var("HOME", "");
        std::env::set_var("USERPROFILE", "");
    }
    assert!(!maybe_first_run_import(true).unwrap());
    unsafe {
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match prev_profile {
            Some(v) => std::env::set_var("USERPROFILE", v),
            None => std::env::remove_var("USERPROFILE"),
        }
    }
}

#[test]
fn first_run_declines_then_already_asked() {
    let iso = crate::cmd::helpers::IsolatedHome::new();
    let home = iso.dir().join("agent-home");
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::write(
        home.join(".claude.json"),
        r#"{"mcpServers":{"fs":{"command":"npx","args":["-y","@mcp/fs"]}}}"#,
    )
    .unwrap();
    let prev_home = std::env::var_os("HOME");
    let prev_profile = std::env::var_os("USERPROFILE");
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("USERPROFILE", &home);
    }
    crate::cmd::helpers::install_test_repl_lines(["n"]);
    assert!(!maybe_first_run_import(true).unwrap());
    crate::cmd::helpers::clear_test_repl_lines();
    // Consent already recorded the ask.
    assert!(!maybe_first_run_import(true).unwrap());
    unsafe {
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match prev_profile {
            Some(v) => std::env::set_var("USERPROFILE", v),
            None => std::env::remove_var("USERPROFILE"),
        }
    }
}

#[test]
fn first_run_empty_home_marks_asked_then_yes_imports() {
    let iso = crate::cmd::helpers::IsolatedHome::new();
    let empty = iso.dir().join("empty-home");
    std::fs::create_dir_all(&empty).unwrap();
    let prev_home = std::env::var_os("HOME");
    let prev_profile = std::env::var_os("USERPROFILE");
    unsafe {
        std::env::set_var("HOME", &empty);
        std::env::set_var("USERPROFILE", &empty);
    }
    assert!(!maybe_first_run_import(true).unwrap());
    // Asked marker is set; a later call with files still returns false.
    let home = iso.dir().join("agent-home");
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::write(
        home.join(".claude.json"),
        r#"{"mcpServers":{"fs":{"command":"npx","args":["-y","@mcp/fs"]}}}"#,
    )
    .unwrap();
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("USERPROFILE", &home);
    }
    assert!(!maybe_first_run_import(true).unwrap());

    // Fresh data dir so the asked marker is gone, then accept the prompt.
    let fresh = iso.dir().join("fresh");
    std::fs::create_dir_all(&fresh).unwrap();
    unsafe { std::env::set_var("WHYCODES_HOME", &fresh) };
    crate::cmd::helpers::install_test_repl_lines(["y"]);
    assert!(maybe_first_run_import(true).unwrap());
    crate::cmd::helpers::clear_test_repl_lines();
    unsafe { std::env::set_var("WHYCODES_HOME", iso.dir()) };

    unsafe {
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match prev_profile {
            Some(v) => std::env::set_var("USERPROFILE", v),
            None => std::env::remove_var("USERPROFILE"),
        }
    }
}

#[test]
fn run_import_yes_writes_then_nothing_new() {
    let _iso = crate::cmd::helpers::IsolatedHome::new();
    let (_dir, home) = foreign_home();
    let data = tempfile::tempdir().unwrap();
    let consent = ConsentStore::new(data.path());
    run_import(&consent, &home, None, false, true, false, false).unwrap();
    run_import(&consent, &home, Some("claude"), false, true, false, false).unwrap();
}

#[test]
fn prompt_item_selection_yes_word_keeps() {
    let mut plan = ImportPlan::default();
    plan.mcp_add.push(("fs".into(), sample_mcp()));
    let _guard = crate::cmd::helpers::lock_env();
    crate::cmd::helpers::install_test_repl_lines(["yes"]);
    prompt_item_selection(&mut plan).unwrap();
    crate::cmd::helpers::clear_test_repl_lines();
    assert_eq!(plan.mcp_add.len(), 1);
}
