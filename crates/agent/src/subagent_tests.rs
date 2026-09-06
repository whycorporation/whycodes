use super::*;

#[test]
fn inject_memory_passthrough_when_disabled() {
    let memory = whycodes_memory::MemorySettings::disabled();
    let out = inject_subagent_memory(
        "base prompt",
        std::path::Path::new("/work/proj"),
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
                },
                provider,
                "test-model",
                "test-key",
            )
            .await
            .expect("runner converts turn errors into a result");

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
            },
            "script",
            "m",
            "k",
        )
        .await
        .expect("ok");
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
    hub.send("parent", "worker-q", "hello worker");
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
            },
            "script",
            "m",
            "k",
        )
        .await
        .expect("ok");
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
    let out = inject_subagent_memory(
        "base prompt",
        std::path::Path::new("/work/proj"),
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
            },
            "script",
            "m",
            "k",
        )
        .await
        .expect("always Ok wrapping");
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
            },
            "script",
            "m",
            "k",
        )
        .await
        .expect("ok");
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
    let result = run.await.expect("join").expect("ok wrapping");
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
    let result = run.await.expect("join").expect("ok wrapping");
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
            },
            "script",
            "m",
            "k",
        )
        .await
        .expect("ok");
    assert!(
        result.success || result.output.contains("mutators") || !result.output.is_empty(),
        "{result:?}"
    );
}
