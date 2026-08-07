use async_trait::async_trait;
use serde_json::json;

use crate::file::paths::display_path;
use crate::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;
use whycode_format::diff::{first_line_number, format_edit_preview_at};

pub struct EditTool;

impl Default for EditTool {
    fn default() -> Self {
        Self::new()
    }
}

impl EditTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        "Make targeted edits to a file by finding and replacing exact text."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to edit"
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact text to find"
                },
                "new_string": {
                    "type": "string",
                    "description": "The replacement text"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace all occurrences (default: false)"
                }
            },
            "required": ["path", "old_string", "new_string"]
        })
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let path_str = args["path"].as_str().unwrap_or("");
        let old_string = args["old_string"].as_str().unwrap_or("");
        let new_string = args["new_string"].as_str().unwrap_or("");
        let replace_all = args["replace_all"].as_bool().unwrap_or(false);

        let full_path = if std::path::Path::new(path_str).is_absolute() {
            path_str.to_string()
        } else {
            std::path::Path::new(&ctx.working_dir)
                .join(path_str)
                .to_string_lossy()
                .to_string()
        };

        if let Err(msg) = ctx.check_file_write(std::path::Path::new(&full_path)) {
            return ToolResult {
                tool_call_id: String::new(),
                content: msg,
                is_error: true,
            };
        }

        let shown = display_path(std::path::Path::new(&full_path), &ctx.working_dir);

        match std::fs::read_to_string(&full_path) {
            Ok(original) => {
                if replace_all {
                    let count = original.matches(old_string).count();
                    if count == 0 {
                        return ToolResult {
                            tool_call_id: String::new(),
                            content: "Could not find the specified text in the file.".to_string(),
                            is_error: true,
                        };
                    }
                    let modified = original.replace(old_string, new_string);
                    let start = first_line_number(&original, old_string);
                    match std::fs::write(&full_path, &modified) {
                        Ok(_) => ToolResult {
                            tool_call_id: String::new(),
                            content: format_edit_preview_at(
                                &shown, old_string, new_string, count, start,
                            ),
                            is_error: false,
                        },
                        Err(e) => ToolResult {
                            tool_call_id: String::new(),
                            content: format!("Error writing file: {}", e),
                            is_error: true,
                        },
                    }
                } else {
                    let count = original.matches(old_string).count();
                    if count == 0 {
                        ToolResult {
                            tool_call_id: String::new(),
                            content: "Could not find the specified text in the file.".to_string(),
                            is_error: true,
                        }
                    } else if count > 1 {
                        ToolResult {
                            tool_call_id: String::new(),
                            content: format!(
                                "Found {} occurrences of the search text. Use replace_all=true or provide a more specific match.",
                                count
                            ),
                            is_error: true,
                        }
                    } else {
                        let modified = original.replacen(old_string, new_string, 1);
                        let start = first_line_number(&original, old_string);
                        match std::fs::write(&full_path, &modified) {
                            Ok(_) => ToolResult {
                                tool_call_id: String::new(),
                                content: format_edit_preview_at(
                                    &shown, old_string, new_string, 1, start,
                                ),
                                is_error: false,
                            },
                            Err(e) => ToolResult {
                                tool_call_id: String::new(),
                                content: format!("Error writing file: {}", e),
                                is_error: true,
                            },
                        }
                    }
                }
            }
            Err(e) => ToolResult {
                tool_call_id: String::new(),
                content: format!("Error reading file: {}", e),
                is_error: true,
            },
        }
    }
}
