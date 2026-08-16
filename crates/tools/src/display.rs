use async_trait::async_trait;
use serde_json::json;

use super::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

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
            "diff" => whycode_format::diff::render_diff_unified(content),
            "table" => {
                // Parse content as simple newline-and-comma table
                content.to_string()
            }
            _ => content.to_string(),
        };

        ToolResult {
            tool_call_id: String::new(),
            content: result,
            is_error: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolContext;

    fn ctx() -> ToolContext {
        ToolContext::new("/tmp")
    }

    #[tokio::test]
    async fn metadata_describes_display_tool() {
        let t = DisplayTool::new();
        assert_eq!(t.name(), "display");
        assert!(t.description().contains("syntax highlighting"));
        let params = t.parameters();
        assert_eq!(params["required"][0], "content");
        assert_eq!(params["properties"]["format"]["enum"][0], "code");
    }

    #[tokio::test]
    async fn code_format_highlights_when_language_given() {
        let t = DisplayTool::new();
        let out = t
            .execute(
                json!({ "content": "fn main() {}", "language": "rust", "format": "code" }),
                &ctx(),
            )
            .await;
        assert!(!out.is_error);
        // Highlighted output carries ANSI escape sequences.
        assert!(out.content.contains('\x1b'), "{}", out.content);
    }

    #[tokio::test]
    async fn code_format_passes_plain_text_through_without_language() {
        let t = DisplayTool::new();
        let out = t
            .execute(
                json!({ "content": "hello world", "format": "code" }),
                &ctx(),
            )
            .await;
        assert!(!out.is_error);
        assert_eq!(out.content, "hello world");
    }

    #[tokio::test]
    async fn code_format_detects_language_from_path_like_content() {
        let t = DisplayTool::new();
        // First line looks like a file path → language detected → ANSI codes.
        let out = t
            .execute(
                json!({ "content": "main.rs\nfn main() {}", "format": "code" }),
                &ctx(),
            )
            .await;
        assert!(!out.is_error);
        assert!(out.content.contains('\x1b'), "{}", out.content);
    }

    #[tokio::test]
    async fn diff_format_colors_hunk_lines() {
        let t = DisplayTool::new();
        let out = t
            .execute(
                json!({ "content": "@@ -1,2 +1,2 @@\n-old\n+new", "format": "diff" }),
                &ctx(),
            )
            .await;
        assert!(!out.is_error);
        assert!(
            out.content.contains('\x1b'),
            "diff colored: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn table_and_unknown_formats_pass_through() {
        let t = DisplayTool::new();
        let out = t
            .execute(json!({ "content": "a,b\nc,d", "format": "table" }), &ctx())
            .await;
        assert_eq!(out.content, "a,b\nc,d");

        let out = t
            .execute(json!({ "content": "raw", "format": "bogus" }), &ctx())
            .await;
        assert_eq!(out.content, "raw");
    }

    #[tokio::test]
    async fn missing_content_yields_empty_output() {
        let t = DisplayTool::new();
        let out = t.execute(json!({}), &ctx()).await;
        assert!(!out.is_error);
        assert_eq!(out.content, "");
    }
}
