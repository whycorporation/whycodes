/// Shared GitHub REST API helpers for tools (issues, PRs, etc.)
use std::env;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use reqwest::header::{HeaderMap, HeaderValue};
use whycodes_core::network::NetworkPolicy;

const GITHUB_API_BASE: &str = "https://api.github.com";
const GH_AUTH_TIMEOUT: Duration = Duration::from_secs(3);
const GIT_CREDENTIAL_TIMEOUT: Duration = Duration::from_secs(2);

/// Resolve a GitHub token without prompting.
///
/// Order: explicit tool arg → `GITHUB_TOKEN` / `GH_TOKEN` → `gh auth token`
/// → `gh` `hosts.yml` (terminal `gh auth login`) → non-interactive
/// `git credential fill` (Git Credential Manager / stored https creds).
///
/// Never opens a login GUI: `GH_PROMPT_DISABLED=1`, `GIT_TERMINAL_PROMPT=0`,
/// `GCM_INTERACTIVE=never`, and both child processes are killed on timeout.
pub fn resolve_token(explicit_token: Option<&str>) -> Option<String> {
    nonempty(explicit_token)
        .or_else(env_token)
        .or_else(gh_auth_token)
        .or_else(gh_hosts_file_token)
        .or_else(git_credential_token)
}

/// User-facing line when [`resolve_token`] returns `None`.
pub fn missing_token_message() -> &'static str {
    "GitHub token not found. Set GITHUB_TOKEN or GH_TOKEN, run `gh auth login`, \
     or store https credentials for github.com (Git Credential Manager). \
     WhyCodes also reads an existing terminal login (`gh` hosts.yml / `git credential`). \
     SSH-only git remotes do not yield an API token."
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
    let child = spawn_gh_auth_token()?;
    wait_child_stdout(child, GH_AUTH_TIMEOUT, "gh auth token")
}

fn configure_gh_auth_token(cmd: &mut Command) {
    cmd.args(["auth", "token"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("GH_PROMPT_DISABLED", "1");
}

fn spawn_gh_auth_token() -> Option<Child> {
    let mut cmd = Command::new("gh");
    configure_gh_auth_token(&mut cmd);
    match cmd.spawn() {
        Ok(child) => Some(child),
        Err(err) => {
            tracing::debug!(error = %err, "gh not on PATH; trying known locations");
            for path in well_known_gh_paths() {
                if !path.is_file() {
                    continue;
                }
                let mut cmd = Command::new(&path);
                configure_gh_auth_token(&mut cmd);
                match cmd.spawn() {
                    Ok(child) => return Some(child),
                    Err(e) => {
                        tracing::debug!(
                            error = %e,
                            path = %path.display(),
                            "gh spawn failed"
                        );
                    }
                }
            }
            None
        }
    }
}

fn well_known_gh_paths() -> Vec<PathBuf> {
    well_known_gh_paths_from(
        env::var_os("ProgramFiles").map(PathBuf::from),
        env::var_os("LOCALAPPDATA").map(PathBuf::from),
        env::var_os("USERPROFILE").map(PathBuf::from),
        env::var_os("HOME").map(PathBuf::from),
    )
}

pub(crate) fn well_known_gh_paths_from(
    program_files: Option<PathBuf>,
    local_app_data: Option<PathBuf>,
    user_profile: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(pf) = program_files {
        v.push(pf.join("GitHub CLI").join("gh.exe"));
    }
    if let Some(local) = local_app_data {
        v.push(local.join("GitHub CLI").join("gh.exe"));
        v.push(local.join("Programs").join("GitHub CLI").join("gh.exe"));
    }
    let profile = user_profile.or_else(|| home.clone());
    if let Some(profile) = profile {
        v.push(
            profile
                .join("scoop")
                .join("apps")
                .join("gh")
                .join("current")
                .join("bin")
                .join("gh.exe"),
        );
        v.push(profile.join("scoop").join("shims").join("gh.exe"));
    }
    if let Some(h) = home {
        v.push(h.join(".local").join("bin").join("gh"));
    }
    v.push(PathBuf::from("/opt/homebrew/bin/gh"));
    v.push(PathBuf::from("/usr/local/bin/gh"));
    v.push(PathBuf::from("/usr/bin/gh"));
    v
}

fn gh_hosts_file_token() -> Option<String> {
    #[cfg(test)]
    {
        env::var_os("WHYCODES_TEST_GH_HOSTS_TOKEN").and_then(|v| {
            let s = v.to_string_lossy().into_owned();
            nonempty_str(&s)
        })
    }
    #[cfg(not(test))]
    {
        gh_hosts_file_token_from_disk()
    }
}

fn gh_hosts_file_token_from_disk() -> Option<String> {
    let path = gh_hosts_path()?;
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(err) => {
            tracing::debug!(error = %err, path = %path.display(), "gh hosts.yml unreadable");
            return None;
        }
    };
    parse_gh_hosts_yaml(&text, &github_host())
}

fn gh_hosts_path() -> Option<PathBuf> {
    gh_hosts_path_from(
        env::var_os("GH_CONFIG_DIR").map(PathBuf::from),
        env::var_os("APPDATA").map(PathBuf::from),
        env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        env::var_os("HOME").map(PathBuf::from),
    )
}

pub(crate) fn gh_hosts_path_from(
    gh_config_dir: Option<PathBuf>,
    appdata: Option<PathBuf>,
    xdg_config_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(dir) = gh_config_dir.filter(|p| !p.as_os_str().is_empty()) {
        return Some(dir.join("hosts.yml"));
    }
    if let Some(appdata) = appdata.filter(|p| !p.as_os_str().is_empty()) {
        return Some(appdata.join("GitHub CLI").join("hosts.yml"));
    }
    if let Some(xdg) = xdg_config_home.filter(|p| !p.as_os_str().is_empty()) {
        return Some(xdg.join("gh").join("hosts.yml"));
    }
    home.filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.join(".config").join("gh").join("hosts.yml"))
}

