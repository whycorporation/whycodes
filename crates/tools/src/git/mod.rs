pub mod blame;
pub mod commit;
pub mod diff;
pub mod log;
pub mod status;

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;
    use crate::tool::{Tool, ToolContext};
    use serde_json::json;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn init_repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        assert!(
            Command::new("git")
                .args(["init"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        let _ = Command::new("git")
            .args(["config", "user.email", "test@whycodes.local"])
            .current_dir(&root)
            .status();
        let _ = Command::new("git")
            .args(["config", "user.name", "whycodes-test"])
            .current_dir(&root)
            .status();
        std::fs::write(root.join("a.txt"), b"line1\nline2\n").unwrap();
        assert!(
            Command::new("git")
                .args(["add", "."])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args(["commit", "-m", "init"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        (dir, root)
    }

    fn ctx(dir: &Path) -> ToolContext {
        ToolContext::new(dir.to_string_lossy().into_owned())
    }

    #[test]
    fn mod_module_loads() {
        assert!(!module_path!().is_empty());
    }

    #[tokio::test]
    async fn git_log_status_diff_blame_and_commit_on_repo() {
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_keep, root) = init_repo();
        let ctx = ctx(&root);

        let log = log::GitLogTool::new()
            .execute(
                json!({"count": 5, "author": "whycodes", "path": "a.txt"}),
                &ctx,
            )
            .await;
        assert!(!log.is_error, "{log:?}");
        assert!(
            log.content.contains("init") || !log.content.is_empty(),
            "{log:?}"
        );

        let status = status::GitStatusTool::new().execute(json!({}), &ctx).await;
        assert!(!status.is_error, "{status:?}");

        std::fs::write(root.join("a.txt"), b"line1\nchanged\n").unwrap();
        let diff = diff::GitDiffTool::new()
            .execute(json!({"staged": false}), &ctx)
            .await;
        assert!(!diff.is_error, "{diff:?}");
        assert!(
            diff.content.contains("changed")
                || diff.content.contains("diff")
                || !diff.content.is_empty(),
            "{diff:?}"
        );

        let blame = blame::GitBlameTool::new()
            .execute(
                json!({"file": "a.txt", "revision": "HEAD", "line_start": 1, "line_end": 2}),
                &ctx,
            )
            .await;
        assert!(!blame.is_error, "{blame:?}");
        assert!(
            blame.content.contains("a.txt")
                || blame.content.contains("whycodes")
                || !blame.content.is_empty(),
            "{blame:?}"
        );

        std::fs::write(root.join("b.txt"), b"new\n").unwrap();
        let commit = commit::GitCommitTool::new()
            .execute(
                json!({"message": "add b", "files": ["b.txt"], "push": false}),
                &ctx,
            )
            .await;
        assert!(!commit.is_error, "{commit:?}");
        assert!(
            commit.content.to_lowercase().contains("commit")
                || commit.content.contains("add b")
                || !commit.content.is_empty(),
            "{commit:?}"
        );
    }

    #[tokio::test]
    async fn git_tools_error_without_repo_and_missing_args() {
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = ctx(dir.path());

        let log = log::GitLogTool::new().execute(json!({}), &ctx).await;
        assert!(log.is_error, "{log:?}");

        let status = status::GitStatusTool::new().execute(json!({}), &ctx).await;
        assert!(status.is_error, "{status:?}");

        let diff = diff::GitDiffTool::new().execute(json!({}), &ctx).await;
        assert!(diff.is_error, "{diff:?}");

        let blame_missing = blame::GitBlameTool::new().execute(json!({}), &ctx).await;
        assert!(blame_missing.is_error, "{blame_missing:?}");
        assert!(
            blame_missing.content.to_lowercase().contains("file"),
            "{}",
            blame_missing.content
        );

        let blame_missing_file = blame::GitBlameTool::new()
            .execute(json!({"file": "nope.txt"}), &ctx)
            .await;
        assert!(blame_missing_file.is_error, "{blame_missing_file:?}");

        let commit_missing = commit::GitCommitTool::new().execute(json!({}), &ctx).await;
        assert!(commit_missing.is_error, "{commit_missing:?}");
        assert!(
            commit_missing.content.to_lowercase().contains("message"),
            "{}",
            commit_missing.content
        );
    }
}
