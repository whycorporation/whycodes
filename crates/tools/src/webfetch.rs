use async_trait::async_trait;
use serde_json::json;

use super::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

pub struct WebFetchTool;

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WebFetchTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "webfetch"
    }

    fn description(&self) -> &str {
        "Fetch content from a URL and return it as text."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch"
                },
                "max_length": {
                    "type": "integer",
                    "description": "Maximum content length to return (default: 5000)"
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, args: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let url = args["url"].as_str().unwrap_or("");
        let max_length = args["max_length"].as_u64().unwrap_or(5000) as usize;

        if url.is_empty() {
            return ToolResult {
                tool_call_id: String::new(),
                content: "URL is required.".to_string(),
                is_error: true,
            };
        }

        match reqwest::get(url).await {
            Ok(response) => {
                let status = response.status();
                match response.text().await {
                    Ok(text) => {
                        // Simple HTML tag stripping
                        let mut clean = String::new();
                        let mut in_tag = false;
                        for c in text.chars() {
                            if c == '<' {
                                in_tag = true;
                            } else if c == '>' {
                                in_tag = false;
                            } else if !in_tag {
                                clean.push(c);
                            }
                        }

                        // Clean up whitespace
                        let lines: Vec<&str> = clean
                            .lines()
                            .map(|l| l.trim())
                            .filter(|l| !l.is_empty())
                            .collect();
                        let result = lines.join("\n");

                        let truncated = if result.len() > max_length {
                            format!("{}...\n[truncated]", &result[..max_length])
                        } else {
                            result
                        };

                        ToolResult {
                            tool_call_id: String::new(),
                            content: format!(
                                "URL: {}\nStatus: {}\n\n{}",
                                url,
                                status.as_u16(),
                                truncated
                            ),
                            is_error: !status.is_success(),
                        }
                    }
                    Err(e) => ToolResult {
                        tool_call_id: String::new(),
                        content: format!("Error reading response: {}", e),
                        is_error: true,
                    },
                }
            }
            Err(e) => ToolResult {
                tool_call_id: String::new(),
                content: format!("Error fetching URL: {}", e),
                is_error: true,
            },
        }
    }
}
