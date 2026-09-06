use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn normalize_adds_scheme() {
    assert_eq!(normalize_base("127.0.0.1:3030"), "http://127.0.0.1:3030");
    assert_eq!(normalize_base("http://localhost:9/"), "http://localhost:9");
    assert_eq!(
        normalize_base("  https://example.com/  "),
        "https://example.com"
    );
    assert_eq!(normalize_base("localhost"), "http://localhost");
    let remote = RemoteAttach::new("127.0.0.1:9/", "abc");
    assert_eq!(remote.base_url, "http://127.0.0.1:9");
    assert_eq!(remote.session_id, "abc");
}

#[test]
fn sse_text_delta() {
    let v = serde_json::json!({"type":"text_delta","text":"hi"});
    match json_to_turn_event(&v) {
        Some(TurnEvent::TextDelta(t)) => assert_eq!(t, "hi"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn parse_data_frame() {
    let v = parse_sse_data("data: {\"type\":\"done\"}\n").unwrap();
    assert_eq!(v["type"], "done");
}

#[test]
fn parse_sse_skips_ping_empty_and_invalid() {
    assert!(parse_sse_data("data: ping\n").is_none());
    assert!(parse_sse_data("data:\n").is_none());
    assert!(parse_sse_data("data:    \n").is_none());
    assert!(parse_sse_data("data: not-json\n").is_none());
    assert!(parse_sse_data(": comment only\n").is_none());
    assert!(parse_sse_data("event: foo\n").is_none());
    // `strip_prefix("data:")?` exits the function on a non-data first line.
    assert!(parse_sse_data("event: x\ndata: {\"type\":\"status\"}\n").is_none());
    // ping is skipped; the next data line is parsed.
    let v = parse_sse_data("data: ping\ndata: {\"type\":\"done\"}\n").unwrap();
    assert_eq!(v["type"], "done");
    let v = parse_sse_data("data:{\"type\":\"done\"}").unwrap();
    assert_eq!(v["type"], "done");
}

#[test]
fn json_to_turn_event_covers_every_wire_type() {
    match json_to_turn_event(&serde_json::json!({"type":"thinking_delta","text":"hmm"})) {
        Some(TurnEvent::ThinkingDelta(t)) => assert_eq!(t, "hmm"),
        other => panic!("{other:?}"),
    }
    match json_to_turn_event(&serde_json::json!({
        "type":"tool_start","id":"t1","name":"bash"
    })) {
        Some(TurnEvent::ToolStart { id, name, input }) => {
            assert_eq!(id, "t1");
            assert_eq!(name, "bash");
            assert_eq!(input, serde_json::json!({}));
        }
        other => panic!("{other:?}"),
    }
    match json_to_turn_event(&serde_json::json!({
        "type":"tool_start","id":"t2","name":"read","input":{"path":"a.rs"}
    })) {
        Some(TurnEvent::ToolStart { input, .. }) => {
            assert_eq!(input["path"], "a.rs");
        }
        other => panic!("{other:?}"),
    }
    match json_to_turn_event(&serde_json::json!({
        "type":"tool_end","id":"t1","content":"ok","is_error":true
    })) {
        Some(TurnEvent::ToolEnd {
            id,
            content,
            is_error,
        }) => {
            assert_eq!(id, "t1");
            assert_eq!(content, "ok");
            assert!(is_error);
        }
        other => panic!("{other:?}"),
    }
    match json_to_turn_event(&serde_json::json!({"type":"tool_end","id":"t9"})) {
        Some(TurnEvent::ToolEnd {
            content, is_error, ..
        }) => {
            assert_eq!(content, "");
            assert!(!is_error);
        }
        other => panic!("{other:?}"),
    }
    match json_to_turn_event(&serde_json::json!({"type":"status","message":"working"})) {
        Some(TurnEvent::Status(s)) => assert_eq!(s, "working"),
        other => panic!("{other:?}"),
    }
    match json_to_turn_event(&serde_json::json!({"type":"error","message":"boom"})) {
        Some(TurnEvent::Status(s)) => assert_eq!(s, "error:boom"),
        other => panic!("{other:?}"),
    }
    match json_to_turn_event(&serde_json::json!({"type":"error"})) {
        Some(TurnEvent::Status(s)) => assert_eq!(s, "error:remote"),
        other => panic!("{other:?}"),
    }
    assert!(json_to_turn_event(&serde_json::json!({"type":"usage"})).is_none());
    assert!(json_to_turn_event(&serde_json::json!({"type":"done"})).is_none());
    assert!(json_to_turn_event(&serde_json::json!({"type":"nope"})).is_none());
    assert!(json_to_turn_event(&serde_json::json!({})).is_none());
    assert!(json_to_turn_event(&serde_json::json!({"type":"text_delta"})).is_none());
    assert!(json_to_turn_event(&serde_json::json!({"type":"tool_start","id":"x"})).is_none());
    assert!(json_to_turn_event(&serde_json::json!({"type":"status"})).is_none());
}

async fn spawn_router() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 16_384];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let line = req.lines().next().unwrap_or("");
                let (status, ctype, body) = if line.contains("GET /api/health") {
                    (200, "application/json", r#"{"ok":true}"#.to_string())
                } else if line.contains("POST /api/session/new") {
                    (
                        200,
                        "application/json",
                        r#"{"session_id":"sess-1"}"#.to_string(),
                    )
                } else if line.contains("GET /api/sessions") {
                    (
                        200,
                        "application/json",
                        r#"{"sessions":[{"id":"sess-1"}]}"#.to_string(),
                    )
                } else if line.contains("/messages") {
                    (
                        200,
                        "application/json",
                        r#"{"title":"hello","messages":[{"role":"user","content":"hi"}]}"#
                            .to_string(),
                    )
                } else if line.contains("/chat") {
                    let sse = "data: {\"type\":\"text_delta\",\"text\":\"hel\"}\n\n\
                               data: {\"type\":\"thinking_delta\",\"text\":\"hmm\"}\n\n\
                               data: {\"type\":\"tool_start\",\"id\":\"t1\",\"name\":\"bash\"}\n\n\
                               data: {\"type\":\"tool_end\",\"id\":\"t1\",\"content\":\"ok\"}\n\n\
                               data: {\"type\":\"status\",\"message\":\"ok\"}\n\n\
                               data: ping\n\n\
                               data: {\"type\":\"done\"}\n\n";
                    (200, "text/event-stream", sse.to_string())
                } else {
                    (404, "application/json", r#"{"err":"no"}"#.to_string())
                };
                let resp = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    format!("http://{addr}")
}

async fn spawn_status(path_substr: &'static str, status: u16, body: &'static str) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let Ok((mut sock, _)) = listener.accept().await else {
            return;
        };
        let mut buf = vec![0u8; 8192];
        let n = sock.read(&mut buf).await.unwrap_or(0);
        let req = String::from_utf8_lossy(&buf[..n]);
        let line = req.lines().next().unwrap_or("");
        let (code, payload) = if line.contains(path_substr) {
            (status, body)
        } else {
            (404, "{}")
        };
        let reason = if code == 200 { "OK" } else { "ERR" };
        let resp = format!(
            "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            payload.len()
        );
        let _ = sock.write_all(resp.as_bytes()).await;
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn health_create_list_and_fetch_round_trip() {
    let base = spawn_router().await;
    let h = health(&base).await.expect("health");
    assert_eq!(h["ok"], true);

    let id = create_session(&base).await.expect("create");
    assert_eq!(id, "sess-1");

    let latest = latest_session(&base).await.expect("latest");
    assert_eq!(latest.as_deref(), Some("sess-1"));

    let (title, msgs) = fetch_messages(&base, "sess-1").await.expect("fetch");
    assert_eq!(title, "hello");
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].content.as_text(), Some("hi"));
}

#[tokio::test]
async fn stream_chat_concatenates_text_and_forwards_events() {
    let base = spawn_router().await;
    let remote = RemoteAttach::new(&base, "sess-1");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let text = stream_chat(&remote, "hello", tx, None)
        .await
        .expect("stream");
    assert_eq!(text, "hel");
    let mut saw_tool = false;
    while let Ok(ev) = rx.try_recv() {
        if matches!(ev, TurnEvent::ToolStart { .. }) {
            saw_tool = true;
        }
    }
    assert!(saw_tool, "tool_start should be forwarded");
}

#[tokio::test]
async fn stream_chat_stops_when_the_sink_closes() {
    let base = spawn_router().await;
    let remote = RemoteAttach::new(&base, "sess-1");
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    drop(rx);
    let text = stream_chat(&remote, "hello", tx, None)
        .await
        .expect("closed sink is not an error");
    assert_eq!(text, "hel");
}

#[tokio::test]
async fn stream_chat_honours_cancel() {
    let base = spawn_router().await;
    let remote = RemoteAttach::new(&base, "sess-1");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let flag = whycodes_agent::new_cancel_flag();
    whycodes_agent::request_cancel(&flag);
    let err = stream_chat(&remote, "hello", tx, Some(flag))
        .await
        .expect_err("cancelled");
    assert!(err.to_string().contains("cancelled"), "{err}");
}

#[tokio::test]
async fn http_error_paths() {
    let health_url = spawn_status("/api/health", 503, r#"{"err":1}"#).await;
    assert!(health(&health_url).await.is_err());

    let create_url = spawn_status("/api/session/new", 500, "nope").await;
    assert!(create_session(&create_url).await.is_err());

    let missing_id = spawn_status("/api/session/new", 200, r#"{"ok":true}"#).await;
    assert!(create_session(&missing_id).await.is_err());

    let list_err = spawn_status("/api/sessions", 500, "x").await;
    assert!(latest_session(&list_err).await.is_err());

    let list_empty = spawn_status("/api/sessions", 200, r#"{"sessions":[]}"#).await;
    assert_eq!(latest_session(&list_empty).await.unwrap(), None);

    let not_found = spawn_status("/messages", 404, "gone").await;
    let err = fetch_messages(&not_found, "nope").await.unwrap_err();
    assert!(err.to_string().contains("not found"), "{err}");

    let fetch_err = spawn_status("/messages", 500, "x").await;
    assert!(fetch_messages(&fetch_err, "x").await.is_err());

    let no_msgs = spawn_status("/messages", 200, r#"{"title":"t"}"#).await;
    let (title, msgs) = fetch_messages(&no_msgs, "x").await.unwrap();
    assert_eq!(title, "t");
    assert!(msgs.is_empty());

    let chat_err = spawn_status("/chat", 500, "no").await;
    let remote = RemoteAttach::new(&chat_err, "s");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    assert!(stream_chat(&remote, "x", tx, None).await.is_err());
}
