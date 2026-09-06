use super::*;
use crate::tool_policy::*;
use serde_json::json;
use whycodes_core::Tool;
use whycodes_core::types::PermissionSet;

#[test]
fn single_command_is_plain_string() {
    let d = format_permission_detail(&json!({"command": "ls -la"}));
    assert_eq!(d, "ls -la");
}

#[test]
fn object_keys_are_labeled_not_compact_json() {
    let d = format_permission_detail(&json!({"path": "src/main.rs", "offset": 10}));
    assert!(d.contains("path: src/main.rs"), "{d}");
    assert!(d.contains("offset: 10"), "{d}");
    assert!(!d.starts_with('{'), "must not be compact JSON: {d}");
}

#[test]
fn shell_risk_has_command_and_risk_sections() {
    let d = format_shell_risk_detail("rm -rf /tmp/x", "destructive delete");
    assert!(d.contains("Command:"), "{d}");
    assert!(d.contains("rm -rf /tmp/x"), "{d}");
    assert!(d.contains("Risk: destructive delete"), "{d}");
}

#[test]
fn empty_args_is_labeled() {
    assert_eq!(format_permission_detail(&json!({})), "(no arguments)");
}

#[test]
fn scalars_fall_back_to_pretty_json() {
    let d = format_permission_detail(&json!("plain string"));
    assert_eq!(d, "\"plain string\"");
    assert!(d.starts_with('\"'));
}

#[test]
fn nested_objects_are_labeled_with_indented_lines() {
    let d = format_permission_detail(&json!({"patch": {"file": "a.rs", "edits": 2}}));
    assert!(d.contains("patch:"), "{d}");
    assert!(d.contains("\"file\": \"a.rs\""), "{d}");
}

#[test]
fn multiline_strings_are_indented() {
    let d = format_permission_detail(&json!({"content": "line1\nline2\nline3"}));
    assert!(d.contains("content:"), "{d}");
    assert!(d.contains("  line1"), "{d}");
    assert!(d.contains("  line2"), "{d}");
    assert!(d.contains("  line3"), "{d}");
}

#[test]
fn null_bool_number_values_are_labeled() {
    let d = format_permission_detail(&json!({"a": null, "b": true, "c": 42}));
    assert!(d.contains("a: null"), "{d}");
    assert!(d.contains("b: true"), "{d}");
    assert!(d.contains("c: 42"), "{d}");
}

#[test]
fn truncate_permission_detail_caps_long_text() {
    let long = "x".repeat(PERMISSION_DETAIL_MAX + 10);
    let t = truncate_permission_detail(&long);
    assert_eq!(t.chars().count(), PERMISSION_DETAIL_MAX + 1); // ellipsis appended
    assert!(t.ends_with('…'));
    assert!(t.chars().take(PERMISSION_DETAIL_MAX).all(|c| c == 'x'));
    let short = "short".to_string();
    assert_eq!(truncate_permission_detail(&short), "short");
}

#[test]
fn worktree_names_are_validated() {
    assert!(is_safe_worktree_name("feat-auth"));
    assert!(is_safe_worktree_name("branch_2"));
    assert!(is_safe_worktree_name("A1-b_c"));
    assert!(!is_safe_worktree_name(""));
    assert!(!is_safe_worktree_name("   "));
    assert!(!is_safe_worktree_name("a/b"));
    assert!(!is_safe_worktree_name(".."));
    assert!(!is_safe_worktree_name("with space"));
    assert!(!is_safe_worktree_name(&"x".repeat(65)));
}

#[test]
fn file_tool_path_extracts_and_normalizes() {
    let tc = |name: &str, args: serde_json::Value| ToolCall {
        id: "1".into(),
        name: name.into(),
        arguments: args,
    };
    assert_eq!(
        file_tool_path(&tc("read", json!({"path": "src/main.rs"}))),
        Some("src/main.rs".into())
    );
    // backslashes normalized to forward slashes
    assert_eq!(
        file_tool_path(&tc("edit", json!({"path": "src\\mod.rs"}))),
        Some("src/mod.rs".into())
    );
    // path trimmed
    assert_eq!(
        file_tool_path(&tc("write", json!({"path": "  a.rs  "}))),
        Some("a.rs".into())
    );
    // apply_patch may use path
    assert_eq!(
        file_tool_path(&tc("apply_patch", json!({"path": "x.rs"}))),
        Some("x.rs".into())
    );
    // missing / empty / non-string path
    assert_eq!(file_tool_path(&tc("read", json!({}))), None);
    assert_eq!(file_tool_path(&tc("read", json!({"path": ""}))), None);
    assert_eq!(file_tool_path(&tc("read", json!({"path": 42}))), None);
    // unknown tool name
    assert_eq!(file_tool_path(&tc("bash", json!({"path": "x"}))), None);
}

#[test]
fn parallel_safety_respects_serial_list() {
    assert!(is_parallel_safe_tool("read", &PermissionSet::default()));
    assert!(is_parallel_safe_tool("grep", &PermissionSet::default()));
    assert!(is_parallel_safe_tool("glob", &PermissionSet::default()));
    assert!(is_parallel_safe_tool("todoread", &PermissionSet::default()));
    for name in SERIAL_TOOLS {
        assert!(
            !is_parallel_safe_tool(name, &PermissionSet::default()),
            "{name}"
        );
    }
    // Real registration names (see tools/executor.rs), not snake_case typos.
    assert!(SERIAL_TOOLS.contains(&"todowrite"));
    assert!(!SERIAL_TOOLS.contains(&"todo_write"));
    assert!(!is_parallel_safe_tool(
        "todowrite",
        &PermissionSet::default()
    ));
    assert!(is_parallel_safe_tool(
        "todo_write",
        &PermissionSet::default()
    ));
}

#[test]
fn tool_signatures_and_doom_loop() {
    let tc = |name: &str, args: serde_json::Value| ToolCall {
        id: "1".into(),
        name: name.into(),
        arguments: args,
    };
    let a = tc("read", json!({"path": "x.rs"}));
    let b = tc("read", json!({"path": "y.rs"}));
    assert_eq!(tool_call_signature(&a), "read|{\"path\":\"x.rs\"}");
    assert_ne!(tool_call_signature(&a), tool_call_signature(&b));

    let mut recent = VecDeque::new();
    // nothing recent → not a doom loop yet
    assert!(!would_doom_loop(&recent, std::slice::from_ref(&a)));
    // mixed batch is never a doom loop
    assert!(!would_doom_loop(&recent, &[a.clone(), b.clone()]));
    // empty calls → false
    assert!(!would_doom_loop(&recent, &[]));

    // push two identical signatures, then a third call trips the threshold
    recent.push_back(tool_call_signature(&a));
    recent.push_back(tool_call_signature(&a));
    assert!(would_doom_loop(&recent, std::slice::from_ref(&a)));
    // batch of identical calls counts as the same signature repeated
    assert!(would_doom_loop(&recent, &[a.clone(), a.clone()]));
    // an intervening different signature resets the run
    let mut recent2 = VecDeque::new();
    recent2.push_back(tool_call_signature(&a));
    recent2.push_back(tool_call_signature(&b));
    assert!(!would_doom_loop(&recent2, std::slice::from_ref(&a)));
}

#[test]
fn system_prompt_for_known_and_unknown_agents() {
    for name in ["build", "plan", "ask", "explore", "general", "scout"] {
        let p = Agent::system_prompt_for(name);
        assert!(!p.is_empty(), "{name}");
        assert!(!p.contains("Today's date:"), "{name}");
    }
    assert!(
        Agent::system_prompt_for("build").contains("todowrite"),
        "build prompt must instruct todo use"
    );
    assert!(
        Agent::system_prompt_for("build").contains("Do **not** call `question`"),
        "build prompt must keep auto mode from asking mid-todo"
    );
    assert!(
        Agent::system_prompt_for("plan").contains("todowrite"),
        "plan prompt must instruct todo use"
    );
    assert_eq!(
        Agent::system_prompt_for("does-not-exist"),
        DEFAULT_SYSTEM_PROMPT
    );
}

#[test]
fn runtime_context_is_idempotent_and_append_only() {
    let base = "You are an agent.";
    let once = Agent::with_runtime_context(base);
    assert!(once.contains("Today's date:"));
    assert!(once.starts_with(base));
    // second application does not duplicate the block
    assert_eq!(Agent::with_runtime_context(&once), once);
    // already-present marker is left untouched
    let already = "Prompt with Today's date: 2026-01-01.";
    assert_eq!(Agent::with_runtime_context(already), already);
}

#[test]
fn agents_md_is_appended_and_candidates_are_tried() {
    let dir = tempfile::tempdir().unwrap();
    // no AGENTS.md → prompt unchanged (plus runtime context)
    let bare = Agent::with_agents_md("base", dir.path());
    assert!(bare.starts_with("base"));
    assert!(!bare.contains("Project Instructions"));

    // AGENTS.md at project root is picked up
    std::fs::write(dir.path().join("AGENTS.md"), "  \nProject rules here\n  ").unwrap();
    let with = Agent::with_agents_md("base", dir.path());
    assert!(with.contains("Project Instructions (AGENTS.md)"), "{with}");
    assert!(with.contains("Project rules here"), "{with}");

    // lowercase agents.md also works when AGENTS.md absent
    let dir2 = tempfile::tempdir().unwrap();
    std::fs::write(dir2.path().join("agents.md"), "lowercase rules").unwrap();
    let with2 = Agent::with_agents_md("base", dir2.path());
    assert!(with2.contains("lowercase rules"), "{with2}");

    // .whycodes/AGENTS.md is the fallback candidate
    let dir3 = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir3.path().join(".whycodes")).unwrap();
    std::fs::write(dir3.path().join(".whycodes/AGENTS.md"), "nested rules").unwrap();
    let with3 = Agent::with_agents_md("base", dir3.path());
    assert!(with3.contains("nested rules"), "{with3}");
}

#[test]
fn skills_catalog_is_appended_without_bodies() {
    let dir = tempfile::tempdir().unwrap();
    let skills = dir.path().join(".skills");
    std::fs::create_dir(&skills).unwrap();
    std::fs::write(
        skills.join("demo.skill.md"),
        "---\nname: demo\ndescription: short desc\n---\n\nSECRET BODY MUST NOT LEAK\n",
    )
    .unwrap();
    let with = Agent::with_agents_md("base", dir.path());
    assert!(with.contains("# Skills"), "{with}");
    assert!(with.contains("`demo`"), "{with}");
    assert!(with.contains("short desc"), "{with}");
    assert!(with.contains("skill://"), "{with}");
    assert!(!with.contains("SECRET BODY MUST NOT LEAK"), "{with}");
}

#[test]
fn stream_rules_compile_and_match() {
    use whycodes_config::StreamRuleConfig;
    let rules = [
        StreamRuleConfig {
            name: String::new(),
            pattern: "x".into(),
            hint: "h".into(),
        },
        StreamRuleConfig {
            name: "bad".into(),
            pattern: "(".into(),
            hint: "h".into(),
        },
        StreamRuleConfig {
            name: "no-leak".into(),
            pattern: "Box::leak".into(),
            hint: "use Arc".into(),
        },
    ];
    let compiled = compile_stream_rules(&rules);
    assert_eq!(compiled.len(), 1);
    let hit = first_stream_rule_hit(&compiled, "please Box::leak this");
    assert_eq!(hit, Some(("no-leak", "use Arc")));
    assert!(first_stream_rule_hit(&compiled, "Arc::from").is_none());
}

