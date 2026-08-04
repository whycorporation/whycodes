use async_trait::async_trait;
use serde_json::json;

use super::websearch::WebSearchTool;
use crate::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

/// MCP-accessible web search tool.
///
/// Currently wraps the built-in `WebSearchTool` logic but is registered
/// under the name `"mcp_websearch"` so it can be accessed via MCP.
pub struct McpWebSearchTool {
    inner: WebSearchTool,
}

impl McpWebSearchTool {
    pub fn new() -> Self {
        Self {
            inner: WebSearchTool::new(),
        }
    }

    /// The MCP source marker — callers can use this to route the tool
    /// differently (e.g. through an MCP server) when it is marked as MCP.
    pub fn source(&self) -> &str {
        "mcp"
    }
}

#[async_trait]
impl Tool for McpWebSearchTool {
    fn name(&self) -> &str {
        "mcp_websearch"
    }

    fn description(&self) -> &str {
        "Search the web via MCP. Wraps the built-in websearch tool."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                },
                "num_results": {
                    "type": "integer",
                    "description": "Number of results to return (default: 10)"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        self.inner.execute(args, ctx).await
    }
}
