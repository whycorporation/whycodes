//! Pre/post tool hooks: shell commands from config that run around tool calls.

use std::process::Stdio;
use std::time::Duration;

use whycodes_config::{HookConfig, HookEvent};

/// Truncate long tool output before stuffing it into an env var.
const OUTPUT_ENV_MAX: usize = 8_192;

/// Context passed into a hook invocation.
#[derive(Debug, Clone)]
pub struct HookContext {
    pub event: HookEvent,
    pub tool_name: String,
    pub tool_id: String,
    pub tool_input: String,
    pub session_id: Option<String>,
    pub working_dir: String,
    /// Only meaningful for `PostTool`.
    pub tool_is_error: Option<bool>,
    /// Only meaningful for `PostTool` (truncated).
    pub tool_output: Option<String>,
}

impl HookContext {
    /// Build a pre-tool context (no result fields).
    pub fn pre(
        tool_name: impl Into<String>,
        tool_id: impl Into<String>,
        tool_input: impl Into<String>,
        session_id: Option<String>,
        working_dir: impl Into<String>,
    ) -> Self {
        Self {
            event: HookEvent::PreTool,
            tool_name: tool_name.into(),
            tool_id: tool_id.into(),
            tool_input: tool_input.into(),
            session_id,
            working_dir: working_dir.into(),
            tool_is_error: None,
            tool_output: None,
        }
    }

    /// Build a post-tool context from the tool result.
    pub fn post(
        tool_name: impl Into<String>,
        tool_id: impl Into<String>,
        tool_input: impl Into<String>,
        session_id: Option<String>,
        working_dir: impl Into<String>,
        is_error: bool,
        output: &str,
    ) -> Self {
        Self {
            event: HookEvent::PostTool,
            tool_name: tool_name.into(),
            tool_id: tool_id.into(),
            tool_input: tool_input.into(),
            session_id,
            working_dir: working_dir.into(),
            tool_is_error: Some(is_error),
            tool_output: Some(truncate_output(output)),
        }
    }
}

/// Result of running one hook command.
#[derive(Debug, Clone)]
pub struct HookRunResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl HookRunResult {
    pub fn success(&self) -> bool {
        !self.timed_out && self.exit_code == 0
    }
}

/// Outcome of the pre-tool hook chain for one tool call.
#[derive(Debug, Clone)]
pub enum PreHookDecision {
    /// Continue with the tool.
    Allow,
    /// Refuse the tool; message is shown to the model / user.
    Block { reason: String },
}

/// Whether a tool name matches a hook `match` pattern.
///
/// Supports exact names, `*` (all), `prefix*`, and `*suffix`.
pub fn tool_matches(pattern: &str, tool_name: &str) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() || pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*')
        && !prefix.is_empty()
        && !prefix.contains('*')
    {
        return tool_name.starts_with(prefix);
    }
    if let Some(suffix) = pattern.strip_prefix('*')
        && !suffix.is_empty()
        && !suffix.contains('*')
    {
        return tool_name.ends_with(suffix);
    }
    pattern == tool_name
}

/// Filter hooks for an event + tool name.
pub fn matching_hooks<'a>(
    hooks: &'a [HookConfig],
    event: HookEvent,
    tool_name: &str,
) -> Vec<&'a HookConfig> {
    hooks
        .iter()
        .filter(|h| h.event == event && tool_matches(&h.tool_match, tool_name))
        .collect()
}

/// Run a single hook command asynchronously with a timeout.
pub async fn run_hook(hook: &HookConfig, ctx: &HookContext) -> HookRunResult {
    let timeout = Duration::from_secs(hook.timeout_secs.max(1));

    let mut cmd = {
        #[cfg(windows)]
        {
            let mut c = tokio::process::Command::new("cmd");
            c.arg("/C").arg(&hook.command);
            c
        }
        #[cfg(not(windows))]
        {
            let mut c = tokio::process::Command::new("sh");
            c.arg("-c").arg(&hook.command);
            c
        }
    };

    cmd.current_dir(&ctx.working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env("WHYCODES_HOOK_EVENT", event_env(ctx.event))
        .env("WHYCODES_TOOL_NAME", &ctx.tool_name)
        .env("WHYCODES_TOOL_ID", &ctx.tool_id)
        .env("WHYCODES_TOOL_INPUT", &ctx.tool_input)
        .env("WHYCODES_WORKING_DIR", &ctx.working_dir);

    if let Some(ref sid) = ctx.session_id {
        cmd.env("WHYCODES_SESSION_ID", sid);
    }
    if let Some(is_err) = ctx.tool_is_error {
        cmd.env("WHYCODES_TOOL_IS_ERROR", if is_err { "1" } else { "0" });
    }
    if let Some(ref out) = ctx.tool_output {
        cmd.env("WHYCODES_TOOL_OUTPUT", out);
    }

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return HookRunResult {
                exit_code: -1,
                stdout: String::new(),
                stderr: format!("failed to spawn hook: {e}"),
                timed_out: false,
            };
        }
    };

    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(result) => hook_result_from_wait(result),
        Err(_elapsed) => hook_timeout_result(hook.timeout_secs),
    }
}

