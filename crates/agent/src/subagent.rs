use std::sync::Arc;
use std::time::Instant;

use whycodes_command_risk::{Decision, RiskThreshold, assess, decide};
use whycodes_config::{HookConfig, HookEvent};
use whycodes_core::SandboxSettings;
use whycodes_core::network::NetworkPolicy;
use whycodes_core::tool::ToolContext;
use whycodes_core::types::{AgentInfo, ApprovalMode, PermissionAction, PermissionSet};
use whycodes_llm::provider::ProviderRegistry;
use whycodes_memory::MemorySettings;
use whycodes_plugin::hooks::{
    HookContext, PreHookDecision, append_feedback, run_hooks, run_post_hooks, run_pre_hooks,
    run_transform_hooks,
};
use whycodes_session::session::Session;
use whycodes_tools::executor::ToolExecutor;
use whycodes_tools::question::parse_questions;

use crate::question::{
    QuestionPrompter, default_question_prompter, run_question_tool, should_prompt_questions,
};
use crate::structured_output::{self, Structured};

use super::agent::DEFAULT_SYSTEM_PROMPT;
use crate::tool_policy::{SHELL_TOOLS, file_tool_path};

/// Parameters for spawning a subagent
#[derive(Debug, Clone)]
pub struct SubagentTask {
    /// The goal/task description for the subagent
    pub goal: String,
    /// Additional context for the subagent
    pub context: Option<String>,
    /// Specific tool names to grant the subagent (None = all tools)
    pub tools: Option<Vec<String>>,
    /// Maximum conversation turns for the subagent
    pub max_turns: usize,
    /// Prepared result schema ([`crate::structured_output::prepare`]); the
    /// worker must end with a matching fenced `json` block.
    pub output_schema: Option<serde_json::Value>,
}

/// Result from a subagent run
#[derive(Debug, Clone)]
pub struct SubagentResult {
    /// The original goal that was given to the subagent
    pub goal: String,
    /// The textual output from the subagent
    pub output: String,
    /// Whether the subagent completed successfully
    pub success: bool,
    /// Wall-clock duration of the subagent run
    pub duration: std::time::Duration,
    /// Provider-reported token usage across all subagent LLM steps (fold into parent).
    pub usage: whycodes_core::types::Usage,
    /// The user cancelled the worker (agent hub); `output` says so.
    pub cancelled: bool,
    /// Structured result when the task had an `output_schema`.
    pub structured: Option<Structured>,
}

/// Result text of a worker the user cancelled.
pub const CANCELLED_BY_USER: &str = "cancelled by the user";

/// Longest worker log carried on a progress event; older lines drop first.
const MAX_WORKER_LOG: usize = 16_000;

/// Where a worker reports its steps (parent TUI agent hub).
struct WorkerProgress {
    events: crate::events::EventSink,
    id: String,
    kind: String,
    description: String,
}

/// What the worker loop produced.
struct LoopOutput {
    text: String,
    usage: whycodes_core::types::Usage,
    cancelled: bool,
}

/// Runner that executes a subagent task by creating a fresh session and
/// delegating to `Agent::run_turn` with the goal as the initial user message.
pub struct SubagentRunner {
    provider_registry: Arc<ProviderRegistry>,
    tool_executor: Arc<ToolExecutor>,
    info: AgentInfo,
    project_path: std::path::PathBuf,
    sandbox: SandboxSettings,
    network: NetworkPolicy,
    memory: MemorySettings,
    /// Project whose memory bank to use when it differs from `project_path`
    /// (swarm worktrees run in a throwaway checkout of the main project).
    memory_project: Option<std::path::PathBuf>,
    /// Shared file-claim registry when running inside a swarm.
    file_claims: Option<whycodes_core::FileClaimRegistry>,
    agent_id: Option<String>,
    agent_label: Option<String>,
    /// Workspace file index inherited from the parent agent (tools fast path).
    file_index: Option<Arc<whycodes_index::WorkspaceIndex>>,
    /// Parent TUI panel sink (so workers can pin a preview).
    panel: Option<whycodes_core::PanelSink>,
    swarm_hub: Option<whycodes_core::SwarmHub>,
    /// The parent's steering inbox and this subagent's key in it (`/tell`).
    steer: Option<(crate::steer::SteerInbox, String)>,
    /// Cancel registry and this worker's key in it (agent hub `k`).
    cancel: Option<(crate::worker_cancel::WorkerCancels, String)>,
    /// Step reports for the parent UI.
    progress: Option<WorkerProgress>,
    question_prompter: Arc<dyn QuestionPrompter>,
    approval_mode: ApprovalMode,
    /// Parent config hooks: tool hooks run here too, plus `subagent_stop`.
    hooks: Vec<HookConfig>,
    /// Parent `[security] bash_risk_threshold`; workers cannot prompt, so a
    /// command the parent would confirm is allowed only in `auto` mode.
    risk_threshold: RiskThreshold,
    /// Redacts outgoing requests and restores placeholders in tool input.
    secrets: Option<Arc<whycodes_llm::Redactor>>,
}

