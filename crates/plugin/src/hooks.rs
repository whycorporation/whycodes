//! Config hooks: shell commands that run around tool calls and at session
//! lifecycle points (session start, prompt submit, stop, compaction, subagent).

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
    /// Event-specific variables (`WHYCODES_PROMPT`, …).
    pub extra_env: Vec<(&'static str, String)>,
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
            extra_env: Vec::new(),
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
            extra_env: Vec::new(),
        }
    }

    /// Context for a lifecycle event. `subject` is what `match` filters
    /// (the agent type for `SubagentStop`; empty otherwise).
    pub fn lifecycle(
        event: HookEvent,
        subject: impl Into<String>,
        session_id: Option<String>,
        working_dir: impl Into<String>,
    ) -> Self {
        Self {
            event,
            tool_name: subject.into(),
            tool_id: String::new(),
            tool_input: String::new(),
            session_id,
            working_dir: working_dir.into(),
            tool_is_error: None,
            tool_output: None,
            extra_env: Vec::new(),
        }
    }

    /// Add an event variable; long values are truncated like tool output.
    pub fn with_env(mut self, key: &'static str, value: &str) -> Self {
        self.extra_env.push((key, truncate_output(value)));
        self
    }
}

/// What a hook chain asks of the caller.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookOutcome {
    /// First blocking failure (`pre_tool` / `user_prompt_submit` only).
    pub block: Option<String>,
    /// Trimmed stdout of successful `feedback = true` hooks, in order.
    pub feedback: Vec<String>,
}

