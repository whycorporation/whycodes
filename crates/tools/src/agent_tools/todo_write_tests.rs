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