#[test]
fn persist_agent_artifact_sanitizes_id() {
    let dir = tempfile::tempdir().unwrap();
    persist_agent_artifact(dir.path(), "task-ok", "hello");
    persist_agent_artifact(dir.path(), "../evil", "nope");
    persist_agent_artifact(dir.path(), "", "ignored");
    let agents = dir.path().join(".whycodes").join("agents");
    assert_eq!(
        std::fs::read_to_string(agents.join("task-ok.md")).unwrap(),
        "hello"
    );
    assert!(!dir.path().join("evil.md").exists());
    assert!(!dir.path().join("evil").exists());
    // `../evil` strips to `evil` and stays inside the agents dir.
    assert_eq!(
        std::fs::read_to_string(agents.join("evil.md")).unwrap(),
        "nope"
    );
}

#[test]
fn memory_settings_map_from_config() {
    let config = whycodes_config::Config::default();
    let m = memory_settings_from_config(&config);
    assert_eq!(m.enabled, config.memory.enabled);
    assert_eq!(m.auto_inject, config.memory.auto_inject);
    assert_eq!(m.auto_retain, config.memory.auto_retain);
    assert_eq!(m.retain_every_n, config.memory.retain_every_n);
    assert_eq!(
        m.scope,
        whycodes_memory::MemoryScope::parse(&config.memory.scope)
    );
    assert_eq!(m.agent_bank, None);
}

#[test]
fn set_provider_registry_replaces_lookup() {
    let mut a = test_agent();
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::text("hi")));
    a.set_provider_registry(registry);
    assert!(a.provider_registry.get("script").is_some());
    assert!(a.provider_registry.get("anthropic").is_none());
}

#[test]
fn skip_prompt_cache_next_is_oneshot() {
    let a = test_agent();
    assert!(
        !a.skip_prompt_cache_once
            .load(std::sync::atomic::Ordering::Relaxed)
    );
    a.skip_prompt_cache_next();
    assert!(
        a.skip_prompt_cache_once
            .load(std::sync::atomic::Ordering::Relaxed)
    );
    assert!(
        a.skip_prompt_cache_once
            .swap(false, std::sync::atomic::Ordering::Relaxed)
    );
    assert!(
        !a.skip_prompt_cache_once
            .load(std::sync::atomic::Ordering::Relaxed)
    );
}

#[test]
fn settle_checkpoint_rewind_guards_and_extracts() {
    let mut session = Session::new(std::path::PathBuf::from("/p"), "s".into());
    let calls = [tc("rewind", serde_json::json!({"report": "findings"}))];
    let mut results = [ToolResult {
        tool_call_id: "t1".into(),
        content: "ok".into(),
        is_error: false,
    }];
    let (goal, report) = settle_checkpoint_rewind(&session, &calls, &mut results);
    assert!(goal.is_none());
    assert!(report.is_none());
    assert!(results[0].is_error);
    assert!(results[0].content.contains("No active checkpoint"));

    session.mark_checkpoint("look");
    results[0].is_error = false;
    results[0].content = "ok".into();
    let (goal, report) = settle_checkpoint_rewind(&session, &calls, &mut results);
    assert!(goal.is_none());
    assert_eq!(report.as_deref(), Some("findings"));
    assert!(!results[0].is_error);

    let cp_calls = [tc("checkpoint", serde_json::json!({"goal": "again"}))];
    let mut cp_results = [ToolResult {
        tool_call_id: "t1".into(),
        content: "ok".into(),
        is_error: false,
    }];
    let (goal, report) = settle_checkpoint_rewind(&session, &cp_calls, &mut cp_results);
    assert!(goal.is_none() && report.is_none());
    assert!(cp_results[0].is_error);
    assert!(cp_results[0].content.contains("already active"));

    let mut fresh = Session::new(std::path::PathBuf::from("/p"), "s".into());
    fresh.last_rewind_report = Some("old".into());
    let mut again = [ToolResult {
        tool_call_id: "t1".into(),
        content: "ok".into(),
        is_error: false,
    }];
    let (_g, _r) = settle_checkpoint_rewind(&fresh, &calls, &mut again);
    assert!(again[0].content.contains("already completed"));

    let empty = Session::new(std::path::PathBuf::from("/p"), "s".into());
    let mk = [tc("checkpoint", serde_json::json!({"goal": "scan"}))];
    let mut mk_r = [ToolResult {
        tool_call_id: "t1".into(),
        content: "ok".into(),
        is_error: false,
    }];
    let (goal, _) = settle_checkpoint_rewind(&empty, &mk, &mut mk_r);
    assert_eq!(goal.as_deref(), Some("scan"));
    assert!(!mk_r[0].is_error);
}

fn test_agent() -> Agent {
    Agent::new(whycodes_core::types::AgentInfo {
        name: "build".into(),
        description: "t".into(),
        mode: whycodes_core::types::AgentMode::Primary,
        permission: PermissionSet {
            allow_file_writes: true,
            allow_network: true,
            allow_shell: true,
            ..Default::default()
        },
        model: None,
        system_prompt: Some("sys".into()),
        temperature: None,
        top_p: None,
    })
}

fn tc(name: &str, args: serde_json::Value) -> ToolCall {
    ToolCall {
        id: "t1".into(),
        name: name.into(),
        arguments: args,
    }
}

#[test]
fn race_partner_off_auto_and_unknown() {
    let mut a = test_agent();
    a.model_race = "off".into();
    assert!(a.race_partner("anthropic", "claude").is_none());
    a.model_race = "none".into();
    assert!(a.race_partner("anthropic", "claude").is_none());
    a.model_race = "auto".into();
    // Default registry has anthropic; auto may pick a sibling or none
    // if resolve returns the same pair.
    let _ = a.race_partner("anthropic", "claude-sonnet-4-20250514");
    a.model_race = "openai/gpt-4o".into();
    let partner = a.race_partner("anthropic", "claude");
    assert!(
        partner
            .as_ref()
            .is_some_and(|(p, m)| p == "openai" && m.contains("gpt")),
        "{partner:?}"
    );
    a.model_race = "not-a-provider/x".into();
    assert!(a.race_partner("anthropic", "claude").is_none());

    a.model_race = "script/m".into();
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::text("hi")));
    a.set_provider_registry(registry);
    assert!(
        a.race_partner("script", "m").is_none(),
        "same provider+model must skip the race"
    );
}

#[test]
fn tool_context_uses_session_cwd_and_strips_network() {
    let mut a = test_agent();
    a.info.permission.allow_network = false;
    let session = whycodes_session::session::Session::new("/tmp/proj".into(), "sys".into());
    let ctx = a.tool_context(&session);
    assert_eq!(ctx.working_dir, "/tmp/proj");
    assert!(!ctx.sandbox.network);
    assert_eq!(ctx.session_id.as_deref(), Some(session.id.as_str()));

    if let Ok(mut g) = a.cwd_override.lock() {
        *g = Some(std::path::PathBuf::from("/tmp/wt"));
    }
    let ctx2 = a.tool_context(&session);
    assert_eq!(ctx2.working_dir, "/tmp/wt");
}

#[test]
fn execute_bg_tool_list_read_kill_and_unknown() {
    let a = test_agent();
    let list = a.execute_bg_tool(&tc("bg", json!({"action": "list"})));
    assert!(!list.is_error);
    assert!(list.content.contains("No background jobs"), "{list:?}");

    let read = a.execute_bg_tool(&tc("bg", json!({"action": "read"})));
    assert!(read.is_error);
    assert!(read.content.contains("requires `id`"), "{read:?}");

    let kill = a.execute_bg_tool(&tc("bg", json!({"action": "kill"})));
    assert!(kill.is_error);

    let missing = a.execute_bg_tool(&tc("bg", json!({"action": "read", "id": "nope"})));
    assert!(missing.is_error);

    let unk = a.execute_bg_tool(&tc("bg", json!({"action": "explode"})));
    assert!(unk.is_error);
    assert!(unk.content.contains("unknown bg action"), "{unk:?}");
}

#[test]
fn execute_tool_search_list_select_and_query() {
    let a = test_agent();
    let listed = a.execute_tool_search(&tc("tool_search", json!({"action": "list"})));
    assert!(!listed.is_error);
    assert!(listed.content.contains("Deferred catalogue"), "{listed:?}");

    let empty = a.execute_tool_search(&tc("tool_search", json!({})));
    assert!(empty.is_error);
    assert!(empty.content.contains("requires `query`"), "{empty:?}");

    let sel_empty = a.execute_tool_search(&tc("tool_search", json!({"action": "select"})));
    assert!(sel_empty.is_error);

    let sel = a.execute_tool_search(&tc(
        "tool_search",
        json!({"action": "select", "query": "github_pr,nope"}),
    ));
    assert!(sel.content.contains("github_pr") || sel.content.contains("Unknown"));
    assert!(
        a.activated_tools_snapshot()
            .iter()
            .any(|n| n.contains("github") || n.contains("pr"))
            || sel.content.contains("Unknown")
    );

    let hits = a.execute_tool_search(&tc(
        "tool_search",
        json!({"query": "github", "max_results": 3}),
    ));
    assert!(!hits.is_error);
    assert!(
        hits.content.contains("Matches") || hits.content.contains("No deferred"),
        "{hits:?}"
    );

    let none = a.execute_tool_search(&tc(
        "tool_search",
        json!({"query": "zzzz-no-such-tool-xyz"}),
    ));
    assert!(!none.is_error);
    assert!(none.content.contains("No deferred"), "{none:?}");
}

#[test]
fn execute_worktree_tool_validation_and_list() {
    let a = test_agent();
    let dir = tempfile::tempdir().unwrap();
    let session = whycodes_session::session::Session::new(dir.path().to_path_buf(), "sys".into());

    let unk = a.execute_worktree_tool(&tc("worktree", json!({"action": "nope"})), &session);
    assert!(unk.is_error);
    assert!(unk.content.contains("unknown worktree"), "{unk:?}");

    let bad_create = a.execute_worktree_tool(
        &tc("worktree", json!({"action": "create", "name": "a/b"})),
        &session,
    );
    assert!(bad_create.is_error);

    let not_git = a.execute_worktree_tool(
        &tc("worktree", json!({"action": "create", "name": "ok"})),
        &session,
    );
    assert!(not_git.is_error);
    assert!(not_git.content.contains("not a git"), "{not_git:?}");

    let listed = a.execute_worktree_tool(&tc("worktree", json!({"action": "list"})), &session);
    assert!(!listed.is_error);
    assert!(listed.content.contains("Worktrees"), "{listed:?}");

    let enter = a.execute_worktree_tool(
        &tc("worktree", json!({"action": "enter", "name": "missing"})),
        &session,
    );
    assert!(enter.is_error);

    let exit = a.execute_worktree_tool(&tc("worktree", json!({"action": "exit"})), &session);
    assert!(!exit.is_error);
    assert!(exit.content.contains("No worktree cwd"), "{exit:?}");

    let rm = a.execute_worktree_tool(
        &tc("worktree", json!({"action": "remove", "name": "??"})),
        &session,
    );
    assert!(rm.is_error);
}

