//! Streamable HTTP and legacy HTTP+SSE transports for MCP.

use std::collections::HashMap;
use std::time::Duration;

use crate::error::{McpError, Result};
use futures::StreamExt;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::sse::{SseEvent, SseParser, parse_sse_body};
use crate::types::{JsonRpcRequest, JsonRpcResponse};

const ACCEPT_BOTH: &str = "application/json, text/event-stream";
const ACCEPT_SSE: &str = "text/event-stream";
const SESSION_HEADER: &str = "mcp-session-id";

fn sse_endpoint_timeout() -> Duration {
    sse_timeout(
        cfg!(test),
        Duration::from_secs(15),
        Duration::from_millis(80),
    )
}

fn sse_response_timeout() -> Duration {
    sse_timeout(
        cfg!(test),
        Duration::from_secs(60),
        Duration::from_millis(80),
    )
}

fn sse_timeout(for_test: bool, production: Duration, test: Duration) -> Duration {
    if for_test { test } else { production }
}

fn remaining_until(deadline: tokio::time::Instant) -> Option<Duration> {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    if remaining.is_zero() {
        None
    } else {
        Some(remaining)
    }
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .connect_timeout(Duration::from_secs(15))
        .user_agent(format!("whycodes-mcp/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| McpError::msg(format!("failed to build HTTP client: {e}")))
}

fn header_map(extra: &HashMap<String, String>) -> Result<HeaderMap> {
    let mut map = HeaderMap::new();
    for (k, v) in extra {
        let name = HeaderName::from_bytes(k.as_bytes())
            .map_err(|e| McpError::msg(format!("invalid header name: {k}: {e}")))?;
        let value = HeaderValue::from_str(v)
            .map_err(|e| McpError::msg(format!("invalid header value for {k}: {e}")))?;
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
        return Err(McpError::msg(format!(
            "MCP error [{}]: {}",
            error.code, error.message
        )));
    }
    response
        .result
        .ok_or_else(|| McpError::msg("MCP response has no result"))
}

pub fn resolve_endpoint_url(sse_url: &str, endpoint: &str) -> Result<String> {
    let endpoint = endpoint.trim();
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        return Ok(endpoint.to_string());
    }
    let base = reqwest::Url::parse(sse_url)
        .map_err(|e| McpError::msg(format!("invalid SSE URL: {sse_url}: {e}")))?;
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
        .map_err(|e| {
            McpError::msg(format!(
                "failed to join endpoint '{endpoint}' onto {sse_url}: {e}"
            ))
        })?
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
    Err(McpError::msg(format!(
        "no JSON-RPC response found in SSE body for id={expected_id}: {}",
        truncate(body, 300)
    )))
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
            .map_err(|e| McpError::msg(format!("HTTP POST to {} failed: {e}", self.url)))?;
        self.capture_session(&response);

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(McpError::msg(format!(
                "MCP HTTP error {status} for method '{method}': {}",
                truncate(&text, 500)
            )));
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
                .map_err(|e| McpError::msg(format!("failed to read SSE response body: {e}")))?;
            return extract_jsonrpc_result_from_sse(&text, id);
        }

        let response: JsonRpcResponse = response
            .json()
            .await
            .map_err(|e| McpError::msg(format!("failed to parse JSON-RPC response: {e}")))?;
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
        let response = builder.send().await.map_err(|e| {
            McpError::msg(format!(
                "HTTP POST notification to {} failed: {e}",
                self.url
            ))
        })?;
        self.capture_session(&response);
        let status = response.status();
        if status.as_u16() == 202 || status.is_success() {
            return Ok(());
        }
        let text = response.text().await.unwrap_or_default();
        Err(McpError::msg(format!(
            "MCP notification '{method}' failed with {status}: {}",
            truncate(&text, 300)
        )))
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
            .map_err(|e| McpError::msg(format!("SSE GET to {sse_url} failed: {e}")))?;

        if !response.status().is_success() {
            return Err(McpError::msg(format!(
                "SSE connect failed with status {} for {sse_url}",
                response.status()
            )));
        }

        let (tx, mut rx) = mpsc::unbounded_channel::<SseEvent>();
        let reader = spawn_sse_reader(response, tx);

        let post_url = tokio::time::timeout(sse_endpoint_timeout(), async {
            while let Some(ev) = rx.recv().await {
                let name = ev.event.as_deref().unwrap_or("");
                if name == "endpoint" || (name.is_empty() && looks_like_endpoint(&ev.data)) {
                    return resolve_endpoint_url(&sse_url, &ev.data);
                }
                debug!(event = ?ev.event, "SSE event before endpoint (ignored)");
            }
            Err(McpError::msg("SSE stream closed before endpoint event"))
        })
        .await
        .map_err(|e| McpError::msg(format!("timed out waiting for SSE endpoint event: {e}")))??;

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
            .map_err(|e| McpError::msg(format!("HTTP POST to {} failed: {e}", self.post_url)))?;

        let status = response.status();
        if status.as_u16() == 202 || status.as_u16() == 204 {
            return self.wait_for_response(id).await;
        }
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(McpError::msg(format!(
                "MCP SSE POST error {status} for '{method}': {}",
                truncate(&text, 500)
            )));
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
                let rpc: JsonRpcResponse = serde_json::from_slice(&bytes).map_err(|e| {
                    McpError::msg(format!("failed to parse JSON-RPC from POST body: {e}"))
                })?;
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
            .map_err(|e| {
                McpError::msg(format!(
                    "HTTP POST notification to {} failed: {e}",
                    self.post_url
                ))
            })?;
        let status = response.status();
        if status.as_u16() == 202 || status.is_success() {
            return Ok(());
        }
        let text = response.text().await.unwrap_or_default();
        Err(McpError::msg(format!(
            "MCP SSE notification '{method}' failed with {status}: {}",
            truncate(&text, 300)
        )))
    }

    async fn wait_for_response(&mut self, expected_id: u64) -> Result<serde_json::Value> {
        self.wait_for_response_until(
            expected_id,
            tokio::time::Instant::now() + sse_response_timeout(),
        )
        .await
    }

    async fn wait_for_response_until(
        &mut self,
        expected_id: u64,
        deadline: tokio::time::Instant,
    ) -> Result<serde_json::Value> {
        loop {
            let remaining = remaining_until(deadline).ok_or_else(|| {
                McpError::msg(format!(
                    "timed out waiting for MCP SSE response id={expected_id}"
                ))
            })?;
            let ev = tokio::time::timeout(remaining, self.rx.recv())
                .await
                .map_err(|e| McpError::msg(format!("timed out waiting for MCP SSE response: {e}")))?
                .ok_or_else(|| McpError::msg("SSE stream closed while waiting for response"))?;

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
            let rpc: JsonRpcResponse = serde_json::from_value(value).map_err(|e| {
                McpError::msg(format!(
                    "failed to parse JSON-RPC response from SSE message: {e}"
                ))
            })?;
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

    #[cfg(test)]
    fn from_rx_for_test(rx: mpsc::UnboundedReceiver<SseEvent>) -> Self {
        Self {
            client: http_client().expect("http client"),
            headers: HeaderMap::new(),
            post_url: "http://127.0.0.1:1/messages".into(),
            rx,
            _reader: tokio::spawn(async {}),
            next_id: 1,
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
#[path = "http_tests.rs"]
mod tests;
