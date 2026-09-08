use super::*;
use whycodes_agent::permission::PermissionPrompter;
use whycodes_agent::question::QuestionPrompter;
use whycodes_protocol::sdk::PermissionDecision;

#[test]
fn maps_text_and_drops_panel() {
    let ev = from_turn_event(&TurnEvent::TextDelta("x".into())).unwrap();
    assert!(matches!(ev, SdkEvent::TextDelta { text } if text == "x"));
    assert!(
        from_turn_event(&TurnEvent::EnqueuePrompt {
            text: "later".into()
        })
        .is_none()
    );
}

#[test]
fn maps_every_wire_event() {
    use whycodes_core::PanelUpdate;

    assert!(matches!(
        from_turn_event(&TurnEvent::ToolStart {
            id: "t1".into(),
            name: "bash".into(),
            input: serde_json::json!({})
        }),
        Some(SdkEvent::ToolStart { name, .. }) if name == "bash"
    ));
    assert!(matches!(
        from_turn_event(&TurnEvent::ToolEnd {
            id: "t1".into(),
            content: "ok".into(),
            is_error: false
        }),
        Some(SdkEvent::ToolEnd { content, .. }) if content == "ok"
    ));
    assert!(matches!(
        from_turn_event(&TurnEvent::Cancelled),
        Some(SdkEvent::Cancelled)
    ));
    assert!(matches!(
        from_turn_event(&TurnEvent::Status("busy".into())),
        Some(SdkEvent::Status { message }) if message == "busy"
    ));
    assert!(matches!(
        from_turn_event(&TurnEvent::PermissionAsk {
            request_id: "perm-1".into(),
            tool_name: "bash".into(),
            detail: "run".into()
        }),
        Some(SdkEvent::PermissionRequest { tool_name, .. }) if tool_name == "bash"
    ));
    assert!(matches!(
        from_turn_event(&TurnEvent::QuestionAsk {
            request_id: "q-1".into(),
            questions: serde_json::json!([])
        }),
        Some(SdkEvent::QuestionRequest { request_id, .. }) if request_id == "q-1"
    ));

    // TUI-only chrome is not part of the v1 contract.
    assert!(from_turn_event(&TurnEvent::Panel(PanelUpdate::Clear)).is_none());
    assert!(
        from_turn_event(&TurnEvent::Todos {
            todos: vec![whycodes_core::TodoItem::new(
                "a",
                "x",
                whycodes_core::TodoStatus::Pending
            )]
        })
        .is_none()
    );
    assert!(
        from_turn_event(&TurnEvent::SwarmMessage {
            from: "a".into(),
            to: "b".into(),
            text: "hi".into()
        })
        .is_none()
    );
    assert!(
        from_turn_event(&TurnEvent::FileStale {
            path: "p".into(),
            reader: "r".into(),
            writer: "w".into()
        })
        .is_none()
    );
    assert!(
        from_turn_event(&TurnEvent::Subagent {
            id: "sa".into(),
            kind: "explore".into(),
            description: "d".into(),
            status: "running".into(),
            activity: String::new(),
            elapsed_ms: 0,
            output: String::new(),
        })
        .is_none()
    );

    assert!(matches!(
        from_turn_event(&TurnEvent::ThinkingDelta("think".into())),
        Some(SdkEvent::ReasoningDelta { text }) if text == "think"
    ));
    assert!(matches!(
        from_turn_event(&TurnEvent::Usage(whycodes_core::types::Usage {
            input_tokens: 1,
            output_tokens: 2,
            cache_read_input_tokens: None,
            cache_creation_input_tokens: Some(4),
        })),
        Some(SdkEvent::Usage {
            input_tokens: 1,
            output_tokens: 2,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 4,
        })
    ));
    assert!(matches!(
        from_turn_event(&TurnEvent::Intent {
            kind: "plan".into(),
            confidence: 0.9,
            badge: "plan".into(),
            notice_kind: String::new(),
            notice: String::new(),
        }),
        Some(SdkEvent::Intent { kind, .. }) if kind == "plan"
    ));
    assert!(matches!(
        from_turn_event(&TurnEvent::FileConflict {
            path: "p".into(),
            claimant: "c".into(),
            owner: "o".into(),
        }),
        Some(SdkEvent::FileConflict { path, .. }) if path == "p"
    ));
    assert!(matches!(
        from_turn_event(&TurnEvent::SwarmStatus {
            active: 1,
            total: 2,
            message: "m".into(),
        }),
        Some(SdkEvent::SwarmStatus { total: 2, .. })
    ));
    assert!(matches!(
        from_turn_event(&TurnEvent::Background {
            id: "b".into(),
            status: "done".into(),
            summary: "s".into(),
        }),
        Some(SdkEvent::Background { id, .. }) if id == "b"
    ));
}

