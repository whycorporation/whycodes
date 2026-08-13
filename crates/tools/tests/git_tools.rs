//! Integration tests for git tools: git_diff, git_log, git_status.
//!
//! These tests run against the whycode repository itself.
//! They verify that the tools produce well-formed output when executed
//! inside a real git repository.

use whycode_core::ToolContext;
use whycode_tools::executor::ToolExecutor;

/// Build a ToolContext pointing at the whycode workspace root.
/// This is a real git repo, so all git tools should work.
fn repo_ctx() -> ToolContext {
    // Find the workspace root (where .git lives) from the manifest dir
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent() // crates/tools -> crates
        .unwrap()
        .parent() // crates -> workspace root
        .unwrap();

    ToolContext {
        working_dir: workspace_root.to_string_lossy().to_string(),
        session_id: None,
        sandbox: whycode_core::SandboxSettings::off(),
        network: whycode_core::NetworkPolicy::unrestricted(),
        file_claims: None,
        agent_id: None,
        agent_label: None,
        file_index: None,
        panel: None,
    }
}

// ---------------------------------------------------------------------------
// git_diff
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_git_diff() {
    let executor = ToolExecutor::new();
    let tool = executor.get("git_diff").expect("git_diff tool not found");
    let ctx = repo_ctx();

    let args = serde_json::json!({});
    let result = tool.execute(args, &ctx).await;

    // In a repo without uncommitted changes, diff should succeed but may be empty.
    assert!(
        !result.is_error,
        "git_diff should succeed: {}",
        result.content
    );

    // The output should either contain diff content or the "no changes" message
    let has_diff = result.content.contains("diff --git")
        || result.content.contains("No changes")
        || result.content.is_empty();
    assert!(has_diff, "unexpected git_diff output: {}", result.content);
}

#[tokio::test]
async fn test_git_diff_staged() {
    let executor = ToolExecutor::new();
    let tool = executor.get("git_diff").expect("git_diff tool not found");
    let ctx = repo_ctx();

    let args = serde_json::json!({"staged": true});
    let result = tool.execute(args, &ctx).await;

    assert!(
        !result.is_error,
        "git_diff --staged should succeed: {}",
        result.content
    );
}

#[tokio::test]
async fn test_git_diff_path_filter() {
    let executor = ToolExecutor::new();
    let tool = executor.get("git_diff").expect("git_diff tool not found");
    let ctx = repo_ctx();

    let args = serde_json::json!({"path": "README.md"});
    let result = tool.execute(args, &ctx).await;

    assert!(
        !result.is_error,
        "git_diff with path should succeed: {}",
        result.content
    );
}

// ---------------------------------------------------------------------------
// git_log
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_git_log() {
    let executor = ToolExecutor::new();
    let tool = executor.get("git_log").expect("git_log tool not found");
    let ctx = repo_ctx();

    let args = serde_json::json!({"count": 5});
    let result = tool.execute(args, &ctx).await;

    assert!(
        !result.is_error,
        "git_log should succeed: {}",
        result.content
    );
    // git log --oneline output: "<hash> <message>"
    // Should have at least one line with a short hash
    assert!(
        !result.content.is_empty() || result.content.contains("No commits"),
        "git_log should produce output or say 'No commits': {}",
        result.content
    );

    // If there are commits, each line should match the oneline format
    if !result.content.contains("No commits") && !result.content.is_empty() {
        for line in result.content.trim().lines() {
            // Format: <hash> <message>  (hash is hex, at least 7 chars)
            let parts: Vec<&str> = line.splitn(2, ' ').collect();
            assert!(
                !parts.is_empty(),
                "each line should have a hash: '{}'",
                line
            );
            assert!(
                parts[0].len() >= 7,
                "hash should be at least 7 chars: '{}'",
                parts[0]
            );
        }
    }
}

#[tokio::test]
async fn test_git_log_with_count() {
    let executor = ToolExecutor::new();
    let tool = executor.get("git_log").expect("git_log tool not found");
    let ctx = repo_ctx();

    let args = serde_json::json!({"count": 1});
    let result = tool.execute(args, &ctx).await;

    assert!(
        !result.is_error,
        "git_log count=1 should succeed: {}",
        result.content
    );

    if !result.content.contains("No commits") && !result.content.is_empty() {
        let lines: Vec<&str> = result.content.trim().lines().collect();
        assert!(
            lines.len() <= 1,
            "should have at most 1 commit, got {}",
            lines.len()
        );
    }
}

// ---------------------------------------------------------------------------
// git_status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_git_status() {
    let executor = ToolExecutor::new();
    let tool = executor
        .get("git_status")
        .expect("git_status tool not found");
    let ctx = repo_ctx();

    let args = serde_json::json!({});
    let result = tool.execute(args, &ctx).await;

    assert!(
        !result.is_error,
        "git_status should succeed: {}",
        result.content
    );

    // Status output is either a list of status codes or "clean" message
    let valid = result.content.contains("Working tree clean")
        || result.content.is_empty()
        || result.content.trim().lines().any(|line| {
            // git status --short lines start with status codes like " M", "?? ", "A "
            line.len() >= 2
                && (line.starts_with(' ')
                    || line.starts_with('M')
                    || line.starts_with('A')
                    || line.starts_with('D')
                    || line.starts_with('R')
                    || line.starts_with('?')
                    || line.starts_with('!'))
        });
    // If empty, that's still valid (no changes reported)
    assert!(
        valid || result.content.is_empty(),
        "unexpected git_status output: '{}'",
        result.content
    );
}

#[tokio::test]
async fn test_git_status_path_filter() {
    let executor = ToolExecutor::new();
    let tool = executor
        .get("git_status")
        .expect("git_status tool not found");
    let ctx = repo_ctx();

    // Filter to a path that definitely exists in the repo
    let args = serde_json::json!({"path": "Cargo.toml"});
    let result = tool.execute(args, &ctx).await;

    assert!(
        !result.is_error,
        "git_status with path should succeed: {}",
        result.content
    );
}