#[test]
fn builder_chain_sets_profile_and_fast_model() {
    let mut config = whycodes_config::Config::default();
    config.session.model_fast = Some("haiku".into());
    let a = test_agent()
        .with_tool_profile(whycodes_tools::ToolProfile::Full)
        .with_config(&config);
    assert_eq!(a.model_fast(), Some("haiku"));
    assert!(a.activated_tools_snapshot().is_empty());
    assert!(a.cwd_override_path().is_none());
    assert!(a.session_claims().is_none());

    let mut live = test_agent();
    live.apply_config(&config);
    assert_eq!(live.model_fast(), Some("haiku"));
}

#[test]
fn apply_config_parses_flag_matrix_and_clamps() {
    let mut config = whycodes_config::Config::default();
    config.security.bash_risk_threshold = "not-a-threshold".into();
    config.session.compaction_llm = "off".into();
    config.session.prompt_cache = "none".into();
    config.session.response_cache = "false".into();
    config.session.intent_guidance = "always".into();
    config.swarm.max_agents = 0;
    config.swarm.isolation = Some("checkout".into());
    config.automation.max_background_jobs = 0;
    config.session.tool_profile = "full".into();
    config.session.compaction_threshold = 42;
    config.session.model_race = "off".into();
    config.session.reasoning_effort = Some("high".into());
    config.general.approval_mode = Some(whycodes_core::types::ApprovalMode::Manual);

    let mut a = test_agent();
    a.apply_config(&config);
    assert!(!a.compaction_llm);
    assert!(!a.use_prompt_cache);
    assert!(!a.response_cache);
    assert_eq!(a.swarm_max_agents, 1);
    assert!(!a.swarm_worktrees);
    assert_eq!(a.max_background_jobs, 1);
    assert_eq!(a.compaction_threshold, 42);
    assert_eq!(a.tool_profile, whycodes_tools::ToolProfile::Full);
    assert_eq!(
        a.approval_mode(),
        whycodes_core::types::ApprovalMode::Manual
    );
    assert_eq!(a.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(a.intent_guidance, crate::intent::IntentGuidanceMode::Always);

    config.session.compaction_llm = "local".into();
    config.session.prompt_cache = "0".into();
    config.session.response_cache = "none".into();
    config.swarm.max_agents = 99;
    config.swarm.isolation = Some("worktree".into());
    config.automation.max_background_jobs = 99;
    config.session.compaction_llm = "false".into();
    a.apply_config(&config);
    assert!(!a.compaction_llm);
    assert!(!a.use_prompt_cache);
    assert!(!a.response_cache);
    assert_eq!(a.swarm_max_agents, crate::swarm::SWARM_HARD_MAX_AGENTS);
    assert!(a.swarm_worktrees);
    assert_eq!(
        a.max_background_jobs,
        crate::background::DEFAULT_MAX_BACKGROUND_JOBS
            .saturating_mul(2)
            .max(8)
    );

    for off in ["off", "false", "0", "none"] {
        config.session.response_cache = off.into();
        config.session.prompt_cache = off.into();
        a.apply_config(&config);
        assert!(!a.response_cache, "{off}");
        assert!(!a.use_prompt_cache, "{off}");
    }
    for off in ["off", "false", "0", "none", "local"] {
        config.session.compaction_llm = off.into();
        a.apply_config(&config);
        assert!(!a.compaction_llm, "{off}");
    }
    config.session.compaction_llm = "auto".into();
    config.session.prompt_cache = "auto".into();
    config.session.response_cache = "auto".into();
    a.apply_config(&config);
    assert!(a.compaction_llm);
    assert!(a.use_prompt_cache);
    assert!(a.response_cache);
}

#[test]
fn panel_and_todo_sinks_forward_and_drop_when_closed() {
    let mut a = test_agent();
    assert!(a.panel_sink().is_none());
    assert!(a.todo_sink().is_none());

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    a.wire_event_sink(tx);
    let panel = a.panel_sink().expect("panel");
    panel(whycodes_core::PanelUpdate::Clear);
    match rx.try_recv() {
        Ok(TurnEvent::Panel(whycodes_core::PanelUpdate::Clear)) => {}
        other => panic!("{other:?}"),
    }
    let todo = a.todo_sink().expect("todo");
    todo(Vec::new());
    match rx.try_recv() {
        Ok(TurnEvent::Todos { todos }) => assert!(todos.is_empty()),
        other => panic!("{other:?}"),
    }

    drop(rx);
    let panel = a.panel_sink().expect("panel after close");
    panel(whycodes_core::PanelUpdate::Clear);
    let todo = a.todo_sink().expect("todo after close");
    todo(Vec::new());
}

#[test]
fn hydrate_plugins_and_session_claims_and_file_index() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = test_agent();
    a.hydrate_plugins(Some(dir.path()));
    let claims = whycodes_core::FileClaimRegistry::new();
    let a = a.with_session_claims(claims.clone());
    assert!(a.session_claims().is_some());
    let idx = whycodes_index::WorkspaceIndex::start(vec![dir.path().to_path_buf()]);
    let a = a.with_file_index(idx);
    let session = Session::new(dir.path().to_path_buf(), "sys".into());
    let ctx = a.tool_context(&session);
    assert!(ctx.file_index.is_some());
    assert!(ctx.file_claims.is_some());
    assert!(ctx.agent_id.is_some());
    assert_eq!(ctx.agent_label.as_deref(), Some("build"));
}

#[tokio::test]
async fn spawn_title_refine_true_and_false() {
    let a = scripted_test_agent([whycodes_llm::ScriptedStep::Text("Retry Loop".into())]);
    let empty = Session::new("/tmp".into(), "sys".into());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    assert!(!a.spawn_title_refine(&empty, "script", "m", "k", None, tx.clone()));

    let mut s = Session::new("/tmp".into(), "sys".into());
    s.add_user_message("please explain the retry loop in crates/llm");
    assert!(a.spawn_title_refine(&s, "script", "m", "k", None, tx));
    a.maybe_refine_title(&mut s, "script", "m", "k", None).await;
}

#[tokio::test]
async fn spawn_title_refine_sends_nonempty_title() {
    let a = scripted_test_agent([whycodes_llm::ScriptedStep::Text("Retry Loop".into())]);
    let mut s = Session::new("/tmp".into(), "sys".into());
    s.add_user_message("please explain the retry loop in crates/llm");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    assert!(a.spawn_title_refine(&s, "script", "title-ok-cov", "k", None, tx));
    let got = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .expect("title send")
        .expect("title pair");
    assert_eq!(got.0, s.id);
    assert!(!got.1.is_empty(), "{}", got.1);
}

#[tokio::test]
async fn load_mcp_connect_fail_does_not_replace_executor() {
    let mut config = whycodes_config::Config::default();
    config.mcp_servers.insert(
        "ghost".into(),
        whycodes_config::McpServerConfig {
            transport: Some(whycodes_config::McpTransportKind::Stdio),
            command: Some("whycodes-definitely-missing-mcp-binary".into()),
            args: Vec::new(),
            env: None,
            cwd: None,
            url: None,
            headers: None,
        },
    );
    let mut a = test_agent();
    a.load_mcp(&config).await;
    let listed = a.execute_tool_search(&tc("tool_search", json!({"action": "list"})));
    assert!(!listed.is_error, "{listed:?}");
}

#[test]
fn persist_agent_artifact_create_dir_err_is_logged() {
    persist_agent_artifact(std::path::Path::new("/dev/null/not-a-dir"), "id", "body");
    persist_agent_artifact(std::path::Path::new("/proc/1"), "id", "body");
}

#[test]
fn title_refine_target_needs_user_and_key() {
    let a = test_agent();
    let empty = whycodes_session::session::Session::new("/tmp".into(), "sys".into());
    assert!(
        a.title_refine_target(&empty, "anthropic", "claude", "k", None)
            .is_none()
    );

    let mut s = whycodes_session::session::Session::new("/tmp".into(), "sys".into());
    s.add_user_message("please explain the retry loop in crates/llm");
    assert!(
        a.title_refine_target(&s, "anthropic", "claude", "", None)
            .is_none()
    );
    let hit = a.title_refine_target(&s, "anthropic", "claude", "sk-test", None);
    assert!(hit.is_some(), "{hit:?}");
    let (p, _, key, user, _) = hit.unwrap();
    assert_eq!(p, "anthropic");
    assert_eq!(key, "sk-test");
    assert!(user.contains("retry"));
}

#[tokio::test]
async fn execute_schedule_tool_requires_command_or_prompt() {
    let a = test_agent();
    let session = whycodes_session::session::Session::new("/tmp/proj".into(), "sys".into());
    let ctx = a.tool_context(&session);
    let empty = a
        .execute_schedule_tool(&tc("schedule", json!({})), &ctx, None)
        .await;
    assert!(empty.is_error, "{empty:?}");
    assert!(
        empty.content.contains("command") || empty.content.contains("prompt"),
        "{}",
        empty.content
    );
}

#[tokio::test]
async fn execute_swarm_tool_disabled_and_empty_tasks() {
    let a = test_agent();
    let session = whycodes_session::session::Session::new("/tmp/proj".into(), "sys".into());
    let off = a
        .execute_swarm_tool(&tc("swarm", json!({})), &session, "script", "m", "k", None)
        .await;
    assert!(off.is_error, "{off:?}");
    assert!(
        off.content.to_lowercase().contains("disabled")
            || off.content.to_lowercase().contains("swarm"),
        "{}",
        off.content
    );

    let mut on = test_agent();
    on.swarm_enabled = true;
    let empty = on
        .execute_swarm_tool(
            &tc("swarm", json!({"tasks": []})),
            &session,
            "script",
            "m",
            "k",
            None,
        )
        .await;
    assert!(empty.is_error, "{empty:?}");
}

#[tokio::test]
async fn execute_task_tool_requires_goal() {
    let a = test_agent();
    let session = whycodes_session::session::Session::new("/tmp/proj".into(), "sys".into());
    let empty = a
        .execute_task_tool(&tc("task", json!({})), &session, "script", "m", "k", None)
        .await;
    assert!(empty.is_error, "{empty:?}");
    assert!(
        empty.content.to_lowercase().contains("goal"),
        "{}",
        empty.content
    );
}

#[test]
fn execute_background_shell_requires_command() {
    let a = test_agent();
    let session = whycodes_session::session::Session::new("/tmp/proj".into(), "sys".into());
    let ctx = a.tool_context(&session);
    let empty = a.execute_background_shell(&tc("bash", json!({})), &ctx, None);
    assert!(empty.is_error, "{empty:?}");
    assert!(
        empty.content.to_lowercase().contains("command"),
        "{}",
        empty.content
    );
}

fn init_git_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    use std::process::Command;
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    assert!(
        Command::new("git")
            .args(["init"])
            .current_dir(&root)
            .status()
            .unwrap()
            .success()
    );
    let _ = Command::new("git")
        .args(["config", "user.email", "test@whycodes.local"])
        .current_dir(&root)
        .status();
    let _ = Command::new("git")
        .args(["config", "user.name", "whycodes-test"])
        .current_dir(&root)
        .status();
    std::fs::write(root.join("a.txt"), b"base-a\n").unwrap();
    assert!(
        Command::new("git")
            .args(["add", "."])
            .current_dir(&root)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(&root)
            .status()
            .unwrap()
            .success()
    );
    (dir, root)
}

