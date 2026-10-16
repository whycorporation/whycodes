//! `pr://` and `issue://` for the `read` tool: the pull request / issue of
//! the project's `origin` GitHub remote, through the same token discovery
//! and network policy as `github_pr` / `github_issue`.

use reqwest::header::HeaderValue;
use serde_json::Value;
use whycodes_core::network::NetworkPolicy;
use whycodes_core::types::ToolResult;

use crate::github::api;
use crate::tool::ToolContext;

/// Which GitHub object a scheme names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Pr,
    Issue,
}

impl Kind {
    fn scheme(self) -> &'static str {
        match self {
            Kind::Pr => "pr",
            Kind::Issue => "issue",
        }
    }
}

/// What the part after `scheme://` asks for.
#[derive(Debug, PartialEq, Eq)]
enum Target {
    List,
    View(u64),
    Diff(u64),
}

type Remote = Result<(String, String), String>;

/// First page of a listing, as `github_pr` / `github_issue` paginate.
const LIST_PAGE: usize = 30;

pub(crate) async fn read_github(kind: Kind, rest: &str, ctx: &ToolContext) -> ToolResult {
    let target = match parse_target(kind, rest) {
        Ok(t) => t,
        Err(msg) => return err(&msg),
    };
    let working_dir = ctx.working_dir.clone();
    let (owner, repo) = match remote_blocking(Box::new(move || github_remote(&working_dir))).await {
        Ok(remote) => remote,
        Err(msg) => return err(&msg),
    };
    if let Err(msg) = ctx.network.check_url("https://api.github.com/") {
        return err(&msg);
    }
    let Some(token) = api::resolve_token(None) else {
        return err(api::missing_token_message());
    };
    let client = reqwest::Client::new();
    let net = &ctx.network;
    let result = match target {
        Target::List => match kind {
            Kind::Pr => list_prs(&client, &token, &owner, &repo, net).await,
            Kind::Issue => list_issues(&client, &token, &owner, &repo, net).await,
        },
        Target::View(n) => match kind {
            Kind::Pr => view_pr(&client, &token, &owner, &repo, n, net).await,
            Kind::Issue => view_issue(&client, &token, &owner, &repo, n, net).await,
        },
        Target::Diff(n) => pr_diff(&client, &token, &owner, &repo, n, net).await,
    };
    match result {
        Ok(body) => ok(&body),
        Err(msg) => err(&msg),
    }
}

fn parse_target(kind: Kind, rest: &str) -> Result<Target, String> {
    let rest = rest.trim().trim_matches('/');
    if rest.is_empty() {
        return Ok(Target::List);
    }
    let (number, sub) = rest.split_once('/').unwrap_or((rest, ""));
    let scheme = kind.scheme();
    let number: u64 = number.parse().map_err(|_: std::num::ParseIntError| {
        format!("{scheme}://<number> needs a number (`read {scheme}://` lists the open ones).")
    })?;
    match sub {
        "" => Ok(Target::View(number)),
        "diff" if kind == Kind::Pr => Ok(Target::Diff(number)),
        other => Err(format!(
            "Unknown {scheme}:// suffix '/{other}'. Use {scheme}://{number}{}.",
            if kind == Kind::Pr {
                " or pr://<number>/diff"
            } else {
                ""
            }
        )),
    }
}

/// Run the remote lookup on the blocking pool (it spawns `git`).
async fn remote_blocking(f: Box<dyn FnOnce() -> Remote + Send>) -> Remote {
    #[allow(clippy::redundant_closure)]
    match tokio::task::spawn_blocking(move || f()).await {
        Ok(remote) => remote,
        Err(e) => Err(format!("background task failed: {e}")),
    }
}

/// `(owner, repo)` of the `origin` remote when it points at github.com.
fn github_remote(working_dir: &str) -> Remote {
    let output = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(working_dir)
        .output();
    github_remote_from(output)
}

