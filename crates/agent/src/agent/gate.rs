//! Tool-call scheduling and permission / risk / hook / sandbox gates.

use whycodes_command_risk::{Decision, assess, decide};
use whycodes_core::todo::{has_open, load_todos};
use whycodes_core::tool::ToolContext;
use whycodes_core::types::{ApprovalMode, PermissionAction, ToolCall, ToolResult};
use whycodes_plugin::hooks::{HookContext, PreHookDecision, run_post_hooks, run_pre_hooks};
use whycodes_session::session::Session;

use crate::events::{CancelFlag, EventSink, TurnEvent, emit, is_cancelled, wait_until_cancelled};
use crate::question::{QuestionPrompter, run_question_tool, should_prompt_questions};
use crate::tool_policy::*;
use whycodes_tools::question::parse_questions;

use super::Agent;

/// Extra tool attempts in auto mode after the first failure (total tries = 1 + this).
pub(crate) const AUTO_TOOL_RETRY_LIMIT: u32 = 2;

fn session_has_open_todos(session: &Session) -> bool {
    let sid = session.id.trim();
    let sid = if sid.is_empty() { None } else { Some(sid) };
    has_open(&load_todos(&session.project_path, sid))
}

fn refuse_question_open_work(tool_call_id: &str, detail: &str) -> ToolResult {
    ToolResult {
        tool_call_id: tool_call_id.to_string(),
        content: format!(
            "Auto mode: do not ask the user while {detail} remain open. \
             Keep working: retry the failed step, mark todos completed, \
             or cancel items that no longer apply. Ask only after the list is done."
        ),
        is_error: true,
    }
}

fn tool_error_is_retryable(name: &str, result: &ToolResult) -> bool {
    if !result.is_error {
        return false;
    }
    match name {
        "question" | "task" | "swarm" | "bg" | "schedule" | "worktree" | "todowrite" | "todo"
        | "todoread" | "checkpoint" | "rewind" => false,
        _ => {
            let c = result.content.to_ascii_lowercase();
            !(c.contains("permission denied")
                || c.contains("user denied")
                || c.contains("refused")
                || c.contains("doom loop")
                || c.contains("cannot be approved")
                || c.contains("catastrophic"))
        }
    }
}

