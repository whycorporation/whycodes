use super::*;
use crate::tool::ToolContext;

#[test]
fn plan_module_loads() {
    assert!(!module_path!().is_empty());
}

#[tokio::test]
async fn execute_enter_exit_and_invalid_action() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path().to_string_lossy());
    let tool = PlanTool::new();

    let entered = tool
        .execute(serde_json::json!({"action": "enter"}), &ctx)
        .await;
    assert!(!entered.is_error, "{}", entered.content);
    assert!(
        entered.content.contains("Planning mode entered"),
        "{}",
        entered.content
    );
    let marker = whycodes_core::project_dir(dir.path()).join("plan_mode");
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "1");

    let exited = tool
        .execute(serde_json::json!({"action": "exit"}), &ctx)
        .await;
    assert!(!exited.is_error, "{}", exited.content);
    assert!(
        exited.content.contains("Planning mode exited"),
        "{}",
        exited.content
    );
    assert!(!marker.exists());

    let again = tool
        .execute(serde_json::json!({"action": "exit"}), &ctx)
        .await;
    assert!(!again.is_error, "{}", again.content);

    let bad = tool
        .execute(serde_json::json!({"action": "nope"}), &ctx)
        .await;
    assert!(bad.is_error, "{}", bad.content);
    assert!(bad.content.contains("Invalid action"), "{}", bad.content);
}

#[tokio::test]
async fn default_and_fs_error_paths() {
    let t = PlanTool;
    assert_eq!(t.name(), "plan");
    assert!(!t.description().is_empty());
    let _ = t.parameters();

    let dir = tempfile::tempdir().unwrap();
    let why = whycodes_core::project_dir(dir.path());
    std::fs::write(&why, "not-a-dir").unwrap();
    let ctx = ToolContext::new(dir.path().to_string_lossy());
    let enter = t
        .execute(serde_json::json!({"action": "enter"}), &ctx)
        .await;
    assert!(enter.is_error, "{}", enter.content);
    assert!(
        enter.content.contains("Error creating .whycodes")
            || enter.content.contains("Error entering"),
        "{}",
        enter.content
    );

    let dir = tempfile::tempdir().unwrap();
    let why = whycodes_core::project_dir(dir.path());
    std::fs::create_dir_all(&why).unwrap();
    let marker = why.join("plan_mode");
    std::fs::create_dir(&marker).unwrap();
    let ctx = ToolContext::new(dir.path().to_string_lossy());
    let exit = t.execute(serde_json::json!({"action": "exit"}), &ctx).await;
    assert!(exit.is_error, "{}", exit.content);
    assert!(
        exit.content.contains("Error exiting planning mode"),
        "{}",
        exit.content
    );
}

#[test]
fn plan_result_helpers_cover_ok_fs_and_invalid() {
    let ok = plan_ok(PLAN_ENTERED);
    assert!(!ok.is_error);
    assert_eq!(ok.content, PLAN_ENTERED);

    let missing = plan_exit_remove_error(std::io::Error::new(std::io::ErrorKind::NotFound, "gone"));
    assert!(!missing.is_error);
    assert_eq!(missing.content, PLAN_NOT_ACTIVE);

    let other = plan_exit_remove_error(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "nope",
    ));
    assert!(other.is_error);
    assert!(other.content.contains("Error exiting planning mode"));

    let mkdir = plan_fs_error(
        "Error creating .whycodes directory",
        std::io::Error::other("boom"),
    );
    assert!(mkdir.is_error);
    assert!(mkdir.content.contains("Error creating .whycodes directory"));

    let bad = plan_invalid_action("nope");
    assert!(bad.is_error);
    assert!(bad.content.contains("Invalid action: 'nope'"));
}
