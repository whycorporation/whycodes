use super::*;
use crate::tool::ToolContext;

#[test]
fn task_module_loads() {
    assert!(!module_path!().is_empty());
}

#[tokio::test]
async fn execute_reports_missing_intercept() {
    let t = TaskTool;
    assert_eq!(t.name(), "task");
    assert!(!t.description().is_empty());
    assert_eq!(t.parameters()["required"][0], "goal");
    let r = t
        .execute(
            serde_json::json!({"goal": "explore"}),
            &ToolContext::new("."),
        )
        .await;
    assert!(r.is_error);
    assert!(r.content.contains("not intercepted"), "{}", r.content);
    assert!(r.content.contains("explore"), "{}", r.content);
    let missing = t
        .execute(serde_json::json!({}), &ToolContext::new("."))
        .await;
    assert!(
        missing.content.contains("no goal specified"),
        "{}",
        missing.content
    );
}