impl Agent {
    /// Run a batch of tool calls, parallelizing independent read-only tools.
    ///
    /// Results are returned in the **same order** as `tool_calls` (required by
    /// the messages API). Shell, mutators, and tools that need an interactive
    /// permission ask stay sequential so risk/UI semantics stay single-threaded.
    ///
    /// `speculative` holds early `read` jobs started while args were still
    /// streaming; matching calls skip a second disk pass.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn execute_tool_calls(
        &self,
        tool_calls: &[ToolCall],
        session: &Session,
        tool_ctx: &ToolContext,
        provider_name: &str,
        model: &str,
        api_key: &str,
        events: &Option<EventSink>,
        cancel: &Option<CancelFlag>,
        turn_intent: Option<&crate::intent::IntentAssessment>,
        speculative: &mut Vec<crate::speculative_read::SpeculativeRead>,
    ) -> whycodes_core::Result<Vec<ToolResult>> {
        if tool_calls.is_empty() {
            return Ok(Vec::new());
        }

        // Single call — no fan-out overhead.
        if tool_calls.len() == 1 {
            let tc = &tool_calls[0];
            if is_cancelled(cancel) {
                emit(events, TurnEvent::Cancelled);
                return Err(whycodes_core::Error::Agent("Cancelled".into()));
            }
            emit(
                events,
                TurnEvent::Status(format!("Running tool `{}`…", tc.name)),
            );
            let result =
                if let Some(early) = self.take_speculative_read(tc, tool_ctx, speculative).await {
                    early
                } else {
                    tokio::select! {
                        biased;
                        _ = wait_until_cancelled(cancel) => {
                            emit(events, TurnEvent::Cancelled);
                            return Err(whycodes_core::Error::Agent("Cancelled".into()));
                        }
                        r = self.execute_with_permission(
                            tc,
                            session,
                            tool_ctx,
                            provider_name,
                            model,
                            api_key,
                            turn_intent,
                            events.as_ref(),
                        ) => r,
                    }
                };
            emit(
                events,
                TurnEvent::ToolEnd {
                    id: tc.id.clone(),
                    content: result.content.clone(),
                    is_error: result.is_error,
                },
            );
            return Ok(vec![result]);
        }

        let all_parallel = tool_calls
            .iter()
            .all(|tc| is_parallel_safe_tool(&tc.name, &self.info.permission));

        if all_parallel {
            let names: Vec<&str> = tool_calls.iter().map(|t| t.name.as_str()).collect();
            emit(
                events,
                TurnEvent::Status(format!(
                    "Running {} tools in parallel: {}…",
                    tool_calls.len(),
                    names.join(", ")
                )),
            );
            // Consume matching speculative reads first (I/O already overlapped
            // the LLM stream). Remaining calls run in parallel as before.
            let mut early: Vec<Option<ToolResult>> = Vec::with_capacity(tool_calls.len());
            for tc in tool_calls {
                early.push(self.take_speculative_read(tc, tool_ctx, speculative).await);
            }
            // ToolStart already emitted by the caller for every call.
            let futs: Vec<_> = tool_calls
                .iter()
                .zip(early)
                .map(|(tc, pre)| {
                    let this = self;
                    async move {
                        if let Some(r) = pre {
                            return r;
                        }
                        this.execute_with_permission(
                            tc,
                            session,
                            tool_ctx,
                            provider_name,
                            model,
                            api_key,
                            turn_intent,
                            events.as_ref(),
                        )
                        .await
                    }
                })
                .collect();
            let results = tokio::select! {
                biased;
                _ = wait_until_cancelled(cancel) => {
                    emit(events, TurnEvent::Cancelled);
                    return Err(whycodes_core::Error::Agent("Cancelled".into()));
                }
                r = futures::future::join_all(futs) => r,
            };
            for (tc, result) in tool_calls.iter().zip(results.iter()) {
                emit(
                    events,
                    TurnEvent::ToolEnd {
                        id: tc.id.clone(),
                        content: result.content.clone(),
                        is_error: result.is_error,
                    },
                );
            }
            return Ok(results);
        }

        // Mixed or unsafe batch — sequential (correct + simple).
        let mut results = Vec::with_capacity(tool_calls.len());
        for tc in tool_calls {
            if is_cancelled(cancel) {
                emit(events, TurnEvent::Cancelled);
                return Err(whycodes_core::Error::Agent("Cancelled".into()));
            }
            emit(
                events,
                TurnEvent::Status(format!("Running tool `{}`…", tc.name)),
            );
            let result =
                if let Some(early) = self.take_speculative_read(tc, tool_ctx, speculative).await {
                    early
                } else {
                    tokio::select! {
                        biased;
                        _ = wait_until_cancelled(cancel) => {
                            emit(events, TurnEvent::Cancelled);
                            return Err(whycodes_core::Error::Agent("Cancelled".into()));
                        }
                        r = self.execute_with_permission(
                            tc,
                            session,
                            tool_ctx,
                            provider_name,
                            model,
                            api_key,
                            turn_intent,
                            events.as_ref(),
                        ) => r,
                    }
                };
            emit(
                events,
                TurnEvent::ToolEnd {
                    id: tc.id.clone(),
                    content: result.content.clone(),
                    is_error: result.is_error,
                },
            );
            results.push(result);
        }
        Ok(results)
    }

    /// Use a speculative early `read` if path/window still match the final call.
    async fn take_speculative_read(
        &self,
        tc: &ToolCall,
        tool_ctx: &ToolContext,
        speculative: &mut Vec<crate::speculative_read::SpeculativeRead>,
    ) -> Option<ToolResult> {
        if tc.name != "read" || speculative.is_empty() {
            return None;
        }
        let path = tc.arguments.get("path")?.as_str()?;
        let (offset, limit) = crate::speculative_read::window_from_args(&tc.arguments);
        let result = crate::speculative_read::take_matching(
            speculative,
            &tc.id,
            path,
            offset,
            limit,
            &tool_ctx.working_dir,
        )
        .await?;
        tracing::debug!(id = %tc.id, path, "speculative early read hit");
        Some(result)
    }

    /// Whether this permission `ask` is high-risk under `important` mode.
    ///
    /// High-risk: `browser`; `bash`/`shell` at or above `bash_risk_threshold`;
    /// file tools whose path is outside the workspace. Never used to skip
    /// deny / catastrophic / sandbox — those gates run first.
    fn approval_ask_is_high_risk(&self, tc: &ToolCall, working_dir: &str) -> bool {
        match tc.name.as_str() {
            "browser" => true,
            "bash" | "shell" => {
                let command = tc
                    .arguments
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                !matches!(
                    decide(
                        &assess(command, std::path::Path::new(working_dir)),
                        self.risk_threshold,
                    ),
                    Decision::Allow
                )
            }
            "schedule" => tc
                .arguments
                .get("command")
                .and_then(|v| v.as_str())
                .filter(|c| !c.trim().is_empty())
                .is_some_and(|command| {
                    !matches!(
                        decide(
                            &assess(command, std::path::Path::new(working_dir)),
                            self.risk_threshold,
                        ),
                        Decision::Allow
                    )
                }),
            _ => file_tool_path(tc).is_some_and(|path| path_outside_workspace(&path, working_dir)),
        }
    }

    /// Overlay: `auto` skips every ask; `important` skips low-risk asks;
    /// `manual` never skips. Deny / catastrophic still refuse above this.
    fn approval_skips_ask(&self, tc: &ToolCall, working_dir: &str) -> bool {
        match self.approval_mode {
            ApprovalMode::Auto => true,
            ApprovalMode::Manual => false,
            ApprovalMode::Important => !self.approval_ask_is_high_risk(tc, working_dir),
        }
    }

    /// Open session todos or live background jobs — auto mode must not interrupt.
    fn auto_has_open_work(&self, session: &Session) -> Option<&'static str> {
        if session_has_open_todos(session) {
            return Some("todos");
        }
        if self.background.running_count() > 0 {
            return Some("background tasks");
        }
        None
    }

    async fn ask_permission(&self, tc: &ToolCall, working_dir: &str, detail: &str) -> bool {
        if self.approval_skips_ask(tc, working_dir) {
            return true;
        }
        self.permission_prompter.ask(&tc.name, detail).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn dispatch_tool(
        &self,
        tc: &ToolCall,
        session: &Session,
        tool_ctx: &ToolContext,
        provider_name: &str,
        model: &str,
        api_key: &str,
        events: Option<&EventSink>,
    ) -> ToolResult {
        if tc.name == "task" {
            self.execute_task_tool(tc, session, provider_name, model, api_key, events)
                .await
        } else if tc.name == "swarm" {
            self.execute_swarm_tool(tc, session, provider_name, model, api_key, events)
                .await
        } else if tc.name == "bg" {
            self.execute_bg_tool(tc)
        } else if tc.name == "schedule" {
            self.execute_schedule_tool(tc, tool_ctx, events).await
        } else if tc.name == "tool_search" {
            self.execute_tool_search(tc)
        } else if tc.name == "worktree" {
            self.execute_worktree_tool(tc, session)
        } else if (tc.name == "bash" || tc.name == "shell")
            && tc
                .arguments
                .get("background")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        {
            self.execute_background_shell(tc, tool_ctx, events)
        } else {
            self.tool_executor
                .execute(tc, tool_ctx, &self.info.permission)
                .await
        }
    }

    /// Apply the shell risk gate, then allow/ask/deny, then execute (or spawn
    /// a task subagent).
    ///
    /// `pub(crate)` so the risk gate can be tested at this level: the unit
    /// tests in `command-risk` cover classification, but only this method
    /// proves that a catastrophic command is refused even when the permission
    /// map says `allow`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn execute_with_permission(
        &self,
        tc: &ToolCall,
        session: &Session,
        tool_ctx: &ToolContext,
        provider_name: &str,
        model: &str,
        api_key: &str,
        turn_intent: Option<&crate::intent::IntentAssessment>,
        events: Option<&EventSink>,
    ) -> ToolResult {
        // Questionnaire: UI-backed channel (TUI) or stdin/auto — never race in
        // parallel with other tools (SERIAL_TOOLS). Skip permission map; asking
        // the user *is* the interaction.
        if tc.name == "question" {
            if self.approval_mode == ApprovalMode::Auto
                && let Some(kind) = self.auto_has_open_work(session)
            {
                return refuse_question_open_work(&tc.id, kind);
            }
            let questions = match parse_questions(&tc.arguments) {
                Ok(q) => q,
                Err(e) => {
                    return ToolResult {
                        tool_call_id: tc.id.clone(),
                        content: format!("Invalid questionnaire: {e}"),
                        is_error: true,
                    };
                }
            };
            let must_prompt = should_prompt_questions(self.approval_mode, &questions);
            let prompter: &dyn QuestionPrompter = if must_prompt {
                self.question_prompter.as_ref()
            } else {
                &crate::question::AutoAnswerPrompter
            };
            return run_question_tool(prompter, &tc.arguments, &tc.id).await;
        }

        // Shell commands (and `schedule` with a delayed shell payload) are gated
        // on what the command would destroy. The permission map below only sees
        // the tool name, so on its own `allow` would run anything the model emits.
        // Shell-scoped rules (`bash(git *)`) can skip or force prompts for Safe cmds.
        let mut risk_confirmed = false;
        let scheduled_shell = (tc.name == "schedule")
            .then(|| {
                tc.arguments
                    .get("command")
                    .and_then(|v| v.as_str())
                    .filter(|c| !c.trim().is_empty())
            })
            .flatten();
        if SHELL_TOOLS.contains(&tc.name.as_str()) || scheduled_shell.is_some() {
            let command = scheduled_shell.unwrap_or_else(|| {
                tc.arguments
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
            });
            let assessment = assess(command, std::path::Path::new(&tool_ctx.working_dir));

            match decide(&assessment, self.risk_threshold) {
                Decision::Allow => {}
                Decision::Refuse { reason } => {
                    tracing::warn!(command, reason, "refused catastrophic shell command");
                    return ToolResult {
                        tool_call_id: tc.id.clone(),
                        content: format!(
                            "Refused: {reason}.\n\
                             This command is classified catastrophic and cannot be approved. \
                             Run it yourself if you are certain."
                        ),
                        is_error: true,
                    };
                }
                Decision::Confirm { reason } => {
                    // Structured for the TUI permission dialog (see format_permission_detail).
                    let detail = format_shell_risk_detail(command, &reason);
                    if !self
                        .ask_permission(tc, &tool_ctx.working_dir, &detail)
                        .await
                    {
                        return ToolResult {
                            tool_call_id: tc.id.clone(),
                            content: format!("User denied permission for tool '{}'.", tc.name),
                            is_error: true,
                        };
                    }
                    risk_confirmed = true;
                }
            }

            // Shell-scoped permission rules (Claude Code `Bash(git *)` spirit).
            if let Some(shell_act) = self.info.permission.action_for_shell(command) {
                match shell_act {
                    PermissionAction::Deny => {
                        return ToolResult {
                            tool_call_id: tc.id.clone(),
                            content: format!(
                                "Permission denied for shell command by rule matching `{command}`."
                            ),
                            is_error: true,
                        };
                    }
                    PermissionAction::Allow => {
                        // Safe path only: skip further tool-level Ask when risk allowed.
                        // Destructive Confirm already handled above.
                        if matches!(decide(&assessment, self.risk_threshold), Decision::Allow) {
                            risk_confirmed = true;
                        }
                    }
                    PermissionAction::Ask if !risk_confirmed => {
                        let detail =
                            format!("Shell rule requires confirmation\n\nCommand:\n{command}");
                        if !self
                            .ask_permission(tc, &tool_ctx.working_dir, &detail)
                            .await
                        {
                            return ToolResult {
                                tool_call_id: tc.id.clone(),
                                content: format!("User denied permission for tool '{}'.", tc.name),
                                is_error: true,
                            };
                        }
                        risk_confirmed = true;
                    }
                    PermissionAction::Ask => {}
                }
            }
        }

        // Intent authorization (Claude-style): question/plan/ambiguous-always
        // turns must not silently mutate. After blast-radius, before permission.
        if let Some(intent) = turn_intent {
            let command = tc.arguments.get("command").and_then(|v| v.as_str());
            match crate::intent::authorize_tool(
                intent,
                &self.info.name,
                &tc.name,
                command,
                self.intent_guidance,
            ) {
                crate::intent::ToolAuthDecision::Allow => {}
                crate::intent::ToolAuthDecision::Refuse { reason } => {
                    tracing::info!(
                        tool = %tc.name,
                        intent = intent.intent.as_str(),
                        "intent auth refused tool"
                    );
                    return ToolResult {
                        tool_call_id: tc.id.clone(),
                        content: format!("Refused (intent): {reason}"),
                        is_error: true,
                    };
                }
                crate::intent::ToolAuthDecision::Confirm { reason } => {
                    if !risk_confirmed {
                        let detail = format_permission_detail(&tc.arguments);
                        let body = format!("{detail}\n\nIntent check:\n{reason}");
                        if !self.ask_permission(tc, &tool_ctx.working_dir, &body).await {
                            return ToolResult {
                                tool_call_id: tc.id.clone(),
                                content: format!(
                                    "User denied permission for tool '{}' (intent gate).",
                                    tc.name
                                ),
                                is_error: true,
                            };
                        }
                        risk_confirmed = true;
                    }
                }
            }
        }

        // Path-scoped rules: `edit(src/**)`, `write(docs/**)`, …
        if let Some(path) = file_tool_path(tc)
            && let Some(path_act) = self.info.permission.action_for_path(&tc.name, &path)
        {
            match path_act {
                PermissionAction::Deny => {
                    return ToolResult {
                        tool_call_id: tc.id.clone(),
                        content: format!(
                            "Permission denied for `{}` on path `{path}` by path rule.",
                            tc.name
                        ),
                        is_error: true,
                    };
                }
                PermissionAction::Allow => {
                    risk_confirmed = true;
                }
                PermissionAction::Ask if !risk_confirmed => {
                    let detail = format!(
                        "Path rule requires confirmation\n\nTool: {}\nPath: {path}",
                        tc.name
                    );
                    if !self
                        .ask_permission(tc, &tool_ctx.working_dir, &detail)
                        .await
                    {
                        return ToolResult {
                            tool_call_id: tc.id.clone(),
                            content: format!("User denied permission for tool '{}'.", tc.name),
                            is_error: true,
                        };
                    }
                    risk_confirmed = true;
                }
                PermissionAction::Ask => {}
            }
        }

        match self.info.permission.action_for(&tc.name) {
            PermissionAction::Deny => {
                return ToolResult {
                    tool_call_id: tc.id.clone(),
                    content: format!(
                        "Permission denied for tool '{}'. Adjust agent permissions or config.permission.",
                        tc.name
                    ),
                    is_error: true,
                };
            }
            // Already confirmed with the command in hand; do not ask twice.
            PermissionAction::Ask if risk_confirmed => {}
            PermissionAction::Ask => {
                let detail = format_permission_detail(&tc.arguments);
                let allowed = self
                    .ask_permission(tc, &tool_ctx.working_dir, &detail)
                    .await;
                if !allowed {
                    return ToolResult {
                        tool_call_id: tc.id.clone(),
                        content: format!("User denied permission for tool '{}'.", tc.name),
                        is_error: true,
                    };
                }
            }
            PermissionAction::Allow => {}
        }

        // Pre-tool hooks (after risk + permission, before execution).
        let tool_input = tc.arguments.to_string();
        let pre_ctx = HookContext::pre(
            tc.name.clone(),
            tc.id.clone(),
            tool_input.clone(),
            Some(session.id.clone()),
            tool_ctx.working_dir.clone(),
        );
        match run_pre_hooks(&self.hooks, &pre_ctx).await {
            PreHookDecision::Allow => {}
            PreHookDecision::Block { reason } => {
                return ToolResult {
                    tool_call_id: tc.id.clone(),
                    content: reason,
                    is_error: true,
                };
            }
        }

        let mut result = self
            .dispatch_tool(tc, session, tool_ctx, provider_name, model, api_key, events)
            .await;

        if self.approval_mode == ApprovalMode::Auto {
            let mut attempt = 0;
            while attempt < AUTO_TOOL_RETRY_LIMIT && tool_error_is_retryable(&tc.name, &result) {
                attempt += 1;
                tracing::info!(
                    tool = %tc.name,
                    attempt,
                    "auto mode retrying failed tool"
                );
                if let Some(sink) = events {
                    emit(
                        &Some(sink.clone()),
                        TurnEvent::Status(format!(
                            "Auto: retrying `{}` ({attempt}/{AUTO_TOOL_RETRY_LIMIT})…",
                            tc.name
                        )),
                    );
                }
                result = self
                    .dispatch_tool(tc, session, tool_ctx, provider_name, model, api_key, events)
                    .await;
            }
        }

        // Post-tool hooks never block; failures are logged inside the runner.
        let post_ctx = HookContext::post(
            tc.name.clone(),
            tc.id.clone(),
            tool_input,
            Some(session.id.clone()),
            tool_ctx.working_dir.clone(),
            result.is_error,
            &result.content,
        );
        run_post_hooks(&self.hooks, &post_ctx).await;

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::PermissionPrompter;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use whycodes_core::Tool;
    use whycodes_core::types::{
        AgentInfo, AgentMode, ApprovalMode, PermissionAction, PermissionSet, ToolCall,
    };

    fn info(name: &str) -> AgentInfo {
        AgentInfo {
            name: name.to_string(),
            description: format!("Test agent: {name}"),
            mode: AgentMode::Primary,
            permission: PermissionSet {
                allowed_tools: None,
                denied_tools: None,
                allow_file_writes: true,
                allow_network: true,
                allow_shell: true,
                allowed_paths: None,
                rules: Default::default(),
            },
            model: None,
            system_prompt: Some("You are a test agent.".to_string()),
            temperature: Some(0.5),
            top_p: None,
        }
    }

    fn agent() -> Agent {
        Agent::new(info("build"))
    }

    fn tc(name: &str, args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "t1".into(),
            name: name.into(),
            arguments: args,
        }
    }

    fn session_ctx(agent: &Agent) -> (Session, ToolContext) {
        let session = Session::new(std::path::PathBuf::from("/work/proj"), "test".into());
        let ctx = agent.tool_context(&session);
        (session, ctx)
    }

    struct CountingDenyPrompter {
        asks: AtomicUsize,
    }

    impl PermissionPrompter for CountingDenyPrompter {
        fn ask<'a>(
            &'a self,
            _tool_name: &'a str,
            _detail: &'a str,
        ) -> crate::permission::PermissionAskFuture<'a> {
            Box::pin(async move {
                self.asks.fetch_add(1, Ordering::SeqCst);
                false
            })
        }
    }

    struct FlakyRead {
        hits: Arc<AtomicUsize>,
    }

    impl whycodes_core::Tool for FlakyRead {
        fn name(&self) -> &str {
            "read"
        }
        fn description(&self) -> &str {
            "flaky"
        }
        fn parameters(&self) -> serde_json::Value {
            json!({"type": "object"})
        }
        fn execute<'a>(
            &'a self,
            _args: serde_json::Value,
            _ctx: &'a whycodes_core::ToolContext,
        ) -> whycodes_core::ToolFuture<'a> {
            let hits = Arc::clone(&self.hits);
            Box::pin(async move {
                let n = hits.fetch_add(1, Ordering::SeqCst) + 1;
                if n < 3 {
                    ToolResult {
                        tool_call_id: String::new(),
                        content: format!("transient fail {n}"),
                        is_error: true,
                    }
                } else {
                    ToolResult {
                        tool_call_id: String::new(),
                        content: "ok".into(),
                        is_error: false,
                    }
                }
            })
        }
    }

    #[test]
    fn retryable_skips_policy_and_question() {
        let err = |c: &str| ToolResult {
            tool_call_id: "t".into(),
            content: c.into(),
            is_error: true,
        };
        assert!(!tool_error_is_retryable(
            "read",
            &ToolResult {
                tool_call_id: "t".into(),
                content: "ok".into(),
                is_error: false,
            }
        ));
        assert!(tool_error_is_retryable("read", &err("transient fail 1")));
        assert!(!tool_error_is_retryable("question", &err("do not ask")));
        assert!(!tool_error_is_retryable("task", &err("subagent failed")));
        assert!(!tool_error_is_retryable("swarm", &err("x")));
        assert!(!tool_error_is_retryable("bg", &err("x")));
        assert!(!tool_error_is_retryable("schedule", &err("x")));
        assert!(!tool_error_is_retryable("worktree", &err("x")));
        assert!(!tool_error_is_retryable("todowrite", &err("x")));
        assert!(!tool_error_is_retryable("todo", &err("x")));
        assert!(!tool_error_is_retryable("todoread", &err("x")));
        assert!(!tool_error_is_retryable("checkpoint", &err("x")));
        assert!(!tool_error_is_retryable("rewind", &err("x")));
        assert!(!tool_error_is_retryable(
            "read",
            &err("Permission denied for tool 'read'.")
        ));
        assert!(!tool_error_is_retryable("bash", &err("user denied")));
        assert!(!tool_error_is_retryable("bash", &err("doom loop")));
        assert!(!tool_error_is_retryable("bash", &err("cannot be approved")));
        assert!(!tool_error_is_retryable("bash", &err("catastrophic")));
        assert!(!tool_error_is_retryable(
            "bash",
            &err("Refused: catastrophic")
        ));
        assert_eq!(AUTO_TOOL_RETRY_LIMIT, 2);
    }

    #[tokio::test]
    async fn dispatch_tool_special_names_via_permission() {
        let mut a = agent();
        a.swarm_enabled = false;
        let (session, ctx) = session_ctx(&a);

        let bg = a
            .execute_with_permission(
                &tc("bg", json!({"action": "list"})),
                &session,
                &ctx,
                "script",
                "m",
                "k",
                None,
                None,
            )
            .await;
        assert!(bg.content.contains("No background jobs"), "{bg:?}");

        let search = a
            .execute_with_permission(
                &tc("tool_search", json!({"action": "list"})),
                &session,
                &ctx,
                "script",
                "m",
                "k",
                None,
                None,
            )
            .await;
        assert!(!search.is_error, "{search:?}");

        let wt = a
            .execute_with_permission(
                &tc("worktree", json!({"action": "nope"})),
                &session,
                &ctx,
                "script",
                "m",
                "k",
                None,
                None,
            )
            .await;
        assert!(wt.is_error, "{wt:?}");

        let sched = a
            .execute_with_permission(
                &tc("schedule", json!({})),
                &session,
                &ctx,
                "script",
                "m",
                "k",
                None,
                None,
            )
            .await;
        assert!(sched.is_error, "{sched:?}");

        let task = a
            .execute_with_permission(
                &tc("task", json!({})),
                &session,
                &ctx,
                "script",
                "m",
                "k",
                None,
                None,
            )
            .await;
        assert!(task.is_error, "{task:?}");

        let swarm = a
            .execute_with_permission(
                &tc("swarm", json!({"tasks": [{"goal": "x"}]})),
                &session,
                &ctx,
                "script",
                "m",
                "k",
                None,
                None,
            )
            .await;
        assert!(swarm.is_error, "{swarm:?}");
        assert!(
            swarm.content.to_lowercase().contains("disabled"),
            "{swarm:?}"
        );

        let bg_shell = a
            .execute_with_permission(
                &tc("bash", json!({"command": "", "background": true})),
                &session,
                &ctx,
                "script",
                "m",
                "k",
                None,
                None,
            )
            .await;
        assert!(bg_shell.is_error, "{bg_shell:?}");
        assert!(
            bg_shell.content.to_lowercase().contains("command"),
            "{bg_shell:?}"
        );
    }

    #[tokio::test]
    async fn confirm_class_bash_denied_by_prompter() {
        let prompter = Arc::new(CountingDenyPrompter {
            asks: AtomicUsize::new(0),
        });
        let mut a = Agent::new(info("build")).with_permission_prompter(prompter.clone());
        a.set_approval_mode(ApprovalMode::Manual);
        let (session, ctx) = session_ctx(&a);
        let result = a
            .execute_with_permission(
                &tc("bash", json!({"command": "rm -rf /tmp/scratch"})),
                &session,
                &ctx,
                "script",
                "m",
                "k",
                None,
                None,
            )
            .await;
        assert_eq!(prompter.asks.load(Ordering::SeqCst), 1);
        assert!(
            result.content.contains("User denied permission"),
            "{}",
            result.content
        );
    }

    #[tokio::test]
    async fn shell_rule_deny_allow_and_ask() {
        let mut deny_info = info("build");
        deny_info
            .permission
            .rules
            .insert("bash(git *)".into(), PermissionAction::Deny);
        let deny = Agent::new(deny_info);
        let (session, ctx) = session_ctx(&deny);
        let denied = deny
            .execute_with_permission(
                &tc("bash", json!({"command": "git status"})),
                &session,
                &ctx,
                "script",
                "m",
                "k",
                None,
                None,
            )
            .await;
        assert!(denied.is_error, "{denied:?}");
        assert!(
            denied.content.contains("Permission denied for shell"),
            "{}",
            denied.content
        );

        let asks = Arc::new(CountingDenyPrompter {
            asks: AtomicUsize::new(0),
        });
        let mut allow_info = info("build");
        allow_info
            .permission
            .rules
            .insert("bash(git *)".into(), PermissionAction::Allow);
        let mut allow = Agent::new(allow_info).with_permission_prompter(asks.clone());
        allow.set_approval_mode(ApprovalMode::Manual);
        let (session, ctx) = session_ctx(&allow);
        let _ = allow
            .execute_with_permission(
                &tc("bash", json!({"command": "git status"})),
                &session,
                &ctx,
                "script",
                "m",
                "k",
                None,
                None,
            )
            .await;
        assert_eq!(asks.asks.load(Ordering::SeqCst), 0);

        let ask_p = Arc::new(CountingDenyPrompter {
            asks: AtomicUsize::new(0),
        });
        let mut ask_info = info("build");
        ask_info
            .permission
            .rules
            .insert("bash(git status)".into(), PermissionAction::Ask);
        let mut ask_agent = Agent::new(ask_info).with_permission_prompter(ask_p.clone());
        ask_agent.set_approval_mode(ApprovalMode::Manual);
        let (session, ctx) = session_ctx(&ask_agent);
        let asked = ask_agent
            .execute_with_permission(
                &tc("bash", json!({"command": "git status"})),
                &session,
                &ctx,
                "script",
                "m",
                "k",
                None,
                None,
            )
            .await;
        assert_eq!(ask_p.asks.load(Ordering::SeqCst), 1);
        assert!(
            asked.content.contains("User denied permission"),
            "{}",
            asked.content
        );
    }

    #[tokio::test]
    async fn intent_refuse_and_confirm_deny() {
        let ask = Agent::new(info("ask"));
        let (session, ctx) = session_ctx(&ask);
        let intent = crate::intent::classify_user_intent("please write a file");
        let refused = ask
            .execute_with_permission(
                &tc("write", json!({"path": "a.md", "content": "x"})),
                &session,
                &ctx,
                "script",
                "m",
                "k",
                Some(&intent),
                None,
            )
            .await;
        assert!(refused.is_error, "{refused:?}");
        assert!(
            refused.content.contains("intent") || refused.content.contains("read-only"),
            "{}",
            refused.content
        );

        let deny = Arc::new(CountingDenyPrompter {
            asks: AtomicUsize::new(0),
        });
        let mut build = Agent::new(info("build")).with_permission_prompter(deny.clone());
        build.set_approval_mode(ApprovalMode::Manual);
        let (session, ctx) = session_ctx(&build);
        let q = crate::intent::classify_user_intent("how does auth work?");
        let confirmed = build
            .execute_with_permission(
                &tc("write", json!({"path": "a.md", "content": "x"})),
                &session,
                &ctx,
                "script",
                "m",
                "k",
                Some(&q),
                None,
            )
            .await;
        assert_eq!(deny.asks.load(Ordering::SeqCst), 1);
        assert!(
            confirmed.content.contains("intent gate"),
            "{}",
            confirmed.content
        );
    }

    #[tokio::test]
    async fn path_rule_deny_allow_and_ask() {
        let mut deny_info = info("build");
        deny_info
            .permission
            .rules
            .insert("edit(src/**)".into(), PermissionAction::Deny);
        let deny = Agent::new(deny_info);
        let (session, ctx) = session_ctx(&deny);
        let denied = deny
            .execute_with_permission(
                &tc(
                    "edit",
                    json!({"path": "src/lib.rs", "old_string": "a", "new_string": "b"}),
                ),
                &session,
                &ctx,
                "script",
                "m",
                "k",
                None,
                None,
            )
            .await;
        assert!(denied.is_error, "{denied:?}");
        assert!(denied.content.contains("path rule"), "{}", denied.content);

        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("docs")).unwrap();
        let asks = Arc::new(CountingDenyPrompter {
            asks: AtomicUsize::new(0),
        });
        let mut allow_info = info("build");
        allow_info
            .permission
            .rules
            .insert("write(docs/**)".into(), PermissionAction::Allow);
        let mut allow = Agent::new(allow_info).with_permission_prompter(asks.clone());
        allow.set_approval_mode(ApprovalMode::Manual);
        let session = Session::new(dir.path().to_path_buf(), "test".into());
        let ctx = allow.tool_context(&session);
        let _ = allow
            .execute_with_permission(
                &tc("write", json!({"path": "docs/a.md", "content": "ok"})),
                &session,
                &ctx,
                "script",
                "m",
                "k",
                None,
                None,
            )
            .await;
        assert_eq!(asks.asks.load(Ordering::SeqCst), 0);

        let ask_p = Arc::new(CountingDenyPrompter {
            asks: AtomicUsize::new(0),
        });
        let mut ask_info = info("build");
        ask_info
            .permission
            .rules
            .insert("write(docs/**)".into(), PermissionAction::Ask);
        let mut ask_agent = Agent::new(ask_info).with_permission_prompter(ask_p.clone());
        ask_agent.set_approval_mode(ApprovalMode::Manual);
        let (session, ctx) = session_ctx(&ask_agent);
        let asked = ask_agent
            .execute_with_permission(
                &tc("write", json!({"path": "docs/a.md", "content": "ok"})),
                &session,
                &ctx,
                "script",
                "m",
                "k",
                None,
                None,
            )
            .await;
        assert_eq!(ask_p.asks.load(Ordering::SeqCst), 1);
        assert!(
            asked.content.contains("User denied permission"),
            "{}",
            asked.content
        );
    }

    #[tokio::test]
    async fn invalid_question_and_background_open_work() {
        let mut a = agent();
        a.set_approval_mode(ApprovalMode::Auto);
        let (session, ctx) = session_ctx(&a);
        let invalid = a
            .execute_with_permission(
                &tc("question", json!({})),
                &session,
                &ctx,
                "script",
                "m",
                "k",
                None,
                None,
            )
            .await;
        assert!(invalid.is_error, "{invalid:?}");
        assert!(
            invalid.content.contains("Invalid questionnaire"),
            "{}",
            invalid.content
        );

        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(dir.path().to_path_buf(), "test".into());
        let ctx = a.tool_context(&session);
        let started = a.execute_background_shell(
            &tc(
                "bash",
                json!({"command": "sleep 30", "description": "hold"}),
            ),
            &ctx,
            None,
        );
        assert!(!started.is_error, "{started:?}");
        let q = a
            .execute_with_permission(
                &tc(
                    "question",
                    json!({"question": "Pick", "choices": ["A", "B"]}),
                ),
                &session,
                &ctx,
                "script",
                "m",
                "k",
                None,
                None,
            )
            .await;
        a.background.kill_all();
        assert!(q.is_error, "{q:?}");
        assert!(q.content.contains("background tasks"), "{}", q.content);
    }

    #[tokio::test]
    async fn auto_retry_emits_status() {
        let hits = Arc::new(AtomicUsize::new(0));
        let mut exec = whycodes_tools::executor::ToolExecutor::new();
        exec.register(Box::new(FlakyRead {
            hits: Arc::clone(&hits),
        }));
        let mut a = Agent::new(info("build")).with_tool_executor(exec);
        a.set_approval_mode(ApprovalMode::Auto);
        let (session, ctx) = session_ctx(&a);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let result = a
            .execute_with_permission(
                &tc("read", json!({"path": "x"})),
                &session,
                &ctx,
                "script",
                "m",
                "k",
                None,
                Some(&tx),
            )
            .await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(hits.load(Ordering::SeqCst), 3);
        let mut saw = false;
        while let Ok(ev) = rx.try_recv() {
            if let TurnEvent::Status(s) = ev
                && s.contains("Auto: retrying")
            {
                saw = true;
            }
        }
        assert!(saw, "expected Auto: retrying status");
    }

    #[tokio::test]
    async fn execute_tool_calls_empty_and_cancel() {
        let a = agent();
        let (session, ctx) = session_ctx(&a);
        let empty = a
            .execute_tool_calls(
                &[],
                &session,
                &ctx,
                "script",
                "m",
                "k",
                &None,
                &None,
                None,
                &mut Vec::new(),
            )
            .await
            .expect("empty");
        assert!(empty.is_empty());

        let cancel = crate::events::new_cancel_flag();
        crate::events::request_cancel(&cancel);
        let single = a
            .execute_tool_calls(
                &[tc("read", json!({"path": "x"}))],
                &session,
                &ctx,
                "script",
                "m",
                "k",
                &None,
                &Some(cancel.clone()),
                None,
                &mut Vec::new(),
            )
            .await;
        assert!(single.is_err());

        let parallel = a
            .execute_tool_calls(
                &[
                    ToolCall {
                        id: "a".into(),
                        name: "read".into(),
                        arguments: json!({"path": "a"}),
                    },
                    ToolCall {
                        id: "b".into(),
                        name: "read".into(),
                        arguments: json!({"path": "b"}),
                    },
                ],
                &session,
                &ctx,
                "script",
                "m",
                "k",
                &None,
                &Some(cancel.clone()),
                None,
                &mut Vec::new(),
            )
            .await;
        assert!(parallel.is_err());

        let sequential = a
            .execute_tool_calls(
                &[
                    tc("read", json!({"path": "a"})),
                    tc("bash", json!({"command": "echo hi"})),
                ],
                &session,
                &ctx,
                "script",
                "m",
                "k",
                &None,
                &Some(cancel),
                None,
                &mut Vec::new(),
            )
            .await;
        assert!(sequential.is_err());
    }

    #[tokio::test]
    async fn take_speculative_read_hits_matching_call() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("n.txt"), "payload").unwrap();
        let a = agent();
        let session = Session::new(dir.path().to_path_buf(), "test".into());
        let ctx = a.tool_context(&session);
        let mut jobs = Vec::new();
        crate::speculative_read::maybe_start(&mut jobs, "t1", "read", r#"{"path":"n.txt"}"#, &ctx);
        assert_eq!(jobs.len(), 1);
        let out = a
            .execute_tool_calls(
                &[tc("read", json!({"path": "n.txt"}))],
                &session,
                &ctx,
                "script",
                "m",
                "k",
                &None,
                &None,
                None,
                &mut jobs,
            )
            .await
            .expect("read");
        assert_eq!(out.len(), 1);
        assert!(out[0].content.contains("payload"), "{:?}", out[0]);
        assert!(jobs.is_empty());
    }

    #[tokio::test]
    async fn important_prompts_high_risk_schedule() {
        let asks = Arc::new(CountingDenyPrompter {
            asks: AtomicUsize::new(0),
        });
        let mut info = info("build");
        info.permission
            .rules
            .insert("schedule".into(), PermissionAction::Ask);
        let mut a = Agent::new(info).with_permission_prompter(asks.clone());
        a.set_approval_mode(ApprovalMode::Important);
        let (session, ctx) = session_ctx(&a);
        let result = a
            .execute_with_permission(
                &tc(
                    "schedule",
                    json!({"command": "rm -rf /tmp/scratch", "after_secs": 0}),
                ),
                &session,
                &ctx,
                "script",
                "m",
                "k",
                None,
                None,
            )
            .await;
        assert_eq!(asks.asks.load(Ordering::SeqCst), 1);
        assert!(
            result.content.contains("User denied permission"),
            "{}",
            result.content
        );
    }

    #[tokio::test]
    async fn parallel_speculative_and_sequential_speculative() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "alpha").unwrap();
        std::fs::write(dir.path().join("b.txt"), "beta").unwrap();
        let a = agent();
        let session = Session::new(dir.path().to_path_buf(), "test".into());
        let ctx = a.tool_context(&session);
        let mut jobs = Vec::new();
        crate::speculative_read::maybe_start(&mut jobs, "a", "read", r#"{"path":"a.txt"}"#, &ctx);
        crate::speculative_read::maybe_start(&mut jobs, "b", "read", r#"{"path":"b.txt"}"#, &ctx);
        let parallel = a
            .execute_tool_calls(
                &[
                    ToolCall {
                        id: "a".into(),
                        name: "read".into(),
                        arguments: json!({"path": "a.txt"}),
                    },
                    ToolCall {
                        id: "b".into(),
                        name: "read".into(),
                        arguments: json!({"path": "b.txt"}),
                    },
                ],
                &session,
                &ctx,
                "script",
                "m",
                "k",
                &None,
                &None,
                None,
                &mut jobs,
            )
            .await
            .expect("parallel");
        assert_eq!(parallel.len(), 2);
        assert!(parallel[0].content.contains("alpha"), "{:?}", parallel[0]);
        assert!(parallel[1].content.contains("beta"), "{:?}", parallel[1]);

        let mut jobs = Vec::new();
        crate::speculative_read::maybe_start(&mut jobs, "t1", "read", r#"{"path":"a.txt"}"#, &ctx);
        let mixed = a
            .execute_tool_calls(
                &[
                    tc("read", json!({"path": "a.txt"})),
                    tc("bash", json!({"command": "echo hi"})),
                ],
                &session,
                &ctx,
                "script",
                "m",
                "k",
                &None,
                &None,
                None,
                &mut jobs,
            )
            .await
            .expect("mixed");
        assert_eq!(mixed.len(), 2);
        assert!(mixed[0].content.contains("alpha"), "{:?}", mixed[0]);
    }

    struct HangRead;

    impl whycodes_core::Tool for HangRead {
        fn name(&self) -> &str {
            "read"
        }
        fn description(&self) -> &str {
            "hang"
        }
        fn parameters(&self) -> serde_json::Value {
            json!({"type": "object"})
        }
        fn execute<'a>(
            &'a self,
            _args: serde_json::Value,
            _ctx: &'a whycodes_core::ToolContext,
        ) -> whycodes_core::ToolFuture<'a> {
            Box::pin(async move {
                // Longer than the cancel-during-execute wait (30ms) so cancel
                // wins; short enough that hang_read_execute can await it.
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                ToolResult {
                    tool_call_id: String::new(),
                    content: "never".into(),
                    is_error: false,
                }
            })
        }
    }

    #[tokio::test]
    async fn cancel_during_single_and_sequential_execute() {
        let mut exec = whycodes_tools::executor::ToolExecutor::new();
        exec.register(Box::new(HangRead));
        let a = Agent::new(info("build")).with_tool_executor(exec);
        let (session, ctx) = session_ctx(&a);
        let cancel = crate::events::new_cancel_flag();
        let handle = {
            let cancel = cancel.clone();
            let session = session.clone();
            let ctx = ctx.clone();
            tokio::spawn(async move {
                a.execute_tool_calls(
                    &[tc("read", json!({"path": "x"}))],
                    &session,
                    &ctx,
                    "script",
                    "m",
                    "k",
                    &None,
                    &Some(cancel),
                    None,
                    &mut Vec::new(),
                )
                .await
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        crate::events::request_cancel(&cancel);
        let err = handle.await.expect("join");
        assert!(err.is_err(), "{err:?}");

        let mut exec = whycodes_tools::executor::ToolExecutor::new();
        exec.register(Box::new(HangRead));
        let a = Agent::new(info("build")).with_tool_executor(exec);
        let (session, ctx) = session_ctx(&a);
        let cancel = crate::events::new_cancel_flag();
        let handle = {
            let cancel = cancel.clone();
            let session = session.clone();
            let ctx = ctx.clone();
            tokio::spawn(async move {
                a.execute_tool_calls(
                    &[
                        tc("read", json!({"path": "x"})),
                        tc("bash", json!({"command": "echo hi"})),
                    ],
                    &session,
                    &ctx,
                    "script",
                    "m",
                    "k",
                    &None,
                    &Some(cancel),
                    None,
                    &mut Vec::new(),
                )
                .await
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        crate::events::request_cancel(&cancel);
        let err = handle.await.expect("join");
        assert!(err.is_err(), "{err:?}");
    }

    struct CountingAllowPrompter {
        asks: AtomicUsize,
    }

    impl PermissionPrompter for CountingAllowPrompter {
        fn ask<'a>(
            &'a self,
            _tool_name: &'a str,
            _detail: &'a str,
        ) -> crate::permission::PermissionAskFuture<'a> {
            Box::pin(async move {
                self.asks.fetch_add(1, Ordering::SeqCst);
                true
            })
        }
    }

    #[tokio::test]
    async fn confirm_then_ask_rule_skips_second_prompt() {
        let asks = Arc::new(CountingAllowPrompter {
            asks: AtomicUsize::new(0),
        });
        let mut info = info("build");
        info.permission
            .rules
            .insert("bash(rm *)".into(), PermissionAction::Ask);
        let mut a = Agent::new(info).with_permission_prompter(asks.clone());
        a.set_approval_mode(ApprovalMode::Manual);
        let (session, ctx) = session_ctx(&a);
        let result = a
            .execute_with_permission(
                &tc("bash", json!({"command": "rm -rf /tmp/scratch-cov"})),
                &session,
                &ctx,
                "script",
                "m",
                "k",
                None,
                None,
            )
            .await;
        assert_eq!(asks.asks.load(Ordering::SeqCst), 1, "{result:?}");
        assert!(!result.content.contains("User denied"), "{result:?}");
    }

    #[test]
    fn dummy_tool_trait_methods_are_callable() {
        let flaky = FlakyRead {
            hits: Arc::new(AtomicUsize::new(0)),
        };
        assert_eq!(flaky.description(), "flaky");
        assert_eq!(flaky.parameters()["type"], "object");
        let hang = HangRead;
        assert_eq!(hang.description(), "hang");
        assert_eq!(hang.parameters()["type"], "object");
    }

    #[tokio::test]
    async fn hang_read_execute_returns_after_short_sleep() {
        // Cover the post-sleep ToolResult in HangRead without a 30s wait.
        struct QuickHang;
        impl whycodes_core::Tool for QuickHang {
            fn name(&self) -> &str {
                "read"
            }
            fn description(&self) -> &str {
                "hang"
            }
            fn parameters(&self) -> serde_json::Value {
                json!({"type": "object"})
            }
            fn execute<'a>(
                &'a self,
                _args: serde_json::Value,
                _ctx: &'a whycodes_core::ToolContext,
            ) -> whycodes_core::ToolFuture<'a> {
                Box::pin(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                    ToolResult {
                        tool_call_id: String::new(),
                        content: "never".into(),
                        is_error: false,
                    }
                })
            }
        }
        let tool = QuickHang;
        let ctx = whycodes_core::ToolContext::new("/tmp");
        let out = tool.execute(json!({}), &ctx).await;
        assert_eq!(out.content, "never");
        let hang = HangRead;
        let _ = hang.description();
        let _ = hang.parameters();
        let hang_out = hang.execute(json!({}), &ctx).await;
        assert_eq!(hang_out.content, "never");
    }

    #[tokio::test]
    async fn intent_confirm_skipped_after_risk_confirmed() {
        let asks = Arc::new(CountingAllowPrompter {
            asks: AtomicUsize::new(0),
        });
        let mut a = Agent::new(info("build")).with_permission_prompter(asks.clone());
        a.set_approval_mode(ApprovalMode::Manual);
        a.intent_guidance = crate::intent::IntentGuidanceMode::Always;
        let (session, ctx) = session_ctx(&a);
        let q = crate::intent::classify_user_intent("how does auth work?");
        let result = a
            .execute_with_permission(
                &tc("bash", json!({"command": "rm -rf /tmp/scratch-intent-cov"})),
                &session,
                &ctx,
                "script",
                "m",
                "k",
                Some(&q),
                None,
            )
            .await;
        // Destructive Confirm already set risk_confirmed; intent Confirm must not ask again.
        assert_eq!(asks.asks.load(Ordering::SeqCst), 1, "{result:?}");
        assert!(!result.content.contains("User denied"), "{result:?}");
    }

    #[tokio::test]
    async fn path_ask_skipped_after_intent_confirm() {
        let asks = Arc::new(CountingAllowPrompter {
            asks: AtomicUsize::new(0),
        });
        let mut info = info("build");
        info.permission
            .rules
            .insert("write(docs/**)".into(), PermissionAction::Ask);
        let mut a = Agent::new(info).with_permission_prompter(asks.clone());
        a.set_approval_mode(ApprovalMode::Manual);
        a.intent_guidance = crate::intent::IntentGuidanceMode::Always;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("docs")).unwrap();
        let session = Session::new(dir.path().to_path_buf(), "test".into());
        let ctx = a.tool_context(&session);
        let q = crate::intent::classify_user_intent("how does auth work?");
        let result = a
            .execute_with_permission(
                &tc("write", json!({"path": "docs/a.md", "content": "ok"})),
                &session,
                &ctx,
                "script",
                "m",
                "k",
                Some(&q),
                None,
            )
            .await;
        assert_eq!(asks.asks.load(Ordering::SeqCst), 1, "{result:?}");
        assert!(!result.content.contains("User denied"), "{result:?}");
    }

    #[tokio::test]
    async fn tool_ask_skipped_after_path_allow() {
        let asks = Arc::new(CountingAllowPrompter {
            asks: AtomicUsize::new(0),
        });
        let mut info = info("build");
        info.permission
            .rules
            .insert("write(docs/**)".into(), PermissionAction::Allow);
        info.permission
            .rules
            .insert("write".into(), PermissionAction::Ask);
        let mut a = Agent::new(info).with_permission_prompter(asks.clone());
        a.set_approval_mode(ApprovalMode::Manual);
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("docs")).unwrap();
        let session = Session::new(dir.path().to_path_buf(), "test".into());
        let ctx = a.tool_context(&session);
        let result = a
            .execute_with_permission(
                &tc("write", json!({"path": "docs/a.md", "content": "ok"})),
                &session,
                &ctx,
                "script",
                "m",
                "k",
                None,
                None,
            )
            .await;
        assert_eq!(asks.asks.load(Ordering::SeqCst), 0, "{result:?}");
        assert!(!result.content.contains("User denied"), "{result:?}");
    }
}
