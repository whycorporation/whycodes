//! In-process axum stand-in for `whycodes serve` — covers the HTTP client
//! without spawning the binary.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use axum::Router;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{Json, response::IntoResponse};
use serde_json::json;
use tokio::net::TcpListener;
use whycodes_protocol::sdk::{
    Handshake, PROTOCOL_MAJOR, QuestionAnswerWire, SdkEvent, SessionHistory, SessionInfo,
    SessionList,
};

use crate::{ErrorCode, LaunchOptions, RunOptions, WhyCodesClient};

#[derive(Clone)]
struct Mode(Arc<AtomicU8>);

const MODE_OK: u8 = 0;
const MODE_BAD_PROTOCOL: u8 = 1;
const MODE_NO_HEALTH: u8 = 2;

async fn health(axum::extract::State(mode): axum::extract::State<Mode>) -> impl IntoResponse {
    match mode.0.load(Ordering::SeqCst) {
        MODE_NO_HEALTH => StatusCode::NOT_FOUND.into_response(),
        MODE_BAD_PROTOCOL => Json(Handshake {
            protocol: 99,
            version: "9.0.0".into(),
            healthy: true,
            project: "/tmp".into(),
            uptime_secs: 1,
            sessions_in_memory: 0,
        })
        .into_response(),
        _ => Json(Handshake {
            protocol: PROTOCOL_MAJOR,
            version: "0.1.0".into(),
            healthy: true,
            project: "/tmp".into(),
            uptime_secs: 1,
            sessions_in_memory: 1,
        })
        .into_response(),
    }
}

async fn list_sessions() -> Json<SessionList> {
    Json(SessionList {
        sessions: vec![SessionInfo {
            id: "s1".into(),
            title: "one".into(),
            project: "/tmp".into(),
            messages: Some(0),
            updated_at: None,
            source: Some("memory".into()),
        }],
    })
}

async fn create_session() -> Json<SessionInfo> {
    Json(SessionInfo {
        id: "s-new".into(),
        title: "new".into(),
        project: "/tmp".into(),
        messages: Some(0),
        updated_at: None,
        source: Some("memory".into()),
    })
}

async fn get_session(Path(id): Path<String>) -> Result<Json<SessionInfo>, StatusCode> {
    if id == "missing" {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(SessionInfo {
        id,
        title: "got".into(),
        project: "/tmp".into(),
        messages: Some(2),
        updated_at: None,
        source: Some("memory".into()),
    }))
}

async fn history(Path(id): Path<String>) -> Result<Json<SessionHistory>, StatusCode> {
    if id == "missing" {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(SessionHistory {
        id,
        title: "h".into(),
        messages: vec![],
    }))
}

async fn list_models() -> Json<serde_json::Value> {
    Json(json!({"models": [], "providers": ["anthropic"]}))
}

async fn set_model(Path(id): Path<String>) -> StatusCode {
    if id == "missing" {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::NO_CONTENT
    }
}

async fn rename(Path(id): Path<String>) -> Result<Json<SessionInfo>, StatusCode> {
    if id == "missing" {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(SessionInfo {
        id,
        title: "renamed".into(),
        project: "/tmp".into(),
        messages: Some(0),
        updated_at: None,
        source: Some("memory".into()),
    }))
}

async fn rewind(Path(id): Path<String>) -> Result<Json<SessionHistory>, StatusCode> {
    if id == "missing" {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(SessionHistory {
        id,
        title: "rw".into(),
        messages: vec![],
    }))
}

async fn compact(Path(id): Path<String>) -> Result<Json<SessionHistory>, StatusCode> {
    if id == "missing" {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(SessionHistory {
        id,
        title: "c".into(),
        messages: vec![],
    }))
}

async fn cancel(Path(id): Path<String>) -> StatusCode {
    if id == "missing" {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::ACCEPTED
    }
}

async fn permission(Path(id): Path<String>) -> StatusCode {
    if id == "missing" {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::NO_CONTENT
    }
}

async fn question(Path(id): Path<String>) -> StatusCode {
    if id == "missing" {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::NO_CONTENT
    }
}

