//! MCP client: stdio, Streamable HTTP, and legacy HTTP+SSE.

use std::collections::{HashMap, VecDeque};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use crate::error::{McpError, Result};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt,
    BufReader as AsyncBufReader,
};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tracing::{debug, info, warn};

use crate::http::{LegacySseTransport, StreamableHttpTransport};
use crate::types::{
    CallToolParams, CallToolResult, GetPromptResult, InitializeParams, InitializeResult,
    InitializedNotification, JsonRpcRequest, JsonRpcResponse, ListToolsResult, McpResourceContent,
    McpTool, ReadResourceResult, ServerCapabilities,
};

const PROTOCOL_VERSION: &str = "2025-03-26";

/// Stderr lines kept to explain a server that exits or answers badly.
const STDERR_TAIL_LINES: usize = 5;

/// Last lines a stdio server wrote to stderr.
type StderrTail = Arc<Mutex<VecDeque<String>>>;

#[allow(clippy::large_enum_variant)]
enum Transport {
    Stdio {
        /// Never read directly: held so `kill_on_drop(true)` on the spawned
        /// command reaps the server process when the transport is dropped.
        #[allow(dead_code)]
        child: Child,
        writer: ChildStdin,
        reader: AsyncBufReader<ChildStdout>,
        next_id: u64,
        stderr_tail: StderrTail,
    },
    StreamableHttp(StreamableHttpTransport),
    LegacySse(LegacySseTransport),
}

/// MCP client supporting stdio, Streamable HTTP, and legacy HTTP+SSE.
pub struct McpClient {
    transport: Transport,
    /// What the server advertised in `initialize`.
    capabilities: ServerCapabilities,
}

impl McpClient {
    pub async fn connect_stdio(command: &str, args: &[&str]) -> Result<Self> {
        Self::connect_stdio_with(command, args, None, None).await
    }

