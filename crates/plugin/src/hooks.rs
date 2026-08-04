//! Pre/post tool hooks: shell commands from config that run around tool calls.

use std::process::Stdio;
use std::time::Duration;

use whycode_config::{HookConfig, HookEvent};

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
        .env("WHYCODE_HOOK_EVENT", event_env(ctx.event))
        .env("WHYCODE_TOOL_NAME", &ctx.tool_name)
        .env("WHYCODE_TOOL_ID", &ctx.tool_id)
        .env("WHYCODE_TOOL_INPUT", &ctx.tool_input)
        .env("WHYCODE_WORKING_DIR", &ctx.working_dir);

    if let Some(ref sid) = ctx.session_id {
        cmd.env("WHYCODE_SESSION_ID", sid);
    }
    if let Some(is_err) = ctx.tool_is_error {
        cmd.env("WHYCODE_TOOL_IS_ERROR", if is_err { "1" } else { "0" });
    }
    if let Some(ref out) = ctx.tool_output {
        cmd.env("WHYCODE_TOOL_OUTPUT", out);
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
        Ok(Ok(output)) => HookRunResult {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            timed_out: false,
        },
        Ok(Err(e)) => HookRunResult {
            exit_code: -1,
            stdout: String::new(),
            stderr: format!("hook wait failed: {e}"),
            timed_out: false,
        },
        Err(_elapsed) => HookRunResult {
            exit_code: -1,
            stdout: String::new(),
            stderr: format!("hook timed out after {}s", hook.timeout_secs.max(1)),
            timed_out: true,
        },
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
            tracing::warn!(
                tool = %ctx.tool_name,
                detail = %format_failure(hook, &result),
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
    use whycode_config::HookConfig;

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
        match run_pre_hooks(&hooks, &ctx).await {
            PreHookDecision::Block { reason } => {
                assert!(reason.contains("blocked"), "{reason}");
            }
            PreHookDecision::Allow => panic!("expected block"),
        }
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
        match run_pre_hooks(&hooks, &ctx).await {
            PreHookDecision::Allow => {}
            PreHookDecision::Block { reason } => panic!("should not block: {reason}"),
        }
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
        match run_pre_hooks(&hooks, &ctx).await {
            PreHookDecision::Allow => {}
            PreHookDecision::Block { reason } => panic!("should allow: {reason}"),
        }
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
}
