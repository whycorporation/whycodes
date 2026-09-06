use super::*;
use crate::tool::ToolContext;

#[test]
fn background_module_loads() {
    assert!(!module_path!().is_empty());
}

#[tokio::test]
async fn execute_reports_missing_intercept() {
    let t = BgTool;
    assert_eq!(t.name(), "bg");
    assert!(!t.description().is_empty());
    let params = t.parameters();
    assert_eq!(params["properties"]["action"]["enum"][0], "list");
    let r = t
        .execute(serde_json::json!({}), &ToolContext::new("."))
        .await;
    assert!(r.is_error);
    assert!(r.content.contains("not intercepted"), "{}", r.content);
}
