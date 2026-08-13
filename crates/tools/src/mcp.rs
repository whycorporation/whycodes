use std::sync::Arc;

use async_trait::async_trait;

use super::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

/// Trait for calling an MCP tool on a remote server.
///
/// Implementors wrap an MCP client (e.g. `McpClient` from `whycode-mcp`)
/// and delegate `tools/call` requests.
#[async_trait]
pub trait McpCaller: Send + Sync {
    /// Call the named MCP tool with the given arguments, returning the
    /// text content of the result.
    async fn call_mcp_tool(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<String, String>;
}

/// A `Tool` that bridges an MCP server's tool into the whycode tool system.
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
    /// * `mcp_tool` — an MCP tool definition (e.g. `whycode_mcp::McpTool`),
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

#[async_trait]
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

    async fn execute(&self, args: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
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
    }
}
