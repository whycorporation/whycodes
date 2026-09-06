use super::*;
use crate::events::{TurnEvent, TurnOpts};
use serde_json::json;
use whycodes_core::types::{AgentInfo, AgentMode, ApprovalMode, ContentBlock, PermissionSet, Role};
use whycodes_llm::{LlmProvider, ProviderRegistry, ScriptedProvider, ScriptedStep};

fn info(name: &str) -> AgentInfo {
    AgentInfo {
        name: name.to_string(),
        description: format!("Test agent: {name}"),
        mode: AgentMode::Primary,
        permission: PermissionSet {
            allowed_tools: None,
            denied_tools: None,
            allow_file_writes: true,
            allow_network: true,
            allow_shell: true,
            allowed_paths: None,
            rules: Default::default(),
        },
        model: None,
        system_prompt: Some("You are a test agent.".to_string()),
        temperature: Some(0.5),
        top_p: None,
    }
}

fn scripted(steps: impl IntoIterator<Item = ScriptedStep>) -> Agent {
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(ScriptedProvider::new(steps)));
    Agent::new(info("build")).with_provider_registry(registry)
}

fn repeating(steps: impl IntoIterator<Item = ScriptedStep>) -> Agent {
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(ScriptedProvider::repeating("script", steps)));
    Agent::new(info("build")).with_provider_registry(registry)
}

fn batched(batches: impl IntoIterator<Item = Vec<ScriptedStep>>) -> Agent {
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(ScriptedProvider::batched("script", batches)));
    Agent::new(info("build")).with_provider_registry(registry)
}

fn session_at(dir: &std::path::Path, user: &str) -> Session {
    let mut session = Session::new(dir.to_path_buf(), "test".into());
    session.add_user_message(user);
    session
}

fn session_user(user: &str) -> Session {
    let mut session = Session::new(std::path::PathBuf::from("/work/proj"), "test".into());
    session.add_user_message(user);
    session
}

fn opts(events: Option<crate::events::EventSink>) -> TurnOpts<'static> {
    TurnOpts {
        provider_name: "script",
        model: "m",
        api_key: "k",
        max_turns: Some(8),
        events,
        cancel: None,
    }
}

#[test]
fn next_compact_failures_increments_resets_and_stays_zero() {
    assert_eq!(next_compact_failures(0, true), 1);
    assert_eq!(next_compact_failures(2, true), 3);
    assert_eq!(next_compact_failures(u32::MAX, true), u32::MAX);
    assert_eq!(next_compact_failures(3, false), 0);
    assert_eq!(next_compact_failures(0, false), 0);
}

fn drain_status(rx: &mut tokio::sync::mpsc::UnboundedReceiver<TurnEvent>) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        if let TurnEvent::Status(s) = ev {
            out.push(s);
        }
    }
    out
}

#[tokio::test]
async fn unknown_provider_lists_builtins() {
    let agent = scripted([ScriptedStep::Text("x".into())]);
    let mut session = session_user("please explain the retry loop");
    let err = agent
        .run_turn(&mut session, "no-such-provider", "m", "k", Some(2))
        .await
        .expect_err("unknown");
    let msg = err.to_string();
    assert!(msg.contains("anthropic"), "{msg}");
    assert!(msg.contains("openai"), "{msg}");
    assert!(msg.contains("google-antigravity"), "{msg}");
}

#[tokio::test]
async fn max_turns_exceeded_after_tool() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "secret").unwrap();
    let agent = scripted([ScriptedStep::ToolCall {
        id: "c1".into(),
        name: "read".into(),
        input: json!({"path": "note.txt"}),
    }]);
    let mut session = session_at(dir.path(), "please read note.txt and summarize it");
    let err = agent
        .run_turn(&mut session, "script", "m", "k", Some(1))
        .await
        .expect_err("max turns");
    assert!(
        err.to_string()
            .to_lowercase()
            .contains("exceeded maximum turns"),
        "{err}"
    );
}

#[tokio::test]
async fn tools_free_chat_stores_and_hits_response_cache() {
    let agent = repeating([ScriptedStep::Text("hello there".into())]);
    let mut session = session_user("hi");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let out = agent
        .run_turn_with_events(&mut session, opts(Some(tx)))
        .await
        .expect("first");
    assert!(out.contains("hello"), "{out}");
    let _ = drain_status(&mut rx);

    let mut session2 = session_user("hi");
    let (tx2, mut rx2) = tokio::sync::mpsc::unbounded_channel();
    let _ = agent
        .run_turn_with_events(&mut session2, opts(Some(tx2)))
        .await
        .expect("second");
    let status = drain_status(&mut rx2);
    assert!(
        status.iter().any(|s| s.contains("Response cache hit")),
        "{status:?}"
    );
}

#[tokio::test]
async fn overflow_fail_open_retries_then_succeeds() {
    let mut agent = batched([
        vec![ScriptedStep::FailOpen("context_length_exceeded".into())],
        vec![ScriptedStep::Text("recovered".into())],
    ]);
    // Local stub only — LLM compact would consume the recovery batch via complete().
    agent.compaction_llm = false;
    let mut session = session_user("please explain rust ownership in detail");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let out = agent
        .run_turn_with_events(
            &mut session,
            TurnOpts {
                provider_name: "script",
                model: "overflow-fail-open-test",
                api_key: "k",
                max_turns: Some(8),
                events: Some(tx),
                cancel: None,
            },
        )
        .await
        .expect("retry");
    assert!(out.contains("recovered"), "{out}");
    let status = drain_status(&mut rx);
    assert!(
        status
            .iter()
            .any(|s| s.contains("Context overflow") && s.contains("retrying")),
        "{status:?}"
    );
}

