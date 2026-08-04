use async_trait::async_trait;
use serde_json::json;
use std::process::Command;

use crate::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

/// Tool that lists a directory and truncates the output to fit context.
pub struct TruncationDirTool;

impl TruncationDirTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for TruncationDirTool {
    fn name(&self) -> &str {
        "truncation_dir"
    }

    fn description(&self) -> &str {
        "Get a directory listing, truncated to fit within context limits."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory path to list (default: current working directory)"
                },
                "max_entries": {
                    "type": "integer",
                    "description": "Maximum number of entries to return (default: 50)"
                }
            },
            "required": []
        })
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let path = args["path"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| ctx.working_dir.clone());
        let max_entries = args["max_entries"].as_u64().unwrap_or(50) as usize;

        let output = match Command::new("ls").arg("-la").arg(&path).output() {
            Ok(o) => o,
            Err(e) => {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Error running ls: {e}"),
                    is_error: true,
                };
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return ToolResult {
                tool_call_id: String::new(),
                content: format!("ls failed: {stderr}"),
                is_error: true,
            };
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().collect();
        let total = lines.len();

        // The first line is typically the "total N" line; keep it.
        // Then take up to max_entries more lines.
        let header_idx = if lines.first().map_or(false, |l| l.starts_with("total ")) {
            1
        } else {
            0
        };

        let content_lines: Vec<&str> = lines[header_idx..]
            .iter()
            .take(max_entries)
            .copied()
            .collect();

        let mut result = String::new();

        // Include the "total" line if present
        if header_idx > 0 {
            result.push_str(lines[0]);
            result.push('\n');
        }

        for line in &content_lines {
            result.push_str(line);
            result.push('\n');
        }

        let shown = content_lines.len();
        if shown < total.saturating_sub(header_idx) {
            result.push_str(&format!(
                "\n[... {} entries truncated from {} total]",
                total.saturating_sub(header_idx + shown),
                total.saturating_sub(header_idx)
            ));
        }

        ToolResult {
            tool_call_id: String::new(),
            content: result,
            is_error: false,
        }
    }
}
