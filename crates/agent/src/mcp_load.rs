//! Load MCP servers from config and register their tools on a ToolExecutor.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tracing::{info, warn};
use whycodes_config::{Config, McpServerConfig, McpTransportKind};
use whycodes_mcp::McpTool;
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
            // Fail now, not after the connect timeout.
            if let Some(problem) = server.stdio_problem() {
                anyhow::bail!(problem);
            }
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

/// A configured server that did not connect, with what to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpFailure {
    pub server: String,
    pub error: String,
    /// Empty when there is nothing specific to suggest.
    pub hint: String,
}

/// Connect every configured server and register its tools, plus the
/// resource tools when a server offers resources. Returns the number of
/// tools registered, the connected servers and the ones that failed.
pub async fn register_mcp_tools(
    executor: &mut ToolExecutor,
    config: &Config,
) -> (usize, Vec<McpServerHandle>, Vec<McpFailure>) {
    register_mcp_tools_within(executor, config, SERVER_START_TIMEOUT).await
}

/// How long one server may take to connect and list its tools. `npx -y`
/// can download the server first, so this is generous; nothing waits on it
/// in the foreground for long (the TUI loads servers in the background).
pub(crate) const SERVER_START_TIMEOUT: Duration = Duration::from_secs(60);

async fn register_mcp_tools_within(
    executor: &mut ToolExecutor,
    config: &Config,
    limit: Duration,
) -> (usize, Vec<McpServerHandle>, Vec<McpFailure>) {
    let (mut connects, rest) = connect_servers(config, limit, limit).await;
    if let Some(rest) = rest {
        connects.absorb(rest.await);
    }
    let count = connects.register(executor).await;
    (count, connects.handles(), connects.failures)
}

/// A connected server with the tools it listed.
struct Connected {
    name: String,
    transport: &'static str,
    client: Arc<Mutex<McpClient>>,
    tools: Vec<McpTool>,
}

/// The servers that finished connecting, one way or the other.
#[derive(Default)]
pub(crate) struct McpConnects {
    connected: Vec<Connected>,
    pub(crate) failures: Vec<McpFailure>,
}

/// Servers still connecting after the fast window; resolves when all are done.
pub(crate) type McpRest = futures::future::BoxFuture<'static, McpConnects>;

impl McpConnects {
    fn add(&mut self, name: String, server: &McpServerConfig, result: StartResult) {
        match result {
            Ok((client, tools)) => self.connected.push(Connected {
                name,
                transport: client.transport_name(),
                client: Arc::new(Mutex::new(client)),
                tools,
            }),
            Err(e) => self.failures.push(failure(&name, server, &e)),
        }
    }

    /// Fold a later batch in (its failures replace nothing: they are new).
    pub(crate) fn absorb(&mut self, later: McpConnects) {
        self.connected.extend(later.connected);
        self.failures.extend(later.failures);
    }

    pub(crate) fn handles(&self) -> Vec<McpServerHandle> {
        self.connected
            .iter()
            .map(|c| McpServerHandle {
                name: c.name.clone(),
                client: Arc::clone(&c.client),
            })
            .collect()
    }

    /// Register every connected server's tools (and resource tools) on
    /// `executor`. Returns how many tools were registered.
    pub(crate) async fn register(&self, executor: &mut ToolExecutor) -> usize {
        let mut count = 0usize;
        for server in &self.connected {
            for tool in &server.tools {
                register_bridge(executor, server, tool);
                count += 1;
            }
        }
        count + crate::mcp_content::register_resource_tools(executor, &self.handles()).await
    }
}

