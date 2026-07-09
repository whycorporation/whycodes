use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TodoItem {
    id: String,
    content: String,
    status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TodoList {
    todos: Vec<TodoItem>,
}

pub struct TodoWriteTool;

impl TodoWriteTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &str {
        "todowrite"
    }

    fn description(&self) -> &str {
        "Create and manage a structured task list"
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
                    "description": "If true, merge with existing todos; if false, replace entirely"
                }
            },
            "required": ["todos"]
        })
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let merge = args
            .get("merge")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Parse new todos from args
        let new_todos: Vec<TodoItem> = match serde_json::from_value(args["todos"].clone()) {
            Ok(t) => t,
            Err(e) => {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Error parsing todos: {}", e),
                    is_error: true,
                };
            }
        };

        // Determine the storage directory
        let whycode_dir = std::path::Path::new(&ctx.working_dir).join(".whycode");
        let todos_path = whycode_dir.join("todos.json");

        // Create .whycode directory if it doesn't exist
        if let Err(e) = std::fs::create_dir_all(&whycode_dir) {
            return ToolResult {
                tool_call_id: String::new(),
                content: format!("Error creating .whycode directory: {}", e),
                is_error: true,
            };
        }

        // Load existing todos if merging
        let final_todos: Vec<TodoItem> = if merge {
            match std::fs::read_to_string(&todos_path) {
                Ok(content) => {
                    match serde_json::from_str::<TodoList>(&content) {
                        Ok(list) => {
                            let mut existing = list.todos;
                            // Merge: update existing items by id, add new ones
                            for new_item in new_todos {
                                if let Some(existing_item) =
                                    existing.iter_mut().find(|t| t.id == new_item.id)
                                {
                                    existing_item.content = new_item.content;
                                    existing_item.status = new_item.status;
                                } else {
                                    existing.push(new_item);
                                }
                            }
                            existing
                        }
                        Err(_) => new_todos,
                    }
                }
                Err(_) => new_todos,
            }
        } else {
            new_todos
        };

        // Save todos
        let todo_list = TodoList {
            todos: final_todos.clone(),
        };

        let json_str = match serde_json::to_string_pretty(&todo_list) {
            Ok(s) => s,
            Err(e) => {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Error serializing todos: {}", e),
                    is_error: true,
                };
            }
        };

        if let Err(e) = std::fs::write(&todos_path, &json_str) {
            return ToolResult {
                tool_call_id: String::new(),
                content: format!("Error writing todos file: {}", e),
                is_error: true,
            };
        }

        // Format the result
        let mut result = String::from("Todos:\n");
        let status_icons = [
            ("pending", "⏳"),
            ("in_progress", "🔄"),
            ("completed", "✅"),
            ("cancelled", "❌"),
        ];

        for item in &final_todos {
            let icon = status_icons
                .iter()
                .find(|(s, _)| s == &item.status.as_str())
                .map(|(_, i)| *i)
                .unwrap_or("❓");
            result.push_str(&format!("  {} [{}] {}\n", icon, item.id, item.content));
        }

        result.push_str(&format!(
            "\nStored {} todos in {}",
            final_todos.len(),
            todos_path.display()
        ));

        ToolResult {
            tool_call_id: String::new(),
            content: result,
            is_error: false,
        }
    }
}
