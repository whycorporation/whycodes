//! Streamable HTTP and legacy HTTP+SSE transports for MCP.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures::StreamExt;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::sse::{SseEvent, SseParser, parse_sse_body};
use crate::types::{JsonRpcRequest, JsonRpcResponse};

const ACCEPT_BOTH: &str = "application/json, text/event-stream";
const ACCEPT_SSE: &str = "text/event-stream";
const SESSION_HEADER: &str = "mcp-session-id";

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .connect_timeout(Duration::from_secs(15))
        .user_agent(format!("whycode-mcp/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("failed to build HTTP client")
}

fn header_map(extra: &HashMap<String, String>) -> Result<HeaderMap> {
    let mut map = HeaderMap::new();
    for (k, v) in extra {
        let name = HeaderName::from_bytes(k.as_bytes())
            .with_context(|| format!("invalid header name: {k}"))?;
        let value =
            HeaderValue::from_str(v).with_context(|| format!("invalid header value for {k}"))?;
        map.insert(name, value);
    }
    Ok(map)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        // Byte cap: never slice mid-codepoint (`ö`, CJK, emoji).
        format!("{}…", &s[..s.floor_char_boundary(max)])
    }
}

fn unwrap_rpc(response: JsonRpcResponse, expected_id: u64) -> Result<serde_json::Value> {
    if response.id != expected_id {
        warn!(
            expected = expected_id,
            got = response.id,
            "MCP response id mismatch"
        );
    }
    if let Some(error) = response.error {
        bail!("MCP error [{}]: {}", error.code, error.message);
    }
    response.result.context("MCP response has no result")
}

pub fn resolve_endpoint_url(sse_url: &str, endpoint: &str) -> Result<String> {
    let endpoint = endpoint.trim();
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        return Ok(endpoint.to_string());
    }
    let base =
        reqwest::Url::parse(sse_url).with_context(|| format!("invalid SSE URL: {sse_url}"))?;
    if endpoint.starts_with('/') {
        let mut abs = base;
        abs.set_path(endpoint.split('?').next().unwrap_or(endpoint));
        if let Some(q) = endpoint.split_once('?').map(|(_, q)| q) {
            abs.set_query(Some(q));
        } else {
            abs.set_query(None);
        }
        return Ok(abs.to_string());
    }
    Ok(base
        .join(endpoint)
        .with_context(|| format!("failed to join endpoint '{endpoint}' onto {sse_url}"))?
        .to_string())
}

fn extract_jsonrpc_result_from_sse(body: &str, expected_id: u64) -> Result<serde_json::Value> {
    let events = parse_sse_body(body);
    if events.is_empty()
        && !body.trim().is_empty()
        && let Ok(rpc) = serde_json::from_str::<JsonRpcResponse>(body.trim())
    {
        return unwrap_rpc(rpc, expected_id);
    }
    for ev in &events {
        if ev.data.trim().is_empty() {
            continue;
        }
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&ev.data)
            && val.get("method").is_some()
            && val.get("result").is_none()
            && val.get("error").is_none()
        {
            continue;
        }
        if let Ok(rpc) = serde_json::from_str::<JsonRpcResponse>(&ev.data)
            && rpc.id == expected_id
        {
            return unwrap_rpc(rpc, expected_id);
        }
    }
    for ev in &events {
        if let Ok(rpc) = serde_json::from_str::<JsonRpcResponse>(&ev.data) {
            return unwrap_rpc(rpc, expected_id);
        }
    }
    bail!(
        "no JSON-RPC response found in SSE body for id={expected_id}: {}",
        truncate(body, 300)
    )
}

// ── Streamable HTTP ─────────────────────────────────────────────────────────

pub struct StreamableHttpTransport {
    client: reqwest::Client,
    url: String,
    headers: HeaderMap,
    session_id: Option<String>,
    next_id: u64,
}

impl StreamableHttpTransport {
    pub fn new(url: impl Into<String>, headers: &HashMap<String, String>) -> Result<Self> {
        Ok(Self {
            client: http_client()?,
            url: url.into(),
            headers: header_map(headers)?,
            session_id: None,
            next_id: 1,
        })
    }

    fn apply_common_headers(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let mut b = builder.headers(self.headers.clone());
        if let Some(sid) = &self.session_id {
            b = b.header(SESSION_HEADER, sid);
        }
        b
    }