fn github_host() -> String {
    env::var_os("GH_HOST")
        .and_then(|v| {
            let owned = v.to_string_lossy().into_owned();
            nonempty_str(&owned)
        })
        .unwrap_or_else(|| "github.com".to_string())
}

/// Pull `oauth_token` for `host` out of `gh`'s `hosts.yml`.
pub(crate) fn parse_gh_hosts_yaml(text: &str, host: &str) -> Option<String> {
    let want = host.trim().trim_matches('"').trim_matches('\'');
    let mut in_host = false;
    let mut current_user: Option<String> = None;
    let mut default_user: Option<String> = None;
    let mut tokens: Vec<(Option<String>, String)> = Vec::new();

    for raw in text.lines() {
        let indent = raw.chars().take_while(|c| *c == ' ' || *c == '\t').count();
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if indent == 0 {
            in_host = host_heading(trimmed, want);
            current_user = None;
            continue;
        }
        if !in_host {
            continue;
        }
        if let Some(token) = yaml_scalar_key(trimmed, "oauth_token") {
            tokens.push((current_user.clone(), token));
            continue;
        }
        if let Some(user) = yaml_scalar_key(trimmed, "user") {
            default_user = Some(user);
            continue;
        }
        if trimmed == "users:" {
            continue;
        }
        if let Some(name) = yaml_map_key(trimmed) {
            match name.as_str() {
                "git_protocol" | "users" | "oauth_token" | "user" => {}
                _ => current_user = Some(name),
            }
        }
    }

    if let Some(user) = default_user
        && let Some((_, token)) = tokens
            .iter()
            .find(|(u, _)| u.as_deref() == Some(user.as_str()))
    {
        return Some(token.clone());
    }
    tokens.into_iter().map(|(_, t)| t).next()
}

fn host_heading(trimmed: &str, want: &str) -> bool {
    let Some(name) = yaml_map_key(trimmed) else {
        return false;
    };
    name.eq_ignore_ascii_case(want)
}

fn yaml_scalar_key(trimmed: &str, key: &str) -> Option<String> {
    let rest = trimmed.strip_prefix(key)?;
    let rest = rest.strip_prefix(':')?;
    let v = unquote(rest.trim());
    if v.is_empty() { None } else { Some(v) }
}

fn yaml_map_key(trimmed: &str) -> Option<String> {
    let key = trimmed.strip_suffix(':')?;
    if key.is_empty() || key.contains(':') {
        return None;
    }
    let key = unquote(key.trim());
    if key.is_empty() { None } else { Some(key) }
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

fn git_credential_token() -> Option<String> {
    #[cfg(test)]
    {
        if env::var_os("WHYCODES_TEST_SKIP_GIT_CREDENTIAL").is_some() {
            None
        } else {
            env::var_os("WHYCODES_TEST_GIT_CREDENTIAL_TOKEN").and_then(|v| {
                let s = v.to_string_lossy().into_owned();
                nonempty_str(&s)
            })
        }
    }
    #[cfg(not(test))]
    {
        git_credential_token_from_cli()
    }
}

/// `git credential fill` with prompts/GUI disabled. Timeout-killed.
fn git_credential_token_from_cli() -> Option<String> {
    let host = github_host();
    let mut cmd = Command::new("git");
    cmd.args(["credential", "fill"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GH_PROMPT_DISABLED", "1");
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(err) => {
            tracing::debug!(error = %err, "git credential fill: spawn failed");
            return None;
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let payload = format!("protocol=https\nhost={host}\n\n");
        if let Err(err) = stdin.write_all(payload.as_bytes()) {
            tracing::debug!(error = %err, "git credential fill: write stdin");
            if let Err(kill_err) = child.kill() {
                tracing::debug!(error = %kill_err, "git credential fill: kill after stdin error");
            }
            return None;
        }
    }
    let text = wait_child_stdout(child, GIT_CREDENTIAL_TIMEOUT, "git credential fill")?;
    parse_git_credential_fill(&text)
}

pub(crate) fn parse_git_credential_fill(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("password=") {
            return nonempty_str(rest);
        }
    }
    None
}

fn wait_child_stdout(mut child: Child, timeout: Duration, what: &'static str) -> Option<String> {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let mut stdout = child.stdout.take()?;
                let mut buf = String::new();
                match stdout.read_to_string(&mut buf) {
                    Ok(_) => return nonempty_str(&buf),
                    Err(err) => {
                        tracing::debug!(error = %err, context = what, "read stdout");
                        return None;
                    }
                }
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    if let Err(err) = child.kill() {
                        tracing::debug!(error = %err, context = what, "kill after timeout");
                    }
                    if let Err(err) = child.wait() {
                        tracing::debug!(error = %err, context = what, "wait after kill");
                    }
                    return None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(err) => {
                tracing::debug!(error = %err, context = what, "wait failed");
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
