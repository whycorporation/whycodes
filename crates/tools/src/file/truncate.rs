use async_trait::async_trait;
use serde_json::json;

use crate::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

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

#[async_trait]
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

    async fn execute(&self, args: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let text = args["text"].as_str().unwrap_or("");
        let max_lines = args["max_lines"].as_u64().unwrap_or(200) as usize;
        let max_chars = args["max_chars"].as_u64().unwrap_or(8000) as usize;

        let result = whycode_format::truncate::truncate(text, max_lines, max_chars);

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
    async fn metadata_describes_truncate_tool() {
        let t = TruncateTool::new();
        assert_eq!(t.name(), "truncate");
        assert!(t.description().contains("Truncate"));
        let params = t.parameters();
        assert_eq!(params["required"][0], "text");
    }

    #[tokio::test]
    async fn short_text_passes_through() {
        let out = TruncateTool::new()
            .execute(json!({ "text": "hello" }), &ctx())
            .await;
        assert!(!out.is_error);
        assert_eq!(out.content, "hello");
    }

    #[tokio::test]
    async fn max_lines_caps_output() {
        let text = (0..10)
            .map(|i| format!("L{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = TruncateTool::new()
            .execute(
                json!({ "text": text, "max_lines": 3, "max_chars": 8000 }),
                &ctx(),
            )
            .await;
        assert!(!out.is_error);
        assert!(out.content.contains("L0"), "{}", out.content);
        assert!(
            !out.content.contains("L9"),
            "later lines dropped: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn missing_text_yields_empty() {
        let out = TruncateTool::new().execute(json!({}), &ctx()).await;
        assert!(!out.is_error);
        assert_eq!(out.content, "");
    }
}