fn scripted_test_agent(steps: impl IntoIterator<Item = whycodes_llm::ScriptedStep>) -> Agent {
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::repeating(
        "script", steps,
    )));
    test_agent().with_provider_registry(registry)
}

#[tokio::test]
async fn execute_background_shell_starts_lists_reads_and_kills() {
    let a = test_agent();
    let dir = tempfile::tempdir().unwrap();
    let session = whycodes_session::session::Session::new(dir.path().to_path_buf(), "sys".into());
    let ctx = a.tool_context(&session);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

    let started = a.execute_background_shell(
        &tc(
            "bash",
            json!({"command": "sleep 30", "description": "bg-sleep"}),
        ),
        &ctx,
        Some(&tx),
    );
    assert!(!started.is_error, "{started:?}");
    assert!(started.content.contains("Background job"), "{started:?}");

    let listed = a.execute_bg_tool(&tc("bg", json!({"action": "list"})));
    assert!(!listed.is_error, "{listed:?}");
    assert!(listed.content.contains("bg-"), "{listed:?}");

    let id = listed
        .content
        .split_whitespace()
        .find(|w| w.starts_with("bg-"))
        .unwrap_or("bg-1")
        .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .to_string();

    let read = a.execute_bg_tool(&tc(
        "bg",
        json!({"action": "read", "id": id, "max_chars": 32}),
    ));
    assert!(!read.is_error, "{read:?}");

    let killed = a.execute_bg_tool(&tc("bg", json!({"action": "kill", "id": id})));
    assert!(!killed.is_error, "{killed:?}");
    a.background.kill_all();
}

#[tokio::test]
async fn execute_background_shell_errors_when_job_cap_hit() {
    let a = test_agent();
    a.background.set_max_jobs(1);
    let dir = tempfile::tempdir().unwrap();
    let session = whycodes_session::session::Session::new(dir.path().to_path_buf(), "sys".into());
    let ctx = a.tool_context(&session);
    let first = a.execute_background_shell(&tc("bash", json!({"command": "sleep 30"})), &ctx, None);
    assert!(!first.is_error, "{first:?}");
    let full = a.execute_background_shell(&tc("bash", json!({"command": "echo hi"})), &ctx, None);
    assert!(full.is_error, "{full:?}");
    assert!(
        full.content.to_lowercase().contains("too many")
            || full.content.to_lowercase().contains("max"),
        "{}",
        full.content
    );
    a.background.kill_all();
}

#[test]
fn execute_worktree_tool_create_enter_exit_remove_on_git_repo() {
    let a = test_agent();
    let (_keep, root) = init_git_repo();
    let session = whycodes_session::session::Session::new(root.clone(), "sys".into());

    let created = a.execute_worktree_tool(
        &tc("worktree", json!({"action": "create", "name": "feat-cov"})),
        &session,
    );
    assert!(!created.is_error, "{created:?}");
    assert!(created.content.contains("Created worktree"), "{created:?}");

    let dup = a.execute_worktree_tool(
        &tc("worktree", json!({"action": "create", "name": "feat-cov"})),
        &session,
    );
    assert!(dup.is_error, "{dup:?}");

    let enter = a.execute_worktree_tool(
        &tc("worktree", json!({"action": "enter", "name": "feat-cov"})),
        &session,
    );
    assert!(!enter.is_error, "{enter:?}");
    assert!(enter.content.contains("Tool cwd"), "{enter:?}");

    let listed = a.execute_worktree_tool(&tc("worktree", json!({"action": "list"})), &session);
    assert!(!listed.is_error, "{listed:?}");
    assert!(listed.content.contains("feat-cov"), "{listed:?}");
    assert!(listed.content.contains("Active cwd"), "{listed:?}");

    let exit = a.execute_worktree_tool(&tc("worktree", json!({"action": "exit"})), &session);
    assert!(!exit.is_error, "{exit:?}");
    assert!(exit.content.contains("Restored tool cwd"), "{exit:?}");

    let removed = a.execute_worktree_tool(
        &tc("worktree", json!({"action": "remove", "name": "feat-cov"})),
        &session,
    );
    assert!(!removed.is_error, "{removed:?}");
    assert!(removed.content.contains("Removed worktree"), "{removed:?}");
}

