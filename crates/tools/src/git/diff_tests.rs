use super::*;
use crate::tool::ToolContext;
use serde_json::json;
use std::process::Command;

#[test]
fn diff_module_loads() {
    assert!(!module_path!().is_empty());
}

#[tokio::test]
async fn diff_staged_and_path() {
    let t = GitDiffTool;
    assert_eq!(t.name(), "git_diff");
    assert!(!t.description().is_empty());
    let _ = t.parameters();
    let dir = tempfile::TempDir::new().unwrap();
    let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
    let err = t
        .execute(json!({"staged": true, "path": "a.txt"}), &ctx)
        .await;
    assert!(err.is_error, "{}", err.content);
}

fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().unwrap();
    assert!(
        Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success()
    );
    let _ = Command::new("git")
        .args(["config", "user.email", "test@whycodes.local"])
        .current_dir(dir.path())
        .status();
    let _ = Command::new("git")
        .args(["config", "user.name", "whycodes-test"])
        .current_dir(dir.path())
        .status();
    std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
    assert!(
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success()
    );
    dir
}

#[tokio::test]
async fn diff_clean_tree_and_path_filter() {
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = init_repo();
    let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
    let clean = GitDiffTool::new().execute(json!({}), &ctx).await;
    assert!(!clean.is_error, "{}", clean.content);
    assert!(
        clean.content.contains("No changes") || clean.content.is_empty(),
        "{}",
        clean.content
    );
    std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
    let filtered = GitDiffTool::new()
        .execute(json!({"path": "a.txt", "staged": false}), &ctx)
        .await;
    assert!(!filtered.is_error, "{}", filtered.content);
}

#[tokio::test]
async fn diff_fails_when_git_missing_from_path() {
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var_os("PATH");
    unsafe { std::env::set_var("PATH", "/nonexistent-whycodes-path") };
    let dir = tempfile::TempDir::new().unwrap();
    let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
    let out = GitDiffTool::new().execute(json!({}), &ctx).await;
    unsafe {
        match prev {
            Some(v) => std::env::set_var("PATH", v),
            None => std::env::remove_var("PATH"),
        }
    }
    assert!(out.is_error, "{}", out.content);
    assert!(
        out.content.contains("Failed to run git diff"),
        "{}",
        out.content
    );
}
