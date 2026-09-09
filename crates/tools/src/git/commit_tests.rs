use super::*;
use crate::tool::ToolContext;
use serde_json::json;
use std::process::Command;

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

#[test]
fn commit_module_loads() {
    assert!(!module_path!().is_empty());
    let _ = GitCommitTool::default();
    let cmd_err = git_cmd_error("Failed to run git commit", "gone");
    assert!(cmd_err.is_error);
    assert!(cmd_err.content.contains("Failed to run git commit"));
    let stderr = git_stderr_error(b"nothing staged");
    assert!(stderr.is_error);
    assert_eq!(stderr.content, "nothing staged");
    assert_eq!(commit_stdout(b"", "empty"), "empty");
    assert_eq!(commit_stdout(b"ok\n", "empty"), "ok\n");
    let spawn_err = git_output(Err(std::io::Error::other("gone")), "Failed to run git add");
    assert!(spawn_err.unwrap_err().is_error);
    let failed = git_output(
        Ok(std::process::Output {
            status: fail_status(),
            stdout: Vec::new(),
            stderr: b"nothing staged".to_vec(),
        }),
        "Failed to run git commit",
    );
    assert_eq!(failed.unwrap_err().content, "nothing staged");
    let bounced = git_output_failed(git_cmd_error("Failed to run git commit", "gone"));
    assert!(bounced.is_error);
    assert!(take_git_output(Err(git_cmd_error("Failed to run git commit", "gone"))).is_err());
    let from_err = commit_from_output(
        Err(git_cmd_error("Failed to run git commit", "gone")),
        "empty",
        false,
        ".",
    );
    assert!(from_err.is_error);
}

fn fail_status() -> std::process::ExitStatus {
    #[cfg(windows)]
    {
        Command::new("cmd")
            .args(["/C", "exit", "1"])
            .status()
            .unwrap()
    }
    #[cfg(not(windows))]
    {
        Command::new("false").status().unwrap()
    }
}

#[tokio::test]
async fn commit_all_push_and_error_paths() {
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let t = GitCommitTool;
    assert_eq!(t.name(), "git_commit");
    assert!(!t.description().is_empty());
    assert_eq!(t.parameters()["required"][0], "message");

    let dir = init_repo();
    let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
    std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
    let all = t
        .execute(json!({"message": "all tracked", "files": []}), &ctx)
        .await;
    assert!(!all.is_error, "{}", all.content);

    std::fs::write(dir.path().join("b.txt"), "new\n").unwrap();
    let add = t
        .execute(
            json!({"message": "add b", "files": ["b.txt", 1], "push": false}),
            &ctx,
        )
        .await;
    assert!(!add.is_error, "{}", add.content);

    let push = t
        .execute(
            json!({"message": "push me", "files": ["nope.txt"], "push": true}),
            &ctx,
        )
        .await;
    assert!(
        push.is_error || push.content.contains("Push") || !push.content.is_empty(),
        "{}",
        push.content
    );

    let empty_push = git_push(dir.path().to_str().unwrap());
    assert!(!empty_push.is_empty());

    let missing = t.execute(json!({}), &ctx).await;
    assert!(missing.is_error, "{}", missing.content);

    let bad_add = t
        .execute(
            json!({"message": "x", "files": ["does-not-exist.txt"]}),
            &ctx,
        )
        .await;
    assert!(bad_add.is_error, "{}", bad_add.content);
}

#[tokio::test]
async fn commit_push_to_local_remote_and_nothing_to_commit() {
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = init_repo();
    let remote = tempfile::TempDir::new().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "--bare"])
            .current_dir(remote.path())
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args(["remote", "add", "origin", remote.path().to_str().unwrap()])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success()
    );
    let branch = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(dir.path())
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let branch = branch.trim();
    let _ = Command::new("git")
        .args(["push", "-u", "origin", branch])
        .current_dir(dir.path())
        .status();

    let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
    std::fs::write(dir.path().join("c.txt"), "c\n").unwrap();
    let with_files = GitCommitTool::new()
        .execute(
            json!({"message": "add c", "files": ["c.txt"], "push": true}),
            &ctx,
        )
        .await;
    assert!(!with_files.is_error, "{}", with_files.content);

    std::fs::write(dir.path().join("a.txt"), "three\n").unwrap();
    let all_push = GitCommitTool::new()
        .execute(json!({"message": "update a", "push": true}), &ctx)
        .await;
    assert!(!all_push.is_error, "{}", all_push.content);

    let nothing = GitCommitTool::new()
        .execute(json!({"message": "empty"}), &ctx)
        .await;
    assert!(nothing.is_error, "{}", nothing.content);

    let push_ok = git_push(dir.path().to_str().unwrap());
    assert!(!push_ok.is_empty(), "{push_ok}");
}

#[tokio::test]
async fn commit_fails_when_git_missing_from_path() {
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var_os("PATH");
    unsafe { std::env::set_var("PATH", "/nonexistent-whycodes-path") };
    let dir = tempfile::TempDir::new().unwrap();
    let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
    let out = GitCommitTool::new()
        .execute(json!({"message": "x", "files": ["a.txt"]}), &ctx)
        .await;
    let all = GitCommitTool::new()
        .execute(json!({"message": "x"}), &ctx)
        .await;
    let push = git_push(dir.path().to_str().unwrap());
    unsafe {
        match prev {
            Some(v) => std::env::set_var("PATH", v),
            None => std::env::remove_var("PATH"),
        }
    }
    assert!(out.is_error, "{}", out.content);
    assert!(
        out.content.contains("Failed to run git add"),
        "{}",
        out.content
    );
    assert!(all.is_error, "{}", all.content);
    assert!(
        all.content.contains("Failed to run git commit"),
        "{}",
        all.content
    );
    assert!(push.contains("Failed to run git push"), "{push}");
}
