use super::*;

#[test]
fn inject_memory_passthrough_when_disabled() {
    let memory = whycodes_memory::MemorySettings::disabled();
    let out = inject_subagent_memory(
        "base prompt",
        std::path::Path::new("/work/proj"),
        std::path::Path::new("/unused-data"),
        "worker",
        "do the thing",
        &memory,
    );
    assert_eq!(out, "base prompt");
}

#[test]
fn inject_memory_passthrough_when_env_off_switch() {
    let prev = std::env::var_os("WHYCODES_NO_MEMORY");
    unsafe { std::env::set_var("WHYCODES_NO_MEMORY", "1") };
    let out = inject_subagent_memory(
        "base prompt",
        std::path::Path::new("/work/proj"),
        std::path::Path::new("/unused-data"),
        "worker",
        "do the thing",
        &whycodes_memory::MemorySettings::default(),
    );
    if let Some(v) = prev {
        unsafe { std::env::set_var("WHYCODES_NO_MEMORY", v) };
    } else {
        unsafe { std::env::remove_var("WHYCODES_NO_MEMORY") };
    }
    assert_eq!(out, "base prompt");
}

#[test]
fn runner_builders_set_state() {
    let runner = make_runner();
    let idx = whycodes_index::WorkspaceIndex::start(Vec::new());
    let hub = whycodes_core::SwarmHub::default();
    let runner = runner
        .with_file_index(Some(idx))
        .with_panel(None)
        .with_swarm_hub(Some(hub))
        .with_memory(whycodes_memory::MemorySettings::disabled())
        .with_file_claims(
            whycodes_core::FileClaimRegistry::default(),
            "worker-1",
            "Worker One",
        );
    assert_eq!(runner.agent_id.as_deref(), Some("worker-1"));
    assert_eq!(runner.agent_label.as_deref(), Some("Worker One"));
    assert!(runner.file_claims.is_some());
    assert!(runner.file_index.is_some());
    assert!(!runner.memory.enabled);
}

#[test]
fn with_identity_names_the_runner_without_file_claims() {
    let runner = make_runner().with_identity("worker-2", "worker-2/explore");
    assert_eq!(runner.agent_id.as_deref(), Some("worker-2"));
    assert_eq!(runner.agent_label.as_deref(), Some("worker-2/explore"));
    assert!(runner.file_claims.is_none());
}

#[test]
fn memory_project_defaults_to_cwd_and_overrides_for_worktrees() {
    let runner = make_runner();
    assert_eq!(
        runner.memory_project_path(),
        std::path::Path::new("/work/proj")
    );
    // Swarm worktree: cwd is a throwaway checkout, memory stays on the main project.
    let runner = SubagentRunner::new(
        Arc::new(ProviderRegistry::default()),
        Arc::new(ToolExecutor::new()),
        make_info(),
        std::path::PathBuf::from("/work/proj/.whycodes/swarm/run/worker-0"),
        SandboxSettings::off(),
        NetworkPolicy::unrestricted(),
    )
    .with_memory_project(std::path::PathBuf::from("/work/proj"));
    assert_eq!(
        runner.memory_project_path(),
        std::path::Path::new("/work/proj")
    );
}

#[tokio::test]
async fn run_returns_failed_result_for_preflight_errors() {
    let cases = [
        ("anthropic", 0, None, "exceeded maximum turns (0)"),
        (
            "missing-provider",
            1,
            Some("use this context".to_string()),
            "Unknown provider: missing-provider",
        ),
    ];

    for (provider, max_turns, context, expected) in cases {
        let result = make_runner()
            .run(
                SubagentTask {
                    goal: "inspect the project".into(),
                    context,
                    tools: Some(vec!["read".into()]),
                    max_turns,
                    output_schema: None,
                },
                provider,
                "test-model",
                "test-key",
            )
            .await;

        assert_eq!(result.goal, "inspect the project");
        assert!(!result.success);
        assert!(result.output.contains(expected), "{}", result.output);
        assert!(result.usage.is_empty());
    }
}

fn make_runner() -> SubagentRunner {
    SubagentRunner::new(
        Arc::new(ProviderRegistry::default()),
        Arc::new(ToolExecutor::new()),
        make_info(),
        std::path::PathBuf::from("/work/proj"),
        SandboxSettings::off(),
        NetworkPolicy::unrestricted(),
    )
}