fn register_bridge(executor: &mut ToolExecutor, server: &Connected, tool: &McpTool) {
    let bridge_name = bridge_tool_name(&server.name, &tool.name);
    let description = tool
        .description
        .clone()
        .unwrap_or_else(|| format!("MCP tool from '{}'", server.name));
    let schema = if tool.input_schema.is_null() {
        serde_json::json!({"type": "object", "properties": {}})
    } else {
        tool.input_schema.clone()
    };
    let caller: Arc<dyn McpCaller> = Arc::new(SharedMcpCaller {
        client: Arc::clone(&server.client),
        remote_name: tool.name.clone(),
    });
    executor.register(Box::new(McpToolBridge::new(
        caller,
        bridge_name.clone(),
        description,
        schema,
    )));
    let (server_name, transport) = (&server.name, server.transport);
    info!(server = %server_name, tool = %bridge_name, transport, "Registered MCP tool");
}

fn failure(server_name: &str, server: &McpServerConfig, e: &anyhow::Error) -> McpFailure {
    let detail = match server.resolved_transport() {
        Ok(McpTransportKind::Stdio) => {
            format!("command={}", server.command.as_deref().unwrap_or("?"))
        }
        Ok(_) => format!("url={}", server.url.as_deref().unwrap_or("?")),
        Err(msg) => msg,
    };
    let hint = connect_hint(server_name, &e.to_string());
    warn!(server = %server_name, %detail, error = %e, hint, "Failed to connect MCP server");
    McpFailure {
        server: server_name.to_string(),
        error: e.to_string(),
        hint,
    }
}

type StartResult = anyhow::Result<(McpClient, Vec<McpTool>)>;

/// Start every configured server at once (cold start is the slowest one,
/// not the sum). Returns the servers done within `fast`, and the rest as a
/// future when any are still connecting: one hung server no longer holds
/// back the tools of the servers that answered.
pub(crate) async fn connect_servers(
    config: &Config,
    limit: Duration,
    fast: Duration,
) -> (McpConnects, Option<McpRest>) {
    use futures::StreamExt;
    let data_dir = whycodes_core::paths::data_dir();
    let mut pending: futures::stream::FuturesUnordered<_> = config
        .mcp_servers
        .iter()
        .map(|(server_name, server)| {
            let server_name = server_name.clone();
            let server = server.clone();
            let data_dir = data_dir.clone();
            async move {
                let server = with_oauth_header(&server_name, server, &data_dir).await;
                let result = start_server(&server, limit).await;
                (server_name, server, result)
            }
        })
        .collect();
    let mut done = McpConnects::default();
    let window = tokio::time::sleep(fast);
    tokio::pin!(window);
    loop {
        tokio::select! {
            next = pending.next() => match next {
                Some((name, server, result)) => done.add(name, &server, result),
                None => return (done, None),
            },
            _ = &mut window => break,
        }
    }
    let rest = async move {
        let mut later = McpConnects::default();
        while let Some((name, server, result)) = pending.next().await {
            later.add(name, &server, result);
        }
        later
    };
    (done, Some(Box::pin(rest)))
}

/// Connect `server` and list its tools, giving up after `limit` so a server
/// that never answers cannot hold the others (or a turn waiting on them).
async fn start_server(
    server: &McpServerConfig,
    limit: Duration,
) -> anyhow::Result<(McpClient, Vec<McpTool>)> {
    let start = async {
        let mut client = connect_mcp_server(server).await?;
        let tools = client.list_tools().await?;
        anyhow::Ok((client, tools))
    };
    tokio::time::timeout(limit, start)
        .await
        .map_err(|_| anyhow::anyhow!("no answer within {} s", limit.as_secs_f32()))?
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

/// What to do about a connect error: a 401 means the server wants OAuth;
/// a program that would not start or a server that never answered points
/// at its `command` / `args`.
pub(crate) fn connect_hint(server: &str, error: &str) -> String {
    if error.contains("401") {
        format!("run `whycodes mcp login {server}`")
    } else if error.contains("failed to spawn") {
        "install the program, or put its full path in `command`".into()
    } else if error.contains("no answer within") {
        format!(
            "run its `command` and `args` in a terminal to see why it hangs, \
             or `whycodes mcp remove {server}`"
        )
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