#[test]
fn history_from_session_empty_content_and_limit_noop() {
    let mut s = whycodes_session::session::Session::new("/tmp".into(), "sys".into());
    s.messages.push(whycodes_core::types::Message {
        role: whycodes_core::types::Role::Assistant,
        content: whycodes_core::types::MessageContent::Blocks(vec![
            whycodes_core::types::ContentBlock::ToolUse {
                id: "t".into(),
                name: "read".into(),
                input: serde_json::json!({}),
            },
        ]),
        tool_call_id: Some("t".into()),
        name: Some("read".into()),
        created_at: None,
    });
    let hist = history_from_session(&s, Some(8));
    assert_eq!(hist.messages.len(), 1);
    assert_eq!(hist.messages[0].role, "assistant");
    assert_eq!(hist.messages[0].content, "");
    assert_eq!(hist.messages[0].tool_call_id.as_deref(), Some("t"));
    assert_eq!(hist.messages[0].name.as_deref(), Some("read"));
}

#[tokio::test]
async fn list_models_uses_catalog_and_default_fallback() {
    use std::collections::HashMap;
    use whycodes_config::Config;
    use whycodes_core::types::{ModelConfig, ProviderConfig};

    fn model(provider: &str, id: &str) -> ModelConfig {
        ModelConfig {
            model_id: id.into(),
            provider_id: provider.into(),
            max_tokens: None,
            context_window: None,
            temperature: None,
            top_p: None,
            thinking: None,
            supports_tools: None,
            supports_images: None,
        }
    }

    let mut state = crate::http_tests::test_state();
    let mut cfg = Config::default();
    cfg.models.insert("gpt".into(), model("openai", "gpt-4o"));
    cfg.default_model = Some(model("openai", "gpt-4o"));
    cfg.providers.insert(
        "openai".into(),
        ProviderConfig {
            name: "openai".into(),
            api_key: None,
            api_base: None,
            base_url: None,
            headers: None,
            models: Vec::new(),
            tool_arguments: None,
            extra: HashMap::new(),
        },
    );
    state.config = std::sync::Arc::new(cfg);
    let listed = list_models(State(state.clone())).await;
    assert!(listed.models.iter().any(|m| m.id == "gpt-4o" && m.default));

    let cfg2 = Config {
        default_model: Some(model("xai", "grok")),
        ..Default::default()
    };
    state.config = std::sync::Arc::new(cfg2);
    let listed2 = list_models(State(state)).await;
    assert_eq!(listed2.models[0].id, "grok");
    assert!(listed2.models[0].default);
}

#[tokio::test]
async fn persist_list_rename_rewind_compact_hit_db() {
    let _home = crate::http_tests::IsolatedHome::new();
    let state = crate::http_tests::test_state();
    let created = create_session(
        State(state.clone()),
        Json(CreateSessionRequest {
            project: Some("/tmp/v1".into()),
            persist: Some(true),
        }),
    )
    .await;
    let id = created.id.clone();

    let listed = list_sessions(State(state.clone())).await;
    assert!(listed.sessions.iter().any(|s| s.id == id));

    let cold = crate::http_tests::test_state();
    let listed_cold = list_sessions(State(cold.clone())).await;
    assert!(
        listed_cold
            .sessions
            .iter()
            .any(|s| s.id == id && s.source.as_deref() == Some("db")),
        "{listed_cold:?}"
    );

    let renamed = rename(
        State(cold.clone()),
        Path(id.clone()),
        Json(RenameRequest {
            title: "persisted-title".into(),
        }),
    )
    .await
    .unwrap();
    assert_eq!(renamed.title, "persisted-title");

    let rewound = rewind(
        State(cold.clone()),
        Path(id.clone()),
        Json(RewindRequest { index: 0 }),
    )
    .await
    .unwrap();
    assert_eq!(rewound.id, id);

    let compacted = compact(
        State(cold),
        Path(id),
        Json(CompactRequest {
            max_tokens: Some(12),
        }),
    )
    .await
    .unwrap();
    assert!(compacted.messages.is_empty() || !compacted.id.is_empty());
}

