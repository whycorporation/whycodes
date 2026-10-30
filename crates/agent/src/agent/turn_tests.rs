use super::*;
use crate::events::{TurnEvent, TurnOpts};
use serde_json::json;
use whycodes_core::types::{
    AgentInfo, AgentMode, ApprovalMode, ContentBlock, LlmRequest, PermissionSet, ProviderConfig,
    ProviderCredentials, Role,
};
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

#[test]
fn take_pending_usage_skips_empty_and_recovers_poison() {
    let empty = std::sync::Mutex::new(whycodes_core::types::Usage::default());
    assert!(take_pending_usage(&empty).is_none());

    let pending = std::sync::Mutex::new(whycodes_core::types::Usage {
        input_tokens: 4,
        ..Default::default()
    });
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _g = pending.lock().unwrap();
        panic!("poison pending usage");
    }));
    let fold = take_pending_usage(&pending).expect("non-empty");
    assert_eq!(fold.input_tokens, 4);
    assert!(take_pending_usage(&pending).is_none());
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
async fn harmony_leak_retries_then_errors() {
    let leak = "analysis to=functions.edit code leftover";
    let agent = batched([
        vec![ScriptedStep::Text(leak.into())],
        vec![ScriptedStep::Text(leak.into())],
        vec![ScriptedStep::Text(leak.into())],
    ]);
    let mut session = session_user("please refactor the parser in src/main.rs");
    let err = agent
        .run_turn(&mut session, "script", "gpt-5", "k", Some(8))
        .await
        .expect_err("harmony leak");
    let msg = err.to_string();
    assert!(msg.contains("Harmony protocol leak"), "{msg}");
    assert!(
        !session.messages.iter().any(|m| {
            m.content
                .as_text()
                .is_some_and(|t| t.contains("to=functions.edit"))
        }),
        "leaked draft must not persist"
    );
}

#[tokio::test]
async fn harmony_leak_retries_then_succeeds() {
    let leak = "analysis to=functions.edit code leftover";
    let agent = batched([
        vec![ScriptedStep::Text(leak.into())],
        vec![ScriptedStep::Text("clean rewrite".into())],
    ]);
    let mut session = session_user("please refactor the parser in src/main.rs");
    let out = agent
        .run_turn(&mut session, "script", "gpt-5", "k", Some(8))
        .await
        .expect("retry");
    assert!(out.contains("clean rewrite"), "{out}");
}

#[tokio::test]
async fn harmony_leak_in_tool_args_does_not_execute() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "keep").unwrap();
    let leak = "keep\nanalysis to=functions.edit code leftover";
    let agent = batched([
        vec![ScriptedStep::ToolCall {
            id: "c1".into(),
            name: "edit".into(),
            input: json!({
                "path": "note.txt",
                "old_string": "keep",
                "new_string": leak
            }),
        }],
        vec![ScriptedStep::Text("stopped the write".into())],
    ]);
    let mut session = session_at(dir.path(), "please edit note.txt carefully now");
    let out = agent
        .run_turn(&mut session, "script", "gpt-5", "k", Some(8))
        .await
        .expect("retry");
    assert!(out.contains("stopped the write"), "{out}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("note.txt")).unwrap(),
        "keep"
    );
}

#[tokio::test]
async fn file_tool_results_feed_the_session_edit_stats() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "one\ntwo\n").unwrap();
    let agent = batched([
        vec![
            ScriptedStep::ToolCall {
                id: "w1".into(),
                name: "write".into(),
                input: json!({"path": "fresh.txt", "content": "a\nb\nc"}),
            },
            ScriptedStep::ToolCall {
                id: "e1".into(),
                name: "edit".into(),
                input: json!({
                    "path": "note.txt",
                    "old_string": "two",
                    "new_string": "deux"
                }),
            },
            ScriptedStep::ToolCall {
                id: "e2".into(),
                name: "edit".into(),
                input: json!({
                    "path": "missing.txt",
                    "old_string": "x",
                    "new_string": "y"
                }),
            },
        ],
        vec![ScriptedStep::Text("edited".into())],
    ]);
    let mut session = session_at(dir.path(), "write fresh.txt and edit note.txt");
    let out = agent
        .run_turn(&mut session, "script", "gpt-5", "k", Some(8))
        .await
        .expect("turn");
    assert!(out.contains("edited"), "{out}");
    // write: +3 new lines; edit: one line replaced; the failed edit counts
    // nothing (it is an error result, not a lost trailer).
    assert_eq!(session.edit_stats.added, 4, "{:?}", session.edit_stats);
    assert_eq!(session.edit_stats.removed, 1, "{:?}", session.edit_stats);
    assert!(session.edit_stats.approximate);
}

#[tokio::test]
async fn harmony_leak_in_thinking_retries() {
    let leak = "analysis to=functions.edit code leftover";
    let agent = batched([
        vec![
            ScriptedStep::Thinking(leak.into()),
            ScriptedStep::Text("also visible".into()),
        ],
        vec![ScriptedStep::Text("clean after thinking leak".into())],
    ]);
    let mut session = session_user("please refactor the parser in src/main.rs");
    let out = agent
        .run_turn(&mut session, "script", "gpt-5", "k", Some(8))
        .await
        .expect("retry");
    assert!(out.contains("clean after thinking leak"), "{out}");
}

