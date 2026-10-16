use super::*;
use std::process::Command;

fn restore_var(key: &str, prev: Option<std::ffi::OsString>) {
    unsafe {
        match prev {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }
}

/// Holds `ENV_LOCK`, points the API base at a stub and supplies a token.
struct EnvGuard {
    prev: Vec<(&'static str, Option<std::ffi::OsString>)>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

const KEYS: [&str; 7] = [
    "WHYCODES_GITHUB_API_BASE",
    "GITHUB_TOKEN",
    "GH_TOKEN",
    "WHYCODES_TEST_SKIP_GH_AUTH",
    "WHYCODES_TEST_SKIP_GIT_CREDENTIAL",
    "WHYCODES_TEST_GH_HOSTS_TOKEN",
    "WHYCODES_TEST_GH_AUTH_TOKEN",
];

impl EnvGuard {
    fn set(base: &str, token: Option<&str>) -> Self {
        let lock = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = KEYS.iter().map(|k| (*k, std::env::var_os(k))).collect();
        unsafe {
            for key in KEYS {
                std::env::remove_var(key);
            }
            std::env::set_var("WHYCODES_GITHUB_API_BASE", base);
            std::env::set_var("WHYCODES_TEST_SKIP_GH_AUTH", "1");
            std::env::set_var("WHYCODES_TEST_SKIP_GIT_CREDENTIAL", "1");
            if let Some(t) = token {
                std::env::set_var("GITHUB_TOKEN", t);
            }
        }
        Self { prev, _lock: lock }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, prev) in self.prev.drain(..) {
            restore_var(key, prev);
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

fn git(dir: &std::path::Path, args: &[&str]) {
    assert!(
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap()
            .success(),
        "git {args:?}"
    );
}

fn repo_with_origin(url: &str) -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().unwrap();
    git(dir.path(), &["init", "-q"]);
    git(dir.path(), &["remote", "add", "origin", url]);
    dir
}

fn ctx_for(dir: &tempfile::TempDir) -> ToolContext {
    ToolContext::new(dir.path().to_string_lossy())
}

#[test]
fn target_parsing_accepts_numbers_and_pr_diff_only() {
    assert_eq!(parse_target(Kind::Pr, ""), Ok(Target::List));
    assert_eq!(parse_target(Kind::Issue, " / "), Ok(Target::List));
    assert_eq!(parse_target(Kind::Pr, "42"), Ok(Target::View(42)));
    assert_eq!(parse_target(Kind::Issue, "/7/"), Ok(Target::View(7)));
    assert_eq!(parse_target(Kind::Pr, "42/diff"), Ok(Target::Diff(42)));
    let nan = parse_target(Kind::Pr, "abc").unwrap_err();
    assert!(nan.contains("pr://<number>"), "{nan}");
    let issue_diff = parse_target(Kind::Issue, "7/diff").unwrap_err();
    assert!(issue_diff.contains("Use issue://7."), "{issue_diff}");
    let pr_other = parse_target(Kind::Pr, "7/files").unwrap_err();
    assert!(pr_other.contains("or pr://<number>/diff"), "{pr_other}");
    assert_eq!(Kind::Issue.scheme(), "issue");
}

#[test]
fn remote_url_forms_map_to_owner_and_repo() {
    let want = Some(("o".to_string(), "r".to_string()));
    for url in [
        "https://github.com/o/r.git",
        "https://github.com/o/r",
        "https://github.com/o/r/",
        "http://user:tok@github.com/o/r.git",
        "git@github.com:o/r.git",
        "ssh://git@github.com/o/r.git",
        "github.com/o/r",
    ] {
        assert_eq!(parse_github_remote(url), want, "{url}");
    }
    for url in [
        "https://gitlab.com/o/r.git",
        "https://github.com/o",
        "https://github.com/o/r/extra",
        "https://github.com",
        "https://mygithub.com/o/r",
        "https://github.com-x/o/r",
        "",
    ] {
        assert_eq!(parse_github_remote(url), None, "{url}");
    }
}

#[tokio::test]
async fn remote_lookup_reports_git_failures_and_non_github_origins() {
    let io = std::io::Error::other("no git");
    let err = github_remote_from(Err(io)).unwrap_err();
    assert!(err.contains("Cannot run git"), "{err}");

    let no_repo = tempfile::TempDir::new().unwrap();
    let err = github_remote(&no_repo.path().to_string_lossy()).unwrap_err();
    assert!(err.contains("No `origin` remote"), "{err}");

    let gitlab = repo_with_origin("https://gitlab.com/o/r.git");
    let err = github_remote(&gitlab.path().to_string_lossy()).unwrap_err();
    assert!(err.contains("not a github.com repository"), "{err}");

    let github = repo_with_origin("git@github.com:o/r.git");
    assert_eq!(
        github_remote(&github.path().to_string_lossy()),
        Ok(("o".to_string(), "r".to_string()))
    );

    let joined = remote_blocking(Box::new(|| Ok(("a".into(), "b".into())))).await;
    assert_eq!(joined, Ok(("a".to_string(), "b".to_string())));
    let panicked = remote_blocking(Box::new(|| panic!("boom"))).await;
    assert!(
        panicked.unwrap_err().contains("background task failed"),
        "join error surfaces"
    );
}

#[tokio::test]
async fn errors_before_any_request() {
    let _g = EnvGuard::set("http://127.0.0.1:9", Some("t"));
    let repo = repo_with_origin("https://github.com/o/r.git");
    let ctx = ctx_for(&repo);

    let nan = read_github(Kind::Pr, "x", &ctx).await;
    assert!(nan.is_error);
    assert!(nan.content.contains("needs a number"), "{}", nan.content);

    let plain = tempfile::TempDir::new().unwrap();
    let no_remote = read_github(Kind::Issue, "1", &ctx_for(&plain)).await;
    assert!(no_remote.is_error);
    assert!(
        no_remote.content.contains("github_issue"),
        "{}",
        no_remote.content
    );

    let mut blocked = ctx_for(&repo);
    blocked.network = whycodes_core::NetworkPolicy {
        allowlist: vec!["example.com".into()],
        denylist: vec![],
    };
    let denied = read_github(Kind::Pr, "", &blocked).await;
    assert!(denied.is_error);
    assert!(
        denied.content.contains("Network policy"),
        "{}",
        denied.content
    );
    drop(_g);

    let _g = EnvGuard::set("http://127.0.0.1:9", None);
    let no_token = read_github(Kind::Pr, "1", &ctx).await;
    assert!(no_token.is_error);
    assert_eq!(no_token.content, api::missing_token_message());
}

#[tokio::test]
async fn pr_listing_view_and_diff_on_loopback() {
    let repo = repo_with_origin("https://github.com/o/r.git");
    let ctx = ctx_for(&repo);

    let list_body = r#"[
        {"number":7,"title":"Add x","draft":false,"user":{"login":"ann"},"head":{"ref":"feat"},"base":{"ref":"main"}},
        {"number":8,"title":"WIP","draft":true,"user":{"login":"bob"},"head":{"ref":"wip"},"base":{"ref":"main"}}
    ]"#;
    let addr = spawn_server("200 OK", list_body, 1);
    let g = EnvGuard::set(&format!("http://{addr}"), Some("t"));
    let list = read_github(Kind::Pr, "", &ctx).await;
    assert!(!list.is_error, "{}", list.content);
    assert!(
        list.content
            .starts_with("Open pull requests in o/r (first page;"),
        "{}",
        list.content
    );
    assert!(
        list.content.contains("- #7 Add x — ann, feat → main\n"),
        "{}",
        list.content
    );
    assert!(
        list.content
            .contains("- #8 WIP [draft] — bob, wip → main\n"),
        "{}",
        list.content
    );
    drop(g);

    let addr = spawn_server("200 OK", "[]", 1);
    let g = EnvGuard::set(&format!("http://{addr}"), Some("t"));
    let empty = read_github(Kind::Pr, "/", &ctx).await;
    assert_eq!(empty.content, "No open pull requests in o/r.");
    drop(g);

    let view_body = r#"{"number":7,"title":"Add x","state":"open","draft":true,
        "user":{"login":"ann"},"head":{"label":"ann:feat"},"base":{"ref":"main"},
        "created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-02T00:00:00Z",
        "additions":10,"deletions":2,"changed_files":3,"commits":1,
        "labels":[{"name":"bug"},{"name":"docs"}],"html_url":"https://github.com/o/r/pull/7",
        "body":"  Why this change.  "}"#;
    let addr = spawn_server("200 OK", view_body, 1);
    let g = EnvGuard::set(&format!("http://{addr}"), Some("t"));
    let view = read_github(Kind::Pr, "7", &ctx).await;
    assert!(!view.is_error, "{}", view.content);
    assert_eq!(
        view.content,
        "# pr://7 — Add x\n\
         state: open (draft) · author: ann · ann:feat → main · created 2026-01-01T00:00:00Z · updated 2026-01-02T00:00:00Z\n\
         changes: +10 −2 in 3 file(s) · commits: 1\n\
         labels: bug, docs\n\
         url: https://github.com/o/r/pull/7\n\
         diff: `read pr://7/diff`\n\n\
         Why this change."
    );
    drop(g);

    let addr = spawn_server("200 OK", "diff --git a/x b/x\n+added\n", 1);
    let g = EnvGuard::set(&format!("http://{addr}"), Some("t"));
    let diff = read_github(Kind::Pr, "7/diff", &ctx).await;
    assert!(!diff.is_error, "{}", diff.content);
    assert_eq!(
        diff.content,
        "# pr://7/diff\n\ndiff --git a/x b/x\n+added\n"
    );
    drop(g);
}

#[tokio::test]
async fn issue_listing_and_view_on_loopback() {
    let repo = repo_with_origin("https://github.com/o/r.git");
    let ctx = ctx_for(&repo);

    let list_body = r#"[
        {"number":3,"title":"Crash","user":{"login":"ann"},"labels":[{"name":"bug"},{"name":"p1"}]},
        {"number":4,"title":"A PR","user":{"login":"bob"},"labels":[],"pull_request":{"url":"x"}},
        {"number":5,"title":"Idea","user":{"login":"cy"}}
    ]"#;
    let addr = spawn_server("200 OK", list_body, 1);
    let g = EnvGuard::set(&format!("http://{addr}"), Some("t"));
    let list = read_github(Kind::Issue, "", &ctx).await;
    assert!(!list.is_error, "{}", list.content);
    assert_eq!(
        list.content,
        "Open issues in o/r (first page; read one with `read issue://<number>`):\n\
         - #3 Crash [bug, p1] — ann\n\
         - #5 Idea — cy\n"
    );
    drop(g);

    let addr = spawn_server("200 OK", "{}", 1);
    let g = EnvGuard::set(&format!("http://{addr}"), Some("t"));
    let empty = read_github(Kind::Issue, "", &ctx).await;
    assert_eq!(empty.content, "No open issues in o/r.");
    drop(g);

    let view_body = r#"{"number":4,"title":"A PR","state":"closed","user":{"login":"bob"},
        "created_at":"c","updated_at":"u","comments":2,"html_url":"https://github.com/o/r/pull/4",
        "pull_request":{"url":"x"},"body":null}"#;
    let addr = spawn_server("200 OK", view_body, 1);
    let g = EnvGuard::set(&format!("http://{addr}"), Some("t"));
    let view = read_github(Kind::Issue, "4", &ctx).await;
    assert!(!view.is_error, "{}", view.content);
    assert_eq!(
        view.content,
        "# issue://4 — A PR\n\
         state: closed · author: bob · created c · updated u · comments: 2\n\
         url: https://github.com/o/r/pull/4\n\
         note: #4 is a pull request; `read pr://4` shows its branches and diff.\n\n\
         (no description)"
    );
    drop(g);
}

