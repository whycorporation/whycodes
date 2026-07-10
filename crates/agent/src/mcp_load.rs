//! Load MCP servers from config and register their tools on a ToolExecutor.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{info, warn};
use whycode_core::config::Config;
use whycode_mcp::client::McpClient;
use whycode_tools::executor::ToolExecutor;
use whycode_tools::mcp_tool::{McpCaller, McpToolBridge};

/// Shared MCP client for one server process.
struct SharedMcpCaller {
    client: Arc<Mutex<McpClient>>,
    /// Remote tool name as advertised by the MCP server
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

/// Connect configured MCP servers and register tools as `{server}_{tool}`.
/// Failures for individual servers are logged and skipped.
pub async fn register_mcp_tools(executor: &mut ToolExecutor, config: &Config) -> usize {
    let mut count = 0usize;

    for (server_name, server) in &config.mcp_servers {
        let args: Vec<&str> = server.args.iter().map(|s| s.as_str()).collect();
        match McpClient::connect_stdio(&server.command, &args).await {
            Ok(client) => {
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
                    info!(server = %server_name, tool = %bridge_name, "Registered MCP tool");
                }
            }
            Err(e) => {
                warn!(
                    server = %server_name,
                    command = %server.command,
                    error = %e,
                    "Failed to connect MCP server"
                );
            }
        }
    }

    count
}