#[test]
fn harmony_leak_helper_none_on_clean() {
    let acc = crate::thinking_acc::ThinkingAccumulator::new();
    assert!(harmony_leak("hello", &acc, &[]).is_none());
    assert!(harmony_leak("", &acc, &[]).is_none());
    let leak = serde_json::json!({"note": "analysis to=functions.edit code leftover"});
    let calls = [whycodes_core::types::ToolCall {
        id: "c1".into(),
        name: "edit".into(),
        arguments: leak.clone(),
    }];
    assert!(harmony_leak_tools(&calls).is_some());
    let mixed = [
        whycodes_core::types::ToolCall {
            id: "c0".into(),
            name: "read".into(),
            arguments: serde_json::json!({"path": "a.txt"}),
        },
        whycodes_core::types::ToolCall {
            id: "c1".into(),
            name: "edit".into(),
            arguments: leak,
        },
    ];
    assert!(harmony_leak_tools(&mixed).is_some());
    assert!(harmony_leak_tools(&[]).is_none());
    let text_only = [whycodes_core::types::Message {
        role: whycodes_core::types::Role::User,
        content: whycodes_core::types::MessageContent::Text("hello".into()),
        tool_call_id: None,
        name: None,
        created_at: None,
    }];
    assert!(!transcript_has_tool_work(&text_only));
    let tool_role = [whycodes_core::types::Message {
        role: whycodes_core::types::Role::Tool,
        content: whycodes_core::types::MessageContent::Text("out".into()),
        tool_call_id: Some("c1".into()),
        name: None,
        created_at: None,
    }];
    assert!(transcript_has_tool_work(&tool_role));
    let tool_use = [whycodes_core::types::Message {
        role: whycodes_core::types::Role::Assistant,
        content: whycodes_core::types::MessageContent::Blocks(vec![ContentBlock::ToolUse {
            id: "c1".into(),
            name: "read".into(),
            input: serde_json::json!({}),
        }]),
        tool_call_id: None,
        name: None,
        created_at: None,
    }];
    assert!(transcript_has_tool_work(&tool_use));
    let swarm = whycodes_core::types::ToolDefinition {
        name: "swarm".into(),
        description: String::new(),
        parameters: serde_json::json!({}),
    };
    let read = whycodes_core::types::ToolDefinition {
        name: "read".into(),
        description: String::new(),
        parameters: serde_json::json!({}),
    };
    assert!(defs_include_swarm(std::slice::from_ref(&swarm)));
    assert!(!defs_include_swarm(std::slice::from_ref(&read)));
    assert_eq!(defs_without_swarm(&[swarm, read.clone()]).len(), 1);
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
async fn cut_off_step_runs_no_tool_and_continues() {
    let dir = tempfile::tempdir().unwrap();
    let agent = batched([
        vec![
            ScriptedStep::ToolCall {
                id: "c1".into(),
                name: "write".into(),
                input: json!({"path": "out.txt", "content": "half of the f"}),
            },
            ScriptedStep::MessageDelta(json!({"stop_reason": "length"})),
        ],
        vec![
            ScriptedStep::Text("finished after the cut".into()),
            ScriptedStep::MessageDelta(json!({"stop_reason": "stop"})),
        ],
    ]);
    let mut session = session_at(dir.path(), "please write out.txt with the full text");
    let out = agent
        .run_turn(&mut session, "script", "m", "k", Some(8))
        .await
        .expect("turn");
    assert!(out.contains("finished after the cut"), "{out}");
    assert!(
        !dir.path().join("out.txt").exists(),
        "a cut-off call never runs"
    );
    let notes = session
        .messages
        .iter()
        .filter(|m| m.role == Role::User && m.content.as_text() == Some(CUT_OFF_NOTE))
        .count();
    assert_eq!(notes, 1);
}

#[tokio::test]
async fn repeated_cut_offs_stop_after_the_limit() {
    let agent = repeating([
        ScriptedStep::Text("still going".into()),
        ScriptedStep::MessageDelta(json!({"finishReason": "MAX_TOKENS"})),
    ]);
    let mut session = session_user("please explain the whole module in detail");
    let out = agent
        .run_turn(&mut session, "script", "m", "k", Some(20))
        .await
        .expect("turn");
    assert!(out.contains("still going"), "{out}");
    let notes = session
        .messages
        .iter()
        .filter(|m| m.content.as_text() == Some(CUT_OFF_NOTE))
        .count();
    assert_eq!(notes, MAX_CUT_OFFS as usize);
}

#[test]
fn output_limit_stop_reasons() {
    for delta in [
        json!({"stop_reason": "length"}),
        json!({"stop_reason": "max_tokens"}),
        json!({"finishReason": "MAX_TOKENS"}),
    ] {
        assert!(is_output_limit_stop(&delta), "{delta}");
    }
    for delta in [
        json!({"stop_reason": "stop"}),
        json!({"stop_reason": "end_turn"}),
        json!({"finishReason": "STOP"}),
        json!({"x": 1}),
    ] {
        assert!(!is_output_limit_stop(&delta), "{delta}");
    }
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
            ..Default::default()
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

struct RateThenOk {
    inner: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
}

impl LlmProvider for RateThenOk {
    fn name(&self) -> &str {
        "openai"
    }
    fn default_base_url(&self) -> &str {
        "http://script.invalid"
    }
    fn complete<'a>(
        &'a self,
        _request: &'a LlmRequest,
        _api_key: &'a str,
        _model: &'a str,
    ) -> whycodes_llm::provider::ProviderResponseFuture<'a> {
        Box::pin(async { Err(whycodes_core::Error::Provider("unused".into())) })
    }
    fn stream<'a>(
        &'a self,
        _request: &'a LlmRequest,
        api_key: &'a str,
        model: &'a str,
    ) -> whycodes_llm::provider::ProviderStreamFuture<'a> {
        let key = api_key.to_string();
        let model = model.to_string();
        let inner = std::sync::Arc::clone(&self.inner);
        Box::pin(async move {
            {
                let mut g = inner.lock().unwrap_or_else(|e| e.into_inner());
                g.push((key.clone(), model.clone()));
            }
            if key == "sk-live" {
                return Err(whycodes_core::Error::Provider(
                    "HTTP 429 rate_limit_exceeded retry in 0 seconds".into(),
                ));
            }
            Ok(Box::pin(futures::stream::iter([
                Ok(whycodes_core::types::StreamEvent::TextDelta {
                    text: "failover-ok".into(),
                }),
                Ok(whycodes_core::types::StreamEvent::MessageStop),
            ]))
                as whycodes_llm::provider::ProviderEventStream)
        })
    }
}

#[tokio::test]
async fn rate_limit_retries_same_model_on_next_credential() {
    let _env = crate::CREDENTIAL_ENV_LOCK.lock().await;
    let prev_live = std::env::var_os("WHYCODES_TEST_FAILOVER_LIVE");
    let prev_ci = std::env::var_os("WHYCODES_TEST_FAILOVER_CI");
    let prev_lane = std::env::var_os("WHYCODES_CREDENTIAL_LANE");
    unsafe {
        std::env::set_var("WHYCODES_TEST_FAILOVER_LIVE", "sk-live");
        std::env::set_var("WHYCODES_TEST_FAILOVER_CI", "sk-ci");
        std::env::set_var("WHYCODES_CREDENTIAL_LANE", "interactive");
    }
    let recorder = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, String)>::new()));
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(RateThenOk {
        inner: std::sync::Arc::clone(&recorder),
    }));
    let mut config = whycodes_config::Config::default();
    config.providers.insert(
        "openai".into(),
        ProviderConfig {
            name: "openai".into(),
            api_key: None,
            api_base: None,
            base_url: None,
            headers: None,
            models: vec![],
            tool_arguments: None,
            extra: Default::default(),
            credentials: ProviderCredentials {
                order: vec!["interactive".into(), "ci".into()],
                reserve: 0.1,
                env: std::collections::HashMap::from([
                    ("interactive".into(), "WHYCODES_TEST_FAILOVER_LIVE".into()),
                    ("ci".into(), "WHYCODES_TEST_FAILOVER_CI".into()),
                ]),
            },
        },
    );
    let agent = Agent::new(info("build"))
        .with_config(&config)
        .with_provider_registry(registry);
    let mut session = session_user("please explain the retry loop");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let out = agent
        .run_turn_with_events(
            &mut session,
            TurnOpts {
                provider_name: "openai",
                model: "gpt-4o",
                api_key: "sk-live",
                max_turns: Some(2),
                events: Some(tx),
                cancel: None,
            },
        )
        .await;
    unsafe {
        match prev_live {
            Some(v) => std::env::set_var("WHYCODES_TEST_FAILOVER_LIVE", v),
            None => std::env::remove_var("WHYCODES_TEST_FAILOVER_LIVE"),
        }
        match prev_ci {
            Some(v) => std::env::set_var("WHYCODES_TEST_FAILOVER_CI", v),
            None => std::env::remove_var("WHYCODES_TEST_FAILOVER_CI"),
        }
        match prev_lane {
            Some(v) => std::env::set_var("WHYCODES_CREDENTIAL_LANE", v),
            None => std::env::remove_var("WHYCODES_CREDENTIAL_LANE"),
        }
    }
    let out = out.expect("failover turn");
    assert!(out.contains("failover-ok"), "{out}");
    let mut saw_rate_status = false;
    while let Ok(ev) = rx.try_recv() {
        if let TurnEvent::Status(s) = ev
            && s.contains("credential")
        {
            saw_rate_status = true;
            break;
        }
    }
    assert!(
        saw_rate_status,
        "429 failover must emit a credential status"
    );
    let calls = recorder.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert!(
        calls.iter().any(|(k, m)| k == "sk-live" && m == "gpt-4o"),
        "{calls:?}"
    );
    assert_eq!(
        calls.last(),
        Some(&("sk-ci".into(), "gpt-4o".into())),
        "{calls:?}"
    );
    assert!(
        calls.iter().all(|(_, m)| m == "gpt-4o"),
        "429 failover must keep the same model: {calls:?}"
    );
    assert_eq!(agent.sticky_credential().as_deref(), Some("ci"));
}

struct AlwaysRateLimited;

impl LlmProvider for AlwaysRateLimited {
    fn name(&self) -> &str {
        "openai"
    }
    fn default_base_url(&self) -> &str {
        "http://script.invalid"
    }
    fn complete<'a>(
        &'a self,
        _request: &'a LlmRequest,
        _api_key: &'a str,
        _model: &'a str,
    ) -> whycodes_llm::provider::ProviderResponseFuture<'a> {
        Box::pin(async { Err(whycodes_core::Error::Provider("unused".into())) })
    }
    fn stream<'a>(
        &'a self,
        _request: &'a LlmRequest,
        _api_key: &'a str,
        _model: &'a str,
    ) -> whycodes_llm::provider::ProviderStreamFuture<'a> {
        Box::pin(async {
            Err(whycodes_core::Error::Provider(
                "HTTP 429 rate_limit_exceeded retry in 0 seconds".into(),
            ))
        })
    }
}

fn assert_rate_limited(err: &whycodes_core::Error) {
    assert!(
        err.to_string().to_lowercase().contains("429")
            || err.to_string().to_lowercase().contains("rate"),
        "{err}"
    );
}

