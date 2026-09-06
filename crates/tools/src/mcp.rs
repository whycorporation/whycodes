use std::sync::Arc;

use super::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

/// Trait for calling an MCP tool on a remote server.
///
/// Implementors wrap an MCP client (e.g. `McpClient` from `whycodes-mcp`)
/// and delegate `tools/call` requests.
pub trait McpCaller: Send + Sync {
    /// Call the named MCP tool with the given arguments, returning the
    /// text content of the result.
    fn call_mcp_tool<'a>(
        &'a self,
        tool_name: &'a str,
        arguments: serde_json::Value,
    ) -> futures::future::BoxFuture<'a, Result<String, String>>;
}

/// A `Tool` that bridges an MCP server's tool into the whycodes tool system.
///
/// Each instance wraps one MCP tool advertised by a server.  It is not
/// auto-registered — callers create one `McpToolBridge` per MCP tool they
/// wish to expose and register it with `ToolExecutor::register`.
pub struct McpToolBridge {
    caller: Arc<dyn McpCaller>,
    tool_name: String,
    description_text: String,
    params_schema: serde_json::Value,
}

impl McpToolBridge {
    /// Create a new bridge for a single MCP tool.
    ///
    /// * `caller` — something that can forward `call_mcp_tool` requests.
    /// * `mcp_tool` — an MCP tool definition (e.g. `whycodes_mcp::McpTool`),
    ///   from which the bridge derives its name, description, and parameter
    ///   schema.
    pub fn new(
        caller: Arc<dyn McpCaller>,
        tool_name: String,
        description: String,
        input_schema: serde_json::Value,
    ) -> Self {
        Self {
            caller,
            tool_name,
            description_text: description,
            params_schema: input_schema,
        }
    }
}
impl Tool for McpToolBridge {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        &self.description_text
    }

    fn parameters(&self) -> serde_json::Value {
        self.params_schema.clone()
    }

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        _ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            match self.caller.call_mcp_tool(&self.tool_name, args).await {
                Ok(text) => ToolResult {
                    tool_call_id: String::new(),
                    content: text,
                    is_error: false,
                },
                Err(e) => ToolResult {
                    tool_call_id: String::new(),
                    content: format!("MCP tool '{}' error: {}", self.tool_name, e),
                    is_error: true,
                },
            }
        })
    }
}

#[cfg(test)]
#[path = "mcp_tests.rs"]
mod tests;
