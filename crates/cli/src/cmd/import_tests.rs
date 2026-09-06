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
fn first_run_skips_when_ci_or_skip_env() {
    let prev_ci = std::env::var_os("CI");
    unsafe { std::env::set_var("CI", "1") };
    assert!(!maybe_first_run_import(true).unwrap());
    match prev_ci {
        Some(v) => unsafe { std::env::set_var("CI", v) },
        None => unsafe { std::env::remove_var("CI") },
    }
    let prev_skip = std::env::var_os("WHYCODES_SKIP_IMPORT");
    unsafe { std::env::set_var("WHYCODES_SKIP_IMPORT", "1") };
    assert!(!maybe_first_run_import(true).unwrap());
    match prev_skip {
        Some(v) => unsafe { std::env::set_var("WHYCODES_SKIP_IMPORT", v) },
        None => unsafe { std::env::remove_var("WHYCODES_SKIP_IMPORT") },
    }
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
    crate::cmd::helpers::install_test_repl_lines([""]);
    prompt_item_selection(&mut plan).unwrap();
    crate::cmd::helpers::clear_test_repl_lines();
    assert_eq!(plan.mcp_add.len(), 1);
}
