//! In-process HTTP coverage for `/api/*` and `/v1/*` (no live LLM).

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;
use whycodes_core::types::ContentBlock;
use whycodes_session::session::Session;

use crate::{create_router, test_state};

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
