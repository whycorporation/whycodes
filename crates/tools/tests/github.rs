//! Integration tests for GitHub API tools (github_issue, github_pr).
//!
//! These tests probe the live GitHub API when GITHUB_TOKEN is set.
//! When the token is absent, tests are skipped with [`std::process::exit(0)`].

use whycodes_core::ToolContext;
use whycodes_tools::executor::ToolExecutor;

/// Env tokens only (not `gh auth`). Live tests skip when neither is set.
fn github_token() -> Option<String> {
    std::env::var("GITHUB_TOKEN")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::env::var("GH_TOKEN")
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
}

fn neutral_ctx() -> ToolContext {
    ToolContext {
        working_dir: "/tmp".to_string(),
        session_id: None,
        sandbox: whycodes_core::SandboxSettings::off(),
        network: whycodes_core::NetworkPolicy::unrestricted(),
        file_claims: None,
        agent_id: None,
        agent_label: None,
        file_index: None,
        panel: None,
        todo_sink: None,
        swarm_hub: None,
    }
}

/// A small helper: use a well-known public repo that won't hit rate-limit issues
/// for list/view operations when authenticated.
const TEST_OWNER: &str = "rust-lang";
const TEST_REPO: &str = "rust";

// ---------------------------------------------------------------------------
// github_issue
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_github_issue_list() {
    let token = match github_token() {
        Some(t) => t,
        None => {
            eprintln!("SKIP: GITHUB_TOKEN not set, skipping GitHub issue list test");
            return;
        }
    };

    let executor = ToolExecutor::new();
    let tool = executor
        .get("github_issue")
        .expect("github_issue tool not found");
    let ctx = neutral_ctx();

    let args = serde_json::json!({
        "action": "list",
        "owner": TEST_OWNER,
        "repo": TEST_REPO,
        "token": token,
        "state": "open",
        "per_page": 3
    });
    let result = tool.execute(args, &ctx).await;

    assert!(
        !result.is_error,
        "github_issue list should succeed: {}",
        result.content
    );

    // The response should be valid JSON containing an array
    let trimmed = result.content.trim();
    assert!(
        trimmed.starts_with('['),
        "expected JSON array, got: {}",
        &trimmed[..trimmed.len().min(200)]
    );
}

#[tokio::test]
async fn test_github_issue_list_without_token() {
    let executor = ToolExecutor::new();
    let tool = executor
        .get("github_issue")
        .expect("github_issue tool not found");
    let ctx = neutral_ctx();

    let args = serde_json::json!({
        "action": "list",
        "owner": TEST_OWNER,
        "repo": TEST_REPO,
        "state": "open"
    });

    // Env token present: empty `token` falls through to env / `gh`, so skip.
    // No env token: missing-token error, unless the host has `gh auth login`.
    if github_token().is_some() {
        eprintln!("SKIP: GITHUB_TOKEN/GH_TOKEN set; missing-token path not isolated");
        return;
    }
    let result = tool.execute(args, &ctx).await;
    if result.is_error {
        assert!(
            result.content.contains("token not found")
                || result.content.contains("gh auth login")
                || result.content.contains("token"),
            "should mention token: {}",
            result.content
        );
    }
}

// ---------------------------------------------------------------------------
// github_pr
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_github_pr_list() {
    let token = match github_token() {
        Some(t) => t,
        None => {
            eprintln!("SKIP: GITHUB_TOKEN not set, skipping GitHub PR list test");
            return;
        }
    };

    let executor = ToolExecutor::new();
    let tool = executor.get("github_pr").expect("github_pr tool not found");
    let ctx = neutral_ctx();

    let args = serde_json::json!({
        "action": "list",
        "owner": TEST_OWNER,
        "repo": TEST_REPO,
        "token": token,
        "state": "open"
    });
    let result = tool.execute(args, &ctx).await;

    assert!(
        !result.is_error,
        "github_pr list should succeed: {}",
        result.content
    );

    // The response embeds "Status: 200" at the top and then JSON array
    assert!(
        result.content.contains("Status:"),
        "should contain status line: {}",
        &result.content[..result.content.len().min(200)]
    );
}

#[tokio::test]
async fn test_github_pr_list_without_token() {
    let executor = ToolExecutor::new();
    let tool = executor.get("github_pr").expect("github_pr tool not found");
    let ctx = neutral_ctx();

    let args = serde_json::json!({
        "action": "list",
        "owner": TEST_OWNER,
        "repo": TEST_REPO,
        "state": "open"
    });

    if github_token().is_some() {
        eprintln!("SKIP: GITHUB_TOKEN/GH_TOKEN set; missing-token path not isolated");
        return;
    }
    let result = tool.execute(args, &ctx).await;
    if result.is_error {
        assert!(
            result.content.contains("token")
                || result.content.contains("gh auth login")
                || result.content.contains("required"),
            "should mention token: {}",
            result.content
        );
    }
}

#[tokio::test]
async fn test_github_pr_invalid_action() {
    let executor = ToolExecutor::new();
    let tool = executor.get("github_pr").expect("github_pr tool not found");
    let ctx = neutral_ctx();

    // When no token is available, this will fail on token before action validation.
    // We'll test action validation with an invalid token just to hit the right path.
    let args = serde_json::json!({
        "action": "invalid_action_xyz",
        "owner": TEST_OWNER,
        "repo": TEST_REPO,
        "token": "not_a_real_token"
    });
    let result = tool.execute(args, &ctx).await;

    // It should fail — either because of the action or the invalid token via API.
    // We just verify it doesn't panic.
    assert!(result.is_error, "invalid action should produce an error");
}
