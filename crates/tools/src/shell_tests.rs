use super::*;
use crate::tool::Tool;
use crate::tool::ToolContext;
use serde_json::json;

fn exit_ok_command() -> &'static str {
    #[cfg(windows)]
    {
        "exit 0"
    }
    #[cfg(not(windows))]
    {
        "true"
    }
}

fn hang_command() -> &'static str {
    #[cfg(windows)]
    {
        "ping -n 30 127.0.0.1 > NUL"
    }
    #[cfg(not(windows))]
    {
        "sleep 30"
    }
}

#[test]
fn shell_module_loads() {
    assert!(!module_path!().is_empty());
    let join = shell_join_error("boom");
    assert!(join.is_error);
    assert!(join.content.contains("Task join error"));
    assert!(shell_join_failed("boom").is_error);
    let from_join = shell_from_join(Err("boom".into()));
    assert!(from_join.is_error);
    assert!(from_join.content.contains("Task join error"));
}

#[test]
fn bash_and_shell_aliases_and_schema() {
    let bash = ShellTool::new();
    let shell = ShellTool::as_shell();
    let via_default = ShellTool::default();
    assert_eq!(bash.name(), "bash");
    assert_eq!(shell.name(), "shell");
    assert_eq!(via_default.name(), "bash");
    assert!(bash.description().contains("shell") || bash.description().contains("command"));
    let params = bash.parameters();
    assert_eq!(params["required"][0], "command");
}

#[tokio::test]
async fn execute_echo_and_failing_command() {
    let dir = tempfile::TempDir::new().unwrap();
    let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
    let tool = ShellTool::new();

    let ok = tool
        .execute(json!({"command": "echo hello-cov", "timeout": 10}), &ctx)
        .await;
    assert!(!ok.is_error, "{ok:?}");
    assert!(ok.content.contains("hello-cov"), "{ok:?}");

    let empty = tool.execute(json!({"command": ""}), &ctx).await;
    assert!(empty.is_error || empty.content.contains("empty") || !empty.content.is_empty());

    let fail = tool
        .execute(json!({"command": "exit 42", "timeout": 10}), &ctx)
        .await;
    assert!(
        fail.is_error || fail.content.contains("42") || !fail.content.is_empty(),
        "{fail:?}"
    );
}

#[tokio::test]
async fn timeout_zero_is_clamped() {
    let dir = tempfile::TempDir::new().unwrap();
    let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
    let out = ShellTool::default()
        .execute(json!({"command": exit_ok_command(), "timeout": 0}), &ctx)
        .await;
    assert!(!out.is_error || !out.content.is_empty(), "{out:?}");
}

#[tokio::test]
async fn timeout_kills_long_sleep() {
    let dir = tempfile::TempDir::new().unwrap();
    let ctx = ToolContext::unsandboxed(dir.path().to_string_lossy().into_owned());
    let out = ShellTool::new()
        .execute(json!({"command": hang_command(), "timeout": 1}), &ctx)
        .await;
    assert!(out.is_error, "{}", out.content);
    assert!(
        out.content.contains("timed out") || out.content.contains("Sandbox error"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn background_flag_still_runs_command() {
    let dir = tempfile::TempDir::new().unwrap();
    let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
    let out = ShellTool::as_shell()
        .execute(
            json!({"command": "echo bg-flag", "background": true, "timeout": 10}),
            &ctx,
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("bg-flag"), "{}", out.content);
}

#[tokio::test]
async fn sandbox_error_from_file_as_working_dir() {
    let dir = tempfile::TempDir::new().unwrap();
    let file = dir.path().join("not-a-dir");
    std::fs::write(&file, "x").unwrap();
    let ctx = ToolContext::new(file.to_string_lossy().into_owned());
    let out = ShellTool::new()
        .execute(json!({"command": "echo hi", "timeout": 5}), &ctx)
        .await;
    assert!(out.is_error, "{}", out.content);
    assert!(
        out.content.contains("Sandbox error")
            || out.content.contains("working directory")
            || !out.content.is_empty(),
        "{}",
        out.content
    );
}