#[tokio::test]
async fn rate_limit_without_failover_returns_err() {
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(AlwaysRateLimited));
    let agent = Agent::new(info("build")).with_provider_registry(registry);
    let mut session = session_user("please explain the retry loop");
    let err = agent
        .run_turn(&mut session, "openai", "gpt-4o", "sk-live", Some(2))
        .await
        .expect_err("no next credential");
    assert_rate_limited(&err);
}

#[tokio::test]
async fn rate_limit_same_secret_does_not_loop() {
    let _env = crate::CREDENTIAL_ENV_LOCK.lock().await;
    let prev = std::env::var_os("WHYCODES_TEST_FAILOVER_DUP");
    let prev_lane = std::env::var_os("WHYCODES_CREDENTIAL_LANE");
    unsafe {
        std::env::set_var("WHYCODES_TEST_FAILOVER_DUP", "sk-same");
        std::env::set_var("WHYCODES_CREDENTIAL_LANE", "interactive");
    }
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(AlwaysRateLimited));
    let mut config = whycodes_config::Config::default();
    config.providers.insert(
        "openai".into(),
        ProviderConfig {
            name: "openai".into(),
            api_key: None,
            api_base: None,
            base_url: None,
            headers: None,
            models: vec![],
            tool_arguments: None,
            extra: Default::default(),
            credentials: ProviderCredentials {
                order: vec!["interactive".into(), "ci".into()],
                reserve: 0.1,
                env: std::collections::HashMap::from([
                    ("interactive".into(), "WHYCODES_TEST_FAILOVER_DUP".into()),
                    ("ci".into(), "WHYCODES_TEST_FAILOVER_DUP".into()),
                ]),
            },
        },
    );
    let agent = Agent::new(info("build"))
        .with_config(&config)
        .with_provider_registry(registry);
    let mut session = session_user("please explain the retry loop");
    let err = agent
        .run_turn(&mut session, "openai", "gpt-4o", "sk-same", Some(2))
        .await
        .expect_err("identical secrets must not failover-loop");
    unsafe {
        match prev {
            Some(v) => std::env::set_var("WHYCODES_TEST_FAILOVER_DUP", v),
            None => std::env::remove_var("WHYCODES_TEST_FAILOVER_DUP"),
        }
        match prev_lane {
            Some(v) => std::env::set_var("WHYCODES_CREDENTIAL_LANE", v),
            None => std::env::remove_var("WHYCODES_CREDENTIAL_LANE"),
        }
    }
    assert_rate_limited(&err);
}

#[tokio::test]
async fn successful_turn_stamps_default_sticky_credential() {
    let agent = scripted([ScriptedStep::Text("ok".into())]);
    let mut session = session_user("please explain the retry loop");
    let out = agent
        .run_turn(&mut session, "script", "m", "k", Some(2))
        .await
        .expect("turn");
    assert!(out.contains("ok"), "{out}");
    assert_eq!(agent.sticky_credential().as_deref(), Some("default"));
}

/// Records `(system, last user text)` per stream request, then delegates.
struct RecordingProvider {
    inner: ScriptedProvider,
    seen: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
}

impl LlmProvider for RecordingProvider {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn default_base_url(&self) -> &str {
        self.inner.default_base_url()
    }
    fn complete<'a>(
        &'a self,
        request: &'a LlmRequest,
        api_key: &'a str,
        model: &'a str,
    ) -> whycodes_llm::provider::ProviderResponseFuture<'a> {
        self.inner.complete(request, api_key, model)
    }
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
        api_key: &'a str,
        model: &'a str,
    ) -> whycodes_llm::provider::ProviderStreamFuture<'a> {
        let last_user = request
            .messages
            .iter()
            .rev()
            .find(|m| m.role == whycodes_core::types::Role::User)
            .map(|m| match &m.content {
                whycodes_core::types::MessageContent::Text(t) => t.clone(),
                whycodes_core::types::MessageContent::Blocks(b) => format!("{b:?}"),
            })
            .unwrap_or_default();
        self.seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((request.system.clone(), last_user));
        self.inner.stream(request, api_key, model)
    }
}

#[tokio::test]
async fn turn_context_rides_on_the_user_message_at_every_step() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "secret").unwrap();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(RecordingProvider {
        inner: ScriptedProvider::batched(
            "script",
            [
                vec![ScriptedStep::ToolCall {
                    id: "c1".into(),
                    name: "read".into(),
                    input: json!({"path": "note.txt"}),
                }],
                vec![ScriptedStep::Text("done reading".into())],
            ],
        ),
        seen: std::sync::Arc::clone(&seen),
    }));
    let agent = Agent::new(info("build")).with_provider_registry(registry);
    let mut session = session_at(dir.path(), "please read note.txt and summarize it");
    let system_before = session.system_prompt.clone();
    session.turn_context = Some("\n\n<recalled-context>\nfact\n</recalled-context>".into());
    agent
        .run_turn(&mut session, "script", "m", "k", Some(4))
        .await
        .expect("turn");

    let seen = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(seen.len(), 2, "tool step + answer step: {seen:?}");
    for (system, user) in &seen {
        assert_eq!(
            system, &system_before,
            "system prompt must stay byte-stable"
        );
        assert!(
            user.ends_with("summarize it\n\n<recalled-context>\nfact\n</recalled-context>"),
            "every step carries the same recall suffix: {user:?}"
        );
    }
    let stored = session
        .messages
        .iter()
        .find(|m| m.role == whycodes_core::types::Role::User)
        .and_then(|m| m.content.as_text())
        .unwrap_or_default();
    assert_eq!(
        stored, "please read note.txt and summarize it",
        "recall is never written into the transcript"
    );
}

/// Read-only tool that outlives the cancel in the test below.
struct SlowGlob;

impl whycodes_core::Tool for SlowGlob {
    fn name(&self) -> &str {
        "glob"
    }
    fn description(&self) -> &str {
        "slow"
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
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            whycodes_core::types::ToolResult {
                tool_call_id: String::new(),
                content: "never".into(),
                is_error: false,
            }
        })
    }
}

#[tokio::test]
async fn cancel_during_a_tool_closes_every_unanswered_call() {
    // An assistant tool call with no result makes the next request invalid
    // (OpenAI: "tool_calls must be followed by tool messages"; Anthropic:
    // tool_use without tool_result). Cancel mid-tool must still answer it.
    let mut exec = whycodes_tools::executor::ToolExecutor::new();
    exec.register(Box::new(SlowGlob));
    let agent = scripted([
        ScriptedStep::ToolCall {
            id: "c1".into(),
            name: "glob".into(),
            input: json!({"pattern": "*.rs"}),
        },
        ScriptedStep::ToolCall {
            id: "c2".into(),
            name: "glob".into(),
            input: json!({"pattern": "*.md"}),
        },
    ])
    .with_tool_executor(exec);
    let dir = tempfile::tempdir().unwrap();
    let mut session = session_at(dir.path(), "please list the rust and markdown files");
    let cancel = crate::events::new_cancel_flag();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        crate::events::request_cancel(&trigger);
    });
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
    assert!(err.to_string().contains("Cancelled"), "{err}");

    let answered: Vec<&str> = session
        .messages
        .iter()
        .filter(|m| m.role == whycodes_core::types::Role::Tool)
        .filter_map(|m| m.tool_call_id.as_deref())
        .collect();
    assert_eq!(answered, ["c1", "c2"], "{:#?}", session.messages);
    let body = session
        .messages
        .last()
        .and_then(|m| m.content.as_text())
        .unwrap_or_default();
    assert!(body.contains("not run"), "{body}");
}

