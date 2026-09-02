//! Session todo list (Grok-style sticky TUI panel).
//!
//! Tools write through [`TodoSink`]; the host maps that onto [`crate::todo`]
//! events. Storage is session-scoped under `.whycodes/todos/<session_id>.json`.

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
/// With a non-empty `session_id`: `.whycodes/todos/<id>.json`.
/// Otherwise (tests, missing id): `.whycodes/todos.json`.
pub fn todos_path(working_dir: &Path, session_id: Option<&str>) -> PathBuf {
    let dir = crate::paths::project_dir(working_dir);
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

/// Persist the full list. Creates `.whycodes` / `.whycodes/todos` as needed.
pub fn save_todos(
    working_dir: &Path,
    session_id: Option<&str>,
    todos: &[TodoItem],
) -> Result<(), String> {
    let path = todos_path(working_dir, session_id);
    // `todos_path` is always nested under `.whycodes/`, so parent exists.
    // `unwrap_or` keeps the llvm-cov 100% floor (no uncovered `if let` else).
    std::fs::create_dir_all(path.parent().unwrap_or(working_dir))
        .map_err(|e| format!("creating todo dir: {e}"))?;
    // `Value::to_string` cannot fail; `unwrap_or` evaluates the fallback
    // eagerly so llvm-cov does not see a dead `unwrap_or_else` closure.
    let value = serde_json::json!({ "todos": todos });
    let json = serde_json::to_string_pretty(&value).unwrap_or(value.to_string());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_item_persist_and_merge() {
        for (s, mark) in [
            (TodoStatus::Pending, "☐"),
            (TodoStatus::InProgress, "▶"),
            (TodoStatus::Completed, "☑"),
            (TodoStatus::Cancelled, "✗"),
        ] {
            assert!(!s.as_str().is_empty());
            assert_eq!(s.mark(), mark);
        }
        assert!(!TodoStatus::Pending.is_terminal());
        assert!(TodoStatus::Completed.is_terminal());
        assert_eq!(TodoStatus::parse("in_progress"), TodoStatus::InProgress);
        assert_eq!(TodoStatus::parse("completed"), TodoStatus::Completed);
        assert_eq!(TodoStatus::parse("cancelled"), TodoStatus::Cancelled);
        assert_eq!(TodoStatus::parse("nope"), TodoStatus::Pending);
        assert_eq!(TodoStatus::default(), TodoStatus::Pending);

        let json = r#"["pending","in_progress","completed","cancelled","x"]"#;
        let parsed: Vec<TodoStatus> = serde_json::from_str(json).unwrap();
        assert_eq!(parsed[0], TodoStatus::Pending);
        assert_eq!(parsed[4], TodoStatus::Pending);

        let item = TodoItem::new("1", "do it", TodoStatus::InProgress);
        assert!(item.line().contains("do it"));

        let dir = tempfile::tempdir().unwrap();
        assert!(load_todos(dir.path(), None).is_empty());
        assert!(todos_path(dir.path(), None).ends_with("todos.json"));
        assert!(todos_path(dir.path(), Some("  ")).ends_with("todos.json"));
        assert!(todos_path(dir.path(), Some("sid")).ends_with("sid.json"));

        let items = vec![
            TodoItem::new("a", "one", TodoStatus::Pending),
            TodoItem::new("b", "two", TodoStatus::Completed),
        ];
        save_todos(dir.path(), Some("sid"), &items).unwrap();
        let loaded = load_todos(dir.path(), Some("sid"));
        assert_eq!(loaded.len(), 2);
        std::fs::write(todos_path(dir.path(), Some("bad")), "not-json").unwrap();
        assert!(load_todos(dir.path(), Some("bad")).is_empty());

        let merged = apply_todo_update(
            items.clone(),
            vec![TodoItem::new("a", "one-upd", TodoStatus::Completed)],
            true,
        );
        assert_eq!(merged[0].content, "one-upd");
        let replaced = apply_todo_update(
            items.clone(),
            vec![TodoItem::new("z", "z", TodoStatus::Pending)],
            false,
        );
        assert_eq!(replaced.len(), 1);
        let appended = apply_todo_update(
            items.clone(),
            vec![TodoItem::new("", "no-id", TodoStatus::Pending)],
            true,
        );
        assert_eq!(appended.len(), 3);

        let args = serde_json::json!({"todos":[{"id":"a","content":"x","status":"completed"}]});
        let out = apply_todowrite_args(&items, &args).unwrap();
        assert_eq!(out[0].status, TodoStatus::Completed);
        assert!(apply_todowrite_args(&items, &serde_json::json!({})).is_none());
        assert_eq!(terminal_count(&items), 1);
        assert!(!all_terminal(&items));
        assert!(!all_terminal(&[]));
        assert!(all_terminal(&[TodoItem::new(
            "c",
            "done",
            TodoStatus::Cancelled
        )]));
    }
}