    pub async fn connect_stdio_with(
        command: &str,
        args: &[&str],
        env: Option<&HashMap<String, String>>,
        cwd: Option<&str>,
    ) -> Result<Self> {
        // Stderr is piped, not inherited: server logs written straight to
        // the terminal land on top of the TUI.
        let mut cmd = Command::new(spawnable_program(command));
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        whycodes_core::secret_env::strip_secret_env_vars(|name| {
            cmd.env_remove(name);
        });
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
            .map_err(|e| McpError::msg(&format!("failed to spawn MCP server {command:?}: {e}")))?;
        let (writer, stdout) = take_stdio_pipes(&mut child)?;
        let reader = AsyncBufReader::new(stdout);
        let stderr_tail = StderrTail::default();
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(drain_stderr(
                stderr,
                command.to_string(),
                Arc::clone(&stderr_tail),
            ));
        }
        let mut client = Self {
            transport: Transport::Stdio {
                child,
                writer,
                reader,
                next_id: 1,
                stderr_tail,
            },
            capabilities: ServerCapabilities::default(),
        };
        client.initialize_handshake().await?;
        Ok(client)
    }

    pub async fn connect_http(url: &str, headers: &HashMap<String, String>) -> Result<Self> {
        let transport = StreamableHttpTransport::new(url, headers)?;
        let mut client = Self {
            transport: Transport::StreamableHttp(transport),
            capabilities: ServerCapabilities::default(),
        };
        client.initialize_handshake().await?;
        Ok(client)
    }

    pub async fn connect_sse(sse_url: &str, headers: &HashMap<String, String>) -> Result<Self> {
        let transport = LegacySseTransport::connect(sse_url, headers).await?;
        let mut client = Self {
            transport: Transport::LegacySse(transport),
            capabilities: ServerCapabilities::default(),
        };
        client.initialize_handshake().await?;
        Ok(client)
    }

    pub async fn connect_auto(url: &str, headers: &HashMap<String, String>) -> Result<Self> {
        match Self::connect_http(url, headers).await {
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
                    return Err(McpError::msg(&format!(
                        "Streamable HTTP connect failed: {http_err}"
                    )));
                }
                warn!(
                    url = %url,
                    error = %msg,
                    "Streamable HTTP failed; falling back to legacy SSE"
                );
                Self::connect_sse(url, headers).await.map_err(|sse_err| {
                    McpError::msg(&format!(
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
            .map_err(|e| McpError::msg(&format!("initialize request failed: {e}")))?;
        let init_result: InitializeResult = serde_json::from_value(result)
            .map_err(|e| McpError::msg(&format!("failed to parse initialize result: {e}")))?;
        debug!(
            server_name = %init_result.server_info.name,
            server_version = %init_result.server_info.version,
            protocol_version = %init_result.protocol_version,
            "MCP initialize handshake complete"
        );
        self.send_notification("notifications/initialized", None)
            .await
            .map_err(|e| McpError::msg(&format!("failed to send initialized notification: {e}")))?;
        self.capabilities = init_result.capabilities.clone();
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
                stderr_tail,
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
                read_stdio_response(reader, writer, id)
                    .await
                    .map_err(|e| with_stderr_tail(e, stderr_tail))
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
            .map_err(tools_list_failed)?;
        let list: ListToolsResult = serde_json::from_value(result)
            .map_err(|e| McpError::msg(&format!("failed to parse tools/list result: {e}")))?;
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
            .map_err(tools_call_failed)?;
        let call_result: CallToolResult = serde_json::from_value(result)
            .map_err(|e| McpError::msg(&format!("failed to parse tools/call result: {e}")))?;
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
            .map_err(resources_list_failed)?;
        let list: crate::types::ListResourcesResult = serde_json::from_value(result)
            .map_err(|e| McpError::msg(&format!("failed to parse resources/list result: {e}")))?;
        info!(count = list.resources.len(), "MCP resources listed");
        Ok(list.resources)
    }

    pub async fn list_prompts(&mut self) -> Result<Vec<crate::types::McpPrompt>> {
        let result = self
            .send_request("prompts/list", None)
            .await
            .map_err(prompts_list_failed)?;
        let list: crate::types::ListPromptsResult = serde_json::from_value(result)
            .map_err(|e| McpError::msg(&format!("failed to parse prompts/list result: {e}")))?;
        info!(count = list.prompts.len(), "MCP prompts listed");
        Ok(list.prompts)
    }

    /// Contents of one resource (`resources/read`).
    pub async fn read_resource(&mut self, uri: &str) -> Result<Vec<McpResourceContent>> {
        let params = serde_json::json!({"uri": uri});
        let result = self
            .send_request("resources/read", Some(params))
            .await
            .map_err(|e| McpError::msg(&format!("resources/read failed: {e}")))?;
        let read: ReadResourceResult = serde_json::from_value(result)
            .map_err(|e| McpError::msg(&format!("failed to parse resources/read result: {e}")))?;
        Ok(read.contents)
    }

    /// A prompt rendered with `arguments` (`prompts/get`).
    pub async fn get_prompt(
        &mut self,
        name: &str,
        arguments: &HashMap<String, String>,
    ) -> Result<GetPromptResult> {
        let params = serde_json::json!({"name": name, "arguments": arguments});
        let result = self
            .send_request("prompts/get", Some(params))
            .await
            .map_err(|e| McpError::msg(&format!("prompts/get failed: {e}")))?;
        serde_json::from_value(result)
            .map_err(|e| McpError::msg(&format!("failed to parse prompts/get result: {e}")))
    }

    /// The server advertised `prompts` in `initialize`.
    pub fn supports_prompts(&self) -> bool {
        self.capabilities.prompts.is_some()
    }

    /// The server advertised `resources` in `initialize`.
    pub fn supports_resources(&self) -> bool {
        self.capabilities.resources.is_some()
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

/// Read lines until the response to `expected_id`.
///
/// Servers interleave other traffic on stdout: notifications (logging,
/// progress, list changes), their own requests (`ping`), stray log lines,
/// and late answers to requests the caller gave up on. None of that is the
/// response, so it is skipped (requests are answered) instead of failing
/// the call or, for a stale id, returning another request's result.
async fn read_stdio_response(
    reader: &mut (dyn AsyncBufRead + Send + Unpin),
    writer: &mut (dyn AsyncWrite + Send + Unpin),
    expected_id: u64,
) -> Result<serde_json::Value> {
    let mut line = String::new();
    let response = loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .await
            .map_err(stdio_read_failed)?;
        if read == 0 {
            return Err(McpError::msg("MCP server closed stdout (empty response)"));
        }
        let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) else {
            debug!(line = line.trim_end(), "MCP server wrote a non-JSON line");
            continue;
        };
        if let Some(method) = message.get("method").and_then(|m| m.as_str()) {
            if let Some(id) = message.get("id") {
                answer_server_request(writer, id, method).await?;
            }
            continue;
        }
        let response: JsonRpcResponse = serde_json::from_value(message)
            .map_err(|e| McpError::msg(&format!("failed to parse MCP response: {line}: {e}")))?;
        if response.id == expected_id {
            break response;
        }
        warn!(
            expected = expected_id,
            got = response.id,
            "MCP response for an earlier request; skipped"
        );
    };
    if let Some(error) = response.error {
        return Err(McpError::msg(&format!(
            "MCP error [{}]: {}",
            error.code, error.message
        )));
    }
    response
        .result
        .ok_or_else(|| McpError::msg("MCP response has no result"))
}

/// Answer a request the server sent us. Only `ping` is supported: this
/// client advertises no capabilities, so anything else is "not found".
async fn answer_server_request(
    writer: &mut (dyn AsyncWrite + Send + Unpin),
    id: &serde_json::Value,
    method: &str,
) -> Result<()> {
    let reply = if method == "ping" {
        serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {}})
    } else {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": -32601, "message": format!("method not found: {method}")},
        })
    };
    writer.write_all(format!("{reply}\n").as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}