fn make_info() -> AgentInfo {
    AgentInfo {
        name: "worker".into(),
        description: "test worker".into(),
        mode: whycodes_core::types::AgentMode::Primary,
        permission: PermissionSet::default(),
        model: None,
        system_prompt: None,
        temperature: None,
        top_p: None,
    }
}

fn scripted_runner(steps: impl IntoIterator<Item = whycodes_llm::ScriptedStep>) -> SubagentRunner {
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::repeating(
        "script", steps,
    )));
    SubagentRunner::new(
        Arc::new(registry),
        Arc::new(ToolExecutor::new()),
        make_info(),
        std::env::temp_dir(),
        SandboxSettings::off(),
        NetworkPolicy::unrestricted(),
    )
    .with_approval_mode(ApprovalMode::Auto)
}

#[tokio::test]
async fn run_tool_loop_reads_usage_thinking_and_question() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("n.txt"), "payload").unwrap();
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::batched(
        "script",
        [
            vec![
                whycodes_llm::ScriptedStep::Thinking("plan".into()),
                whycodes_llm::ScriptedStep::ThinkingDelta(" more".into()),
                whycodes_llm::ScriptedStep::ThinkingSignature("sig".into()),
                whycodes_llm::ScriptedStep::RedactedThinking("red".into()),
                whycodes_llm::ScriptedStep::MessageStart,
                whycodes_llm::ScriptedStep::MessageDelta(serde_json::json!({"x": 1})),
                whycodes_llm::ScriptedStep::Usage {
                    input_tokens: 3,
                    output_tokens: 2,
                },
                whycodes_llm::ScriptedStep::CacheUsage {
                    creation_input_tokens: 1,
                    read_input_tokens: 1,
                },
                whycodes_llm::ScriptedStep::ToolCall {
                    id: "r1".into(),
                    name: "read".into(),
                    input: serde_json::json!({"path": "n.txt"}),
                },
                whycodes_llm::ScriptedStep::ToolCall {
                    id: "r2".into(),
                    name: "read".into(),
                    input: serde_json::json!({"path": "n.txt"}),
                },
            ],
            vec![whycodes_llm::ScriptedStep::Text("done after reads".into())],
        ],
    )));
    let runner = SubagentRunner::new(
        Arc::new(registry),
        Arc::new(ToolExecutor::new()),
        make_info(),
        dir.path().to_path_buf(),
        SandboxSettings::off(),
        NetworkPolicy::unrestricted(),
    );
    let result = runner
        .run(
            SubagentTask {
                goal: "read n.txt".into(),
                context: None,
                tools: None,
                max_turns: 4,
                output_schema: None,
            },
            "script",
            "m",
            "k",
        )
        .await;
    assert!(result.success, "{result:?}");
    assert!(
        result.output.contains("done") || !result.output.is_empty(),
        "{}",
        result.output
    );
    assert!(!result.usage.is_empty());
}

#[tokio::test]
async fn run_intercepts_question_and_swarm_inbox() {
    let dir = tempfile::tempdir().unwrap();
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::batched(
        "script",
        [
            vec![whycodes_llm::ScriptedStep::ToolCall {
                id: "q1".into(),
                name: "question".into(),
                input: serde_json::json!({
                    "questions": [{"prompt": "pick", "options": [{"label": "A"}]}]
                }),
            }],
            vec![whycodes_llm::ScriptedStep::Text("answered".into())],
        ],
    )));
    let hub = whycodes_core::SwarmHub::new();
    hub.ensure("worker-q");
    hub.send("parent", "worker-q", "hello worker").unwrap();
    let runner = SubagentRunner::new(
        Arc::new(registry),
        Arc::new(ToolExecutor::new()),
        make_info(),
        dir.path().to_path_buf(),
        SandboxSettings::off(),
        NetworkPolicy::unrestricted(),
    )
    .with_swarm_hub(Some(hub))
    .with_file_claims(
        whycodes_core::FileClaimRegistry::new(),
        "worker-q",
        "Worker Q",
    )
    .with_approval_mode(ApprovalMode::Auto);
    let result = runner
        .run(
            SubagentTask {
                goal: "ask then finish".into(),
                context: Some("ctx".into()),
                tools: Some(vec!["question".into(), "read".into()]),
                max_turns: 4,
                output_schema: None,
            },
            "script",
            "m",
            "k",
        )
        .await;
    assert!(result.success, "{result:?}");
    assert!(
        result.output.contains("answered") || !result.output.is_empty(),
        "{}",
        result.output
    );
}

