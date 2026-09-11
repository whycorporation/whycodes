use super::*;

fn client_and_headers() -> (reqwest::Client, reqwest::header::HeaderMap) {
    (reqwest::Client::new(), reqwest::header::HeaderMap::new())
}

#[test]
fn metadata_describes_supported_actions_and_required_repository() {
    let tool = GithubIssueTool::default();
    let parameters = tool.parameters();

    assert_eq!(tool.name(), "github_issue");
    assert_eq!(
        parameters["properties"]["action"]["enum"],
        json!(["create", "list", "view", "close", "reopen", "comment"])
    );
    assert_eq!(parameters["required"], json!(["action", "owner", "repo"]));
}

#[tokio::test]
async fn execute_rejects_missing_repository_before_token_or_network() {
    let result = GithubIssueTool::new()
        .execute(
            json!({ "action": "list", "owner": "owner" }),
            &ToolContext::new("."),
        )
        .await;

    assert!(result.is_error);
    assert_eq!(result.content, "owner and repo are required.");
}

#[tokio::test]
async fn execute_rejects_unknown_action_without_requesting() {
    let result = GithubIssueTool::new()
        .execute(
            json!({
                "action": "archive",
                "owner": "owner",
                "repo": "repo",
                "token": "token"
            }),
            &ToolContext::new("."),
        )
        .await;

    assert!(result.is_error);
    assert_eq!(
        result.content,
        "Unknown action: 'archive'. Valid: create, list, view, close, reopen, comment"
    );
}

#[tokio::test]
async fn create_requires_a_title_before_requesting() {
    let (client, headers) = client_and_headers();
    let error = issue_create(&client, &headers, "owner", "repo", &json!({}))
        .await
        .expect_err("missing title must fail");

    assert_eq!(error, "title is required for create action.");
}

#[tokio::test]
async fn numbered_actions_validate_the_issue_number() {
    let (client, headers) = client_and_headers();
    let args = json!({});

    assert_eq!(
        issue_view(&client, &headers, "owner", "repo", &args)
            .await
            .expect_err("missing number must fail"),
        "issue_number is required and must be an integer for view action."
    );
    assert_eq!(
        issue_set_state(&client, &headers, "owner", "repo", &args, "closed")
            .await
            .expect_err("missing number must fail"),
        "issue_number is required and must be an integer for closed action."
    );
    assert_eq!(
        issue_comment(&client, &headers, "owner", "repo", &args)
            .await
            .expect_err("missing number must fail"),
        "issue_number is required and must be an integer for comment action."
    );
}

#[tokio::test]
async fn comment_requires_a_nonempty_body_before_requesting() {
    let (client, headers) = client_and_headers();
    let error = issue_comment(
        &client,
        &headers,
        "owner",
        "repo",
        &json!({ "issue_number": 42, "body": "" }),
    )
    .await
    .expect_err("empty comment must fail");

    assert_eq!(error, "body is required for comment action.");
}

struct ApiBaseGuard {
    prev: Option<std::ffi::OsString>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl ApiBaseGuard {
    fn set(base: &str) -> Self {
        let lock = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("WHYCODES_GITHUB_API_BASE");
        unsafe { std::env::set_var("WHYCODES_GITHUB_API_BASE", base) };
        Self { prev, _lock: lock }
    }
}

impl Drop for ApiBaseGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var("WHYCODES_GITHUB_API_BASE", v),
                None => std::env::remove_var("WHYCODES_GITHUB_API_BASE"),
            }
        }
    }
}

fn spawn_json_server(status: &str, body: &str, n: usize) -> std::net::SocketAddr {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let payload = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    std::thread::spawn(move || {
        for _ in 0..n {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(payload.as_bytes());
            }
        }
    });
    addr
}

