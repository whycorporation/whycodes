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

            let entries = match list_dir_entries(&path, &[]) {
                Ok(e) => e,
                Err(e) => {
                    return ToolResult {
                        tool_call_id: String::new(),
                        content: e,
                        is_error: true,
                    };
                }
            };

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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolContext;

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext::new(dir.to_string_lossy().into_owned())
    }

    #[tokio::test]
    async fn lists_files_and_dirs_with_sizes() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), "hello").expect("write");
        std::fs::create_dir(dir.path().join("sub")).expect("mkdir");

        let out = TruncationDirTool::new()
            .execute(json!({}), &ctx(dir.path()))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("Contents of"), "{}", out.content);
        assert!(out.content.contains("a.txt"), "{}", out.content);
        assert!(out.content.contains("sub/"), "{}", out.content);
        assert!(
            out.content.contains("(5 B)"),
            "5-byte file shown: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn truncates_entries_over_max() {
        let dir = tempfile::tempdir().expect("tempdir");
        for i in 0..5 {
            std::fs::write(dir.path().join(format!("f{i}.txt")), "x").expect("write");
        }
        let out = TruncationDirTool::new()
            .execute(json!({ "max_entries": 2 }), &ctx(dir.path()))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content
                .contains("[... 3 entries truncated from 5 total]"),
            "{}",
            out.content
        );
        // Only 2 entries listed.
        let listed = out.content.matches(".txt").count();
        assert_eq!(listed, 2, "{}", out.content);
    }

    #[tokio::test]
    async fn missing_path_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = TruncationDirTool::new()
            .execute(json!({ "path": "nope" }), &ctx(dir.path()))
            .await;
        assert!(out.is_error);
        assert!(
            out.content.contains("Path does not exist"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn file_path_is_not_a_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), "x").expect("write");
        let out = TruncationDirTool::new()
            .execute(json!({ "path": "a.txt" }), &ctx(dir.path()))
            .await;
        assert!(out.is_error);
        assert!(out.content.contains("Not a directory"), "{}", out.content);
    }

    #[tokio::test]
    async fn relative_path_resolves_from_working_dir() {
        let root = tempfile::tempdir().expect("tempdir");
        let nested = root.path().join("nested");
        std::fs::create_dir(&nested).expect("mkdir");
        std::fs::write(nested.join("b.txt"), "hi").expect("write");

        let out = TruncationDirTool::new()
            .execute(json!({ "path": "nested" }), &ctx(root.path()))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("b.txt"), "{}", out.content);
        assert!(
            out.content.contains("nested"),
            "shown path relative: {}",
            out.content
        );
    }
}