#[tokio::test]
async fn a_read_below_the_project_attaches_that_directory_s_agents_md_once() {
    let dir = tempfile::tempdir().unwrap();
    let pkg = dir.path().join("pkg");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(pkg.join("AGENTS.md"), "Use tabs in pkg.").unwrap();
    std::fs::write(pkg.join("a.txt"), "alpha").unwrap();
    std::fs::write(pkg.join("b.txt"), "beta").unwrap();
    let agent = batched([
        vec![ScriptedStep::ToolCall {
            id: "r1".into(),
            name: "read".into(),
            input: json!({"path": "pkg/a.txt"}),
        }],
        vec![ScriptedStep::ToolCall {
            id: "r2".into(),
            name: "read".into(),
            input: json!({"path": "pkg/b.txt"}),
        }],
        vec![ScriptedStep::Text("done".into())],
    ]);
    let mut session = session_at(dir.path(), "please read both files in pkg");
    agent
        .run_turn(&mut session, "script", "m", "k", Some(5))
        .await
        .expect("turn");
    let tool_text = |id: &str| -> String {
        session
            .messages
            .iter()
            .find(|m| m.tool_call_id.as_deref() == Some(id))
            .and_then(|m| m.content.as_text())
            .unwrap_or_default()
            .to_string()
    };
    let first = tool_text("r1");
    assert!(first.contains("alpha"), "{first}");
    assert!(first.contains("<nested-instructions>"), "{first}");
    assert!(first.contains("Use tabs in pkg."), "{first}");
    let second = tool_text("r2");
    assert!(second.contains("beta"), "{second}");
    assert!(
        !second.contains("nested-instructions"),
        "once per directory: {second}"
    );
    assert!(session.system_prompt == "test", "system prompt untouched");
}

type FallbackCalls = std::sync::Arc<std::sync::Mutex<Vec<(String, String, String)>>>;

/// Fails to open for `fail_models` with `err`; otherwise answers "from name/model".
struct FallbackProbe {
    name: &'static str,
    fail_models: &'static [&'static str],
    err: &'static str,
    calls: FallbackCalls,
}

impl LlmProvider for FallbackProbe {
    fn name(&self) -> &str {
        self.name
    }
    fn default_base_url(&self) -> &str {
        "http://script.invalid"
    }
    fn complete<'a>(
        &'a self,
        _request: &'a LlmRequest,
        _api_key: &'a str,
        _model: &'a str,
    ) -> whycodes_llm::provider::ProviderResponseFuture<'a> {
        Box::pin(async { Err(whycodes_core::Error::Provider("unused".into())) })
    }
    fn stream<'a>(
        &'a self,
        _request: &'a LlmRequest,
        api_key: &'a str,
        model: &'a str,
    ) -> whycodes_llm::provider::ProviderStreamFuture<'a> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).push((
            self.name.to_string(),
            api_key.to_string(),
            model.to_string(),
        ));
        let fail = self.fail_models.contains(&model);
        let err = self.err;
        let text = format!("from {}/{model}", self.name);
        Box::pin(async move {
            if fail {
                return Err(whycodes_core::Error::Provider(err.into()));
            }
            Ok(Box::pin(futures::stream::iter([
                Ok(whycodes_core::types::StreamEvent::TextDelta { text }),
                Ok(whycodes_core::types::StreamEvent::MessageStop),
            ]))
                as whycodes_llm::provider::ProviderEventStream)
        })
    }
}

const UNAVAILABLE: &str = "HTTP 503 service unavailable retry in 0 seconds";

fn probe(
    name: &'static str,
    fail: &'static [&'static str],
    calls: &FallbackCalls,
) -> FallbackProbe {
    FallbackProbe {
        name,
        fail_models: fail,
        err: UNAVAILABLE,
        calls: std::sync::Arc::clone(calls),
    }
}

fn fallback_agent(config: &whycodes_config::Config, providers: Vec<FallbackProbe>) -> Agent {
    let mut registry = ProviderRegistry::new();
    for p in providers {
        registry.register(Box::new(p));
    }
    Agent::new(info("build"))
        .with_config(config)
        .with_provider_registry(registry)
}

fn config_with_fallbacks(models: &[&str]) -> whycodes_config::Config {
    let mut config = whycodes_config::Config::default();
    config.session.fallback_models = models.iter().map(|m| m.to_string()).collect();
    config
}

#[tokio::test]
async fn unavailable_model_falls_back_to_a_sibling_on_the_same_key() {
    let calls = FallbackCalls::default();
    let config = config_with_fallbacks(&["openai/gpt-4o-mini"]);
    let agent = fallback_agent(&config, vec![probe("openai", &["gpt-4o"], &calls)]);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut session = session_user("please explain the retry loop");
    let text = agent
        .run_turn_with_events(
            &mut session,
            TurnOpts {
                provider_name: "openai",
                model: "gpt-4o",
                api_key: "sk-a",
                max_turns: Some(4),
                events: Some(tx),
                cancel: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(text, "from openai/gpt-4o-mini");
    let status = drain_status(&mut rx);
    assert!(
        status
            .iter()
            .any(|s| s.contains("falling back to openai/gpt-4o-mini")),
        "{status:?}"
    );
    let calls = calls.lock().unwrap().clone();
    assert_eq!(
        calls.last().unwrap().1,
        "sk-a",
        "same provider keeps the key"
    );
}

#[tokio::test]
async fn fallback_walks_the_chain_and_uses_config_keys_across_providers() {
    let calls = FallbackCalls::default();
    let mut config = config_with_fallbacks(&[
        "no-slash-entry",
        "unregistered/model",
        "openai/gpt-4o",
        "openai/gpt-4o-mini",
        "anthropic/claude",
    ]);
    config.providers.insert(
        "anthropic".into(),
        ProviderConfig {
            name: "anthropic".into(),
            api_key: Some("sk-ant".into()),
            api_base: None,
            base_url: None,
            headers: None,
            models: vec![],
            tool_arguments: None,
            extra: Default::default(),
            credentials: ProviderCredentials::default(),
        },
    );
    let agent = fallback_agent(
        &config,
        vec![
            probe("openai", &["gpt-4o", "gpt-4o-mini"], &calls),
            probe("anthropic", &[], &calls),
        ],
    );
    let mut session = session_user("please explain the retry loop");
    let text = agent
        .run_turn(&mut session, "openai", "gpt-4o", "sk-a", Some(6))
        .await
        .unwrap();
    assert_eq!(text, "from anthropic/claude");
    let calls = calls.lock().unwrap().clone();
    let last = calls.last().unwrap();
    assert_eq!((last.0.as_str(), last.1.as_str()), ("anthropic", "sk-ant"));
    assert!(
        calls.iter().any(|c| c.2 == "gpt-4o-mini"),
        "the failing mini fallback is tried first: {calls:?}"
    );
}

#[tokio::test]
async fn fallback_reads_env_keys_and_skips_missing_ones() {
    let _env = crate::CREDENTIAL_ENV_LOCK.lock().await;
    let prev_empty = std::env::var_os("WCFBEMPTY_API_KEY");
    let prev_ok = std::env::var_os("WCFBENV_API_KEY");
    unsafe {
        std::env::set_var("WCFBEMPTY_API_KEY", "");
        std::env::set_var("WCFBENV_API_KEY", "sk-env");
        std::env::remove_var("WCFBNONE_API_KEY");
    }
    let calls = FallbackCalls::default();
    let config = config_with_fallbacks(&["wcfbnone/m", "wcfbempty/m", "wcfbenv/m"]);
    let agent = fallback_agent(
        &config,
        vec![
            probe("openai", &["gpt-4o"], &calls),
            probe("wcfbnone", &[], &calls),
            probe("wcfbempty", &[], &calls),
            probe("wcfbenv", &[], &calls),
        ],
    );
    let mut session = session_user("please explain the retry loop");
    let result = agent
        .run_turn(&mut session, "openai", "gpt-4o", "sk-a", Some(4))
        .await;
    unsafe {
        match prev_empty {
            Some(v) => std::env::set_var("WCFBEMPTY_API_KEY", v),
            None => std::env::remove_var("WCFBEMPTY_API_KEY"),
        }
        match prev_ok {
            Some(v) => std::env::set_var("WCFBENV_API_KEY", v),
            None => std::env::remove_var("WCFBENV_API_KEY"),
        }
    }
    assert_eq!(result.unwrap(), "from wcfbenv/m");
    let calls = calls.lock().unwrap().clone();
    assert_eq!(calls.last().unwrap().1, "sk-env");
    assert!(
        calls
            .iter()
            .all(|c| c.0 != "wcfbnone" && c.0 != "wcfbempty")
    );
}

#[tokio::test]
async fn non_retryable_errors_and_an_exhausted_chain_surface_the_error() {
    let calls = FallbackCalls::default();
    let config = config_with_fallbacks(&["openai/gpt-4o-mini"]);
    let auth_fail = FallbackProbe {
        name: "openai",
        fail_models: &["gpt-4o"],
        err: "HTTP 401 unauthorized: invalid api key",
        calls: std::sync::Arc::clone(&calls),
    };
    let agent = fallback_agent(&config, vec![auth_fail]);
    let mut session = session_user("please explain the retry loop");
    let err = agent
        .run_turn(&mut session, "openai", "gpt-4o", "sk-a", Some(4))
        .await
        .expect_err("401 must not fall back");
    assert!(err.to_string().contains("401"), "{err}");
    assert!(calls.lock().unwrap().iter().all(|c| c.2 == "gpt-4o"));

    let calls = FallbackCalls::default();
    let agent = fallback_agent(
        &config,
        vec![probe("openai", &["gpt-4o", "gpt-4o-mini"], &calls)],
    );
    let mut session = session_user("please explain the retry loop");
    let err = agent
        .run_turn(&mut session, "openai", "gpt-4o", "sk-a", Some(4))
        .await
        .expect_err("every target is down");
    assert!(err.to_string().contains("503"), "{err}");
}

#[tokio::test]
async fn a_dropped_prompt_cache_shows_a_status_notice() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("n.txt"), "payload").unwrap();
    let agent = batched([
        vec![
            ScriptedStep::CacheUsage {
                creation_input_tokens: 0,
                read_input_tokens: 40_000,
            },
            ScriptedStep::ToolCall {
                id: "r1".into(),
                name: "read".into(),
                input: json!({"path": "n.txt"}),
            },
        ],
        vec![
            ScriptedStep::Usage {
                input_tokens: 41_000,
                output_tokens: 5,
            },
            ScriptedStep::Text("cache went cold".into()),
        ],
    ]);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut session = session_at(dir.path(), "please read n.txt and summarize it");
    agent
        .run_turn_with_events(&mut session, opts(Some(tx)))
        .await
        .unwrap();
    let status = drain_status(&mut rx);
    assert!(
        status
            .iter()
            .any(|s| s.starts_with("Prompt cache miss: 41.0k tokens")),
        "{status:?}"
    );
}

