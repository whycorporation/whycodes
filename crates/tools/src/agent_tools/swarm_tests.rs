use super::*;
use crate::tool::ToolContext;

#[test]
fn swarm_module_loads() {
    assert!(!module_path!().is_empty());
}

#[tokio::test]
async fn execute_reports_task_count() {
    let t = SwarmTool;
    assert_eq!(t.name(), "swarm");
    assert!(!t.description().is_empty());
    let params = t.parameters();
    assert_eq!(params["required"][0], "tasks");
    let r = t
        .execute(
            serde_json::json!({"tasks": [{"goal": "a"}, {"goal": "b"}]}),
            &ToolContext::new("."),
        )
        .await;
    assert!(r.is_error);
    assert!(r.content.contains("2 tasks"), "{}", r.content);
    let empty = t
        .execute(serde_json::json!({}), &ToolContext::new("."))
        .await;
    assert!(empty.content.contains("0 tasks"), "{}", empty.content);
}
