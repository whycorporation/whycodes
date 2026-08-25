use async_trait::async_trait;
use serde_json::json;

use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

pub struct CodeModeTool;

impl Default for CodeModeTool {
    fn default() -> Self {
        Self::new()
    }
}

impl CodeModeTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for CodeModeTool {
    fn name(&self) -> &str {
        "code_mode"
    }

    fn description(&self) -> &str {
        "Transform or refactor code files"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the code file to transform"
                },
                "instruction": {
                    "type": "string",
                    "description": "Natural language description of the transformation to apply"
                },
                "language": {
                    "type": "string",
                    "description": "Programming language of the file (optional)"
                }
            },
            "required": ["path", "instruction"]
        })
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let path_str = args["path"].as_str().unwrap_or("");
        let instruction = args["instruction"].as_str().unwrap_or("");
        let language = args["language"].as_str().unwrap_or("");

        let full_path = if std::path::Path::new(path_str).is_absolute() {
            path_str.to_string()
        } else {
            std::path::Path::new(&ctx.working_dir)
                .join(path_str)
                .to_string_lossy()
                .to_string()
        };

        let file_content = match std::fs::read_to_string(&full_path) {
            Ok(content) => content,
            Err(e) => {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Error reading file '{}': {}", full_path, e),
                    is_error: true,
                };
            }
        };

        let lang_hint = if language.is_empty() {
            String::new()
        } else {
            format!("Language: {}\n", language)
        };

        let prompt = format!(
            "// Instruction: {}\n// {}\n\nThe file at {} has been loaded. Apply the following transformation: {}. The current content is:\n\n{}",
            instruction,
            lang_hint.trim_end(),
            path_str,
            instruction,
            file_content
        );

        ToolResult {
            tool_call_id: String::new(),
            content: prompt,
            is_error: false,
        }
    }
}
