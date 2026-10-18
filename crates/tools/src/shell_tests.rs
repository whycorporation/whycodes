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
    assert_eq!(join_error_string("boom"), "boom");
    let join = shell_join_error("boom");
    assert!(join.is_error);
    assert!(join.content.contains("Task join error"));
    assert!(shell_join_failed("boom").is_error);
    let from_join = shell_from_join(Err("boom".into()));
    assert!(from_join.is_error);
    assert!(from_join.content.contains("Task join error"));

    let timed = shell_from_join(Ok(Err(whycodes_sandbox::SandboxError::TimedOut(3))));
    assert!(timed.is_error);
    assert!(timed.content.contains("timed out after 3"));

    let sandbox = shell_from_join(Ok(Err(whycodes_sandbox::SandboxError::Unavailable(
        "no bwrap".into(),
    ))));
    assert!(sandbox.is_error);
    assert!(sandbox.content.contains("Sandbox error"));
    assert!(sandbox.content.contains("no bwrap"));
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
    assert!(
        bash.description().contains("agent://tools/bash"),
        "{}",
        bash.description()
    );
    assert!(!bash.description().contains("bubblewrap"));
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

/// Prints `$WHYCODES_SCRATCH`.
#[cfg(windows)]
fn print_scratch_command() -> &'static str {
    // Git Bash / the embedded shell expand `$VAR`; `cmd.exe` expands `%VAR%`.
    "echo \"$WHYCODES_SCRATCH\" %WHYCODES_SCRATCH%"
}

#[cfg(not(windows))]
fn print_scratch_command() -> &'static str {
    "echo \"$WHYCODES_SCRATCH\""
}

async fn scratch_seen_from(working_dir: &std::path::Path) -> (String, std::path::PathBuf) {
    let ctx = ToolContext::new(working_dir.to_string_lossy().into_owned());
    let out = ShellTool::new()
        .execute(
            json!({"command": print_scratch_command(), "timeout": 10}),
            &ctx,
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    let want = whycodes_core::project_scratch_dir(&std::path::absolute(working_dir).unwrap());
    (out.content, want)
}

#[tokio::test]
async fn bash_exports_the_project_scratch_dir() {
    let dir = tempfile::TempDir::new().unwrap();
    let (out, want) = scratch_seen_from(dir.path()).await;
    assert!(out.contains(&*want.to_string_lossy()), "{out}");
    assert!(want.is_dir(), "created before the command runs");
}

#[tokio::test]
async fn swarm_worktree_gets_its_own_scratch_dir() {
    let dir = tempfile::TempDir::new().unwrap();
    // A swarm worker runs with its worktree as the working dir.
    let worktree = dir
        .path()
        .join(".whycodes")
        .join("swarm")
        .join("run-1")
        .join("w1");
    std::fs::create_dir_all(&worktree).unwrap();
    let (out, want) = scratch_seen_from(&worktree).await;
    assert!(want.starts_with(&worktree));
    assert!(out.contains(&*want.to_string_lossy()), "{out}");
    let parent = whycodes_core::project_scratch_dir(dir.path());
    assert!(!out.trim().ends_with(&*parent.to_string_lossy()), "{out}");
}
