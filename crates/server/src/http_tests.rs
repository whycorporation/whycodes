//! In-process HTTP coverage for `/api/*` and `/v1/*` (no live LLM).

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tower::ServiceExt;
use whycodes_agent::agent::Agent;
use whycodes_agent::events::new_cancel_flag;
use whycodes_config::Config;
use whycodes_core::types::ContentBlock;
use whycodes_session::session::Session;

use crate::{AppState, create_router};

/// Serializes tests that mutate process-global env (`WHYCODES_HOME`, cwd, keys).
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Point `WHYCODES_HOME` at a temp dir until dropped.
pub(crate) struct IsolatedHome {
    _guard: std::sync::MutexGuard<'static, ()>,
    dir: tempfile::TempDir,
    prev: Option<std::ffi::OsString>,
}

impl IsolatedHome {
    pub(crate) fn new() -> Self {
        let guard = lock_env();
        let dir = tempfile::tempdir().expect("tempdir");
        let prev = std::env::var_os("WHYCODES_HOME");
        unsafe { std::env::set_var("WHYCODES_HOME", dir.path()) };
        Self {
            _guard: guard,
            dir,
            prev,
        }
    }

    pub(crate) fn path(&self) -> &std::path::Path {
        self.dir.path()
    }

    /// Override what Drop writes back. Call only while this isolation is live.
    pub(crate) fn set_prev(&mut self, prev: Option<std::ffi::OsString>) {
        self.prev = prev;
    }

    /// Restore `WHYCODES_HOME` while still holding `ENV_LOCK`.
    pub(crate) fn restore_env(&self) {
        match &self.prev {
            Some(v) => unsafe { std::env::set_var("WHYCODES_HOME", v) },
            None => unsafe { std::env::remove_var("WHYCODES_HOME") },
        }
    }
}

impl Drop for IsolatedHome {
    fn drop(&mut self) {
        self.restore_env();
    }
}

/// Build a minimal, fully in-memory [`AppState`] for unit tests.
pub(crate) fn test_state() -> AppState {
    test_state_with_registry(None)
}

/// Like [`test_state`], with an optional LLM registry (scripted providers).
pub(crate) fn test_state_with_registry(
    registry: Option<whycodes_llm::provider::ProviderRegistry>,
) -> AppState {
    use whycodes_core::types::{AgentInfo, AgentMode, PermissionSet};

    let mut agent = Agent::new(AgentInfo {
        name: "test".into(),
        description: "test agent".into(),
        mode: AgentMode::Primary,
        permission: PermissionSet::default(),
        model: None,
        system_prompt: None,
        temperature: None,
        top_p: None,
    });
    if let Some(registry) = registry {
        agent = agent.with_provider_registry(registry);
    }
    AppState {
        agent: Arc::new(agent),
        config: Arc::new(Config::default()),
        project_dir: std::env::temp_dir(),
        sessions: Arc::new(std::sync::Mutex::new(HashMap::new())),
        max_turns: Some(5),
        mcp_warm: false,
        index_warm: false,
        started_at: std::time::Instant::now(),
        cancel_flags: Arc::new(std::sync::Mutex::new(HashMap::new())),
        perm: crate::perm::PermHub::new(),
        session_route: Arc::new(std::sync::Mutex::new(HashMap::new())),
    }
}

fn poison_mutex<T>(m: &std::sync::Mutex<T>) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _g = m.lock().unwrap();
        panic!("poison");
    }));
}

/// Point cwd at a temp dir until dropped. Nested with [`IsolatedHome`].
pub(crate) struct IsolatedCwd {
    _home: IsolatedHome,
    prev: std::path::PathBuf,
}

impl IsolatedCwd {
    pub(crate) fn new() -> Self {
        let home = IsolatedHome::new();
        let prev = std::env::current_dir().expect("cwd");
        let target = home.path().to_path_buf();
        std::env::set_current_dir(&target)
            .unwrap_or_else(|e| panic!("chdir {}: {e}", target.display()));
        let now = std::env::current_dir().expect("cwd after chdir");
        let now_c = now.canonicalize().unwrap_or(now);
        let want_c = target.canonicalize().unwrap_or(target);
        assert_eq!(now_c, want_c, "chdir did not stick");
        Self { _home: home, prev }
    }

