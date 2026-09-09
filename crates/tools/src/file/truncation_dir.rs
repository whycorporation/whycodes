use serde_json::json;

use super::paths::{display_path, human_size, list_dir_entries, resolve_path};
use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

/// Tool that lists a directory and truncates the output to fit context.
/// In-process (no shell `ls`) — consistent with `list`.
pub struct TruncationDirTool;

impl Default for TruncationDirTool {
    fn default() -> Self {
        Self::new()
    }
}

impl TruncationDirTool {
    pub fn new() -> Self {
        Self
    }
}
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

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            let path_arg = args["path"].as_str().unwrap_or(".");
            let path = resolve_path(&ctx.working_dir, path_arg);
            let shown = display_path(&path, &ctx.working_dir);
            let max_entries = args["max_entries"].as_u64().unwrap_or(50) as usize;

            if !path.exists() {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Path does not exist: {}", shown),
                    is_error: true,
                };
            }
            if !path.is_dir() {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Not a directory: {}", shown),
                    is_error: true,
                };
            }

            truncation_from(
                take_dir_entries(list_dir_or_err(&path)),
                &shown,
                max_entries,
            )
        })
    }
}

fn list_dir_or_err(path: &std::path::Path) -> Result<Vec<super::paths::DirEntryInfo>, ToolResult> {
    list_dir_entries(path, &[]).map_err(list_error)
}

fn take_dir_entries(
    result: Result<Vec<super::paths::DirEntryInfo>, ToolResult>,
) -> Result<Vec<super::paths::DirEntryInfo>, ToolResult> {
    result
}

fn truncation_from(
    result: Result<Vec<super::paths::DirEntryInfo>, ToolResult>,
    shown: &str,
    max_entries: usize,
) -> ToolResult {
    match result {
        Ok(entries) => truncation_ok(shown, entries, max_entries),
        Err(e) => e,
    }
}

fn truncation_ok(
    shown: &str,
    entries: Vec<super::paths::DirEntryInfo>,
    max_entries: usize,
) -> ToolResult {
    let total = entries.len();
    let shown_entries = entries.into_iter().take(max_entries);

    let mut result = format!("Contents of {} ({} entries):\n", shown, total);
    for e in shown_entries {
        if e.is_dir {
            result.push_str(&format!("  {}/\n", e.name));
        } else {
            let sz = e.size.map(human_size).unwrap_or_else(|| "?".into());
            result.push_str(&format!("  {}  ({})\n", e.name, sz));
        }
    }

    if total > max_entries {
        result.push_str(&format!(
            "\n[... {} entries truncated from {} total]",
            total - max_entries,
            total
        ));
    }

    ToolResult {
        tool_call_id: String::new(),
        content: result,
        is_error: false,
    }
}

fn list_error(e: String) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: e,
        is_error: true,
    }
}

#[cfg(test)]
#[path = "truncation_dir_tests.rs"]
mod tests;
