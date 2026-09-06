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
    let prev_skip_git = std::env::var_os("WHYCODES_TEST_SKIP_GIT_CREDENTIAL");
    let prev_hosts = std::env::var_os("WHYCODES_TEST_GH_HOSTS_TOKEN");
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
        std::env::set_var("WHYCODES_TEST_SKIP_GIT_CREDENTIAL", "1");
        std::env::remove_var("WHYCODES_TEST_GH_HOSTS_TOKEN");
    }
    assert_eq!(resolve_token(None), None);
    restore_var("GITHUB_TOKEN", prev_github);
    restore_var("GH_TOKEN", prev_gh);
    restore_var("WHYCODES_TEST_SKIP_GH_AUTH", prev_skip);
    restore_var("WHYCODES_TEST_GH_AUTH_TOKEN", prev_mock);
    restore_var("WHYCODES_TEST_SKIP_GIT_CREDENTIAL", prev_skip_git);
    restore_var("WHYCODES_TEST_GH_HOSTS_TOKEN", prev_hosts);
}

#[test]
fn missing_token_message_mentions_gh_login() {
    let msg = missing_token_message();
    assert!(msg.contains("gh auth login"), "{msg}");
    assert!(msg.contains("GITHUB_TOKEN"), "{msg}");
    assert!(msg.contains("GH_TOKEN"), "{msg}");
    assert!(msg.contains("git credential"), "{msg}");
}

#[test]
fn resolve_token_falls_back_to_hosts_file_then_git_credential() {
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev_github = std::env::var_os("GITHUB_TOKEN");
    let prev_gh = std::env::var_os("GH_TOKEN");
    let prev_skip = std::env::var_os("WHYCODES_TEST_SKIP_GH_AUTH");
    let prev_mock = std::env::var_os("WHYCODES_TEST_GH_AUTH_TOKEN");
    let prev_skip_git = std::env::var_os("WHYCODES_TEST_SKIP_GIT_CREDENTIAL");
    let prev_hosts = std::env::var_os("WHYCODES_TEST_GH_HOSTS_TOKEN");
    let prev_git = std::env::var_os("WHYCODES_TEST_GIT_CREDENTIAL_TOKEN");
    unsafe {
        std::env::remove_var("GITHUB_TOKEN");
        std::env::remove_var("GH_TOKEN");
        std::env::set_var("WHYCODES_TEST_SKIP_GH_AUTH", "1");
        std::env::remove_var("WHYCODES_TEST_GH_AUTH_TOKEN");
        std::env::remove_var("WHYCODES_TEST_SKIP_GIT_CREDENTIAL");
        std::env::set_var("WHYCODES_TEST_GH_HOSTS_TOKEN", "from-hosts");
        std::env::set_var("WHYCODES_TEST_GIT_CREDENTIAL_TOKEN", "from-git");
    }
    assert_eq!(resolve_token(None), Some("from-hosts".into()));
    unsafe {
        std::env::remove_var("WHYCODES_TEST_GH_HOSTS_TOKEN");
    }
    assert_eq!(resolve_token(None), Some("from-git".into()));
    unsafe {
        std::env::set_var("WHYCODES_TEST_SKIP_GIT_CREDENTIAL", "1");
    }
    assert_eq!(resolve_token(None), None);
    restore_var("GITHUB_TOKEN", prev_github);
    restore_var("GH_TOKEN", prev_gh);
    restore_var("WHYCODES_TEST_SKIP_GH_AUTH", prev_skip);
    restore_var("WHYCODES_TEST_GH_AUTH_TOKEN", prev_mock);
    restore_var("WHYCODES_TEST_SKIP_GIT_CREDENTIAL", prev_skip_git);
    restore_var("WHYCODES_TEST_GH_HOSTS_TOKEN", prev_hosts);
    restore_var("WHYCODES_TEST_GIT_CREDENTIAL_TOKEN", prev_git);
}

#[test]
fn parse_gh_hosts_yaml_classic_and_multi_user() {
    let classic = "\
github.com:
    oauth_token: gho_classic
    user: octocat
    git_protocol: https
";
    assert_eq!(
        parse_gh_hosts_yaml(classic, "github.com").as_deref(),
        Some("gho_classic")
    );

    let quoted = "\
github.com:
    oauth_token: \"gho_quoted\"
";
    assert_eq!(
        parse_gh_hosts_yaml(quoted, "github.com").as_deref(),
        Some("gho_quoted")
    );

    let multi = "\
github.com:
    users:
        alice:
            oauth_token: gho_alice
        bob:
            oauth_token: gho_bob
    user: bob
";
    assert_eq!(
        parse_gh_hosts_yaml(multi, "github.com").as_deref(),
        Some("gho_bob")
    );

    let other = "\
enterprise.example:
    oauth_token: gho_ent
github.com:
    oauth_token: gho_dotcom
";
    assert_eq!(
        parse_gh_hosts_yaml(other, "enterprise.example").as_deref(),
        Some("gho_ent")
    );
    assert!(parse_gh_hosts_yaml(classic, "nope.example").is_none());
    assert!(parse_gh_hosts_yaml("# empty\n", "github.com").is_none());
}