impl HookOutcome {
    /// Feedback joined for the model, or `None` when no hook spoke.
    pub fn feedback_text(&self) -> Option<String> {
        if self.feedback.is_empty() {
            return None;
        }
        Some(self.feedback.join("\n\n"))
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
    let uses_match = matches!(
        event,
        HookEvent::PreTool | HookEvent::PostTool | HookEvent::SubagentStop
    );
    hooks
        .iter()
        .filter(|h| h.event == event && (!uses_match || tool_matches(&h.tool_match, tool_name)))
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
        .env("WHYCODES_WORKING_DIR", &ctx.working_dir)
        .envs(ctx.extra_env.iter().map(|(k, v)| (*k, v.as_str())));

    if matches!(ctx.event, HookEvent::PreTool | HookEvent::PostTool) {
        cmd.env("WHYCODES_TOOL_NAME", &ctx.tool_name)
            .env("WHYCODES_TOOL_ID", &ctx.tool_id)
            .env("WHYCODES_TOOL_INPUT", &ctx.tool_input);
    }

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
    event.as_str()
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

/// Run every hook matching `ctx`. First blocking failure wins.
///
/// Blocking applies to `pre_tool` / `user_prompt_submit`; feedback to
/// `post_tool` / `session_start` / `user_prompt_submit`. Other failures are
/// logged only.
pub async fn run_hooks(hooks: &[HookConfig], ctx: &HookContext) -> HookOutcome {
    let label = event_env(ctx.event);
    let blockable = ctx.event.can_block();
    let speaks = matches!(
        ctx.event,
        HookEvent::PostTool | HookEvent::SessionStart | HookEvent::UserPromptSubmit
    );
    let mut outcome = HookOutcome::default();
    for hook in matching_hooks(hooks, ctx.event, &ctx.tool_name) {
        if hook.command.trim().is_empty() {
            continue;
        }
        tracing::debug!(event = label, subject = %ctx.tool_name, command = %hook.command, "running hook");
        let result = run_hook(hook, ctx).await;
        if result.success() {
            let out = result.stdout.trim();
            if hook.feedback && speaks && !out.is_empty() {
                outcome.feedback.push(truncate_output(out));
            }
            continue;
        }
        let detail = format_failure(hook, &result);
        if hook.block_on_failure && blockable {
            tracing::warn!(event = label, subject = %ctx.tool_name, %detail, "hook blocked");
            outcome.block = Some(detail);
            return outcome;
        }
        tracing::warn!(event = label, subject = %ctx.tool_name, %detail, "hook failed (non-blocking)");
    }
    outcome
}

/// Run all matching pre-tool hooks. First blocking failure wins.
pub async fn run_pre_hooks(hooks: &[HookConfig], ctx: &HookContext) -> PreHookDecision {
    match run_hooks(hooks, ctx).await.block {
        None => PreHookDecision::Allow,
        Some(detail) => PreHookDecision::Block {
            reason: format!("pre_tool hook blocked `{}`: {detail}", ctx.tool_name),
        },
    }
}

/// Run all matching post-tool hooks; returns feedback for the tool result.
pub async fn run_post_hooks(hooks: &[HookConfig], ctx: &HookContext) -> Option<String> {
    run_hooks(hooks, ctx).await.feedback_text()
}

/// Append post-tool hook feedback to a tool result body.
pub fn append_feedback(content: &mut String, feedback: Option<String>) {
    if let Some(text) = feedback {
        content.push_str("\n\n[post_tool hook]\n");
        content.push_str(&text);
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
                feedback: false,
                timeout_secs: 5,
            },
            HookConfig {
                event: HookEvent::PostTool,
                tool_match: "*".into(),
                command: "true".into(),
                block_on_failure: false,
                feedback: false,
                timeout_secs: 5,
            },
            HookConfig {
                event: HookEvent::PreTool,
                tool_match: "write".into(),
                command: "true".into(),
                block_on_failure: false,
                feedback: false,
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
            feedback: false,
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
            feedback: false,
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
            feedback: false,
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
            feedback: false,
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
            feedback: false,
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

    /// Shell snippet that prints an env var (sh vs cmd).
    fn echo_var(name: &str) -> String {
        if cfg!(windows) {
            format!("echo %{name}%")
        } else {
            format!("echo \"${name}\"")
        }
    }

    fn speaking(event: HookEvent, tool_match: &str, command: &str) -> HookConfig {
        HookConfig {
            feedback: true,
            ..hook(event, tool_match, command, false, 5)
        }
    }

    #[tokio::test]
    async fn lifecycle_hooks_see_event_env_and_ignore_match() {
        let hooks = vec![
            speaking(
                HookEvent::UserPromptSubmit,
                "bash",
                &echo_var("WHYCODES_PROMPT"),
            ),
            speaking(
                HookEvent::UserPromptSubmit,
                "*",
                &echo_var("WHYCODES_HOOK_EVENT"),
            ),
            hook(HookEvent::PreTool, "*", "echo not-me", false, 5),
        ];
        let ctx = HookContext::lifecycle(HookEvent::UserPromptSubmit, "", None, work_dir())
            .with_env("WHYCODES_PROMPT", "fix the build");
        let outcome = run_hooks(&hooks, &ctx).await;
        assert_eq!(outcome.block, None);
        assert_eq!(
            outcome.feedback,
            vec!["fix the build", "user_prompt_submit"]
        );
        assert_eq!(
            outcome.feedback_text().as_deref(),
            Some("fix the build\n\nuser_prompt_submit")
        );
    }

    #[tokio::test]
    async fn lifecycle_hooks_do_not_get_tool_variables() {
        let hooks = vec![speaking(
            HookEvent::SessionStart,
            "*",
            &echo_var("WHYCODES_TOOL_NAME"),
        )];
        let ctx = HookContext::lifecycle(HookEvent::SessionStart, "subject", None, work_dir());
        let out = run_hooks(&hooks, &ctx).await.feedback;
        assert!(out.iter().all(|line| !line.contains("subject")), "{out:?}");
    }

    #[tokio::test]
    async fn only_prompt_and_tool_events_can_block() {
        for event in [HookEvent::UserPromptSubmit, HookEvent::PreTool] {
            let hooks = vec![
                hook(event, "*", "exit 3", true, 5),
                speaking(event, "*", "echo after"),
            ];
            let ctx = HookContext::lifecycle(event, "bash", None, work_dir());
            let outcome = run_hooks(&hooks, &ctx).await;
            assert!(outcome.block.unwrap().contains("exit 3"));
            assert!(outcome.feedback.is_empty(), "stops at the block");
        }
        for event in [HookEvent::Stop, HookEvent::PreCompact, HookEvent::PostTool] {
            let hooks = vec![hook(event, "*", "exit 3", true, 5)];
            let ctx = HookContext::lifecycle(event, "", None, work_dir());
            assert_eq!(run_hooks(&hooks, &ctx).await, HookOutcome::default());
        }
    }

    #[tokio::test]
    async fn feedback_needs_the_flag_a_speaking_event_and_output() {
        let hooks = vec![
            hook(HookEvent::PostTool, "*", "echo quiet", false, 5),
            speaking(HookEvent::PostTool, "*", "exit 0"),
            speaking(HookEvent::PostTool, "*", "echo lint: 2 warnings"),
            speaking(HookEvent::Stop, "*", "echo ignored"),
        ];
        let ctx = HookContext::post("edit", "id", "{}", None, work_dir(), false, "ok");
        let feedback = run_post_hooks(&hooks, &ctx).await;
        assert_eq!(feedback.as_deref(), Some("lint: 2 warnings"));
        let stop = HookContext::lifecycle(HookEvent::Stop, "", None, work_dir());
        assert_eq!(run_hooks(&hooks, &stop).await.feedback_text(), None);

        let mut body = "edited".to_string();
        append_feedback(&mut body, None);
        assert_eq!(body, "edited");
        append_feedback(&mut body, feedback);
        assert_eq!(body, "edited\n\n[post_tool hook]\nlint: 2 warnings");
    }

    #[test]
    fn subagent_stop_filters_by_agent_type() {
        let hooks = vec![
            hook(HookEvent::SubagentStop, "explore", "echo x", false, 5),
            hook(HookEvent::SessionStart, "explore", "echo x", false, 5),
        ];
        assert_eq!(
            matching_hooks(&hooks, HookEvent::SubagentStop, "explore").len(),
            1
        );
        assert!(matching_hooks(&hooks, HookEvent::SubagentStop, "build").is_empty());
        assert_eq!(matching_hooks(&hooks, HookEvent::SessionStart, "").len(), 1);
        let ctx = HookContext::lifecycle(HookEvent::Stop, "", None, ".")
            .with_env("WHYCODES_RESPONSE", &"r".repeat(OUTPUT_ENV_MAX + 1));
        assert!(ctx.extra_env[0].1.ends_with("…[truncated]"));
        assert_eq!(event_env(HookEvent::SubagentStop), "subagent_stop");
    }
}