/// Log a stdio server's stderr and keep its last lines for errors. Reads
/// until EOF, so a chatty server never blocks on a full pipe; lines are
/// bytes, so non-UTF-8 output cannot stop the drain.
async fn drain_stderr(stderr: impl AsyncRead + Unpin, command: String, tail: StderrTail) {
    let mut lines = AsyncBufReader::new(stderr).split(b'\n');
    while let Ok(Some(bytes)) = lines.next_segment().await {
        let line = String::from_utf8_lossy(&bytes).trim_end().to_string();
        debug!(server = %command, "{line}");
        let mut tail = tail.lock().unwrap_or_else(|e| e.into_inner());
        if tail.len() == STDERR_TAIL_LINES {
            tail.pop_front();
        }
        tail.push_back(line);
    }
}

/// `e` plus what the server last wrote to stderr, which usually says why.
fn with_stderr_tail(e: McpError, tail: &StderrTail) -> McpError {
    let tail = tail.lock().unwrap_or_else(|e| e.into_inner());
    if tail.is_empty() {
        return e;
    }
    let lines: Vec<&str> = tail.iter().map(String::as_str).collect();
    McpError::msg(&format!("{e} (server stderr: {})", lines.join(" | ")))
}

/// The program to spawn for `command`.
///
/// Windows starts only `.exe` files by bare name, but launchers such as
/// `npx` and `uvx` are `.cmd` scripts, so a bare name is resolved through
/// `PATH` and `PATHEXT` the way a shell would.
#[cfg(windows)]
fn spawnable_program(command: &str) -> std::path::PathBuf {
    resolve_on_path(
        command,
        &std::env::var_os("PATH").unwrap_or_default(),
        &std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into()),
    )
}

#[cfg(windows)]
fn resolve_on_path(command: &str, dirs: &std::ffi::OsStr, exts: &str) -> std::path::PathBuf {
    let path = std::path::Path::new(command);
    if path.extension().is_some() || path.components().count() > 1 {
        return path.to_path_buf();
    }
    std::env::split_paths(dirs)
        .flat_map(|dir| {
            exts.split(';')
                .filter(|e| !e.is_empty())
                .map(move |ext| dir.join(format!("{command}{ext}")))
        })
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| path.to_path_buf())
}

#[cfg(not(windows))]
fn spawnable_program(command: &str) -> &str {
    command
}

fn tools_list_failed(e: McpError) -> McpError {
    McpError::msg(&format!("tools/list failed: {e}"))
}

fn tools_call_failed(e: McpError) -> McpError {
    McpError::msg(&format!("tools/call failed: {e}"))
}

fn resources_list_failed(e: McpError) -> McpError {
    McpError::msg(&format!("resources/list failed: {e}"))
}

fn prompts_list_failed(e: McpError) -> McpError {
    McpError::msg(&format!("prompts/list failed: {e}"))
}

fn stdio_read_failed(e: std::io::Error) -> McpError {
    McpError::msg(&format!("failed to read response from MCP server: {e}"))
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
#[path = "client_tests.rs"]
mod tests;
