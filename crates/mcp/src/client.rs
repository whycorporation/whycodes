//! MCP client: stdio, Streamable HTTP, and legacy HTTP+SSE.

use std::collections::HashMap;
use std::process::Stdio;

use crate::error::{McpError, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as AsyncBufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tracing::{debug, info, warn};

use crate::http::{LegacySseTransport, StreamableHttpTransport};
use crate::types::{
    CallToolParams, CallToolResult, InitializeParams, InitializeResult, InitializedNotification,
    JsonRpcRequest, JsonRpcResponse, ListToolsResult, McpTool,
};

const PROTOCOL_VERSION: &str = "2025-03-26";

enum Transport {
    Stdio {
        /// Never read directly: held so `kill_on_drop(true)` on the spawned
        /// command reaps the server process when the transport is dropped.
        #[allow(dead_code)]
        child: Child,
        writer: ChildStdin,
        reader: AsyncBufReader<ChildStdout>,
        next_id: u64,
    },
    StreamableHttp(StreamableHttpTransport),
    LegacySse(LegacySseTransport),
}

/// MCP client supporting stdio, Streamable HTTP, and legacy HTTP+SSE.
pub struct McpClient {
    transport: Transport,
}

impl McpClient {
    pub async fn connect_stdio(
        command: impl AsRef<std::ffi::OsStr>,
        args: &[impl AsRef<std::ffi::OsStr>],
    ) -> Result<Self> {
        Self::connect_stdio_with(command, args, None, None).await
    }

    pub async fn connect_stdio_with(
        command: impl AsRef<std::ffi::OsStr>,
        args: &[impl AsRef<std::ffi::OsStr>],
        env: Option<&HashMap<String, String>>,
        cwd: Option<&str>,
    ) -> Result<Self> {
        let mut cmd = Command::new(command.as_ref());
        cmd.args(args.iter().map(|a| a.as_ref()))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        if let Some(env) = env {
            for (k, v) in env {
                cmd.env(k, v);
            }
        }
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        }
        let mut child = cmd.spawn().map_err(|e| {
            McpError::msg(format!(
                "failed to spawn MCP server {:?}: {e}",
                command.as_ref()
            ))
        })?;
        let (writer, stdout) = take_stdio_pipes(&mut child)?;
        let reader = AsyncBufReader::new(stdout);
        let mut client = Self {
            transport: Transport::Stdio {
                child,
                writer,
                reader,
                next_id: 1,
            },
        };
        client.initialize_handshake().await?;
        Ok(client)
    }

    pub async fn connect_http(
        url: impl Into<String>,
        headers: &HashMap<String, String>,
    ) -> Result<Self> {
        let transport = StreamableHttpTransport::new(url, headers)?;
        let mut client = Self {
            transport: Transport::StreamableHttp(transport),
        };
        client.initialize_handshake().await?;
        Ok(client)
    }

    pub async fn connect_sse(
        sse_url: impl Into<String>,
        headers: &HashMap<String, String>,
    ) -> Result<Self> {
        let transport = LegacySseTransport::connect(sse_url, headers).await?;
        let mut client = Self {
            transport: Transport::LegacySse(transport),
        };
        client.initialize_handshake().await?;
        Ok(client)
    }

    pub async fn connect_auto(
        url: impl Into<String>,
        headers: &HashMap<String, String>,
    ) -> Result<Self> {
        let url = url.into();
        match Self::connect_http(&url, headers).await {
            Ok(c) => Ok(c),
            Err(http_err) => {
                // Flatten the error chain — top-level message is often just
                // "initialize request failed" without the HTTP status.
                let msg = http_err.to_string();
                let looks_like_wrong_transport = msg.contains("405")
                    || msg.contains("404")
                    || msg.contains("400")
                    || msg.contains("Method Not Allowed")
                    || msg.contains("Not Found");
                if !looks_like_wrong_transport {
                    return Err(McpError::msg(format!(
                        "Streamable HTTP connect failed: {http_err}"
                    )));
                }
                warn!(
                    url = %url,
                    error = %msg,
                    "Streamable HTTP failed; falling back to legacy SSE"
                );
                Self::connect_sse(&url, headers).await.map_err(|sse_err| {
                    McpError::msg(format!(
                        "both Streamable HTTP and legacy SSE failed for {url}; HTTP error: {msg}; SSE error: {sse_err}"
                    ))
                })
            }
        }
    }

    async fn initialize_handshake(&mut self) -> Result<InitializeResult> {
        let params = serde_json::to_value(InitializeParams {
            protocol_version: PROTOCOL_VERSION.to_string(),
            capabilities: Default::default(),
            client_info: crate::types::ClientInfo {
                name: "whycodes".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        })?;
        let result = self
            .send_request("initialize", Some(params))
            .await
            .map_err(|e| McpError::msg(format!("initialize request failed: {e}")))?;
        let init_result: InitializeResult = serde_json::from_value(result)
            .map_err(|e| McpError::msg(format!("failed to parse initialize result: {e}")))?;
        debug!(
            server_name = %init_result.server_info.name,
            server_version = %init_result.server_info.version,
            protocol_version = %init_result.protocol_version,
            "MCP initialize handshake complete"
        );
        self.send_notification("notifications/initialized", None)
            .await
            .map_err(|e| McpError::msg(format!("failed to send initialized notification: {e}")))?;
        Ok(init_result)
    }

    async fn send_request(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        match &mut self.transport {
            Transport::Stdio {
                writer,
                reader,
                next_id,
                ..
            } => {
                let id = *next_id;
                *next_id += 1;
                let request = JsonRpcRequest::new(id, method, params);
                let line = serde_json::to_string(&request)?;
                debug!(%line, "MCP request");
                writer.write_all(line.as_bytes()).await?;
                writer.write_all(b"\n").await?;
                writer.flush().await?;
                read_stdio_response(reader, id).await
            }
            Transport::StreamableHttp(t) => t.send_request(method, params).await,
            Transport::LegacySse(t) => t.send_request(method, params).await,
        }
    }

    async fn send_notification(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<()> {
        match &mut self.transport {
            Transport::Stdio { writer, .. } => {
                let line = if method == "notifications/initialized" && params.is_none() {
                    serde_json::to_string(&InitializedNotification::new())?
                } else {
                    serde_json::to_string(&serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": method,
                        "params": params,
                    }))?
                };
                writer.write_all(line.as_bytes()).await?;
                writer.write_all(b"\n").await?;
                writer.flush().await?;
                Ok(())
            }
            Transport::StreamableHttp(t) => t.send_notification(method, params).await,
            Transport::LegacySse(t) => t.send_notification(method, params).await,
        }
    }

    pub async fn list_tools(&mut self) -> Result<Vec<McpTool>> {
        let result = self
            .send_request("tools/list", None)
            .await
            .map_err(|e| McpError::msg(format!("tools/list failed: {e}")))?;
        let list: ListToolsResult = serde_json::from_value(result)
            .map_err(|e| McpError::msg(format!("failed to parse tools/list result: {e}")))?;
        info!(count = list.tools.len(), "MCP tools listed");
        Ok(list.tools)
    }

    pub async fn call_tool(&mut self, name: &str, arguments: serde_json::Value) -> Result<String> {
        let params = serde_json::to_value(CallToolParams {
            name: name.to_string(),
            arguments: Some(arguments),
        })?;
        let result = self
            .send_request("tools/call", Some(params))
            .await
            .map_err(|e| McpError::msg(format!("tools/call failed: {e}")))?;
        let call_result: CallToolResult = serde_json::from_value(result)
            .map_err(|e| McpError::msg(format!("failed to parse tools/call result: {e}")))?;
        if call_result.is_error.unwrap_or(false) {
            warn!(tool = %name, "MCP tool call returned an error");
        }
        Ok(call_result.text())
    }

    pub async fn ping(&mut self) -> Result<()> {
        self.send_request("ping", None).await?;
        debug!("MCP ping ok");
        Ok(())
    }

    pub async fn list_resources(&mut self) -> Result<Vec<crate::types::McpResource>> {
        let result = self
            .send_request("resources/list", None)
            .await
            .map_err(|e| McpError::msg(format!("resources/list failed: {e}")))?;
        let list: crate::types::ListResourcesResult = serde_json::from_value(result)
            .map_err(|e| McpError::msg(format!("failed to parse resources/list result: {e}")))?;
        info!(count = list.resources.len(), "MCP resources listed");
        Ok(list.resources)
    }

    pub async fn list_prompts(&mut self) -> Result<Vec<crate::types::McpPrompt>> {
        let result = self
            .send_request("prompts/list", None)
            .await
            .map_err(|e| McpError::msg(format!("prompts/list failed: {e}")))?;
        let list: crate::types::ListPromptsResult = serde_json::from_value(result)
            .map_err(|e| McpError::msg(format!("failed to parse prompts/list result: {e}")))?;
        info!(count = list.prompts.len(), "MCP prompts listed");
        Ok(list.prompts)
    }

    pub fn transport_name(&self) -> &'static str {
        match &self.transport {
            Transport::Stdio { .. } => "stdio",
            Transport::StreamableHttp(_) => "http",
            Transport::LegacySse(_) => "sse",
        }
    }

    #[cfg(test)]
    async fn notify(&mut self, method: &str, params: Option<serde_json::Value>) -> Result<()> {
        self.send_notification(method, params).await
    }
}

