//! Integration tests for MCP Streamable HTTP and legacy HTTP+SSE transports.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use tokio::sync::{Mutex, mpsc};
use whycode_mcp::McpClient;
use whycode_mcp::http::StreamableHttpTransport;

#[derive(Clone)]
struct MockState {
    session: Arc<Mutex<Option<String>>>,
    tool_name: String,
    sse_tx: Arc<Mutex<Option<mpsc::UnboundedSender<String>>>>,
    posts: Arc<Mutex<Vec<String>>>,
    next_session: Arc<AtomicU64>,
}

impl MockState {
    fn new() -> Self {
        Self {
            session: Arc::new(Mutex::new(None)),
            tool_name: "echo".into(),
            sse_tx: Arc::new(Mutex::new(None)),
            posts: Arc::new(Mutex::new(Vec::new())),
            next_session: Arc::new(AtomicU64::new(1)),
        }
    }
}

fn json_rpc_result(id: u64, result: serde_json::Value) -> String {
    serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

fn handle_mcp_method(
    method: &str,
    id: u64,
    params: Option<&serde_json::Value>,
    tool: &str,
) -> serde_json::Value {
    match method {
        "initialize" => serde_json::json!({
            "protocolVersion": "2025-03-26",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "mock-mcp", "version": "0.0.1" }
        }),
        "tools/list" => serde_json::json!({
            "tools": [{
                "name": tool,
                "description": "Echo tool",
                "inputSchema": {
                    "type": "object",
                    "properties": { "text": { "type": "string" } }
                }
            }]
        }),
        "tools/call" => {
            let text = params
                .and_then(|p| p.get("arguments"))
                .and_then(|a| a.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            serde_json::json!({
                "content": [{ "type": "text", "text": format!("echo:{text}") }],
                "isError": false
            })
        }
        "ping" => serde_json::json!({}),
        "resources/list" => serde_json::json!({
            "resources": [{
                "uri": "file:///guide.md",
                "name": "guide",
                "description": "Local guide",
                "mimeType": "text/markdown"
            }]
        }),
        "prompts/list" => serde_json::json!({
            "prompts": [{
                "name": "review",
                "description": "Review code",
                "arguments": [{"name": "path", "required": true}]
            }]
        }),
        other => serde_json::json!({ "error": format!("unknown method {other}"), "id": id }),
    }
}

async fn streamable_post(
    State(state): State<MockState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    state.posts.lock().await.push(body.clone());
    let value: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid json").into_response(),
    };
    if value.get("id").is_none() {
        return StatusCode::ACCEPTED.into_response();
    }
    let id = value.get("id").and_then(|i| i.as_u64()).unwrap_or(0);
    let method = value
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();
    let params = value.get("params").cloned();

    if method == "initialize" {
        let sid = format!("sess-{}", state.next_session.fetch_add(1, Ordering::SeqCst));
        *state.session.lock().await = Some(sid.clone());
        let result = handle_mcp_method(&method, id, params.as_ref(), &state.tool_name);
        let body = json_rpc_result(id, result);
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .header("mcp-session-id", sid)
            .body(Body::from(body))
            .unwrap()
            .into_response();
    }

    if let Some(expected) = state.session.lock().await.clone() {
        let got = headers
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if got != expected {
            return (StatusCode::BAD_REQUEST, "missing or wrong session").into_response();
        }
    }

    if method == "tools/call" {
        let result = handle_mcp_method(&method, id, params.as_ref(), &state.tool_name);
        let payload = json_rpc_result(id, result);
        let sse = format!("event: message\ndata: {payload}\n\n");
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(sse))
            .unwrap()
            .into_response();
    }

    let result = handle_mcp_method(&method, id, params.as_ref(), &state.tool_name);
    let body = json_rpc_result(id, result);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap()
        .into_response()
}

async fn spawn_streamable_server() -> (SocketAddr, MockState) {
    let state = MockState::new();
    let app = Router::new()
        .route("/mcp", post(streamable_post))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::task::yield_now().await;
    (addr, state)
}

async fn legacy_sse_get(State(state): State<MockState>) -> Response {
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    *state.sse_tx.lock().await = Some(tx);
    let stream = async_stream::stream! {
        yield Ok::<_, std::io::Error>("event: endpoint\ndata: /messages?sessionId=legacy-1\n\n".to_string());
        while let Some(msg) = rx.recv().await {
            yield Ok::<_, std::io::Error>(format!("event: message\ndata: {msg}\n\n"));
        }
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}

async fn legacy_messages_post(State(state): State<MockState>, body: String) -> impl IntoResponse {
    state.posts.lock().await.push(body.clone());
    let value: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return StatusCode::BAD_REQUEST,
    };
    if value.get("id").is_none() {
        return StatusCode::ACCEPTED;
    }
    let id = value.get("id").and_then(|i| i.as_u64()).unwrap_or(0);
    let method = value
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();
    let params = value.get("params").cloned();
    let result = handle_mcp_method(&method, id, params.as_ref(), &state.tool_name);
    let payload = json_rpc_result(id, result);
    if let Some(tx) = state.sse_tx.lock().await.as_ref() {
        let _ = tx.send(payload);
    }
    StatusCode::ACCEPTED
}