impl SubagentRunner {
    /// Create a new runner from the agent's shared state.
    pub fn new(
        provider_registry: Arc<ProviderRegistry>,
        tool_executor: Arc<ToolExecutor>,
        info: AgentInfo,
        project_path: std::path::PathBuf,
        sandbox: SandboxSettings,
        network: NetworkPolicy,
    ) -> Self {
        Self {
            provider_registry,
            tool_executor,
            info,
            project_path,
            sandbox,
            network,
            memory: default_memory(),
            memory_project: None,
            file_claims: None,
            agent_id: None,
            agent_label: None,
            file_index: None,
            panel: None,
            swarm_hub: None,
            steer: None,
            cancel: None,
            progress: None,
            question_prompter: default_question_prompter(),
            approval_mode: ApprovalMode::Auto,
            hooks: Vec::new(),
            risk_threshold: RiskThreshold::default(),
            secrets: whycodes_llm::secrets::installed(),
        }
    }

    /// Use `redactor` instead of the process default.
    pub fn with_redactor(mut self, redactor: Option<Arc<whycodes_llm::Redactor>>) -> Self {
        self.secrets = redactor;
        self
    }

    /// Inherit the parent's shell risk threshold.
    pub fn with_risk_threshold(mut self, threshold: RiskThreshold) -> Self {
        self.risk_threshold = threshold;
        self
    }

    /// Inherit the parent's config hooks.
    pub fn with_hooks(mut self, hooks: Vec<HookConfig>) -> Self {
        self.hooks = hooks;
        self
    }

    pub fn with_question_prompter(mut self, prompter: Arc<dyn QuestionPrompter>) -> Self {
        self.question_prompter = prompter;
        self
    }

    pub fn with_approval_mode(mut self, mode: ApprovalMode) -> Self {
        self.approval_mode = mode;
        self
    }

    /// Inherit the parent agent's workspace file index.
    pub fn with_file_index(mut self, index: Option<Arc<whycodes_index::WorkspaceIndex>>) -> Self {
        self.file_index = index;
        self
    }

    /// Inherit the parent agent's side-panel sink.
    pub fn with_panel(mut self, panel: Option<whycodes_core::PanelSink>) -> Self {
        self.panel = panel;
        self
    }

    /// Read messages the user addresses to `key` (`/tell <key> …`) at each step.
    pub fn with_steer(mut self, inbox: crate::steer::SteerInbox, key: impl Into<String>) -> Self {
        self.steer = Some((inbox, key.into()));
        self
    }

    /// Stop at the next step once the UI cancels `key` in `cancels`.
    pub fn with_cancel(
        mut self,
        cancels: crate::worker_cancel::WorkerCancels,
        key: impl Into<String>,
    ) -> Self {
        self.cancel = Some((cancels, key.into()));
        self
    }

    /// Send a `running` [`crate::events::TurnEvent::Subagent`] with the log so
    /// far after each step, so the parent UI can show live activity.
    pub fn with_progress(
        mut self,
        events: Option<crate::events::EventSink>,
        id: impl Into<String>,
        kind: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        let (id, kind, description) = (id.into(), kind.into(), description.into());
        self.progress = events.map(|events| WorkerProgress {
            events,
            id,
            kind,
            description,
        });
        self
    }

    pub fn with_swarm_hub(mut self, hub: Option<whycodes_core::SwarmHub>) -> Self {
        self.swarm_hub = hub;
        self
    }

    /// Attach parent memory settings (subagent_banks, inject knobs).
    pub fn with_memory(mut self, memory: MemorySettings) -> Self {
        self.memory = memory;
        self
    }

