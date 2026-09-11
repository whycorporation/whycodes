use super::*;
use crate::tool::ToolContext;
use serde_json::json;

#[test]
fn blame_module_loads() {
    assert!(!module_path!().is_empty());
    let _ = GitBlameTool::default();
    assert_eq!(blame_stdout(b""), "No blame information available.");
    assert_eq!(blame_stdout(b"line\n"), "line\n");
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

#[test]
fn blame_line_args_covers_range_and_open_end() {
    assert_eq!(blame_line_args(Some(1), Some(2)).as_deref(), Some("1,2"));
    assert_eq!(blame_line_args(Some(3), None).as_deref(), Some("3,"));
    assert!(blame_line_args(None, Some(4)).is_none());
    assert!(blame_line_args(None, None).is_none());
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