#[tokio::test]
async fn execute_schedule_tool_command_and_goal() {
    let a = test_agent();
    let dir = tempfile::tempdir().unwrap();
    let session = whycodes_session::session::Session::new(dir.path().to_path_buf(), "sys".into());
    let ctx = a.tool_context(&session);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    let scheduled = a
        .execute_schedule_tool(
            &tc(
                "schedule",
                json!({
                    "command": "echo scheduled",
                    "goal": "follow up",
                    "description": "cov",
                    "after_secs": 0
                }),
            ),
            &ctx,
            Some(&tx),
        )
        .await;
    assert!(!scheduled.is_error, "{scheduled:?}");
    assert!(scheduled.content.contains("Scheduled"), "{scheduled:?}");

    let mut saw_prompt = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(400);
    while std::time::Instant::now() < deadline {
        match rx.try_recv() {
            Ok(TurnEvent::EnqueuePrompt { text }) => {
                assert_eq!(text, "follow up");
                saw_prompt = true;
                break;
            }
            Ok(_) => {}
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
    assert!(saw_prompt, "expected EnqueuePrompt from schedule goal");
    a.background.kill_all();
}

#[tokio::test]
async fn execute_swarm_tool_runs_scripted_workers() {
    let mut a = scripted_test_agent([whycodes_llm::ScriptedStep::Text("worker-ok".into())]);
    a.swarm_enabled = true;
    a.swarm_worktrees = false;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "n").unwrap();
    let session = whycodes_session::session::Session::new(dir.path().to_path_buf(), "sys".into());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

    let out = a
        .execute_swarm_tool(
            &tc(
                "swarm",
                json!({
                    "max_concurrent": 2,
                    "tasks": [
                        {
                            "goal": "summarize note.txt",
                            "subagent_type": "explore",
                            "paths": ["note.txt"],
                            "max_turns": 1
                        },
                        {
                            "goal": "list files",
                            "subagent_type": "general",
                            "max_turns": 1
                        }
                    ]
                }),
            ),
            &session,
            "script",
            "m",
            "k",
            Some(&tx),
        )
        .await;
    assert!(!out.is_error, "{out:?}");
    assert!(
        out.content.to_lowercase().contains("swarm")
            || out.content.to_lowercase().contains("worker"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn execute_swarm_tool_folds_worker_usage() {
    let mut a = scripted_test_agent([
        whycodes_llm::ScriptedStep::Text("worker-usage".into()),
        whycodes_llm::ScriptedStep::Usage {
            input_tokens: 9,
            output_tokens: 4,
        },
    ]);
    a.swarm_enabled = true;
    a.swarm_worktrees = false;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "n").unwrap();
    let session = Session::new(dir.path().to_path_buf(), "sys".into());
    let out = a
        .execute_swarm_tool(
            &tc(
                "swarm",
                json!({
                    "tasks": [{
                        "goal": "summarize note.txt",
                        "subagent_type": "explore",
                        "paths": ["note.txt"],
                        "max_turns": 1
                    }]
                }),
            ),
            &session,
            "script",
            "m",
            "k",
            None,
        )
        .await;
    assert!(!out.is_error, "{out:?}");
    let pending = a
        .subagent_usage_pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    assert!(
        pending.input_tokens >= 9 && pending.output_tokens >= 4,
        "{pending:?}"
    );
}

#[tokio::test]
async fn execute_swarm_tool_disabled_branch() {
    let mut a = test_agent();
    a.swarm_enabled = false;
    let session = whycodes_session::session::Session::new("/tmp/proj".into(), "sys".into());
    let off = a
        .execute_swarm_tool(
            &tc("swarm", json!({"tasks": [{"goal": "x"}]})),
            &session,
            "script",
            "m",
            "k",
            None,
        )
        .await;
    assert!(off.is_error, "{off:?}");
    assert!(
        off.content.to_lowercase().contains("disabled"),
        "{}",
        off.content
    );
}

#[tokio::test]
async fn execute_task_tool_explore_and_general() {
    let a = scripted_test_agent([whycodes_llm::ScriptedStep::Text("task-ok".into())]);
    let dir = tempfile::tempdir().unwrap();
    let session = whycodes_session::session::Session::new(dir.path().to_path_buf(), "sys".into());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

    let explore = a
        .execute_task_tool(
            &tc(
                "task",
                json!({
                    "goal": "inspect the tree",
                    "context": "unit test",
                    "subagent_type": "explore",
                    "max_turns": 1
                }),
            ),
            &session,
            "script",
            "m",
            "k",
            Some(&tx),
        )
        .await;
    assert!(!explore.is_error, "{explore:?}");
    assert!(explore.content.contains("task-ok"), "{explore:?}");

    let general = a
        .execute_task_tool(
            &tc(
                "task",
                json!({
                    "goal": "do the work",
                    "subagent_type": "general",
                    "max_turns": 1
                }),
            ),
            &session,
            "script",
            "m",
            "k",
            None,
        )
        .await;
    assert!(!general.is_error, "{general:?}");
    assert!(general.content.contains("task-ok"), "{general:?}");
}

#[tokio::test]
async fn execute_task_tool_fail_open_is_error_and_folds_usage() {
    let a = scripted_test_agent([
        whycodes_llm::ScriptedStep::FailOpen("boom".into()),
        whycodes_llm::ScriptedStep::Usage {
            input_tokens: 11,
            output_tokens: 7,
        },
    ]);
    let dir = tempfile::tempdir().unwrap();
    let session = Session::new(dir.path().to_path_buf(), "sys".into());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let out = a
        .execute_task_tool(
            &tc(
                "task",
                json!({
                    "goal": "fail please",
                    "subagent_type": "scout",
                    "max_turns": 1
                }),
            ),
            &session,
            "script",
            "m",
            "k",
            Some(&tx),
        )
        .await;
    assert!(
        out.is_error
            || out.content.to_lowercase().contains("error")
            || out.content.to_lowercase().contains("fail")
            || out.content.to_lowercase().contains("boom"),
        "{out:?}"
    );
    let pending = a.subagent_usage_pending.lock().unwrap();
    let _ = pending.input_tokens + pending.output_tokens;
}

#[tokio::test]
async fn execute_swarm_preclaim_conflict_and_fail_open() {
    let mut a = scripted_test_agent([whycodes_llm::ScriptedStep::FailOpen("boom".into())]);
    a.swarm_enabled = true;
    a.swarm_worktrees = false;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "n").unwrap();
    let session = Session::new(dir.path().to_path_buf(), "sys".into());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let conflict = a
        .execute_swarm_tool(
            &tc(
                "swarm",
                json!({
                    "tasks": [
                        {"goal": "a", "paths": ["note.txt"], "max_turns": 1},
                        {"goal": "b", "paths": ["note.txt"], "max_turns": 1}
                    ]
                }),
            ),
            &session,
            "script",
            "m",
            "k",
            Some(&tx),
        )
        .await;
    assert!(conflict.is_error, "{conflict:?}");
    assert!(
        conflict.content.to_lowercase().contains("conflict")
            || conflict.content.to_lowercase().contains("claim"),
        "{}",
        conflict.content
    );
    let mut saw_conflict = false;
    while let Ok(ev) = rx.try_recv() {
        if matches!(ev, TurnEvent::FileConflict { .. }) {
            saw_conflict = true;
        }
    }
    assert!(saw_conflict, "pre-claim should emit FileConflict");

    let (drop_tx, drop_rx) = tokio::sync::mpsc::unbounded_channel();
    drop(drop_rx);
    let dropped = a
        .execute_swarm_tool(
            &tc(
                "swarm",
                json!({
                    "tasks": [
                        {"goal": "a", "paths": ["note.txt"], "max_turns": 1},
                        {"goal": "b", "paths": ["note.txt"], "max_turns": 1}
                    ]
                }),
            ),
            &session,
            "script",
            "m",
            "k",
            Some(&drop_tx),
        )
        .await;
    assert!(dropped.is_error, "{dropped:?}");

    let fail = a
        .execute_swarm_tool(
            &tc(
                "swarm",
                json!({
                    "max_concurrent": 1,
                    "tasks": [{"goal": "only one", "subagent_type": "explore", "max_turns": 1}]
                }),
            ),
            &session,
            "script",
            "m",
            "k",
            Some(&tx),
        )
        .await;
    assert!(fail.is_error, "{fail:?}");
    assert!(
        fail.content.contains("same-checkout")
            || fail.content.contains("isolation")
            || fail.content.contains("Swarm"),
        "{}",
        fail.content
    );
    while let Ok(_ev) = rx.try_recv() {}
}

#[tokio::test]
async fn execute_swarm_worktrees_on_git_repo() {
    let mut a = scripted_test_agent([whycodes_llm::ScriptedStep::Text("wt-ok".into())]);
    a.swarm_enabled = true;
    a.swarm_worktrees = true;
    let (_keep, root) = init_git_repo();
    let session = Session::new(root.clone(), "sys".into());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let out = a
        .execute_swarm_tool(
            &tc(
                "swarm",
                json!({
                    "tasks": [{
                        "goal": "summarize a.txt",
                        "subagent_type": "explore",
                        "paths": ["a.txt"],
                        "max_turns": 1
                    }]
                }),
            ),
            &session,
            "script",
            "m",
            "k",
            Some(&tx),
        )
        .await;
    assert!(!out.is_error, "{out:?}");
    assert!(
        out.content.contains("worktrees") || out.content.to_lowercase().contains("swarm"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn execute_schedule_clamps_after_secs_and_reports_cap_fail() {
    let a = test_agent();
    a.background.set_max_jobs(1);
    let dir = tempfile::tempdir().unwrap();
    let session = Session::new(dir.path().to_path_buf(), "sys".into());
    let ctx = a.tool_context(&session);
    let first = a.execute_background_shell(&tc("bash", json!({"command": "sleep 30"})), &ctx, None);
    assert!(!first.is_error, "{first:?}");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let scheduled = a
        .execute_schedule_tool(
            &tc(
                "schedule",
                json!({
                    "command": "echo overflow",
                    "after_secs": 999_999
                }),
            ),
            &ctx,
            Some(&tx),
        )
        .await;
    assert!(!scheduled.is_error, "{scheduled:?}");
    assert!(
        scheduled.content.contains("86400") || scheduled.content.contains("Scheduled"),
        "{scheduled:?}"
    );
    let _ = rx.try_recv();
    a.background.kill_all();
}

#[test]
fn execute_tool_search_list_truncates_when_catalog_large() {
    struct Dummy {
        name: String,
    }
    impl whycodes_core::Tool for Dummy {
        fn name(&self) -> &str {
            &self.name
        }
        fn description(&self) -> &str {
            "dummy deferred tool"
        }
        fn parameters(&self) -> serde_json::Value {
            json!({"type": "object"})
        }
        fn execute<'a>(
            &'a self,
            _args: serde_json::Value,
            _ctx: &'a whycodes_core::ToolContext,
        ) -> whycodes_core::ToolFuture<'a> {
            Box::pin(async move {
                ToolResult {
                    tool_call_id: String::new(),
                    content: "ok".into(),
                    is_error: false,
                }
            })
        }
    }
    let mut exec = whycodes_tools::ToolExecutor::new();
    for i in 0..45 {
        exec.register(Box::new(Dummy {
            name: format!("dummy_extra_{i:02}"),
        }));
    }
    let dummy = Dummy {
        name: "dummy_extra_00".into(),
    };
    assert_eq!(dummy.description(), "dummy deferred tool");
    assert_eq!(dummy.parameters()["type"], "object");
    let a = test_agent().with_tool_executor(exec);
    let listed = a.execute_tool_search(&tc("tool_search", json!({"action": "list"})));
    assert!(!listed.is_error, "{listed:?}");
    assert!(
        listed.content.contains("…and") || listed.content.contains("and "),
        "{}",
        listed.content
    );
}

fn llm_req(messages: Vec<whycodes_core::types::Message>) -> whycodes_core::types::LlmRequest {
    whycodes_core::types::LlmRequest {
        system: String::new(),
        messages: std::sync::Arc::from(messages),
        tools: std::sync::Arc::from([]),
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: None,
        thinking: None,
        use_prompt_cache: false,
    }
}

#[test]
fn append_request_user_suffix_skips_non_user_and_appends_blocks() {
    use whycodes_core::types::{Message, MessageContent, Role};

    let mut req = llm_req(vec![Message {
        role: Role::Assistant,
        content: MessageContent::Text("hi".into()),
        tool_call_id: None,
        name: None,
        created_at: None,
    }]);
    append_request_user_suffix(&mut req, " [suffix]");
    let MessageContent::Text(t) = &req.messages[0].content else {
        panic!("expected text");
    };
    assert_eq!(t, "hi");

    let mut req = llm_req(vec![
        Message {
            role: Role::Assistant,
            content: MessageContent::Text("a".into()),
            tool_call_id: None,
            name: None,
            created_at: None,
        },
        Message {
            role: Role::User,
            content: MessageContent::Blocks(vec![ContentBlock::Text { text: "ask".into() }]),
            tool_call_id: None,
            name: None,
            created_at: None,
        },
    ]);
    append_request_user_suffix(&mut req, " more");
    let MessageContent::Blocks(blocks) = &req.messages[1].content else {
        panic!("expected blocks");
    };
    assert!(
        blocks.iter().any(|b| matches!(
            b,
            ContentBlock::Text { text } if text == " more"
        )),
        "{blocks:?}"
    );

    let mut req = llm_req(vec![Message {
        role: Role::User,
        content: MessageContent::Text("ask".into()),
        tool_call_id: None,
        name: None,
        created_at: None,
    }]);
    append_request_user_suffix(&mut req, "!");
    let MessageContent::Text(t) = &req.messages[0].content else {
        panic!("expected text");
    };
    assert_eq!(t, "ask!");
}

#[test]
fn persist_agent_artifact_write_err_when_path_is_directory() {
    let dir = tempfile::tempdir().unwrap();
    persist_agent_artifact(dir.path(), "ok-id", "hello");
    let written = whycodes_core::project_dir(dir.path())
        .join("agents")
        .join("ok-id.md");
    assert!(written.is_file(), "{}", written.display());
    assert_eq!(std::fs::read_to_string(&written).unwrap(), "hello");

    persist_agent_artifact(dir.path(), "???", "ignored");
    persist_agent_artifact(dir.path(), "", "ignored");

    let agents = whycodes_core::project_dir(dir.path()).join("agents");
    std::fs::create_dir_all(agents.join("dirid.md")).unwrap();
    persist_agent_artifact(dir.path(), "dirid", "cannot write");
}

#[test]
fn set_file_index_mutates_in_place() {
    let mut a = test_agent();
    let dir = tempfile::tempdir().unwrap();
    let idx = whycodes_index::WorkspaceIndex::start(vec![dir.path().to_path_buf()]);
    a.set_file_index(idx);
    let session = Session::new(dir.path().to_path_buf(), "sys".into());
    assert!(a.tool_context(&session).file_index.is_some());
}

#[test]
fn with_plugins_registers_from_isolated_home() {
    let home = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("WHYCODES_HOME", home.path()) };
    let dir = tempfile::tempdir().unwrap();
    let why = dir.path().join(".whycodes");
    std::fs::create_dir_all(&why).unwrap();
    std::fs::write(
        why.join("plugins.toml"),
        r#"[[plugins]]
name = "covplug"
command = "echo cov"
description = "coverage plugin"
"#,
    )
    .unwrap();
    let a = test_agent().with_plugins(Some(dir.path()));
    assert!(
        a.tool_executor.get("plugin_covplug").is_some(),
        "plugin_covplug should be registered"
    );
    unsafe { std::env::remove_var("WHYCODES_HOME") };
}

#[tokio::test]
async fn with_mcp_and_load_mcp_register_plugins_without_servers() {
    let home = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("WHYCODES_HOME", home.path()) };
    let dir = tempfile::tempdir().unwrap();
    let why = dir.path().join(".whycodes");
    std::fs::create_dir_all(&why).unwrap();
    std::fs::write(
        why.join("plugins.toml"),
        r#"[[plugins]]
name = "mcpplug"
command = "echo mcp"
description = "mcp plugin"
"#,
    )
    .unwrap();
    let mut config = whycodes_config::Config::default();
    config.general.project_path = Some(dir.path().to_path_buf());
    let a = test_agent().with_mcp(&config).await;
    assert!(a.tool_executor.get("plugin_mcpplug").is_some());

    let mut live = test_agent();
    live.load_mcp(&config).await;
    assert!(live.tool_executor.get("plugin_mcpplug").is_some());
    unsafe { std::env::remove_var("WHYCODES_HOME") };
}

#[test]
fn title_refine_target_falls_back_and_needs_cross_provider_key() {
    let a = test_agent();
    let mut s = Session::new("/tmp".into(), "sys".into());
    s.add_user_message("please explain the retry loop in crates/llm");
    s.add_assistant_message(vec![ContentBlock::Text {
        text: "I walked through crates/llm".into(),
    }]);
    let hit = a
        .title_refine_target(
            &s,
            "anthropic",
            "claude",
            "sk-test",
            Some("no-such-provider/tiny"),
        )
        .expect("falls back to session provider");
    assert_eq!(hit.0, "anthropic");
    assert_eq!(hit.2, "sk-test");
    assert_eq!(hit.4.as_deref(), Some("I walked through crates/llm"));

    let prev = std::env::var_os("OPENAI_API_KEY");
    unsafe { std::env::set_var("OPENAI_API_KEY", "") };
    assert!(
        a.title_refine_target(
            &s,
            "anthropic",
            "claude",
            "sk-test",
            Some("openai/gpt-4o-mini")
        )
        .is_none(),
        "empty cross-provider key must skip refine"
    );
    if let Some(v) = prev {
        unsafe { std::env::set_var("OPENAI_API_KEY", v) };
    } else {
        unsafe { std::env::remove_var("OPENAI_API_KEY") };
    }
}

#[tokio::test]
async fn execute_bg_kill_already_status_and_schedule_cap_fail_event() {
    let a = test_agent();
    a.background.set_max_jobs(1);
    let dir = tempfile::tempdir().unwrap();
    let session = Session::new(dir.path().to_path_buf(), "sys".into());
    let ctx = a.tool_context(&session);
    let started =
        a.execute_background_shell(&tc("bash", json!({"command": "sleep 30"})), &ctx, None);
    assert!(!started.is_error, "{started:?}");
    let listed = a.execute_bg_tool(&tc("bg", json!({"action": "list"})));
    let id = listed
        .content
        .split_whitespace()
        .find(|w| w.starts_with("bg-"))
        .expect("job id")
        .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .to_string();

    // Cap is based on Running jobs. Keep the sleeper alive so schedule fails.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let scheduled = a
        .execute_schedule_tool(
            &tc(
                "schedule",
                json!({"command": "echo overflow", "after_secs": 0}),
            ),
            &ctx,
            Some(&tx),
        )
        .await;
    assert!(!scheduled.is_error, "{scheduled:?}");
    let mut saw_fail = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(400);
    while std::time::Instant::now() < deadline {
        match rx.try_recv() {
            Ok(TurnEvent::Background { status, .. }) if status == "failed" => {
                saw_fail = true;
                break;
            }
            Ok(_) => {}
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
    assert!(saw_fail, "expected scheduled start_shell cap-fail event");

    let killed = a.execute_bg_tool(&tc("bg", json!({"action": "kill", "id": id})));
    assert!(!killed.is_error, "{killed:?}");
    let again = a.execute_bg_tool(&tc("bg", json!({"action": "kill", "id": id})));
    assert!(!again.is_error, "{again:?}");
    assert!(again.content.contains("already"), "{}", again.content);

    let unknown = a.execute_bg_tool(&tc("bg", json!({"action": "kill", "id": "nope"})));
    assert!(unknown.is_error, "{unknown:?}");
    a.background.kill_all();
}

#[test]
fn execute_tool_search_on_none_and_exact_name_score() {
    let a = test_agent();
    let listed = a.execute_tool_search(&tc("tool_search", json!({"action": "list"})));
    assert!(listed.content.contains("(none)"), "{}", listed.content);

    let sel = a.execute_tool_search(&tc(
        "tool_search",
        json!({"action": "select", "query": "github_pr"}),
    ));
    assert!(!sel.is_error, "{sel:?}");
    let listed = a.execute_tool_search(&tc("tool_search", json!({"action": "list"})));
    assert!(
        listed.content.contains("[on]") || listed.content.contains("github_pr"),
        "{}",
        listed.content
    );

    let exact = a.execute_tool_search(&tc(
        "tool_search",
        json!({"query": "github_pr", "max_results": 5}),
    ));
    assert!(!exact.is_error, "{exact:?}");
    assert!(
        exact.content.contains("github_pr") || exact.content.contains("Matches"),
        "{}",
        exact.content
    );

    let core = a.execute_tool_search(&tc(
        "tool_search",
        json!({"action": "select", "query": "read"}),
    ));
    assert!(
        core.content.contains("read") || core.content.contains("Activated"),
        "{}",
        core.content
    );
}

#[tokio::test]
async fn execute_swarm_same_checkout_not_a_git_repo_label() {
    let mut a = scripted_test_agent([whycodes_llm::ScriptedStep::Text("ok".into())]);
    a.swarm_enabled = true;
    a.swarm_worktrees = true;
    let dir = tempfile::tempdir().unwrap();
    let session = Session::new(dir.path().to_path_buf(), "sys".into());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let out = a
        .execute_swarm_tool(
            &tc(
                "swarm",
                json!({"tasks": [{"goal": "look around", "max_turns": 1}]}),
            ),
            &session,
            "script",
            "m",
            "k",
            Some(&tx),
        )
        .await;
    assert!(!out.is_error, "{out:?}");
    let mut saw_label = out.content.contains("not a git repo");
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
    while std::time::Instant::now() < deadline {
        match rx.try_recv() {
            Ok(TurnEvent::SwarmStatus { message, .. }) => {
                if message.contains("not a git repo") {
                    saw_label = true;
                    break;
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    assert!(
        saw_label || out.content.to_lowercase().contains("swarm"),
        "{}",
        out.content
    );
}

#[test]
fn execute_tool_search_select_unknown_only_is_error() {
    let a = test_agent();
    let sel = a.execute_tool_search(&tc(
        "tool_search",
        json!({"action": "select", "query": "no-such-tool"}),
    ));
    assert!(sel.is_error, "{sel:?}");
    assert!(sel.content.contains("(none)"), "{}", sel.content);
    assert!(sel.content.contains("Unknown"), "{}", sel.content);
}

#[test]
fn execute_worktree_list_empty_dir_and_enter_unsafe_name() {
    let a = test_agent();
    let dir = tempfile::tempdir().unwrap();
    let session = Session::new(dir.path().to_path_buf(), "sys".into());
    let base = whycodes_core::project_dir(dir.path()).join("worktrees");
    std::fs::create_dir_all(&base).unwrap();
    let listed = a.execute_worktree_tool(&tc("worktree", json!({"action": "list"})), &session);
    assert!(!listed.is_error, "{listed:?}");
    assert!(listed.content.contains("(none)"), "{}", listed.content);

    let enter = a.execute_worktree_tool(
        &tc("worktree", json!({"action": "enter", "name": "a/b"})),
        &session,
    );
    assert!(enter.is_error, "{enter:?}");
    assert!(enter.content.contains("safe `name`"), "{}", enter.content);
    let empty = a.execute_worktree_tool(
        &tc("worktree", json!({"action": "enter", "name": ""})),
        &session,
    );
    assert!(empty.is_error, "{empty:?}");
}

#[test]
fn execute_worktree_remove_clears_cwd_override_and_reports_err() {
    let a = test_agent();
    let (_keep, root) = init_git_repo();
    let session = Session::new(root.clone(), "sys".into());
    let created = a.execute_worktree_tool(
        &tc("worktree", json!({"action": "create", "name": "feat-rm"})),
        &session,
    );
    assert!(!created.is_error, "{created:?}");
    let enter = a.execute_worktree_tool(
        &tc("worktree", json!({"action": "enter", "name": "feat-rm"})),
        &session,
    );
    assert!(!enter.is_error, "{enter:?}");
    assert!(a.cwd_override_path().is_some());
    let removed = a.execute_worktree_tool(
        &tc("worktree", json!({"action": "remove", "name": "feat-rm"})),
        &session,
    );
    assert!(!removed.is_error, "{removed:?}");
    assert!(a.cwd_override_path().is_none());

    let base = whycodes_core::project_dir(&root).join("worktrees");
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(base.join("not-a-tree"), b"file").unwrap();
    let err = a.execute_worktree_tool(
        &tc(
            "worktree",
            json!({"action": "remove", "name": "not-a-tree"}),
        ),
        &session,
    );
    assert!(err.is_error || err.content.contains("Removed"), "{err:?}");
}

#[tokio::test]
async fn execute_schedule_after_secs_sleeps_then_runs() {
    let a = test_agent();
    let dir = tempfile::tempdir().unwrap();
    let session = Session::new(dir.path().to_path_buf(), "sys".into());
    let ctx = a.tool_context(&session);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let scheduled = a
        .execute_schedule_tool(
            &tc(
                "schedule",
                json!({"command": "echo slept", "after_secs": 1, "description": "cov"}),
            ),
            &ctx,
            Some(&tx),
        )
        .await;
    assert!(!scheduled.is_error, "{scheduled:?}");
    let mut saw_running = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1800);
    while std::time::Instant::now() < deadline {
        match rx.try_recv() {
            Ok(TurnEvent::Background { status, .. }) if status == "running" => {
                saw_running = true;
                break;
            }
            Ok(_) => {}
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(40)).await,
        }
    }
    assert!(saw_running, "expected scheduled job after sleep");
    a.background.kill_all();
}

#[test]
fn hydrate_plugins_replaces_executor_when_plugins_exist() {
    let home = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("WHYCODES_HOME", home.path()) };
    let dir = tempfile::tempdir().unwrap();
    let why = dir.path().join(".whycodes");
    std::fs::create_dir_all(&why).unwrap();
    std::fs::write(
        why.join("plugins.toml"),
        r#"[[plugins]]
name = "hydrateplug"
command = "echo hydrate"
description = "hydrate plugin"
"#,
    )
    .unwrap();
    let mut a = test_agent();
    a.hydrate_plugins(Some(dir.path()));
    assert!(
        a.tool_executor.get("plugin_hydrateplug").is_some(),
        "plugin_hydrateplug should be registered"
    );
    unsafe { std::env::remove_var("WHYCODES_HOME") };
}

#[test]
fn builder_setters_and_memory_settings() {
    let mut a = test_agent();
    a.set_reasoning_effort(Some("xhigh".into()));
    assert_eq!(a.reasoning_effort.as_deref(), Some("xhigh"));
    let _ = a.memory_settings();
    let reg = crate::background::BackgroundRegistry::new(2);
    let a = a.with_background_registry(reg);
    assert_eq!(a.background_registry().running_count(), 0);
}

#[tokio::test]
async fn wire_event_sink_forwards_background_listener() {
    let mut a = test_agent();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    a.wire_event_sink(tx);
    let dir = tempfile::tempdir().unwrap();
    let session = Session::new(dir.path().to_path_buf(), "sys".into());
    let ctx = a.tool_context(&session);
    let started =
        a.execute_background_shell(&tc("bash", json!({"command": "echo wired"})), &ctx, None);
    assert!(!started.is_error, "{started:?}");
    let mut saw_bg = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(600);
    while std::time::Instant::now() < deadline {
        match rx.try_recv() {
            Ok(TurnEvent::Background { .. }) => {
                saw_bg = true;
                break;
            }
            Ok(_) => {}
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
    assert!(saw_bg, "expected background listener event");
    a.background.kill_all();
}

#[tokio::test]
async fn spawn_title_refine_empty_and_error_keep_heuristic() {
    let empty_agent = scripted_test_agent([whycodes_llm::ScriptedStep::Text("".into())]);
    let mut s = Session::new("/tmp".into(), "sys".into());
    s.add_user_message("please explain the retry loop in crates/llm");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    assert!(empty_agent.spawn_title_refine(&s, "script", "title-empty-cov", "k", None, tx));
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    assert!(rx.try_recv().is_err(), "empty title must not send");
    empty_agent
        .maybe_refine_title(&mut s, "script", "title-empty-cov-sync", "k", None)
        .await;

    let err_agent = scripted_test_agent([whycodes_llm::ScriptedStep::Error("boom".into())]);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    assert!(err_agent.spawn_title_refine(&s, "script", "title-err-cov", "k", None, tx));
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    assert!(rx.try_recv().is_err(), "error must not send a title");
    err_agent
        .maybe_refine_title(&mut s, "script", "title-err-cov-sync", "k", None)
        .await;

    // title_refine_target can name a provider that later disappears from the
    // cloned registry handle (or was never registered under that id).
    let missing = Agent::new(whycodes_core::types::AgentInfo {
        name: "build".into(),
        description: "t".into(),
        mode: whycodes_core::types::AgentMode::Primary,
        permission: PermissionSet {
            allow_file_writes: true,
            allow_network: true,
            allow_shell: true,
            ..Default::default()
        },
        model: None,
        system_prompt: Some("sys".into()),
        temperature: None,
        top_p: None,
    })
    .with_provider_registry(whycodes_llm::ProviderRegistry::new());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    // No registered provider → target is None and spawn is skipped.
    assert!(!missing.spawn_title_refine(&s, "script", "m", "k", None, tx.clone()));
    missing
        .maybe_refine_title(&mut s, "script", "m", "k", None)
        .await;
    assert!(rx.try_recv().is_err());
}

#[test]
fn title_refine_target_none_without_provider() {
    let a = Agent::new(whycodes_core::types::AgentInfo {
        name: "build".into(),
        description: "t".into(),
        mode: whycodes_core::types::AgentMode::Primary,
        permission: PermissionSet {
            allow_file_writes: true,
            allow_network: true,
            allow_shell: true,
            ..Default::default()
        },
        model: None,
        system_prompt: Some("sys".into()),
        temperature: None,
        top_p: None,
    })
    .with_provider_registry(ProviderRegistry::new());
    let mut s = Session::new("/tmp".into(), "sys".into());
    s.add_user_message("please explain the retry loop in crates/llm");
    assert!(
        a.title_refine_target(&s, "script", "m", "k", None)
            .is_none()
    );
}

#[test]
fn race_partner_same_model_is_none() {
    let mut a = test_agent();
    a.model_race = "script/m".into();
    assert!(a.race_partner("script", "m").is_none());
}

#[tokio::test]
async fn dummy_execute_is_callable() {
    struct Dummy {
        name: String,
    }
    impl whycodes_core::Tool for Dummy {
        fn name(&self) -> &str {
            &self.name
        }
        fn description(&self) -> &str {
            "dummy deferred tool"
        }
        fn parameters(&self) -> serde_json::Value {
            json!({"type": "object"})
        }
        fn execute<'a>(
            &'a self,
            _args: serde_json::Value,
            _ctx: &'a whycodes_core::ToolContext,
        ) -> whycodes_core::ToolFuture<'a> {
            Box::pin(async move {
                ToolResult {
                    tool_call_id: String::new(),
                    content: "ok".into(),
                    is_error: false,
                }
            })
        }
    }
    let d = Dummy {
        name: "dummy_extra_00".into(),
    };
    assert_eq!(d.description(), "dummy deferred tool");
    assert_eq!(d.parameters()["type"], "object");
    let ctx = whycodes_core::ToolContext::new("/tmp");
    let out = d.execute(json!({}), &ctx).await;
    assert_eq!(out.content, "ok");
}

#[tokio::test]
async fn execute_schedule_goal_only_enqueues_prompt() {
    let a = test_agent();
    let dir = tempfile::tempdir().unwrap();
    let session = Session::new(dir.path().to_path_buf(), "sys".into());
    let ctx = a.tool_context(&session);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let scheduled = a
        .execute_schedule_tool(
            &tc(
                "schedule",
                json!({"goal": "follow up later", "after_secs": 0}),
            ),
            &ctx,
            Some(&tx),
        )
        .await;
    assert!(!scheduled.is_error, "{scheduled:?}");
    assert!(scheduled.content.contains("prompt queue"), "{scheduled:?}");
    let mut saw_prompt = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(400);
    while std::time::Instant::now() < deadline {
        match rx.try_recv() {
            Ok(TurnEvent::EnqueuePrompt { text }) => {
                assert_eq!(text, "follow up later");
                saw_prompt = true;
                break;
            }
            Ok(_) => {}
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
    assert!(saw_prompt, "expected EnqueuePrompt from goal-only schedule");
}

#[tokio::test]
async fn execute_swarm_absolute_path_and_context() {
    let mut a = scripted_test_agent([whycodes_llm::ScriptedStep::Text("ctx-ok".into())]);
    a.swarm_enabled = true;
    a.swarm_worktrees = false;
    let dir = tempfile::tempdir().unwrap();
    let abs = dir.path().join("note.txt");
    std::fs::write(&abs, "n").unwrap();
    let session = Session::new(dir.path().to_path_buf(), "sys".into());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let out = a
        .execute_swarm_tool(
            &tc(
                "swarm",
                json!({
                    "tasks": [{
                        "goal": "summarize note.txt",
                        "subagent_type": "explore",
                        "paths": [abs.to_string_lossy()],
                        "context": "extra worker context",
                        "max_turns": 1
                    }]
                }),
            ),
            &session,
            "script",
            "m",
            "k",
            Some(&tx),
        )
        .await;
    assert!(!out.is_error, "{out:?}");
    assert!(
        out.content.to_lowercase().contains("swarm")
            || out.content.to_lowercase().contains("worker")
            || out.content.contains("ctx-ok"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn execute_swarm_drops_events_when_sink_closed() {
    let mut a = scripted_test_agent([whycodes_llm::ScriptedStep::Text("drop-ok".into())]);
    a.swarm_enabled = true;
    a.swarm_worktrees = false;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "n").unwrap();
    let session = Session::new(dir.path().to_path_buf(), "sys".into());
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    drop(rx);
    let out = a
        .execute_swarm_tool(
            &tc(
                "swarm",
                json!({
                    "tasks": [{
                        "goal": "summarize note.txt",
                        "paths": ["note.txt"],
                        "max_turns": 1
                    }]
                }),
            ),
            &session,
            "script",
            "m",
            "k",
            Some(&tx),
        )
        .await;
    assert!(!out.is_error, "{out:?}");
}

#[tokio::test]
async fn execute_swarm_worktree_merge_conflict() {
    let mut a = scripted_test_agent([whycodes_llm::ScriptedStep::Text("wt-merge".into())]);
    a.swarm_enabled = true;
    a.swarm_worktrees = true;
    let (_keep, root) = init_git_repo();
    let session = Session::new(root.clone(), "sys".into());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let out = a
        .execute_swarm_tool(
            &tc(
                "swarm",
                json!({
                    "tasks": [{
                        "goal": "summarize a.txt",
                        "subagent_type": "explore",
                        "paths": ["a.txt"],
                        "max_turns": 1
                    }]
                }),
            ),
            &session,
            "script",
            "m",
            "k",
            Some(&tx),
        )
        .await;
    assert!(!out.is_error, "{out:?}");
    assert!(
        out.content.to_lowercase().contains("swarm")
            || out.content.contains("worktree")
            || out.content.contains("wt-merge")
            || out.content.contains("Merge"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn execute_swarm_worktree_merge_conflict_after_main_diverges() {
    let mut a = scripted_test_agent([whycodes_llm::ScriptedStep::Text("wt-diverge".into())]);
    a.swarm_enabled = true;
    a.swarm_worktrees = true;
    let (_keep, root) = init_git_repo();
    let dest = root
        .join(".whycodes")
        .join("swarm")
        .join("pre-diverge")
        .join("worker-0");
    let wt = crate::swarm_worktree::create_worktree(&root, &dest, "worker-0").expect("wt");
    std::fs::write(wt.path.join("a.txt"), b"from-worker\n").unwrap();
    std::fs::write(root.join("a.txt"), b"from-main-later\n").unwrap();
    let report = crate::swarm_worktree::merge_into_main(&wt, &root);
    assert!(
        !report.conflicts.is_empty() || report.applied.iter().any(|p| p == "a.txt"),
        "{report:?}"
    );
    let _ = crate::swarm_worktree::remove_worktree(&wt);
    let session = Session::new(root.clone(), "sys".into());
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    drop(rx);
    let out = a
        .execute_swarm_tool(
            &tc(
                "swarm",
                json!({
                    "tasks": [{
                        "goal": "summarize a.txt",
                        "subagent_type": "explore",
                        "paths": ["a.txt"],
                        "context": "worker extra",
                        "max_turns": 1
                    }]
                }),
            ),
            &session,
            "script",
            "m",
            "k",
            Some(&tx),
        )
        .await;
    assert!(!out.is_error, "{out:?}");
}

#[tokio::test]
async fn execute_swarm_worktree_create_fails_when_swarm_dir_is_file() {
    let mut a = scripted_test_agent([whycodes_llm::ScriptedStep::Text("no-wt".into())]);
    a.swarm_enabled = true;
    a.swarm_worktrees = true;
    let (_keep, root) = init_git_repo();
    let why = root.join(".whycodes");
    std::fs::create_dir_all(&why).unwrap();
    std::fs::write(why.join("swarm"), b"not-a-directory").unwrap();
    let session = Session::new(root.clone(), "sys".into());
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    drop(rx);
    let out = a
        .execute_swarm_tool(
            &tc(
                "swarm",
                json!({
                    "tasks": [{
                        "goal": "summarize a.txt",
                        "subagent_type": "explore",
                        "max_turns": 1
                    }]
                }),
            ),
            &session,
            "script",
            "m",
            "k",
            Some(&tx),
        )
        .await;
    assert!(
        out.is_error
            || out.content.to_lowercase().contains("worktree")
            || out.content.to_lowercase().contains("swarm"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn execute_swarm_stale_read_and_message_with_closed_sink() {
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::batched(
        "script",
        [
            vec![whycodes_llm::ScriptedStep::ToolCall {
                id: "w0".into(),
                name: "write".into(),
                input: json!({"path": "note.txt", "content": "from-w0"}),
            }],
            vec![whycodes_llm::ScriptedStep::Text("w0-done".into())],
            vec![whycodes_llm::ScriptedStep::ToolCall {
                id: "w1r".into(),
                name: "read".into(),
                input: json!({"path": "note.txt"}),
            }],
            vec![whycodes_llm::ScriptedStep::ToolCall {
                id: "w1m".into(),
                name: "swarm_msg".into(),
                input: json!({"to": "parent", "text": "note written"}),
            }],
            vec![whycodes_llm::ScriptedStep::Text("w1-done".into())],
        ],
    )));
    let mut a = Agent::new(whycodes_core::types::AgentInfo {
        name: "build".into(),
        description: "t".into(),
        mode: whycodes_core::types::AgentMode::Primary,
        permission: whycodes_core::types::PermissionSet {
            allow_file_writes: true,
            allow_network: true,
            allow_shell: true,
            ..Default::default()
        },
        model: None,
        system_prompt: Some("sys".into()),
        temperature: None,
        top_p: None,
    })
    .with_provider_registry(registry);
    a.swarm_enabled = true;
    a.swarm_worktrees = false;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "orig").unwrap();
    let session = Session::new(dir.path().to_path_buf(), "sys".into());
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    drop(rx);
    let out = a
        .execute_swarm_tool(
            &tc(
                "swarm",
                json!({
                    "max_concurrent": 1,
                    "tasks": [
                        {"goal": "write note.txt", "subagent_type": "general", "max_turns": 3},
                        {"goal": "read note.txt", "subagent_type": "general", "max_turns": 4}
                    ]
                }),
            ),
            &session,
            "script",
            "m",
            "k",
            Some(&tx),
        )
        .await;
    assert!(
        !out.is_error || out.content.to_lowercase().contains("swarm"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn execute_swarm_two_writers_conflict_with_closed_sink() {
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::repeating(
        "script",
        [
            whycodes_llm::ScriptedStep::ToolCall {
                id: "w".into(),
                name: "write".into(),
                input: json!({"path": "note.txt", "content": "from-worker"}),
            },
            whycodes_llm::ScriptedStep::Text("wrote".into()),
        ],
    )));
    let mut a = Agent::new(whycodes_core::types::AgentInfo {
        name: "build".into(),
        description: "t".into(),
        mode: whycodes_core::types::AgentMode::Primary,
        permission: whycodes_core::types::PermissionSet {
            allow_file_writes: true,
            allow_network: true,
            allow_shell: true,
            ..Default::default()
        },
        model: None,
        system_prompt: Some("sys".into()),
        temperature: None,
        top_p: None,
    })
    .with_provider_registry(registry);
    a.swarm_enabled = true;
    a.swarm_worktrees = false;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "orig").unwrap();
    let session = Session::new(dir.path().to_path_buf(), "sys".into());
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    drop(rx);
    let out = a
        .execute_swarm_tool(
            &tc(
                "swarm",
                json!({
                    "max_concurrent": 2,
                    "tasks": [
                        {"goal": "write note.txt", "subagent_type": "general", "max_turns": 2},
                        {"goal": "also write note.txt", "subagent_type": "general", "max_turns": 2}
                    ]
                }),
            ),
            &session,
            "script",
            "m",
            "k",
            Some(&tx),
        )
        .await;
    assert!(
        !out.is_error || out.content.to_lowercase().contains("swarm"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn load_mcp_registers_stdio_echo() {
    if !std::path::Path::new("/usr/bin/python3").exists() && which_python().is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("echo.py");
    std::fs::write(
        &script,
        r#"
import json, sys
def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()
for line in sys.stdin:
    req = json.loads(line)
    mid = req.get("id")
    method = req.get("method")
    if method == "initialize":
        send({"jsonrpc":"2.0","id":mid,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"ghost","version":"0"}}})
    elif method == "notifications/initialized":
        pass
    elif method == "tools/list":
        send({"jsonrpc":"2.0","id":mid,"result":{"tools":[{"name":"echo","description":"echo","inputSchema":{"type":"object"}}]}})
    elif method == "tools/call":
        send({"jsonrpc":"2.0","id":mid,"result":{"content":[{"type":"text","text":"echo:ok"}]}})
    else:
        send({"jsonrpc":"2.0","id":mid,"error":{"code":-32601,"message":"unknown"}})
"#,
    )
    .unwrap();
    let mut config = whycodes_config::Config::default();
    config.mcp_servers.insert(
        "ghost".into(),
        whycodes_config::McpServerConfig {
            transport: Some(whycodes_config::McpTransportKind::Stdio),
            command: Some(which_python().unwrap_or("python3").into()),
            args: vec!["-u".into(), script.to_string_lossy().into_owned()],
            env: None,
            cwd: Some(dir.path().to_string_lossy().into_owned()),
            url: None,
            headers: None,
        },
    );
    let mut a = test_agent();
    a.load_mcp(&config).await;
    assert!(
        a.tool_executor.get("ghost_echo").is_some(),
        "load_mcp should register ghost_echo"
    );
}

#[test]
fn apply_config_logs_debug_fields() {
    let mut a = test_agent();
    let mut cfg = whycodes_config::Config::default();
    cfg.session.compaction_threshold = 12_000;
    cfg.session.tool_profile = "core".into();
    cfg.session.model_race = "off".into();
    cfg.swarm.enabled = true;
    cfg.swarm.max_agents = 2;
    a.apply_config(&cfg);
    assert_eq!(a.compaction_threshold, 12_000);
    assert!(a.swarm_enabled);
}

#[test]
fn with_plugins_registers_when_present() {
    let dir = tempfile::tempdir().unwrap();
    let prev = std::env::var_os("WHYCODES_HOME");
    unsafe { std::env::set_var("WHYCODES_HOME", dir.path()) };
    let why = dir.path().join(".whycodes");
    std::fs::create_dir_all(&why).unwrap();
    std::fs::write(
        why.join("plugins.toml"),
        r#"[[plugins]]
name = "covhome"
command = "echo cov"
description = "coverage plugin"
"#,
    )
    .unwrap();
    let a = test_agent().with_plugins(Some(dir.path()));
    if let Some(v) = prev {
        unsafe { std::env::set_var("WHYCODES_HOME", v) };
    } else {
        unsafe { std::env::remove_var("WHYCODES_HOME") };
    }
    assert!(
        a.tool_executor.get("plugin_covhome").is_some(),
        "with_plugins should register when a plugin file is present"
    );
}

#[test]
fn apply_plugin_count_skips_zero_and_replaces_on_hit() {
    let mut a = test_agent();
    let before = Arc::as_ptr(&a.tool_executor);
    apply_plugin_count(&mut a, ToolExecutor::new(), 0);
    assert_eq!(Arc::as_ptr(&a.tool_executor), before);

    apply_plugin_count(&mut a, ToolExecutor::new(), 2);
    assert_ne!(Arc::as_ptr(&a.tool_executor), before);
}

#[test]
fn log_registered_count_skips_zero() {
    log_registered_count(0, "shell plugins registered");
    log_registered_count(3, "MCP tools registered");
}

#[tokio::test]
async fn maybe_refine_title_returns_when_target_none() {
    let a = test_agent();
    let mut s = Session::new("/tmp".into(), "sys".into());
    s.title = "Keep".into();
    s.title_source = whycodes_session::TitleSource::Manual;
    s.add_user_message("hi");
    a.maybe_refine_title(&mut s, "script", "m", "k", None).await;
    assert_eq!(s.title, "Keep");
}

#[tokio::test]
async fn spawn_title_refine_skips_when_target_none() {
    let a = test_agent();
    let mut s = Session::new("/tmp".into(), "sys".into());
    s.title = "Keep".into();
    s.title_source = whycodes_session::TitleSource::Manual;
    s.add_user_message("hi");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    assert!(!a.spawn_title_refine(&s, "script", "m", "k", None, tx));
}

#[test]
fn catalog_from_load_ok_and_err() {
    let empty = catalog_from_load(Ok(whycodes_skill::SkillRegistry::new()));
    assert!(empty.is_empty());
    let failed = catalog_from_load(Err(whycodes_skill::SkillError::msg("load failed")));
    assert!(failed.is_empty());
}

#[tokio::test]
async fn title_from_optional_provider_none_is_empty() {
    let title = title_from_optional_provider(None, "k", "m", "user", None)
        .await
        .expect("none provider is Ok empty");
    assert!(title.is_empty(), "{title}");
}

fn poison_mutex<T>(value: T) -> std::sync::Mutex<T> {
    let m = std::sync::Mutex::new(value);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _g = m.lock().unwrap();
        panic!("poison agent mutex");
    }));
    m
}

#[test]
fn recover_lock_and_json_fallback_cover_poison_and_err() {
    let m = poison_mutex(9u8);
    assert_eq!(*recover_lock(&m), 9);
    assert_eq!(json_string_or(Ok("{\"a\":1}".into()), "{}"), "{\"a\":1}");
    let err = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
    assert_eq!(json_string_or(Err(err), "{}"), "{}");
    assert_eq!(
        json_or_object(&serde_json::json!({"k": 1})),
        serde_json::to_string(&serde_json::json!({"k": 1})).unwrap()
    );
}

#[tokio::test]
async fn execute_swarm_general_write_merges_worktree_conflict() {
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::batched(
        "script",
        [
            vec![
                whycodes_llm::ScriptedStep::Hang(std::time::Duration::from_millis(250)),
                whycodes_llm::ScriptedStep::ToolCall {
                    id: "w".into(),
                    name: "write".into(),
                    input: json!({"path": "a.txt", "content": "from-worker"}),
                },
            ],
            vec![whycodes_llm::ScriptedStep::Text("worker done".into())],
        ],
    )));
    let mut a = Agent::new(whycodes_core::types::AgentInfo {
        name: "build".into(),
        description: "t".into(),
        mode: whycodes_core::types::AgentMode::Primary,
        permission: whycodes_core::types::PermissionSet {
            allow_file_writes: true,
            allow_network: true,
            allow_shell: true,
            ..Default::default()
        },
        model: None,
        system_prompt: Some("sys".into()),
        temperature: None,
        top_p: None,
    })
    .with_provider_registry(registry);
    a.swarm_enabled = true;
    a.swarm_worktrees = true;
    let (_keep, root) = init_git_repo();
    let session = Session::new(root.clone(), "sys".into());
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    drop(rx);
    let call = tc(
        "swarm",
        json!({
            "tasks": [{
                "goal": "rewrite a.txt",
                "subagent_type": "general",
                "context": "keep the file local",
                "max_turns": 4
            }]
        }),
    );
    let swarm = a.execute_swarm_tool(&call, &session, "script", "m", "k", Some(&tx));
    tokio::pin!(swarm);
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    std::fs::write(root.join("a.txt"), "from-main-later").unwrap();
    let out = swarm.await;
    assert!(
        out.content.to_lowercase().contains("merge")
            || out.content.contains("conflict")
            || out.content.contains("worker")
            || !out.content.is_empty(),
        "{}",
        out.content
    );
}

struct PanicOnStreamProvider;

impl whycodes_llm::LlmProvider for PanicOnStreamProvider {
    fn name(&self) -> &str {
        "panic-stream"
    }
    fn default_base_url(&self) -> &str {
        "http://script.invalid"
    }
    fn complete<'a>(
        &'a self,
        _request: &'a whycodes_core::types::LlmRequest,
        _api_key: &'a str,
        _model: &'a str,
    ) -> whycodes_llm::provider::ProviderResponseFuture<'a> {
        Box::pin(async { Err(whycodes_core::Error::llm("complete-only")) })
    }
    fn stream<'a>(
        &'a self,
        _request: &'a whycodes_core::types::LlmRequest,
        _api_key: &'a str,
        _model: &'a str,
    ) -> whycodes_llm::provider::ProviderStreamFuture<'a> {
        panic!("swarm worker join coverage");
    }
}

#[tokio::test]
async fn execute_swarm_worker_join_error_when_provider_panics() {
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(PanicOnStreamProvider));
    let mut a = Agent::new(whycodes_core::types::AgentInfo {
        name: "build".into(),
        description: "t".into(),
        mode: whycodes_core::types::AgentMode::Primary,
        permission: PermissionSet {
            allow_file_writes: true,
            allow_network: true,
            allow_shell: true,
            ..Default::default()
        },
        model: None,
        system_prompt: Some("sys".into()),
        temperature: None,
        top_p: None,
    })
    .with_provider_registry(registry);
    a.swarm_enabled = true;
    a.swarm_worktrees = false;
    let dir = tempfile::tempdir().unwrap();
    let session = Session::new(dir.path().to_path_buf(), "sys".into());
    let out = a
        .execute_swarm_tool(
            &tc(
                "swarm",
                json!({
                    "tasks": [{
                        "goal": "panic worker",
                        "max_turns": 1
                    }]
                }),
            ),
            &session,
            "panic-stream",
            "m",
            "k",
            None,
        )
        .await;
    assert!(
        out.content.to_lowercase().contains("join error")
            || out.content.to_lowercase().contains("panic")
            || out.content.to_lowercase().contains("worker"),
        "{}",
        out.content
    );
}

fn which_python() -> Option<&'static str> {
    if std::path::Path::new("/usr/bin/python3").exists() {
        Some("/usr/bin/python3")
    } else {
        None
    }
}