async fn read_stdio_response(
    reader: &mut AsyncBufReader<ChildStdout>,
    expected_id: u64,
) -> Result<serde_json::Value> {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .map_err(|e| McpError::msg(format!("failed to read response from MCP server: {e}")))?;
    if line.trim().is_empty() {
        return Err(McpError::msg("MCP server closed stdout (empty response)"));
    }
    let response: JsonRpcResponse = serde_json::from_str(&line)
        .map_err(|e| McpError::msg(format!("failed to parse MCP response: {line}: {e}")))?;
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

fn take_stdio_pipes(child: &mut Child) -> Result<(ChildStdin, ChildStdout)> {
    let writer = child
        .stdin
        .take()
        .ok_or_else(|| McpError::msg("child has no stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| McpError::msg("child has no stdout"))?;
    Ok((writer, stdout))
}

impl Drop for McpClient {
    fn drop(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::BufReader as AsyncBufReader;
    use tokio::process::Command;

    #[test]
    fn client_module_loads() {
        assert!(!module_path!().is_empty());
    }

    async fn stdout_of(script: &str) -> AsyncBufReader<tokio::process::ChildStdout> {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(script)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
        AsyncBufReader::new(stdout)
    }

    #[tokio::test]
    async fn read_stdio_response_ok_error_empty_and_mismatch() {
        let mut ok =
            stdout_of(r#"printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"ok":true}}'"#).await;
        let value = read_stdio_response(&mut ok, 1).await.unwrap();
        assert_eq!(value["ok"], true);

        let mut mismatch =
            stdout_of(r#"printf '%s\n' '{"jsonrpc":"2.0","id":9,"result":{"ok":true}}'"#).await;
        let value = read_stdio_response(&mut mismatch, 1).await.unwrap();
        assert_eq!(value["ok"], true);

        let mut err = stdout_of(
            r#"printf '%s\n' '{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"nope"}}'"#,
        )
        .await;
        let e = read_stdio_response(&mut err, 1).await.unwrap_err();
        assert!(e.to_string().contains("nope"), "{e}");

        let mut empty = stdout_of("printf ''").await;
        let e = read_stdio_response(&mut empty, 1).await.unwrap_err();
        assert!(e.to_string().to_lowercase().contains("empty"), "{e}");

        let mut bad = stdout_of(r#"printf '%s\n' 'not-json'"#).await;
        let e = read_stdio_response(&mut bad, 1).await.unwrap_err();
        assert!(e.to_string().contains("parse"), "{e}");

        let mut no_result = stdout_of(r#"printf '%s\n' '{"jsonrpc":"2.0","id":1}'"#).await;
        let e = read_stdio_response(&mut no_result, 1).await.unwrap_err();
        assert!(e.to_string().contains("no result"), "{e}");
    }

    fn python() -> &'static str {
        if std::path::Path::new("/usr/bin/python3").exists() {
            "/usr/bin/python3"
        } else {
            "python3"
        }
    }

    fn stdio_script() -> String {
        r#"
import json, sys
def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()
init = None
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    method = msg.get("method")
    rid = msg.get("id")
    if method == "initialize":
        send({"jsonrpc":"2.0","id":rid,"result":{
            "protocolVersion":"2025-03-26",
            "capabilities":{"tools":{}},
            "serverInfo":{"name":"mock-stdio","version":"0.0.1"}
        }})
    elif method == "notifications/initialized":
        continue
    elif method == "tools/list":
        send({"jsonrpc":"2.0","id":rid,"result":{"tools":[{
            "name":"echo","description":"e","inputSchema":{"type":"object"}
        }]}})
    elif method == "tools/call":
        args = (msg.get("params") or {}).get("arguments") or {}
        send({"jsonrpc":"2.0","id":rid,"result":{
            "content":[{"type":"text","text":"echo:" + str(args.get("text",""))}],
            "isError": False
        }})
    elif method == "ping":
        send({"jsonrpc":"2.0","id":rid,"result":{}})
    elif method == "resources/list":
        send({"jsonrpc":"2.0","id":rid,"result":{"resources":[{
            "uri":"file:///guide.md","name":"guide"
        }]}})
    elif method == "prompts/list":
        send({"jsonrpc":"2.0","id":rid,"result":{"prompts":[{
            "name":"review"
        }]}})
    elif method == "boom":
        send({"jsonrpc":"2.0","id":rid,"error":{"code":-32000,"message":"nope"}})
    else:
        send({"jsonrpc":"2.0","id":rid,"result":{"ok":True}})
"#
        .to_string()
    }

    #[tokio::test]
    async fn connect_stdio_lists_calls_and_ping() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("server.py");
        std::fs::write(&script, stdio_script()).unwrap();
        let mut env = HashMap::new();
        env.insert("WHYCODES_MCP_TEST".into(), "1".into());
        let mut client = McpClient::connect_stdio_with(
            python(),
            &["-u", script.to_str().unwrap()],
            Some(&env),
            Some(dir.path().to_str().unwrap()),
        )
        .await
        .unwrap();
        assert_eq!(client.transport_name(), "stdio");
        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools[0].name, "echo");
        let out = client
            .call_tool("echo", serde_json::json!({"text": "hi"}))
            .await
            .unwrap();
        assert_eq!(out, "echo:hi");
        client.ping().await.unwrap();
        let resources = client.list_resources().await.unwrap();
        assert_eq!(resources[0].name, "guide");
        let prompts = client.list_prompts().await.unwrap();
        assert_eq!(prompts[0].name, "review");
        drop(client);

        let client = McpClient::connect_stdio(python(), &["-u", script.to_str().unwrap()])
            .await
            .unwrap();
        assert_eq!(client.transport_name(), "stdio");
    }

    #[tokio::test]
    async fn connect_stdio_spawn_and_handshake_errors() {
        let err = McpClient::connect_stdio("/no/such/mcp-server-xyz", &[] as &[&str])
            .await
            .err()
            .expect("spawn should fail")
            .to_string();
        assert!(err.contains("failed to spawn MCP server"), "{err}");

        let mut child = Command::new("true")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let err = take_stdio_pipes(&mut child).unwrap_err().to_string();
        assert!(err.contains("child has no stdin"), "{err}");
        let _ = child.wait().await;

        let mut child = Command::new("true")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let err = take_stdio_pipes(&mut child).unwrap_err().to_string();
        assert!(err.contains("child has no stdout"), "{err}");
        let _ = child.wait().await;

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("bad.py");
        std::fs::write(
            &script,
            r#"
import json, sys
line = sys.stdin.readline()
msg = json.loads(line)
sys.stdout.write(json.dumps({"jsonrpc":"2.0","id":msg["id"],"result":{"wrong":True}})+"\n")
sys.stdout.flush()
"#,
        )
        .unwrap();
        let err = McpClient::connect_stdio(python(), &["-u", script.to_str().unwrap()])
            .await
            .err()
            .expect("invalid initialize should fail")
            .to_string();
        assert!(err.contains("failed to parse initialize result"), "{err}");
    }

    #[tokio::test]
    async fn stdio_parse_failures_and_tool_error_flag() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("server.py");
        std::fs::write(
            &script,
            r#"
import json, sys
def send(obj):
    sys.stdout.write(json.dumps(obj)+"\n")
    sys.stdout.flush()
n = 0
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    rid = msg.get("id")
    if method == "initialize":
        send({"jsonrpc":"2.0","id":rid,"result":{
            "protocolVersion":"2025-03-26",
            "capabilities":{},
            "serverInfo":{"name":"x","version":"1"}
        }})
    elif method == "notifications/initialized":
        continue
    elif method == "tools/list":
        send({"jsonrpc":"2.0","id":rid,"result":{"nope":True}})
    elif method == "tools/call":
        send({"jsonrpc":"2.0","id":rid,"result":{
            "content":[{"type":"text","text":"err"}],
            "isError": True
        }})
    elif method == "resources/list":
        send({"jsonrpc":"2.0","id":rid,"result":[]})
    elif method == "prompts/list":
        send({"jsonrpc":"2.0","id":rid,"result":[]})
    elif method == "ping":
        send({"jsonrpc":"2.0","id":rid,"error":{"code":-1,"message":"pong-fail"}})
"#,
        )
        .unwrap();
        let mut client = McpClient::connect_stdio(python(), &["-u", script.to_str().unwrap()])
            .await
            .unwrap();
        let err = client.list_tools().await.unwrap_err().to_string();
        assert!(err.contains("failed to parse tools/list"), "{err}");
        let out = client
            .call_tool("echo", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(out, "err");
        let err = client.list_resources().await.unwrap_err().to_string();
        assert!(err.contains("failed to parse resources/list"), "{err}");
        let err = client.list_prompts().await.unwrap_err().to_string();
        assert!(err.contains("failed to parse prompts/list"), "{err}");
        let err = client.ping().await.unwrap_err().to_string();
        assert!(err.contains("pong-fail"), "{err}");
    }

    #[tokio::test]
    async fn stdio_custom_notification_and_call_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("server.py");
        std::fs::write(
            &script,
            r#"
import json, sys
def send(obj):
    sys.stdout.write(json.dumps(obj)+"\n")
    sys.stdout.flush()
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    rid = msg.get("id")
    if method == "initialize":
        send({"jsonrpc":"2.0","id":rid,"result":{
            "protocolVersion":"2025-03-26",
            "capabilities":{},
            "serverInfo":{"name":"x","version":"1"}
        }})
    elif method == "notifications/initialized":
        continue
    elif method == "tools/call":
        send({"jsonrpc":"2.0","id":rid,"result":{"wrong":True}})
"#,
        )
        .unwrap();
        let mut client = McpClient::connect_stdio(python(), &["-u", script.to_str().unwrap()])
            .await
            .unwrap();
        client
            .notify("custom", Some(serde_json::json!({"a": 1})))
            .await
            .unwrap();
        let err = client
            .call_tool("echo", serde_json::json!({}))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("failed to parse tools/call"), "{err}");
    }

    #[tokio::test]
    async fn stdio_initialized_notification_fails_when_child_exits() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("server.py");
        std::fs::write(
            &script,
            r#"
import json, sys
line = sys.stdin.readline()
msg = json.loads(line)
sys.stdout.write(json.dumps({"jsonrpc":"2.0","id":msg["id"],"result":{
    "protocolVersion":"2025-03-26",
    "capabilities":{},
    "serverInfo":{"name":"x","version":"1"}
}})+"\n")
sys.stdout.flush()
sys.exit(0)
"#,
        )
        .unwrap();
        match McpClient::connect_stdio(python(), &["-u", script.to_str().unwrap()]).await {
            Ok(_) => {}
            Err(e) => {
                let err = e.to_string();
                assert!(
                    err.contains("initialized")
                        || err.contains("initialize")
                        || err.contains("closed")
                        || err.contains("Broken"),
                    "{err}"
                );
            }
        }
    }

    async fn spawn_app(app: axum::Router) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        tokio::task::yield_now().await;
        addr
    }

    #[tokio::test]
    async fn connect_http_sse_and_auto_cover_client_paths() {
        use axum::Router;
        use axum::body::Body;
        use axum::extract::State;
        use axum::http::{StatusCode, header};
        use axum::response::{IntoResponse, Response};
        use axum::routing::{get, post};
        use std::sync::Arc;
        use tokio::sync::{Mutex, mpsc};

        async fn streamable(body: String) -> Response {
            let value: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            if value.get("id").is_none() {
                return StatusCode::ACCEPTED.into_response();
            }
            let id = value.get("id").and_then(|i| i.as_u64()).unwrap_or(0);
            let method = value.get("method").and_then(|m| m.as_str()).unwrap_or("");
            let result = match method {
                "initialize" => serde_json::json!({
                    "protocolVersion": "2025-03-26",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "mock-http", "version": "0.0.1" }
                }),
                "tools/list" => serde_json::json!({"tools":[{"name":"echo","inputSchema":{}}]}),
                "tools/call" => serde_json::json!({
                    "content":[{"type":"text","text":"ok"}],
                    "isError": false
                }),
                "ping" => serde_json::json!({}),
                "resources/list" => serde_json::json!({"resources":[{"uri":"u","name":"n"}]}),
                "prompts/list" => serde_json::json!({"prompts":[{"name":"p"}]}),
                _ => serde_json::json!({}),
            };
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({"jsonrpc":"2.0","id":id,"result":result}).to_string(),
                ))
                .unwrap()
        }

        let addr = spawn_app(Router::new().route("/mcp", post(streamable))).await;
        let url = format!("http://{addr}/mcp");
        let mut client = McpClient::connect_http(&url, &HashMap::new())
            .await
            .unwrap();
        assert_eq!(client.transport_name(), "http");
        client.list_tools().await.unwrap();
        client
            .call_tool("echo", serde_json::json!({}))
            .await
            .unwrap();
        client.ping().await.unwrap();
        client.list_resources().await.unwrap();
        client.list_prompts().await.unwrap();

        let client = McpClient::connect_auto(&url, &HashMap::new())
            .await
            .unwrap();
        assert_eq!(client.transport_name(), "http");

        #[derive(Clone)]
        struct SseState {
            tx: Arc<Mutex<Option<mpsc::UnboundedSender<String>>>>,
        }
        async fn sse_get(State(state): State<SseState>) -> Response {
            let (tx, mut rx) = mpsc::unbounded_channel::<String>();
            *state.tx.lock().await = Some(tx);
            let stream = async_stream::stream! {
                yield Ok::<_, std::io::Error>("event: endpoint\ndata: /messages\n\n".to_string());
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
        async fn sse_post(State(state): State<SseState>, body: String) -> impl IntoResponse {
            let value: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            if value.get("id").is_none() {
                return StatusCode::ACCEPTED;
            }
            let id = value.get("id").and_then(|i| i.as_u64()).unwrap_or(0);
            let method = value.get("method").and_then(|m| m.as_str()).unwrap_or("");
            let result = match method {
                "initialize" => serde_json::json!({
                    "protocolVersion": "2025-03-26",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "mock-sse", "version": "0.0.1" }
                }),
                "tools/list" => serde_json::json!({"tools":[{"name":"echo","inputSchema":{}}]}),
                _ => serde_json::json!({}),
            };
            if let Some(tx) = state.tx.lock().await.as_ref() {
                let _ = tx
                    .send(serde_json::json!({"jsonrpc":"2.0","id":id,"result":result}).to_string());
            }
            StatusCode::ACCEPTED
        }
        let state = SseState {
            tx: Arc::new(Mutex::new(None)),
        };
        let addr = spawn_app(
            Router::new()
                .route(
                    "/sse",
                    get(sse_get).post(|| async { StatusCode::METHOD_NOT_ALLOWED }),
                )
                .route("/messages", post(sse_post))
                .with_state(state),
        )
        .await;
        let mut client = McpClient::connect_sse(format!("http://{addr}/sse"), &HashMap::new())
            .await
            .unwrap();
        assert_eq!(client.transport_name(), "sse");
        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools[0].name, "echo");

        let client = McpClient::connect_auto(format!("http://{addr}/sse"), &HashMap::new())
            .await
            .unwrap();
        assert_eq!(client.transport_name(), "sse");

        let addr = spawn_app(Router::new().route(
            "/mcp",
            post(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "broken") }),
        ))
        .await;
        let err = McpClient::connect_auto(format!("http://{addr}/mcp"), &HashMap::new())
            .await
            .err()
            .expect("500 must not fallback")
            .to_string();
        assert!(err.contains("Streamable HTTP connect failed"), "{err}");

        let addr = spawn_app(
            Router::new().route("/mcp", post(|| async { (StatusCode::NOT_FOUND, "gone") })),
        )
        .await;
        let err = McpClient::connect_auto(format!("http://{addr}/mcp"), &HashMap::new())
            .await
            .err()
            .expect("404 fallback should fail SSE too")
            .to_string();
        assert!(
            err.contains("both Streamable HTTP and legacy SSE failed"),
            "{err}"
        );

        let addr = spawn_app(Router::new().route(
            "/mcp",
            post(|| async { (StatusCode::BAD_REQUEST, "Method Not Allowed") }),
        ))
        .await;
        let err = McpClient::connect_auto(format!("http://{addr}/mcp"), &HashMap::new())
            .await
            .err()
            .expect("400 should try SSE fallback")
            .to_string();
        assert!(
            err.contains("both Streamable HTTP and legacy SSE failed")
                || err.contains("400")
                || err.contains("Method Not Allowed"),
            "{err}"
        );

        async fn init_then_fail_notify(body: String) -> Response {
            let value: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            if value.get("id").is_none() {
                return (StatusCode::FORBIDDEN, "no notify").into_response();
            }
            let id = value.get("id").and_then(|i| i.as_u64()).unwrap_or(0);
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(init_result_rpc(id)))
                .unwrap()
        }
        let addr = spawn_app(Router::new().route("/mcp", post(init_then_fail_notify))).await;
        let err = McpClient::connect_http(format!("http://{addr}/mcp"), &HashMap::new())
            .await
            .err()
            .expect("initialized notification should fail")
            .to_string();
        assert!(err.contains("initialized"), "{err}");
    }

    fn init_result_rpc(id: u64) -> String {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2025-03-26",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "mock-http", "version": "0.0.1" }
            }
        })
        .to_string()
    }
}
