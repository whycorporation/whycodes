use serde_json::json;

use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

pub struct TruncateTool;

impl Default for TruncateTool {
    fn default() -> Self {
        Self::new()
    }
}

impl TruncateTool {
    pub fn new() -> Self {
        Self
    }
}
impl Tool for TruncateTool {
    fn name(&self) -> &str {
        "truncate"
    }

    fn description(&self) -> &str {
        "Truncate long output to fit context window"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "The text to truncate"
                },
                "max_lines": {
                    "type": "integer",
                    "description": "Maximum number of lines to keep (default: 200)"
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Maximum number of characters to keep (default: 8000)"
                }
            },
            "required": ["text"]
        })
    }

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        _ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            let text = args["text"].as_str().unwrap_or("");
            let max_lines = args["max_lines"].as_u64().unwrap_or(200) as usize;
            let max_chars = args["max_chars"].as_u64().unwrap_or(8000) as usize;

            let result = whycodes_format::truncate::truncate(text, max_lines, max_chars);

            ToolResult {
                tool_call_id: String::new(),
                content: result,
                is_error: false,
            }
        })
    }
}

#[cfg(test)]
#[path = "truncate_tests.rs"]
mod tests;