#[tokio::test]
async fn transport_and_response_failures_are_errors() {
    let repo = repo_with_origin("https://github.com/o/r.git");
    let ctx = ctx_for(&repo);

    let addr = spawn_server("404 Not Found", r#"{"message":"Not Found"}"#, 1);
    let g = EnvGuard::set(&format!("http://{addr}"), Some("t"));
    let missing = read_github(Kind::Issue, "999", &ctx).await;
    assert!(missing.is_error);
    assert!(
        missing
            .content
            .starts_with("GitHub API error (404): {\"message\":\"Not Found\"}"),
        "{}",
        missing.content
    );
    drop(g);

    let addr = spawn_server("200 OK", "not-json", 1);
    let g = EnvGuard::set(&format!("http://{addr}"), Some("t"));
    let garbage = read_github(Kind::Pr, "1", &ctx).await;
    assert!(garbage.is_error);
    assert!(
        garbage.content.contains("Unexpected GitHub response"),
        "{}",
        garbage.content
    );
    drop(g);

    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = closed.local_addr().unwrap();
    drop(closed);
    let g = EnvGuard::set(&format!("http://{addr}"), Some("t"));
    let refused = read_github(Kind::Pr, "1/diff", &ctx).await;
    assert!(refused.is_error);
    assert!(
        refused.content.contains("GitHub API request failed"),
        "{}",
        refused.content
    );
    drop(g);

    let g = EnvGuard::set("http://127.0.0.1:9", Some("bad\ntoken"));
    let headers = read_github(Kind::Pr, "1", &ctx).await;
    assert!(headers.is_error);
    assert!(
        headers.content.contains("Invalid token"),
        "{}",
        headers.content
    );
    drop(g);
}

#[test]
fn row_helpers_tolerate_missing_fields() {
    let v = serde_json::json!({});
    assert_eq!(pr_row(&v), "#null ? — ?, ? → ?");
    assert_eq!(issue_row(&v), "#null ? — ?");
    assert_eq!(body(&v), "(no description)");
    assert_eq!(labels(&v), "");
    assert_eq!(text(&v, "x"), "?");
    assert!(ok("x").content == "x" && !ok("x").is_error);
}
