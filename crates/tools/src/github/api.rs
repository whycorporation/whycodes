/// Shared GitHub REST API helpers for tools (issues, PRs, etc.)
use std::env;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use reqwest::header::{HeaderMap, HeaderValue};
use whycodes_core::network::NetworkPolicy;

const GITHUB_API_BASE: &str = "https://api.github.com";
const GH_AUTH_TIMEOUT: Duration = Duration::from_secs(2);

/// Resolve a GitHub token without prompting.
///
/// Order: explicit tool arg → `GITHUB_TOKEN` → `GH_TOKEN` → `gh auth token`.
/// Git credential helpers are skipped: they can open a GUI / hang on Windows.
pub fn resolve_token(explicit_token: Option<&str>) -> Option<String> {
    nonempty(explicit_token)
        .or_else(env_token)
        .or_else(gh_auth_token)
}

/// User-facing line when [`resolve_token`] returns `None`.
pub fn missing_token_message() -> &'static str {
    "GitHub token not found. Set GITHUB_TOKEN or GH_TOKEN, run `gh auth login`, or pass 'token'."
}

fn nonempty(value: Option<&str>) -> Option<String> {
    nonempty_str(value?)
}

fn nonempty_str(value: &str) -> Option<String> {
    let s = value.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

fn env_token() -> Option<String> {
    ["GITHUB_TOKEN", "GH_TOKEN"]
        .into_iter()
        .find_map(|key| env::var(key).ok().and_then(|s| nonempty_str(&s)))
}

fn gh_auth_token() -> Option<String> {
    #[cfg(test)]
    {
        gh_auth_token_from_test_env()
    }
    #[cfg(not(test))]
    {
        gh_auth_token_from_cli()
    }
}

/// Host `gh` is never spawned from unit tests (CI / developer logins).
#[cfg(test)]
fn gh_auth_token_from_test_env() -> Option<String> {
    if env::var_os("WHYCODES_TEST_SKIP_GH_AUTH").is_some() {
        None
    } else {
        env::var("WHYCODES_TEST_GH_AUTH_TOKEN")
            .ok()
            .and_then(|s| nonempty_str(&s))
    }
}

/// Non-interactive `gh auth token`. Kills the child after [`GH_AUTH_TIMEOUT`].
fn gh_auth_token_from_cli() -> Option<String> {
    let mut child = Command::new("gh")
        .args(["auth", "token"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let mut buf = String::new();
                child.stdout.take()?.read_to_string(&mut buf).ok()?;
                return nonempty_str(&buf);
            }
            Ok(None) => {
                if start.elapsed() >= GH_AUTH_TIMEOUT {
                    if let Err(err) = child.kill() {
                        tracing::debug!(error = %err, "gh auth token: kill after timeout");
                    }
                    if let Err(err) = child.wait() {
                        tracing::debug!(error = %err, "gh auth token: wait after kill");
                    }
                    return None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(err) => {
                tracing::debug!(error = %err, "gh auth token: wait failed");
                return None;
            }
        }
    }
}

/// Build common headers for GitHub API requests (auth, accept, user-agent).
pub fn github_headers(token: &str) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    headers.insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|e| format!("Invalid token: {e}"))?,
    );
    headers.insert(
        "Accept",
        HeaderValue::from_static("application/vnd.github+json"),
    );
    headers.insert(
        "X-GitHub-Api-Version",
        HeaderValue::from_static("2022-11-28"),
    );
    headers.insert("User-Agent", HeaderValue::from_static("whycodes"));
    Ok(headers)
}

/// Build a full GitHub API URL: https://api.github.com/{path}
pub fn api_url(path: &str) -> String {
    format!("{}/{path}", github_api_base())
}

fn github_api_base() -> String {
    #[cfg(test)]
    if let Ok(base) = env::var("WHYCODES_GITHUB_API_BASE")
        && !base.is_empty()
    {
        return base;
    }
    GITHUB_API_BASE.to_string()
}