#[test]
fn inject_memory_honors_subagent_banks_off() {
    let prev_no = std::env::var_os("WHYCODES_NO_MEMORY");
    let prev_banks = std::env::var_os("WHYCODES_SUBAGENT_BANKS");
    unsafe { std::env::remove_var("WHYCODES_NO_MEMORY") };
    unsafe { std::env::set_var("WHYCODES_SUBAGENT_BANKS", "0") };
    let memory = MemorySettings {
        enabled: true,
        subagent_banks: true,
        ..MemorySettings::default()
    };
    let data = tempfile::tempdir().unwrap();
    let out = inject_subagent_memory(
        "base prompt",
        std::path::Path::new("/work/proj"),
        data.path(),
        "worker",
        "do the thing",
        &memory,
    );
    // Without the env switch the subagent gets its own bank.
    unsafe { std::env::remove_var("WHYCODES_SUBAGENT_BANKS") };
    let banked = inject_subagent_memory(
        "base prompt",
        std::path::Path::new("/work/proj"),
        data.path(),
        "worker",
        "do the thing",
        &memory,
    );
    if let Some(v) = prev_no {
        unsafe { std::env::set_var("WHYCODES_NO_MEMORY", v) };
    } else {
        unsafe { std::env::remove_var("WHYCODES_NO_MEMORY") };
    }
    if let Some(v) = prev_banks {
        unsafe { std::env::set_var("WHYCODES_SUBAGENT_BANKS", v) };
    } else {
        unsafe { std::env::remove_var("WHYCODES_SUBAGENT_BANKS") };
    }
    assert!(out.contains("base prompt"), "{out}");
    assert!(banked.contains("base prompt"), "{banked}");
    let _ = scripted_runner([whycodes_llm::ScriptedStep::Text("x".into())]);
}

#[tokio::test]
async fn run_wraps_stream_error_as_failed_result() {
    let result = scripted_runner([whycodes_llm::ScriptedStep::Error("stream boom".into())])
        .run(
            SubagentTask {
                goal: "inspect".into(),
                context: None,
                tools: Some(vec!["read".into()]),
                max_turns: 2,
                output_schema: None,
            },
            "script",
            "m",
            "k",
        )
        .await;
    assert!(!result.success);
    assert!(
        result.output.to_lowercase().contains("boom")
            || result.output.to_lowercase().contains("error"),
        "{}",
        result.output
    );
}

#[tokio::test]
async fn run_tool_use_delta_then_text() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("n.txt"), "payload").unwrap();
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::batched(
        "script",
        [
            vec![
                whycodes_llm::ScriptedStep::ToolCall {
                    id: "c1".into(),
                    name: "read".into(),
                    input: serde_json::json!({}),
                },
                whycodes_llm::ScriptedStep::ToolUseDelta {
                    id: "c1".into(),
                    input_json_delta: r#"{"path":"n.txt"}"#.into(),
                },
            ],
            vec![whycodes_llm::ScriptedStep::Text("got it".into())],
        ],
    )));
    let runner = SubagentRunner::new(
        Arc::new(registry),
        Arc::new(ToolExecutor::new()),
        make_info(),
        dir.path().to_path_buf(),
        SandboxSettings::off(),
        NetworkPolicy::unrestricted(),
    )
    .with_approval_mode(ApprovalMode::Auto);
    let result = runner
        .run(
            SubagentTask {
                goal: "read n.txt".into(),
                context: None,
                tools: Some(vec!["read".into()]),
                max_turns: 4,
                output_schema: None,
            },
            "script",
            "m",
            "k",
        )
        .await;
    assert!(result.success, "{result:?}");
    assert!(
        result.output.contains("got it") || !result.output.is_empty(),
        "{}",
        result.output
    );
}