fn github_remote_from(output: std::io::Result<std::process::Output>) -> Remote {
    let output = output.map_err(|e| format!("Cannot run git to find the GitHub remote: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "No `origin` remote in this project ({}). pr:// and issue:// read the GitHub \
             repository behind `origin`; use `github_pr` / `github_issue` with owner and repo \
             otherwise.",
            stderr.trim()
        ));
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    parse_github_remote(&url).ok_or_else(|| {
        format!(
            "Remote `origin` ({url}) is not a github.com repository. pr:// and issue:// need \
             one; use `github_pr` / `github_issue` with owner and repo otherwise."
        )
    })
}

/// `owner/repo` from an https, ssh, or scp-style github.com URL.
fn parse_github_remote(url: &str) -> Option<(String, String)> {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let idx = url.find("github.com")?;
    let preceded_ok = idx == 0 || matches!(url.as_bytes()[idx - 1], b'@' | b'/');
    if !preceded_ok {
        return None;
    }
    let rest = &url[idx + "github.com".len()..];
    let rest = rest.strip_prefix(':').or_else(|| rest.strip_prefix('/'))?;
    let mut parts = rest.split('/').filter(|s| !s.is_empty());
    let owner = parts.next()?;
    let repo = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    Some((owner.to_string(), repo.to_string()))
}

const DIFF_MEDIA_TYPE: &str = "application/vnd.github.diff";

/// GET `path`; a non-2xx status is an error that carries the body.
async fn get(
    client: &reqwest::Client,
    token: &str,
    path: &str,
    accept: Option<&'static str>,
    network: &NetworkPolicy,
) -> Result<String, String> {
    let mut headers = api::github_headers(token)?;
    if let Some(media) = accept {
        headers.insert("Accept", HeaderValue::from_static(media));
    }
    let url = api::api_url(path);
    network.check_url(&url)?;
    let resp = client
        .get(&url)
        .headers(headers)
        .send()
        .await
        .map_err(api::github_api_request_error)?;
    let status = resp.status();
    let text = resp.text().await.map_err(api::github_api_response_error)?;
    if status.is_success() {
        Ok(text)
    } else {
        Err(format!(
            "GitHub API error ({}): {}",
            status.as_u16(),
            text.trim()
        ))
    }
}

async fn get_json(
    client: &reqwest::Client,
    token: &str,
    path: &str,
    network: &NetworkPolicy,
) -> Result<Value, String> {
    let text = get(client, token, path, None, network).await?;
    serde_json::from_str(&text).map_err(|e| format!("Unexpected GitHub response: {e}"))
}

async fn list_prs(
    client: &reqwest::Client,
    token: &str,
    owner: &str,
    repo: &str,
    network: &NetworkPolicy,
) -> Result<String, String> {
    let path = format!(
        "repos/{owner}/{repo}/pulls?state=open&sort=updated&direction=desc&per_page={LIST_PAGE}"
    );
    let items = get_json(client, token, &path, network).await?;
    let rows: Vec<String> = items
        .as_array()
        .map(|a| a.iter().map(pr_row).collect())
        .unwrap_or_default();
    Ok(listing("pull requests", "pr", owner, repo, &rows))
}

async fn list_issues(
    client: &reqwest::Client,
    token: &str,
    owner: &str,
    repo: &str,
    network: &NetworkPolicy,
) -> Result<String, String> {
    let path = format!("repos/{owner}/{repo}/issues?state=open&per_page={LIST_PAGE}");
    let items = get_json(client, token, &path, network).await?;
    // The issues endpoint also returns pull requests; those live under pr://.
    let rows: Vec<String> = items
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|v| v.get("pull_request").is_none())
                .map(issue_row)
                .collect()
        })
        .unwrap_or_default();
    Ok(listing("issues", "issue", owner, repo, &rows))
}

fn listing(what: &str, scheme: &str, owner: &str, repo: &str, rows: &[String]) -> String {
    if rows.is_empty() {
        return format!("No open {what} in {owner}/{repo}.");
    }
    let mut out = format!(
        "Open {what} in {owner}/{repo} (first page; read one with `read {scheme}://<number>`):\n"
    );
    for row in rows {
        out.push_str("- ");
        out.push_str(row);
        out.push('\n');
    }
    out
}

