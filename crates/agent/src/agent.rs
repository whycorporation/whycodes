use futures::StreamExt;
use std::sync::Arc;
use whycode_core::SandboxSettings;
use whycode_core::network::NetworkPolicy;
use whycode_core::tool::ToolContext;
use whycode_core::types::{
    AgentInfo, ContentBlock, PermissionAction, StreamEvent, ToolCall, ToolResult,
};
use whycode_llm::provider::ProviderRegistry;
use whycode_tools::executor::ToolExecutor;

use whycode_session::session::Session;

use super::events::{CancelFlag, EventSink, TurnEvent, emit, is_cancelled};
use super::permission::{PermissionPrompter, default_prompter};
use super::subagent::{SubagentRunner, SubagentTask};
use super::tool_stream::ToolCallAssembler;
use whycode_command_risk::{Decision, RiskThreshold, assess, decide};

/// Tool names that run an arbitrary shell command string.
const SHELL_TOOLS: &[&str] = &["bash", "shell"];

/// Default system prompt (loaded from prompts/build.txt at compile time)
pub const DEFAULT_SYSTEM_PROMPT: &str = include_str!("../prompts/build.txt");

/// Main agent orchestrating the conversation loop
pub struct Agent {
    pub info: AgentInfo,
    provider_registry: Arc<ProviderRegistry>,
    tool_executor: Arc<ToolExecutor>,
    permission_prompter: Arc<dyn PermissionPrompter>,
    risk_threshold: RiskThreshold,
    sandbox: SandboxSettings,
    network: NetworkPolicy,
}

impl Agent {
    pub fn new(info: AgentInfo) -> Self {
        Self {
            info,
            provider_registry: Arc::new(ProviderRegistry::default()),
            tool_executor: Arc::new(ToolExecutor::new()),
            permission_prompter: default_prompter(),
            risk_threshold: RiskThreshold::default(),
            sandbox: SandboxSettings::default(),
            network: NetworkPolicy::unrestricted(),
        }
    }

    pub fn with_provider_registry(mut self, registry: ProviderRegistry) -> Self {
        self.provider_registry = Arc::new(registry);
        self
    }

    pub fn with_tool_executor(mut self, executor: ToolExecutor) -> Self {
        self.tool_executor = Arc::new(executor);
        self
    }

    pub fn with_permission_prompter(mut self, prompter: Arc<dyn PermissionPrompter>) -> Self {
        self.permission_prompter = prompter;
        self
    }

    /// Load custom providers from config and merge global permission rules.
    pub fn with_config(mut self, config: &whycode_config::Config) -> Self {
        let mut registry = ProviderRegistry::default();
        registry.register_from_config(config);
        self.provider_registry = Arc::new(registry);
        self.info.permission = config.effective_permission(&self.info.permission);
        self.risk_threshold = config
            .security
            .bash_risk_threshold
            .parse()
            .unwrap_or_else(|e| {
                tracing::warn!("{e}; falling back to the default");
                RiskThreshold::default()
            });
        self.sandbox = config.security.sandbox_settings();
        self.network = config.security.network_policy();
        tracing::debug!(
            sandbox = %whycode_sandbox::describe_backend(&self.sandbox),
            network_allow = self.network.allowlist.len(),
            network_deny = self.network.denylist.len(),
            "shell sandbox and network policy"
        );
        self
    }

    /// Build tool context for a session, applying permission network flags.
    fn tool_context(&self, session: &Session) -> ToolContext {
        let mut sandbox = self.sandbox.clone();
        if !self.info.permission.allow_network {
            sandbox.network = false;
        }
        ToolContext {
            working_dir: session.project_path.to_string_lossy().to_string(),
            session_id: Some(session.id.clone()),
            sandbox,
            network: self.network.clone(),
        }
    }

    /// Connect MCP servers from config and register their tools on a fresh executor.
    pub async fn with_mcp(mut self, config: &whycode_config::Config) -> Self {
        if config.mcp_servers.is_empty() {
            return self;
        }
        let mut full = ToolExecutor::new();
        let n = super::mcp_load::register_mcp_tools(&mut full, config).await;
        if n > 0 {
            self.tool_executor = Arc::new(full);
            tracing::info!(count = n, "MCP tools registered");
        }
        self
    }

