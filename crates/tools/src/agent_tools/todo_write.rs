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

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
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
        })
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
#[path = "todo_write_tests.rs"]
mod tests;
