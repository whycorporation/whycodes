//! MCP client: stdio, Streamable HTTP, and legacy HTTP+SSE.

use std::collections::HashMap;
use std::process::Stdio;

use anyhow::{Context, Result, bail};
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
        let mut child = cmd
            .spawn()
            .with_context(|| format!("failed to spawn MCP server: {:?}", command.as_ref()))?;
        let writer = child.stdin.take().context("child has no stdin")?;
        let stdout = child.stdout.take().context("child has no stdout")?;
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
                // Flatten the anyhow chain — top-level message is often just
                // "initialize request failed" without the HTTP status.
                let msg = format!("{http_err:#}");
                let looks_like_wrong_transport = msg.contains("405")
                    || msg.contains("404")
                    || msg.contains("400")
                    || msg.contains("Method Not Allowed")
                    || msg.contains("Not Found");
                if !looks_like_wrong_transport {
                    return Err(http_err).context("Streamable HTTP connect failed");
                }
                warn!(
                    url = %url,
                    error = %msg,
                    "Streamable HTTP failed; falling back to legacy SSE"
                );
                Self::connect_sse(&url, headers).await.with_context(|| {
                    format!(
                        "both Streamable HTTP and legacy SSE failed for {url}; HTTP error: {msg}"
                    )
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
            .context("initialize request failed")?;
        let init_result: InitializeResult =
            serde_json::from_value(result).context("failed to parse initialize result")?;
        debug!(
            server_name = %init_result.server_info.name,
            server_version = %init_result.server_info.version,
            protocol_version = %init_result.protocol_version,
            "MCP initialize handshake complete"
        );
        self.send_notification("notifications/initialized", None)
            .await
            .context("failed to send initialized notification")?;
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
            .context("tools/list failed")?;
        let list: ListToolsResult =
            serde_json::from_value(result).context("failed to parse tools/list result")?;
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
            .context("tools/call failed")?;
        let call_result: CallToolResult =
            serde_json::from_value(result).context("failed to parse tools/call result")?;
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
            .context("resources/list failed")?;
        let list: crate::types::ListResourcesResult =
            serde_json::from_value(result).context("failed to parse resources/list result")?;
        info!(count = list.resources.len(), "MCP resources listed");
        Ok(list.resources)
    }

    pub async fn list_prompts(&mut self) -> Result<Vec<crate::types::McpPrompt>> {
        let result = self
            .send_request("prompts/list", None)
            .await
            .context("prompts/list failed")?;
        let list: crate::types::ListPromptsResult =
            serde_json::from_value(result).context("failed to parse prompts/list result")?;
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
}

async fn read_stdio_response(
    reader: &mut AsyncBufReader<ChildStdout>,
    expected_id: u64,
) -> Result<serde_json::Value> {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .context("failed to read response from MCP server")?;
    if line.trim().is_empty() {
        bail!("MCP server closed stdout (empty response)");
    }
    let response: JsonRpcResponse = serde_json::from_str(&line)
        .with_context(|| format!("failed to parse MCP response: {}", line))?;
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

impl Drop for McpClient {
    fn drop(&mut self) {}
}
