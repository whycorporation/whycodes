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
        other => serde_json::json!({ "error": format!("unknown method {other}"), "id": id }),
    }
}

async fn streamable_post(State(state): State<MockState>, headers: HeaderMap, body: String) -> Response {
    state.posts.lock().await.push(body.clone());
    let value: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid json").into_response(),
    };
    if value.get("id").is_none() {
        return StatusCode::ACCEPTED.into_response();
    }
    let id = value.get("id").and_then(|i| i.as_u64()).unwrap_or(0);
    let method = value.get("method").and_then(|m| m.as_str()).unwrap_or("").to_string();
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
    let method = value.get("method").and_then(|m| m.as_str()).unwrap_or("").to_string();
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
