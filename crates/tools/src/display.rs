use serde_json::json;

use super::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

pub struct DisplayTool;

impl Default for DisplayTool {
    fn default() -> Self {
        Self::new()
    }
}

impl DisplayTool {
    pub fn new() -> Self {
        Self
    }
}
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

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        _ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            let content = args["content"].as_str().unwrap_or("");
            let language = args["language"].as_str().unwrap_or("");
            let format = args["format"].as_str().unwrap_or("code");
            ToolResult {
                tool_call_id: String::new(),
                content: format_display_content(content, language, format),
                is_error: false,
            }
        })
    }
}

fn format_display_content(content: &str, language: &str, format: &str) -> String {
    match format {
        "code" => {
            let lang = if language.is_empty() {
                whycodes_format::highlight::detect_language(content.lines().next().unwrap_or(""))
                    .unwrap_or("")
            } else {
                language
            };
            if lang.is_empty() {
                content.to_string()
            } else {
                whycodes_format::highlight::highlight_code(content, lang)
            }
        }
        "diff" => whycodes_format::diff::render_diff_unified(content),
        _ => content.to_string(),
    }
}

#[cfg(test)]
#[path = "display_tests.rs"]
mod tests;