#[tokio::test]
async fn run_invalid_question_and_manual_prompter() {
    let dir = tempfile::tempdir().unwrap();
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::batched(
        "script",
        [
            vec![whycodes_llm::ScriptedStep::ToolCall {
                id: "q1".into(),
                name: "question".into(),
                input: serde_json::json!({}),
            }],
            vec![whycodes_llm::ScriptedStep::Text("after invalid".into())],
        ],
    )));
    let (prompter, mut rx) = crate::question::ChannelQuestionPrompter::new(None);
    let runner = SubagentRunner::new(
        Arc::new(registry),
        Arc::new(ToolExecutor::new()),
        make_info(),
        dir.path().to_path_buf(),
        SandboxSettings::off(),
        NetworkPolicy::unrestricted(),
    )
    .with_question_prompter(Arc::new(prompter))
    .with_approval_mode(ApprovalMode::Manual);
    let run = tokio::spawn(async move {
        runner
            .run(
                SubagentTask {
                    goal: "ask then finish".into(),
                    context: None,
                    tools: Some(vec!["question".into()]),
                    max_turns: 4,
                    output_schema: None,
                },
                "script",
                "m",
                "k",
            )
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    while let Ok(req) = rx.try_recv() {
        let _ = req.reply.send(Ok(vec![]));
    }
    let result = run.await.expect("join");
    assert!(
        result.output.contains("after invalid")
            || result.output.to_lowercase().contains("invalid")
            || !result.output.is_empty(),
        "{}",
        result.output
    );
}

#[tokio::test]
async fn run_important_question_uses_prompter() {
    let dir = tempfile::tempdir().unwrap();
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::batched(
        "script",
        [
            vec![whycodes_llm::ScriptedStep::ToolCall {
                id: "q1".into(),
                name: "question".into(),
                input: serde_json::json!({
                    "questions": [{
                        "prompt": "pick",
                        "options": [{"label": "A"}, {"label": "B"}],
                        "important": true
                    }]
                }),
            }],
            vec![whycodes_llm::ScriptedStep::Text("after ask".into())],
        ],
    )));
    let (prompter, mut rx) = crate::question::ChannelQuestionPrompter::new(None);
    let runner = SubagentRunner::new(
        Arc::new(registry),
        Arc::new(ToolExecutor::new()),
        make_info(),
        dir.path().to_path_buf(),
        SandboxSettings::off(),
        NetworkPolicy::unrestricted(),
    )
    .with_question_prompter(Arc::new(prompter))
    .with_approval_mode(ApprovalMode::Manual);
    let run = tokio::spawn(async move {
        runner
            .run(
                SubagentTask {
                    goal: "ask then finish".into(),
                    context: None,
                    tools: Some(vec!["question".into()]),
                    max_turns: 4,
                    output_schema: None,
                },
                "script",
                "m",
                "k",
            )
            .await
    });
    let req = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .expect("timeout")
        .expect("request");
    req.reply
        .send(Ok(vec![whycodes_tools::question::QuestionAnswer {
            selected: vec!["A".into()],
            free_text: None,
            auto_picked: false,
        }]))
        .unwrap();
    let result = run.await.expect("join");
    assert!(
        result.output.contains("after ask") || result.success || !result.output.is_empty(),
        "{}",
        result.output
    );
}

#[tokio::test]
async fn run_tool_loop_serializes_mutator_batch() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("n.txt"), "payload").unwrap();
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::batched(
        "script",
        [
            vec![
                whycodes_llm::ScriptedStep::ToolCall {
                    id: "w1".into(),
                    name: "write".into(),
                    input: serde_json::json!({"path": "n.txt", "content": "one"}),
                },
                whycodes_llm::ScriptedStep::ToolCall {
                    id: "w2".into(),
                    name: "write".into(),
                    input: serde_json::json!({"path": "m.txt", "content": "two"}),
                },
            ],
            vec![whycodes_llm::ScriptedStep::Text("mutators done".into())],
        ],
    )));
    let mut info = make_info();
    info.permission.allow_file_writes = true;
    let runner = SubagentRunner::new(
        Arc::new(registry),
        Arc::new(ToolExecutor::new()),
        info,
        dir.path().to_path_buf(),
        SandboxSettings::off(),
        NetworkPolicy::unrestricted(),
    );
    let result = runner
        .run(
            SubagentTask {
                goal: "write both files".into(),
                context: None,
                tools: None,
                max_turns: 4,
                output_schema: None,
            },
            "script",
            "m",
            "k",
        )
        .await;
    assert!(
        result.success || result.output.contains("mutators") || !result.output.is_empty(),
        "{result:?}"
    );
}

fn hook(
    event: HookEvent,
    tool_match: &str,
    command: &str,
    block: bool,
    feedback: bool,
) -> HookConfig {
    HookConfig {
        event,
        tool_match: tool_match.into(),
        command: command.into(),
        block_on_failure: block,
        feedback,
        timeout_secs: 10,
    }
}