#[tokio::test]
async fn run_streams_scripted_success_and_error() {
    let _home = crate::http_tests::IsolatedHome::new();
    let mut registry = whycodes_llm::provider::ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::repeating(
        "ollama",
        [whycodes_llm::ScriptedStep::Text("v1-ok".into())],
    )));
    let state = crate::http_tests::test_state_with_registry(Some(registry));
    let session = whycodes_session::session::Session::new("/tmp".into(), "sys".into());
    let id = session.id.clone();
    state.insert_session(session);

    let ok = run(
        State(state.clone()),
        Path(id.clone()),
        Json(RunRequest {
            message: "v1-success-unique".into(),
            provider: Some("ollama".into()),
            model: Some("tiny-v1-ok".into()),
            max_turns: Some(0),
            auto_approve: Some(true),
        }),
    )
    .await
    .unwrap();
    let bytes = axum::body::to_bytes(ok.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("v1-ok") || text.contains("text_delta"),
        "{text}"
    );

    let mut registry = whycodes_llm::provider::ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::named(
        "ollama",
        [whycodes_llm::ScriptedStep::FailOpen("v1-fail".into())],
    )));
    let err_state = crate::http_tests::test_state_with_registry(Some(registry));
    let session = whycodes_session::session::Session::new("/tmp".into(), "sys".into());
    let id = session.id.clone();
    err_state.insert_session(session);
    let err = run(
        State(err_state),
        Path(id),
        Json(RunRequest {
            message: "v1-error-unique".into(),
            provider: Some("ollama".into()),
            model: Some("tiny-v1-err".into()),
            max_turns: None,
            auto_approve: Some(false),
        }),
    )
    .await
    .unwrap();
    let bytes = axum::body::to_bytes(err.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("v1-fail") || text.contains("error"), "{text}");
}

#[tokio::test]
async fn run_persists_after_scripted_turn() {
    let _home = crate::http_tests::IsolatedHome::new();
    let mut registry = whycodes_llm::provider::ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::repeating(
        "ollama",
        [whycodes_llm::ScriptedStep::Text("persist-ok".into())],
    )));
    let state = crate::http_tests::test_state_with_registry(Some(registry));
    let session = whycodes_session::session::Session::new("/tmp".into(), "sys".into());
    let id = session.id.clone();
    state.insert_session(session);
    let resp = run(
        State(state.clone()),
        Path(id.clone()),
        Json(RunRequest {
            message: "v1-persist-after-run-unique".into(),
            provider: Some("ollama".into()),
            model: Some("tiny-v1-persist".into()),
            max_turns: Some(1),
            auto_approve: Some(true),
        }),
    )
    .await
    .unwrap();
    let _ = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let db = crate::AppState::open_db().expect("db");
    let loaded = whycodes_session::session::Session::load_from_db(&db, &id)
        .unwrap()
        .expect("persisted session");
    assert!(!loaded.messages.is_empty());
}

