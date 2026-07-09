use std::process::Stdio;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as AsyncBufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tracing::{debug, info, warn};

use crate::types::{
    CallToolParams, CallToolResult, InitializeParams, InitializeResult, InitializedNotification,
    JsonRpcRequest, JsonRpcResponse, ListToolsResult, McpTool,
};

/// A client for communicating with an MCP server via stdio.
///
/// Uses line-delimited JSON over stdin/stdout.  Spawns the server process
/// and performs the MCP handshake (`initialize` + `notifications/initialized`) on
/// connect.
pub struct McpClient {
    #[allow(dead_code)]
    child: Child,
    writer: ChildStdin,
    reader: AsyncBufReader<ChildStdout>,
    next_id: u64,
}

impl McpClient {
    /// Spawn an MCP server process, perform the initialize handshake, and
    /// return a ready-to-use client.
    pub async fn connect_stdio(
        command: impl AsRef<std::ffi::OsStr>,
        args: &[impl AsRef<std::ffi::OsStr>],
    ) -> Result<Self> {
        let mut child = Command::new(command.as_ref())
            .args(args.iter().map(|a| a.as_ref()))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to spawn MCP server: {:?}", command.as_ref()))?;

        let writer = child.stdin.take().context("child has no stdin")?;
        let stdout = child.stdout.take().context("child has no stdout")?;

        let reader = AsyncBufReader::new(stdout);

        let mut client = Self {
            child,
            writer,
            reader,
            next_id: 1,
        };

        // Perform MCP handshake
        client.initialize_handshake().await?;

        Ok(client)
    }

    /// Send `initialize` + `notifications/initialized` to complete the
    /// MCP handshake.  Called automatically by `connect_stdio`.
    async fn initialize_handshake(&mut self) -> Result<InitializeResult> {
        let params = serde_json::to_value(InitializeParams {
            protocol_version: "2025-03-26".to_string(),
            capabilities: Default::default(),
            client_info: crate::types::ClientInfo {
                name: "whycode".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        })?;

        let result = self
            .send_request("initialize", Some(params))
            .await
            .context("initialize request failed")?;

        let init_result: InitializeResult = serde_json::from_value(result)
            .context("failed to parse initialize result")?;

        debug!(
            server_name = %init_result.server_info.name,
            server_version = %init_result.server_info.version,
            protocol_version = %init_result.protocol_version,
            "MCP initialize handshake complete"
        );

        // Send `notifications/initialized`
        let notification = serde_json::to_string(&InitializedNotification::new())?;
        self.writer
            .write_all(notification.as_bytes())
            .await
            .context("failed to send initialized notification")?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await?;

        Ok(init_result)
    }

    /// Send a JSON-RPC request and return the parsed `result` field.
    async fn send_request(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let id = self.next_id;
        self.next_id += 1;

        let request = JsonRpcRequest::new(id, method, params);
        let line = serde_json::to_string(&request)?;

        debug!(%line, "MCP request");

        self.writer.write_all(line.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await?;

        self.read_response(id).await
    }

    /// Read a single JSON-RPC response line and match it to the expected id.
    async fn read_response(&mut self, expected_id: u64) -> Result<serde_json::Value> {
        let mut line = String::new();
        self.reader
            .read_line(&mut line)
            .await
            .context("failed to read response from MCP server")?;

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
            anyhow::bail!(
                "MCP error [{}]: {}",
                error.code,
                error.message
            );
        }

        response.result.context("MCP response has no result")
    }

    /// List the tools advertised by the MCP server.
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

    /// Call a named tool with the given arguments.
    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<String> {
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

    /// Ping the server (useful for health checks).
    pub async fn ping(&mut self) -> Result<()> {
        self.send_request("ping", None).await?;
        debug!("MCP ping ok");
        Ok(())
    }

    /// List resources advertised by the MCP server.
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

    /// List prompts advertised by the MCP server.
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
}

impl Drop for McpClient {
    fn drop(&mut self) {
        // `kill_on_drop` will handle process cleanup
    }
}
