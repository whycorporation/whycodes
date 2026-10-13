//! MCP prompts and resources: resource tools for the model, prompt
//! rendering for `/mcp__<server>__<prompt>` slash commands.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Value, json};
use whycodes_mcp::McpResourceContent;
use whycodes_tools::executor::ToolExecutor;
use whycodes_tools::mcp::{McpCaller, McpToolBridge};

use crate::mcp_load::McpServerHandle;

/// Tool that lists resources across servers.
pub const LIST_RESOURCES_TOOL: &str = "mcp_list_resources";
/// Tool that reads one resource.
pub const READ_RESOURCE_TOOL: &str = "mcp_read_resource";

/// Register the resource tools when at least one server offers resources.
/// Returns the number of tools registered (0 or 2).
pub(crate) async fn register_resource_tools(
    executor: &mut ToolExecutor,
    servers: &[McpServerHandle],
) -> usize {
    let mut with_resources = Vec::new();
    for server in servers {
        if server.client.lock().await.supports_resources() {
            with_resources.push(server.clone());
        }
    }
    if with_resources.is_empty() {
        return 0;
    }
    let names: Vec<&str> = with_resources.iter().map(|s| s.name.as_str()).collect();
    let server_list = names.join(", ");
    executor.register(Box::new(McpToolBridge::new(
        Arc::new(ResourceCaller {
            servers: with_resources.clone(),
            read: false,
        }),
        LIST_RESOURCES_TOOL.to_string(),
        format!(
            "List resources (files, docs, records) offered by MCP servers: {server_list}. \
             Read one with {READ_RESOURCE_TOOL}."
        ),
        json!({
            "type": "object",
            "properties": {
                "server": {"type": "string", "description": "Only this server"}
            }
        }),
    )));
    executor.register(Box::new(McpToolBridge::new(
        Arc::new(ResourceCaller {
            servers: with_resources,
            read: true,
        }),
        READ_RESOURCE_TOOL.to_string(),
        format!("Read an MCP resource by server and URI (servers: {server_list})."),
        json!({
            "type": "object",
            "properties": {
                "server": {"type": "string"},
                "uri": {"type": "string"}
            },
            "required": ["server", "uri"]
        }),
    )));
    2
}

struct ResourceCaller {
    servers: Vec<McpServerHandle>,
    read: bool,
}

impl McpCaller for ResourceCaller {
    fn call_mcp_tool<'a>(
        &'a self,
        _tool_name: &'a str,
        arguments: Value,
    ) -> futures::future::BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            if self.read {
                read_resource(&self.servers, &arguments).await
            } else {
                Ok(list_resources(&self.servers, &arguments).await)
            }
        })
    }
}

async fn list_resources(servers: &[McpServerHandle], args: &Value) -> String {
    let only = args.get("server").and_then(Value::as_str);
    let mut lines = Vec::new();
    for server in servers.iter().filter(|s| only.is_none_or(|o| o == s.name)) {
        match server.client.lock().await.list_resources().await {
            Ok(resources) => {
                for r in resources {
                    let desc = r.description.map(|d| format!(" — {d}")).unwrap_or_default();
                    lines.push(format!("[{}] {} <{}>{desc}", server.name, r.name, r.uri));
                }
            }
            Err(e) => lines.push(format!("[{}] error: {e}", server.name)),
        }
    }
    if lines.is_empty() {
        "No MCP resources.".to_string()
    } else {
        lines.join("\n")
    }
}

async fn read_resource(servers: &[McpServerHandle], args: &Value) -> Result<String, String> {
    let name = args.get("server").and_then(Value::as_str).unwrap_or("");
    let uri = args.get("uri").and_then(Value::as_str).unwrap_or("");
    let server = servers.iter().find(|s| s.name == name).ok_or_else(|| {
        format!(
            "unknown MCP server '{name}' (has resources: {})",
            names(servers)
        )
    })?;
    let contents = server
        .client
        .lock()
        .await
        .read_resource(uri)
        .await
        .map_err(|e| e.to_string())?;
    Ok(contents
        .iter()
        .map(content_text)
        .collect::<Vec<_>>()
        .join("\n\n"))
}

