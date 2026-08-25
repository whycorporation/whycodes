//! HTTP client for `whycodes serve` (TUI attach).

use std::time::Duration;

use futures::StreamExt;
use whycodes_agent::TurnEvent;
use whycodes_core::types::Message;

/// How the TUI talks to a warm `whycodes serve` process.
#[derive(Debug, Clone)]
pub struct RemoteAttach {
    pub base_url: String,
    pub session_id: String,
}

impl RemoteAttach {
    pub fn new(base_url: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            base_url: normalize_base(&base_url.into()),
            session_id: session_id.into(),
        }
    }
}

/// Prepend `http://` when the user typed `host:port`.
pub fn normalize_base(addr: &str) -> String {
    let t = addr.trim().trim_end_matches('/');
    if t.starts_with("http://") || t.starts_with("https://") {
        t.to_string()
    } else {
        format!("http://{t}")
    }
}

fn client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(600))
        .build()
}

/// `GET /api/health`. Errors if the daemon is down.
pub async fn health(base: &str) -> anyhow::Result<serde_json::Value> {
    let url = format!("{}/api/health", normalize_base(base));
    let res = client()?.get(&url).send().await?;
    if !res.status().is_success() {
        anyhow::bail!("serve health {}: {}", res.status(), url);
    }
    Ok(res.json().await?)
}

/// `POST /api/session/new` → session id.
pub async fn create_session(base: &str) -> anyhow::Result<String> {
    let url = format!("{}/api/session/new", normalize_base(base));
    let res = client()?
        .post(&url)
        .json(&serde_json::json!({}))
        .send()
        .await?;
    if !res.status().is_success() {
        anyhow::bail!("create session failed: {}", res.status());
    }
    let v: serde_json::Value = res.json().await?;
    v.get("session_id")
        .and_then(|s| s.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("create session: missing session_id"))
}

/// Latest warm or db session id, if any.
pub async fn latest_session(base: &str) -> anyhow::Result<Option<String>> {
    let url = format!("{}/api/sessions", normalize_base(base));
    let res = client()?.get(&url).send().await?;
    if !res.status().is_success() {
        anyhow::bail!("list sessions failed: {}", res.status());
    }
    let v: serde_json::Value = res.json().await?;
    let id = v
        .get("sessions")
        .and_then(|s| s.as_array())
        .and_then(|arr| arr.first())
        .and_then(|s| s.get("id"))
        .and_then(|s| s.as_str())
        .map(str::to_string);
    Ok(id)
}

/// Hydrate native messages for attach.
pub async fn fetch_messages(
    base: &str,
    session_id: &str,
) -> anyhow::Result<(String, Vec<Message>)> {
    let url = format!("{}/api/session/{session_id}/messages", normalize_base(base));
    let res = client()?.get(&url).send().await?;
    if res.status() == reqwest::StatusCode::NOT_FOUND {
        anyhow::bail!("session {session_id} not found on serve");
    }
    if !res.status().is_success() {
        anyhow::bail!("fetch messages failed: {}", res.status());
    }
    let v: serde_json::Value = res.json().await?;
    let title = v
        .get("title")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    let msgs = v
        .get("messages")
        .cloned()
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();
    Ok((title, msgs))
}

/// Stream a chat turn into `event_tx`. Returns the concatenated assistant text.
pub async fn stream_chat(
    remote: &RemoteAttach,
    message: &str,
    event_tx: tokio::sync::mpsc::UnboundedSender<TurnEvent>,
    cancel: Option<whycodes_agent::CancelFlag>,
) -> anyhow::Result<String> {
    let url = format!("{}/api/session/{}/chat", remote.base_url, remote.session_id);
    let res = client()?
        .post(&url)
        .json(&serde_json::json!({ "message": message }))
        .send()
        .await?;
    if !res.status().is_success() {
        anyhow::bail!("chat {}: {}", res.status(), url);
    }

    let mut stream = res.bytes_stream();
    let mut buf = String::new();
    let mut text = String::new();
    while let Some(chunk) = stream.next().await {
        if whycodes_agent::is_cancelled(&cancel) {
            anyhow::bail!("cancelled");
        }
        let chunk = chunk?;
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(idx) = buf.find("\n\n") {
            let frame = buf[..idx].to_string();
            buf = buf[idx + 2..].to_string();
            if let Some(ev) = parse_sse_data(&frame) {
                if ev.get("type").and_then(|t| t.as_str()) == Some("done") {
                    return Ok(text);
                }
                if let Some(te) = json_to_turn_event(&ev) {
                    if let TurnEvent::TextDelta(ref t) = te {
                        text.push_str(t);
                    }
                    if event_tx.send(te).is_err() {
                        return Ok(text);
                    }
                }
            }
        }
    }
    Ok(text)
}

fn parse_sse_data(frame: &str) -> Option<serde_json::Value> {
    for line in frame.lines() {
        let line = line.trim();
        let data = line.strip_prefix("data:")?.trim();
        if data.is_empty() || data == "ping" {
            continue;
        }
        return match serde_json::from_str(data) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::debug!(error = %e, "sse data is not JSON");
                None
            }
        };
    }
    None
}

pub fn json_to_turn_event(v: &serde_json::Value) -> Option<TurnEvent> {
    let ty = v.get("type")?.as_str()?;
    Some(match ty {
        "text_delta" => TurnEvent::TextDelta(v.get("text")?.as_str()?.to_string()),
        "thinking_delta" => TurnEvent::ThinkingDelta(v.get("text")?.as_str()?.to_string()),
        "tool_start" => TurnEvent::ToolStart {
            id: v.get("id")?.as_str()?.to_string(),
            name: v.get("name")?.as_str()?.to_string(),
            input: v.get("input").cloned().unwrap_or(serde_json::json!({})),
        },
        "tool_end" => TurnEvent::ToolEnd {
            id: v.get("id")?.as_str()?.to_string(),
            content: v
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .into(),
            is_error: v.get("is_error").and_then(|b| b.as_bool()).unwrap_or(false),
        },
        "status" => TurnEvent::Status(v.get("message")?.as_str()?.to_string()),
        "usage" => return None,
        "error" => TurnEvent::Status(format!(
            "error:{}",
            v.get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("remote")
        )),
        "done" => return None,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
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
}
