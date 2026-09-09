use super::*;
use crate::tool::ToolContext;

#[test]
fn worktree_module_loads() {
    assert!(!module_path!().is_empty());
    let _ = WorktreeTool::default();
}

#[tokio::test]
async fn execute_reports_missing_intercept() {
    let t = WorktreeTool;
    assert_eq!(t.name(), "worktree");
    assert!(!t.description().is_empty());
    assert_eq!(t.parameters()["required"][0], "action");
    let r = t
        .execute(serde_json::json!({}), &ToolContext::new("."))
        .await;
    assert!(r.is_error);
    assert!(r.content.contains("not intercepted"), "{}", r.content);
}
