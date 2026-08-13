//! HTTP client for `whycode serve` (TUI attach).

use std::time::Duration;

use futures::StreamExt;
use whycode_agent::TurnEvent;
use whycode_core::types::Message;

/// How the TUI talks to a warm `whycode serve` process.
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
    cancel: Option<whycode_agent::CancelFlag>,
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
        if whycode_agent::is_cancelled(&cancel) {
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

    #[test]
    fn normalize_adds_scheme() {
        assert_eq!(normalize_base("127.0.0.1:3030"), "http://127.0.0.1:3030");
        assert_eq!(normalize_base("http://localhost:9/"), "http://localhost:9");
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
}
