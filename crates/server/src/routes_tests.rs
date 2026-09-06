use super::*;
use std::collections::HashMap;
use whycodes_config::Config;
use whycodes_core::types::{ModelConfig, ProviderConfig};

#[test]
fn html_escape_escapes_special_chars() {
    assert_eq!(html_escape("a & b < c > d"), "a &amp; b &lt; c &gt; d");
    assert_eq!(html_escape("plain"), "plain");
    assert_eq!(html_escape(""), "");
}

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

#[test]
#[allow(clippy::field_reassign_with_default)]
fn default_model_wins_when_configured() {
    let mut c = Config::default();
    c.default_model = Some(model("openai", "gpt-4o"));
    let (p, m) = default_provider_model(&c);
    assert_eq!((p.as_str(), m.as_str()), ("openai", "gpt-4o"));
}

#[test]
#[allow(clippy::field_reassign_with_default)]
fn first_provider_is_the_fallback_without_a_default_model() {
    let mut c = Config::default();
    c.default_model = None;
    c.providers.insert(
        "groq".into(),
        ProviderConfig {
            name: "groq".into(),
            api_key: None,
            api_base: None,
            base_url: None,
            headers: None,
            models: Vec::new(),
            tool_arguments: None,
            extra: HashMap::new(),
        },
    );
    let (p, m) = default_provider_model(&c);
    assert_eq!(p, "groq");
    assert_eq!(m, "default");
}

#[test]
fn hardcoded_fallback_when_nothing_is_configured() {
    let c = Config::default();
    let (p, m) = default_provider_model(&c);
    assert_eq!(
        (p.as_str(), m.as_str()),
        ("anthropic", "claude-sonnet-4-20250514")
    );
}

#[test]
fn find_share_file_in_strips_ext_and_picks_first_hit() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join("abc.md"), "# hi").unwrap();
    std::fs::write(b.join("abc.md"), "# later").unwrap();

    let hit = find_share_file_in(&[a.clone(), b.clone()], "abc.json", "md").unwrap();
    assert_eq!(hit, a.join("abc.md"));
    assert!(find_share_file_in(&[a, b], "missing", "json").is_none());
}