async fn run(Path(id): Path<String>) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if id == "missing" {
        return Err(StatusCode::NOT_FOUND);
    }
    let events = [
        SdkEvent::TextDelta { text: "hi".into() },
        SdkEvent::ToolStart {
            id: "t1".into(),
            name: "read".into(),
            input: json!({}),
        },
        SdkEvent::ToolEnd {
            id: "t1".into(),
            content: "ok".into(),
            is_error: false,
        },
        SdkEvent::Usage {
            input_tokens: 1,
            output_tokens: 2,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
        },
        SdkEvent::TurnDone {
            text: "done".into(),
        },
    ];
    let stream = futures::stream::iter(events.into_iter().map(|ev| {
        let data = serde_json::to_string(&ev).unwrap();
        Ok::<_, std::convert::Infallible>(Event::default().data(data))
    }));
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

async fn spawn_ok() -> (String, tokio::task::JoinHandle<()>) {
    let mode = Mode(Arc::new(AtomicU8::new(MODE_OK)));
    bind_with(mode).await
}

async fn bind_with(mode: Mode) -> (String, tokio::task::JoinHandle<()>) {
    let app = Router::new()
        .route("/v1/health", get(health))
        .route("/v1/sessions", get(list_sessions).post(create_session))
        .route("/v1/sessions/{id}", get(get_session))
        .route("/v1/sessions/{id}/messages", get(history))
        .route("/v1/sessions/{id}/model", post(set_model))
        .route("/v1/sessions/{id}/rename", post(rename))
        .route("/v1/sessions/{id}/rewind", post(rewind))
        .route("/v1/sessions/{id}/compact", post(compact))
        .route("/v1/sessions/{id}/cancel", post(cancel))
        .route("/v1/sessions/{id}/permission", post(permission))
        .route("/v1/sessions/{id}/question", post(question))
        .route("/v1/sessions/{id}/run", post(run))
        .route("/v1/models", get(list_models))
        .with_state(mode);
    bind_app(app).await
}

async fn bind_app(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (format!("http://127.0.0.1:{}", addr.port()), handle)
}

async fn healthy() -> Json<Handshake> {
    Json(Handshake {
        protocol: PROTOCOL_MAJOR,
        version: "test".into(),
        healthy: true,
        project: "/test".into(),
        uptime_secs: 1,
        sessions_in_memory: 0,
    })
}

fn sse(
    events: impl IntoIterator<Item = SdkEvent>,
) -> Sse<futures::stream::Iter<std::vec::IntoIter<Result<Event, std::convert::Infallible>>>> {
    let events: Vec<_> = events
        .into_iter()
        .map(|event| Ok(Event::default().data(serde_json::to_string(&event).unwrap())))
        .collect();
    Sse::new(futures::stream::iter(events))
}

