//! Load MCP servers from config and register their tools on a ToolExecutor.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{info, warn};
use whycodes_config::{Config, McpServerConfig, McpTransportKind};
use whycodes_mcp::client::McpClient;
use whycodes_tools::executor::ToolExecutor;
use whycodes_tools::mcp::{McpCaller, McpToolBridge};

struct SharedMcpCaller {
    client: Arc<Mutex<McpClient>>,
    remote_name: String,
}

impl McpCaller for SharedMcpCaller {
    fn call_mcp_tool<'a>(
        &'a self,
        _tool_name: &'a str,
        arguments: serde_json::Value,
    ) -> futures::future::BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let mut client = self.client.lock().await;
            client
                .call_tool(&self.remote_name, arguments)
                .await
                .map_err(|e| e.to_string())
        })
    }
}

pub async fn connect_mcp_server(server: &McpServerConfig) -> anyhow::Result<McpClient> {
    let kind = server
        .resolved_transport()
        .map_err(|e| anyhow::anyhow!(e))?;
    let empty_headers = HashMap::new();
    let headers = server.headers.as_ref().unwrap_or(&empty_headers);
    match kind {
        McpTransportKind::Stdio => {
            let command = server
                .command
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("stdio MCP server missing `command`"))?;
            let args: Vec<&str> = server.args.iter().map(|s| s.as_str()).collect();
            Ok(McpClient::connect_stdio_with(
                command,
                &args,
                server.env.as_ref(),
                server.cwd.as_deref(),
            )
            .await?)
        }
        McpTransportKind::Http => {
            let url = server
                .url
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("http MCP server missing `url`"))?;
            Ok(McpClient::connect_http(url, headers).await?)
        }
        McpTransportKind::Sse => {
            let url = server
                .url
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("sse MCP server missing `url`"))?;
            Ok(McpClient::connect_sse(url, headers).await?)
        }
        McpTransportKind::Auto => {
            let url = server
                .url
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("auto MCP server missing `url`"))?;
            Ok(McpClient::connect_auto(url, headers).await?)
        }
    }
}

