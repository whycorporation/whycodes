//! Minimal MCP **server** (stdio JSON-RPC) exporting whycodes tools (A5).
//!
//! Implements `initialize`, `notifications/initialized`, `tools/list`, `tools/call`.
//! Designed for local hosts (Cursor / Claude Desktop / other agents).

use std::io::{BufRead, Write};
use std::sync::Arc;

use serde_json::{Value, json};
use whycodes_core::types::{PermissionSet, ToolCall};
use whycodes_tools::executor::ToolExecutor;
use whycodes_tools::profile::ToolProfile;
use whycodes_tools::tool::ToolContext;

use crate::types::{
    CallToolResult, InitializeResult, ListToolsResult, McpTool, ServerCapabilities,
    ServerCapabilityTools, ServerInfo, ToolContent,
};

const PROTOCOL_VERSION: &str = "2024-11-05";

#[cfg(test)]
thread_local! {
    static TEST_STDIN: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
    static TEST_STDOUT: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Run the stdio MCP server until stdin closes.
pub async fn run_stdio_server(
    executor: Arc<ToolExecutor>,
    permissions: PermissionSet,
    profile: ToolProfile,
    working_dir: String,
) -> crate::error::Result<()> {
    let (mut stdin, mut stdout) = open_stdio(cfg!(test));
    run_stdio_io(
        &mut stdin,
        &mut stdout,
        executor,
        permissions,
        profile,
        working_dir,
    )
    .await
}

fn open_stdio(for_test: bool) -> (Box<dyn BufRead + Send>, Box<dyn Write + Send>) {
    #[cfg(test)]
    if for_test {
        let input = TEST_STDIN.with(|s| std::mem::take(&mut *s.borrow_mut()));
        return (
            Box::new(std::io::Cursor::new(input)),
            Box::new(CaptureStdout),
        );
    }
    let _ = for_test;
    (
        Box::new(std::io::BufReader::new(std::io::stdin())),
        Box::new(std::io::stdout()),
    )
}

#[cfg(test)]
struct CaptureStdout;

#[cfg(test)]
impl Write for CaptureStdout {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        TEST_STDOUT.with(|s| s.borrow_mut().extend_from_slice(buf));
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

async fn run_stdio_io<R: BufRead, W: Write>(
    reader: &mut R,
    stdout: &mut W,
    executor: Arc<ToolExecutor>,
    permissions: PermissionSet,
    profile: ToolProfile,
    working_dir: String,
) -> crate::error::Result<()> {
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "mcp-serve: bad json");
                continue;
            }
        };
        if let Some(response) =
            handle_rpc(msg, &executor, &permissions, profile, &working_dir).await?
        {
            let line_out = serde_json::to_string(&response)?;
            writeln!(stdout, "{line_out}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// Handle one JSON-RPC message. `None` means a notification (no reply).
pub(crate) async fn handle_rpc(
    msg: Value,
    executor: &ToolExecutor,
    permissions: &PermissionSet,
    profile: ToolProfile,
    working_dir: &str,
) -> crate::error::Result<Option<Value>> {
    let Some(id) = msg.get("id").cloned() else {
        return Ok(None);
    };
    let method = msg
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);

    let response = match method.as_str() {
        "initialize" => {
            let result = InitializeResult {
                protocol_version: PROTOCOL_VERSION.into(),
                capabilities: ServerCapabilities {
                    tools: Some(ServerCapabilityTools {
                        list_changed: Some(false),
                    }),
                    prompts: None,
                    resources: None,
                },
                server_info: ServerInfo {
                    name: "whycodes".into(),
                    version: env!("CARGO_PKG_VERSION").into(),
                },
            };
            rpc_ok(id, serde_json::to_value(result)?)
        }
        "tools/list" => {
            let defs = executor.get_definitions_profile(permissions, profile);
            let tools: Vec<McpTool> = defs
                .iter()
                .map(|d| McpTool {
                    name: d.name.clone(),
                    description: Some(d.description.clone()),
                    input_schema: d.parameters.clone(),
                })
                .collect();
            let result = ListToolsResult {
                tools,
                next_cursor: None,
            };
            rpc_ok(id, serde_json::to_value(result)?)
        }
        "tools/call" => {
            let name = params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            if name.is_empty() {
                rpc_err(id, -32602, "tools/call requires name")
            } else {
                let call = ToolCall {
                    id: "mcp-1".into(),
                    name: name.clone(),
                    arguments,
                };
                let ctx = ToolContext {
                    working_dir: working_dir.to_string(),
                    session_id: None,
                    sandbox: whycodes_core::SandboxSettings::default(),
                    network: whycodes_core::NetworkPolicy::unrestricted(),
                    file_claims: None,
                    agent_id: None,
                    agent_label: None,
                    file_index: None,
                    panel: None,
                    todo_sink: None,
                    swarm_hub: None,
                };
                let result = executor.execute(&call, &ctx, permissions).await;
                let out = CallToolResult {
                    content: vec![ToolContent::Text {
                        text: result.content,
                    }],
                    is_error: Some(result.is_error),
                };
                rpc_ok(id, serde_json::to_value(out)?)
            }
        }
        "ping" => rpc_ok(id, json!({})),
        other => rpc_err(id, -32601, &format!("method not found: {other}")),
    };
    Ok(Some(response))
}

fn rpc_ok(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

fn rpc_err(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
