use serde_json::json;
use std::path::Path;

use crate::tool::{Tool, ToolContext};
use whycodes_core::todo::load_todos;
use whycodes_core::types::ToolResult;

/// Read the current session todo list — OpenCode `todoread` tool.
pub struct TodoReadTool;

impl Default for TodoReadTool {
    fn default() -> Self {
        Self::new()
    }
}

impl TodoReadTool {
    pub fn new() -> Self {
        Self
    }
}
impl Tool for TodoReadTool {
    fn name(&self) -> &str {
        "todoread"
    }

    fn description(&self) -> &str {
        "Read the current todo list for this session"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    fn execute<'a>(
        &'a self,
        _args: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            let todos = load_todos(Path::new(&ctx.working_dir), ctx.session_id.as_deref());
            if todos.is_empty() {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: "No todos yet. Use todowrite to create a task list.".to_string(),
                    is_error: false,
                };
            }
            let mut result = String::from("Todos:\n");
            for item in &todos {
                result.push_str(&format!(
                    "  {} [{}] {} ({})\n",
                    item.status.mark(),
                    item.id,
                    item.content,
                    item.status.as_str()
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
    use whycodes_core::todo::{TodoItem, TodoStatus, save_todos};

    fn ctx(dir: &std::path::Path, session: Option<&str>) -> ToolContext {
        let mut c = ToolContext::unsandboxed(dir.to_string_lossy().into_owned());
        c.session_id = session.map(str::to_string);
        c
    }

    #[tokio::test]
    async fn empty_and_populated() {
        let dir = tempfile::tempdir().unwrap();
        let tool = TodoReadTool::new();
        assert_eq!(tool.name(), "todoread");
        let empty = tool.execute(json!({}), &ctx(dir.path(), None)).await;
        assert!(!empty.is_error);
        assert!(empty.content.contains("No todos yet"));

        save_todos(
            dir.path(),
            Some("s1"),
            &[
                TodoItem::new("a", "one", TodoStatus::Pending),
                TodoItem::new("b", "two", TodoStatus::InProgress),
                TodoItem::new("c", "done", TodoStatus::Completed),
                TodoItem::new("d", "skip", TodoStatus::Cancelled),
            ],
        )
        .unwrap();
        let list = tool.execute(json!({}), &ctx(dir.path(), Some("s1"))).await;
        assert!(list.content.contains("☐ [a] one (pending)"));
        assert!(list.content.contains("▶ [b] two (in_progress)"));
        assert!(list.content.contains("☑ [c] done (completed)"));
        assert!(list.content.contains("✗ [d] skip (cancelled)"));
        let other = tool
            .execute(json!({}), &ctx(dir.path(), Some("other")))
            .await;
        assert!(other.content.contains("No todos yet"));
    }
}