fn hook_result_from_wait(result: Result<std::process::Output, std::io::Error>) -> HookRunResult {
    match result {
        Ok(output) => HookRunResult {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            timed_out: false,
        },
        Err(e) => HookRunResult {
            exit_code: -1,
            stdout: String::new(),
            stderr: format!("hook wait failed: {e}"),
            timed_out: false,
        },
    }
}

fn hook_timeout_result(timeout_secs: u64) -> HookRunResult {
    HookRunResult {
        exit_code: -1,
        stdout: String::new(),
        stderr: format!("hook timed out after {}s", timeout_secs.max(1)),
        timed_out: true,
    }
}

fn event_env(event: HookEvent) -> &'static str {
    match event {
        HookEvent::PreTool => "pre_tool",
        HookEvent::PostTool => "post_tool",
    }
}

/// Truncate tool output for env injection.
pub fn truncate_output(s: &str) -> String {
    let n = s.chars().count();
    if n <= OUTPUT_ENV_MAX {
        return s.to_string();
    }
    let mut out: String = s.chars().take(OUTPUT_ENV_MAX).collect();
    out.push_str("\n…[truncated]");
    out
}

/// Run all matching pre-tool hooks. First blocking failure wins.
pub async fn run_pre_hooks(hooks: &[HookConfig], ctx: &HookContext) -> PreHookDecision {
    let matched = matching_hooks(hooks, HookEvent::PreTool, &ctx.tool_name);
    if matched.is_empty() {
        return PreHookDecision::Allow;
    }

    for hook in matched {
        if hook.command.trim().is_empty() {
            continue;
        }
        tracing::debug!(
            tool = %ctx.tool_name,
            command = %hook.command,
            "running pre_tool hook"
        );
        let result = run_hook(hook, ctx).await;
        if result.success() {
            continue;
        }
        let detail = format_failure(hook, &result);
        if hook.block_on_failure {
            tracing::warn!(tool = %ctx.tool_name, %detail, "pre_tool hook blocked tool");
            return PreHookDecision::Block {
                reason: format!("pre_tool hook blocked `{}`: {detail}", ctx.tool_name),
            };
        }
        tracing::warn!(
            tool = %ctx.tool_name,
            %detail,
            "pre_tool hook failed (non-blocking)"
        );
    }
    PreHookDecision::Allow
}

/// Run all matching post-tool hooks. Failures are logged only.
pub async fn run_post_hooks(hooks: &[HookConfig], ctx: &HookContext) {
    let matched = matching_hooks(hooks, HookEvent::PostTool, &ctx.tool_name);
    if matched.is_empty() {
        return;
    }

    for hook in matched {
        if hook.command.trim().is_empty() {
            continue;
        }
        tracing::debug!(
            tool = %ctx.tool_name,
            command = %hook.command,
            "running post_tool hook"
        );
        let result = run_hook(hook, ctx).await;
        if !result.success() {
            let detail = format_failure(hook, &result);
            tracing::warn!(
                tool = %ctx.tool_name,
                %detail,
                "post_tool hook failed"
            );
        }
    }
}