    /// Key memory by `project` instead of the working directory.
    pub fn with_memory_project(mut self, project: std::path::PathBuf) -> Self {
        self.memory_project = Some(project);
        self
    }

    fn memory_project_path(&self) -> &std::path::Path {
        self.memory_project.as_deref().unwrap_or(&self.project_path)
    }

    /// Name this runner (`worker-0`, …) for swarm mail and conflict labels,
    /// whether or not file claims are on.
    pub fn with_identity(
        mut self,
        agent_id: impl Into<String>,
        agent_label: impl Into<String>,
    ) -> Self {
        self.agent_id = Some(agent_id.into());
        self.agent_label = Some(agent_label.into());
        self
    }

    /// Bind this runner to a swarm file-claim registry and worker identity.
    pub fn with_file_claims(
        mut self,
        claims: whycodes_core::FileClaimRegistry,
        agent_id: impl Into<String>,
        agent_label: impl Into<String>,
    ) -> Self {
        self.agent_id = Some(agent_id.into());
        self.agent_label = Some(agent_label.into());
        self.file_claims = Some(claims);
        self
    }

    /// Run a single subagent task synchronously (awaited).
    ///
    /// Inner turn errors become `success: false` rather than a `Result::Err`, so
    /// callers always get a `SubagentResult`.
    pub async fn run(
        &self,
        task: SubagentTask,
        provider_name: &str,
        model: &str,
        api_key: &str,
    ) -> SubagentResult {
        let start = Instant::now();

        // Build the full prompt from goal + optional context
        let user_message = if let Some(ctx) = &task.context {
            format!(
                "GOAL: {}\n\nCONTEXT:\n{}\n\nPlease accomplish the goal above.",
                task.goal, ctx
            )
        } else {
            format!("GOAL: {}\n\nPlease accomplish the goal above.", task.goal)
        };

        // Resolve tools: if `task.tools` is Some, build a PermissionSet with allowed_tools
        let mut permission = self.info.permission.clone();
        if let Some(ref tool_names) = task.tools {
            permission.allowed_tools = Some(tool_names.clone());
        }

        // Create a fresh session for this subagent. Inject agent-scoped memory
        // bank (Claude subagent memory parity) when configured.
        let mut system_prompt = self
            .info
            .system_prompt
            .clone()
            .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string());
        system_prompt = inject_subagent_memory(
            &system_prompt,
            self.memory_project_path(),
            &whycodes_core::paths::data_dir(),
            &self.info.name,
            &user_message,
            &self.memory,
        );
        if let Some(schema) = &task.output_schema {
            system_prompt.push_str(&structured_output::instruction(schema));
        }

        let mut session = Session::new(self.project_path.clone(), system_prompt);
        session.add_user_message(&user_message);

        if let Some((cancels, key)) = &self.cancel {
            cancels.register(key);
        }
        // Execute the conversation loop — reuse the same run_turn logic as Agent
        let output = self
            .run_turn_inner(
                &mut session,
                provider_name,
                model,
                api_key,
                task.max_turns,
                &permission,
            )
            .await;
        if let Some((cancels, key)) = &self.cancel {
            cancels.finish(key);
        }

        let mut structured = None;
        let output = match (output, &task.output_schema) {
            (Ok(out), Some(schema)) if !out.cancelled => {
                let route = (provider_name, model, api_key);
                let (text, usage, s) = self
                    .structured_result(
                        &mut session,
                        schema,
                        out.text,
                        out.usage,
                        route,
                        &permission,
                    )
                    .await;
                structured = Some(s);
                Ok(LoopOutput {
                    text,
                    usage,
                    cancelled: false,
                })
            }
            (other, _) => other,
        };

        let duration = start.elapsed();