    fn capture_session(&mut self, response: &reqwest::Response) {
        if let Some(val) = response.headers().get(SESSION_HEADER)
            && let Ok(s) = val.to_str()
            && !s.is_empty()
        {
            self.session_id = Some(s.to_string());
            debug!(session_id = %s, "MCP session id captured");
        }
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub async fn send_request(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let id = self.next_id;
        self.next_id += 1;
        let request = JsonRpcRequest::new(id, method, params);
        let body = serde_json::to_vec(&request)?;
        debug!(%method, id, url = %self.url, "MCP streamable HTTP request");

        let builder = self
            .client
            .post(&self.url)
            .header(ACCEPT, ACCEPT_BOTH)
            .header(CONTENT_TYPE, "application/json")
            .body(body);
        let builder = self.apply_common_headers(builder);
        let response = builder
            .send()
            .await
            .with_context(|| format!("HTTP POST to {} failed", self.url))?;
        self.capture_session(&response);

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            bail!(
                "MCP HTTP error {status} for method '{method}': {}",
                truncate(&text, 500)
            );
        }

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();

        if content_type.contains("text/event-stream") {
            let text = response
                .text()
                .await
                .context("failed to read SSE response body")?;
            return extract_jsonrpc_result_from_sse(&text, id);
        }

        let response: JsonRpcResponse = response
            .json()
            .await
            .context("failed to parse JSON-RPC response")?;
        unwrap_rpc(response, id)
    }

    pub async fn send_notification(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<()> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        debug!(%method, url = %self.url, "MCP streamable HTTP notification");
        let builder = self
            .client
            .post(&self.url)
            .header(ACCEPT, ACCEPT_BOTH)
            .header(CONTENT_TYPE, "application/json")
            .json(&body);
        let builder = self.apply_common_headers(builder);
        let response = builder
            .send()
            .await
            .with_context(|| format!("HTTP POST notification to {} failed", self.url))?;
        self.capture_session(&response);
        let status = response.status();
        if status.as_u16() == 202 || status.is_success() {
            return Ok(());
        }
        let text = response.text().await.unwrap_or_default();
        bail!(
            "MCP notification '{method}' failed with {status}: {}",
            truncate(&text, 300)
        )
    }
}

// ── Legacy HTTP+SSE ─────────────────────────────────────────────────────────

pub struct LegacySseTransport {
    client: reqwest::Client,
    headers: HeaderMap,
    post_url: String,
    rx: mpsc::UnboundedReceiver<SseEvent>,
    _reader: tokio::task::JoinHandle<()>,
    next_id: u64,
}

impl LegacySseTransport {
    pub async fn connect(
        sse_url: impl Into<String>,
        headers: &HashMap<String, String>,
    ) -> Result<Self> {
        let sse_url = sse_url.into();
        let client = http_client()?;
        let headers = header_map(headers)?;

        let response = client
            .get(&sse_url)
            .header(ACCEPT, ACCEPT_SSE)
            .headers(headers.clone())
            .send()
            .await
            .with_context(|| format!("SSE GET to {sse_url} failed"))?;

        if !response.status().is_success() {
            bail!(
                "SSE connect failed with status {} for {sse_url}",
                response.status()
            );
        }

        let (tx, mut rx) = mpsc::unbounded_channel::<SseEvent>();
        let reader = spawn_sse_reader(response, tx);

        let post_url = tokio::time::timeout(Duration::from_secs(15), async {
            while let Some(ev) = rx.recv().await {
                let name = ev.event.as_deref().unwrap_or("");
                if name == "endpoint" || (name.is_empty() && looks_like_endpoint(&ev.data)) {
                    return resolve_endpoint_url(&sse_url, &ev.data);
                }
                debug!(event = ?ev.event, "SSE event before endpoint (ignored)");
            }
            bail!("SSE stream closed before endpoint event")
        })
        .await
        .context("timed out waiting for SSE endpoint event")??;

        debug!(%post_url, "MCP legacy SSE endpoint resolved");
        Ok(Self {
            client,
            headers,
            post_url,
            rx,
            _reader: reader,
            next_id: 1,
        })
    }

    pub async fn send_request(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let id = self.next_id;
        self.next_id += 1;
        let request = JsonRpcRequest::new(id, method, params);
        let body = serde_json::to_vec(&request)?;
        debug!(%method, id, url = %self.post_url, "MCP legacy SSE request");

        let response = self
            .client
            .post(&self.post_url)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, ACCEPT_BOTH)
            .headers(self.headers.clone())
            .body(body)
            .send()
            .await
            .with_context(|| format!("HTTP POST to {} failed", self.post_url))?;