    pub(crate) fn path(&self) -> &std::path::Path {
        self._home.path()
    }
}

impl Drop for IsolatedCwd {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.prev);
    }
}

async fn call(app: axum::Router, req: Request<Body>) -> (StatusCode, Vec<u8>) {
    let resp = app.oneshot(req).await.expect("oneshot");
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.expect("body");
    (status, bytes.to_vec())
}

async fn json_get(app: axum::Router, uri: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("req");
    let (status, bytes) = call(app, req).await;
    let v = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, v)
}

async fn json_post(app: axum::Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("req");
    let (status, bytes) = call(app, req).await;
    let v = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    (status, v)
}

#[tokio::test]
async fn api_health_tools_models_and_empty_sessions() {
    let state = test_state();
    let app = create_router(state);

    let (st, health) = json_get(app.clone(), "/api/health").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(health["status"], "ok");
    assert_eq!(health["warm"]["mcp"], false);
    assert_eq!(health["warm"]["index"], false);
    assert_eq!(health["max_turns"], 5);

    let (st, tools) = json_get(app.clone(), "/api/tools").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(tools["profile"], "core");
    assert!(
        tools["tools"]
            .as_array()
            .is_some_and(|a| a.iter().any(|t| t["name"] == "read")),
        "{tools}"
    );

    let (st, models) = json_get(app.clone(), "/api/models").await;
    assert_eq!(st, StatusCode::OK);
    assert!(models.get("models").is_some());
    assert!(models.get("providers").is_some());

    let (st, sessions) = json_get(app, "/api/sessions").await;
    assert_eq!(st, StatusCode::OK);
    assert!(sessions["sessions"].is_array());
}

