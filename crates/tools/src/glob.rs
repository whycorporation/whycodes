use async_trait::async_trait;
use serde_json::json;

use super::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

pub struct GlobTool;

impl Default for GlobTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Find files matching a glob pattern. Fast, file-based search."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match (e.g., '*.rs', 'src/**/*.ts')"
                },
                "path": {
                    "type": "string",
                    "description": "Root directory for the search (default: current directory)"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let pattern_str = args["pattern"].as_str().unwrap_or("");
        let root = args["path"]
            .as_str()
            .unwrap_or(&ctx.working_dir)
            .to_string();

        let full_pattern = format!("{}/{}", root, pattern_str);

        match glob::glob(&full_pattern) {
            Ok(paths) => {
                let mut results: Vec<String> = Vec::new();
                for entry in paths {
                    match entry {
                        Ok(path) => {
                            if let Ok(relative) = path.strip_prefix(&root) {
                                results.push(relative.to_string_lossy().to_string());
                            } else {
                                results.push(path.to_string_lossy().to_string());
                            }
                        }
                        Err(e) => {
                            results.push(format!("Error: {}", e));
                        }
                    }
                }

                // Sort results
                results.sort();

                // Limit to 200 results
                let total = results.len();
                results.truncate(200);

                let mut output = results.join("\n");
                if total > 200 {
                    output.push_str(&format!("\n... and {} more", total - 200));
                }

                ToolResult {
                    tool_call_id: String::new(),
                    content: if output.is_empty() {
                        "No files matched the pattern.".to_string()
                    } else {
                        format!("Found {} files:\n{}", total, output)
                    },
                    is_error: false,
                }
            }
            Err(e) => ToolResult {
                tool_call_id: String::new(),
                content: format!("Invalid glob pattern: {}", e),
                is_error: true,
            },
        }
    }
}