#[tokio::test]
async fn connect_and_session_surface() {
    let (base, _h) = spawn_ok().await;
    let c = WhyCodesClient::connect(&base).await.expect("connect");
    assert!(c.base_url().contains("127.0.0.1"));
    let hs = c.health().await.unwrap();
    assert_eq!(hs.protocol, PROTOCOL_MAJOR);

    let listed = c.list_sessions().await.unwrap();
    assert_eq!(listed[0].id, "s1");

    let created = c.create_session(Some("/tmp")).await.unwrap();
    assert_eq!(created.id, "s-new");
    let _ = c.create_session(None::<String>).await.unwrap();

    let got = c.get_session("abc").await.unwrap();
    assert_eq!(got.title, "got");
    let miss = c.get_session("missing").await.unwrap_err();
    assert_eq!(miss.code, ErrorCode::UnknownSession);

    let hist = c.get_history("abc", Some(3)).await.unwrap();
    assert!(hist.messages.is_empty());
    assert!(c.peek("abc", 1).await.unwrap().is_empty());
    assert_eq!(
        c.get_history("missing", None).await.unwrap_err().code,
        ErrorCode::UnknownSession
    );

    let models = c.list_models().await.unwrap();
    assert!(models.providers.iter().any(|p| p == "anthropic"));

    c.set_model("abc", "openai", "gpt-4o").await.unwrap();
    assert_eq!(
        c.set_model("missing", "openai", "gpt-4o")
            .await
            .unwrap_err()
            .code,
        ErrorCode::UnknownSession
    );

    let renamed = c.rename_session("abc", "n").await.unwrap();
    assert_eq!(renamed.title, "renamed");
    assert!(c.rename_session("missing", "n").await.is_err());

    c.rewind("abc", 0).await.unwrap();
    assert!(c.rewind("missing", 0).await.is_err());
    c.compact("abc", Some(10)).await.unwrap();
    assert!(c.compact("missing", None).await.is_err());

    c.cancel("abc").await.unwrap();
    assert_eq!(
        c.cancel("missing").await.unwrap_err().code,
        ErrorCode::UnknownSession
    );
    c.respond_to_permission(
        "abc",
        "r1",
        whycodes_protocol::sdk::PermissionDecision::Allow,
    )
    .await
    .unwrap();
    assert!(
        c.respond_to_permission(
            "missing",
            "r1",
            whycodes_protocol::sdk::PermissionDecision::Deny
        )
        .await
        .is_err()
    );
    c.respond_to_question("abc", "q1", None, false)
        .await
        .unwrap();
    assert!(
        c.respond_to_question("missing", "q1", None, true)
            .await
            .is_err()
    );

    let turn = c.run("abc", "hello", RunOptions::default()).await.unwrap();
    assert!(turn.text.contains("hi") || turn.text.contains("done"));
    assert_eq!(turn.tool_calls.len(), 1);
    assert!(!turn.cancelled);
    assert!(turn.usage.is_some());

    assert_eq!(
        c.run("missing", "x", RunOptions::default())
            .await
            .unwrap_err()
            .code,
        ErrorCode::UnknownSession
    );

    c.close().await.unwrap();
}

#[tokio::test]
async fn handshake_rejects_wrong_protocol_and_missing_route() {
    let mode = Mode(Arc::new(AtomicU8::new(MODE_BAD_PROTOCOL)));
    let (base, _h) = bind_with(mode.clone()).await;
    let err = match WhyCodesClient::connect(&base).await {
        Err(e) => e,
        Ok(_) => panic!("expected unsupported protocol"),
    };
    assert_eq!(err.code, ErrorCode::UnsupportedVersion);

    mode.0.store(MODE_NO_HEALTH, Ordering::SeqCst);
    let (base2, _h2) = bind_with(mode).await;
    let err = match WhyCodesClient::connect(&base2).await {
        Err(e) => e,
        Ok(_) => panic!("expected missing /v1/health"),
    };
    assert_eq!(err.code, ErrorCode::UnsupportedVersion);
}

#[tokio::test]
async fn connect_refused_is_disconnected() {
    let err = match WhyCodesClient::connect("127.0.0.1:1").await {
        Err(e) => e,
        Ok(_) => panic!("expected disconnect"),
    };
    assert_eq!(err.code, ErrorCode::Disconnected);
}

