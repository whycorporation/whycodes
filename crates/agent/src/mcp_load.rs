//! Load MCP servers from config and register their tools on a ToolExecutor.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{info, warn};
use whycodes_config::{Config, McpServerConfig, McpTransportKind};
use whycodes_mcp::client::McpClient;
use whycodes_tools::executor::ToolExecutor;
use whycodes_tools::mcp::{McpCaller, McpToolBridge};

/// A connected MCP server, kept for its prompts and resources.
#[derive(Clone)]
pub struct McpServerHandle {
    pub name: String,
    pub client: Arc<Mutex<McpClient>>,
}

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
            mcp_call_error(client.call_tool(&self.remote_name, arguments).await)
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
            // `resolved_transport` already requires `command` for stdio.
            let command = stdio_command(server.command.as_deref());
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
            let url = required_mcp_field(server.url.as_deref(), "http MCP server missing `url`")?;
            Ok(McpClient::connect_http(url, headers).await?)
        }
        McpTransportKind::Sse => {
            let url = required_mcp_field(server.url.as_deref(), "sse MCP server missing `url`")?;
            Ok(McpClient::connect_sse(url, headers).await?)
        }
        McpTransportKind::Auto => {
            let url = required_mcp_field(server.url.as_deref(), "auto MCP server missing `url`")?;
            Ok(McpClient::connect_auto(url, headers).await?)
        }
    }
}

/// Connect every configured server and register its tools, plus the
/// resource tools when a server offers resources. Returns the number of
/// tools registered and the connected servers.
pub async fn register_mcp_tools(
    executor: &mut ToolExecutor,
    config: &Config,
) -> (usize, Vec<McpServerHandle>) {
    // Connect all servers concurrently so multi-MCP cold start is max(latency),
    // not sum — list_tools still runs after each connect succeeds.
    let data_dir = whycodes_core::paths::data_dir();
    let connect_futs: Vec<_> = config
        .mcp_servers
        .iter()
        .map(|(server_name, server)| {
            let server_name = server_name.clone();
            let server = server.clone();
            let data_dir = data_dir.clone();
            async move {
                let server = with_oauth_header(&server_name, server, &data_dir).await;
                let result = connect_mcp_server(&server).await;
                (server_name, server, result)
            }
        })
        .collect();
    let connected = futures::future::join_all(connect_futs).await;

    let mut count = 0usize;
    let mut servers = Vec::new();
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
                servers.push(McpServerHandle {
                    name: server_name.clone(),
                    client: Arc::clone(&client),
                });
                for tool in tools {
                    let bridge_name = bridge_tool_name(&server_name, &tool.name);
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
                let hint = login_hint(&server_name, &e.to_string());
                warn!(
                    server = %server_name,
                    %detail,
                    error = %e,
                    hint,
                    "Failed to connect MCP server"
                );
            }
        }
    }
    count += crate::mcp_content::register_resource_tools(executor, &servers).await;
    (count, servers)
}

/// Add a stored OAuth bearer (`whycodes mcp login`) to a remote server's
/// headers, unless the config already sets `Authorization`.
pub(crate) async fn with_oauth_header(
    name: &str,
    mut server: McpServerConfig,
    data_dir: &std::path::Path,
) -> McpServerConfig {
    if !server.is_remote() {
        return server;
    }
    let headers = server.headers.get_or_insert_with(HashMap::new);
    if headers
        .keys()
        .any(|k| k.eq_ignore_ascii_case("authorization"))
    {
        return server;
    }
    if let Some(token) = whycodes_mcp::oauth::bearer(name, data_dir).await {
        headers.insert("Authorization".into(), format!("Bearer {token}"));
    }
    server
}

/// What to do about a connect error: a 401 means the server wants OAuth.
pub(crate) fn login_hint(server: &str, error: &str) -> String {
    if error.contains("401") {
        format!("run `whycodes mcp login {server}`")
    } else {
        String::new()
    }
}

fn mcp_call_error(result: Result<String, whycodes_mcp::McpError>) -> Result<String, String> {
    result.map_err(|e| e.to_string())
}

fn required_mcp_field<'a>(
    value: Option<&'a str>,
    missing: &'static str,
) -> anyhow::Result<&'a str> {
    value.ok_or_else(|| anyhow::anyhow!(missing))
}

fn stdio_command(command: Option<&str>) -> &str {
    command.unwrap_or("")
}

/// Longest tool name OpenAI-compatible APIs accept.
const MAX_TOOL_NAME: usize = 64;

/// Wire-safe `{server}_{tool}` name for a bridged MCP tool.
///
/// OpenAI-compatible APIs accept `^[a-zA-Z0-9_-]{1,64}$` and reject the
/// whole request otherwise, while MCP servers may use dots, slashes or long
/// names. A valid name is left unchanged (tool names are stable API). Any
/// other character becomes `_`; past 64 chars the name keeps a 55-char
/// prefix plus an FNV-1a hash of the full name, so long siblings stay
/// distinct and the name is the same in every session.
pub(crate) fn bridge_tool_name(server: &str, tool: &str) -> String {
    let raw = format!("{server}_{tool}");
    let mut name: String = raw.chars().map(wire_char).collect();
    if name.len() > MAX_TOOL_NAME {
        let hash = fnv1a(raw.as_bytes());
        name.truncate(MAX_TOOL_NAME - 9);
        name.push_str(&format!("_{hash:08x}"));
    }
    name
}

fn wire_char(c: char) -> char {
    if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
        c
    } else {
        '_'
    }
}

fn fnv1a(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for b in bytes {
        hash ^= u32::from(*b);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

#[cfg(test)]
#[path = "mcp_load_tests.rs"]
mod tests;
