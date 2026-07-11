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

#[async_trait]
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

    async fn execute(&self, _args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let todos_path = std::path::Path::new(&ctx.working_dir)
            .join(".whycode")
            .join("todos.json");

        if !todos_path.exists() {
            return ToolResult {
                tool_call_id: String::new(),
                content: "No todos yet. Use todowrite to create a task list.".to_string(),
                is_error: false,
            };
        }

        match std::fs::read_to_string(&todos_path) {
            Ok(content) => match serde_json::from_str::<TodoList>(&content) {
                Ok(list) => {
                    if list.todos.is_empty() {
                        return ToolResult {
                            tool_call_id: String::new(),
                            content: "Todo list is empty.".to_string(),
                            is_error: false,
                        };
                    }
                    let mut result = String::from("Todos:\n");
                    for item in &list.todos {
                        let icon = match item.status.as_str() {
                            "pending" => "⏳",
                            "in_progress" => "🔄",
                            "completed" => "✅",
                            "cancelled" => "❌",
                            _ => "❓",
                        };
                        result.push_str(&format!(
                            "  {} [{}] {} ({})\n",
                            icon, item.id, item.content, item.status
                        ));
                    }
                    ToolResult {
                        tool_call_id: String::new(),
                        content: result,
                        is_error: false,
                    }
                }
                Err(e) => ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Failed to parse todos: {}", e),
                    is_error: true,
                },
            },
            Err(e) => ToolResult {
                tool_call_id: String::new(),
                content: format!("Failed to read todos: {}", e),
                is_error: true,
            },
        }
    }
}
