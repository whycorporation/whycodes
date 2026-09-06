use super::*;
use crate::tool::ToolContext;

#[test]
fn schedule_module_loads() {
    assert!(!module_path!().is_empty());
}

#[tokio::test]
async fn execute_reports_missing_intercept() {
    let t = ScheduleTool;
    assert_eq!(t.name(), "schedule");
    assert!(!t.description().is_empty());
    let _ = t.parameters();
    let r = t
        .execute(serde_json::json!({}), &ToolContext::new("."))
        .await;
    assert!(r.is_error);
    assert!(r.content.contains("not intercepted"), "{}", r.content);
}