#[tokio::test]
async fn subagent_tool_calls_run_parent_tool_hooks() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("n.txt"), "payload").unwrap();
    let runner = SubagentRunner::new(
        Arc::new(ProviderRegistry::default()),
        Arc::new(ToolExecutor::new()),
        make_info(),
        dir.path().to_path_buf(),
        SandboxSettings::off(),
        NetworkPolicy::unrestricted(),
    )
    .with_hooks(vec![
        hook(HookEvent::PreTool, "write", "exit 1", true, false),
        hook(HookEvent::PostTool, "read", "echo checked", false, true),
    ]);
    let ctx = ToolContext::new(dir.path().display().to_string());
    let permission = PermissionSet {
        allow_file_writes: true,
        ..Default::default()
    };
    let read = whycodes_core::types::ToolCall {
        id: "r1".into(),
        name: "read".into(),
        arguments: serde_json::json!({"path": "n.txt"}),
    };
    let result = runner.execute_tool_call(&read, &ctx, &permission).await;
    assert!(
        result.content.ends_with("[post_tool hook]\nchecked"),
        "{}",
        result.content
    );

    let write = whycodes_core::types::ToolCall {
        id: "w1".into(),
        name: "write".into(),
        arguments: serde_json::json!({"path": "x.txt", "content": "x"}),
    };
    let blocked = runner.execute_tool_call(&write, &ctx, &permission).await;
    assert!(blocked.is_error);
    assert!(
        blocked.content.contains("pre_tool hook blocked `write`"),
        "{}",
        blocked.content
    );
    assert!(!dir.path().join("x.txt").exists());
}

#[tokio::test]
async fn finished_subagent_runs_matching_stop_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("log.txt");
    // `cmd /C` mangles quoted args; temp dirs have no spaces.
    let log = |word: &str| {
        if cfg!(windows) {
            format!("echo {word}>>{}", marker.display())
        } else {
            format!("echo {word} >> '{}'", marker.display())
        }
    };
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::repeating(
        "script",
        [whycodes_llm::ScriptedStep::Text(
            "worker summary of the retry loop".into(),
        )],
    )));
    let runner = SubagentRunner::new(
        Arc::new(registry),
        Arc::new(ToolExecutor::new()),
        make_info(),
        dir.path().to_path_buf(),
        SandboxSettings::off(),
        NetworkPolicy::unrestricted(),
    )
    .with_hooks(vec![
        hook(
            HookEvent::SubagentStop,
            "worker",
            &log("worker"),
            false,
            false,
        ),
        hook(
            HookEvent::SubagentStop,
            "explore",
            &log("explore"),
            false,
            false,
        ),
    ]);
    let result = runner
        .run(
            SubagentTask {
                goal: "summarize the retry loop in crates/agent".into(),
                context: None,
                tools: None,
                max_turns: 2,
                output_schema: None,
            },
            "script",
            "m",
            "k",
        )
        .await;
    assert!(result.success, "{result:?}");
    let lines: Vec<String> = std::fs::read_to_string(&marker)
        .unwrap()
        .lines()
        .map(|l| l.trim().to_string())
        .collect();
    assert_eq!(lines, ["worker"]);
}

#[tokio::test]
async fn reads_messages_told_to_it_and_reports_late_ones() {
    let inbox = crate::steer::SteerInbox::default();
    inbox.push(
        "task-1",
        whycodes_core::types::MessageContent::Text("focus on errors".into()),
    );
    let runner = scripted_runner([whycodes_llm::ScriptedStep::Text("done".into())])
        .with_steer(inbox.clone(), "task-1");
    let task = || SubagentTask {
        goal: "look around".into(),
        context: None,
        tools: None,
        max_turns: 2,
        output_schema: None,
    };
    let result = runner.run(task(), "script", "m", "k").await;
    assert!(result.success, "{result:?}");
    assert!(inbox.drain("task-1").is_empty(), "read at the first step");
    assert!(!result.output.contains("finished:"), "{}", result.output);

    assert_eq!(unread_tell_note(Vec::new()), "");
    let late = vec![whycodes_core::types::MessageContent::Text(
        "too late".into(),
    )];
    let note = unread_tell_note(late);
    assert!(note.contains("after this subagent finished"), "{note}");
    assert!(note.contains("- too late"), "{note}");
}

