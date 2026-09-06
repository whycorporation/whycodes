use super::*;

#[test]
fn metadata_describes_supported_actions_and_required_repository() {
    let tool = GitHubPrTool::new();
    let parameters = tool.parameters();

    assert_eq!(tool.name(), "github_pr");
    assert_eq!(
        parameters["properties"]["action"]["enum"],
        json!(["create", "list", "view", "merge"])
    );
    assert_eq!(parameters["required"], json!(["action", "owner", "repo"]));
}

#[tokio::test]
async fn execute_rejects_missing_repository_before_token_or_network() {
    let result = GitHubPrTool::new()
        .execute(
            json!({ "action": "list", "owner": "owner" }),
            &ToolContext::new("."),
        )
        .await;

    assert!(result.is_error);
    assert_eq!(result.content, "Both 'owner' and 'repo' are required.");
}

#[tokio::test]
async fn execute_rejects_unknown_action_without_requesting() {
    let result = GitHubPrTool::new()
        .execute(
            json!({
                "action": "close",
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
        "Unknown action 'close'. Valid actions: create, list, view, merge."
    );
}

#[tokio::test]
async fn create_validates_required_fields_before_requesting() {
    let result = GitHubPrTool::create_pr(
        &reqwest::Client::new(),
        "token",
        "owner",
        "repo",
        &json!({ "title": "Title", "head": "feature" }),
    )
    .await;

    assert!(result.is_error);
    assert_eq!(
        result.content,
        "For 'create' action, 'title', 'head', and 'base' are required."
    );
}

#[tokio::test]
async fn view_and_merge_require_a_pull_request_number() {
    let client = reqwest::Client::new();
    let args = json!({});

    let view = GitHubPrTool::view_pr(&client, "token", "owner", "repo", &args).await;
    assert!(view.is_error);
    assert_eq!(view.content, "For 'view' action, 'pr_number' is required.");

    let merge = GitHubPrTool::merge_pr(&client, "token", "owner", "repo", &args).await;
    assert!(merge.is_error);
    assert_eq!(
        merge.content,
        "For 'merge' action, 'pr_number' is required."
    );
}

#[tokio::test]
async fn merge_rejects_an_invalid_method_before_requesting() {
    let result = GitHubPrTool::merge_pr(
        &reqwest::Client::new(),
        "token",
        "owner",
        "repo",
        &json!({ "pr_number": 7, "method": "fast-forward" }),
    )
    .await;

    assert!(result.is_error);
    assert_eq!(
        result.content,
        "Invalid merge method 'fast-forward'. Valid methods: merge, squash, rebase."
    );
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

fn spawn_server(status: &str, body: &str, n: usize) -> std::net::SocketAddr {
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
async fn execute_create_list_view_merge_on_loopback() {
    let addr = spawn_server("200 OK", r#"{"number":7,"title":"ok"}"#, 6);
    let base = format!("http://{addr}");
    let tool = GitHubPrTool;
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
                "title": "PR",
                "head": "feat",
                "base": "main",
                "body": "desc"
            }),
            &ctx,
        )
        .await;
    assert!(!create.is_error, "{}", create.content);
    assert!(create.content.contains("Status:"), "{}", create.content);

    let list = tool
        .execute(
            json!({
                "action": "list",
                "owner": "o",
                "repo": "r",
                "token": "t",
                "state": "closed"
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
                "pr_number": 7
            }),
            &ctx,
        )
        .await;
    assert!(!view.is_error, "{}", view.content);

    let merge = tool
        .execute(
            json!({
                "action": "merge",
                "owner": "o",
                "repo": "r",
                "token": "t",
                "pr_number": 7,
                "method": "squash"
            }),
            &ctx,
        )
        .await;
    assert!(!merge.is_error, "{}", merge.content);
}

#[tokio::test]
async fn execute_missing_token_and_non_json_error() {
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
        let missing = GitHubPrTool::new()
            .execute(
                json!({"action": "list", "owner": "o", "repo": "r"}),
                &ToolContext::new("."),
            )
            .await;
        assert!(missing.is_error, "{}", missing.content);
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

    let addr = spawn_server("500 Internal Server Error", "not-json", 1);
    let base = format!("http://{addr}");
    let _g2 = ApiBaseGuard::set(&base);
    let err = GitHubPrTool::new()
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
    assert!(err.content.contains("not-json"), "{}", err.content);
    drop(_g2);

    let mut ctx = ToolContext::new(".");
    ctx.network = whycodes_core::NetworkPolicy {
        allowlist: vec!["example.com".into()],
        denylist: vec![],
    };
    let blocked = GitHubPrTool::new()
        .execute(
            json!({"action": "list", "owner": "o", "repo": "r", "token": "t"}),
            &ctx,
        )
        .await;
    assert!(blocked.is_error, "{}", blocked.content);
}

#[tokio::test]
async fn request_errors_and_non_json_success() {
    let addr = spawn_server("200 OK", "plain-text", 4);
    let base = format!("http://{addr}");
    let _g = ApiBaseGuard::set(&base);
    let tool = GitHubPrTool::new();
    let ctx = ToolContext::new(".");
    let create = tool
        .execute(
            json!({
                "action": "create",
                "owner": "o",
                "repo": "r",
                "token": "t",
                "title": "PR",
                "head": "feat",
                "base": "main"
            }),
            &ctx,
        )
        .await;
    assert!(!create.is_error, "{}", create.content);
    assert!(create.content.contains("plain-text"), "{}", create.content);

    drop(_g);
    unsafe { std::env::set_var("WHYCODES_GITHUB_API_BASE", "http://127.0.0.1:1") };
    let _g2 = ApiBaseGuard::set("http://127.0.0.1:1");
    for action in [
        json!({"action":"create","owner":"o","repo":"r","token":"t","title":"t","head":"h","base":"b"}),
        json!({"action":"list","owner":"o","repo":"r","token":"t"}),
        json!({"action":"view","owner":"o","repo":"r","token":"t","pr_number":1}),
        json!({"action":"merge","owner":"o","repo":"r","token":"t","pr_number":1}),
    ] {
        let err = tool.execute(action, &ctx).await;
        assert!(err.is_error, "{}", err.content);
    }
}