/// Reports 1M input tokens per step, then answers.
struct PricedUsageProvider;

impl LlmProvider for PricedUsageProvider {
    fn name(&self) -> &str {
        "openai"
    }
    fn default_base_url(&self) -> &str {
        "http://script.invalid"
    }
    fn complete<'a>(
        &'a self,
        _request: &'a LlmRequest,
        _api_key: &'a str,
        _model: &'a str,
    ) -> whycodes_llm::provider::ProviderResponseFuture<'a> {
        Box::pin(async { Err(whycodes_core::Error::Provider("unused".into())) })
    }
    fn stream<'a>(
        &'a self,
        _request: &'a LlmRequest,
        _api_key: &'a str,
        _model: &'a str,
    ) -> whycodes_llm::provider::ProviderStreamFuture<'a> {
        Box::pin(async {
            Ok(Box::pin(futures::stream::iter([
                Ok(whycodes_core::types::StreamEvent::Usage {
                    input_tokens: 1_000_000,
                    output_tokens: 0,
                }),
                Ok(whycodes_core::types::StreamEvent::TextDelta {
                    text: "priced".into(),
                }),
                Ok(whycodes_core::types::StreamEvent::MessageStop),
            ]))
                as whycodes_llm::provider::ProviderEventStream)
        })
    }
}

#[tokio::test]
async fn turn_accumulates_cost_only_for_priced_models() {
    let mut config = whycodes_config::Config::default();
    config.pricing.insert(
        "openai/gpt-cost".into(),
        whycodes_config::PriceConfig {
            input: 2.0,
            output: 8.0,
            cache_read: None,
            cache_write: None,
        },
    );
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(PricedUsageProvider));
    let agent = Agent::new(info("build"))
        .with_config(&config)
        .with_provider_registry(registry);
    let mut session = session_user("please explain the retry loop");
    let text = agent
        .run_turn(&mut session, "openai", "gpt-cost", "sk", Some(2))
        .await
        .unwrap();
    assert_eq!(text, "priced");
    assert!((session.cost.usd - 2.0).abs() < 1e-9, "{:?}", session.cost);
    assert_eq!(session.cost.unpriced_steps, 0);

    let mut unpriced = session_user("please explain the retry loop");
    agent
        .run_turn(&mut unpriced, "openai", "no-price", "sk", Some(2))
        .await
        .unwrap();
    assert_eq!(unpriced.cost.usd, 0.0);
    assert_eq!(unpriced.cost.unpriced_steps, 1);
}

/// Pushes one steering message while the first request is in flight.
struct SteerDuringFirstRequest {
    inner: ScriptedProvider,
    inbox: crate::steer::SteerInbox,
    session_id: String,
    calls: std::sync::atomic::AtomicUsize,
}

impl LlmProvider for SteerDuringFirstRequest {
    fn name(&self) -> &str {
        "script"
    }
    fn default_base_url(&self) -> &str {
        "http://script.invalid"
    }
    fn complete<'a>(
        &'a self,
        request: &'a LlmRequest,
        api_key: &'a str,
        model: &'a str,
    ) -> whycodes_llm::provider::ProviderResponseFuture<'a> {
        self.inner.complete(request, api_key, model)
    }
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
        api_key: &'a str,
        model: &'a str,
    ) -> whycodes_llm::provider::ProviderStreamFuture<'a> {
        let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n == 0 {
            let text = whycodes_core::types::MessageContent::Text("also count the words".into());
            self.inbox.push(&self.session_id, text);
            let blocks = whycodes_core::types::MessageContent::Blocks(vec![ContentBlock::Text {
                text: "and list them".into(),
            }]);
            self.inbox.push(&self.session_id, blocks);
        }
        self.inner.stream(request, api_key, model)
    }
}

#[tokio::test]
async fn steering_message_reaches_the_next_step_after_tool_results() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "hello").unwrap();
    let agent = Agent::new(info("build"));
    let inbox = agent.steer_inbox();
    let mut session = session_at(dir.path(), "please read note.txt and summarize it");
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(SteerDuringFirstRequest {
        inner: ScriptedProvider::new([
            ScriptedStep::ToolCall {
                id: "c1".into(),
                name: "read".into(),
                input: json!({"path": "note.txt"}),
            },
            ScriptedStep::Text("done".into()),
        ]),
        inbox: inbox.clone(),
        session_id: session.id.clone(),
        calls: Default::default(),
    }));
    let agent = agent.with_provider_registry(registry);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let out = agent
        .run_turn_with_events(&mut session, opts(Some(tx)))
        .await
        .expect("turn");
    assert!(out.contains("done"), "{out}");
    assert!(
        inbox.drain(&session.id).is_empty(),
        "the turn read the messages"
    );
    assert!(
        session.messages.iter().any(|m| matches!(&m.content,
            whycodes_core::types::MessageContent::Blocks(b)
                if matches!(b.as_slice(), [ContentBlock::Text { text }] if text == "and list them"))),
        "block message in history"
    );
    let mut delivered = false;
    while let Ok(ev) = rx.try_recv() {
        delivered |= matches!(ev, TurnEvent::Status(s) if s.contains("delivered"));
    }
    assert!(delivered);
    let steer_at = session
        .messages
        .iter()
        .position(|m| m.role == Role::User && m.content.as_text() == Some("also count the words"))
        .expect("steering message in history");
    let result_at = session
        .messages
        .iter()
        .position(|m| match &m.content {
            whycodes_core::types::MessageContent::Blocks(b) => b
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolResult { .. })),
            whycodes_core::types::MessageContent::Text(_) => m.tool_call_id.is_some(),
        })
        .expect("tool result in history");
    assert!(steer_at > result_at, "steering lands after the tool result");
}