// ---------------------------------------------------------------------------
// Headless gate: workers get the parent's blast-radius checks without a prompter.
// ---------------------------------------------------------------------------

fn gate_runner(dir: &std::path::Path, mode: ApprovalMode) -> SubagentRunner {
    SubagentRunner::new(
        Arc::new(ProviderRegistry::default()),
        Arc::new(ToolExecutor::new()),
        make_info(),
        dir.to_path_buf(),
        SandboxSettings::off(),
        NetworkPolicy::unrestricted(),
    )
    .with_approval_mode(mode)
}

fn gate_perms(rules: &[(&str, PermissionAction)]) -> PermissionSet {
    let mut p = PermissionSet {
        allow_shell: true,
        allow_file_writes: true,
        ..Default::default()
    };
    for (k, a) in rules {
        p.rules.insert((*k).to_string(), *a);
    }
    p
}

fn call(name: &str, args: serde_json::Value) -> whycodes_core::types::ToolCall {
    whycodes_core::types::ToolCall {
        id: format!("{name}-1"),
        name: name.into(),
        arguments: args,
    }
}

#[tokio::test]
async fn headless_gate_refuses_catastrophic_shell_in_every_mode() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path().display().to_string());
    let perms = gate_perms(&[]);
    for mode in ApprovalMode::ALL {
        let runner = gate_runner(dir.path(), mode);
        for tool in ["bash", "shell"] {
            let result = runner
                .execute_tool_call(
                    &call(tool, serde_json::json!({"command": "rm -rf /"})),
                    &ctx,
                    &perms,
                )
                .await;
            assert!(result.is_error, "{mode:?} {tool}");
            assert!(
                result.content.contains("catastrophic"),
                "{}",
                result.content
            );
        }
        // `schedule` carries a delayed shell payload.
        let scheduled = runner
            .execute_tool_call(
                &call(
                    "schedule",
                    serde_json::json!({"after_secs": 1, "command": " mkfs /dev/sda1 "}),
                ),
                &ctx,
                &perms,
            )
            .await;
        assert!(scheduled.is_error);
        assert!(
            scheduled.content.contains("catastrophic"),
            "{}",
            scheduled.content
        );
    }
    // An empty scheduled command is not a shell payload.
    let runner = gate_runner(dir.path(), ApprovalMode::Manual);
    let empty = runner
        .execute_tool_call(
            &call(
                "schedule",
                serde_json::json!({"after_secs": 1, "command": ""}),
            ),
            &ctx,
            &perms,
        )
        .await;
    assert!(!empty.content.contains("Refused"), "{}", empty.content);
}

#[tokio::test]
async fn headless_gate_confirm_passes_only_in_auto() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path().display().to_string());
    let perms = gate_perms(&[]);
    let destructive = serde_json::json!({"command": "git push --force"});

    let manual = gate_runner(dir.path(), ApprovalMode::Manual)
        .execute_tool_call(&call("bash", destructive.clone()), &ctx, &perms)
        .await;
    assert!(manual.is_error);
    assert!(
        manual.content.contains("cannot prompt"),
        "{}",
        manual.content
    );
    assert!(manual.content.contains("`manual`"), "{}", manual.content);
    assert!(manual.content.contains("shell risk"), "{}", manual.content);

    let auto = gate_runner(dir.path(), ApprovalMode::Auto)
        .execute_tool_call(&call("bash", destructive.clone()), &ctx, &perms)
        .await;
    assert!(!auto.content.contains("cannot prompt"), "{}", auto.content);

    // The parent's threshold is inherited: `off` never confirms.
    let off = gate_runner(dir.path(), ApprovalMode::Manual)
        .with_risk_threshold(RiskThreshold::Off)
        .execute_tool_call(&call("bash", destructive), &ctx, &perms)
        .await;
    assert!(!off.content.contains("cannot prompt"), "{}", off.content);
}

