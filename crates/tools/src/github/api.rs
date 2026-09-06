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
#[path = "api_tests.rs"]
mod tests;
