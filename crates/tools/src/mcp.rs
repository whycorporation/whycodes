use std::sync::Arc;

use async_trait::async_trait;

use super::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

/// Trait for calling an MCP tool on a remote server.
///
/// Implementors wrap an MCP client (e.g. `McpClient` from `whycodes-mcp`)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolContext;
    use serde_json::json;
    use std::sync::Mutex;

    /// Records the args it was called with so tests can assert forwarding.
    struct FakeCaller {
        calls: Mutex<Vec<serde_json::Value>>,
        reply: Result<String, String>,
    }

    #[async_trait]
    impl McpCaller for FakeCaller {
        async fn call_mcp_tool(
            &self,
            _tool_name: &str,
            arguments: serde_json::Value,
        ) -> Result<String, String> {
            self.calls.lock().unwrap().push(arguments);
            self.reply.clone()
        }
    }

    fn bridge(reply: Result<String, String>) -> (McpToolBridge, Arc<FakeCaller>) {
        let caller = Arc::new(FakeCaller {
            calls: Mutex::new(Vec::new()),
            reply,
        });
        let b = McpToolBridge::new(
            caller.clone(),
            "server_tool".into(),
            "Does a thing".into(),
            json!({ "type": "object", "properties": { "x": { "type": "string" } } }),
        );
        (b, caller)
    }

    #[tokio::test]
    async fn metadata_passes_through_name_description_schema() {
        let (b, _) = bridge(Ok("ok".into()));
        assert_eq!(b.name(), "server_tool");
        assert_eq!(b.description(), "Does a thing");
        assert_eq!(b.parameters()["properties"]["x"]["type"], "string");
    }

    #[tokio::test]
    async fn execute_forwards_args_and_returns_text() {
        let (b, caller) = bridge(Ok("done".into()));
        let out = b
            .execute(json!({ "x": "v" }), &ToolContext::new("/tmp"))
            .await;
        assert!(!out.is_error);
        assert_eq!(out.content, "done");
        assert_eq!(caller.calls.lock().unwrap().len(), 1);
        assert_eq!(caller.calls.lock().unwrap()[0]["x"], "v");
    }

    #[tokio::test]
    async fn execute_surfaces_caller_error() {
        let (b, _) = bridge(Err("boom".into()));
        let out = b.execute(json!({}), &ToolContext::new("/tmp")).await;
        assert!(out.is_error);
        assert_eq!(out.content, "MCP tool 'server_tool' error: boom");
    }
}
