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
    #[cfg(test)]
    {
        let input = TEST_STDIN.with(|s| std::mem::take(&mut *s.borrow_mut()));
        let mut cursor = std::io::Cursor::new(input);
        let mut captured = Vec::new();
        let result = run_stdio_io(
            &mut cursor,
            &mut captured,
            executor,
            permissions,
            profile,
            working_dir,
        )
        .await;
        TEST_STDOUT.with(|s| *s.borrow_mut() = captured);
        result
    }
    #[cfg(not(test))]
    {
        let stdin = std::io::stdin();
        let mut stdout = std::io::stdout();
        let mut reader = stdin.lock();
        run_stdio_io(
            &mut reader,
            &mut stdout,
            executor,
            permissions,
            profile,
            working_dir,
        )
        .await
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
mod tests {
    use super::*;
    use whycodes_tools::executor::ToolExecutor;

    fn exec() -> ToolExecutor {
        ToolExecutor::new()
    }

    fn perms() -> PermissionSet {
        PermissionSet {
            allow_file_writes: true,
            allow_network: true,
            allow_shell: true,
            ..Default::default()
        }
    }

    async fn rpc(msg: Value) -> Option<Value> {
        handle_rpc(msg, &exec(), &perms(), ToolProfile::Core, ".")
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn notification_has_no_reply() {
        let msg = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        assert!(rpc(msg).await.is_none());
    }

    #[tokio::test]
    async fn initialize_and_ping() {
        let init = rpc(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}))
            .await
            .unwrap();
        assert_eq!(init["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(init["result"]["serverInfo"]["name"], "whycodes");

        let ping = rpc(json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}))
            .await
            .unwrap();
        assert_eq!(ping["result"], json!({}));
    }

    #[tokio::test]
    async fn tools_list_includes_core_read() {
        let resp = rpc(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"}))
            .await
            .unwrap();
        let tools = resp["result"]["tools"].as_array().expect("tools array");
        assert!(
            tools.iter().any(|t| t["name"] == "read"),
            "core profile should advertise read: {tools:?}"
        );
    }

    #[tokio::test]
    async fn tools_call_requires_name() {
        let resp = rpc(json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {}
        }))
        .await
        .unwrap();
        assert_eq!(resp["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn tools_call_reads_a_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("note.txt"), "hello mcp").unwrap();
        let resp = handle_rpc(
            json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": {
                    "name": "read",
                    "arguments": {"path": "note.txt"}
                }
            }),
            &exec(),
            &perms(),
            ToolProfile::Core,
            dir.path().to_str().unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(resp["result"]["isError"], false);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("hello mcp"), "{text}");
    }

    #[tokio::test]
    async fn unknown_method_is_minus_32601() {
        let resp = rpc(json!({"jsonrpc": "2.0", "id": 6, "method": "nope"}))
            .await
            .unwrap();
        assert_eq!(resp["error"]["code"], -32601);
        assert!(
            resp["error"]["message"]
                .as_str()
                .unwrap()
                .contains("method not found"),
            "{resp}"
        );
    }

    #[tokio::test]
    async fn tools_call_defaults_arguments_and_empty_method() {
        let missing_method = rpc(json!({"jsonrpc": "2.0", "id": 7})).await.unwrap();
        assert_eq!(missing_method["error"]["code"], -32601);

        let numeric_method = rpc(json!({"jsonrpc": "2.0", "id": 8, "method": 1}))
            .await
            .unwrap();
        assert_eq!(numeric_method["error"]["code"], -32601);

        let numeric_name = rpc(json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": "tools/call",
            "params": {"name": 3}
        }))
        .await
        .unwrap();
        assert_eq!(numeric_name["error"]["code"], -32602);

        let resp = rpc(json!({
            "jsonrpc": "2.0",
            "id": 10,
            "method": "tools/call",
            "params": {"name": "read"}
        }))
        .await
        .unwrap();
        assert_eq!(resp["result"]["isError"], true);
    }

    #[tokio::test]
    async fn run_stdio_io_skips_blank_and_bad_json_then_replies() {
        let input = concat!(
            "\n",
            "   \n",
            "not-json\n",
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
            "\n",
        );
        let mut reader = std::io::Cursor::new(input.as_bytes().to_vec());
        let mut stdout = Vec::new();
        run_stdio_io(
            &mut reader,
            &mut stdout,
            Arc::new(exec()),
            perms(),
            ToolProfile::Core,
            ".".into(),
        )
        .await
        .unwrap();
        let text = String::from_utf8(stdout).unwrap();
        assert!(text.contains("\"id\":1"), "{text}");
        assert!(text.contains("\"result\":{}"), "{text}");
    }

    #[tokio::test]
    async fn run_stdio_server_uses_test_stdin() {
        let input = concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
            "\n",
        );
        TEST_STDIN.with(|s| *s.borrow_mut() = input.as_bytes().to_vec());
        run_stdio_server(Arc::new(exec()), perms(), ToolProfile::Core, ".".into())
            .await
            .unwrap();
        let out = TEST_STDOUT.with(|s| s.borrow().clone());
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("protocolVersion"), "{text}");
        assert!(text.contains("\"id\":2"), "{text}");
    }

    #[tokio::test]
    async fn run_stdio_io_surfaces_read_and_write_errors() {
        struct FailRead;
        impl std::io::Read for FailRead {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("read boom"))
            }
        }
        impl std::io::BufRead for FailRead {
            fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
                Err(std::io::Error::other("read boom"))
            }
            fn consume(&mut self, _: usize) {}
        }
        struct FailWrite;
        impl std::io::Write for FailWrite {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("write boom"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let err = run_stdio_io(
            &mut FailRead,
            &mut Vec::new(),
            Arc::new(exec()),
            perms(),
            ToolProfile::Core,
            ".".into(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("read boom"), "{err}");

        let mut reader =
            std::io::Cursor::new(br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#.to_vec());
        let err = run_stdio_io(
            &mut reader,
            &mut FailWrite,
            Arc::new(exec()),
            perms(),
            ToolProfile::Core,
            ".".into(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("write boom"), "{err}");
    }
}
