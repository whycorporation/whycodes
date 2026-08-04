use async_trait::async_trait;
use serde_json::json;
use std::io::Write;
use std::process::Command;

use super::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

pub struct ApplyPatchTool;

impl Default for ApplyPatchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ApplyPatchTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ApplyPatchTool {
    fn name(&self) -> &str {
        "apply_patch"
    }

    fn description(&self) -> &str {
        "Apply a unified diff patch to a file"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to patch"
                },
                "patch_content": {
                    "type": "string",
                    "description": "The unified diff patch content to apply"
                }
            },
            "required": ["path", "patch_content"]
        })
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let path_str = args["path"].as_str().unwrap_or("");
        let patch_content = args["patch_content"].as_str().unwrap_or("");

        if path_str.is_empty() {
            return ToolResult {
                tool_call_id: String::new(),
                content: "Error: 'path' parameter is required.".to_string(),
                is_error: true,
            };
        }

        let full_path = if std::path::Path::new(path_str).is_absolute() {
            path_str.to_string()
        } else {
            std::path::Path::new(&ctx.working_dir)
                .join(path_str)
                .to_string_lossy()
                .to_string()
        };

        // Write patch content to a temporary file
        let temp_dir = match std::env::temp_dir().to_str() {
            Some(d) => d.to_string(),
            None => "/tmp".to_string(),
        };
        let temp_file = format!("{}/whycode_patch_{}.diff", temp_dir, std::process::id());

        let mut file = match std::fs::File::create(&temp_file) {
            Ok(f) => f,
            Err(e) => {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Error creating temp patch file: {}", e),
                    is_error: true,
                };
            }
        };

        if let Err(e) = file.write_all(patch_content.as_bytes()) {
            return ToolResult {
                tool_call_id: String::new(),
                content: format!("Error writing patch content to temp file: {}", e),
                is_error: true,
            };
        }

        // Ensure temp file is flushed
        drop(file);

        let stdin = match std::fs::File::open(&temp_file) {
            Ok(f) => f,
            Err(e) => {
                // Best-effort cleanup; ignore failure on the error path.
                drop(std::fs::remove_file(&temp_file));
                return ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Error reopening temp patch file: {}", e),
                    is_error: true,
                };
            }
        };

        // Run patch command
        let output = Command::new("patch")
            .arg("-u")
            .arg(&full_path)
            .stdin(stdin)
            .output();

        // Clean up temp file
        let _ = std::fs::remove_file(&temp_file);

        match output {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let mut result = String::new();

                if !stdout.is_empty() {
                    result.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !result.is_empty() {
                        result.push('\n');
                    }
                    result.push_str(&stderr);
                }

                if output.status.success() {
                    ToolResult {
                        tool_call_id: String::new(),
                        content: if result.is_empty() {
                            format!("Patch applied successfully to '{}'", full_path)
                        } else {
                            format!("Patch applied successfully to '{}'\n{}", full_path, result)
                        },
                        is_error: false,
                    }
                } else {
                    ToolResult {
                        tool_call_id: String::new(),
                        content: format!(
                            "Patch failed on '{}': {}",
                            full_path,
                            if result.is_empty() {
                                "unknown error"
                            } else {
                                &result
                            }
                        ),
                        is_error: true,
                    }
                }
            }
            Err(e) => ToolResult {
                tool_call_id: String::new(),
                content: format!("Error running patch command: {}", e),
                is_error: true,
            },
        }
    }
}