pub async fn register_mcp_tools(executor: &mut ToolExecutor, config: &Config) -> usize {
    // Connect all servers concurrently so multi-MCP cold start is max(latency),
    // not sum — list_tools still runs after each connect succeeds.
    let connect_futs: Vec<_> = config
        .mcp_servers
        .iter()
        .map(|(server_name, server)| {
            let server_name = server_name.clone();
            let server = server.clone();
            async move {
                let result = connect_mcp_server(&server).await;
                (server_name, server, result)
            }
        })
        .collect();
    let connected = futures::future::join_all(connect_futs).await;

    let mut count = 0usize;
    for (server_name, server, result) in connected {
        match result {
            Ok(client) => {
                let transport = client.transport_name();
                let client = Arc::new(Mutex::new(client));
                let tools = {
                    let mut c = client.lock().await;
                    match c.list_tools().await {
                        Ok(t) => t,
                        Err(e) => {
                            warn!(server = %server_name, error = %e, "MCP tools/list failed");
                            continue;
                        }
                    }
                };
                for tool in tools {
                    let bridge_name = format!("{}_{}", server_name, tool.name);
                    let description = tool
                        .description
                        .clone()
                        .unwrap_or_else(|| format!("MCP tool from '{}'", server_name));
                    let schema = if tool.input_schema.is_null() {
                        serde_json::json!({"type": "object", "properties": {}})
                    } else {
                        tool.input_schema.clone()
                    };
                    let caller: Arc<dyn McpCaller> = Arc::new(SharedMcpCaller {
                        client: Arc::clone(&client),
                        remote_name: tool.name.clone(),
                    });
                    executor.register(Box::new(McpToolBridge::new(
                        caller,
                        bridge_name.clone(),
                        description,
                        schema,
                    )));
                    count += 1;
                    info!(
                        server = %server_name,
                        tool = %bridge_name,
                        transport,
                        "Registered MCP tool"
                    );
                }
            }
            Err(e) => {
                let detail = match server.resolved_transport() {
                    Ok(McpTransportKind::Stdio) => {
                        format!("command={}", server.command.as_deref().unwrap_or("?"))
                    }
                    Ok(_) => format!("url={}", server.url.as_deref().unwrap_or("?")),
                    Err(msg) => msg,
                };
                warn!(
                    server = %server_name,
                    %detail,
                    error = %e,
                    "Failed to connect MCP server"
                );
            }
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(transport: McpTransportKind) -> McpServerConfig {
        McpServerConfig {
            transport: Some(transport),
            command: None,
            args: Vec::new(),
            env: None,
            cwd: None,
            url: None,
            headers: None,
        }
    }

    fn err_of(result: anyhow::Result<McpClient>) -> String {
        match result {
            Ok(_) => panic!("expected connect to fail"),
            Err(e) => e.to_string(),
        }
    }

    #[tokio::test]
    async fn stdio_without_command_errors() {
        let err = err_of(connect_mcp_server(&server(McpTransportKind::Stdio)).await);
        assert!(err.contains("command"), "{err}");
    }

    #[tokio::test]
    async fn remote_transports_require_url() {
        for kind in [
            McpTransportKind::Http,
            McpTransportKind::Sse,
            McpTransportKind::Auto,
        ] {
            let err = err_of(connect_mcp_server(&server(kind)).await);
            assert!(err.contains("url"), "{kind:?}: {err}");
        }
    }

    #[tokio::test]
    async fn neither_command_nor_url_errors() {
        let s = McpServerConfig {
            transport: None,
            command: None,
            args: Vec::new(),
            env: None,
            cwd: None,
            url: None,
            headers: None,
        };
        let err = err_of(connect_mcp_server(&s).await);
        assert!(err.contains("command") || err.contains("url"), "{err}");
    }

    #[tokio::test]
    async fn register_with_no_servers_returns_zero() {
        let mut executor = ToolExecutor::new();
        let config = Config::default();
        assert_eq!(config.mcp_servers.len(), 0);
        let count = register_mcp_tools(&mut executor, &config).await;
        assert_eq!(count, 0);
    }

    #[test]
    fn resolved_transport_auto_for_url_only() {
        let s = McpServerConfig {
            transport: None,
            command: None,
            args: Vec::new(),
            env: None,
            cwd: None,
            url: Some("https://mcp.example.com".into()),
            headers: None,
        };
        assert_eq!(s.resolved_transport().unwrap(), McpTransportKind::Auto);
        assert!(s.is_remote());
    }

    #[tokio::test]
    async fn register_failing_stdio_command_returns_zero() {
        let mut executor = ToolExecutor::new();
        let mut config = Config::default();
        config.mcp_servers.insert(
            "ghost".into(),
            McpServerConfig {
                transport: Some(McpTransportKind::Stdio),
                command: Some("whycodes-definitely-missing-mcp-binary".into()),
                args: Vec::new(),
                env: None,
                cwd: None,
                url: None,
                headers: None,
            },
        );
        let count = register_mcp_tools(&mut executor, &config).await;
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn connect_stdio_missing_binary_errors() {
        let s = McpServerConfig {
            transport: Some(McpTransportKind::Stdio),
            command: Some("whycodes-definitely-missing-mcp-binary".into()),
            args: vec!["--help".into()],
            env: None,
            cwd: None,
            url: None,
            headers: None,
        };
        let err = err_of(connect_mcp_server(&s).await);
        assert!(!err.is_empty(), "{err}");
    }

    #[tokio::test]
    async fn register_failing_url_logs_url_detail() {
        let mut executor = ToolExecutor::new();
        let mut config = Config::default();
        config.mcp_servers.insert(
            "ghost-http".into(),
            McpServerConfig {
                transport: Some(McpTransportKind::Http),
                command: None,
                args: Vec::new(),
                env: None,
                cwd: None,
                url: Some("http://127.0.0.1:1".into()),
                headers: None,
            },
        );
        let count = register_mcp_tools(&mut executor, &config).await;
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn connect_http_sse_auto_with_bad_url_errors() {
        for kind in [
            McpTransportKind::Http,
            McpTransportKind::Sse,
            McpTransportKind::Auto,
        ] {
            let s = McpServerConfig {
                transport: Some(kind),
                command: None,
                args: Vec::new(),
                env: None,
                cwd: None,
                url: Some("http://127.0.0.1:1".into()),
                headers: Some(std::collections::HashMap::from([(
                    "X-Test".into(),
                    "1".into(),
                )])),
            };
            let err = err_of(connect_mcp_server(&s).await);
            assert!(!err.is_empty(), "{kind:?}: {err}");
        }
    }

    fn python() -> &'static str {
        if std::path::Path::new("/usr/bin/python3").exists() {
            "/usr/bin/python3"
        } else {
            "python3"
        }
    }

    fn write_stdio_script(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
        let script = dir.join("server.py");
        std::fs::write(&script, body).unwrap();
        script
    }

    fn stdio_echo_script() -> String {
        r#"
import json, sys
def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()
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
        send({"jsonrpc":"2.0","id":rid,"result":{"tools":[
            {"name":"echo","description":"e","inputSchema":{"type":"object"}},
            {"name":"bare","description":"bare","inputSchema":{"type":"object"}}
        ]}})
    elif method == "tools/call":
        args = (msg.get("params") or {}).get("arguments") or {}
        send({"jsonrpc":"2.0","id":rid,"result":{
            "content":[{"type":"text","text":"echo:" + str(args.get("text",""))}],
            "isError": False
        }})
    else:
        send({"jsonrpc":"2.0","id":rid,"result":{"ok":True}})
"#
        .to_string()
    }

    fn stdio_list_error_script() -> String {
        r#"
import json, sys
def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()
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
            "serverInfo":{"name":"mock-list-err","version":"0.0.1"}
        }})
    elif method == "notifications/initialized":
        continue
    elif method == "tools/list":
        send({"jsonrpc":"2.0","id":rid,"error":{"code":-32000,"message":"nope"}})
    else:
        send({"jsonrpc":"2.0","id":rid,"result":{"ok":True}})
"#
        .to_string()
    }

    fn stdio_config(dir: &std::path::Path, script: &std::path::Path) -> McpServerConfig {
        McpServerConfig {
            transport: Some(McpTransportKind::Stdio),
            command: Some(python().into()),
            args: vec!["-u".into(), script.to_string_lossy().into_owned()],
            env: None,
            cwd: Some(dir.to_string_lossy().into_owned()),
            url: None,
            headers: None,
        }
    }

    #[tokio::test]
    async fn register_stdio_success_and_call_bridged_tool() {
        let dir = tempfile::tempdir().unwrap();
        let script = write_stdio_script(dir.path(), &stdio_echo_script());
        let mut executor = ToolExecutor::new();
        let mut config = Config::default();
        config
            .mcp_servers
            .insert("ghost".into(), stdio_config(dir.path(), &script));
        let count = register_mcp_tools(&mut executor, &config).await;
        assert!(count >= 1, "expected at least one MCP tool, got {count}");
        assert!(executor.get("ghost_echo").is_some());
        assert!(executor.get("ghost_bare").is_some());

        let ctx = whycodes_core::ToolContext::new(dir.path().to_string_lossy().into_owned());
        let call = whycodes_core::types::ToolCall {
            id: "t1".into(),
            name: "ghost_echo".into(),
            arguments: serde_json::json!({"text": "hi"}),
        };
        let result = executor
            .execute(&call, &ctx, &whycodes_core::types::PermissionSet::default())
            .await;
        assert!(!result.is_error, "{result:?}");
        assert!(
            result.content.contains("echo:") || result.content.contains("hi"),
            "{}",
            result.content
        );
    }

    #[tokio::test]
    async fn register_stdio_list_tools_error_skips_server() {
        let dir = tempfile::tempdir().unwrap();
        let script = write_stdio_script(dir.path(), &stdio_list_error_script());
        let mut executor = ToolExecutor::new();
        let mut config = Config::default();
        config
            .mcp_servers
            .insert("ghost".into(), stdio_config(dir.path(), &script));
        let count = register_mcp_tools(&mut executor, &config).await;
        assert_eq!(count, 0);
        assert!(executor.get("ghost_echo").is_none());
    }

    #[tokio::test]
    async fn register_unresolved_transport_logs_err_detail() {
        let mut executor = ToolExecutor::new();
        let mut config = Config::default();
        config.mcp_servers.insert(
            "ghost-bad".into(),
            McpServerConfig {
                transport: None,
                command: None,
                args: Vec::new(),
                env: None,
                cwd: None,
                url: None,
                headers: None,
            },
        );
        let count = register_mcp_tools(&mut executor, &config).await;
        assert_eq!(count, 0);
    }
}
