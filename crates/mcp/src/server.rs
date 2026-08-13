//! Minimal MCP **server** (stdio JSON-RPC) exporting whycode tools (A5).
//!
//! Implements `initialize`, `notifications/initialized`, `tools/list`, `tools/call`.
//! Designed for local hosts (Cursor / Claude Desktop / other agents).

use std::io::{BufRead, Write};
use std::sync::Arc;

use serde_json::{Value, json};
use whycode_core::types::{PermissionSet, ToolCall};
use whycode_tools::executor::ToolExecutor;
use whycode_tools::profile::ToolProfile;
use whycode_tools::tool::ToolContext;

use crate::types::{
    CallToolResult, InitializeResult, ListToolsResult, McpTool, ServerCapabilities,
    ServerCapabilityTools, ServerInfo, ToolContent,
};

const PROTOCOL_VERSION: &str = "2024-11-05";

/// Run the stdio MCP server until stdin closes.
pub async fn run_stdio_server(
    executor: Arc<ToolExecutor>,
    permissions: PermissionSet,
    profile: ToolProfile,
    working_dir: String,
) -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut reader = stdin.lock();
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
        // Notification (no id)
        if msg.get("id").is_none() {
            let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
            if method == "notifications/initialized" {
                continue;
            }
            continue;
        }
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
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
                        name: "whycode".into(),
                        version: env!("CARGO_PKG_VERSION").into(),
                    },
                };
                rpc_ok(id, serde_json::to_value(result)?)
            }
            "tools/list" => {
                let defs = executor.get_definitions_profile(&permissions, profile);
                let tools: Vec<McpTool> = defs
                    .into_iter()
                    .map(|d| McpTool {
                        name: d.name,
                        description: Some(d.description),
                        input_schema: d.parameters,
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
                        working_dir: working_dir.clone(),
                        session_id: None,
                        sandbox: whycode_core::SandboxSettings::default(),
                        network: whycode_core::NetworkPolicy::unrestricted(),
                        file_claims: None,
                        agent_id: None,
                        agent_label: None,
                        file_index: None,
                        panel: None,
                        swarm_hub: None,
                    };
                    let result = executor.execute(&call, &ctx, &permissions).await;
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

        let line_out = serde_json::to_string(&response)?;
        writeln!(stdout, "{line_out}")?;
        stdout.flush()?;
    }
    Ok(())
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