#[test]
fn turn_events_have_stable_api_payloads() {
    let cases = [
        (
            TurnEvent::TextDelta("hello".into()),
            serde_json::json!({"type": "text_delta", "text": "hello"}),
        ),
        (
            TurnEvent::ToolStart {
                id: "call-1".into(),
                name: "read".into(),
                input: serde_json::json!({"path": "README.md"}),
            },
            serde_json::json!({
                "type": "tool_start",
                "id": "call-1",
                "name": "read",
                "input": {"path": "README.md"},
            }),
        ),
        (
            TurnEvent::PermissionAsk {
                request_id: "perm-1".into(),
                tool_name: "bash".into(),
                detail: "cargo test".into(),
            },
            serde_json::json!({
                "type": "permission_request",
                "request_id": "perm-1",
                "tool_name": "bash",
                "detail": "cargo test",
            }),
        ),
        (
            TurnEvent::Panel(whycodes_core::PanelUpdate::File {
                path: "src/lib.rs".into(),
                text: "fn main() {}".into(),
            }),
            serde_json::json!({
                "type": "panel",
                "action": "file",
                "path": "src/lib.rs",
            }),
        ),
    ];

    for (event, expected) in cases {
        assert_eq!(turn_event_json(&event), Some(expected));
    }

    assert_eq!(
        turn_event_json(&TurnEvent::Status("error:provider unavailable".into())),
        Some(serde_json::json!({
            "type": "error",
            "message": "provider unavailable",
        }))
    );
    assert!(turn_event_json(&TurnEvent::Status("done:42chars".into())).is_none());
    assert_eq!(
        turn_event_json(&TurnEvent::Status("working".into())),
        Some(serde_json::json!({"type": "status", "message": "working"}))
    );
    assert_eq!(
        turn_event_json(&TurnEvent::ThinkingDelta("hmm".into())),
        Some(serde_json::json!({"type": "thinking_delta", "text": "hmm"}))
    );
    assert_eq!(
        turn_event_json(&TurnEvent::ToolEnd {
            id: "t".into(),
            content: "out".into(),
            is_error: true,
        }),
        Some(serde_json::json!({
            "type": "tool_end",
            "id": "t",
            "content": "out",
            "is_error": true,
        }))
    );
    assert_eq!(
        turn_event_json(&TurnEvent::Usage(whycodes_core::types::Usage {
            input_tokens: 1,
            output_tokens: 2,
            cache_read_input_tokens: Some(3),
            cache_creation_input_tokens: Some(4),
        })),
        Some(serde_json::json!({
            "type": "usage",
            "input_tokens": 1,
            "output_tokens": 2,
            "cache_read_input_tokens": 3,
            "cache_creation_input_tokens": 4,
        }))
    );
    assert_eq!(
        turn_event_json(&TurnEvent::Cancelled),
        Some(serde_json::json!({"type": "cancelled"}))
    );
    assert_eq!(
        turn_event_json(&TurnEvent::Intent {
            kind: "change".into(),
            confidence: 0.5,
            badge: "chg".into(),
            notice_kind: "info".into(),
            notice: "n".into(),
        }),
        Some(serde_json::json!({
            "type": "intent",
            "kind": "change",
            "confidence": 0.5,
            "badge": "chg",
            "notice_kind": "info",
            "notice": "n",
        }))
    );
    assert_eq!(
        turn_event_json(&TurnEvent::FileConflict {
            path: "a.rs".into(),
            claimant: "c".into(),
            owner: "o".into(),
        }),
        Some(serde_json::json!({
            "type": "file_conflict",
            "path": "a.rs",
            "claimant": "c",
            "owner": "o",
        }))
    );
    assert_eq!(
        turn_event_json(&TurnEvent::SwarmStatus {
            active: 1,
            total: 2,
            message: "go".into(),
        }),
        Some(serde_json::json!({
            "type": "swarm_status",
            "active": 1,
            "total": 2,
            "message": "go",
        }))
    );
    assert_eq!(
        turn_event_json(&TurnEvent::Background {
            id: "b".into(),
            status: "running".into(),
            summary: "s".into(),
        }),
        Some(serde_json::json!({
            "type": "background",
            "id": "b",
            "status": "running",
            "summary": "s",
        }))
    );
    assert_eq!(
        turn_event_json(&TurnEvent::EnqueuePrompt {
            text: "next".into()
        }),
        Some(serde_json::json!({"type": "enqueue_prompt", "text": "next"}))
    );
    assert_eq!(
        turn_event_json(&TurnEvent::SwarmMessage {
            from: "a".into(),
            to: "b".into(),
            text: "hi".into(),
        }),
        Some(serde_json::json!({
            "type": "swarm_message",
            "from": "a",
            "to": "b",
            "text": "hi",
        }))
    );
    assert_eq!(
        turn_event_json(&TurnEvent::FileStale {
            path: "p".into(),
            reader: "r".into(),
            writer: "w".into(),
        }),
        Some(serde_json::json!({
            "type": "file_stale",
            "path": "p",
            "reader": "r",
            "writer": "w",
        }))
    );
    assert_eq!(
        turn_event_json(&TurnEvent::QuestionAsk {
            request_id: "q1".into(),
            questions: serde_json::json!([{"prompt": "ok?"}]),
        }),
        Some(serde_json::json!({
            "type": "question_request",
            "request_id": "q1",
            "questions": [{"prompt": "ok?"}],
        }))
    );
    assert_eq!(
        turn_event_json(&TurnEvent::Subagent {
            id: "sa".into(),
            kind: "explore".into(),
            description: "d".into(),
            status: "running".into(),
            activity: "Thinking".into(),
            elapsed_ms: 9,
            output: "out".into(),
        }),
        Some(serde_json::json!({
            "type": "subagent",
            "id": "sa",
            "kind": "explore",
            "description": "d",
            "status": "running",
            "activity": "Thinking",
            "elapsed_ms": 9,
        }))
    );
    assert_eq!(
        turn_event_json(&TurnEvent::Todos {
            todos: vec![
                whycodes_core::TodoItem::new("1", "a", whycodes_core::TodoStatus::Pending),
                whycodes_core::TodoItem::new("2", "b", whycodes_core::TodoStatus::Completed),
            ],
        }),
        Some(serde_json::json!({"type": "todos", "count": 2, "done": 1}))
    );
    assert_eq!(
        turn_event_json(&TurnEvent::Panel(whycodes_core::PanelUpdate::Clear)),
        Some(serde_json::json!({"type": "panel", "action": "clear"}))
    );
    assert_eq!(
        turn_event_json(&TurnEvent::Panel(whycodes_core::PanelUpdate::Diff {
            path: "x.rs".into(),
            unified: "--- a".into(),
        })),
        Some(serde_json::json!({"type": "panel", "action": "diff", "path": "x.rs"}))
    );
    assert_eq!(
        turn_event_json(&TurnEvent::Panel(whycodes_core::PanelUpdate::Mermaid {
            source: "graph TD; A-->B".into(),
        })),
        Some(serde_json::json!({"type": "panel", "action": "mermaid"}))
    );
}

