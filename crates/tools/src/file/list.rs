use async_trait::async_trait;
use serde_json::json;
use std::fs;
use std::path::Path;

use crate::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

/// List files and directories — OpenCode `list` tool equivalent.
pub struct ListTool;

impl Default for ListTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ListTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ListTool {
    fn name(&self) -> &str {
        "list"
    }

    fn description(&self) -> &str {
        "List files and directories in a path. Returns names sorted with directories first, then files."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory path to list (relative to project root or absolute). Defaults to project root."
                },
                "ignore": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Glob patterns to ignore (optional)"
                }
            },
            "required": []
        })
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let rel = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let base = Path::new(&ctx.working_dir);
        let target = if Path::new(rel).is_absolute() {
            Path::new(rel).to_path_buf()
        } else {
            base.join(rel)
        };

        if !target.exists() {
            return ToolResult {
                tool_call_id: String::new(),
                content: format!("Path does not exist: {}", target.display()),
                is_error: true,
            };
        }
        if !target.is_dir() {
            return ToolResult {
                tool_call_id: String::new(),
                content: format!("Not a directory: {}", target.display()),
                is_error: true,
            };
        }

        let ignore: Vec<String> = args
            .get("ignore")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let mut dirs = Vec::new();
        let mut files = Vec::new();

        match fs::read_dir(&target) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with('.') && name != ".gitignore" {
                        // still show hidden? OpenCode list typically shows them
                    }
                    if ignore.iter().any(|pat| name_matches(&name, pat)) {
                        continue;
                    }
                    match entry.file_type() {
                        Ok(ft) if ft.is_dir() => dirs.push(format!("{}/", name)),
                        Ok(_) => files.push(name),
                        Err(_) => files.push(name),
                    }
                }
            }
            Err(e) => {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Failed to list {}: {}", target.display(), e),
                    is_error: true,
                };
            }
        }

        dirs.sort();
        files.sort();

        let mut out = format!("Contents of {}:\n", target.display());
        if dirs.is_empty() && files.is_empty() {
            out.push_str("(empty)\n");
        } else {
            for d in &dirs {
                out.push_str(&format!("  {}\n", d));
            }
            for f in &files {
                out.push_str(&format!("  {}\n", f));
            }
        }
        out.push_str(&format!(
            "\n{} directories, {} files",
            dirs.len(),
            files.len()
        ));

        ToolResult {
            tool_call_id: String::new(),
            content: out,
            is_error: false,
        }
    }
}

fn name_matches(name: &str, pattern: &str) -> bool {
    // Simple glob: * and exact match
    if pattern == "*" {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return name.ends_with(suffix);
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return name.starts_with(prefix);
    }
    name == pattern
}