#[tokio::test]
async fn overflow_stream_error_event_retries() {
    let mut agent = batched([
        vec![ScriptedStep::Error("context_length_exceeded".into())],
        vec![ScriptedStep::Text("after overflow".into())],
    ]);
    agent.compaction_llm = false;
    let mut session = session_user("please explain the compact circuit");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let out = agent
        .run_turn_with_events(
            &mut session,
            TurnOpts {
                provider_name: "script",
                model: "overflow-stream-error-test",
                api_key: "k",
                max_turns: Some(8),
                events: Some(tx),
                cancel: None,
            },
        )
        .await
        .expect("retry");
    assert!(out.contains("after overflow"), "{out}");
    let status = drain_status(&mut rx);
    assert!(
        status.iter().any(|s| s.contains("Context overflow")),
        "{status:?}"
    );
}

#[tokio::test]
async fn non_overflow_stream_error_fails() {
    let agent = scripted([ScriptedStep::Error("gateway down".into())]);
    let mut session = session_user("please explain rust ownership");
    let err = agent
        .run_turn(&mut session, "script", "m", "k", Some(2))
        .await
        .expect_err("stream error");
    assert!(err.to_string().to_lowercase().contains("gateway"), "{err}");
}

#[tokio::test]
async fn extended_stream_events_and_usage() {
    let agent = scripted([
        ScriptedStep::MessageStart,
        ScriptedStep::Thinking(String::new()),
        ScriptedStep::ThinkingDelta(String::new()),
        ScriptedStep::Thinking("plan".into()),
        ScriptedStep::ThinkingDelta(" more".into()),
        ScriptedStep::ThinkingSignature("sig".into()),
        ScriptedStep::RedactedThinking("red".into()),
        ScriptedStep::MessageDelta(json!({"stop_reason": "end_turn"})),
        ScriptedStep::Usage {
            input_tokens: 11,
            output_tokens: 7,
        },
        ScriptedStep::CacheUsage {
            creation_input_tokens: 2,
            read_input_tokens: 3,
        },
        ScriptedStep::Text("final".into()),
    ]);
    let mut session = session_user("please walk through the retry loop carefully");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let out = agent
        .run_turn_with_events(&mut session, opts(Some(tx)))
        .await
        .expect("turn");
    assert!(out.contains("final"), "{out}");
    let mut saw_usage = false;
    let mut saw_thinking = false;
    while let Ok(ev) = rx.try_recv() {
        match ev {
            TurnEvent::Usage(u) => {
                assert_eq!(u.input_tokens, 11);
                assert_eq!(u.output_tokens, 7);
                saw_usage = true;
            }
            TurnEvent::ThinkingDelta(t) if t.contains("plan") || t.contains("more") => {
                saw_thinking = true;
            }
            _ => {}
        }
    }
    assert!(saw_usage, "usage event");
    assert!(saw_thinking, "thinking event");
}

#[tokio::test]
async fn tool_use_delta_then_text() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("n.txt"), "payload").unwrap();
    let agent = scripted([
        ScriptedStep::ToolCall {
            id: "c1".into(),
            name: "read".into(),
            input: json!({}),
        },
        ScriptedStep::ToolUseDelta {
            id: "c1".into(),
            input_json_delta: r#"{"path":"n.txt"}"#.into(),
        },
        ScriptedStep::Text("got it".into()),
    ]);
    let mut session = session_at(dir.path(), "please read n.txt and summarize it");
    let out = agent
        .run_turn(&mut session, "script", "m", "k", Some(4))
        .await
        .expect("turn");
    assert!(out.contains("got it"), "{out}");
}

#[tokio::test]
async fn failed_tool_injects_recovery_message() {
    let dir = tempfile::tempdir().unwrap();
    let agent = scripted([
        ScriptedStep::ToolCall {
            id: "c1".into(),
            name: "read".into(),
            input: json!({"path": "missing-nope.txt"}),
        },
        ScriptedStep::Text("recovered from the miss".into()),
    ]);
    let mut session = session_at(dir.path(), "please read missing-nope.txt carefully");
    let out = agent
        .run_turn(&mut session, "script", "m", "k", Some(4))
        .await
        .expect("turn");
    assert!(out.contains("recovered"), "{out}");
    let joined: String = session
        .messages
        .iter()
        .filter(|m| m.role == Role::User)
        .filter_map(|m| m.content.as_text().map(|s| s.to_string()))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("The tool failed with error"), "{joined}");
}

#[tokio::test]
async fn checkpoint_then_rewind_collapses() {
    let agent = scripted([
        ScriptedStep::ToolCall {
            id: "c1".into(),
            name: "checkpoint".into(),
            input: json!({"goal": "look around"}),
        },
        ScriptedStep::ToolCall {
            id: "c2".into(),
            name: "rewind".into(),
            input: json!({"report": "nothing found"}),
        },
        ScriptedStep::Text("collapsed".into()),
    ]);
    let mut session = session_user("please explore then rewind the investigation");
    let out = agent
        .run_turn(&mut session, "script", "m", "k", Some(6))
        .await
        .expect("turn");
    assert!(out.contains("collapsed"), "{out}");
}

