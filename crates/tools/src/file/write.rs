use async_trait::async_trait;
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

#[async_trait]
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

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
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
        crate::blocking::tool(move || Self::run(full_path, shown, content)).await
    }
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

        match std::fs::write(&full_path, &content) {
            Ok(_) => ToolResult {
                tool_call_id: String::new(),
                // Grok-like: +lines preview so the TUI can paint add colours
                // (and syntax-highlight when the path has a known extension).
                content: format_write_preview(&shown, &content),
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
mod tests {
    use super::*;
    use crate::tool::ToolContext;
    use whycodes_core::file_claims::FileClaimRegistry;

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext::new(dir.to_string_lossy().into_owned())
    }

    #[tokio::test]
    async fn metadata_describes_write_tool() {
        let t = WriteTool::new();
        assert_eq!(t.name(), "write");
        assert!(t.description().contains("Write"));
        let params = t.parameters();
        assert_eq!(params["required"][0], "path");
        assert_eq!(params["required"][1], "content");
    }

    #[tokio::test]
    async fn writes_relative_path_and_creates_parents() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = WriteTool::new()
            .execute(
                json!({ "path": "nested/a.txt", "content": "hello\nworld" }),
                &ctx(dir.path()),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("Wrote"), "{}", out.content);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("nested/a.txt")).expect("read"),
            "hello\nworld"
        );
    }

    #[tokio::test]
    async fn writes_absolute_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let abs = dir.path().join("abs.txt");
        let out = WriteTool::new()
            .execute(
                json!({ "path": abs.to_string_lossy(), "content": "abs" }),
                &ctx(dir.path()),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(std::fs::read_to_string(&abs).expect("read"), "abs");
    }

    #[tokio::test]
    async fn empty_content_writes_empty_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = WriteTool::new()
            .execute(json!({ "path": "empty.txt" }), &ctx(dir.path()))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("(empty file)"), "{}", out.content);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("empty.txt")).expect("read"),
            ""
        );
    }

    #[tokio::test]
    async fn file_claim_conflict_blocks_write() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("claimed.txt");
        std::fs::write(&target, "old").expect("seed");

        let claims = FileClaimRegistry::new();
        assert!(matches!(
            claims.try_claim("other", "other-agent", &target),
            whycodes_core::file_claims::ClaimResult::Acquired
        ));

        let mut c = ctx(dir.path());
        c.file_claims = Some(claims);
        c.agent_id = Some("me".into());
        c.agent_label = Some("me-agent".into());

        let out = WriteTool::new()
            .execute(json!({ "path": "claimed.txt", "content": "new" }), &c)
            .await;
        assert!(out.is_error);
        assert!(out.content.contains("File conflict"), "{}", out.content);
        assert_eq!(std::fs::read_to_string(&target).expect("read"), "old");
    }
}