/// Perform a GitHub REST API request and return the body text.
pub async fn make_request(
    client: &reqwest::Client,
    method: reqwest::Method,
    path: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> Result<(reqwest::StatusCode, String), String> {
    make_request_with_policy(
        client,
        method,
        path,
        token,
        body,
        &NetworkPolicy::unrestricted(),
    )
    .await
}

/// Like [`make_request`], but enforces the session network allow/deny policy.
pub async fn make_request_with_policy(
    client: &reqwest::Client,
    method: reqwest::Method,
    path: &str,
    token: &str,
    body: Option<serde_json::Value>,
    network: &NetworkPolicy,
) -> Result<(reqwest::StatusCode, String), String> {
    let headers = github_headers(token)?;
    let url = api_url(path);
    network.check_url(&url)?;

    let mut req = client.request(method, &url).headers(headers);
    if let Some(b) = body {
        req = req.json(&b);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("GitHub API request failed: {e}"))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("Failed to read GitHub API response: {e}"))?;

    Ok((status, text))
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;

    #[test]
    fn explicit_token_takes_precedence_without_environment_access() {
        assert_eq!(
            resolve_token(Some("explicit")),
            Some("explicit".to_string())
        );
    }

    #[test]
    fn headers_include_github_requirements() {
        let headers = github_headers("secret").expect("valid token should build headers");

        assert_eq!(headers["Authorization"], "Bearer secret");
        assert_eq!(headers["Accept"], "application/vnd.github+json");
        assert_eq!(headers["X-GitHub-Api-Version"], "2022-11-28");
        assert_eq!(headers["User-Agent"], "whycodes");
    }

    #[test]
    fn invalid_header_token_is_rejected() {
        let error = github_headers("bad\ntoken").expect_err("newline must be rejected");
        assert!(error.starts_with("Invalid token:"));
    }

    #[test]
    fn api_url_preserves_the_requested_path() {
        assert_eq!(
            api_url("repos/whycodes/whycodes/issues?state=open"),
            "https://api.github.com/repos/whycodes/whycodes/issues?state=open"
        );
    }

    #[tokio::test]
    async fn policy_rejection_happens_before_network_io() {
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("WHYCODES_GITHUB_API_BASE");
        unsafe { std::env::remove_var("WHYCODES_GITHUB_API_BASE") };
        let policy = NetworkPolicy {
            allowlist: vec!["example.com".to_string()],
            denylist: Vec::new(),
        };
        let error = make_request_with_policy(
            &reqwest::Client::new(),
            reqwest::Method::GET,
            "repos/owner/repo",
            "token",
            None,
            &policy,
        )
        .await
        .expect_err("GitHub should be blocked by policy");

        assert!(error.contains("Network policy blocked host"), "{error}");
        unsafe {
            match prev {
                Some(v) => std::env::set_var("WHYCODES_GITHUB_API_BASE", v),
                None => std::env::remove_var("WHYCODES_GITHUB_API_BASE"),
            }
        }
    }

    fn restore_var(key: &str, prev: Option<std::ffi::OsString>) {
        unsafe {
            match prev {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }

    #[test]
    fn resolve_token_falls_back_to_env_and_override_base() {
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_token = std::env::var_os("GITHUB_TOKEN");
        let prev_gh = std::env::var_os("GH_TOKEN");
        let prev_base = std::env::var_os("WHYCODES_GITHUB_API_BASE");
        let prev_skip = std::env::var_os("WHYCODES_TEST_SKIP_GH_AUTH");
        unsafe {
            std::env::set_var("GITHUB_TOKEN", "from-env");
            std::env::remove_var("GH_TOKEN");
            std::env::set_var("WHYCODES_TEST_SKIP_GH_AUTH", "1");
            std::env::set_var("WHYCODES_GITHUB_API_BASE", "http://127.0.0.1:9");
        }
        assert_eq!(resolve_token(None), Some("from-env".into()));
        assert_eq!(resolve_token(Some("")), Some("from-env".into()));
        assert_eq!(resolve_token(Some("  ")), Some("from-env".into()));
        assert_eq!(api_url("repos/x/y"), "http://127.0.0.1:9/repos/x/y");
        restore_var("GITHUB_TOKEN", prev_token);
        restore_var("GH_TOKEN", prev_gh);
        restore_var("WHYCODES_GITHUB_API_BASE", prev_base);
        restore_var("WHYCODES_TEST_SKIP_GH_AUTH", prev_skip);
    }

    #[test]
    fn resolve_token_falls_back_to_gh_token_then_cli_mock() {
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_github = std::env::var_os("GITHUB_TOKEN");
        let prev_gh = std::env::var_os("GH_TOKEN");
        let prev_skip = std::env::var_os("WHYCODES_TEST_SKIP_GH_AUTH");
        let prev_mock = std::env::var_os("WHYCODES_TEST_GH_AUTH_TOKEN");
        unsafe {
            std::env::remove_var("GITHUB_TOKEN");
            std::env::remove_var("WHYCODES_TEST_SKIP_GH_AUTH");
            std::env::set_var("GH_TOKEN", "from-gh-token");
            std::env::set_var("WHYCODES_TEST_GH_AUTH_TOKEN", "from-cli");
        }
        assert_eq!(resolve_token(None), Some("from-gh-token".into()));
        unsafe {
            std::env::remove_var("GH_TOKEN");
        }
        assert_eq!(resolve_token(None), Some("from-cli".into()));
        unsafe {
            std::env::set_var("WHYCODES_TEST_SKIP_GH_AUTH", "1");
        }
        assert_eq!(resolve_token(None), None);
        restore_var("GITHUB_TOKEN", prev_github);
        restore_var("GH_TOKEN", prev_gh);
        restore_var("WHYCODES_TEST_SKIP_GH_AUTH", prev_skip);
        restore_var("WHYCODES_TEST_GH_AUTH_TOKEN", prev_mock);
    }

    #[test]
    fn missing_token_message_mentions_gh_login() {
        let msg = missing_token_message();
        assert!(msg.contains("gh auth login"), "{msg}");
        assert!(msg.contains("GITHUB_TOKEN"), "{msg}");
        assert!(msg.contains("GH_TOKEN"), "{msg}");
    }

    #[test]
    fn gh_auth_cli_probe_does_not_panic() {
        if let Some(token) = gh_auth_token_from_cli() {
            assert!(!token.is_empty());
        }
    }

    #[tokio::test]
    async fn make_request_hits_loopback_with_and_without_body() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            for _ in 0..2 {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut buf = [0u8; 4096];
                    let _ = stream.read(&mut buf);
                    let body = r#"{"ok":true}"#;
                    let resp = format!(
                        "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(resp.as_bytes());
                }
            }
        });
        let prev = std::env::var_os("WHYCODES_GITHUB_API_BASE");
        unsafe {
            std::env::set_var("WHYCODES_GITHUB_API_BASE", format!("http://{addr}"));
        }
        let client = reqwest::Client::new();
        let (status, text) =
            make_request(&client, reqwest::Method::GET, "repos/o/r", "token", None)
                .await
                .expect("get");
        assert!(status.is_success());
        assert!(text.contains("ok"));
        let (status, _) = make_request(
            &client,
            reqwest::Method::POST,
            "repos/o/r",
            "token",
            Some(serde_json::json!({"title": "t"})),
        )
        .await
        .expect("post");
        assert_eq!(status.as_u16(), 201);
        unsafe {
            match prev {
                Some(v) => std::env::set_var("WHYCODES_GITHUB_API_BASE", v),
                None => std::env::remove_var("WHYCODES_GITHUB_API_BASE"),
            }
        }
    }
}
