use serde_json::json;

use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

/// List / read / kill background shell jobs. Intercepted by the agent loop.
pub struct BgTool;

impl Default for BgTool {
    fn default() -> Self {
        Self::new()
    }
}

impl BgTool {
    pub fn new() -> Self {
        Self
    }
}
impl Tool for BgTool {
    fn name(&self) -> &str {
        "bg"
    }

    fn description(&self) -> &str {
        "Manage background shell jobs started with bash `background: true` or `schedule`. \
         Actions: list (default), read (needs id), kill (needs id)."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "read", "kill"],
                    "description": "list | read | kill (default list)"
                },
                "id": {
                    "type": "string",
                    "description": "Job id (e.g. bg-1) for read/kill"
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Max characters of output for read (default 8000)"
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
                content: "bg tool was not intercepted by the agent loop.".into(),
                is_error: true,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn background_module_loads() {
        assert!(!module_path!().is_empty());
    }
}