#[tokio::test]
async fn request_options_are_sent_as_protocol_json() {
    async fn create(Json(body): Json<serde_json::Value>) -> impl IntoResponse {
        assert_eq!(body, json!({"project":"/project","persist":true}));
        Json(json!({
            "id": "created",
            "title": "new",
            "project": "/project",
            "messages": 0
        }))
    }

    async fn model(Json(body): Json<serde_json::Value>) -> StatusCode {
        assert_eq!(body, json!({"provider":"openai","model":"gpt-test"}));
        StatusCode::NO_CONTENT
    }

    async fn compact_body(Json(body): Json<serde_json::Value>) -> Json<serde_json::Value> {
        assert_eq!(body, json!({"max_tokens":321}));
        Json(json!({"id":"s1","title":"compact","messages":[]}))
    }

    async fn run_body(
        Json(body): Json<serde_json::Value>,
    ) -> Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>> {
        assert_eq!(
            body,
            json!({
                "message": "hello",
                "provider": "anthropic",
                "model": "claude-test",
                "max_turns": 4,
                "auto_approve": false
            })
        );
        sse([SdkEvent::TurnDone { text: "ok".into() }])
    }

    let app = Router::new()
        .route("/v1/health", get(healthy))
        .route("/v1/sessions", post(create))
        .route("/v1/sessions/{id}/model", post(model))
        .route("/v1/sessions/{id}/compact", post(compact_body))
        .route("/v1/sessions/{id}/run", post(run_body));
    let (base, _server) = bind_app(app).await;
    let client = WhyCodesClient::connect(base).await.unwrap();

    assert_eq!(
        client.create_session(Some("/project")).await.unwrap().id,
        "created"
    );
    client.set_model("s1", "openai", "gpt-test").await.unwrap();
    assert_eq!(client.compact("s1", Some(321)).await.unwrap().id, "s1");
    let result = client
        .run(
            "s1",
            "hello",
            RunOptions {
                provider: Some("anthropic".into()),
                model: Some("claude-test".into()),
                max_turns: Some(4),
                auto_approve: Some(false),
            },
        )
        .await
        .unwrap();
    assert_eq!(result.text, "ok");
}

#[tokio::test]
async fn http_and_decode_failures_return_stable_error_codes() {
    async fn unauthorized() -> StatusCode {
        StatusCode::UNAUTHORIZED
    }
    async fn bad_request() -> StatusCode {
        StatusCode::BAD_REQUEST
    }
    async fn server_error() -> StatusCode {
        StatusCode::SERVICE_UNAVAILABLE
    }
    async fn invalid_json() -> &'static str {
        "not json"
    }

    let app = Router::new()
        .route("/v1/health", get(healthy))
        .route("/v1/sessions", get(unauthorized).post(bad_request))
        .route("/v1/models", get(invalid_json))
        .route("/v1/sessions/{id}/cancel", post(server_error));
    let (base, _server) = bind_app(app).await;
    let client = WhyCodesClient::connect(base).await.unwrap();

    let auth = client.list_sessions().await.unwrap_err();
    assert_eq!(auth.code, ErrorCode::Auth);
    assert!(auth.message.contains("list sessions failed: 401"));

    let invalid = client.create_session(None::<String>).await.unwrap_err();
    assert_eq!(invalid.code, ErrorCode::InvalidRequest);
    assert!(invalid.message.contains("create session failed: 400"));

    let internal = client.cancel("s1").await.unwrap_err();
    assert_eq!(internal.code, ErrorCode::Internal);
    assert!(internal.message.contains("cancel failed: 503"));

    let decode = client.list_models().await.unwrap_err();
    assert_eq!(decode.code, ErrorCode::Disconnected);
    assert!(decode.source.is_some());
}

#[tokio::test]
async fn run_collects_cancel_and_error_event_branches() {
    async fn events(
        Path(id): Path<String>,
    ) -> Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>> {
        let events = if id == "error" {
            vec![
                SdkEvent::TextDelta {
                    text: "partial".into(),
                },
                SdkEvent::Error {
                    code: ErrorCode::Auth,
                    message: "provider rejected credentials".into(),
                },
                SdkEvent::TurnDone {
                    text: String::new(),
                },
            ]
        } else if id == "empty-done" {
            vec![SdkEvent::TurnDone {
                text: String::new(),
            }]
        } else {
            vec![
                SdkEvent::Cancelled,
                SdkEvent::ToolEnd {
                    id: "orphan".into(),
                    content: "stopped".into(),
                    is_error: true,
                },
                SdkEvent::TurnDone {
                    text: "fallback".into(),
                },
            ]
        };
        sse(events)
    }

    let app = Router::new()
        .route("/v1/health", get(healthy))
        .route("/v1/sessions/{id}/run", post(events));
    let (base, _server) = bind_app(app).await;
    let client = WhyCodesClient::connect(base).await.unwrap();

    let cancelled = client
        .run("cancelled", "go", RunOptions::default())
        .await
        .unwrap();
    assert!(cancelled.cancelled);
    assert_eq!(cancelled.text, "fallback");
    assert_eq!(cancelled.tool_calls[0].id, "orphan");
    assert_eq!(cancelled.tool_calls[0].name, "");
    assert!(cancelled.tool_calls[0].is_error);

    let error = client
        .run("error", "go", RunOptions::default())
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Auth);
    assert_eq!(error.message, "provider rejected credentials");

    let empty = client
        .run("empty-done", "go", RunOptions::default())
        .await
        .unwrap();
    assert!(empty.text.is_empty());
}