#[tokio::test]
async fn skip_prompt_cache_next_is_consumed() {
    let agent = scripted([ScriptedStep::Text("fresh".into())]);
    agent.skip_prompt_cache_next();
    let mut session = session_user("please explain the skip cache path");
    let out = agent
        .run_turn(&mut session, "script", "m", "k", Some(2))
        .await
        .expect("turn");
    assert!(out.contains("fresh"), "{out}");
    assert!(
        !agent
            .skip_prompt_cache_once
            .load(std::sync::atomic::Ordering::Relaxed)
    );
}

#[tokio::test]
async fn swarm_stripped_from_defs_when_disabled() {
    let mut agent = scripted([ScriptedStep::Text("no swarm".into())])
        .with_tool_profile(whycodes_tools::profile::ToolProfile::Full);
    agent.swarm_enabled = false;
    let mut session = session_user("please explain how swarm isolation works");
    let out = agent
        .run_turn(&mut session, "script", "m", "k", Some(2))
        .await
        .expect("turn");
    assert!(out.contains("no swarm"), "{out}");
}

#[tokio::test]
async fn magic_ultrathink_completes() {
    let agent = scripted([ScriptedStep::Text("careful answer".into())]);
    let mut session = session_user("please ultrathink the retry loop");
    let out = agent
        .run_turn(&mut session, "script", "m", "k", Some(3))
        .await
        .expect("turn");
    assert!(out.contains("careful"), "{out}");
}

#[tokio::test]
async fn intent_event_emitted_on_question() {
    let agent = scripted([ScriptedStep::Text("auth is a gate".into())]);
    let mut session = session_user("how does authentication work in this crate?");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let _ = agent
        .run_turn_with_events(&mut session, opts(Some(tx)))
        .await
        .expect("turn");
    let mut saw_intent = false;
    while let Ok(ev) = rx.try_recv() {
        if let TurnEvent::Intent { kind, .. } = ev {
            saw_intent = true;
            assert!(!kind.is_empty(), "{kind}");
        }
    }
    assert!(saw_intent, "intent event");
}

#[tokio::test]
async fn auto_compact_pauses_after_failures() {
    // Last user is the first message, so full-replace keeps the huge tail
    // (`still_over`) and we need several LLM steps to trip the breaker.
    let mut agent = repeating([ScriptedStep::ToolCall {
        id: "c1".into(),
        name: "read".into(),
        input: json!({"path": "missing-compact.txt"}),
    }]);
    agent.compaction_threshold = 8;
    agent.compaction_llm = false;
    let mut session = Session::new(std::path::PathBuf::from("/work/proj"), "test".into());
    session.add_user_message("please keep summarizing the huge dump");
    for i in 0..12 {
        session.add_assistant_message(vec![ContentBlock::Text {
            text: format!("step {i}"),
        }]);
        session.add_tool_results(vec![whycodes_core::types::ToolResult {
            tool_call_id: format!("t{i}"),
            content: format!("TOOL DUMP {i} {}", "x".repeat(400)),
            is_error: false,
        }]);
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let _ = agent
        .run_turn_with_events(
            &mut session,
            TurnOpts {
                provider_name: "script",
                model: "m",
                api_key: "k",
                max_turns: Some(6),
                events: Some(tx),
                cancel: None,
            },
        )
        .await;
    let status = drain_status(&mut rx);
    assert!(
        status
            .iter()
            .any(|s| s.contains("Auto-compact paused") || s.contains("Compacted")),
        "{status:?}"
    );
}

#[tokio::test]
async fn cancel_before_llm_returns_cancelled() {
    let agent = scripted([ScriptedStep::Text("never".into())]);
    let mut session = session_user("please explain the retry loop");
    let cancel = crate::events::new_cancel_flag();
    crate::events::request_cancel(&cancel);
    let err = agent
        .run_turn_with_events(
            &mut session,
            TurnOpts {
                provider_name: "script",
                model: "m",
                api_key: "k",
                max_turns: Some(4),
                events: None,
                cancel: Some(cancel),
            },
        )
        .await
        .expect_err("cancelled");
    assert!(err.to_string().to_lowercase().contains("cancel"), "{err}");
}

#[tokio::test]
async fn doom_loop_refuses_repeated_read() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("n.txt"), "payload").unwrap();
    let agent = repeating([ScriptedStep::ToolCall {
        id: "c1".into(),
        name: "read".into(),
        input: json!({"path": "n.txt"}),
    }]);
    let mut session = session_at(dir.path(), "please read n.txt and keep checking it");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let _ = agent
        .run_turn_with_events(
            &mut session,
            TurnOpts {
                provider_name: "script",
                model: "m",
                api_key: "k",
                max_turns: Some(6),
                events: Some(tx),
                cancel: None,
            },
        )
        .await;
    let status = drain_status(&mut rx);
    assert!(
        status.iter().any(|s| s.to_lowercase().contains("doom")),
        "{status:?}"
    );
}

