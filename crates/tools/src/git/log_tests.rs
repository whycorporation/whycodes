use super::*;
use crate::tool::ToolContext;
use serde_json::json;
use std::process::Command;

#[test]
fn log_module_loads() {
    assert!(!module_path!().is_empty());
}

#[tokio::test]
async fn log_filters_and_empty() {
    let t = GitLogTool;
    assert_eq!(t.name(), "git_log");
    assert!(!t.description().is_empty());
    let _ = t.parameters();
    let dir = tempfile::TempDir::new().unwrap();
    let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
    let err = t
        .execute(
            json!({"count": 1, "author": "x", "since": "2020-01-01", "path": "a.txt"}),
            &ctx,
        )
        .await;
    assert!(err.is_error, "{}", err.content);
}

#[test]
fn git_log_argv_includes_optional_filters() {
    assert_eq!(
        git_log_argv(10, None, None, None),
        vec!["log", "--oneline", "-n", "10"]
    );
    assert_eq!(
        git_log_argv(3, Some("me"), Some("2020-01-01"), Some("a.txt")),
        vec![
            "log",
            "--oneline",
            "-n",
            "3",
            "--author",
            "me",
            "--since",
            "2020-01-01",
            "--",
            "a.txt"
        ]
    );
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
async fn log_no_commits_for_unmatched_author() {
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = init_repo();
    let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
    let out = GitLogTool::new()
        .execute(json!({"author": "nobody-xyz-unmatched"}), &ctx)
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("No commits found"), "{}", out.content);
}

#[tokio::test]
async fn log_fails_when_git_missing_from_path() {
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var_os("PATH");
    unsafe { std::env::set_var("PATH", "/nonexistent-whycodes-path") };
    let dir = tempfile::TempDir::new().unwrap();
    let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
    let out = GitLogTool::new().execute(json!({}), &ctx).await;
    unsafe {
        match prev {
            Some(v) => std::env::set_var("PATH", v),
            None => std::env::remove_var("PATH"),
        }
    }
    assert!(out.is_error, "{}", out.content);
    assert!(
        out.content.contains("Failed to run git log"),
        "{}",
        out.content
    );
}
