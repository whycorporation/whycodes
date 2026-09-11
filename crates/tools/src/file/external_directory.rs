use serde_json::json;

use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

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
    /// Reads the .whycodes/external_dirs_allowed file (relative to `working_dir`)
    /// and checks if the given path (or any of its parent directories) is listed.
    fn is_path_allowed(path: &str, working_dir: &str) -> bool {
        let allowed_file = whycodes_core::project_dir(std::path::Path::new(working_dir))
            .join("external_dirs_allowed");

        let allowed_content = match std::fs::read_to_string(&allowed_file) {
            Ok(content) => content,
            Err(e) => {
                tracing::debug!(path = %allowed_file.display(), error = %e, "external dirs allowlist unreadable; denying");
                return false;
            }
        };

        let canon_path = match std::fs::canonicalize(path) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(path = %path, error = %e, "external path canonicalize failed; denying");
                return false;
            }
        };

        for line in allowed_content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let allowed = match std::fs::canonicalize(line) {
                Ok(p) => p,
                Err(e) => {
                    tracing::debug!(line = %line, error = %e, "allowlist entry canonicalize failed; skipping");
                    continue;
                }
            };
            // Check if the requested path is equal to or a child of an allowed directory
            if canon_path == allowed || canon_path.starts_with(&allowed) {
                return true;
            }
        }

        false
    }
}
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

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
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
                     Add the directory to .whycodes/external_dirs_allowed to grant access.",
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
                        if let Some(line) = format_dir_entry(entry) {
                            output.push_str(&line);
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
        })
    }
}

fn skip_dir_entry(err: &str, msg: &'static str) {
    tracing::debug!(error = %err, "{msg}");
}

fn skip_metadata_failed(err: &str) {
    skip_dir_entry(err, "entry metadata failed; skipping");
}

fn skip_entry_failed(err: &str) {
    skip_dir_entry(err, "dir entry read failed; skipping");
}

fn format_dir_entry(entry: std::io::Result<std::fs::DirEntry>) -> Option<String> {
    match entry {
        Ok(e) => format_dir_meta(e.metadata(), &e.file_name().to_string_lossy()),
        Err(err) => {
            skip_entry_failed(&err.to_string());
            None
        }
    }
}

fn format_dir_meta(meta: std::io::Result<std::fs::Metadata>, name: &str) -> Option<String> {
    match meta {
        Ok(meta) => Some(format!(
            "{:<10} {:>10} {}\n",
            entry_type_label(&meta),
            meta.len(),
            name
        )),
        Err(err) => {
            skip_metadata_failed(&err.to_string());
            None
        }
    }
}

fn entry_type_label(meta: &std::fs::Metadata) -> &'static str {
    entry_type_from_flags(meta.is_dir(), meta.file_type().is_symlink())
}

fn entry_type_from_flags(is_dir: bool, is_symlink: bool) -> &'static str {
    if is_dir {
        "d"
    } else if is_symlink {
        "l"
    } else {
        "-"
    }
}

#[cfg(test)]
#[path = "external_directory_tests.rs"]
mod tests;