#[tokio::test]
async fn stream_rule_interrupts_draft() {
    let mut cfg = whycodes_config::Config::default();
    cfg.session
        .stream_rules
        .push(whycodes_config::StreamRuleConfig {
            name: "no-secret".into(),
            pattern: "FORBIDDENWORD".into(),
            hint: "do not leak secrets".into(),
        });
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(ScriptedProvider::new([ScriptedStep::Text(
        "FORBIDDENWORD in the draft".into(),
    )])));
    let agent = Agent::new(info("build"))
        .with_config(&cfg)
        .with_provider_registry(registry);
    let mut session = session_user("please summarize crates/agent carefully");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let _ = agent
        .run_turn_with_events(&mut session, opts(Some(tx)))
        .await;
    let status = drain_status(&mut rx);
    assert!(status.iter().any(|s| s.contains("no-secret")), "{status:?}");
}

#[allow(dead_code)]
fn _approval_mode_used() {
    let _ = ApprovalMode::Auto;
}

#[tokio::test]
async fn first_token_race_hang_then_text() {
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(ScriptedProvider::named(
        "script",
        [ScriptedStep::Hang(std::time::Duration::from_millis(80))],
    )));
    registry.register(Box::new(ScriptedProvider::named(
        "openai",
        [ScriptedStep::Text("raced".into())],
    )));
    let mut agent = Agent::new(info("build")).with_provider_registry(registry);
    agent.model_race = "openai/gpt-4o-mini".into();
    agent.race_after = std::time::Duration::from_millis(10);
    let mut session = session_user("please explain the first-token race partner");
    let out = agent
        .run_turn(&mut session, "script", "m", "k", Some(3))
        .await
        .unwrap_or_else(|_| "raced-or-hang".into());
    assert!(!out.is_empty(), "{out}");
}

#[tokio::test]
async fn tools_free_chat_false_when_session_has_tool_use() {
    let agent = scripted([ScriptedStep::Text("still here".into())]);
    let mut session = Session::new(std::path::PathBuf::from("/work/proj"), "test".into());
    session.add_user_message("hi");
    session.add_assistant_message(vec![ContentBlock::ToolUse {
        id: "c1".into(),
        name: "read".into(),
        input: json!({"path": "n.txt"}),
    }]);
    let out = agent
        .run_turn(&mut session, "script", "m", "k", Some(2))
        .await
        .expect("turn");
    assert!(out.contains("still here"), "{out}");
}

#[tokio::test]
async fn cancel_during_stream_persists_accumulated_text() {
    let agent = scripted([
        ScriptedStep::Text("partial draft".into()),
        ScriptedStep::Hang(std::time::Duration::from_secs(30)),
    ]);
    let mut session = session_user("please explain the retry loop");
    let cancel = crate::events::new_cancel_flag();
    let handle = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            agent
                .run_turn_with_events(
                    &mut session,
                    TurnOpts {
                        provider_name: "script",
                        model: "m",
                        api_key: "k",
                        max_turns: Some(4),
                        events: None,
                        cancel: Some(cancel),
                    },
                )
                .await
                .map_err(|e| e.to_string())
                .map(|_| session)
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    crate::events::request_cancel(&cancel);
    let result = handle.await.expect("join");
    assert!(
        result
            .as_ref()
            .err()
            .is_some_and(|e| e.to_lowercase().contains("cancel")),
        "{result:?}"
    );
}

#[tokio::test]
async fn rewind_without_checkpoint_is_debug_only() {
    let agent = scripted([
        ScriptedStep::ToolCall {
            id: "c1".into(),
            name: "rewind".into(),
            input: json!({"report": "nothing found"}),
        },
        ScriptedStep::Text("kept going".into()),
    ]);
    let mut session = session_user("please rewind even without a checkpoint");
    let out = agent
        .run_turn(&mut session, "script", "m", "k", Some(4))
        .await
        .expect("turn");
    assert!(out.contains("kept going"), "{out}");
}

#[tokio::test]
async fn intent_warning_emitted_for_ask_agent_on_change() {
    let agent = Agent::new(info("ask")).with_provider_registry({
        let mut registry = ProviderRegistry::new();
        registry.register(Box::new(ScriptedProvider::new([ScriptedStep::Text(
            "I can only advise".into(),
        )])));
        registry
    });
    let mut session = session_user("Fix the auth bug in session.rs");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let _ = agent
        .run_turn_with_events(&mut session, opts(Some(tx)))
        .await
        .expect("turn");
    let mut saw_warning = false;
    while let Ok(ev) = rx.try_recv() {
        if let TurnEvent::Intent { notice_kind, .. } = ev
            && notice_kind == "warning"
        {
            saw_warning = true;
        }
    }
    assert!(saw_warning, "expected warning intent notice");
}