    /// Get the system prompt for this agent (includes runtime context such as today's date).
    pub fn system_prompt(&self) -> String {
        let base = self
            .info
            .system_prompt
            .clone()
            .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string());
        Self::with_runtime_context(&base)
    }

    /// Get the system prompt for a named agent.
    ///
    /// If the agent has an explicit `system_prompt` set in its info, that wins.
    /// Otherwise falls back to loading the matching prompt file from
    /// `crates/agent/prompts/<name>.txt` at compile time.
    ///
    /// Does **not** attach AGENTS.md or runtime context — callers that build a
    /// live session should pass the result through [`Self::with_agents_md`].
    pub fn system_prompt_for(agent_name: &str) -> String {
        match agent_name {
            "build" => include_str!("../prompts/build.txt").to_string(),
            "plan" => include_str!("../prompts/plan.txt").to_string(),
            "explore" => include_str!("../prompts/explore.txt").to_string(),
            "general" => include_str!("../prompts/general.txt").to_string(),
            "scout" => include_str!("../prompts/explore.txt").to_string(),
            _ => DEFAULT_SYSTEM_PROMPT.to_string(),
        }
    }

    /// Append runtime environment facts the model needs for time-sensitive work.
    ///
    /// Idempotent: if the prompt already contains `Today's date:`, it is returned unchanged.
    pub fn with_runtime_context(system_prompt: &str) -> String {
        if system_prompt.contains("Today's date:") {
            return system_prompt.to_string();
        }
        let today = chrono::Local::now().format("%Y-%m-%d");
        format!(
            "{system_prompt}\n\n# Environment\n\n\
             Today's date: {today}.\n\
             When searching for the current or latest version of software, do not pin the query to a past year; \
             prefer canonical sources (npm registry, GitHub Releases, official docs)."
        )
    }

    /// Append project AGENTS.md (OpenCode rules file) and runtime context to a system prompt.
    pub fn with_agents_md(system_prompt: &str, project_path: &std::path::Path) -> String {
        let candidates = [
            project_path.join("AGENTS.md"),
            project_path.join("agents.md"),
            project_path.join(".whycode").join("AGENTS.md"),
        ];
        let with_agents = {
            let mut out = None;
            for path in &candidates {
                if let Ok(content) = std::fs::read_to_string(path) {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() {
                        out = Some(format!(
                            "{}\n\n# Project Instructions (AGENTS.md)\n\n{}",
                            system_prompt, trimmed
                        ));
                        break;
                    }
                }
            }
            out.unwrap_or_else(|| system_prompt.to_string())
        };
        Self::with_runtime_context(&with_agents)
    }

    /// Run a single conversation turn (no streaming UI events).
    pub async fn run_turn(
        &self,
        session: &mut Session,
        provider_name: &str,
        model: &str,
        api_key: &str,
        max_turns: usize,
    ) -> whycode_core::Result<String> {
        self.run_turn_with_events(
            session,
            provider_name,
            model,
            api_key,
            max_turns,
            None,
            None,
        )
        .await
    }

    /// Run a turn, optionally streaming `TurnEvent`s and honouring a cancel flag (Esc).
    #[allow(clippy::too_many_arguments)]
    pub async fn run_turn_with_events(
        &self,
        session: &mut Session,
        provider_name: &str,
        model: &str,
        api_key: &str,
        max_turns: usize,
        events: Option<EventSink>,
        cancel: Option<CancelFlag>,
    ) -> whycode_core::Result<String> {
        let tools = self.tool_executor.get_definitions(&self.info.permission);

        let tool_ctx = self.tool_context(session);

        let provider = self
            .provider_registry
            .get(provider_name)
            .ok_or_else(|| {
                whycode_core::Error::Llm(format!(
                    "Unknown provider: {}. Available: anthropic, openai, google, and configured custom providers",
                    provider_name
                ))
            })?;

        let mut turn_count = 0;
        let mut final_text = String::new();

        loop {
            if is_cancelled(&cancel) {
                emit(&events, TurnEvent::Cancelled);
                return Err(whycode_core::Error::Agent("Cancelled".into()));
            }

            turn_count += 1;
            if turn_count > max_turns {
                return Err(whycode_core::Error::Agent(format!(
                    "Exceeded maximum turns ({})",
                    max_turns
                )));
            }

            emit(
                &events,
                TurnEvent::Status(format!("LLM request (step {turn_count})…")),
            );

            let request = session.build_request(&tools, None, self.info.temperature, Some(true));

            let mut accumulated_text = String::new();
            let mut turn_usage = whycode_core::types::Usage::default();
            let mut assembler = ToolCallAssembler::new();

            let mut event_stream = provider.stream(&request, api_key, model).await?;

            while let Some(event) = event_stream.next().await {
                if is_cancelled(&cancel) {
                    // Persist partial assistant text before aborting
                    if !accumulated_text.is_empty() {
                        session.add_assistant_message(vec![ContentBlock::Text {
                            text: accumulated_text.clone(),
                        }]);
                        final_text.push_str(&accumulated_text);
                    }
                    emit(&events, TurnEvent::Cancelled);
                    return Err(whycode_core::Error::Agent("Cancelled".into()));
                }

                match event? {
                    StreamEvent::TextDelta { text } => {
                        emit(&events, TurnEvent::TextDelta(text.clone()));
                        accumulated_text.push_str(&text);
                    }
                    StreamEvent::ToolUse { id, name, input } => {
                        // Defer ToolStart until after argument fragments are
                        // merged — OpenAI streams send null/empty args first.
                        assembler.on_tool_use(id, name, input);
                    }
                    StreamEvent::ToolUseDelta {
                        id,
                        input_json_delta,
                    } => {
                        assembler.on_tool_use_delta(&id, &input_json_delta);
                    }
                    StreamEvent::Thinking { text } => {
                        emit(&events, TurnEvent::ThinkingDelta(text.clone()));
                        tracing::debug!("Thinking: {}", text);
                    }
                    StreamEvent::ThinkingDelta { text } => {
                        emit(&events, TurnEvent::ThinkingDelta(text.clone()));
                        tracing::debug!("Thinking: {}", text);
                    }
                    StreamEvent::MessageStop => break,
                    StreamEvent::Usage {
                        input_tokens,
                        output_tokens,
                    } => {
                        // Providers report these in pieces — Anthropic sends
                        // input at message_start and output at message_delta —
                        // so accumulate rather than replace.
                        turn_usage.input_tokens += input_tokens;
                        turn_usage.output_tokens += output_tokens;
                    }
                    StreamEvent::CacheUsage {
                        creation_input_tokens,
                        read_input_tokens,
                    } => {
                        *turn_usage.cache_creation_input_tokens.get_or_insert(0) +=
                            creation_input_tokens;
                        *turn_usage.cache_read_input_tokens.get_or_insert(0) += read_input_tokens;
                    }
                    StreamEvent::MessageStart { .. } => {}
                    StreamEvent::MessageDelta { .. } => {}
                    StreamEvent::Error { message } => {
                        return Err(whycode_core::Error::Llm(message));
                    }
                }
            }

            // Merge streamed argument fragments into parsed JSON objects.
            let tool_calls = assembler.finish();

            // Emit ToolStart with final parsed arguments (not the empty first chunk).
            for tc in &tool_calls {
                emit(
                    &events,
                    TurnEvent::ToolStart {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        input: tc.arguments.clone(),
                    },
                );
            }

            // Once per turn, after the stream closes and before any tool runs.
            // A provider that reports nothing produces no event, so a silent
            // provider is distinguishable from a zero-cost turn.
            if !turn_usage.is_empty() {
                session.add_usage(&turn_usage);
                emit(&events, TurnEvent::Usage(turn_usage.clone()));
            }

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

            if tool_calls.is_empty() {
                break;
            }

            let mut results = Vec::new();
            for tc in &tool_calls {
                if is_cancelled(&cancel) {
                    emit(&events, TurnEvent::Cancelled);
                    return Err(whycode_core::Error::Agent("Cancelled".into()));
                }
                emit(
                    &events,
                    TurnEvent::Status(format!("Running tool `{}`…", tc.name)),
                );
                let result = self
                    .execute_with_permission(tc, session, &tool_ctx, provider_name, model, api_key)
                    .await;
                emit(
                    &events,
                    TurnEvent::ToolEnd {
                        id: tc.id.clone(),
                        content: result.content.clone(),
                        is_error: result.is_error,
                    },
                );
                results.push(result);
            }

            session.add_tool_results(results.clone());

            let failed_tools: Vec<String> = results
                .iter()
                .filter(|r| r.is_error)
                .map(|r| {
                    format!(
                        "The tool failed with error: {content}. Please correct your approach.",
                        content = r.content
                    )
                })
                .collect();

            if !failed_tools.is_empty() {
                let recovery_msg = failed_tools.join("\n");
                session.add_user_message(&recovery_msg);
            }
        }

        Ok(final_text)
    }

    /// Apply the shell risk gate, then allow/ask/deny, then execute (or spawn
    /// a task subagent).
    ///
    /// `pub(crate)` so the risk gate can be tested at this level: the unit
    /// tests in `command-risk` cover classification, but only this method
    /// proves that a catastrophic command is refused even when the permission
    /// map says `allow`.
    pub(crate) async fn execute_with_permission(
        &self,
        tc: &ToolCall,
        session: &Session,
        tool_ctx: &ToolContext,
        provider_name: &str,
        model: &str,
        api_key: &str,
    ) -> ToolResult {
        // Shell commands are gated on what the command would destroy. The
        // permission map below only sees the tool name, so on its own `allow`
        // would run anything the model emits.
        let mut risk_confirmed = false;
        if SHELL_TOOLS.contains(&tc.name.as_str()) {
            let command = tc
                .arguments
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
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
                    let detail = format!("{command}\n\nRisk: {reason}");
                    if !self.permission_prompter.ask(&tc.name, &detail).await {
                        return ToolResult {
                            tool_call_id: tc.id.clone(),
                            content: format!("User denied permission for tool '{}'.", tc.name),
                            is_error: true,
                        };
                    }
                    risk_confirmed = true;
                }
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
                let detail = tc.arguments.to_string();
                let detail = if detail.len() > 200 {
                    format!("{}…", &detail[..200])
                } else {
                    detail
                };
                let allowed = self.permission_prompter.ask(&tc.name, &detail).await;
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

        if tc.name == "task" {
            self.execute_task_tool(tc, session, provider_name, model, api_key)
                .await
        } else {
            self.tool_executor
                .execute(tc, tool_ctx, &self.info.permission)
                .await
        }
    }

    /// Execute the `task` tool by spawning a real subagent (OpenCode Task tool parity).
    async fn execute_task_tool(
        &self,
        call: &ToolCall,
        session: &Session,
        provider_name: &str,
        model: &str,
        api_key: &str,
    ) -> whycode_core::types::ToolResult {
        use whycode_core::types::{AgentMode, PermissionSet, ToolResult};

        let goal = call
            .arguments
            .get("goal")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if goal.is_empty() {
            return ToolResult {
                tool_call_id: call.id.clone(),
                content: "task requires a non-empty `goal`".to_string(),
                is_error: true,
            };
        }

        let context = call
            .arguments
            .get("context")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let subagent_type = call
            .arguments
            .get("subagent_type")
            .and_then(|v| v.as_str())
            .unwrap_or("general");
        let max_turns = call
            .arguments
            .get("max_turns")
            .and_then(|v| v.as_u64())
            .unwrap_or(15) as usize;

        // Permission profile per OpenCode subagent type
        let (permission, system_prompt) = match subagent_type {
            "explore" | "scout" => (
                PermissionSet {
                    allowed_tools: Some(vec![
                        "read".into(),
                        "grep".into(),
                        "glob".into(),
                        "list".into(),
                        "webfetch".into(),
                        "websearch".into(),
                        "lsp".into(),
                    ]),
                    denied_tools: Some(vec![
                        "write".into(),
                        "edit".into(),
                        "shell".into(),
                        "bash".into(),
                        "apply_patch".into(),
                        "todowrite".into(),
                        "todo".into(),
                    ]),
                    allow_file_writes: false,
                    allow_network: true,
                    allow_shell: false,
                    allowed_paths: None,
                    rules: Default::default(),
                },
                Self::system_prompt_for(subagent_type),
            ),
            _ => {
                // general: full tools except todo (OpenCode default)
                let mut perm = self.info.permission.clone();
                let mut denied = perm.denied_tools.unwrap_or_default();
                for t in ["todowrite", "todo", "todoread"] {
                    if !denied.iter().any(|x| x == t) {
                        denied.push(t.to_string());
                    }
                }
                perm.denied_tools = Some(denied);
                (perm, Self::system_prompt_for("general"))
            }
        };

        let mut info = self.info.clone();
        info.name = subagent_type.to_string();
        info.mode = AgentMode::Subagent;
        info.permission = permission;
        info.system_prompt = Some(Self::with_agents_md(&system_prompt, &session.project_path));

        let task = SubagentTask {
            goal: goal.clone(),
            context,
            tools: None,
            max_turns,
        };

        let runner = SubagentRunner::new(
            Arc::clone(&self.provider_registry),
            Arc::clone(&self.tool_executor),
            info,
            session.project_path.clone(),
            self.sandbox.clone(),
            self.network.clone(),
        );

        match runner.run(task, provider_name, model, api_key).await {
            Ok(result) => ToolResult {
                tool_call_id: call.id.clone(),
                content: if result.success {
                    format!(
                        "Subagent ({}) completed in {:.1}s:\n\n{}",
                        subagent_type,
                        result.duration.as_secs_f64(),
                        result.output
                    )
                } else {
                    format!(
                        "Subagent ({}) finished with errors:\n\n{}",
                        subagent_type, result.output
                    )
                },
                is_error: !result.success,
            },
            Err(e) => ToolResult {
                tool_call_id: call.id.clone(),
                content: format!("Failed to run subagent: {}", e),
                is_error: true,
            },
        }
    }

    /// Spawn a single subagent to accomplish a goal.
    ///
    /// The subagent runs in a fresh session with its own conversation loop.
    /// Returns the subagent's textual output.
    #[allow(clippy::too_many_arguments)]
    pub async fn spawn_subagent(
        &self,
        goal: String,
        context: Option<String>,
        tools: Option<Vec<String>>,
        max_turns: usize,
        provider_name: &str,
        model: &str,
        api_key: &str,
        project_path: std::path::PathBuf,
    ) -> whycode_core::Result<String> {
        let task = SubagentTask {
            goal: goal.clone(),
            context,
            tools,
            max_turns,
        };

        let runner = SubagentRunner::new(
            Arc::clone(&self.provider_registry),
            Arc::clone(&self.tool_executor),
            self.info.clone(),
            project_path,
            self.sandbox.clone(),
            self.network.clone(),
        );

        let result = runner.run(task, provider_name, model, api_key).await?;

        Ok(result.output)
    }

    /// Spawn multiple subagents in parallel, respecting a concurrency limit.
    ///
    /// Each `SubagentTask` spawns an independent subagent. Up to `max_concurrent`
    /// subagents run at once; the rest are queued. Returns a Vec of outputs in the
    /// same order as the input tasks.
    pub async fn spawn_parallel(
        &self,
        goals: Vec<SubagentTask>,
        max_concurrent: usize,
        provider_name: &str,
        model: &str,
        api_key: &str,
        project_path: std::path::PathBuf,
    ) -> whycode_core::Result<Vec<String>> {
        use tokio::sync::Semaphore;

        let sem = Arc::new(Semaphore::new(max_concurrent.max(1)));
        let provider_name = Arc::from(provider_name.to_string());
        let model = Arc::from(model.to_string());
        let api_key = Arc::from(api_key.to_string());

        let runner = Arc::new(SubagentRunner::new(
            Arc::clone(&self.provider_registry),
            Arc::clone(&self.tool_executor),
            self.info.clone(),
            project_path,
            self.sandbox.clone(),
            self.network.clone(),
        ));

        let mut handles = Vec::with_capacity(goals.len());

        for task in goals {
            let permit = Arc::clone(&sem);
            let r = Arc::clone(&runner);
            let pn = Arc::clone(&provider_name);
            let m = Arc::clone(&model);
            let ak = Arc::clone(&api_key);

            handles.push(tokio::spawn(async move {
                let _guard = permit.acquire().await;
                r.run(task, &pn, &m, &ak).await
            }));
        }

        let mut outputs = Vec::with_capacity(handles.len());
        for handle in handles {
            match handle.await {
                Ok(Ok(result)) => outputs.push(result.output),
                Ok(Err(e)) => outputs.push(format!("Subagent error: {}", e)),
                Err(e) => outputs.push(format!("Join error: {}", e)),
            }
        }

        Ok(outputs)
    }
}