fn names(servers: &[McpServerHandle]) -> String {
    servers
        .iter()
        .map(|s| s.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn content_text(c: &McpResourceContent) -> String {
    match (&c.text, &c.blob) {
        (Some(text), _) => text.clone(),
        (None, Some(blob)) => format!(
            "[binary {} at {}, {} base64 chars]",
            c.mime_type.as_deref().unwrap_or("data"),
            c.uri,
            blob.len()
        ),
        (None, None) => format!("[empty resource {}]", c.uri),
    }
}

/// Render `/mcp__<server>__<prompt> args`. `None` when `command` is not an
/// MCP prompt of a connected server; positional `args` fill the prompt's
/// arguments in order, the last one taking the rest of the line.
pub async fn render_mcp_prompt(
    servers: &[McpServerHandle],
    command: &str,
    args: &str,
) -> Option<Result<String, String>> {
    let (server_name, prompt) = command.strip_prefix("mcp__")?.split_once("__")?;
    let server = servers.iter().find(|s| s.name == server_name)?;
    let mut client = server.client.lock().await;
    let prompts = match client.list_prompts().await {
        Ok(p) => p,
        Err(e) => return Some(Err(e.to_string())),
    };
    let Some(def) = prompts.iter().find(|p| p.name == prompt) else {
        return Some(Err(format!("'{server_name}' has no prompt '{prompt}'")));
    };
    let declared = def.arguments.clone().unwrap_or_default();
    let names: Vec<&str> = declared.iter().map(|a| a.name.as_str()).collect();
    let arguments = positional_arguments(&names, args);
    if let Some(missing) = declared
        .iter()
        .find(|a| a.required == Some(true) && !arguments.contains_key(&a.name))
    {
        return Some(Err(format!(
            "missing argument '{}' — usage: /{command} {}",
            missing.name,
            usage(&names)
        )));
    }
    Some(
        client
            .get_prompt(prompt, &arguments)
            .await
            .map(|r| r.text())
            .map_err(|e| e.to_string()),
    )
}

fn positional_arguments(names: &[&str], args: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut rest = args.trim();
    for (i, name) in names.iter().enumerate() {
        if rest.is_empty() {
            break;
        }
        let value = if i + 1 == names.len() {
            std::mem::take(&mut rest)
        } else {
            let (head, tail) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            rest = tail.trim_start();
            head
        };
        out.insert((*name).to_string(), value.to_string());
    }
    out
}

fn usage(names: &[&str]) -> String {
    names
        .iter()
        .map(|n| format!("<{n}>"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `/mcp` listing: each connected server with its prompts as commands.
pub async fn describe_mcp_servers(servers: &[McpServerHandle]) -> String {
    if servers.is_empty() {
        return "No MCP servers connected. Add one under [mcp] in config.".to_string();
    }
    let mut out = Vec::new();
    for server in servers {
        let mut client = server.client.lock().await;
        let resources = if client.supports_resources() {
            format!(", resources via {LIST_RESOURCES_TOOL}")
        } else {
            String::new()
        };
        out.push(format!(
            "{} ({}){resources}",
            server.name,
            client.transport_name()
        ));
        if !client.supports_prompts() {
            continue;
        }
        match client.list_prompts().await {
            Ok(prompts) => {
                for p in prompts {
                    let names: Vec<String> = p
                        .arguments
                        .unwrap_or_default()
                        .into_iter()
                        .map(|a| a.name)
                        .collect();
                    let names: Vec<&str> = names.iter().map(String::as_str).collect();
                    let desc = p.description.map(|d| format!(" — {d}")).unwrap_or_default();
                    let args = usage(&names);
                    let sep = if args.is_empty() { "" } else { " " };
                    out.push(format!(
                        "  /mcp__{}__{}{sep}{args}{desc}",
                        server.name, p.name
                    ));
                }
            }
            Err(e) => out.push(format!("  prompts unavailable: {e}")),
        }
    }
    out.join("\n")
}

#[cfg(test)]
#[path = "mcp_content_tests.rs"]
mod tests;