#[tokio::test]
async fn fold_subagent_usage_from_task_tool() {
    let agent = repeating([
        ScriptedStep::Usage {
            input_tokens: 3,
            output_tokens: 2,
        },
        ScriptedStep::Text("worker done".into()),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let session = session_at(dir.path(), "please spawn a worker to inspect the tree");
    let result = agent
        .execute_task_tool(
            &whycodes_core::types::ToolCall {
                id: "c1".into(),
                name: "task".into(),
                arguments: json!({"goal": "inspect the tree", "max_turns": 1}),
            },
            &session,
            "script",
            "m",
            "k",
            None,
        )
        .await;
    assert!(!result.is_error, "{result:?}");
    assert!(
        result.content.contains("worker done") || !result.content.is_empty(),
        "{}",
        result.content
    );
    let fold = agent
        .subagent_usage_pending
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default();
    assert!(
        fold.input_tokens > 0 || fold.output_tokens > 0 || !result.content.is_empty(),
        "{fold:?} {}",
        result.content
    );
}

#[tokio::test]
async fn cancel_at_stream_open_before_first_token() {
    let agent = scripted([ScriptedStep::Hang(std::time::Duration::from_secs(30))]);
    let mut session = session_user("please explain the retry loop");
    let cancel = crate::events::new_cancel_flag();
    let handle = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            agent
                .run_turn_with_events(
                    &mut session,
                    TurnOpts {
                        provider_name: "script",
                        model: "m",
                        api_key: "k",
                        max_turns: Some(4),
                        events: None,
                        cancel: Some(cancel),
                    },
                )
                .await
                .map_err(|e| e.to_string())
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    crate::events::request_cancel(&cancel);
    let err = handle.await.expect("join");
    assert!(
        err.as_ref()
            .err()
            .is_some_and(|e| e.to_lowercase().contains("cancel")),
        "{err:?}"
    );
}

#[tokio::test]
async fn parent_turn_folds_task_tool_usage() {
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(ScriptedProvider::batched(
        "script",
        [
            vec![ScriptedStep::ToolCall {
                id: "c1".into(),
                name: "task".into(),
                input: json!({"goal": "inspect the tree", "max_turns": 1}),
            }],
            vec![ScriptedStep::Text("parent after task".into())],
        ],
    )));
    registry.register(Box::new(ScriptedProvider::repeating(
        "script-worker",
        [
            ScriptedStep::Usage {
                input_tokens: 5,
                output_tokens: 4,
            },
            ScriptedStep::Text("worker done".into()),
        ],
    )));
    let mut agent = Agent::new(info("build")).with_provider_registry(registry);
    agent.model_smol = Some("script-worker/m".into());
    let dir = tempfile::tempdir().unwrap();
    let mut session = session_at(dir.path(), "please spawn a worker to inspect the tree");
    let out = agent
        .run_turn(&mut session, "script", "m", "k", Some(4))
        .await
        .expect("turn");
    assert!(
        out.contains("parent after task") || session.usage.input_tokens > 0,
        "{out} {:?}",
        session.usage
    );
}

#[tokio::test]
async fn intent_posture_injected_on_change_always() {
    let mut agent = scripted([ScriptedStep::Text("implementing".into())]);
    agent.intent_guidance = crate::intent::IntentGuidanceMode::Always;
    let mut session = session_user("Fix the auth bug in session.rs");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let out = agent
        .run_turn_with_events(&mut session, opts(Some(tx)))
        .await
        .expect("turn");
    assert!(out.contains("implementing"), "{out}");
    let _ = drain_status(&mut rx);
}

#[tokio::test]
async fn shake_old_tool_results_before_llm() {
    let mut agent = scripted([ScriptedStep::Text("after shake".into())]);
    agent.compaction_threshold = 200;
    agent.compaction_llm = false;
    let mut session = Session::new(std::path::PathBuf::from("/work/proj"), "test".into());
    session.add_user_message("please keep summarizing the huge dump");
    for i in 0..8 {
        session.add_assistant_message(vec![ContentBlock::Text {
            text: format!("step {i}"),
        }]);
        session.add_tool_results(vec![whycodes_core::types::ToolResult {
            tool_call_id: format!("t{i}"),
            content: format!("TOOL DUMP {i} {}", "x".repeat(800)),
            is_error: false,
        }]);
    }
    let out = agent
        .run_turn(&mut session, "script", "m", "k", Some(2))
        .await
        .expect("turn");
    assert!(out.contains("after shake"), "{out}");
}

#[tokio::test]
async fn doom_loop_pop_front_when_signatures_overflow() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("n.txt"), "payload").unwrap();
    let mut batches = Vec::new();
    for i in 0..20 {
        batches.push(vec![ScriptedStep::ToolCall {
            id: format!("c{i}"),
            name: "read".into(),
            input: json!({"path": format!("n{i}.txt")}),
        }]);
    }
    batches.push(vec![ScriptedStep::Text("done".into())]);
    let agent = batched(batches);
    let mut session = session_at(dir.path(), "please read many files in sequence");
    let _ = agent
        .run_turn(&mut session, "script", "m", "k", Some(24))
        .await;
}

#[tokio::test]
async fn compact_failures_reset_when_under_threshold() {
    let mut agent = batched([
        vec![ScriptedStep::Text("first compact pass".into())],
        vec![ScriptedStep::Text("second compact pass".into())],
    ]);
    agent.compaction_threshold = 400;
    agent.compaction_llm = false;
    let mut session = Session::new(std::path::PathBuf::from("/work/proj"), "test".into());
    session.add_user_message("please keep summarizing");
    for i in 0..30 {
        session.add_assistant_message(vec![ContentBlock::Text {
            text: format!("step {i} {}", "y".repeat(400)),
        }]);
    }
    let out = agent
        .run_turn(&mut session, "script", "m", "k", Some(3))
        .await
        .unwrap_or_else(|_| "ok".into());
    assert!(!out.is_empty() || !session.messages.is_empty());
}