#[test]
fn unused_launch_options_fields_exist() {
    let _ = LaunchOptions {
        port: Some(0),
        home: Some(std::path::PathBuf::from("/tmp")),
        ..Default::default()
    };
}

#[tokio::test]
async fn non_404_failures_and_run_event_status_codes() {
    async fn get_session() -> StatusCode {
        StatusCode::INTERNAL_SERVER_ERROR
    }
    async fn history() -> StatusCode {
        StatusCode::FORBIDDEN
    }
    async fn set_model() -> StatusCode {
        StatusCode::INTERNAL_SERVER_ERROR
    }
    async fn rename() -> StatusCode {
        StatusCode::BAD_GATEWAY
    }
    async fn rewind() -> StatusCode {
        StatusCode::INTERNAL_SERVER_ERROR
    }
    async fn compact() -> StatusCode {
        StatusCode::INTERNAL_SERVER_ERROR
    }
    async fn permission() -> StatusCode {
        StatusCode::INTERNAL_SERVER_ERROR
    }
    async fn question() -> StatusCode {
        StatusCode::BAD_REQUEST
    }
    async fn models() -> StatusCode {
        StatusCode::INTERNAL_SERVER_ERROR
    }
    async fn run(Path(id): Path<String>) -> StatusCode {
        if id == "empty" {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }

    let app = Router::new()
        .route("/v1/health", get(healthy))
        .route("/v1/sessions/{id}", get(get_session))
        .route("/v1/sessions/{id}/messages", get(history))
        .route("/v1/sessions/{id}/model", post(set_model))
        .route("/v1/sessions/{id}/rename", post(rename))
        .route("/v1/sessions/{id}/rewind", post(rewind))
        .route("/v1/sessions/{id}/compact", post(compact))
        .route("/v1/sessions/{id}/permission", post(permission))
        .route("/v1/sessions/{id}/question", post(question))
        .route("/v1/sessions/{id}/run", post(run))
        .route("/v1/models", get(models));
    let (base, _server) = bind_app(app).await;
    let client = WhyCodesClient::connect(&base).await.unwrap();

    assert_eq!(
        client.get_session("fail").await.unwrap_err().code,
        ErrorCode::Internal
    );
    assert_eq!(
        client.list_models().await.unwrap_err().code,
        ErrorCode::Internal
    );
    assert_eq!(
        client.get_history("fail", None).await.unwrap_err().code,
        ErrorCode::Internal
    );
    assert_eq!(
        client
            .set_model("fail", "openai", "m")
            .await
            .unwrap_err()
            .code,
        ErrorCode::Internal
    );
    assert_eq!(
        client.rename_session("fail", "t").await.unwrap_err().code,
        ErrorCode::Internal
    );
    assert_eq!(
        client.rewind("fail", 0).await.unwrap_err().code,
        ErrorCode::Internal
    );
    assert_eq!(
        client.compact("fail", None).await.unwrap_err().code,
        ErrorCode::Internal
    );
    assert_eq!(
        client
            .respond_to_permission(
                "fail",
                "r",
                whycodes_protocol::sdk::PermissionDecision::AllowAlways
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::Internal
    );
    assert_eq!(
        client
            .respond_to_question(
                "fail",
                "q",
                Some(vec![QuestionAnswerWire {
                    selected: vec!["a".into()],
                    free_text: Some("b".into()),
                }]),
                false
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        client
            .run("empty", "", RunOptions::default())
            .await
            .unwrap_err()
            .code,
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        client
            .run("fail", "x", RunOptions::default())
            .await
            .unwrap_err()
            .code,
        ErrorCode::Internal
    );
}

#[tokio::test]
async fn health_non_success_and_timeout_map_error_codes() {
    async fn unhealthy() -> StatusCode {
        StatusCode::SERVICE_UNAVAILABLE
    }
    async fn hang() {
        tokio::time::sleep(Duration::from_secs(30)).await;
    }

    let app = Router::new().route("/v1/health", get(unhealthy));
    let (base, _server) = bind_app(app).await;
    let err = match WhyCodesClient::connect(&base).await {
        Err(e) => e,
        Ok(_) => panic!("expected health failure"),
    };
    assert_eq!(err.code, ErrorCode::Internal);

    let app = Router::new().route("/v1/health", get(hang));
    let (base, _server) = bind_app(app).await;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_millis(80))
        .build()
        .unwrap();
    let client = WhyCodesClient::unconnected(&base, http);
    let err = client.health().await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Timeout);
    assert!(err.source.is_some());

    async fn not_json() -> &'static str {
        "not-handshake"
    }
    let app = Router::new().route("/v1/health", get(not_json));
    let (base, _server) = bind_app(app).await;
    let err = match WhyCodesClient::connect(&base).await {
        Err(e) => e,
        Ok(_) => panic!("expected handshake decode failure"),
    };
    assert_eq!(err.code, ErrorCode::Disconnected);
}

