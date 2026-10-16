use serde_json::json;

use crate::file::paths::display_path;
use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;
use whycodes_format::diff::format_write_preview;

pub struct WriteTool;

impl Default for WriteTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WriteTool {
    pub fn new() -> Self {
        Self
    }
}
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "Write content to a file, creating it if it doesn't exist."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                }
            },
            "required": ["path", "content"]
        })
    }

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            let path_str = args["path"].as_str().unwrap_or("").to_string();
            let content = args["content"].as_str().unwrap_or("").to_string();

            let full_path = if std::path::Path::new(&path_str).is_absolute() {
                path_str
            } else {
                std::path::Path::new(&ctx.working_dir)
                    .join(&path_str)
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
            crate::blocking::tool(Box::new(move || Self::run(full_path, shown, content))).await
        })
    }
}

/// What a `write` call would do, without writing (approval prompt): the new
/// file's lines, or for an existing file the line counts and new content.
/// `None` when the target exists but cannot be read (the tool reports it).
pub fn preview_write(args: &serde_json::Value, working_dir: &str) -> Option<String> {
    let content = args["content"].as_str()?;
    let full = crate::file::edit::resolve_path(args["path"].as_str()?, working_dir);
    let shown = display_path(&full, working_dir);
    let preview = format_write_preview(&shown, content);
    let old = match std::fs::read_to_string(&full) {
        Ok(old) => old,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Some(preview.replacen("Wrote ", "Create ", 1));
        }
        Err(e) => {
            tracing::debug!(error = %e, "write preview: target not readable");
            return None;
        }
    };
    if old == content {
        return Some(format!("Overwrite {shown}  ·  no change"));
    }
    let body = preview.split_once("\n\n").map_or("", |(_, body)| body);
    Some(format!(
        "Overwrite {shown}  ·  {} → {} lines\n\n{body}",
        old.lines().count(),
        content.lines().count()
    ))
}

impl WriteTool {
    fn run(full_path: String, shown: String, content: String) -> ToolResult {
        if let Some(parent) = std::path::Path::new(&full_path).parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            return ToolResult {
                tool_call_id: String::new(),
                content: format!("Error creating directory: {}", e),
                is_error: true,
            };
        }

        // Lines the overwrite replaces, for the `[+N -M]` trailer. A file
        // that is not there yet (or cannot be read) removes nothing.
        let removed = std::fs::read_to_string(&full_path)
            .map(|old| old.lines().count())
            .unwrap_or(0);
        match crate::file::atomic::write_atomic(std::path::Path::new(&full_path), &content) {
            Ok(_) => ToolResult {
                tool_call_id: String::new(),
                // +lines preview so the TUI can paint add colours
                // (and syntax-highlight when the path has a known extension).
                content: format!(
                    "{}{}",
                    format_write_preview(&shown, &content),
                    whycodes_core::edit_trailer(content.lines().count(), removed)
                ),
                is_error: false,
            },
            Err(e) => ToolResult {
                tool_call_id: String::new(),
                content: format!("Error writing file '{}': {}", full_path, e),
                is_error: true,
            },
        }
    }
}

#[cfg(test)]
#[path = "write_tests.rs"]
mod tests;