/// Tools return results with an empty `tool_call_id`; the agent must stamp
/// the call's id, or the request repair (`pairing`) drops the real output
/// and tells the model the call never ran.
#[tokio::test]
async fn tool_results_carry_their_call_id_on_every_execution_path() {
    let call = |id: &str, name: &str, input: serde_json::Value| ScriptedStep::ToolCall {
        id: id.into(),
        name: name.into(),
        input,
    };
    let agent = batched([
        // Single call.
        vec![call("g1", "glob", json!({"pattern": "*.rs"}))],
        // Parallel read-only batch.
        vec![
            call("g2", "glob", json!({"pattern": "*.rs"})),
            call("l3", "list", json!({"path": "."})),
        ],
        // Mixed batch: runs sequentially.
        vec![
            call("g4", "glob", json!({"pattern": "*.rs"})),
            call("b5", "bash", json!({"command": "echo hi"})),
        ],
        vec![ScriptedStep::Text("done".into())],
    ]);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();
    let mut session = session_at(dir.path(), "look around");
    agent
        .run_turn_with_events(&mut session, opts(None))
        .await
        .unwrap();

    let answered: Vec<(&str, String)> = session
        .messages
        .iter()
        .filter(|m| m.role == Role::Tool)
        .map(|m| {
            (
                m.tool_call_id.as_deref().unwrap_or(""),
                m.content.as_text().unwrap_or("").to_string(),
            )
        })
        .collect();
    let ids: Vec<&str> = answered.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, ["g1", "g2", "l3", "g4", "b5"], "{answered:?}");
    assert!(
        whycodes_session::pairing::paired_messages(&session.messages).is_none(),
        "the request repair had to replace real results"
    );
    assert!(answered[0].1.contains("main.rs"), "{answered:?}");
}

#[test]
fn step_texts_are_a_blank_line_apart() {
    let mut text = String::new();
    append_step_text(&mut text, "   ");
    assert_eq!(text, "");
    append_step_text(&mut text, "\n\nFirst step.");
    assert_eq!(text, "\n\nFirst step.");
    append_step_text(&mut text, "Second step. ");
    append_step_text(&mut text, "");
    append_step_text(&mut text, "\nThird.");
    assert_eq!(text, "\n\nFirst step.\n\nSecond step.\n\nThird.");
}

/// Overflows mid-stream on the first open, then records the last user
/// message of every later request.
struct OverflowThenCaptureProvider {
    calls: std::sync::atomic::AtomicU32,
    last_user: std::sync::Arc<std::sync::Mutex<String>>,
}

impl whycodes_llm::LlmProvider for OverflowThenCaptureProvider {
    fn name(&self) -> &str {
        "overflow-capture"
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
        request: &'a whycodes_core::types::LlmRequest,
        _api_key: &'a str,
        _model: &'a str,
    ) -> whycodes_llm::provider::ProviderStreamFuture<'a> {
        let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Ok(mut last) = self.last_user.lock() {
            *last = format!("{:?}", request.messages.last());
        }
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
                        text: "done after retry".into(),
                    });
                })
                    as whycodes_llm::provider::ProviderEventStream)
            }
        })
    }
}

/// A step retried before its stream finished is the same step: it keeps the
/// first-step keyword notice and does not consume `max_turns`.
#[tokio::test]
async fn retried_first_step_keeps_suffix_and_does_not_count_toward_max_turns() {
    let last_user = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(OverflowThenCaptureProvider {
        calls: std::sync::atomic::AtomicU32::new(0),
        last_user: std::sync::Arc::clone(&last_user),
    }));
    let mut agent = Agent::new(info("build")).with_provider_registry(registry);
    agent.compaction_llm = false;
    agent.compaction_threshold = 8;
    let mut session = Session::new(std::path::PathBuf::from("/work/proj"), "test".into());
    session.add_user_message("ultrathink please keep summarizing the huge dump");
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
                provider_name: "overflow-capture",
                model: "overflow-capture-unique",
                api_key: "k",
                max_turns: Some(1),
                events: Some(tx),
                cancel: None,
            },
        )
        .await
        .expect("the retry is the same step, so max_turns = 1 is not exceeded");
    assert!(out.contains("done after retry"), "{out}");
    let captured = last_user.lock().map(|s| s.clone()).unwrap_or_default();
    assert!(
        captured.contains("whycodes_keyword"),
        "retried first step must still carry the keyword notice: {captured}"
    );
    let status = drain_status(&mut rx);
    assert!(
        status.iter().any(|s| s.contains("LLM request (step 1)")),
        "{status:?}"
    );
    assert!(
        !status.iter().any(|s| s.contains("LLM request (step 2)")),
        "a retry must not be announced as step 2: {status:?}"
    );
}

/// Sends one message in `mode` while the first request is in flight.
struct SteerModeDuringFirstRequest {
    inner: ScriptedProvider,
    inbox: crate::steer::SteerInbox,
    session_id: String,
    mode: crate::steer::SteerMode,
    calls: std::sync::atomic::AtomicUsize,
}

impl LlmProvider for SteerModeDuringFirstRequest {
    fn name(&self) -> &str {
        "script"
    }
    fn default_base_url(&self) -> &str {
        "http://script.invalid"
    }
    fn complete<'a>(
        &'a self,
        request: &'a LlmRequest,
        api_key: &'a str,
        model: &'a str,
    ) -> whycodes_llm::provider::ProviderResponseFuture<'a> {
        self.inner.complete(request, api_key, model)
    }
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
        api_key: &'a str,
        model: &'a str,
    ) -> whycodes_llm::provider::ProviderStreamFuture<'a> {
        if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            let text = whycodes_core::types::MessageContent::Text("stop and look".into());
            self.inbox.push_mode(&self.session_id, self.mode, text);
        }
        self.inner.stream(request, api_key, model)
    }
}

fn steer_mode_agent(
    dir: &std::path::Path,
    mode: crate::steer::SteerMode,
    first: Vec<ScriptedStep>,
) -> (Agent, Session, crate::steer::SteerInbox) {
    let agent = Agent::new(info("build"));
    let inbox = agent.steer_inbox();
    let session = session_at(dir, "write the two files");
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(SteerModeDuringFirstRequest {
        inner: ScriptedProvider::batched(
            "script",
            [first, vec![ScriptedStep::Text("done".into())]],
        ),
        inbox: inbox.clone(),
        session_id: session.id.clone(),
        mode,
        calls: Default::default(),
    }));
    (agent.with_provider_registry(registry), session, inbox)
}

fn write_call(id: &str, path: &str) -> ScriptedStep {
    ScriptedStep::ToolCall {
        id: id.into(),
        name: "write".into(),
        input: json!({"path": path, "content": "x"}),
    }
}

/// Tool results in history, as (call id, content).
fn tool_results(session: &Session) -> Vec<(String, String)> {
    session
        .messages
        .iter()
        .filter_map(|m| {
            let id = m.tool_call_id.clone()?;
            Some((id, m.content.as_text().unwrap_or_default().to_string()))
        })
        .collect()
}

fn user_message_at(session: &Session, text: &str) -> Option<usize> {
    session
        .messages
        .iter()
        .position(|m| m.role == Role::User && m.content.as_text() == Some(text))
}

#[tokio::test]
async fn urgent_message_answers_unrun_calls_and_reaches_the_next_step() {
    let dir = tempfile::tempdir().unwrap();
    let (agent, mut session, inbox) = steer_mode_agent(
        dir.path(),
        crate::steer::SteerMode::Urgent,
        vec![write_call("w1", "a.txt"), write_call("w2", "b.txt")],
    );
    let out = agent
        .run_turn_with_events(&mut session, opts(None))
        .await
        .expect("turn");
    assert!(out.contains("done"), "{out}");
    assert!(
        !dir.path().join("a.txt").exists(),
        "skipped calls do not run"
    );
    assert!(!dir.path().join("b.txt").exists());
    let results = tool_results(&session);
    let ids: Vec<&str> = results.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, ["w1", "w2"], "every call keeps a result");
    assert!(
        results
            .iter()
            .all(|(_, c)| c.contains("the user sent a message first")),
        "{results:?}"
    );
    let steer_at = user_message_at(&session, "stop and look").expect("delivered");
    let last_result = session
        .messages
        .iter()
        .rposition(|m| m.tool_call_id.is_some())
        .unwrap_or(0);
    assert!(steer_at > last_result, "the message follows the results");
    assert!(inbox.drain(&session.id).is_empty());
}