#[test]
fn emit_status_ignores_a_closed_channel() {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    drop(rx);
    emit_status(&tx, "gone");
}

#[test]
fn system_prompt_includes_runtime_context() {
    let state = crate::http_tests::test_state();
    let prompt = system_prompt_for(&state.agent, &state.project_dir);
    assert!(prompt.contains("Today's date:"), "{prompt}");
}

#[tokio::test]
async fn resolve_api_key_prefers_env_then_config_then_openai() {
    let _home = crate::http_tests::IsolatedHome::new();
    let mut c = Config::default();
    c.providers.insert(
        "groq".into(),
        ProviderConfig {
            name: "groq".into(),
            api_key: Some("cfg-key".into()),
            api_base: None,
            base_url: None,
            headers: None,
            models: Vec::new(),
            tool_arguments: None,
            extra: HashMap::new(),
        },
    );

    let prev_groq = std::env::var_os("GROQ_API_KEY");
    let prev_openai = std::env::var_os("OPENAI_API_KEY");
    unsafe { std::env::set_var("GROQ_API_KEY", "env-key") };
    assert_eq!(
        resolve_api_key("groq", &c).await.as_deref(),
        Some("env-key")
    );
    unsafe { std::env::set_var("GROQ_API_KEY", "") };
    assert_eq!(
        resolve_api_key("groq", &c).await.as_deref(),
        Some("cfg-key")
    );

    unsafe { std::env::set_var("OPENAI_API_KEY", "oa-key") };
    assert_eq!(
        resolve_api_key("openai", &c).await.as_deref(),
        Some("oa-key")
    );
    unsafe { std::env::remove_var("OPENAI_API_KEY") };
    assert!(resolve_api_key("openai", &c).await.is_none());

    let mut empty_key = Config::default();
    empty_key.providers.insert(
        "groq".into(),
        ProviderConfig {
            name: "groq".into(),
            api_key: Some(String::new()),
            api_base: None,
            base_url: None,
            headers: None,
            models: Vec::new(),
            tool_arguments: None,
            extra: HashMap::new(),
        },
    );
    unsafe { std::env::remove_var("GROQ_API_KEY") };
    assert!(resolve_api_key("groq", &empty_key).await.is_none());

    match prev_groq {
        Some(v) => unsafe { std::env::set_var("GROQ_API_KEY", v) },
        None => unsafe { std::env::remove_var("GROQ_API_KEY") },
    }
    match prev_openai {
        Some(v) => unsafe { std::env::set_var("OPENAI_API_KEY", v) },
        None => unsafe { std::env::remove_var("OPENAI_API_KEY") },
    }
}