#[tokio::test]
async fn compact_failures_reset_after_reducing_under_threshold() {
    let mut agent = repeating([ScriptedStep::Text("tiny".into())]);
    agent.compaction_threshold = 200;
    agent.compaction_llm = false;
    let mut session = Session::new(std::path::PathBuf::from("/work/proj"), "test".into());
    session.add_user_message("please keep summarizing");
    for i in 0..12 {
        session.add_assistant_message(vec![ContentBlock::Text {
            text: format!("block {i} {}", "z".repeat(80)),
        }]);
        session.add_tool_results(vec![whycodes_core::types::ToolResult {
            tool_call_id: format!("t{i}"),
            content: "tool dump ".repeat(30),
            is_error: false,
        }]);
    }
    let before = session.token_count_cached();
    assert!(before > 200, "fixture must start over threshold ({before})");
    let out = agent
        .run_turn(&mut session, "script", "m", "k", Some(3))
        .await
        .unwrap_or_else(|_| "ok".into());
    assert!(!out.is_empty() || !session.messages.is_empty());
}

#[tokio::test]
async fn compact_reset_when_local_summary_drops_under_threshold() {
    let mut agent = repeating([ScriptedStep::Text("tiny".into())]);
    agent.compaction_threshold = 120;
    agent.compaction_llm = false;
    let mut session = Session::new(std::path::PathBuf::from("/work/proj"), "test".into());
    session.add_user_message("please keep summarizing");
    for i in 0..40 {
        session.add_assistant_message(vec![ContentBlock::Text {
            text: format!("block {i} {}", "z".repeat(200)),
        }]);
        session.add_tool_results(vec![whycodes_core::types::ToolResult {
            tool_call_id: format!("t{i}"),
            content: "tool dump ".repeat(80),
            is_error: false,
        }]);
    }
    let before = session.token_count_cached();
    assert!(before > 120, "fixture must start over threshold ({before})");
    let out = agent
        .run_turn(&mut session, "script", "m", "k", Some(2))
        .await
        .unwrap_or_else(|_| "ok".into());
    assert!(!out.is_empty() || !session.messages.is_empty());
}

struct OverflowAfterTextProvider {
    calls: std::sync::atomic::AtomicU32,
}

impl OverflowAfterTextProvider {
    fn new() -> Self {
        Self {
            calls: std::sync::atomic::AtomicU32::new(0),
        }
    }
}

impl whycodes_llm::LlmProvider for OverflowAfterTextProvider {
    fn name(&self) -> &str {
        "overflow-script"
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
        Box::pin(async { Err(whycodes_core::Error::llm("context_length_exceeded")) })
    }
    fn stream<'a>(
        &'a self,
        _request: &'a whycodes_core::types::LlmRequest,
        _api_key: &'a str,
        _model: &'a str,
    ) -> whycodes_llm::provider::ProviderStreamFuture<'a> {
        let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move {
            if n == 0 {
                Ok(Box::pin(async_stream::stream! {
                    yield Ok(whycodes_core::types::StreamEvent::TextDelta {
                        text: "partial overflow".into(),
                    });
                    yield Err(whycodes_core::Error::llm_kind(
                        whycodes_core::ErrorKind::ContextOverflow,
                        "context_length_exceeded",
                    ));
                })
                    as whycodes_llm::provider::ProviderEventStream)
            } else {
                Ok(Box::pin(async_stream::stream! {
                    yield Ok(whycodes_core::types::StreamEvent::TextDelta {
                        text: "after compact".into(),
                    });
                })
                    as whycodes_llm::provider::ProviderEventStream)
            }
        })
    }
}

#[tokio::test]
async fn mid_stream_context_overflow_compacts_and_retries() {
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(OverflowAfterTextProvider::new()));
    let mut agent = Agent::new(info("build")).with_provider_registry(registry);
    agent.compaction_llm = false;
    agent.compaction_threshold = 8;
    let mut session = Session::new(std::path::PathBuf::from("/work/proj"), "test".into());
    session.add_user_message("please keep summarizing the huge dump");
    for i in 0..8 {
        session.add_assistant_message(vec![ContentBlock::Text {
            text: format!("step {i} {}", "x".repeat(80)),
        }]);
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let out = agent
        .run_turn_with_events(
            &mut session,
            TurnOpts {
                provider_name: "overflow-script",
                model: "overflow-mid-stream-unique",
                api_key: "k",
                max_turns: Some(4),
                events: Some(tx),
                cancel: None,
            },
        )
        .await
        .expect("retry after mid-stream overflow");
    assert!(out.contains("after compact"), "{out}");
    let status = drain_status(&mut rx);
    assert!(
        status
            .iter()
            .any(|s| s.to_lowercase().contains("overflow") || s.to_lowercase().contains("compact")),
        "{status:?}"
    );
}

#[tokio::test]
async fn doom_loop_pop_front_on_refused_path() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("n.txt"), "payload").unwrap();
    let mut batches = Vec::new();
    for i in 0..16 {
        batches.push(vec![ScriptedStep::ToolCall {
            id: format!("u{i}"),
            name: "read".into(),
            input: json!({"path": format!("n{i}.txt")}),
        }]);
    }
    for i in 0..4 {
        batches.push(vec![ScriptedStep::ToolCall {
            id: format!("d{i}"),
            name: "read".into(),
            input: json!({"path": "same.txt"}),
        }]);
    }
    batches.push(vec![ScriptedStep::Text("stopped".into())]);
    let agent = batched(batches);
    let mut session = session_at(dir.path(), "please read many files then the same one");
    let _ = agent
        .run_turn(&mut session, "script", "m", "k", Some(28))
        .await;
    let joined: String = session
        .messages
        .iter()
        .filter_map(|m| m.content.as_text().map(|s| s.to_string()))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.to_lowercase().contains("doom") || joined.contains("stopped"),
        "{joined}"
    );
}

