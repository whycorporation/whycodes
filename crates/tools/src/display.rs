use async_trait::async_trait;
use serde_json::json;

use super::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

pub struct DisplayTool;

impl DisplayTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for DisplayTool {
    fn name(&self) -> &str {
        "display"
    }

    fn description(&self) -> &str {
        "Display formatted output with syntax highlighting"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "The content to format and display"
                },
                "language": {
                    "type": "string",
                    "description": "Programming language for syntax highlighting (e.g. 'rust', 'python', 'javascript'). Leave empty for auto-detection or plain text."
                },
                "format": {
                    "type": "string",
                    "enum": ["code", "diff", "table", "text"],
                    "description": "Output format: 'code' for syntax-highlighted code, 'diff' for colorized unified diff, 'table' for plain text table, 'text' for no formatting."
                }
            },
            "required": ["content"]
        })
    }

    async fn execute(&self, args: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let content = args["content"].as_str().unwrap_or("");
        let language = args["language"].as_str().unwrap_or("");
        let format = args["format"].as_str().unwrap_or("code");

        let result = match format {
            "code" => {
                // If language is empty and content looks like a path, try to detect
                let lang = if language.is_empty() {
                    whycode_format::highlight::detect_language(content.lines().next().unwrap_or(""))
                        .unwrap_or("")
                } else {
                    language
                };
                if lang.is_empty() {
                    // No language — return as plain text
                    content.to_string()
                } else {
                    whycode_format::highlight::highlight_code(content, lang)
                }
            }
            "diff" => {
                whycode_format::diff::render_diff_unified(content)
            }
            "table" => {
                // Parse content as simple newline-and-comma table
                content.to_string()
            }
            _ => {
                content.to_string()
            }
        };

        ToolResult {
            tool_call_id: String::new(),
            content: result,
            is_error: false,
        }
    }
}