        let status = response.status();
        if status.as_u16() == 202 || status.as_u16() == 204 {
            return self.wait_for_response(id).await;
        }
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            bail!(
                "MCP SSE POST error {status} for '{method}': {}",
                truncate(&text, 500)
            );
        }

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();

        if content_type.contains("application/json") {
            let bytes = response.bytes().await?;
            if !bytes.is_empty() {
                let rpc: JsonRpcResponse = serde_json::from_slice(&bytes)
                    .context("failed to parse JSON-RPC from POST body")?;
                return unwrap_rpc(rpc, id);
            }
            return self.wait_for_response(id).await;
        }
        drop(response);
        self.wait_for_response(id).await
    }

    pub async fn send_notification(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<()> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let response = self
            .client
            .post(&self.post_url)
            .header(CONTENT_TYPE, "application/json")
            .headers(self.headers.clone())
            .json(&body)
            .send()
            .await
            .with_context(|| format!("HTTP POST notification to {} failed", self.post_url))?;
        let status = response.status();
        if status.as_u16() == 202 || status.is_success() {
            return Ok(());
        }
        let text = response.text().await.unwrap_or_default();
        bail!(
            "MCP SSE notification '{method}' failed with {status}: {}",
            truncate(&text, 300)
        )
    }

    async fn wait_for_response(&mut self, expected_id: u64) -> Result<serde_json::Value> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                bail!("timed out waiting for MCP SSE response id={expected_id}");
            }
            let ev = tokio::time::timeout(remaining, self.rx.recv())
                .await
                .context("timed out waiting for MCP SSE response")?
                .context("SSE stream closed while waiting for response")?;

            let name = ev.event.as_deref().unwrap_or("message");
            if name != "message" && !name.is_empty() {
                debug!(event = %name, "ignoring non-message SSE event");
                continue;
            }
            if ev.data.trim().is_empty() {
                continue;
            }
            let value: serde_json::Value = match serde_json::from_str(&ev.data) {
                Ok(v) => v,
                Err(e) => {
                    warn!(error = %e, data = %truncate(&ev.data, 200), "unparseable SSE message");
                    continue;
                }
            };
            if value.get("method").is_some() && value.get("result").is_none() {
                debug!(method = ?value.get("method"), "server notification on SSE (ignored)");
                continue;
            }
            let rpc: JsonRpcResponse = serde_json::from_value(value)
                .context("failed to parse JSON-RPC response from SSE message")?;
            if rpc.id != expected_id {
                warn!(
                    expected = expected_id,
                    got = rpc.id,
                    "SSE response id mismatch"
                );
                continue;
            }
            return unwrap_rpc(rpc, expected_id);
        }
    }
}

fn spawn_sse_reader(
    response: reqwest::Response,
    tx: mpsc::UnboundedSender<SseEvent>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut stream = response.bytes_stream();
        let mut parser = SseParser::new();
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    parser.push(&text);
                    for ev in parser.take_events() {
                        if tx.send(ev).is_err() {
                            return;
                        }
                    }
                }
                Err(e) => {
                    debug!(error = %e, "SSE stream read error");
                    break;
                }
            }
        }
    })
}

