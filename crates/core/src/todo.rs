//! Session todo list (Grok-style sticky TUI panel).
//!
//! Tools write through [`TodoSink`]; the host maps that onto [`crate::todo`]
//! events. Storage is session-scoped under `.whycode/todos/<session_id>.json`.

use serde::{Deserialize, Deserializer, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Status of one todo item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

impl TodoStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }

    /// ASCII mark used in the TUI panel and sidebar.
    pub fn mark(self) -> &'static str {
        match self {
            Self::Pending => "☐",
            Self::InProgress => "▶",
            Self::Completed => "☑",
            Self::Cancelled => "✗",
        }
    }

    /// True when the item is finished (done or skipped).
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "in_progress" => Self::InProgress,
            "completed" => Self::Completed,
            "cancelled" => Self::Cancelled,
            _ => Self::Pending,
        }
    }
}

impl<'de> Deserialize<'de> for TodoStatus {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(Self::parse(&s))
    }
}

/// One row in the session todo list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub status: TodoStatus,
}

impl TodoItem {
    pub fn new(id: impl Into<String>, content: impl Into<String>, status: TodoStatus) -> Self {
        Self {
            id: id.into(),
            content: content.into(),
            status,
        }
    }

    pub fn line(&self) -> String {
        format!("{} {}", self.status.mark(), self.content)
    }
}

/// On-disk envelope.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoList {
    #[serde(default)]
    pub todos: Vec<TodoItem>,
}

/// Callback the host (agent loop) installs so `todowrite` can reach the TUI.
pub type TodoSink = Arc<dyn Fn(Vec<TodoItem>) + Send + Sync>;

/// Path of the session todo file.
///
/// With a non-empty `session_id`: `.whycode/todos/<id>.json`.
/// Otherwise (tests, missing id): `.whycode/todos.json`.
pub fn todos_path(working_dir: &Path, session_id: Option<&str>) -> PathBuf {
    let dir = working_dir.join(".whycode");
    match session_id.map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => dir.join("todos").join(format!("{id}.json")),
        None => dir.join("todos.json"),
    }
}

/// Load todos. Missing or invalid files yield an empty list.
pub fn load_todos(working_dir: &Path, session_id: Option<&str>) -> Vec<TodoItem> {
    let path = todos_path(working_dir, session_id);
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str::<TodoList>(&raw)
        .map(|list| list.todos)
        .unwrap_or_default()
}

/// Persist the full list. Creates `.whycode` / `.whycode/todos` as needed.
pub fn save_todos(
    working_dir: &Path,
    session_id: Option<&str>,
    todos: &[TodoItem],
) -> Result<(), String> {
    let path = todos_path(working_dir, session_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("creating todo dir: {e}"))?;
    }
    // TodoList is always serializable (plain strings + a closed status enum).
    let json = serde_json::to_string_pretty(&TodoList {
        todos: todos.to_vec(),
    })
    .unwrap_or_else(|_| String::from("{\"todos\":[]}"));
    std::fs::write(&path, json).map_err(|e| format!("writing todos: {e}"))
}

/// Merge incoming items into `existing` by `id`. Empty ids always append.
pub fn apply_todo_update(
    mut existing: Vec<TodoItem>,
    incoming: Vec<TodoItem>,
    merge: bool,
) -> Vec<TodoItem> {
    if !merge {
        return incoming;
    }
    for new in incoming {
        if !new.id.is_empty()
            && let Some(old) = existing.iter_mut().find(|t| t.id == new.id)
        {
            *old = new;
        } else {
            existing.push(new);
        }
    }
    existing
}

/// Apply a `todowrite` / `todo` tool argument object.
///
/// `merge` defaults to **true** (Grok Build). Returns `None` when `todos` is
/// missing or not an array of items.
pub fn apply_todowrite_args(
    existing: &[TodoItem],
    input: &serde_json::Value,
) -> Option<Vec<TodoItem>> {
    let incoming: Vec<TodoItem> = serde_json::from_value(input.get("todos")?.clone()).ok()?;
    let merge = input.get("merge").and_then(|v| v.as_bool()).unwrap_or(true);
    Some(apply_todo_update(existing.to_vec(), incoming, merge))
}

/// Completed + cancelled count.
pub fn terminal_count(todos: &[TodoItem]) -> usize {
    todos.iter().filter(|t| t.status.is_terminal()).count()
}

/// True when every item is completed or cancelled (and the list is non-empty).
pub fn all_terminal(todos: &[TodoItem]) -> bool {
    !todos.is_empty() && todos.iter().all(|t| t.status.is_terminal())
}
