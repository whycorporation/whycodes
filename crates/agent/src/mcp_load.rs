//! Load MCP servers from config and register their tools on a ToolExecutor.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
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

#[async_trait]
impl McpCaller for SharedMcpCaller {
    async fn call_mcp_tool(
        &self,
        _tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<String, String> {
        let mut client = self.client.lock().await;
        client
            .call_tool(&self.remote_name, arguments)
            .await
            .map_err(|e| e.to_string())
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
}