fn looks_like_endpoint(data: &str) -> bool {
    let t = data.trim();
    t.starts_with('/') || t.starts_with("http://") || t.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::JsonRpcError;

    fn rpc_response(
        id: u64,
        result: Option<serde_json::Value>,
        error: Option<JsonRpcError>,
    ) -> JsonRpcResponse {
        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result,
            error,
        }
    }

    #[test]
    fn header_map_rejects_invalid_names_and_values() {
        let invalid_name = HashMap::from([("bad header".to_string(), "value".to_string())]);
        let error = header_map(&invalid_name).unwrap_err().to_string();
        assert!(error.contains("invalid header name: bad header"));

        let invalid_value = HashMap::from([("x-test".to_string(), "bad\nvalue".to_string())]);
        let error = header_map(&invalid_value).unwrap_err().to_string();
        assert!(error.contains("invalid header value for x-test"));
    }

    #[test]
    fn truncate_preserves_short_text_and_limits_long_text() {
        assert_eq!(truncate("short", 5), "short");
        assert_eq!(truncate("abcdef", 3), "abc…");
    }

    #[test]
    fn truncate_backs_up_from_mid_codepoint() {
        // 2 ASCII + `ö` (bytes 2..4) — `&s[..3]` panics.
        let s = format!("abö{}", "c".repeat(10));
        assert!(!s.is_char_boundary(3));
        let out = truncate(&s, 3);
        assert_eq!(out, "ab…");
        assert!(out.is_char_boundary(out.len()));
    }

    #[test]
    fn unwrap_rpc_returns_results_and_protocol_errors() {
        let result = unwrap_rpc(
            rpc_response(9, Some(serde_json::json!({"ok": true})), None),
            8,
        )
        .unwrap();
        assert_eq!(result, serde_json::json!({"ok": true}));

        let error = unwrap_rpc(
            rpc_response(
                1,
                None,
                Some(JsonRpcError {
                    code: -32601,
                    message: "unknown method".to_string(),
                    data: None,
                }),
            ),
            1,
        )
        .unwrap_err()
        .to_string();
        assert_eq!(error, "MCP error [-32601]: unknown method");

        let error = unwrap_rpc(rpc_response(1, None, None), 1)
            .unwrap_err()
            .to_string();
        assert_eq!(error, "MCP response has no result");
    }

    #[test]
    fn resolve_absolute_endpoint() {
        let u = resolve_endpoint_url(
            "http://localhost:3000/sse",
            "http://localhost:3000/messages?s=1",
        )
        .unwrap();
        assert_eq!(u, "http://localhost:3000/messages?s=1");
    }

    #[test]
    fn resolve_relative_path_endpoint() {
        let u =
            resolve_endpoint_url("http://localhost:3000/sse", "/messages?sessionId=abc").unwrap();
        assert_eq!(u, "http://localhost:3000/messages?sessionId=abc");
    }

    #[test]
    fn resolve_endpoint_handles_relative_urls_and_clears_base_query() {
        let relative = resolve_endpoint_url(
            "http://localhost:3000/api/sse?old=1",
            "messages?sessionId=abc",
        )
        .unwrap();
        assert_eq!(relative, "http://localhost:3000/api/messages?sessionId=abc");

        let rooted =
            resolve_endpoint_url("http://localhost:3000/sse?old=1", " /messages ").unwrap();
        assert_eq!(rooted, "http://localhost:3000/messages");

        let error = resolve_endpoint_url("not a URL", "/messages")
            .unwrap_err()
            .to_string();
        assert!(error.contains("invalid SSE URL: not a URL"));
    }

    #[test]
    fn extract_result_from_sse_message() {
        let body = concat!(
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n",
            "\n"
        );
        let result = extract_jsonrpc_result_from_sse(body, 1).unwrap();
        assert_eq!(result["ok"], true);
    }

    #[test]
    fn extract_result_skips_notification() {
        let body = concat!(
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n",
            "\n",
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[]}}\n",
            "\n"
        );
        let result = extract_jsonrpc_result_from_sse(body, 2).unwrap();
        assert!(result.get("tools").is_some());
    }

    #[test]
    fn extract_result_accepts_plain_json_and_reports_rpc_error() {
        let result = extract_jsonrpc_result_from_sse(
            r#"{"jsonrpc":"2.0","id":3,"result":{"pong":true}}"#,
            3,
        )
        .unwrap();
        assert_eq!(result, serde_json::json!({"pong": true}));

        let body = concat!(
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":4,\"error\":{\"code\":-32000,\"message\":\"denied\"}}\n",
            "\n"
        );
        let error = extract_jsonrpc_result_from_sse(body, 4)
            .unwrap_err()
            .to_string();
        assert_eq!(error, "MCP error [-32000]: denied");
    }

    #[test]
    fn extract_result_falls_back_to_a_mismatched_response_and_describes_absence() {
        let mismatched = concat!(
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"value\":1}}\n",
            "\n"
        );
        let result = extract_jsonrpc_result_from_sse(mismatched, 8).unwrap();
        assert_eq!(result, serde_json::json!({"value": 1}));

        let error = extract_jsonrpc_result_from_sse("event: ping\ndata: nope\n\n", 8)
            .unwrap_err()
            .to_string();
        assert!(error.contains("no JSON-RPC response found in SSE body for id=8"));
        assert!(error.contains("event: ping"));
    }

    #[test]
    fn endpoint_detection_accepts_only_supported_url_forms() {
        assert!(looks_like_endpoint(" /messages "));
        assert!(looks_like_endpoint("https://example.test/messages"));
        assert!(looks_like_endpoint("http://example.test/messages"));
        assert!(!looks_like_endpoint("messages"));
        assert!(!looks_like_endpoint(""));
    }
}
