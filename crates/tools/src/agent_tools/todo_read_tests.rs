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

#[tokio::test]
async fn default_and_parameters() {
    let t = TodoReadTool::default();
    assert_eq!(t.name(), "todoread");
    assert!(!t.description().is_empty());
    let _ = t.parameters();
}
