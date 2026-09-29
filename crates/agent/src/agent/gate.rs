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
                || c.contains("denied:headless")
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
            let result = match self.take_speculative_read(tc, tool_ctx, speculative).await {
                Some(early) => early,
                None => {
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
                        match pre {
                            Some(r) => r,
                            None => {
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
                        }
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
            let result = match self.take_speculative_read(tc, tool_ctx, speculative).await {
                Some(early) => early,
                None => {
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
            "schedule" => schedule_command_is_high_risk(tc, working_dir, self.risk_threshold),
            _ => file_path_outside(tc, working_dir),
        }
    }

    /// Overlay: `auto` skips every ask; `important` skips low-risk asks;
    /// `manual` never skips. Deny / catastrophic still refuse above this.
    ///
    /// Headless JSON / stream-json sets `fail_closed_ask` so `auto` cannot
    /// silently run a tool the TUI would have asked about.
    fn approval_skips_ask(&self, tc: &ToolCall, working_dir: &str) -> bool {
        if self.fail_closed_ask {
            return false;
        }
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

    fn denied_permission_result(&self, tc: &ToolCall, extra: &str) -> ToolResult {
        let stamp = if self.fail_closed_ask {
            "denied:headless"
        } else {
            "User denied permission"
        };
        let content = if extra.is_empty() {
            format!("{stamp} for tool '{}'.", tc.name)
        } else {
            format!("{stamp} for tool '{}' {extra}", tc.name)
        };
        ToolResult {
            tool_call_id: tc.id.clone(),
            content,
            is_error: true,
        }
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
        } else if (tc.name == "bash" || tc.name == "shell") && self.bash_auto_background {
            self.execute_shell_with_auto_background(tc, tool_ctx, events)
                .await
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
        let scheduled_shell = match tc.name.as_str() {
            "schedule" => nonempty_arg(tc, "command"),
            _ => None,
        };
        if is_shell_tool(&tc.name) || scheduled_shell.is_some() {
            let command = match scheduled_shell {
                Some(command) => command,
                None => tc
                    .arguments
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or(""),
            };
            let assessment = assess(command, std::path::Path::new(&tool_ctx.working_dir));

            match decide(&assessment, self.risk_threshold) {
                Decision::Allow => skip_retry_status(),
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
                        return self.denied_permission_result(tc, "");
                    }
                    risk_confirmed = true;
                }
            }

            // Shell-scoped permission rules (Claude Code `Bash(git *)` spirit).
            match self.info.permission.action_for_shell(command) {
                Some(shell_act) => match shell_act {
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
                    PermissionAction::Ask => {
                        if apply_shell_ask(risk_confirmed) {
                            let detail =
                                format!("Shell rule requires confirmation\n\nCommand:\n{command}");
                            if !self
                                .ask_permission(tc, &tool_ctx.working_dir, &detail)
                                .await
                            {
                                return self.denied_permission_result(tc, "");
                            }
                            risk_confirmed = true;
                        }
                    }
                },
                None => skip_shell_ask(),
            }
        }

        // Intent authorization (Claude-style): question/plan/ambiguous-always
        // turns must not silently mutate. After blast-radius, before permission.
        let intent_body = match turn_intent {
            Some(intent) => intent_confirm_body(
                intent,
                &self.info.name,
                &tc.name,
                tc.arguments.get("command").and_then(|v| v.as_str()),
                self.intent_guidance,
                &tc.arguments,
            ),
            None => IntentGate::Skip,
        };
        match intent_body {
            IntentGate::Refuse(reason) => {
                return ToolResult {
                    tool_call_id: tc.id.clone(),
                    content: format!("Refused (intent): {reason}"),
                    is_error: true,
                };
            }
            IntentGate::Confirm(body) => {
                if !risk_confirmed && !self.ask_permission(tc, &tool_ctx.working_dir, &body).await {
                    return self.denied_permission_result(tc, "(intent gate).");
                }
                if !risk_confirmed {
                    risk_confirmed = true;
                }
            }
            IntentGate::Skip => skip_retry_status(),
        }

        // Path-scoped rules: `edit(src/**)`, `write(docs/**)`, …
        let path_rule = match file_tool_path(tc) {
            Some(path) => path_rule_gate(&self.info.permission, &tc.name, &path, risk_confirmed),
            None => PathRuleGate::Skip,
        };
        match path_rule {
            PathRuleGate::Deny { path } => {
                return ToolResult {
                    tool_call_id: tc.id.clone(),
                    content: format!(
                        "Permission denied for `{}` on path `{path}` by path rule.",
                        tc.name
                    ),
                    is_error: true,
                };
            }
            PathRuleGate::Allow => {
                risk_confirmed = true;
            }
            PathRuleGate::Ask { path } => {
                let detail = format!(
                    "Path rule requires confirmation\n\nTool: {}\nPath: {path}",
                    tc.name
                );
                if !self
                    .ask_permission(tc, &tool_ctx.working_dir, &detail)
                    .await
                {
                    return self.denied_permission_result(tc, "");
                }
                risk_confirmed = true;
            }
            PathRuleGate::Skip => skip_retry_status(),
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
            PermissionAction::Ask => {
                if risk_confirmed {
                    continue_confirmed();
                } else {
                    let detail = format_permission_detail(&tc.arguments);
                    let allowed = self
                        .ask_permission(tc, &tool_ctx.working_dir, &detail)
                        .await;
                    if !allowed {
                        return self.denied_permission_result(tc, "");
                    }
                }
            }
            PermissionAction::Allow => skip_retry_status(),
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
            PreHookDecision::Allow => skip_retry_status(),
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
                match events {
                    Some(sink) => emit(
                        &Some(sink.clone()),
                        TurnEvent::Status(format!(
                            "Auto: retrying `{}` ({attempt}/{AUTO_TOOL_RETRY_LIMIT})…",
                            tc.name
                        )),
                    ),
                    None => skip_retry_status(),
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

fn continue_confirmed() {}

fn skip_retry_status() {}

fn skip_shell_ask() {}

fn apply_shell_ask(risk_confirmed: bool) -> bool {
    !risk_confirmed
}

enum IntentGate {
    Skip,
    Refuse(String),
    Confirm(String),
}

fn intent_confirm_body(
    intent: &crate::intent::IntentAssessment,
    agent_name: &str,
    tool_name: &str,
    command: Option<&str>,
    guidance: crate::intent::IntentGuidanceMode,
    arguments: &serde_json::Value,
) -> IntentGate {
    match crate::intent::authorize_tool(intent, agent_name, tool_name, command, guidance) {
        crate::intent::ToolAuthDecision::Allow => IntentGate::Skip,
        crate::intent::ToolAuthDecision::Refuse { reason } => {
            let intent_s = intent.intent.as_str();
            tracing::info!(tool = %tool_name, intent = intent_s, "intent auth refused tool");
            IntentGate::Refuse(reason)
        }
        crate::intent::ToolAuthDecision::Confirm { reason } => {
            let detail = format_permission_detail(arguments);
            IntentGate::Confirm(format!("{detail}\n\nIntent check:\n{reason}"))
        }
    }
}

enum PathRuleGate {
    Skip,
    Deny { path: String },
    Allow,
    Ask { path: String },
}

fn path_rule_gate(
    permission: &whycodes_core::types::PermissionSet,
    tool_name: &str,
    path: &str,
    risk_confirmed: bool,
) -> PathRuleGate {
    match permission.action_for_path(tool_name, path) {
        Some(PermissionAction::Deny) => PathRuleGate::Deny {
            path: path.to_string(),
        },
        Some(PermissionAction::Allow) => PathRuleGate::Allow,
        Some(PermissionAction::Ask) => {
            if risk_confirmed {
                PathRuleGate::Skip
            } else {
                PathRuleGate::Ask {
                    path: path.to_string(),
                }
            }
        }
        None => PathRuleGate::Skip,
    }
}

fn nonempty_arg<'a>(tc: &'a ToolCall, key: &str) -> Option<&'a str> {
    match tc.arguments.get(key).and_then(|v| v.as_str()) {
        Some(value) => {
            let value = value.trim();
            if value.is_empty() { None } else { Some(value) }
        }
        None => None,
    }
}

fn schedule_command_is_high_risk(
    tc: &ToolCall,
    working_dir: &str,
    threshold: whycodes_command_risk::RiskThreshold,
) -> bool {
    let command = match nonempty_arg(tc, "command") {
        Some(command) => command,
        None => return false,
    };
    !matches!(
        decide(
            &assess(command, std::path::Path::new(working_dir)),
            threshold
        ),
        Decision::Allow
    )
}

fn file_path_outside(tc: &ToolCall, working_dir: &str) -> bool {
    match file_tool_path(tc) {
        Some(path) => path_outside_workspace(&path, working_dir),
        None => false,
    }
}

fn is_shell_tool(name: &str) -> bool {
    SHELL_TOOLS.contains(&name)
}

#[cfg(test)]
#[path = "gate_tests.rs"]
mod tests;
