use async_trait::async_trait;
use serde_json::json;

use super::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

pub struct ExternalDirectoryTool;

impl Default for ExternalDirectoryTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ExternalDirectoryTool {
    pub fn new() -> Self {
        Self
    }

    /// Check if a path is allowed for external access.
    /// Reads the .whycode/external_dirs_allowed file (relative to `working_dir`)
    /// and checks if the given path (or any of its parent directories) is listed.
    fn is_path_allowed(path: &str, working_dir: &str) -> bool {
        let allowed_file = std::path::Path::new(working_dir)
            .join(".whycode")
            .join("external_dirs_allowed");

        let allowed_content = match std::fs::read_to_string(&allowed_file) {
            Ok(content) => content,
            Err(_) => return false,
        };

        let canon_path = match std::fs::canonicalize(path) {
            Ok(p) => p,
            Err(_) => return false,
        };

        for line in allowed_content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let allowed = match std::fs::canonicalize(line) {
                Ok(p) => p,
                Err(_) => continue,
            };
            // Check if the requested path is equal to or a child of an allowed directory
            if canon_path == allowed || canon_path.starts_with(&allowed) {
                return true;
            }
        }

        false
    }
}

#[async_trait]
impl Tool for ExternalDirectoryTool {
    fn name(&self) -> &str {
        "external_directory"
    }

    fn description(&self) -> &str {
        "Access files outside the project directory (requires permission)"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the external file or directory"
                },
                "action": {
                    "type": "string",
                    "description": "Action to perform: 'read' to read a file, 'list' to list a directory",
                    "enum": ["read", "list"]
                }
            },
            "required": ["path", "action"]
        })
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let path_str = args["path"].as_str().unwrap_or("");
        let action = args["action"].as_str().unwrap_or("read");

        let full_path = if std::path::Path::new(path_str).is_absolute() {
            path_str.to_string()
        } else {
            std::path::Path::new(&ctx.working_dir)
                .join(path_str)
                .to_string_lossy()
                .to_string()
        };

        // Security check: verify the path is allowed
        if !Self::is_path_allowed(&full_path, &ctx.working_dir) {
            return ToolResult {
                tool_call_id: String::new(),
                content: format!(
                    "Access denied: '{}' is not in the allowed external directories list. \
                     Add the directory to .whycode/external_dirs_allowed to grant access.",
                    path_str
                ),
                is_error: true,
            };
        }

        match action {
            "list" => {
                let entries = match std::fs::read_dir(&full_path) {
                    Ok(entries) => entries,
                    Err(e) => {
                        return ToolResult {
                            tool_call_id: String::new(),
                            content: format!("Error listing directory '{}': {}", full_path, e),
                            is_error: true,
                        };
                    }
                };

                let mut output = String::new();
                for entry in entries {
                    match entry {
                        Ok(e) => {
                            let meta = match e.metadata() {
                                Ok(m) => m,
                                Err(_) => continue,
                            };
                            let name = e.file_name().to_string_lossy().to_string();
                            let file_type = if meta.is_dir() {
                                "d"
                            } else if meta.is_symlink() {
                                "l"
                            } else {
                                "-"
                            };
                            let size = meta.len();
                            output.push_str(&format!("{:<10} {:>10} {}\n", file_type, size, name));
                        }
                        Err(_) => continue,
                    }
                }

                ToolResult {
                    tool_call_id: String::new(),
                    content: if output.is_empty() {
                        format!("Directory '{}' is empty", full_path)
                    } else {
                        output
                    },
                    is_error: false,
                }
            }
            "read" => match std::fs::read_to_string(&full_path) {
                Ok(content) => ToolResult {
                    tool_call_id: String::new(),
                    content,
                    is_error: false,
                },
                Err(e) => ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Error reading file '{}': {}", full_path, e),
                    is_error: true,
                },
            },
            _ => ToolResult {
                tool_call_id: String::new(),
                content: format!("Unknown action '{}'. Use 'read' or 'list'.", action),
                is_error: true,
            },
        }
    }
}
