use std::sync::Arc;
use std::time::Instant;

use whycodes_core::SandboxSettings;
use whycodes_core::network::NetworkPolicy;
use whycodes_core::tool::ToolContext;
use whycodes_core::types::{AgentInfo, ApprovalMode, PermissionSet};
use whycodes_llm::provider::ProviderRegistry;
use whycodes_memory::MemorySettings;
use whycodes_session::session::Session;
use whycodes_tools::executor::ToolExecutor;
use whycodes_tools::question::parse_questions;

use crate::question::{
    QuestionPrompter, default_question_prompter, run_question_tool, should_prompt_questions,
};

use super::agent::DEFAULT_SYSTEM_PROMPT;

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
    /// Shared file-claim registry when running inside a swarm.
    file_claims: Option<whycodes_core::FileClaimRegistry>,
    agent_id: Option<String>,
    agent_label: Option<String>,
    /// Workspace file index inherited from the parent agent (tools fast path).
    file_index: Option<Arc<whycodes_index::WorkspaceIndex>>,
    /// Parent TUI panel sink (so workers can pin a preview).
    panel: Option<whycodes_core::PanelSink>,
    swarm_hub: Option<whycodes_core::SwarmHub>,
    question_prompter: Arc<dyn QuestionPrompter>,
    approval_mode: ApprovalMode,
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
            memory: MemorySettings::default(),
            file_claims: None,
            agent_id: None,
            agent_label: None,
            file_index: None,
            panel: None,
            swarm_hub: None,
            question_prompter: default_question_prompter(),
            approval_mode: ApprovalMode::Auto,
        }
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

    pub fn with_swarm_hub(mut self, hub: Option<whycodes_core::SwarmHub>) -> Self {
        self.swarm_hub = hub;
        self
    }

    /// Attach parent memory settings (subagent_banks, inject knobs).
    pub fn with_memory(mut self, memory: MemorySettings) -> Self {
        self.memory = memory;
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
    pub async fn run(
        &self,
        task: SubagentTask,
        provider_name: &str,
        model: &str,
        api_key: &str,
    ) -> whycodes_core::Result<SubagentResult> {
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
            &self.project_path,
            &self.info.name,
            &user_message,
            &self.memory,
        );

        let mut session = Session::new(self.project_path.clone(), system_prompt);
        session.add_user_message(&user_message);

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

        let duration = start.elapsed();

        match output {
            Ok((text, usage)) => Ok(SubagentResult {
                goal: task.goal,
                output: text,
                success: true,
                duration,
                usage,
            }),
            Err(e) => Ok(SubagentResult {
                goal: task.goal,
                output: format!("Subagent error: {}", e),
                success: false,
                duration,
                usage: whycodes_core::types::Usage::default(),
            }),
        }
    }

    /// Internal turn loop — mirrors `Agent::run_turn` but accepts an overridden
    /// PermissionSet so that subagents can have a restricted tool set.
    /// Returns (final text, aggregated provider usage).
    async fn run_turn_inner(
        &self,
        session: &mut Session,
        provider_name: &str,
        model: &str,
        api_key: &str,
        max_turns: usize,
        permission: &PermissionSet,
    ) -> whycodes_core::Result<(String, whycodes_core::types::Usage)> {
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

        loop {
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

            session.add_tool_results(results);
        }

        Ok((final_text, total_usage))
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
        self.tool_executor.execute(tc, tool_ctx, permission).await
    }
}

/// Inject memory into the subagent system prompt using parent config.
fn inject_subagent_memory(
    system_prompt: &str,
    project_path: &std::path::Path,
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

    let data_dir = whycodes_core::paths::data_dir();

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
        &data_dir,
        &settings,
        Some(query),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_memory_passthrough_when_disabled() {
        let memory = whycodes_memory::MemorySettings::disabled();
        let out = inject_subagent_memory(
            "base prompt",
            std::path::Path::new("/work/proj"),
            "worker",
            "do the thing",
            &memory,
        );
        assert_eq!(out, "base prompt");
    }

    #[test]
    fn inject_memory_passthrough_when_env_off_switch() {
        let prev = std::env::var_os("WHYCODES_NO_MEMORY");
        unsafe { std::env::set_var("WHYCODES_NO_MEMORY", "1") };
        let out = inject_subagent_memory(
            "base prompt",
            std::path::Path::new("/work/proj"),
            "worker",
            "do the thing",
            &whycodes_memory::MemorySettings::default(),
        );
        match prev {
            Some(v) => unsafe { std::env::set_var("WHYCODES_NO_MEMORY", v) },
            None => unsafe { std::env::remove_var("WHYCODES_NO_MEMORY") },
        }
        assert_eq!(out, "base prompt");
    }

    #[test]
    fn runner_builders_set_state() {
        let runner = make_runner();
        let idx = whycodes_index::WorkspaceIndex::start(Vec::new());
        let hub = whycodes_core::SwarmHub::default();
        let runner = runner
            .with_file_index(Some(idx))
            .with_panel(None)
            .with_swarm_hub(Some(hub))
            .with_memory(whycodes_memory::MemorySettings::disabled())
            .with_file_claims(
                whycodes_core::FileClaimRegistry::default(),
                "worker-1",
                "Worker One",
            );
        assert_eq!(runner.agent_id.as_deref(), Some("worker-1"));
        assert_eq!(runner.agent_label.as_deref(), Some("Worker One"));
        assert!(runner.file_claims.is_some());
        assert!(runner.file_index.is_some());
        assert!(!runner.memory.enabled);
    }

    #[tokio::test]
    async fn run_returns_failed_result_for_preflight_errors() {
        let cases = [
            ("anthropic", 0, None, "exceeded maximum turns (0)"),
            (
                "missing-provider",
                1,
                Some("use this context".to_string()),
                "Unknown provider: missing-provider",
            ),
        ];

        for (provider, max_turns, context, expected) in cases {
            let result = make_runner()
                .run(
                    SubagentTask {
                        goal: "inspect the project".into(),
                        context,
                        tools: Some(vec!["read".into()]),
                        max_turns,
                    },
                    provider,
                    "test-model",
                    "test-key",
                )
                .await
                .expect("runner converts turn errors into a result");

            assert_eq!(result.goal, "inspect the project");
            assert!(!result.success);
            assert!(result.output.contains(expected), "{}", result.output);
            assert!(result.usage.is_empty());
        }
    }

    fn make_runner() -> SubagentRunner {
        SubagentRunner::new(
            Arc::new(ProviderRegistry::default()),
            Arc::new(ToolExecutor::new()),
            make_info(),
            std::path::PathBuf::from("/work/proj"),
            SandboxSettings::off(),
            NetworkPolicy::unrestricted(),
        )
    }

    fn make_info() -> AgentInfo {
        AgentInfo {
            name: "worker".into(),
            description: "test worker".into(),
            mode: whycodes_core::types::AgentMode::Primary,
            permission: PermissionSet::default(),
            model: None,
            system_prompt: None,
            temperature: None,
            top_p: None,
        }
    }
}