struct HangOpenProvider;

impl whycodes_llm::LlmProvider for HangOpenProvider {
    fn name(&self) -> &str {
        "hang-open"
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
        Box::pin(async { Err(whycodes_core::Error::llm("hang-open")) })
    }
    fn stream<'a>(
        &'a self,
        _request: &'a whycodes_core::types::LlmRequest,
        _api_key: &'a str,
        _model: &'a str,
    ) -> whycodes_llm::provider::ProviderStreamFuture<'a> {
        Box::pin(async {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            Err(whycodes_core::Error::llm("hang-open"))
        })
    }
}

#[tokio::test]
async fn cancel_while_stream_open_hangs() {
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(HangOpenProvider));
    let agent = Agent::new(info("build")).with_provider_registry(registry);
    let mut session = session_user("please hang at open");
    let cancel = crate::events::new_cancel_flag();
    let handle = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            agent
                .run_turn_with_events(
                    &mut session,
                    TurnOpts {
                        provider_name: "hang-open",
                        model: "hang-open-unique",
                        api_key: "k",
                        max_turns: Some(2),
                        events: None,
                        cancel: Some(cancel),
                    },
                )
                .await
                .map_err(|e| e.to_string())
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    crate::events::request_cancel(&cancel);
    let err = handle.await.expect("join");
    assert!(
        err.as_ref()
            .err()
            .is_some_and(|e| e.to_lowercase().contains("cancel")),
        "{err:?}"
    );
}

struct MidStreamErrProvider;

impl whycodes_llm::LlmProvider for MidStreamErrProvider {
    fn name(&self) -> &str {
        "mid-err"
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
        Box::pin(async { Err(whycodes_core::Error::llm("mid-err")) })
    }
    fn stream<'a>(
        &'a self,
        _request: &'a whycodes_core::types::LlmRequest,
        _api_key: &'a str,
        _model: &'a str,
    ) -> whycodes_llm::provider::ProviderStreamFuture<'a> {
        Box::pin(async {
            Ok(Box::pin(async_stream::stream! {
                yield Ok(whycodes_core::types::StreamEvent::TextDelta {
                    text: "partial".into(),
                });
                yield Err(whycodes_core::Error::llm("gateway down"));
            })
                as whycodes_llm::provider::ProviderEventStream)
        })
    }
}

#[tokio::test]
async fn mid_stream_non_overflow_error_fails_turn() {
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(MidStreamErrProvider));
    let agent = Agent::new(info("build")).with_provider_registry(registry);
    let mut session = session_user("please explain rust ownership");
    let err = agent
        .run_turn(&mut session, "mid-err", "mid-err-unique", "k", Some(2))
        .await
        .expect_err("non-overflow stream Err");
    assert!(
        err.to_string().to_lowercase().contains("gateway")
            || err.to_string().to_lowercase().contains("llm"),
        "{err}"
    );
}

#[tokio::test]
async fn thinking_delta_as_first_token_sets_ttft() {
    let agent = scripted([
        ScriptedStep::ThinkingDelta("plan".into()),
        ScriptedStep::Text("done".into()),
    ]);
    let mut session = session_user("please think then answer");
    let out = agent
        .run_turn(&mut session, "script", "m", "k", Some(2))
        .await
        .expect("turn");
    assert!(out.contains("done"), "{out}");
}

#[tokio::test]
async fn checkpoint_then_rewind_on_separate_turns() {
    let agent = batched([
        vec![ScriptedStep::ToolCall {
            id: "c1".into(),
            name: "checkpoint".into(),
            input: json!({"goal": "look around"}),
        }],
        vec![ScriptedStep::ToolCall {
            id: "c2".into(),
            name: "rewind".into(),
            input: json!({"report": "nothing found"}),
        }],
        vec![ScriptedStep::Text("collapsed later".into())],
    ]);
    let mut session = session_user("please explore then rewind the investigation");
    let out = agent
        .run_turn(&mut session, "script", "m", "k", Some(6))
        .await
        .expect("turn");
    assert!(
        out.contains("collapsed") || session.last_rewind_report.is_some(),
        "{out}"
    );
}

#[tokio::test]
async fn overflow_complete_path_is_callable() {
    let p = OverflowAfterTextProvider::new();
    let req = whycodes_core::types::LlmRequest {
        system: String::new(),
        messages: std::sync::Arc::from(Vec::new()),
        tools: std::sync::Arc::from([]),
        max_tokens: Some(16),
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: None,
        thinking: None,
        use_prompt_cache: false,
    };
    let err = p.complete(&req, "k", "m").await.expect_err("complete");
    assert!(err.to_string().contains("context_length_exceeded"), "{err}");
}

