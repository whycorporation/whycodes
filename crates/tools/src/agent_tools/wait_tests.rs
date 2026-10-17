use super::*;
use crate::tool::ToolContext;
use serde_json::json;

#[test]
fn wait_module_loads() {
    assert!(!module_path!().is_empty());
    let _ = WaitTool::default();
}

#[tokio::test]
async fn execute_reports_missing_intercept() {
    let t = WaitTool;
    assert_eq!(t.name(), "wait");
    assert!(t.description().contains("background job"));
    let params = t.parameters();
    assert_eq!(params["properties"]["timeout_secs"]["type"], "integer");
    assert_eq!(params["properties"]["job_id"]["type"], "string");
    let r = t.execute(json!({}), &ToolContext::new(".")).await;
    assert!(r.is_error);
    assert!(r.content.contains("not intercepted"), "{}", r.content);
}

#[test]
fn timeout_defaults_and_clamps() {
    assert_eq!(wait_timeout_secs(&json!({})), WAIT_DEFAULT_SECS);
    assert_eq!(wait_timeout_secs(&json!({"timeout_secs": 5})), 5);
    assert_eq!(wait_timeout_secs(&json!({"timeout_secs": 0})), 0);
    assert_eq!(
        wait_timeout_secs(&json!({"timeout_secs": 10_000})),
        WAIT_MAX_SECS
    );
    // Negative and non-integer values fall back to the default.
    assert_eq!(
        wait_timeout_secs(&json!({"timeout_secs": -3})),
        WAIT_DEFAULT_SECS
    );
    assert_eq!(
        wait_timeout_secs(&json!({"timeout_secs": "9"})),
        WAIT_DEFAULT_SECS
    );
}

#[test]
fn job_id_is_trimmed_and_optional() {
    assert_eq!(wait_job_id(&json!({})), None);
    assert_eq!(wait_job_id(&json!({"job_id": ""})), None);
    assert_eq!(wait_job_id(&json!({"job_id": "   "})), None);
    assert_eq!(wait_job_id(&json!({"job_id": 3})), None);
    assert_eq!(wait_job_id(&json!({"job_id": " bg-2 "})), Some("bg-2"));
}