#[test]
fn parse_git_credential_fill_reads_password() {
    let text = "\
protocol=https
host=github.com
username=git
password=gho_from_gcm
";
    assert_eq!(
        parse_git_credential_fill(text).as_deref(),
        Some("gho_from_gcm")
    );
    assert!(parse_git_credential_fill("username=git\n").is_none());
    assert!(parse_git_credential_fill("").is_none());
}

#[test]
fn gh_hosts_path_prefers_config_dir_then_platform() {
    let custom = gh_hosts_path_from(
        Some(PathBuf::from("/custom/gh")),
        Some(PathBuf::from("/appdata")),
        Some(PathBuf::from("/xdg")),
        Some(PathBuf::from("/home/u")),
    );
    assert_eq!(
        custom.as_deref(),
        Some(std::path::Path::new("/custom/gh/hosts.yml"))
    );

    let appdata = gh_hosts_path_from(
        None,
        Some(PathBuf::from("/appdata")),
        Some(PathBuf::from("/xdg")),
        Some(PathBuf::from("/home/u")),
    );
    assert_eq!(
        appdata.as_deref(),
        Some(std::path::Path::new("/appdata/GitHub CLI/hosts.yml"))
    );

    let xdg = gh_hosts_path_from(
        None,
        None,
        Some(PathBuf::from("/xdg")),
        Some(PathBuf::from("/home/u")),
    );
    assert_eq!(
        xdg.as_deref(),
        Some(std::path::Path::new("/xdg/gh/hosts.yml"))
    );

    let home = gh_hosts_path_from(None, None, None, Some(PathBuf::from("/home/u")));
    assert_eq!(
        home.as_deref(),
        Some(std::path::Path::new("/home/u/.config/gh/hosts.yml"))
    );
    assert!(gh_hosts_path_from(None, None, None, None).is_none());
    assert!(gh_hosts_path_from(Some(PathBuf::from("")), None, None, None).is_none());
}

#[test]
fn well_known_gh_paths_include_install_layouts() {
    let paths = well_known_gh_paths_from(
        Some(PathBuf::from(r"C:\Program Files")),
        Some(PathBuf::from(r"C:\Users\me\AppData\Local")),
        Some(PathBuf::from(r"C:\Users\me")),
        Some(PathBuf::from("/home/me")),
    );
    let joined = paths
        .iter()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("GitHub CLI/gh.exe"), "{joined}");
    assert!(
        joined.contains("scoop/apps/gh/current/bin/gh.exe"),
        "{joined}"
    );
    assert!(joined.contains("/home/me/.local/bin/gh"), "{joined}");
    assert!(joined.contains("/opt/homebrew/bin/gh"), "{joined}");
}

#[test]
fn yaml_helpers_ignore_users_key_and_empty_values() {
    assert!(yaml_scalar_key("users:", "user").is_none());
    assert!(yaml_scalar_key("oauth_token:", "oauth_token").is_none());
    assert_eq!(
        yaml_scalar_key("oauth_token: 'gho_s'", "oauth_token").as_deref(),
        Some("gho_s")
    );
    assert!(yaml_map_key("oauth_token: gho_x").is_none());
    assert!(yaml_map_key(":").is_none());
    assert_eq!(unquote("  plain  "), "plain");
    assert!(host_heading("github.com:", "GitHub.com"));
    assert!(!host_heading("not a heading", "github.com"));
}

#[test]
fn gh_hosts_file_token_from_disk_reads_config_dir() {
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("hosts.yml"),
        "github.com:\n    oauth_token: gho_disk\n",
    )
    .unwrap();
    let prev_dir = std::env::var_os("GH_CONFIG_DIR");
    let prev_host = std::env::var_os("GH_HOST");
    unsafe {
        std::env::set_var("GH_CONFIG_DIR", dir.path());
        std::env::remove_var("GH_HOST");
    }
    assert_eq!(gh_hosts_file_token_from_disk().as_deref(), Some("gho_disk"));
    restore_var("GH_CONFIG_DIR", prev_dir);
    restore_var("GH_HOST", prev_host);
}

#[test]
fn gh_hosts_file_token_from_disk_missing_file_is_none() {
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let prev_dir = std::env::var_os("GH_CONFIG_DIR");
    unsafe {
        std::env::set_var("GH_CONFIG_DIR", dir.path().join("missing-gh-config"));
    }
    assert!(gh_hosts_file_token_from_disk().is_none());
    restore_var("GH_CONFIG_DIR", prev_dir);
}

#[test]
fn gh_auth_cli_probe_does_not_panic() {
    if let Some(token) = gh_auth_token_from_cli() {
        assert!(!token.is_empty());
    }
}

#[test]
fn git_credential_cli_probe_does_not_panic() {
    if let Some(token) = git_credential_token_from_cli() {
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
    let (status, text) = make_request(&client, reqwest::Method::GET, "repos/o/r", "token", None)
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