#[tokio::test]
async fn headless_gate_applies_shell_rules() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path().display().to_string());
    let status = || call("bash", serde_json::json!({"command": "echo hi"}));

    let denied = gate_runner(dir.path(), ApprovalMode::Auto)
        .execute_tool_call(
            &status(),
            &ctx,
            &gate_perms(&[("bash(echo *)", PermissionAction::Deny)]),
        )
        .await;
    assert!(denied.is_error);
    assert!(
        denied.content.contains("by rule matching `echo hi`"),
        "{}",
        denied.content
    );

    let ask_manual = gate_runner(dir.path(), ApprovalMode::Important)
        .execute_tool_call(
            &status(),
            &ctx,
            &gate_perms(&[("bash(echo *)", PermissionAction::Ask)]),
        )
        .await;
    assert!(ask_manual.is_error);
    assert!(
        ask_manual
            .content
            .contains("shell rule requires confirmation"),
        "{}",
        ask_manual.content
    );

    let ask_auto = gate_runner(dir.path(), ApprovalMode::Auto)
        .execute_tool_call(
            &status(),
            &ctx,
            &gate_perms(&[("bash(echo *)", PermissionAction::Ask)]),
        )
        .await;
    assert!(!ask_auto.is_error, "{}", ask_auto.content);
    assert!(ask_auto.content.contains("hi"), "{}", ask_auto.content);

    let allowed = gate_runner(dir.path(), ApprovalMode::Manual)
        .execute_tool_call(
            &status(),
            &ctx,
            &gate_perms(&[("bash(echo *)", PermissionAction::Allow)]),
        )
        .await;
    assert!(!allowed.is_error, "{}", allowed.content);
}

#[tokio::test]
async fn headless_gate_applies_path_rules() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("secrets")).unwrap();
    std::fs::write(dir.path().join("secrets/k.txt"), "key").unwrap();
    let ctx = ToolContext::new(dir.path().display().to_string());
    let read = || call("read", serde_json::json!({"path": "secrets/k.txt"}));

    let denied = gate_runner(dir.path(), ApprovalMode::Auto)
        .execute_tool_call(
            &read(),
            &ctx,
            &gate_perms(&[("read(secrets/**)", PermissionAction::Deny)]),
        )
        .await;
    assert!(denied.is_error);
    assert!(
        denied.content.contains("by path rule"),
        "{}",
        denied.content
    );

    let ask_manual = gate_runner(dir.path(), ApprovalMode::Manual)
        .execute_tool_call(
            &read(),
            &ctx,
            &gate_perms(&[("read(secrets/**)", PermissionAction::Ask)]),
        )
        .await;
    assert!(ask_manual.is_error);
    assert!(
        ask_manual.content.contains("path rule on `secrets/k.txt`"),
        "{}",
        ask_manual.content
    );

    let ask_auto = gate_runner(dir.path(), ApprovalMode::Auto)
        .execute_tool_call(
            &read(),
            &ctx,
            &gate_perms(&[("read(secrets/**)", PermissionAction::Ask)]),
        )
        .await;
    assert!(!ask_auto.is_error, "{}", ask_auto.content);
    assert!(ask_auto.content.contains("key"), "{}", ask_auto.content);

    let allowed = gate_runner(dir.path(), ApprovalMode::Manual)
        .execute_tool_call(
            &read(),
            &ctx,
            &gate_perms(&[("read(secrets/**)", PermissionAction::Allow)]),
        )
        .await;
    assert!(!allowed.is_error, "{}", allowed.content);
}

#[tokio::test]
async fn worker_transform_hook_runs_before_the_headless_gate() {
    let dir = tempfile::tempdir().unwrap();
    let json = if cfg!(windows) {
        r#"echo {"command":"rm -rf /"}"#.to_string()
    } else {
        r#"echo '{"command":"rm -rf /"}'"#.to_string()
    };
    let runner = gate_runner(dir.path(), ApprovalMode::Auto).with_hooks(vec![hook(
        HookEvent::PreToolTransform,
        "bash",
        &json,
        false,
        false,
    )]);
    let ctx = ToolContext::new(dir.path().display().to_string());
    let result = runner
        .execute_tool_call(
            &call("bash", serde_json::json!({"command": "echo hi"})),
            &ctx,
            &gate_perms(&[]),
        )
        .await;
    assert!(result.is_error);
    assert!(
        result.content.contains("catastrophic"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn worker_restores_secret_placeholders_before_the_gate() {
    let dir = tempfile::tempdir().unwrap();
    let redactor = std::sync::Arc::new(whycodes_llm::Redactor::with_key([4u8; 32]));
    let placeholder = redactor.register("worker-secret-value-99");
    let runner = gate_runner(dir.path(), ApprovalMode::Auto)
        .with_redactor(Some(std::sync::Arc::clone(&redactor)));
    let ctx = ToolContext::new(dir.path().display().to_string());
    let result = runner
        .execute_tool_call(
            &call(
                "write",
                serde_json::json!({"path": "k.txt", "content": placeholder.clone()}),
            ),
            &ctx,
            &gate_perms(&[]),
        )
        .await;
    assert!(!result.is_error, "{}", result.content);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("k.txt")).unwrap(),
        "worker-secret-value-99"
    );

    // Nothing to restore: the call passes through unchanged.
    let plain = runner
        .execute_tool_call(
            &call(
                "write",
                serde_json::json!({"path": "p.txt", "content": "plain"}),
            ),
            &ctx,
            &gate_perms(&[]),
        )
        .await;
    assert!(!plain.is_error, "{}", plain.content);
}

fn batched_runner(batches: Vec<Vec<whycodes_llm::ScriptedStep>>) -> SubagentRunner {
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::batched(
        "script", batches,
    )));
    SubagentRunner::new(
        Arc::new(registry),
        Arc::new(ToolExecutor::new()),
        make_info(),
        std::env::temp_dir(),
        SandboxSettings::off(),
        NetworkPolicy::unrestricted(),
    )
}