        let result = match output {
            Ok(out) => SubagentResult {
                goal: task.goal,
                output: if out.cancelled {
                    cancelled_output(&out.text)
                } else {
                    out.text
                },
                success: !out.cancelled,
                duration,
                usage: out.usage,
                cancelled: out.cancelled,
                structured,
            },
            Err(e) => SubagentResult {
                goal: task.goal,
                output: format!("Subagent error: {}", e),
                success: false,
                duration,
                usage: whycodes_core::types::Usage::default(),
                cancelled: false,
                structured: None,
            },
        };
        self.run_stop_hooks(&session, &result).await;
        result
    }

    /// Check the worker's final `json` block against `schema`. An invalid or
    /// missing block gets one retry step with the errors; still invalid gives
    /// `value: None` and those errors. Never fails the run.
    async fn structured_result(
        &self,
        session: &mut Session,
        schema: &serde_json::Value,
        text: String,
        mut usage: whycodes_core::types::Usage,
        (provider_name, model, api_key): (&str, &str, &str),
        permission: &PermissionSet,
    ) -> (String, whycodes_core::types::Usage, Structured) {
        let errors = match structured_output::check(schema, &text) {
            Ok(value) => {
                let s = Structured {
                    value: Some(value),
                    errors: Vec::new(),
                };
                return (structured_output::strip_block(&text), usage, s);
            }
            Err(errors) => errors,
        };
        session.add_user_message(&structured_output::retry_message(&errors));
        let retry = self
            .run_turn_inner(session, provider_name, model, api_key, 1, permission)
            .await;
        let (retry, retry_usage) = match retry {
            Ok(r) => (r.text, r.usage),
            Err(e) => {
                let mut errors = errors;
                errors.push(format!("retry failed: {e}"));
                let s = Structured {
                    value: None,
                    errors,
                };
                return (text, usage, s);
            }
        };
        usage.add(&retry_usage);
        match structured_output::check(schema, &retry) {
            Ok(value) => {
                let parts = [
                    structured_output::strip_block(&text),
                    structured_output::strip_block(&retry),
                ];
                let text: Vec<&str> = parts
                    .iter()
                    .map(String::as_str)
                    .filter(|p| !p.is_empty())
                    .collect();
                let s = Structured {
                    value: Some(value),
                    errors: Vec::new(),
                };
                (text.join("\n\n"), usage, s)
            }
            Err(errors) => {
                let s = Structured {
                    value: None,
                    errors,
                };
                (format!("{text}\n\n{retry}"), usage, s)
            }
        }
    }

    /// `subagent_stop` hooks; `match` filters the agent type. Never blocks.
    async fn run_stop_hooks(&self, session: &Session, result: &SubagentResult) {
        if self.hooks.is_empty() {
            return;
        }
        let ctx = HookContext::lifecycle(
            HookEvent::SubagentStop,
            self.info.name.clone(),
            Some(session.id.clone()),
            self.project_path.display().to_string(),
        )
        .with_env("WHYCODES_SUBAGENT_TYPE", &self.info.name)
        .with_env("WHYCODES_SUBAGENT_GOAL", &result.goal)
        .with_env(
            "WHYCODES_SUBAGENT_SUCCESS",
            if result.success { "1" } else { "0" },
        )
        .with_env("WHYCODES_SUBAGENT_OUTPUT", &result.output);
        run_hooks(&self.hooks, &ctx).await;
    }

    /// Internal turn loop — mirrors `Agent::run_turn` but accepts an overridden
    /// PermissionSet so that subagents can have a restricted tool set.
    /// Returns the final text, aggregated provider usage, and whether the
    /// user cancelled it.
    async fn run_turn_inner(
        &self,
        session: &mut Session,
        provider_name: &str,
        model: &str,
        api_key: &str,
        max_turns: usize,
        permission: &PermissionSet,
    ) -> whycodes_core::Result<LoopOutput> {
        use futures::StreamExt;
        use whycodes_core::types::{ContentBlock, StreamEvent};

        use super::tool_stream::ToolCallAssembler;

        // Get tool definitions filtered by the subagent's permission set
        let tools = self.tool_executor.get_definitions(permission);

        let mut sandbox = self.sandbox.clone();
        if !permission.allow_network {
            sandbox.network = false;
        }
        let tool_ctx = ToolContext {
            working_dir: session.project_path.to_string_lossy().to_string(),
            session_id: Some(session.id.clone()),
            sandbox,
            network: self.network.clone(),
            file_claims: self.file_claims.clone(),
            agent_id: self.agent_id.clone(),
            agent_label: self.agent_label.clone(),
            file_index: self.file_index.clone(),
            panel: self.panel.clone(),
            todo_sink: None,
            swarm_hub: self.swarm_hub.clone(),
        };

        let provider = self.provider_registry.get(provider_name).ok_or_else(|| {
            whycodes_core::Error::llm(format!(
                "Unknown provider: {}. Available: anthropic, openai, google, google-antigravity",
                provider_name
            ))
        })?;

        let mut turn_count = 0;
        let mut final_text = String::new();
        let mut total_usage = whycodes_core::types::Usage::default();
        let started = Instant::now();
        let mut log = String::new();
        let mut cancelled = false;

        loop {
            // A cancel takes effect between steps: the last step's tool calls
            // all have their results, and the parent gets a normal result.
            if self.cancel_requested() {
                cancelled = true;
                break;
            }
            turn_count += 1;
            if turn_count > max_turns {
                return Err(whycodes_core::Error::Agent(format!(
                    "Subagent exceeded maximum turns ({})",
                    max_turns
                )));
            }

            if let Some(hub) = &self.swarm_hub {
                let id = self.agent_id.as_deref().unwrap_or("worker");
                let inbox = hub.drain(id);
                if !inbox.is_empty() {
                    let mut note = String::from(
                        "Swarm messages since your last turn (reply with swarm_msg if needed):\n",
                    );
                    for m in inbox {
                        note.push_str(&format!("- from {}: {}\n", m.from, m.text));
                    }
                    session.add_user_message(&note);
                }
            }

            if let Some((inbox, key)) = &self.steer {
                for content in inbox.drain(key) {
                    crate::steer::add_to_session(session, content);
                }
            }

            let request = session.build_request(
                std::sync::Arc::clone(&tools),
                None, // max_tokens
                self.info.temperature,
                Some(true), // thinking
            );

            // Stream response
            let mut accumulated_text = String::new();
            let mut assembler = ToolCallAssembler::new();
            let mut step_usage = whycodes_core::types::Usage::default();

            let mut event_stream = whycodes_llm::default_transport()
                .with_redactor(self.secrets.clone())
                .stream(provider, &request, api_key, model)
                .await?;

            while let Some(event) = event_stream.next().await {
                match event? {
                    StreamEvent::TextDelta { text } => {
                        accumulated_text.push_str(&text);
                    }
                    StreamEvent::ToolUse { id, name, input } => {
                        assembler.on_tool_use(id, name, input);
                    }
                    StreamEvent::ToolUseDelta {
                        id,
                        input_json_delta,
                    } => {
                        assembler.on_tool_use_delta(&id, &input_json_delta);
                    }
                    StreamEvent::Thinking { text } => {
                        tracing::debug!("Subagent thinking: {}", text);
                    }
                    StreamEvent::ThinkingDelta { text } => {
                        tracing::debug!("Subagent thinking: {}", text);
                    }
                    StreamEvent::ThinkingSignature { .. } => {}
                    StreamEvent::RedactedThinking { .. } => {}
                    StreamEvent::MessageStop => break,
                    // Per-step snapshot fold; added into total_usage after
                    // the stream so multi-step workers still sum.
                    StreamEvent::Usage {
                        input_tokens,
                        output_tokens,
                    } => {
                        step_usage.absorb_stream(input_tokens, output_tokens);
                    }
                    StreamEvent::CacheUsage {
                        creation_input_tokens,
                        read_input_tokens,
                    } => {
                        step_usage.absorb_stream_cache(creation_input_tokens, read_input_tokens);
                    }
                    StreamEvent::MessageStart { .. } => {}
                    StreamEvent::MessageDelta { .. } => {}
                    StreamEvent::Error { message } => {
                        return Err(whycodes_core::Error::llm(message));
                    }
                }
            }

            if !step_usage.is_empty() {
                total_usage.add(&step_usage);
            }

            let tool_calls = assembler.finish();

            // Build assistant content blocks
            let mut blocks: Vec<ContentBlock> = Vec::new();

            if !accumulated_text.is_empty() {
                blocks.push(ContentBlock::Text {
                    text: accumulated_text.clone(),
                });
                final_text.push_str(&accumulated_text);
            }

            for tc in &tool_calls {
                blocks.push(ContentBlock::ToolUse {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    input: tc.arguments.clone(),
                });
            }

            // Never persist an empty assistant turn — strict OpenAI-compatible
            // APIs reject assistant messages with no text/tool_calls.
            if !blocks.is_empty() {
                session.add_assistant_message(blocks);
            }

            // If no tool calls, we're done
            if tool_calls.is_empty() {
                break;
            }
            push_step_log(&mut log, &accumulated_text, &tool_calls);
            let activity = tool_calls.last().map(tool_activity).unwrap_or_default();
            self.report(started, &activity, &log, &total_usage);

            // Parallelize independent reads; keep mutators/serial tools sequential.
            let results = if tool_calls.len() > 1
                && tool_calls.iter().all(|tc| {
                    !matches!(
                        tc.name.as_str(),
                        "bash"
                            | "shell"
                            | "write"
                            | "edit"
                            | "apply_patch"
                            | "ast_edit"
                            | "git_commit"
                            | "todo_write"
                            | "todo"
                            | "task"
                            | "swarm"
                            | "plan"
                            | "question"
                            | "code_mode"
                            | "skill"
                            | "external_directory"
                            | "memory"
                            | "browser"
                    )
                }) {
                let futs: Vec<_> = tool_calls
                    .iter()
                    .map(|tc| self.execute_tool_call(tc, &tool_ctx, permission))
                    .collect();
                futures::future::join_all(futs).await
            } else {
                let mut results = Vec::with_capacity(tool_calls.len());
                for tc in &tool_calls {
                    results.push(self.execute_tool_call(tc, &tool_ctx, permission).await);
                }
                results
            };

            push_results_log(&mut log, &results);
            self.report(started, &activity, &log, &total_usage);
            session.add_tool_results(results);
        }

        if let Some((inbox, key)) = &self.steer {
            final_text.push_str(&unread_tell_note(inbox.drain(key)));
        }
        Ok(LoopOutput {
            text: final_text,
            usage: total_usage,
            cancelled,
        })
    }

    fn cancel_requested(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|(cancels, key)| cancels.is_cancelled(key))
    }

    /// One `running` progress event; a closed listener is only logged.
    fn report(
        &self,
        started: Instant,
        activity: &str,
        log: &str,
        usage: &whycodes_core::types::Usage,
    ) {
        let Some(p) = &self.progress else {
            return;
        };
        let event = crate::events::TurnEvent::Subagent {
            id: p.id.clone(),
            kind: p.kind.clone(),
            description: p.description.clone(),
            status: "running".into(),
            activity: activity.to_string(),
            elapsed_ms: started.elapsed().as_millis() as u64,
            output: log.to_string(),
            tokens: usage.input_tokens + usage.output_tokens,
        };
        if let Err(e) = p.events.send(event) {
            tracing::debug!(error = %e, "subagent progress event dropped");
        }
    }

    async fn execute_tool_call(
        &self,
        tc: &whycodes_core::types::ToolCall,
        tool_ctx: &ToolContext,
        permission: &PermissionSet,
    ) -> whycodes_core::types::ToolResult {
        if tc.name == "question" {
            let questions = match parse_questions(&tc.arguments) {
                Ok(q) => q,
                Err(e) => {
                    return whycodes_core::types::ToolResult {
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
        let transformed = self.transform_tool_call(tc, tool_ctx).await;
        let tc = transformed.as_ref().unwrap_or(tc);
        let restored = self.restore_tool_call(tc);
        let tc = restored.as_ref().unwrap_or(tc);
        if let Some(reason) = self.headless_gate(tc, permission, &tool_ctx.working_dir) {
            tracing::warn!(tool = %tc.name, reason, "subagent tool call refused");
            return whycodes_core::types::ToolResult {
                tool_call_id: tc.id.clone(),
                content: reason,
                is_error: true,
            };
        }
        if self.hooks.is_empty() {
            return self.tool_executor.execute(tc, tool_ctx, permission).await;
        }
        let tool_input = tc.arguments.to_string();
        let pre_ctx = HookContext::pre(
            tc.name.clone(),
            tc.id.clone(),
            tool_input.clone(),
            tool_ctx.session_id.clone(),
            tool_ctx.working_dir.clone(),
        );
        if let PreHookDecision::Block { reason } = run_pre_hooks(&self.hooks, &pre_ctx).await {
            return whycodes_core::types::ToolResult {
                tool_call_id: tc.id.clone(),
                content: reason,
                is_error: true,
            };
        }
        let mut result = self.tool_executor.execute(tc, tool_ctx, permission).await;
        let post_ctx = HookContext::post(
            tc.name.clone(),
            tc.id.clone(),
            tool_input,
            tool_ctx.session_id.clone(),
            tool_ctx.working_dir.clone(),
            result.is_error,
            &result.content,
        );
        let feedback = run_post_hooks(&self.hooks, &post_ctx).await;
        append_feedback(&mut result.content, feedback);
        result
    }

    /// Secret placeholders become the real values before the headless gate,
    /// as in the parent.
    fn restore_tool_call(
        &self,
        tc: &whycodes_core::types::ToolCall,
    ) -> Option<whycodes_core::types::ToolCall> {
        let redactor = self.secrets.as_ref()?;
        let mut arguments = tc.arguments.clone();
        if !redactor.restore_json(&mut arguments) {
            return None;
        }
        Some(whycodes_core::types::ToolCall {
            id: tc.id.clone(),
            name: tc.name.clone(),
            arguments,
        })
    }

    /// `pre_tool_transform` hooks rewrite the arguments before the headless
    /// gate, as in the parent.
    async fn transform_tool_call(
        &self,
        tc: &whycodes_core::types::ToolCall,
        tool_ctx: &ToolContext,
    ) -> Option<whycodes_core::types::ToolCall> {
        if self.hooks.is_empty() {
            return None;
        }
        let ctx = HookContext::pre(
            tc.name.clone(),
            tc.id.clone(),
            tc.arguments.to_string(),
            tool_ctx.session_id.clone(),
            tool_ctx.working_dir.clone(),
        );
        let arguments = run_transform_hooks(&self.hooks, &ctx).await?;
        tracing::info!(tool = %tc.name, "pre_tool_transform rewrote a worker tool input");
        Some(whycodes_core::types::ToolCall {
            id: tc.id.clone(),
            name: tc.name.clone(),
            arguments,
        })
    }

    /// The parent's blast-radius gates, without a prompter: catastrophic shell
    /// commands are refused, shell and path `deny` rules apply, and anything the
    /// parent would have asked about (risk `Confirm`, shell/path `ask`) passes
    /// only in `auto` approval mode. Tool-name `ask` and the intent gate stay
    /// with the parent. Returns the refusal text.
    fn headless_gate(
        &self,
        tc: &whycodes_core::types::ToolCall,
        permission: &PermissionSet,
        working_dir: &str,
    ) -> Option<String> {
        let scheduled = match tc.name.as_str() {
            "schedule" => tc
                .arguments
                .get("command")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|c| !c.is_empty()),
            _ => None,
        };
        if SHELL_TOOLS.contains(&tc.name.as_str()) || scheduled.is_some() {
            let command = match scheduled {
                Some(command) => command,
                None => tc
                    .arguments
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or(""),
            };
            let assessment = assess(command, std::path::Path::new(working_dir));
            match decide(&assessment, self.risk_threshold) {
                Decision::Allow => {}
                Decision::Refuse { reason } => {
                    return Some(format!(
                        "Refused: {reason}.
                         This command is classified catastrophic and cannot be approved,                          not even by the parent agent."
                    ));
                }
                Decision::Confirm { reason } => {
                    if let Some(refusal) =
                        self.needs_confirmation(&format!("shell risk ({reason})"))
                    {
                        return Some(refusal);
                    }
                }
            }
            match permission.action_for_shell(command) {
                Some(PermissionAction::Deny) => {
                    return Some(format!(
                        "Permission denied for shell command by rule matching `{command}`."
                    ));
                }
                Some(PermissionAction::Ask) => {
                    if let Some(refusal) =
                        self.needs_confirmation("a shell rule requires confirmation")
                    {
                        return Some(refusal);
                    }
                }
                Some(PermissionAction::Allow) | None => {}
            }
        }
        if let Some(path) = file_tool_path(tc) {
            match permission.action_for_path(&tc.name, &path) {
                Some(PermissionAction::Deny) => {
                    return Some(format!(
                        "Permission denied for `{}` on `{path}` by path rule.",
                        tc.name
                    ));
                }
                Some(PermissionAction::Ask) => {
                    if let Some(refusal) = self.needs_confirmation(&format!(
                        "a path rule on `{path}` requires confirmation"
                    )) {
                        return Some(refusal);
                    }
                }
                Some(PermissionAction::Allow) | None => {}
            }
        }
        None
    }

    /// `Some(refusal)` when the parent would prompt and this worker cannot;
    /// `None` when `auto` mode would have allowed it.
    fn needs_confirmation(&self, what: &str) -> Option<String> {
        match self.approval_mode {
            ApprovalMode::Auto => None,
            mode => Some(format!(
                "Denied: {what}. A subagent cannot prompt for confirmation                  (approval mode `{}`); run it from the parent session instead.",
                mode.as_str()
            )),
        }
    }
}

/// Memory settings before config is applied. Unit tests start with memory off:
/// banks live under the real `~/.whycodes`, and every tempdir project would
/// leave one behind.
#[cfg(test)]
pub(crate) fn default_memory() -> MemorySettings {
    MemorySettings::disabled()
}

#[cfg(not(test))]
pub(crate) fn default_memory() -> MemorySettings {
    MemorySettings::default()
}

/// Result text for a cancelled worker, keeping what it wrote before.
fn cancelled_output(partial: &str) -> String {
    let partial = partial.trim();
    if partial.is_empty() {
        return CANCELLED_BY_USER.to_string();
    }
    format!("{CANCELLED_BY_USER}\n\nOutput before the cancel:\n{partial}")
}

/// Short label for a tool call: its name plus the main argument.
fn tool_activity(tc: &whycodes_core::types::ToolCall) -> String {
    let keys = [
        "path",
        "file_path",
        "command",
        "pattern",
        "query",
        "url",
        "goal",
    ];
    let arg = keys
        .iter()
        .find_map(|k| tc.arguments.get(*k).and_then(|v| v.as_str()))
        .unwrap_or("");
    let arg: String = arg
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .chars()
        .take(60)
        .collect();
    if arg.is_empty() {
        tc.name.clone()
    } else {
        format!("{} {arg}", tc.name)
    }
}

/// Append a step's text and tool calls to the worker log.
fn push_step_log(log: &mut String, text: &str, calls: &[whycodes_core::types::ToolCall]) {
    let text = text.trim();
    if !text.is_empty() {
        log.push_str(text);
        log.push('\n');
    }
    for tc in calls {
        log.push_str(&format!("→ {}\n", tool_activity(tc)));
    }
    cap_log(log);
}

/// Append each tool result's first line to the worker log.
fn push_results_log(log: &mut String, results: &[whycodes_core::types::ToolResult]) {
    for r in results {
        let mark = if r.is_error { "✗" } else { "✓" };
        let first: String = r
            .content
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(100)
            .collect();
        log.push_str(&format!("  {mark} {first}\n"));
    }
    cap_log(log);
}

/// Keep the newest [`MAX_WORKER_LOG`] bytes of the log.
fn cap_log(log: &mut String) {
    if log.len() <= MAX_WORKER_LOG {
        return;
    }
    let mut cut = log.len() - MAX_WORKER_LOG;
    while !log.is_char_boundary(cut) {
        cut += 1;
    }
    log.replace_range(..cut, "…");
}

/// Messages addressed to a subagent that finished before reading them, so
/// the parent still sees them in the result; empty when there are none.
fn unread_tell_note(unread: Vec<whycodes_core::types::MessageContent>) -> String {
    if unread.is_empty() {
        return String::new();
    }
    let mut note = String::from("\n\n[The user sent these after this subagent finished:]\n");
    for content in &unread {
        note.push_str(&format!("- {}\n", crate::steer::content_text(content)));
    }
    note
}

/// Inject memory into the subagent system prompt using parent config.
fn inject_subagent_memory(
    system_prompt: &str,
    project_path: &std::path::Path,
    data_dir: &std::path::Path,
    agent_name: &str,
    query: &str,
    parent_memory: &MemorySettings,
) -> String {
    if !parent_memory.enabled {
        return system_prompt.to_string();
    }
    // Env override still wins for emergency off-switch.
    if std::env::var("WHYCODES_NO_MEMORY")
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
    {
        return system_prompt.to_string();
    }

    let mut settings = parent_memory.clone();
    // Env can force main bank even if config has subagent_banks=true.
    let banks_off = std::env::var("WHYCODES_SUBAGENT_BANKS")
        .map(|v| {
            matches!(
                v.to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        })
        .unwrap_or(false);
    if parent_memory.subagent_banks && !banks_off {
        settings.agent_bank = Some(agent_name.to_string());
    } else {
        settings.agent_bank = None;
    }

    whycodes_memory::apply_memory_prompt(
        system_prompt,
        project_path,
        data_dir,
        &settings,
        Some(query),
    )
}

#[cfg(test)]
#[path = "subagent_tests.rs"]
pub(crate) mod tests;