#[tokio::test]
async fn resolve_api_key_reads_oauth_store() {
    let home = crate::http_tests::IsolatedHome::new();
    whycodes_auth::spec::register_spec(whycodes_auth::spec::ProviderSpec {
        name: "server-cov-oauth".into(),
        label: "cov".into(),
        flow: whycodes_auth::spec::FlowKind::PasteCodePkce,
        client_id: "id".into(),
        client_secret: None,
        authorize_url: "https://example.com/a".into(),
        token_url: "https://example.com/t".into(),
        scopes: "s".into(),
        token_encoding: whycodes_auth::spec::TokenEncoding::Form,
        redirect_uri: Some("https://example.com/cb".into()),
        loopback_port: None,
        loopback_host: None,
        callback_path: String::new(),
        extra_authorize: vec![],
        derived: None,
        suggested_models: vec![],
        inference: None,
    });
    let store = whycodes_auth::store::TokenStore::new(home.path());
    store
        .set(
            "server-cov-oauth",
            whycodes_auth::token::ProviderAuth {
                method: "oauth".into(),
                token: whycodes_auth::token::OAuthToken {
                    access_token: "oauth-tok".into(),
                    refresh_token: None,
                    expires_at: None,
                    extra: serde_json::Map::new(),
                },
            },
        )
        .unwrap();
    let got = resolve_api_key("server-cov-oauth", &Config::default()).await;
    assert_eq!(got.as_deref(), Some("oauth-tok"));
}