#[tokio::test]
async fn permission_and_question_http_success() {
    let state = crate::http_tests::test_state();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    state.perm.register_run("s1", tx);
    let hub = Arc::clone(&state.perm);
    let s = RunScope {
        session_id: "s1".into(),
        auto_approve: false,
        hub: Arc::clone(&hub),
    };
    let prompter = crate::perm::ServePrompter {
        hub: Arc::clone(&hub),
    };
    let task = tokio::spawn(async move {
        RUN.scope(s, async { prompter.ask("bash", "x").await })
            .await
    });
    let ev = rx.recv().await.expect("permission ask");
    let TurnEvent::PermissionAsk { request_id, .. } = ev else {
        panic!("expected PermissionAsk");
    };
    let st = permission(
        State(state.clone()),
        Path("s1".into()),
        Json(PermissionResponse {
            request_id,
            decision: PermissionDecision::Allow,
        }),
    )
    .await
    .unwrap();
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(task.await.unwrap());

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    state.perm.register_run("s1", tx);
    let s = RunScope {
        session_id: "s1".into(),
        auto_approve: false,
        hub: Arc::clone(&hub),
    };
    let qprompter = crate::perm::ServeQuestionPrompter::new(Arc::clone(&hub));
    let task = tokio::spawn(async move {
        RUN.scope(s, async { qprompter.ask(Vec::new()).await })
            .await
    });
    let ev = rx.recv().await.unwrap();
    let TurnEvent::QuestionAsk { request_id, .. } = ev else {
        panic!("expected QuestionAsk");
    };
    let st = question(
        State(state),
        Path("s1".into()),
        Json(QuestionResponse {
            request_id,
            answers: None,
            cancelled: Some(true),
        }),
    )
    .await
    .unwrap();
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert_eq!(
        task.await.unwrap().unwrap_err(),
        whycodes_agent::question::QuestionError::Cancelled
    );
}

#[tokio::test]
async fn question_http_rejects_unknown_labels_with_400() {
    let state = crate::http_tests::test_state();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    state.perm.register_run("s1", tx);
    let hub = Arc::clone(&state.perm);
    let s = RunScope {
        session_id: "s1".into(),
        auto_approve: false,
        hub: Arc::clone(&hub),
    };
    let qprompter = crate::perm::ServeQuestionPrompter::new(Arc::clone(&hub));
    let questions = vec![whycodes_tools::question::QuestionSpec {
        prompt: "Pick".into(),
        options: vec![whycodes_tools::question::QuestionOption {
            label: "A".into(),
            description: String::new(),
            preview: None,
        }],
        multi_select: false,
        important: false,
    }];
    let task =
        tokio::spawn(async move { RUN.scope(s, async { qprompter.ask(questions).await }).await });
    let ev = rx.recv().await.unwrap();
    let TurnEvent::QuestionAsk { request_id, .. } = ev else {
        panic!("expected QuestionAsk");
    };
    let err = question(
        State(state.clone()),
        Path("s1".into()),
        Json(QuestionResponse {
            request_id: request_id.clone(),
            answers: Some(vec![whycodes_protocol::sdk::QuestionAnswerWire {
                selected: vec!["nope".into()],
                free_text: None,
            }]),
            cancelled: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(err.0, StatusCode::BAD_REQUEST);
    let st = question(
        State(state),
        Path("s1".into()),
        Json(QuestionResponse {
            request_id,
            answers: None,
            cancelled: Some(true),
        }),
    )
    .await
    .unwrap();
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert_eq!(
        task.await.unwrap().unwrap_err(),
        whycodes_agent::question::QuestionError::Cancelled
    );
}

struct PanicProvider;

impl whycodes_llm::provider::LlmProvider for PanicProvider {
    fn name(&self) -> &str {
        "ollama"
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
        Box::pin(async { panic!("panic-provider") })
    }
    fn stream<'a>(
        &'a self,
        _request: &'a whycodes_core::types::LlmRequest,
        _api_key: &'a str,
        _model: &'a str,
    ) -> whycodes_llm::provider::ProviderStreamFuture<'a> {
        Box::pin(async { panic!("panic-provider") })
    }
}

#[tokio::test]
async fn run_reports_join_error_when_worker_panics() {
    let _home = crate::http_tests::IsolatedHome::new();
    let mut registry = whycodes_llm::provider::ProviderRegistry::new();
    registry.register(Box::new(PanicProvider));
    let state = crate::http_tests::test_state_with_registry(Some(registry));
    let session = whycodes_session::session::Session::new("/tmp".into(), "sys".into());
    let id = session.id.clone();
    state.insert_session(session);
    let resp = run(
        State(state),
        Path(id),
        Json(RunRequest {
            message: "v1-panic-unique".into(),
            provider: Some("ollama".into()),
            model: Some("tiny-v1-panic".into()),
            max_turns: Some(1),
            auto_approve: Some(true),
        }),
    )
    .await
    .unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("run task") || text.contains("error"),
        "{text}"
    );
}