async fn spawn_legacy_sse_server() -> SocketAddr {
    let state = MockState::new();
    let app = Router::new()
        .route("/sse", get(legacy_sse_get))
        .route("/messages", post(legacy_messages_post))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::task::yield_now().await;
    addr
}

async fn method_not_allowed(_req: Request) -> impl IntoResponse {
    StatusCode::METHOD_NOT_ALLOWED
}

async fn spawn_sse_only_with_http_405() -> SocketAddr {
    let state = MockState::new();
    let app = Router::new()
        .route("/sse", get(legacy_sse_get).post(method_not_allowed))
        .route("/messages", post(legacy_messages_post))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::task::yield_now().await;
    addr
}

async fn spawn_app(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::task::yield_now().await;
    addr
}

#[tokio::test]
async fn streamable_http_initialize_list_and_call() {
    let (addr, state) = spawn_streamable_server().await;
    let url = format!("http://{addr}/mcp");
    let mut client = McpClient::connect_http(&url, &HashMap::new())
        .await
        .expect("connect_http");
    assert_eq!(client.transport_name(), "http");
    let tools = client.list_tools().await.expect("list_tools");
    assert_eq!(tools[0].name, "echo");
    let out = client
        .call_tool("echo", serde_json::json!({"text": "hi"}))
        .await
        .expect("call_tool");
    assert_eq!(out, "echo:hi");
    assert!(state.session.lock().await.is_some());
}

#[tokio::test]
async fn streamable_http_ping() {
    let (addr, _) = spawn_streamable_server().await;
    let mut client = McpClient::connect_http(&format!("http://{addr}/mcp"), &HashMap::new())
        .await
        .unwrap();
    client.ping().await.expect("ping");
}

#[tokio::test]
async fn client_lists_resources_and_prompts() {
    let (addr, _) = spawn_streamable_server().await;
    let mut client = McpClient::connect_http(&format!("http://{addr}/mcp"), &HashMap::new())
        .await
        .unwrap();

    let resources = client.list_resources().await.unwrap();
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0].uri, "file:///guide.md");
    assert_eq!(resources[0].mime_type.as_deref(), Some("text/markdown"));

    let prompts = client.list_prompts().await.unwrap();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].name, "review");
    let arguments = prompts[0].arguments.as_ref().unwrap();
    assert_eq!(arguments[0].name, "path");
    assert_eq!(arguments[0].required, Some(true));
}

#[tokio::test]
async fn streamable_transport_tracks_session_and_increments_request_ids() {
    type RecordedRequests = Vec<(u64, Option<String>)>;

    #[derive(Clone, Default)]
    struct RequestState(Arc<Mutex<RecordedRequests>>);

    async fn respond(
        State(state): State<RequestState>,
        headers: HeaderMap,
        body: String,
    ) -> Response {
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        let id = value["id"].as_u64().unwrap();
        let session = headers
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        state.0.lock().await.push((id, session));
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .header("mcp-session-id", "session-42")
            .body(Body::from(json_rpc_result(
                id,
                serde_json::json!({"id": id}),
            )))
            .unwrap()
    }

    let state = RequestState::default();
    let app = Router::new()
        .route("/mcp", post(respond))
        .with_state(state.clone());
    let addr = spawn_app(app).await;
    let mut transport =
        StreamableHttpTransport::new(format!("http://{addr}/mcp"), &HashMap::new()).unwrap();

    assert_eq!(transport.session_id(), None);
    assert_eq!(
        transport.send_request("first", None).await.unwrap()["id"],
        1
    );
    assert_eq!(transport.session_id(), Some("session-42"));
    assert_eq!(
        transport.send_request("second", None).await.unwrap()["id"],
        2
    );
    assert_eq!(
        *state.0.lock().await,
        vec![(1, None), (2, Some("session-42".to_string()))]
    );
}