#[tokio::test]
async fn share_routes_serve_files_and_error_on_unreadable() {
    let cwd = crate::http_tests::IsolatedCwd::new();
    let project_shares = cwd.path().join(".whycodes").join("shares");
    let global_shares = cwd.path().join("shares");
    std::fs::create_dir_all(&project_shares).unwrap();
    std::fs::create_dir_all(&global_shares).unwrap();
    std::fs::write(project_shares.join("mdonly.md"), "hello & <world>").unwrap();
    std::fs::write(global_shares.join("jsononly.json"), "{\"ok\":true}").unwrap();
    std::fs::create_dir(project_shares.join("baddir.md")).unwrap();
    std::fs::create_dir(project_shares.join("baddir.json")).unwrap();

    let listed = list_shares().await;
    let shares = listed["shares"].as_array().unwrap();
    assert!(shares.iter().any(|s| s["id"] == "jsononly"), "{listed:?}");

    let view = share_view(Path("mdonly".into())).await;
    let (parts, body) = view.into_parts();
    assert_eq!(parts.status, StatusCode::OK);
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    let html = String::from_utf8_lossy(&bytes);
    assert!(html.contains("hello &amp; &lt;world&gt;"), "{html}");

    let view_json = share_view(Path("jsononly".into())).await;
    assert_eq!(view_json.status(), StatusCode::OK);

    let json = share_json(Path("jsononly.json".into())).await;
    assert_eq!(json.status(), StatusCode::OK);
    let md = share_markdown(Path("mdonly.md".into())).await;
    assert_eq!(md.status(), StatusCode::OK);

    let bad_md = share_markdown(Path("baddir.md".into())).await;
    assert_eq!(bad_md.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let bad_json = share_json(Path("baddir.json".into())).await;
    assert_eq!(bad_json.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let dispatched = share_dispatch(Path("mdonly.md".into())).await;
    assert_eq!(dispatched.status(), StatusCode::OK);
    let dispatched_json = share_dispatch(Path("jsononly.json".into())).await;
    assert_eq!(dispatched_json.status(), StatusCode::OK);

    assert!(find_share_file("mdonly", "md").is_some());
    assert!(share_search_dirs().len() >= 2);

    let empty_view = share_view(Path("baddir".into())).await;
    let (parts, body) = empty_view.into_parts();
    assert_eq!(parts.status, StatusCode::OK);
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("(empty)"));
}

#[tokio::test]
async fn list_models_uses_catalog_or_default() {
    let mut state = crate::http_tests::test_state();
    let mut cfg = Config::default();
    cfg.models.insert("gpt".into(), model("openai", "gpt-4o"));
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
    assert_eq!(listed["models"][0]["id"], "gpt-4o");
    assert_eq!(listed["providers"][0]["id"], "openai");

    let cfg2 = Config {
        default_model: Some(model("xai", "grok")),
        ..Default::default()
    };
    state.config = std::sync::Arc::new(cfg2);
    let listed2 = list_models(State(state)).await;
    assert_eq!(listed2["models"][0]["id"], "grok");
    assert_eq!(listed2["models"][0]["default"], true);
}

#[tokio::test]
async fn sessions_persist_merge_and_reload_from_db() {
    let _home = crate::http_tests::IsolatedHome::new();
    let state = crate::http_tests::test_state();
    let created = create_session(
        State(state.clone()),
        Json(CreateSessionRequest {
            project: Some("/tmp/proj".into()),
            persist: Some(true),
        }),
    )
    .await;
    let id = created["session_id"].as_str().unwrap().to_string();

    let listed = list_sessions(State(state.clone())).await;
    assert!(
        listed["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == id && s["source"] == "memory")
    );

    let cold = crate::http_tests::test_state();
    let listed_cold = list_sessions(State(cold.clone())).await;
    assert!(
        listed_cold["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == id && s["source"] == "db"),
        "{listed_cold:?}"
    );
    let loaded = load_or_get_session(&cold, &id).await.expect("load db");
    assert_eq!(loaded.lock().await.id, id);
    let got = get_session(State(cold.clone()), Path(id.clone()))
        .await
        .unwrap();
    assert_eq!(got["id"], id);
    let msgs = get_session_messages(State(cold), Path(id)).await.unwrap();
    assert_eq!(msgs["messages"].as_array().map(Vec::len), Some(0));
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
    assert!(!created["session_id"].as_str().unwrap().is_empty());

    let session = whycodes_session::session::Session::new("/tmp".into(), "sys".into());
    let id = session.id.clone();
    state.insert_session(session);
    let _ = chat(
        State(state),
        Path(id),
        Json(ChatRequest {
            message: "chat-persist-warn-unique".into(),
            provider: Some("ollama".into()),
            model: Some("tiny".into()),
            api_key: Some("k".into()),
            max_turns: Some(1),
        }),
    )
    .await;
}

#[tokio::test]
async fn chat_streams_scripted_success_and_error() {
    let _home = crate::http_tests::IsolatedHome::new();
    let mut registry = whycodes_llm::provider::ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::repeating(
        "ollama",
        [whycodes_llm::ScriptedStep::Text("hello-sse".into())],
    )));
    let state = crate::http_tests::test_state_with_registry(Some(registry));
    let session = whycodes_session::session::Session::new("/tmp".into(), "sys".into());
    let id = session.id.clone();
    state.insert_session(session);

    let ok = chat(
        State(state.clone()),
        Path(id.clone()),
        Json(ChatRequest {
            message: "chat-success-unique".into(),
            provider: Some("ollama".into()),
            model: Some("tiny-chat-ok".into()),
            api_key: Some(String::new()),
            max_turns: Some(0),
        }),
    )
    .await
    .unwrap();
    let bytes = axum::body::to_bytes(ok.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("hello-sse") || text.contains("text_delta"),
        "{text}"
    );
    assert!(text.contains("done"), "{text}");

    let mut registry = whycodes_llm::provider::ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::named(
        "ollama",
        [whycodes_llm::ScriptedStep::FailOpen("scripted-fail".into())],
    )));
    let err_state = crate::http_tests::test_state_with_registry(Some(registry));
    let session = whycodes_session::session::Session::new("/tmp".into(), "sys".into());
    let id = session.id.clone();
    err_state.insert_session(session);
    let err = chat(
        State(err_state),
        Path(id),
        Json(ChatRequest {
            message: "chat-error-unique".into(),
            provider: Some("ollama".into()),
            model: Some("tiny-chat-err".into()),
            api_key: Some("k".into()),
            max_turns: None,
        }),
    )
    .await
    .unwrap();
    let bytes = axum::body::to_bytes(err.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("error") || text.contains("scripted-fail"),
        "{text}"
    );
}

#[tokio::test]
async fn chat_persists_after_scripted_turn() {
    let _home = crate::http_tests::IsolatedHome::new();
    let mut registry = whycodes_llm::provider::ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::repeating(
        "ollama",
        [whycodes_llm::ScriptedStep::Text("chat-persist-ok".into())],
    )));
    let state = crate::http_tests::test_state_with_registry(Some(registry));
    let session = whycodes_session::session::Session::new("/tmp".into(), "sys".into());
    let id = session.id.clone();
    state.insert_session(session);
    let resp = chat(
        State(state),
        Path(id.clone()),
        Json(ChatRequest {
            message: "chat-persist-after-turn-unique".into(),
            provider: Some("ollama".into()),
            model: Some("tiny-chat-persist".into()),
            api_key: Some("k".into()),
            max_turns: Some(1),
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