#[tokio::test]
async fn run_covers_ignored_events_and_keeps_delta_over_turn_done() {
    async fn events() -> impl IntoResponse {
        let mut frames: Vec<String> = [
            SdkEvent::TextDelta {
                text: "keep".into(),
            },
            SdkEvent::ReasoningDelta {
                text: "think".into(),
            },
            SdkEvent::Status {
                message: "working".into(),
            },
            SdkEvent::Intent {
                kind: "code".into(),
                confidence: 0.9,
                badge: String::new(),
                notice_kind: String::new(),
                notice: String::new(),
            },
            SdkEvent::FileConflict {
                path: "a.rs".into(),
                claimant: "a".into(),
                owner: "b".into(),
            },
            SdkEvent::SwarmStatus {
                active: 1,
                total: 2,
                message: "go".into(),
            },
            SdkEvent::Background {
                id: "bg".into(),
                status: "running".into(),
                summary: "s".into(),
            },
            SdkEvent::PermissionRequest {
                request_id: "p".into(),
                tool_name: "read".into(),
                detail: "d".into(),
            },
            SdkEvent::QuestionRequest {
                request_id: "q".into(),
                questions: json!([]),
            },
            SdkEvent::TurnDone {
                text: "ignored".into(),
            },
        ]
        .into_iter()
        .map(|event| serde_json::to_string(&event).unwrap())
        .collect();
        frames.insert(frames.len() - 1, r#"{"ev":"future_event"}"#.into());
        Sse::new(futures::stream::iter(frames.into_iter().map(|data| {
            Ok::<_, std::convert::Infallible>(Event::default().data(data))
        })))
    }

    let app = Router::new()
        .route("/v1/health", get(healthy))
        .route("/v1/sessions/{id}/run", post(events));
    let (base, _server) = bind_app(app).await;
    let client = WhyCodesClient::connect(base).await.unwrap();
    let turn = client.run("s1", "go", RunOptions::default()).await.unwrap();
    assert_eq!(turn.text, "keep");
}

#[tokio::test]
async fn run_stream_timeout_maps_to_timeout_code() {
    async fn hang() {
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
    let app = Router::new()
        .route("/v1/health", get(healthy))
        .route("/v1/sessions/{id}/run", post(hang));
    let (base, _server) = bind_app(app).await;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_millis(80))
        .build()
        .unwrap();
    let client = WhyCodesClient::unconnected(&base, http);
    let err = client
        .run("s1", "go", RunOptions::default())
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Timeout);
}