#[tokio::test]
async fn api_session_crud_without_persist() {
    let app = create_router(test_state());

    let (st, created) = json_post(
        app.clone(),
        "/api/session/new",
        serde_json::json!({"persist": false}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let id = created["session_id"].as_str().expect("id").to_string();
    assert!(!id.is_empty());
    assert_eq!(created["persisted"], false);

    let (st, listed) = json_get(app.clone(), "/api/sessions").await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        listed["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == id),
        "{listed}"
    );

    let (st, got) = json_get(app.clone(), &format!("/api/session/{id}")).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(got["id"], id);
    assert_eq!(got["messages"], 0);

    let (st, msgs) = json_get(app.clone(), &format!("/api/session/{id}/messages")).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(msgs["id"], id);

    let (st, _) = json_get(app.clone(), "/api/session/does-not-exist").await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let (st, _) = json_get(app, "/api/session/does-not-exist/messages").await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_chat_rejects_empty_and_missing_session() {
    let app = create_router(test_state());
    let (st, created) = json_post(
        app.clone(),
        "/api/session/new",
        serde_json::json!({"persist": false}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let id = created["session_id"].as_str().unwrap();

    let (st, _) = json_post(
        app.clone(),
        &format!("/api/session/{id}/chat"),
        serde_json::json!({"message": "   "}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    let (st, _) = json_post(
        app.clone(),
        "/api/session/missing/chat",
        serde_json::json!({"message": "hi"}),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // No API key → SSE error stream (still 200).
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/session/{id}/chat"))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({"message": "hello"}).to_string(),
        ))
        .unwrap();
    let (st, body) = call(app, req).await;
    assert_eq!(st, StatusCode::OK);
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("error") || text.contains("No API key"),
        "{text}"
    );
}

#[tokio::test]
async fn api_shares_and_missing_share_pages() {
    let app = create_router(test_state());
    let (st, listed) = json_get(app.clone(), "/api/shares").await;
    assert_eq!(st, StatusCode::OK);
    assert!(listed["shares"].is_array());

    let (st, body) = call(
        app.clone(),
        Request::builder()
            .uri("/s/no-such-share")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert!(String::from_utf8_lossy(&body).contains("not found"));

    let (st, _) = call(
        app.clone(),
        Request::builder()
            .uri("/s/no-such-share.json")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let (st, _) = call(
        app,
        Request::builder()
            .uri("/s/no-such-share.md")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn v1_health_session_lifecycle_and_model_override() {
    let app = create_router(test_state());

    let (st, hs) = json_get(app.clone(), "/v1/health").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(hs["protocol"], 1);
    assert_eq!(hs["healthy"], true);

    let (st, created) = json_post(
        app.clone(),
        "/v1/sessions",
        serde_json::json!({"persist": false}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let id = created["id"].as_str().expect("id").to_string();

    let (st, listed) = json_get(app.clone(), "/v1/sessions").await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        listed["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == id)
    );

    let (st, got) = json_get(app.clone(), &format!("/v1/sessions/{id}")).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(got["id"], id);

    let (st, _) = json_get(app.clone(), "/v1/sessions/missing").await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let (st, hist) = json_get(app.clone(), &format!("/v1/sessions/{id}/messages")).await;
    assert_eq!(st, StatusCode::OK);
    assert!(hist["messages"].as_array().unwrap().is_empty());

    let (st, models) = json_get(app.clone(), "/v1/models").await;
    assert_eq!(st, StatusCode::OK);
    assert!(models.get("models").is_some());

    let (st, _) = json_post(
        app.clone(),
        &format!("/v1/sessions/{id}/model"),
        serde_json::json!({"provider": "openai", "model": "gpt-4o"}),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    let (st, _) = json_post(
        app.clone(),
        &format!("/v1/sessions/{id}/model"),
        serde_json::json!({"provider": "  ", "model": "x"}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    let (st, _) = json_post(
        app.clone(),
        "/v1/sessions/missing/model",
        serde_json::json!({"provider": "openai", "model": "gpt-4o"}),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let (st, renamed) = json_post(
        app.clone(),
        &format!("/v1/sessions/{id}/rename"),
        serde_json::json!({"title": "renamed"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(renamed["title"], "renamed");

    let (st, _) = json_post(
        app.clone(),
        &format!("/v1/sessions/{id}/rewind"),
        serde_json::json!({"index": 0}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let (st, _) = json_post(
        app.clone(),
        &format!("/v1/sessions/{id}/compact"),
        serde_json::json!({"max_tokens": 100}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let (st, _) = json_post(
        app.clone(),
        &format!("/v1/sessions/{id}/cancel"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let (st, _) = json_post(
        app.clone(),
        "/v1/sessions/missing/cancel",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let (st, err) = json_post(
        app.clone(),
        &format!("/v1/sessions/{id}/permission"),
        serde_json::json!({"request_id": "r1", "decision": "allow"}),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert!(err.is_string() || err.is_null() || err.is_object());

    let (st, _) = json_post(
        app,
        &format!("/v1/sessions/{id}/question"),
        serde_json::json!({"request_id": "q1", "answers": []}),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn v1_run_rejects_empty_and_streams_auth_error() {
    let app = create_router(test_state());
    let (st, created) = json_post(
        app.clone(),
        "/v1/sessions",
        serde_json::json!({"persist": false}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let id = created["id"].as_str().unwrap();

    let (st, _) = json_post(
        app.clone(),
        &format!("/v1/sessions/{id}/run"),
        serde_json::json!({"message": "  "}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    let (st, _) = json_post(
        app.clone(),
        "/v1/sessions/missing/run",
        serde_json::json!({"message": "hi"}),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/sessions/{id}/run"))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({"message": "hello"}).to_string(),
        ))
        .unwrap();
    let (st, body) = call(app, req).await;
    assert_eq!(st, StatusCode::OK);
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("Auth") || text.contains("No API key") || text.contains("error"),
        "{text}"
    );
}

#[tokio::test]
async fn router_rejects_wrong_method_and_malformed_json() {
    let app = create_router(test_state());

    let (st, _) = call(
        app.clone(),
        Request::builder()
            .method("POST")
            .uri("/v1/health")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::METHOD_NOT_ALLOWED);

    let (st, _) = call(
        app,
        Request::builder()
            .method("POST")
            .uri("/v1/sessions")
            .header("content-type", "application/json")
            .body(Body::from("{"))
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn v1_local_mutations_return_not_found_for_missing_session() {
    let app = create_router(test_state());
    let cases = [
        ("rename", serde_json::json!({"title": "new title"})),
        ("rewind", serde_json::json!({"index": 0})),
        ("compact", serde_json::json!({"max_tokens": 100})),
    ];

    for (route, body) in cases {
        let (st, _) = json_post(app.clone(), &format!("/v1/sessions/missing/{route}"), body).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "route: {route}");
    }
}

#[tokio::test]
async fn api_seeded_session_exposes_metadata_and_messages() {
    let state = test_state();
    let project = std::env::temp_dir().join("whycodes-server-route-test");
    let mut session = Session::new(project.clone(), "system prompt".into());
    session.title = "seeded".into();
    session.add_user_message("hello");
    session.add_assistant_message(vec![ContentBlock::Text {
        text: "world".into(),
    }]);
    let id = session.id.clone();
    state.insert_session(session);
    let app = create_router(state);

    let (st, got) = json_get(app.clone(), &format!("/api/session/{id}")).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(got["title"], "seeded");
    assert_eq!(got["project"], project.display().to_string());
    assert_eq!(got["messages"], 2);
    assert!(got["token_estimate"].as_u64().is_some_and(|n| n > 0));

    let (st, history) = json_get(app, &format!("/api/session/{id}/messages")).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(history["messages"].as_array().map(Vec::len), Some(2));
    assert_eq!(history["messages"][0]["role"], "user");
    assert_eq!(history["messages"][1]["role"], "assistant");
}

#[tokio::test]
async fn v1_history_limit_model_cancel_and_rewind_change_live_state() {
    let state = test_state();
    let mut session = Session::new(std::env::temp_dir(), "system prompt".into());
    session.add_user_message("first");
    session.add_assistant_message(vec![ContentBlock::Text {
        text: "answer".into(),
    }]);
    session.add_user_message("last");
    let id = session.id.clone();
    let handle = state.insert_session(session);
    let cancel = whycodes_agent::events::new_cancel_flag();
    state.register_cancel(&id, cancel.clone());
    let app = create_router(state.clone());

    let (st, history) = json_get(app.clone(), &format!("/v1/sessions/{id}/messages?limit=2")).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(history["messages"].as_array().map(Vec::len), Some(2));
    assert_eq!(history["messages"][0]["content"], "answer");
    assert_eq!(history["messages"][1]["content"], "last");

    let (st, body) = json_post(
        app.clone(),
        &format!("/v1/sessions/{id}/model"),
        serde_json::json!({"provider": "test-provider", "model": "test-model"}),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert_eq!(body, Value::Null);
    assert_eq!(
        state
            .session_route
            .lock()
            .expect("session route lock")
            .get(&id),
        Some(&("test-provider".into(), "test-model".into()))
    );

    let (st, _) = json_post(
        app.clone(),
        &format!("/v1/sessions/{id}/cancel"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(st, StatusCode::ACCEPTED);
    assert!(whycodes_agent::events::is_cancelled(&Some(cancel)));

    let (st, rewound) = json_post(
        app,
        &format!("/v1/sessions/{id}/rewind"),
        serde_json::json!({"index": 1}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(rewound["messages"].as_array().map(Vec::len), Some(2));
    assert_eq!(handle.lock().await.messages.len(), 2);
}

#[tokio::test]
async fn api_and_v1_create_with_project_and_scripted_turns() {
    let _home = IsolatedHome::new();
    let mut registry = whycodes_llm::provider::ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::repeating(
        "ollama",
        [whycodes_llm::ScriptedStep::Text("from-http".into())],
    )));
    let state = test_state_with_registry(Some(registry));
    let app = crate::create_router(state);

    let (st, created) = json_post(
        app.clone(),
        "/api/session/new",
        serde_json::json!({"project": "/tmp/http-proj", "persist": true}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let api_id = created["session_id"].as_str().unwrap().to_string();

    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/session/{api_id}/chat"))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "message": "http-chat-unique",
                "provider": "ollama",
                "model": "tiny-http-chat",
                "api_key": "k",
                "max_turns": 1
            })
            .to_string(),
        ))
        .unwrap();
    let (st, body) = call(app.clone(), req).await;
    assert_eq!(st, StatusCode::OK);
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("from-http") || text.contains("text_delta") || text.contains("done"),
        "{text}"
    );

    let (st, v1) = json_post(
        app.clone(),
        "/v1/sessions",
        serde_json::json!({"project": "/tmp/v1-proj", "persist": true}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let v1_id = v1["id"].as_str().unwrap().to_string();

    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/sessions/{v1_id}/run"))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "message": "http-run-unique",
                "provider": "ollama",
                "model": "tiny-http-run",
                "auto_approve": true,
                "max_turns": 1
            })
            .to_string(),
        ))
        .unwrap();
    let (st, body) = call(app.clone(), req).await;
    assert_eq!(st, StatusCode::OK);
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("from-http") || text.contains("text_delta") || text.contains("error"),
        "{text}"
    );

    let (st, _) = json_get(app.clone(), "/v1/sessions/missing/messages").await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[test]
fn isolated_cwd_points_at_home_and_restores() {
    let before = std::env::current_dir()
        .unwrap()
        .canonicalize()
        .unwrap_or_else(|_| std::env::current_dir().unwrap());
    {
        let cwd = IsolatedCwd::new();
        let now = std::env::current_dir()
            .unwrap()
            .canonicalize()
            .unwrap_or_else(|_| std::env::current_dir().unwrap());
        let want = cwd
            .path()
            .canonicalize()
            .unwrap_or_else(|_| cwd.path().to_path_buf());
        assert_eq!(now, want);
    }
    let after = std::env::current_dir()
        .unwrap()
        .canonicalize()
        .unwrap_or_else(|_| std::env::current_dir().unwrap());
    assert_eq!(after, before);
}

#[test]
fn session_round_trip_through_the_warm_map() {
    let state = test_state();

    let s1 = Session::new("/tmp".into(), "sys".into());
    let id1 = s1.id.clone();
    state.insert_session(s1);
    assert!(state.get_session(&id1).is_some());
    assert_eq!(state.list_session_ids(), vec![id1.clone()]);

    let s2 = Session::new("/tmp".into(), "sys".into());
    let id2 = s2.id.clone();
    state.insert_session(s2);
    let mut ids = state.list_session_ids();
    ids.sort();
    let mut want = vec![id1, id2];
    want.sort();
    assert_eq!(ids, want);

    assert!(state.get_session("missing").is_none());
}

#[test]
fn cancel_flags_register_take_and_request() {
    let state = test_state();
    assert!(!state.request_cancel("s1"));
    assert!(state.take_cancel("s1").is_none());

    state.register_cancel("s1", new_cancel_flag());
    assert!(state.request_cancel("s1"));
    assert!(state.take_cancel("s1").is_some());
    assert!(state.take_cancel("s1").is_none());
    assert!(!state.request_cancel("s1"));
}

#[test]
fn poisoned_maps_are_treated_as_empty() {
    let state = test_state();
    poison_mutex(&state.sessions);
    poison_mutex(&state.cancel_flags);
    let s = Session::new("/tmp".into(), "sys".into());
    let handle = state.insert_session(s);
    assert!(state.get_session("anything").is_none());
    assert!(state.list_session_ids().is_empty());
    state.register_cancel("s1", new_cancel_flag());
    assert!(!state.request_cancel("s1"));
    assert!(state.take_cancel("s1").is_none());
    drop(handle);
}

#[test]
fn db_path_follows_isolated_home() {
    let home = IsolatedHome::new();
    let path = AppState::db_path().expect("db path");
    assert_eq!(path, home.path().join("whycodes.db"));
    let db = AppState::open_db().expect("open isolated db");
    drop(db);
}

#[test]
fn isolated_home_restores_previous_env() {
    let sentinel = std::ffi::OsString::from("/tmp/whycodes-prev-home");
    let mut home = IsolatedHome::new();
    home.set_prev(Some(sentinel.clone()));
    home.restore_env();
    assert_eq!(
        std::env::var_os("WHYCODES_HOME").as_deref(),
        Some(sentinel.as_os_str())
    );
    // Drop must not leak the sentinel once the lock is released.
    unsafe { std::env::remove_var("WHYCODES_HOME") };
    home.set_prev(None);
}

#[test]
fn lock_env_recovers_from_poison() {
    poison_mutex(&ENV_LOCK);
    let _g = lock_env();
}
