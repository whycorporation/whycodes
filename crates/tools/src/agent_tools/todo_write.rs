use async_trait::async_trait;
use serde_json::json;
use std::path::Path;

use crate::tool::{Tool, ToolContext};
use whycodes_core::todo::{TodoItem, apply_todo_update, load_todos, save_todos};
use whycodes_core::types::ToolResult;

pub struct TodoWriteTool {
    name: &'static str,
}

impl Default for TodoWriteTool {
    fn default() -> Self {
        Self::new()
    }
}

impl TodoWriteTool {
    pub fn new() -> Self {
        Self { name: "todowrite" }
    }

    /// Alias used by some models
    pub fn as_todo() -> Self {
        Self { name: "todo" }
    }
}

#[async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "Create and manage a structured task list. The user sees this list live at the top of the \
         session. Use for any task with 3+ steps. Mark the current item in_progress (only one) \
         and completed as soon as the step is done. Default merge=true updates items by id."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "description": "List of todo items",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Unique identifier for the todo item"
                            },
                            "content": {
                                "type": "string",
                                "description": "Description of the task"
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed", "cancelled"],
                                "description": "Status of the task"
                            }
                        },
                        "required": ["id", "content", "status"]
                    }
                },
                "merge": {
                    "type": "boolean",
                    "description": "If true (default), merge with existing todos by id; if false, replace entirely"
                }
            },
            "required": ["todos"]
        })
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let merge = args.get("merge").and_then(|v| v.as_bool()).unwrap_or(true);

        let new_todos: Vec<TodoItem> = match serde_json::from_value(args["todos"].clone()) {
            Ok(t) => t,
            Err(e) => {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Error parsing todos: {e}"),
                    is_error: true,
                };
            }
        };

        let working = Path::new(&ctx.working_dir);
        let session_id = ctx.session_id.as_deref();
        let existing = if merge {
            load_todos(working, session_id)
        } else {
            Vec::new()
        };
        let final_todos = apply_todo_update(existing, new_todos, merge);

        if let Err(e) = save_todos(working, session_id, &final_todos) {
            return ToolResult {
                tool_call_id: String::new(),
                content: format!("Error writing todos: {e}"),
                is_error: true,
            };
        }

        if let Some(sink) = &ctx.todo_sink {
            sink(final_todos.clone());
        }

        ToolResult {
            tool_call_id: String::new(),
            content: format_todo_result(&final_todos, working, session_id),
            is_error: false,
        }
    }
}

fn format_todo_result(todos: &[TodoItem], working: &Path, session_id: Option<&str>) -> String {
    let mut result = String::from("Todos:\n");
    for item in todos {
        result.push_str(&format!(
            "  {} [{}] {}\n",
            item.status.mark(),
            item.id,
            item.content
        ));
    }
    result.push_str(&format!(
        "\nStored {} todos in {}",
        todos.len(),
        whycodes_core::todo::todos_path(working, session_id).display()
    ));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use whycodes_core::todo::TodoStatus;

    fn ctx(dir: &std::path::Path, session: Option<&str>) -> ToolContext {
        let mut c = ToolContext::unsandboxed(dir.to_string_lossy().into_owned());
        c.session_id = session.map(str::to_string);
        c
    }

    #[tokio::test]
    async fn writes_merges_and_notifies_sink() {
        let dir = tempfile::tempdir().unwrap();
        let captured = Arc::new(Mutex::new(Vec::new()));
        let cap = captured.clone();
        let mut c = ctx(dir.path(), Some("sess-a"));
        c.todo_sink = Some(Arc::new(move |todos| {
            *cap.lock().unwrap() = todos;
        }));

        let tool = TodoWriteTool::new();
        assert_eq!(tool.name(), "todowrite");
        assert!(tool.description().contains("live"));
        assert_eq!(TodoWriteTool::as_todo().name(), "todo");
        let _ = TodoWriteTool::default();

        let first = tool
            .execute(
                json!({"todos":[
                    {"id":"a","content":"one","status":"pending"},
                    {"id":"b","content":"two","status":"in_progress"}
                ]}),
                &c,
            )
            .await;
        assert!(!first.is_error, "{}", first.content);
        assert!(first.content.contains("☐ [a] one"));
        assert!(first.content.contains("▶ [b] two"));
        assert_eq!(captured.lock().unwrap().len(), 2);

        let merged = tool
            .execute(
                json!({"todos":[{"id":"a","content":"one","status":"completed"}]}),
                &c,
            )
            .await;
        assert!(!merged.is_error, "{}", merged.content);
        let list = captured.lock().unwrap().clone();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].status, TodoStatus::Completed);
        assert_eq!(list[1].status, TodoStatus::InProgress);

        let replaced = tool
            .execute(
                json!({
                    "merge": false,
                    "todos":[{"id":"z","content":"only","status":"pending"}]
                }),
                &c,
            )
            .await;
        assert!(!replaced.is_error, "{}", replaced.content);
        assert_eq!(captured.lock().unwrap().len(), 1);

        let bad = tool.execute(json!({"todos": "nope"}), &c).await;
        assert!(bad.is_error);
        assert!(bad.content.contains("Error parsing todos"));
    }

    #[tokio::test]
    async fn session_path_differs_from_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let tool = TodoWriteTool::new();
        let with_id = ctx(dir.path(), Some("s1"));
        let without = ctx(dir.path(), None);
        let _ = tool
            .execute(
                json!({"todos":[{"id":"a","content":"sess","status":"pending"}]}),
                &with_id,
            )
            .await;
        let _ = tool
            .execute(
                json!({"todos":[{"id":"b","content":"fb","status":"pending"}]}),
                &without,
            )
            .await;
        let sess = whycodes_core::todo::load_todos(dir.path(), Some("s1"));
        let fb = whycodes_core::todo::load_todos(dir.path(), None);
        assert_eq!(sess[0].content, "sess");
        assert_eq!(fb[0].content, "fb");
    }

    #[tokio::test]
    async fn save_error_is_tool_error() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("not-a-dir");
        std::fs::write(&blocker, "x").unwrap();
        let c = ctx(&blocker, None);
        let out = TodoWriteTool::new()
            .execute(
                json!({"todos":[{"id":"a","content":"x","status":"pending"}]}),
                &c,
            )
            .await;
        assert!(out.is_error);
        assert!(
            out.content.contains("Error writing todos"),
            "{}",
            out.content
        );
    }
}