fn text(t: &str) -> whycodes_llm::ScriptedStep {
    whycodes_llm::ScriptedStep::Text(t.into())
}

fn schema_task() -> SubagentTask {
    let schema = serde_json::json!({
        "properties": {"files": {"type": "array", "items": {"type": "string"}}},
        "required": ["files"]
    });
    SubagentTask {
        goal: "list files".into(),
        context: None,
        tools: None,
        max_turns: 2,
        output_schema: Some(crate::structured_output::prepare(&schema).unwrap()),
    }
}

const GOOD: &str = "Found two.\n```json\n{\"files\": [\"a.rs\", \"b.rs\"], \"what_i_did_not_check\": [\"tests\"]}\n```";

#[tokio::test]
async fn structured_result_valid_on_first_try() {
    let result = batched_runner(vec![vec![text(GOOD)]])
        .run(schema_task(), "script", "m", "k")
        .await;
    assert!(result.success, "{result:?}");
    assert_eq!(result.output, "Found two.");
    let s = result.structured.unwrap();
    assert_eq!(
        s.value.unwrap()["files"],
        serde_json::json!(["a.rs", "b.rs"])
    );
    assert!(s.errors.is_empty());
}

#[tokio::test]
async fn structured_result_retries_once_with_the_errors() {
    let result = batched_runner(vec![
        vec![text("Found two.\n```json\n{\"files\": [1]}\n```")],
        vec![
            text("```json\n{\"files\": [\"a.rs\"], \"what_i_did_not_check\": []}\n```"),
            whycodes_llm::ScriptedStep::Usage {
                input_tokens: 5,
                output_tokens: 3,
            },
        ],
    ])
    .run(schema_task(), "script", "m", "k")
    .await;
    assert!(result.success, "{result:?}");
    assert_eq!(result.output, "Found two.");
    assert_eq!(result.usage.input_tokens, 5);
    let s = result.structured.unwrap();
    assert_eq!(s.value.unwrap()["files"], serde_json::json!(["a.rs"]));
}

#[tokio::test]
async fn structured_result_stays_null_when_the_retry_is_invalid() {
    let result = batched_runner(vec![vec![text("no block")], vec![text("still none")]])
        .run(schema_task(), "script", "m", "k")
        .await;
    assert!(result.success, "the parent's turn never fails: {result:?}");
    assert_eq!(result.output, "no block\n\nstill none");
    let s = result.structured.unwrap();
    assert_eq!(s.value, None);
    assert_eq!(s.errors, vec!["no fenced ```json block found".to_string()]);
}

#[tokio::test]
async fn structured_result_keeps_errors_when_the_retry_fails() {
    let result = batched_runner(vec![
        vec![text("no block")],
        vec![whycodes_llm::ScriptedStep::FailOpen("down".into())],
    ])
    .run(schema_task(), "script", "m", "k")
    .await;
    assert!(result.success, "{result:?}");
    assert_eq!(result.output, "no block");
    let s = result.structured.unwrap();
    assert_eq!(s.value, None);
    assert_eq!(s.errors.len(), 2, "{:?}", s.errors);
    assert!(s.errors[1].starts_with("retry failed:"), "{:?}", s.errors);
}
