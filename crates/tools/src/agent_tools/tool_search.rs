use serde_json::json;

use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

/// Discover deferred tools (outside the core profile) and activate them for
/// the rest of the session. Intercepted by the agent loop.
pub struct ToolSearchTool;

impl Default for ToolSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolSearchTool {
    pub fn new() -> Self {
        Self
    }
}
impl Tool for ToolSearchTool {
    fn name(&self) -> &str {
        "tool_search"
    }

    fn description(&self) -> &str {
        "Discover tools not in the core profile (apply_patch, memory, schedule, swarm, \
         web, browser, github, lsp, git_*, plan, …) and activate them for this session. \
         Actions: search (keywords), select (name or comma-separated names), list \
         (activated + deferred catalogue)."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["search", "select", "list"],
                    "description": "search | select | list (default search)"
                },
                "query": {
                    "type": "string",
                    "description": "Keywords for search, or tool name(s) for select (comma-separated)"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Max search hits (default 8)"
                }
            }
        })
    }

    fn execute<'a>(
        &'a self,
        _args: serde_json::Value,
        _ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            ToolResult {
                tool_call_id: String::new(),
                content: "tool_search was not intercepted by the agent loop.".into(),
                is_error: true,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn tool_search_module_loads() {
        assert!(!module_path!().is_empty());
    }
}