fn pr_row(v: &Value) -> String {
    let draft = draft_mark(v, " [draft]");
    format!(
        "#{} {}{draft} — {}, {} → {}",
        v["number"],
        text(v, "title"),
        text(&v["user"], "login"),
        text(&v["head"], "ref"),
        text(&v["base"], "ref")
    )
}

/// `mark` for a draft pull request, nothing otherwise.
fn draft_mark(v: &Value, mark: &'static str) -> &'static str {
    if v["draft"].as_bool().unwrap_or(false) {
        mark
    } else {
        ""
    }
}

fn issue_row(v: &Value) -> String {
    let labels = labels(v);
    let labels = if labels.is_empty() {
        String::new()
    } else {
        format!(" [{labels}]")
    };
    format!(
        "#{} {}{labels} — {}",
        v["number"],
        text(v, "title"),
        text(&v["user"], "login")
    )
}

async fn view_pr(
    client: &reqwest::Client,
    token: &str,
    owner: &str,
    repo: &str,
    number: u64,
    network: &NetworkPolicy,
) -> Result<String, String> {
    let path = format!("repos/{owner}/{repo}/pulls/{number}");
    let v = get_json(client, token, &path, network).await?;
    let draft = draft_mark(&v, " (draft)");
    let mut out = format!(
        "# pr://{number} — {}\nstate: {}{draft} · author: {} · {} → {} · created {} · updated {}\n\
         changes: +{} −{} in {} file(s) · commits: {}\n",
        text(&v, "title"),
        text(&v, "state"),
        text(&v["user"], "login"),
        text(&v["head"], "label"),
        text(&v["base"], "ref"),
        text(&v, "created_at"),
        text(&v, "updated_at"),
        v["additions"],
        v["deletions"],
        v["changed_files"],
        v["commits"]
    );
    push_common(&mut out, &v);
    out.push_str(&format!("diff: `read pr://{number}/diff`\n\n"));
    out.push_str(&body(&v));
    Ok(out)
}

async fn pr_diff(
    client: &reqwest::Client,
    token: &str,
    owner: &str,
    repo: &str,
    number: u64,
    network: &NetworkPolicy,
) -> Result<String, String> {
    let path = format!("repos/{owner}/{repo}/pulls/{number}");
    let diff = get(client, token, &path, Some(DIFF_MEDIA_TYPE), network).await?;
    Ok(format!("# pr://{number}/diff\n\n{diff}"))
}

async fn view_issue(
    client: &reqwest::Client,
    token: &str,
    owner: &str,
    repo: &str,
    number: u64,
    network: &NetworkPolicy,
) -> Result<String, String> {
    let path = format!("repos/{owner}/{repo}/issues/{number}");
    let v = get_json(client, token, &path, network).await?;
    let mut out = format!(
        "# issue://{number} — {}\nstate: {} · author: {} · created {} · updated {} · comments: {}\n",
        text(&v, "title"),
        text(&v, "state"),
        text(&v["user"], "login"),
        text(&v, "created_at"),
        text(&v, "updated_at"),
        v["comments"]
    );
    push_common(&mut out, &v);
    if v.get("pull_request").is_some() {
        out.push_str(&format!(
            "note: #{number} is a pull request; `read pr://{number}` shows its branches and diff.\n"
        ));
    }
    out.push('\n');
    out.push_str(&body(&v));
    Ok(out)
}

/// Labels and URL lines shared by PR and issue views.
fn push_common(out: &mut String, v: &Value) {
    let labels = labels(v);
    if !labels.is_empty() {
        out.push_str(&format!("labels: {labels}\n"));
    }
    out.push_str(&format!("url: {}\n", text(v, "html_url")));
}

fn body(v: &Value) -> String {
    match v["body"].as_str().map(str::trim) {
        Some(b) if !b.is_empty() => b.to_string(),
        _ => "(no description)".to_string(),
    }
}

fn labels(v: &Value) -> String {
    v["labels"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|l| l["name"].as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("?")
}

fn ok(msg: &str) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: msg.to_string(),
        is_error: false,
    }
}

fn err(msg: &str) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: msg.to_string(),
        is_error: true,
    }
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
#[path = "internal_github_tests.rs"]
mod tests;