fn format_failure(hook: &HookConfig, result: &HookRunResult) -> String {
    if result.timed_out {
        return format!("timed out after {}s", hook.timeout_secs.max(1));
    }
    let mut parts = vec![format!("exit {}", result.exit_code)];
    let err = result.stderr.trim();
    if !err.is_empty() {
        let snippet: String = err.chars().take(200).collect();
        parts.push(snippet);
    }
    let out = result.stdout.trim();
    if !out.is_empty() && err.is_empty() {
        let snippet: String = out.chars().take(200).collect();
        parts.push(snippet);
    }
    parts.join(": ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use whycodes_config::HookConfig;

    fn work_dir() -> String {
        std::env::temp_dir().to_str().unwrap_or(".").to_string()
    }

    #[test]
    fn tool_matches_star_and_affixes() {
        assert!(tool_matches("*", "bash"));
        assert!(tool_matches("bash", "bash"));
        assert!(!tool_matches("bash", "shell"));
        assert!(tool_matches("git_*", "git_status"));
        assert!(!tool_matches("git_*", "bash"));
        assert!(tool_matches("*_issue", "github_issue"));
        assert!(!tool_matches("*_issue", "github_pr"));
    }

    #[test]
    fn matching_hooks_filters_event_and_tool() {
        let hooks = vec![
            HookConfig {
                event: HookEvent::PreTool,
                tool_match: "bash".into(),
                command: "true".into(),
                block_on_failure: true,
                timeout_secs: 5,
            },
            HookConfig {
                event: HookEvent::PostTool,
                tool_match: "*".into(),
                command: "true".into(),
                block_on_failure: false,
                timeout_secs: 5,
            },
            HookConfig {
                event: HookEvent::PreTool,
                tool_match: "write".into(),
                command: "true".into(),
                block_on_failure: false,
                timeout_secs: 5,
            },
        ];
        let pre = matching_hooks(&hooks, HookEvent::PreTool, "bash");
        assert_eq!(pre.len(), 1);
        assert_eq!(pre[0].tool_match, "bash");
        let post = matching_hooks(&hooks, HookEvent::PostTool, "bash");
        assert_eq!(post.len(), 1);
    }

    #[tokio::test]
    async fn pre_hook_block_on_failure() {
        let hooks = vec![HookConfig {
            event: HookEvent::PreTool,
            tool_match: "*".into(),
            command: "exit 1".into(),
            block_on_failure: true,
            timeout_secs: 5,
        }];
        let ctx = HookContext::pre("bash", "id1", "{}", None, work_dir());
        let decision = run_pre_hooks(&hooks, &ctx).await;
        assert!(format!("{decision:?}").contains("blocked"));
    }

    #[tokio::test]
    async fn pre_hook_non_blocking_failure_allows() {
        let hooks = vec![HookConfig {
            event: HookEvent::PreTool,
            tool_match: "*".into(),
            command: "exit 1".into(),
            block_on_failure: false,
            timeout_secs: 5,
        }];
        let ctx = HookContext::pre("bash", "id1", "{}", None, work_dir());
        assert!(matches!(
            run_pre_hooks(&hooks, &ctx).await,
            PreHookDecision::Allow
        ));
    }

    #[tokio::test]
    async fn pre_hook_success_allows() {
        let hooks = vec![HookConfig {
            event: HookEvent::PreTool,
            tool_match: "bash".into(),
            command: "true".into(),
            block_on_failure: true,
            timeout_secs: 5,
        }];
        let ctx = HookContext::pre("bash", "id1", "{}", None, work_dir());
        assert!(matches!(
            run_pre_hooks(&hooks, &ctx).await,
            PreHookDecision::Allow
        ));
    }

    #[tokio::test]
    async fn post_hook_runs_without_panic() {
        let hooks = vec![HookConfig {
            event: HookEvent::PostTool,
            tool_match: "*".into(),
            command: "true".into(),
            block_on_failure: false,
            timeout_secs: 5,
        }];
        let ctx = HookContext::post(
            "read",
            "id1",
            r#"{"path":"x"}"#,
            Some("sess".into()),
            work_dir(),
            false,
            "ok",
        );
        run_post_hooks(&hooks, &ctx).await;
    }

    #[test]
    fn truncate_output_short_unchanged() {
        assert_eq!(truncate_output("hi"), "hi");
    }

    #[test]
    fn tool_matches_empty_and_inner_stars() {
        assert!(tool_matches("", "bash"));
        assert!(tool_matches("   ", "anything"));
        assert!(!tool_matches("foo*bar*", "foobar"));
        assert!(!tool_matches("*foo*bar", "foobar"));
        assert!(!tool_matches("exact", "other"));
    }

    fn hook(
        event: HookEvent,
        tool_match: &str,
        command: &str,
        block_on_failure: bool,
        timeout_secs: u64,
    ) -> HookConfig {
        HookConfig {
            event,
            tool_match: tool_match.into(),
            command: command.into(),
            block_on_failure,
            timeout_secs,
        }
    }

    #[tokio::test]
    async fn pre_hooks_allow_when_none_match_or_command_blank() {
        let hooks = vec![
            hook(HookEvent::PostTool, "*", "true", false, 5),
            hook(HookEvent::PreTool, "bash", "   ", true, 5),
        ];
        let ctx = HookContext::pre("bash", "id", "{}", None, work_dir());
        assert!(matches!(
            run_pre_hooks(&hooks, &ctx).await,
            PreHookDecision::Allow
        ));
        let ctx_other = HookContext::pre("write", "id", "{}", None, work_dir());
        assert!(matches!(
            run_pre_hooks(&hooks, &ctx_other).await,
            PreHookDecision::Allow
        ));
    }

    #[tokio::test]
    async fn post_hooks_skip_empty_and_log_failure() {
        let hooks = vec![
            hook(HookEvent::PreTool, "*", "true", false, 5),
            hook(HookEvent::PostTool, "read", "   ", false, 5),
            hook(
                HookEvent::PostTool,
                "read",
                "echo out; echo err >&2; exit 1",
                false,
                5,
            ),
        ];
        let ctx = HookContext::post(
            "read",
            "id1",
            "{}",
            Some("sess".into()),
            work_dir(),
            true,
            "tool-output",
        );
        run_post_hooks(&hooks, &ctx).await;

        let no_match = HookContext::post("write", "id2", "{}", None, work_dir(), false, "ok");
        run_post_hooks(&hooks, &no_match).await;
    }

    #[tokio::test]
    async fn run_hook_spawn_failure() {
        let cfg = hook(HookEvent::PreTool, "*", "true", false, 5);
        let ctx = HookContext::pre(
            "bash",
            "id",
            "{}",
            None,
            "/this/path/does/not/exist/whycodes-plugin-hooks",
        );
        let result = run_hook(&cfg, &ctx).await;
        assert!(!result.success());
        assert!(result.stderr.contains("failed to spawn hook"), "{result:?}");
        assert_eq!(result.exit_code, -1);
        assert!(!result.timed_out);
    }

    #[tokio::test]
    async fn run_hook_times_out() {
        let cfg = hook(HookEvent::PreTool, "*", "sleep 8", true, 0);
        let ctx = HookContext::pre("bash", "id", "{}", None, work_dir());
        let result = run_hook(&cfg, &ctx).await;
        assert!(result.timed_out);
        assert!(!result.success());
        assert!(result.stderr.contains("timed out"), "{result:?}");
    }

    #[test]
    fn hook_wait_and_timeout_helpers() {
        let wait_err = hook_result_from_wait(Err(std::io::Error::other("pipe broke")));
        assert_eq!(wait_err.exit_code, -1);
        assert!(wait_err.stderr.contains("hook wait failed"));
        assert!(!wait_err.success());

        let timed = hook_timeout_result(0);
        assert!(timed.timed_out);
        assert!(timed.stderr.contains("timed out after 1s"));
        assert!(!timed.success());
    }

    #[test]
    fn format_failure_covers_timeout_stderr_and_stdout() {
        let cfg = hook(HookEvent::PreTool, "*", "x", true, 3);
        let timed = HookRunResult {
            exit_code: -1,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: true,
        };
        assert_eq!(format_failure(&cfg, &timed), "timed out after 3s");

        let with_err = HookRunResult {
            exit_code: 2,
            stdout: "ignored".into(),
            stderr: "boom-stderr".into(),
            timed_out: false,
        };
        let err_msg = format_failure(&cfg, &with_err);
        assert!(err_msg.contains("exit 2"));
        assert!(err_msg.contains("boom-stderr"));

        let long_err = "e".repeat(250);
        let long = HookRunResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: long_err,
            timed_out: false,
        };
        assert!(format_failure(&cfg, &long).contains("exit 1"));

        let with_out = HookRunResult {
            exit_code: 4,
            stdout: "only-stdout".into(),
            stderr: "   ".into(),
            timed_out: false,
        };
        let out_msg = format_failure(&cfg, &with_out);
        assert!(out_msg.contains("exit 4"));
        assert!(out_msg.contains("only-stdout"));

        let long_out = "o".repeat(250);
        let long_stdout = HookRunResult {
            exit_code: 7,
            stdout: long_out,
            stderr: String::new(),
            timed_out: false,
        };
        assert!(format_failure(&cfg, &long_stdout).contains("exit 7"));
    }

    #[test]
    fn truncate_output_long_is_cut() {
        let s = "é".repeat(OUTPUT_ENV_MAX + 4);
        let out = truncate_output(&s);
        assert!(out.ends_with("\n…[truncated]"));
        assert_eq!(
            out.chars().count(),
            OUTPUT_ENV_MAX + "\n…[truncated]".chars().count()
        );
    }
}
