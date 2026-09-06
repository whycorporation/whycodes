use super::*;
use crate::tool::ToolContext;
use serde_json::json;

#[test]
fn blame_module_loads() {
    assert!(!module_path!().is_empty());
}

#[tokio::test]
async fn blame_line_start_only_and_missing_file() {
    let t = GitBlameTool;
    assert_eq!(t.name(), "git_blame");
    assert!(!t.description().is_empty());
    let _ = t.parameters();
    let missing = t.execute(json!({}), &ToolContext::new(".")).await;
    assert!(missing.is_error, "{}", missing.content);
    let dir = tempfile::TempDir::new().unwrap();
    let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
    let err = t
        .execute(json!({"file": "a.txt", "line_start": 1}), &ctx)
        .await;
    assert!(err.is_error, "{}", err.content);
}

#[tokio::test]
async fn blame_fails_when_git_missing_from_path() {
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var_os("PATH");
    unsafe { std::env::set_var("PATH", "/nonexistent-whycodes-path") };
    let dir = tempfile::TempDir::new().unwrap();
    let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
    let out = GitBlameTool::new()
        .execute(json!({"file": "a.txt"}), &ctx)
        .await;
    unsafe {
        match prev {
            Some(v) => std::env::set_var("PATH", v),
            None => std::env::remove_var("PATH"),
        }
    }
    assert!(out.is_error, "{}", out.content);
    assert!(
        out.content.contains("Failed to run git blame"),
        "{}",
        out.content
    );
}