#[tokio::test]
async fn streamable_transport_surfaces_http_rpc_parse_and_notification_errors() {
    async fn fail(body: String) -> Response {
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        match value["method"].as_str().unwrap() {
            "http-error" => (StatusCode::BAD_GATEWAY, "upstream unavailable").into_response(),
            "rpc-error" => Response::builder()
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": value["id"],
                        "error": {"code": -32602, "message": "bad params"}
                    })
                    .to_string(),
                ))
                .unwrap(),
            "bad-json" => Response::builder()
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("not json"))
                .unwrap(),
            "notify" => (StatusCode::FORBIDDEN, "notifications disabled").into_response(),
            other => panic!("unexpected method {other}"),
        }
    }

    let addr = spawn_app(Router::new().route("/mcp", post(fail))).await;
    let mut transport =
        StreamableHttpTransport::new(format!("http://{addr}/mcp"), &HashMap::new()).unwrap();

    let error = transport
        .send_request("http-error", None)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("MCP HTTP error 502 Bad Gateway"));
    assert!(error.contains("upstream unavailable"));

    let error = transport
        .send_request("rpc-error", None)
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(error, "MCP error [-32602]: bad params");

    let error = transport
        .send_request("bad-json", None)
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(error, "failed to parse JSON-RPC response");

    let error = transport
        .send_notification("notify", Some(serde_json::json!({"ready": true})))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("MCP notification 'notify' failed with 403 Forbidden"));
    assert!(error.contains("notifications disabled"));
}

#[tokio::test]
async fn client_reports_invalid_initialize_payload_and_non_fallback_http_errors() {
    async fn invalid_initialize() -> Response {
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json_rpc_result(
                1,
                serde_json::json!({"wrong": true}),
            )))
            .unwrap()
    }
    let addr = spawn_app(Router::new().route("/mcp", post(invalid_initialize))).await;
    let error = McpClient::connect_http(format!("http://{addr}/mcp"), &HashMap::new())
        .await
        .err()
        .expect("invalid initialize payload should fail");
    assert!(format!("{error:#}").contains("failed to parse initialize result"));

    async fn unavailable() -> impl IntoResponse {
        (StatusCode::INTERNAL_SERVER_ERROR, "broken")
    }
    let addr = spawn_app(Router::new().route("/mcp", post(unavailable))).await;
    let error = McpClient::connect_auto(format!("http://{addr}/mcp"), &HashMap::new())
        .await
        .err()
        .expect("500 must not trigger legacy SSE fallback");
    let message = format!("{error:#}");
    assert!(message.contains("Streamable HTTP connect failed"));
    assert!(message.contains("500 Internal Server Error"));
}

#[tokio::test]
async fn legacy_sse_initialize_list_and_call() {
    let addr = spawn_legacy_sse_server().await;
    let mut client = McpClient::connect_sse(&format!("http://{addr}/sse"), &HashMap::new())
        .await
        .expect("connect_sse");
    assert_eq!(client.transport_name(), "sse");
    let tools = client.list_tools().await.expect("list_tools");
    assert_eq!(tools[0].name, "echo");
    let out = client
        .call_tool("echo", serde_json::json!({"text": "sse"}))
        .await
        .expect("call_tool");
    assert_eq!(out, "echo:sse");
}

#[tokio::test]
async fn connect_auto_prefers_streamable_http() {
    let (addr, _) = spawn_streamable_server().await;
    let client = McpClient::connect_auto(&format!("http://{addr}/mcp"), &HashMap::new())
        .await
        .expect("connect_auto");
    assert_eq!(client.transport_name(), "http");
}

#[tokio::test]
async fn connect_auto_falls_back_to_sse_on_405() {
    let addr = spawn_sse_only_with_http_405().await;
    let mut client = McpClient::connect_auto(&format!("http://{addr}/sse"), &HashMap::new())
        .await
        .expect("connect_auto fallback");
    assert_eq!(client.transport_name(), "sse");
    let tools = client.list_tools().await.unwrap();
    assert_eq!(tools[0].name, "echo");
}

#[tokio::test]
async fn streamable_http_forwards_custom_headers() {
    async fn authed(headers: HeaderMap, body: String) -> Response {
        let auth = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if auth != "Bearer secret-token" {
            return (StatusCode::UNAUTHORIZED, "nope").into_response();
        }
        let value: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        if value.get("id").is_none() {
            return StatusCode::ACCEPTED.into_response();
        }
        let id = value.get("id").and_then(|i| i.as_u64()).unwrap_or(0);
        let method = value.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let result = handle_mcp_method(method, id, value.get("params"), "echo");
        let body = json_rpc_result(id, result);
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .header("mcp-session-id", "authed-1")
            .body(Body::from(body))
            .unwrap()
            .into_response()
    }
    let app = Router::new().route("/mcp", post(authed));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::task::yield_now().await;
    let mut headers = HashMap::new();
    headers.insert("Authorization".into(), "Bearer secret-token".into());
    let mut client = McpClient::connect_http(&format!("http://{addr}/mcp"), &headers)
        .await
        .unwrap();
    let tools = client.list_tools().await.unwrap();
    assert_eq!(tools[0].name, "echo");
}