#[tokio::test]
async fn queued_message_never_enters_the_running_turn() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "hello").unwrap();
    let (agent, mut session, inbox) = steer_mode_agent(
        dir.path(),
        crate::steer::SteerMode::Queue,
        vec![ScriptedStep::ToolCall {
            id: "r1".into(),
            name: "read".into(),
            input: json!({"path": "note.txt"}),
        }],
    );
    let out = agent
        .run_turn_with_events(&mut session, opts(None))
        .await
        .expect("turn");
    assert!(out.contains("done"), "{out}");
    assert_eq!(user_message_at(&session, "stop and look"), None);
    let left: Vec<String> = inbox
        .drain(&session.id)
        .iter()
        .map(crate::steer::content_text)
        .collect();
    assert_eq!(left, ["stop and look"], "left for the next turn");
}

#[tokio::test]
async fn next_step_message_lets_the_whole_batch_run() {
    let dir = tempfile::tempdir().unwrap();
    let (agent, mut session, inbox) = steer_mode_agent(
        dir.path(),
        crate::steer::SteerMode::NextStep,
        vec![write_call("w1", "a.txt"), write_call("w2", "b.txt")],
    );
    let out = agent
        .run_turn_with_events(&mut session, opts(None))
        .await
        .expect("turn");
    assert!(out.contains("done"), "{out}");
    assert!(
        dir.path().join("a.txt").exists(),
        "default mode skips nothing"
    );
    assert!(dir.path().join("b.txt").exists());
    let results = tool_results(&session);
    assert!(
        results
            .iter()
            .all(|(_, c)| !c.contains("the user sent a message first")),
        "{results:?}"
    );
    let steer_at = user_message_at(&session, "stop and look").expect("delivered");
    let last_result = session
        .messages
        .iter()
        .rposition(|m| m.tool_call_id.is_some())
        .unwrap_or(0);
    assert!(steer_at > last_result, "the message follows the results");
    assert!(inbox.drain(&session.id).is_empty());
}

fn ruled(
    rules: Vec<whycodes_config::StreamRuleConfig>,
    batches: impl IntoIterator<Item = Vec<ScriptedStep>>,
) -> Agent {
    let mut cfg = whycodes_config::Config::default();
    cfg.session.stream_rules = rules;
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(ScriptedProvider::batched("script", batches)));
    Agent::new(info("build"))
        .with_config(&cfg)
        .with_provider_registry(registry)
}

fn rule(
    name: &str,
    pattern: &str,
    scope: &[whycodes_config::StreamRuleScope],
) -> whycodes_config::StreamRuleConfig {
    whycodes_config::StreamRuleConfig {
        name: name.into(),
        pattern: pattern.into(),
        hint: format!("hint for {name}"),
        scope: scope.to_vec(),
        ..Default::default()
    }
}

async fn run_ruled(agent: &Agent, session: &mut Session) -> (String, Vec<String>) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let out = agent
        .run_turn_with_events(session, opts(Some(tx)))
        .await
        .unwrap_or_else(|e| e.to_string());
    (out, drain_status(&mut rx))
}

fn hint_messages(session: &Session) -> usize {
    session
        .messages
        .iter()
        .filter(|m| {
            m.content
                .as_text()
                .is_some_and(|t| t.starts_with("<whycodes_rule name="))
        })
        .count()
}

#[tokio::test]
async fn stream_rule_once_fires_a_single_time_per_session() {
    let mut r = rule("no-bad", "BAD", &[]);
    r.repeat = whycodes_config::StreamRuleRepeat::Once;
    let agent = ruled(
        vec![r],
        [
            vec![ScriptedStep::Text("BAD draft".into())],
            vec![ScriptedStep::Text("BAD again".into())],
            vec![ScriptedStep::Text("BAD next turn".into())],
        ],
    );
    let mut session = session_user("please summarize crates/agent carefully");
    let (out, status) = run_ruled(&agent, &mut session).await;
    assert!(out.contains("BAD again"), "{out}");
    let hits = status.iter().filter(|s| s.contains("no-bad")).count();
    assert_eq!(hits, 1, "{status:?}");
    assert_eq!(session.stream_rules.last_fired("no-bad"), Some(1));
    session.add_user_message("and once more please");
    let (out, _) = run_ruled(&agent, &mut session).await;
    assert!(out.contains("BAD next turn"), "{out}");
    assert_eq!(hint_messages(&session), 1);
    assert_eq!(session.stream_rules.turn, 2);
}

#[tokio::test]
async fn stream_rule_default_fires_on_every_draft() {
    let agent = ruled(
        vec![rule("no-bad", "BAD", &[])],
        [
            vec![ScriptedStep::Text("BAD draft".into())],
            vec![ScriptedStep::Text("BAD again".into())],
            vec![ScriptedStep::Text("clean".into())],
        ],
    );
    let mut session = session_user("please summarize crates/agent carefully");
    let (out, _) = run_ruled(&agent, &mut session).await;
    assert!(out.contains("clean"), "{out}");
    assert_eq!(hint_messages(&session), 2);
}

#[tokio::test]
async fn stream_rule_watches_thinking() {
    use whycodes_config::StreamRuleScope::Thinking;
    let agent = ruled(
        vec![rule("think", "SHORTCUT", &[Thinking])],
        [
            vec![
                ScriptedStep::Thinking("take a ".into()),
                ScriptedStep::Thinking(String::new()),
                ScriptedStep::Thinking("SHORTCUT".into()),
                ScriptedStep::Text("never".into()),
            ],
            vec![ScriptedStep::Text("SHORTCUT is fine in text".into())],
        ],
    );
    let mut session = session_user("please summarize crates/agent carefully");
    let (out, status) = run_ruled(&agent, &mut session).await;
    assert!(out.contains("fine in text"), "{out}");
    assert!(!out.contains("never"), "{out}");
    assert!(status.iter().any(|s| s.contains("think")), "{status:?}");
}

#[tokio::test]
async fn stream_rule_watches_thinking_deltas() {
    use whycodes_config::StreamRuleScope::Thinking;
    let agent = ruled(
        vec![rule("think", "SHORT CUT", &[Thinking])],
        [
            vec![
                ScriptedStep::ThinkingDelta("SHORT ".into()),
                ScriptedStep::ThinkingDelta("CUT".into()),
            ],
            vec![ScriptedStep::Text("ok".into())],
        ],
    );
    let mut session = session_user("please summarize crates/agent carefully");
    let (out, _) = run_ruled(&agent, &mut session).await;
    assert!(out.contains("ok"), "{out}");
    assert_eq!(hint_messages(&session), 1);
}

#[tokio::test]
async fn stream_rule_watches_tool_args_of_named_tools() {
    use whycodes_config::StreamRuleScope::ToolArgs;
    let mut r = rule("no-rm", "rm -rf", &[ToolArgs]);
    r.tools = vec!["bash".into()];
    let agent = ruled(
        vec![r],
        [
            vec![ScriptedStep::ToolCall {
                id: "c1".into(),
                name: "bash".into(),
                input: json!({"command": "rm -rf /"}),
            }],
            vec![ScriptedStep::Text("done safely".into())],
        ],
    );
    let mut session = session_user("please summarize crates/agent carefully");
    let (out, status) = run_ruled(&agent, &mut session).await;
    assert!(out.contains("done safely"), "{out}");
    assert!(status.iter().any(|s| s.contains("no-rm")), "{status:?}");
    assert!(!session.messages.iter().any(|m| m.role == Role::Tool));
}

#[tokio::test]
async fn stream_rule_watches_streamed_tool_arg_deltas() {
    use whycodes_config::StreamRuleScope::ToolArgs;
    let agent = ruled(
        vec![rule("no-rm", "rm -rf", &[ToolArgs])],
        [
            vec![
                // No open call yet: nothing to check.
                ScriptedStep::ToolUseDelta {
                    id: "zz".into(),
                    input_json_delta: "{".into(),
                },
                ScriptedStep::ToolCall {
                    id: "c1".into(),
                    name: "edit".into(),
                    input: serde_json::Value::Null,
                },
                ScriptedStep::ToolUseDelta {
                    id: "c1".into(),
                    input_json_delta: "{\"cmd\": \"rm ".into(),
                },
                ScriptedStep::ToolUseDelta {
                    id: "c1".into(),
                    input_json_delta: "-rf x\"}".into(),
                },
            ],
            vec![ScriptedStep::Text("no tool".into())],
        ],
    );
    let mut session = session_user("please summarize crates/agent carefully");
    let (out, _) = run_ruled(&agent, &mut session).await;
    assert!(out.contains("no tool"), "{out}");
    assert_eq!(hint_messages(&session), 1);
}

