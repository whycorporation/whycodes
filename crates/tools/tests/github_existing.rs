//! Integration tests for GitHub API tools (github_issue, github_pr).
//!
//! These tests probe the live GitHub API when GITHUB_TOKEN is set.
//! When the token is absent, tests are skipped with [`std::process::exit(0)`].

use whycode_core::ToolContext;
use whycode_tools::executor::ToolExecutor;

/// Returns the GitHub token from the environment, or None if not set.
fn github_token() -> Option<String> {
    std::env::var("GITHUB_TOKEN").ok()
}

fn neutral_ctx() -> ToolContext {
    ToolContext {
        working_dir: "/tmp".to_string(),
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

    // Without explicit token and without GITHUB_TOKEN env var,
    // if GITHUB_TOKEN is set in the env this might accidentally pass.
    // So we explicitly clear it for this test.
    if github_token().is_some() {
        // Token IS available. We test with an explicit invalid token
        // to ensure the error path works.
        let args_bad = serde_json::json!({
            "action": "list",
            "owner": TEST_OWNER,
            "repo": TEST_REPO,
            "token": "",
            "state": "open"
        });
        let result = tool.execute(args_bad, &ctx).await;
        // Empty token should fail (resolve_token returns None for empty)
        assert!(result.is_error, "empty token should produce error");
    } else {
        // No token anywhere — the tool should error
        let result = tool.execute(args, &ctx).await;
        assert!(result.is_error, "missing token should produce error");
        assert!(
            result.content.contains("token not found") || result.content.contains("token"),
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
        // Test with empty token
        let args_bad = serde_json::json!({
            "action": "list",
            "owner": TEST_OWNER,
            "repo": TEST_REPO,
            "token": "",
            "state": "open"
        });
        let result = tool.execute(args_bad, &ctx).await;
        assert!(result.is_error, "empty token should produce error");
    } else {
        let result = tool.execute(args, &ctx).await;
        assert!(result.is_error, "missing token should produce error");
        assert!(
            result.content.contains("token") || result.content.contains("required"),
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