#[tokio::test]
async fn run_events_stream_error_maps_disconnected() {
    async fn blank_then_done() -> impl IntoResponse {
        let stream = futures::stream::iter([
            Ok::<_, std::convert::Infallible>(Event::default().comment("keep-alive")),
            Ok(Event::default().data(r#"{"ev":"turn_done","text":"from-blank"}"#)),
        ]);
        Sse::new(stream)
    }
    let app = Router::new()
        .route("/v1/health", get(healthy))
        .route("/v1/sessions/{id}/run", post(blank_then_done));
    let (base, _server) = bind_app(app).await;
    let client = WhyCodesClient::connect(base).await.unwrap();
    let turn = client
        .run("blank", "go", RunOptions::default())
        .await
        .unwrap();
    assert_eq!(turn.text, "from-blank");
    async fn broken() -> impl IntoResponse {
        let stream = futures::stream::iter([
            Ok::<_, std::io::Error>(axum::body::Bytes::from("data: {\"ev\":\"cancelled\"}\n\n")),
            Err(std::io::Error::other("truncated")),
        ]);
        (
            [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
            axum::body::Body::from_stream(stream),
        )
    }

    let app = Router::new()
        .route("/v1/health", get(healthy))
        .route("/v1/sessions/{id}/run", post(broken));
    let (base, _server) = bind_app(app).await;
    let client = WhyCodesClient::connect(base).await.unwrap();
    let err = client
        .run("s1", "go", RunOptions::default())
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Disconnected);
}

#[tokio::test]
async fn run_structured_schema_retry_and_success_paths() {
    async fn scripted(
        Path(id): Path<String>,
        Json(body): Json<serde_json::Value>,
    ) -> Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>> {
        let msg = body.get("message").and_then(|v| v.as_str()).unwrap_or("");
        let retry = msg.contains("previous reply");
        let text = match id.as_str() {
            "ok" => "{\"n\":1}".to_string(),
            "retry-ok" => {
                if retry {
                    "{\"n\":2}".into()
                } else {
                    "not json".into()
                }
            }
            "bad-then-ok" => {
                if retry {
                    "{\"n\":3}".into()
                } else {
                    "{\"n\":\"x\"}".into()
                }
            }
            "always-bad" => "nope".into(),
            "always-schema" => "{\"n\":\"x\"}".into(),
            other => panic!("unexpected session {other}"),
        };
        sse([SdkEvent::TurnDone { text }])
    }

    let schema = json!({"type":"object","properties":{"n":{"type":"integer"}},"required":["n"]});
    let app = Router::new()
        .route("/v1/health", get(healthy))
        .route("/v1/sessions/{id}/run", post(scripted));
    let (base, _server) = bind_app(app).await;
    let client = WhyCodesClient::connect(base).await.unwrap();

    let invalid = client
        .run_structured(
            "ok",
            "give json",
            json!(["not", "object"]),
            RunOptions::default(),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(invalid.code, ErrorCode::StructuredSchemaInvalid);

    let ok = client
        .run_structured(
            "ok",
            "give json",
            schema.clone(),
            RunOptions::default(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(ok.data["n"], 1);
    assert_eq!(ok.attempts.len(), 1);
    assert!(ok.attempts[0].ok);

    let recovered = client
        .run_structured(
            "retry-ok",
            "give json",
            schema.clone(),
            RunOptions::default(),
            Some(1),
        )
        .await
        .unwrap();
    assert_eq!(recovered.data["n"], 2);
    assert_eq!(recovered.attempts.len(), 2);
    assert!(!recovered.attempts[0].ok);

    let schema_recovered = client
        .run_structured(
            "bad-then-ok",
            "give json",
            schema.clone(),
            RunOptions::default(),
            Some(1),
        )
        .await
        .unwrap();
    assert_eq!(schema_recovered.data["n"], 3);

    let parse_fail = client
        .run_structured(
            "always-bad",
            "give json",
            schema.clone(),
            RunOptions::default(),
            Some(0),
        )
        .await
        .unwrap_err();
    assert_eq!(parse_fail.code, ErrorCode::StructuredOutputInvalid);

    let schema_fail = client
        .run_structured(
            "always-schema",
            "give json",
            schema,
            RunOptions::default(),
            Some(0),
        )
        .await
        .unwrap_err();
    assert_eq!(schema_fail.code, ErrorCode::StructuredOutputInvalid);
}