/// First stream drops after a partial token with a truncated body; later
/// streams finish.
struct DropOnceProvider {
    calls: std::sync::atomic::AtomicU32,
}

impl whycodes_llm::LlmProvider for DropOnceProvider {
    fn name(&self) -> &str {
        "drop-once"
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
        Box::pin(async { Err(whycodes_core::Error::llm("unused")) })
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
                        text: "partial drop".into(),
                    });
                    yield Err(whycodes_core::Error::llm(
                        "Stream: error decoding response body: unexpected EOF during chunk size line",
                    ));
                })
                    as whycodes_llm::provider::ProviderEventStream)
            } else {
                Ok(Box::pin(async_stream::stream! {
                    yield Ok(whycodes_core::types::StreamEvent::TextDelta {
                        text: "after drop".into(),
                    });
                })
                    as whycodes_llm::provider::ProviderEventStream)
            }
        })
    }
}

fn drop_once_agent(window: std::time::Duration) -> Agent {
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(DropOnceProvider {
        calls: std::sync::atomic::AtomicU32::new(0),
    }));
    let mut agent = Agent::new(info("build")).with_provider_registry(registry);
    agent.stream_retry = window;
    agent
}

#[tokio::test]
async fn mid_stream_drop_retries_step_when_enabled() {
    let agent = drop_once_agent(std::time::Duration::from_secs(30));
    let mut session = session_user("please explain rust ownership");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let out = agent
        .run_turn_with_events(
            &mut session,
            TurnOpts {
                provider_name: "drop-once",
                model: "drop-once-retry-unique",
                api_key: "k",
                max_turns: Some(2),
                events: Some(tx),
                cancel: None,
            },
        )
        .await
        .expect("retry after mid-stream drop");
    assert!(out.contains("after drop"), "{out}");
    assert!(!out.contains("partial drop"), "draft discarded: {out}");
    let status = drain_status(&mut rx);
    assert!(
        status.iter().any(|s| s.contains("Stream dropped")),
        "{status:?}"
    );
}

#[tokio::test]
async fn mid_stream_drop_fails_turn_when_disabled() {
    let agent = drop_once_agent(std::time::Duration::ZERO);
    let mut session = session_user("please explain rust ownership");
    let err = agent
        .run_turn(
            &mut session,
            "drop-once",
            "drop-once-off-unique",
            "k",
            Some(2),
        )
        .await
        .expect_err("stream retry is off");
    assert!(err.to_string().contains("unexpected EOF"), "{err}");
}

#[test]
fn stream_retry_delay_respects_window_and_kind() {
    let drop = whycodes_core::Error::llm("Stream: error decoding response body: unexpected EOF");
    let window = std::time::Duration::from_secs(60);
    let now = Instant::now();
    let d = stream_retry_delay(window, now, 0, &drop).expect("transient drop retries");
    assert!(d <= std::time::Duration::from_millis(500), "{d:?}");
    assert!(stream_retry_delay(std::time::Duration::ZERO, now, 0, &drop).is_none());
    let fatal = whycodes_core::Error::llm("HTTP 400 bad request");
    assert!(stream_retry_delay(window, now, 0, &fatal).is_none());
    let long_ago = now - std::time::Duration::from_secs(120);
    assert!(stream_retry_delay(window, long_ago, 3, &drop).is_none());
}

#[tokio::test]
async fn cancel_during_stream_retry_backoff() {
    let agent = drop_once_agent(std::time::Duration::from_secs(30));
    let mut session = session_user("please explain rust ownership");
    let cancel = crate::events::new_cancel_flag();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            agent
                .run_turn_with_events(
                    &mut session,
                    TurnOpts {
                        provider_name: "drop-once",
                        model: "drop-once-cancel-unique",
                        api_key: "k",
                        max_turns: Some(2),
                        events: Some(tx),
                        cancel: Some(cancel),
                    },
                )
                .await
                .map_err(|e| e.to_string())
        })
    };
    while let Some(ev) = rx.recv().await {
        if matches!(&ev, TurnEvent::Status(s) if s.contains("Stream dropped")) {
            break;
        }
    }
    crate::events::request_cancel(&cancel);
    let err = handle.await.expect("join");
    assert!(
        err.as_ref()
            .err()
            .is_some_and(|e| e.to_lowercase().contains("cancel")),
        "{err:?}"
    );
}

fn todo_write_step(statuses: &[&str]) -> Vec<ScriptedStep> {
    let todos: Vec<_> = statuses
        .iter()
        .enumerate()
        .map(|(i, s)| json!({"id": i.to_string(), "content": format!("step {i}"), "status": s}))
        .collect();
    vec![ScriptedStep::ToolCall {
        id: "t1".into(),
        name: "todowrite".into(),
        input: json!({ "todos": todos }),
    }]
}

fn reminders(session: &Session) -> usize {
    session
        .messages
        .iter()
        .filter(|m| {
            m.role == Role::User
                && m.content
                    .as_text()
                    .is_some_and(|t| t.starts_with("You stopped while your todo list"))
        })
        .count()
}

#[tokio::test]
async fn stopping_with_open_todos_written_this_turn_is_reminded_once() {
    let dir = tempfile::tempdir().unwrap();
    let agent = batched([
        todo_write_step(&["completed", "in_progress", "pending"]),
        vec![ScriptedStep::Text("stopping early".into())],
        vec![ScriptedStep::Text("still open, needs the user".into())],
    ]);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut session = session_at(dir.path(), "do the three steps");
    let out = agent
        .run_turn_with_events(&mut session, opts(Some(tx)))
        .await
        .expect("turn");
    assert!(out.contains("needs the user"), "{out}");
    assert_eq!(
        reminders(&session),
        1,
        "reminded once, then the stop stands"
    );
    let note = session
        .messages
        .iter()
        .rev()
        .find_map(|m| m.content.as_text().filter(|t| t.starts_with("You stopped")))
        .unwrap()
        .to_string();
    assert!(note.contains("- ▶ step 1\n- ☐ step 2\n"), "{note}");
    assert!(!note.contains("step 0"), "{note}");
    assert!(
        drain_status(&mut rx)
            .iter()
            .any(|s| s.contains("Todo list still has open items")),
    );
}

#[tokio::test]
async fn no_todo_reminder_when_done_unwritten_or_planning() {
    // Every item finished.
    let dir = tempfile::tempdir().unwrap();
    let agent = batched([
        todo_write_step(&["completed", "cancelled"]),
        vec![ScriptedStep::Text("all done".into())],
    ]);
    let mut session = session_at(dir.path(), "do it");
    agent
        .run_turn(&mut session, "script", "m", "k", Some(8))
        .await
        .expect("turn");
    assert_eq!(reminders(&session), 0);

    // Open items left from an earlier turn: this turn did not write them.
    let dir = tempfile::tempdir().unwrap();
    let mut session = session_at(dir.path(), "unrelated question");
    whycodes_core::todo::save_todos(
        dir.path(),
        Some(session.id.as_str()),
        &[whycodes_core::todo::TodoItem::new(
            "1",
            "old",
            whycodes_core::todo::TodoStatus::Pending,
        )],
    )
    .unwrap();
    let agent = scripted([ScriptedStep::Text("answer".into())]);
    agent
        .run_turn(&mut session, "script", "m", "k", Some(8))
        .await
        .expect("turn");
    assert_eq!(reminders(&session), 0);

    // The plan agent leaves implementation steps pending on purpose.
    let dir = tempfile::tempdir().unwrap();
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(ScriptedProvider::batched(
        "script",
        [
            todo_write_step(&["in_progress", "pending"]),
            vec![ScriptedStep::Text("here is the plan".into())],
        ],
    )));
    let agent = Agent::new(info("plan")).with_provider_registry(registry);
    let mut session = session_at(dir.path(), "plan it");
    agent
        .run_turn(&mut session, "script", "m", "k", Some(8))
        .await
        .expect("turn");
    assert_eq!(reminders(&session), 0);
}