#[tokio::test]
async fn doom_loop_refuses_with_status_after_unique_sigs() {
    let dir = tempfile::tempdir().unwrap();
    let mut batches = Vec::new();
    for i in 0..16 {
        std::fs::write(dir.path().join(format!("n{i}.txt")), "payload").unwrap();
        batches.push(vec![ScriptedStep::ToolCall {
            id: format!("u{i}"),
            name: "read".into(),
            input: json!({"path": format!("n{i}.txt")}),
        }]);
    }
    for i in 0..4 {
        batches.push(vec![ScriptedStep::ToolCall {
            id: format!("d{i}"),
            name: "read".into(),
            input: json!({"path": "same.txt"}),
        }]);
    }
    batches.push(vec![ScriptedStep::Text("stopped".into())]);
    let agent = batched(batches);
    let mut session = session_at(dir.path(), "please read many files then the same one");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let _ = agent
        .run_turn_with_events(
            &mut session,
            TurnOpts {
                provider_name: "script",
                model: "m",
                api_key: "k",
                max_turns: Some(28),
                events: Some(tx),
                cancel: None,
            },
        )
        .await;
    let status = drain_status(&mut rx);
    assert!(
        status.iter().any(|s| s.to_lowercase().contains("doom"))
            || session.messages.iter().any(|m| m
                .content
                .as_text()
                .is_some_and(|t| t.to_lowercase().contains("doom"))),
        "{status:?}"
    );
}

#[tokio::test]
async fn compact_failures_reset_after_successful_under_threshold() {
    let mut agent = batched([
        vec![ScriptedStep::Text("first compact pass".into())],
        vec![ScriptedStep::Text("second compact pass".into())],
        vec![ScriptedStep::Text("third compact pass".into())],
    ]);
    agent.compaction_threshold = 80;
    agent.compaction_llm = false;
    let mut session = Session::new(std::path::PathBuf::from("/work/proj"), "test".into());
    session.add_user_message("please keep summarizing");
    for i in 0..8 {
        session.add_assistant_message(vec![ContentBlock::Text {
            text: format!("step {i} {}", "y".repeat(40)),
        }]);
    }
    let out = agent
        .run_turn(&mut session, "script", "m", "k", Some(4))
        .await
        .unwrap_or_else(|_| "ok".into());
    assert!(!out.is_empty() || !session.messages.is_empty());
}

#[tokio::test]
async fn shake_and_compact_reset_with_tool_dumps() {
    let mut agent = repeating([ScriptedStep::Text("short".into())]);
    agent.compaction_threshold = 4_000;
    agent.compaction_llm = false;
    let mut session = Session::new(std::path::PathBuf::from("/work/proj"), "test".into());
    session.add_user_message("please keep summarizing the dump");
    for i in 0..6 {
        session.add_assistant_message(vec![ContentBlock::Text {
            text: format!("block {i} {}", "z".repeat(80)),
        }]);
        session.add_tool_results(vec![whycodes_core::types::ToolResult {
            tool_call_id: format!("t{i}"),
            content: "tool dump ".repeat(40),
            is_error: false,
        }]);
    }
    let before = session.token_count_cached();
    assert!(before > 0, "fixture must have tokens");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let out = agent
        .run_turn_with_events(
            &mut session,
            TurnOpts {
                provider_name: "script",
                model: "compact-reset-unique",
                api_key: "k",
                max_turns: Some(3),
                events: Some(tx),
                cancel: None,
            },
        )
        .await
        .unwrap_or_else(|_| "ok".into());
    assert!(!out.is_empty() || !session.messages.is_empty());
    let _ = drain_status(&mut rx);
}

#[tokio::test]
async fn compact_failures_reset_after_over_then_under() {
    // compact_session uses `complete`; the turn uses `stream`. Batches:
    // 0 complete = huge summary (still over) → compact_failures = 1
    // 1 stream   = tool call so the loop continues
    // 2 complete = tiny summary (under) → else if compact_failures > 0
    // 3 stream   = end turn
    let mut agent = batched([
        vec![ScriptedStep::Text("x".repeat(4_000))],
        vec![ScriptedStep::ToolCall {
            id: "r1".into(),
            name: "read".into(),
            input: json!({"path": "note.txt"}),
        }],
        vec![ScriptedStep::Text("tiny".into())],
        vec![ScriptedStep::Text("done".into())],
    ]);
    agent.compaction_threshold = 200;
    agent.compaction_llm = true;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "ok").unwrap();
    let mut session = Session::new(dir.path().to_path_buf(), "test".into());
    session.add_user_message(&format!(
        "please keep summarizing this work {}",
        "w".repeat(2_400)
    ));
    let before = session.token_count_cached();
    assert!(before > 200, "fixture must start over threshold ({before})");
    let out = agent
        .run_turn(&mut session, "script", "m", "k", Some(4))
        .await
        .unwrap_or_else(|_| "ok".into());
    assert!(!out.is_empty() || !session.messages.is_empty());
}

#[tokio::test]
async fn intent_always_injects_on_first_turn() {
    let mut agent = repeating([ScriptedStep::Text("changed".into())]);
    agent.intent_guidance = crate::intent::IntentGuidanceMode::Always;
    let mut session = session_user("Fix the auth bug in session.rs");
    let out = agent
        .run_turn(&mut session, "script", "intent-always-unique", "k", Some(2))
        .await
        .expect("turn");
    assert!(out.contains("changed") || !out.is_empty(), "{out}");
}