#[tokio::test]
async fn execute_create_list_view_close_reopen_comment_on_loopback() {
    let addr = spawn_json_server("200 OK", r#"{"number":1}"#, 8);
    let base = format!("http://{addr}");
    let tool = GithubIssueTool;
    assert!(!tool.description().is_empty());
    let ctx = ToolContext::new(".");
    let _g = ApiBaseGuard::set(&base);
    let create = tool
        .execute(
            json!({
                "action": "create",
                "owner": "o",
                "repo": "r",
                "token": "t",
                "title": "Bug",
                "body": "details",
                "labels": ["bug"]
            }),
            &ctx,
        )
        .await;
    assert!(!create.is_error, "{}", create.content);

    let list = tool
        .execute(
            json!({
                "action": "list",
                "owner": "o",
                "repo": "r",
                "token": "t",
                "state": "all",
                "per_page": 200
            }),
            &ctx,
        )
        .await;
    assert!(!list.is_error, "{}", list.content);

    let view = tool
        .execute(
            json!({
                "action": "view",
                "owner": "o",
                "repo": "r",
                "token": "t",
                "issue_number": 1
            }),
            &ctx,
        )
        .await;
    assert!(!view.is_error, "{}", view.content);

    let close = tool
        .execute(
            json!({
                "action": "close",
                "owner": "o",
                "repo": "r",
                "token": "t",
                "issue_number": 1
            }),
            &ctx,
        )
        .await;
    assert!(!close.is_error, "{}", close.content);

    let reopen = tool
        .execute(
            json!({
                "action": "reopen",
                "owner": "o",
                "repo": "r",
                "token": "t",
                "issue_number": 1
            }),
            &ctx,
        )
        .await;
    assert!(!reopen.is_error, "{}", reopen.content);

    let comment = tool
        .execute(
            json!({
                "action": "comment",
                "owner": "o",
                "repo": "r",
                "token": "t",
                "issue_number": 1,
                "body": "ship it"
            }),
            &ctx,
        )
        .await;
    assert!(!comment.is_error, "{}", comment.content);
}

#[tokio::test]
async fn execute_missing_token_and_api_error() {
    {
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("GITHUB_TOKEN");
        let prev_gh = std::env::var_os("GH_TOKEN");
        let prev_skip = std::env::var_os("WHYCODES_TEST_SKIP_GH_AUTH");
        let prev_skip_git = std::env::var_os("WHYCODES_TEST_SKIP_GIT_CREDENTIAL");
        let prev_hosts = std::env::var_os("WHYCODES_TEST_GH_HOSTS_TOKEN");
        unsafe {
            std::env::remove_var("GITHUB_TOKEN");
            std::env::remove_var("GH_TOKEN");
            std::env::set_var("WHYCODES_TEST_SKIP_GH_AUTH", "1");
            std::env::set_var("WHYCODES_TEST_SKIP_GIT_CREDENTIAL", "1");
            std::env::remove_var("WHYCODES_TEST_GH_HOSTS_TOKEN");
        }
        let missing = GithubIssueTool::new()
            .execute(
                json!({"action": "list", "owner": "o", "repo": "r"}),
                &ToolContext::new("."),
            )
            .await;
        assert!(missing.is_error, "{}", missing.content);
        assert!(missing.content.contains("token"), "{}", missing.content);
        unsafe {
            match prev {
                Some(v) => std::env::set_var("GITHUB_TOKEN", v),
                None => std::env::remove_var("GITHUB_TOKEN"),
            }
            match prev_gh {
                Some(v) => std::env::set_var("GH_TOKEN", v),
                None => std::env::remove_var("GH_TOKEN"),
            }
            match prev_skip {
                Some(v) => std::env::set_var("WHYCODES_TEST_SKIP_GH_AUTH", v),
                None => std::env::remove_var("WHYCODES_TEST_SKIP_GH_AUTH"),
            }
            match prev_skip_git {
                Some(v) => std::env::set_var("WHYCODES_TEST_SKIP_GIT_CREDENTIAL", v),
                None => std::env::remove_var("WHYCODES_TEST_SKIP_GIT_CREDENTIAL"),
            }
            match prev_hosts {
                Some(v) => std::env::set_var("WHYCODES_TEST_GH_HOSTS_TOKEN", v),
                None => std::env::remove_var("WHYCODES_TEST_GH_HOSTS_TOKEN"),
            }
        }
    }

    let addr = spawn_json_server("404 Not Found", "missing", 1);
    let base = format!("http://{addr}");
    let _g2 = ApiBaseGuard::set(&base);
    let err = GithubIssueTool::new()
        .execute(
            json!({
                "action": "list",
                "owner": "o",
                "repo": "r",
                "token": "t"
            }),
            &ToolContext::new("."),
        )
        .await;
    assert!(err.is_error, "{}", err.content);
    assert!(err.content.contains("GitHub API error"), "{}", err.content);

    drop(_g2);

    let mut ctx = ToolContext::new(".");
    ctx.network = whycodes_core::NetworkPolicy {
        allowlist: vec!["example.com".into()],
        denylist: vec![],
    };
    let blocked = GithubIssueTool::new()
        .execute(
            json!({"action": "list", "owner": "o", "repo": "r", "token": "t"}),
            &ctx,
        )
        .await;
    assert!(blocked.is_error, "{}", blocked.content);

    let bad_token = GithubIssueTool::new()
        .execute(
            json!({
                "action": "list",
                "owner": "o",
                "repo": "r",
                "token": "bad\ntoken"
            }),
            &ToolContext::new("."),
        )
        .await;
    assert!(bad_token.is_error, "{}", bad_token.content);
    assert!(
        bad_token.content.contains("headers"),
        "{}",
        bad_token.content
    );
}

#[tokio::test]
async fn execute_surfaces_connect_error() {
    let _g = ApiBaseGuard::set("http://127.0.0.1:1");
    let err = GithubIssueTool
        .execute(
            json!({
                "action": "list",
                "owner": "o",
                "repo": "r",
                "token": "t"
            }),
            &ToolContext::new("."),
        )
        .await;
    assert!(err.is_error, "{}", err.content);
    assert!(
        err.content.contains("GitHub API request failed") || err.content.contains("error"),
        "{}",
        err.content
    );
}
