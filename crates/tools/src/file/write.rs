use async_trait::async_trait;
use serde_json::json;

use crate::file::paths::display_path;
use crate::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;
use whycode_format::diff::format_write_preview;

pub struct WriteTool;

impl Default for WriteTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WriteTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "Write content to a file, creating it if it doesn't exist."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let path_str = args["path"].as_str().unwrap_or("");
        let content = args["content"].as_str().unwrap_or("");

        let full_path = if std::path::Path::new(path_str).is_absolute() {
            path_str.to_string()
        } else {
            std::path::Path::new(&ctx.working_dir)
                .join(path_str)
                .to_string_lossy()
                .to_string()
        };

        // Create parent directories if needed
        if let Some(parent) = std::path::Path::new(&full_path).parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            return ToolResult {
                tool_call_id: String::new(),
                content: format!("Error creating directory: {}", e),
                is_error: true,
            };
        }

        let shown = display_path(std::path::Path::new(&full_path), &ctx.working_dir);

        match std::fs::write(&full_path, content) {
            Ok(_) => ToolResult {
                tool_call_id: String::new(),
                // Grok-like: +lines preview so the TUI can paint add colours
                // (and syntax-highlight when the path has a known extension).
                content: format_write_preview(&shown, content),
                is_error: false,
            },
            Err(e) => ToolResult {
                tool_call_id: String::new(),
                content: format!("Error writing file '{}': {}", full_path, e),
                is_error: true,
            },
        }
    }
}