#[tokio::test]
async fn set_model_survives_poisoned_route_map() {
    let state = crate::http_tests::test_state();
    let session = whycodes_session::session::Session::new("/tmp".into(), "sys".into());
    let id = session.id.clone();
    state.insert_session(session);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _g = state.session_route.lock().unwrap();
        panic!("poison");
    }));
    let st = set_model(
        State(state.clone()),
        Path(id.clone()),
        Json(SetModelRequest {
            provider: "openai".into(),
            model: "gpt-4o".into(),
        }),
    )
    .await
    .unwrap();
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (p, m) = session_or_default_model(&state, &id);
    let (dp, dm) = default_provider_model(&state.config);
    assert_eq!((p, m), (dp, dm));
}

#[tokio::test]
async fn persist_warns_when_db_path_is_a_directory() {
    let home = crate::http_tests::IsolatedHome::new();
    std::fs::create_dir_all(home.path().join("whycodes.db")).unwrap();
    let state = crate::http_tests::test_state();
    let created = create_session(
        State(state.clone()),
        Json(CreateSessionRequest {
            project: None,
            persist: Some(true),
        }),
    )
    .await;
    let id = created.id.clone();
    let _ = rename(
        State(state.clone()),
        Path(id.clone()),
        Json(RenameRequest { title: "x".into() }),
    )
    .await;
    let _ = rewind(
        State(state.clone()),
        Path(id.clone()),
        Json(RewindRequest { index: 0 }),
    )
    .await;
    let _ = compact(
        State(state),
        Path(id),
        Json(CompactRequest { max_tokens: None }),
    )
    .await;
}

#[test]
fn history_from_session_maps_roles_and_limits() {
    let mut s = whycodes_session::session::Session::new("/tmp".into(), "sys".into());
    s.add_user_message("first");
    s.add_user_message("second");

    let full = history_from_session(&s, None);
    assert_eq!(full.messages.len(), 2);
    assert_eq!(full.messages[0].role, "user");
    assert_eq!(full.messages[0].content, "first");

    let limited = history_from_session(&s, Some(1));
    assert_eq!(limited.messages.len(), 1);
    assert_eq!(limited.messages[0].content, "second");
}

#[tokio::test]
async fn session_or_default_model_prefers_route_override() {
    let state = crate::http_tests::test_state();
    let (p, m) = session_or_default_model(&state, "no-such-session");
    let (dp, dm) = default_provider_model(&state.config);
    assert_eq!((p, m), (dp, dm));

    let state2 = crate::http_tests::test_state();
    if let Ok(mut map) = state2.session_route.lock() {
        map.insert("s1".into(), ("openai".into(), "gpt-4o".into()));
    }
    let (p2, m2) = session_or_default_model(&state2, "s1");
    assert_eq!((p2.as_str(), m2.as_str()), ("openai", "gpt-4o"));
}

#[tokio::test]
async fn list_session_helpers_skip_missing_and_failed_db_rows() {
    let mut live = Vec::new();
    push_live_v1_session(&mut live, None).await;
    assert!(live.is_empty());

    let mut db_sessions = Vec::new();
    push_db_v1_sessions(&mut db_sessions, &[], None);
    assert!(db_sessions.is_empty());

    let overlap = whycodes_storage::models::SessionRow {
        id: "live".into(),
        title: "t".into(),
        created_at: "c".into(),
        updated_at: "u".into(),
        project_path: "/p".into(),
        usage: Default::default(),
    };
    let extra = whycodes_storage::models::SessionRow {
        id: "db-only".into(),
        title: "t2".into(),
        created_at: "c".into(),
        updated_at: "u".into(),
        project_path: "/p".into(),
        usage: Default::default(),
    };
    push_db_v1_sessions(
        &mut db_sessions,
        &["live".into()],
        Some(vec![overlap, extra]),
    );
    assert_eq!(db_sessions.len(), 1);
    assert_eq!(db_sessions[0].id, "db-only");
}
