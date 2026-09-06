use super::*;
use crate::tool::{Tool, ToolContext};
use serde_json::json;
use std::path::Path;

fn ctx(dir: &Path) -> ToolContext {
    ToolContext::new(dir.to_string_lossy().into_owned())
}

#[test]
fn mod_module_loads() {
    assert!(!module_path!().is_empty());
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
