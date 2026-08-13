use std::sync::Arc;
use std::time::Instant;

use whycode_core::SandboxSettings;
use whycode_core::network::NetworkPolicy;
use whycode_core::tool::ToolContext;
use whycode_core::types::{AgentInfo, PermissionSet};
use whycode_llm::provider::ProviderRegistry;
use whycode_memory::MemorySettings;
use whycode_session::session::Session;
use whycode_tools::executor::ToolExecutor;

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
    pub usage: whycode_core::types::Usage,
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
    file_claims: Option<whycode_core::FileClaimRegistry>,
    agent_id: Option<String>,
    agent_label: Option<String>,
    /// Workspace file index inherited from the parent agent (tools fast path).
    file_index: Option<Arc<whycode_index::WorkspaceIndex>>,
    /// Parent TUI panel sink (so workers can pin a preview).
    panel: Option<whycode_core::PanelSink>,
    swarm_hub: Option<whycode_core::SwarmHub>,
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
        }
    }

    /// Inherit the parent agent's workspace file index.
    pub fn with_file_index(mut self, index: Option<Arc<whycode_index::WorkspaceIndex>>) -> Self {
        self.file_index = index;
        self
    }

    /// Inherit the parent agent's side-panel sink.
    pub fn with_panel(mut self, panel: Option<whycode_core::PanelSink>) -> Self {
        self.panel = panel;
        self
    }

    pub fn with_swarm_hub(mut self, hub: Option<whycode_core::SwarmHub>) -> Self {
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
        claims: whycode_core::FileClaimRegistry,
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
    ) -> whycode_core::Result<SubagentResult> {
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
                usage: whycode_core::types::Usage::default(),
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
    ) -> whycode_core::Result<(String, whycode_core::types::Usage)> {
        use futures::StreamExt;
        use whycode_core::types::{ContentBlock, StreamEvent};

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
            swarm_hub: self.swarm_hub.clone(),
        };

        let provider = self.provider_registry.get(provider_name).ok_or_else(|| {
            whycode_core::Error::Llm(format!(
                "Unknown provider: {}. Available: anthropic, openai, google",
                provider_name
            ))
        })?;

        let mut turn_count = 0;
        let mut final_text = String::new();
        let mut total_usage = whycode_core::types::Usage::default();

        loop {
            turn_count += 1;
            if turn_count > max_turns {
                return Err(whycode_core::Error::Agent(format!(
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
                &tools,
                None, // max_tokens
                self.info.temperature,
                Some(true), // thinking
            );

            // Stream response
            let mut accumulated_text = String::new();
            let mut assembler = ToolCallAssembler::new();

            let mut event_stream = whycode_llm::default_transport()
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
                    StreamEvent::MessageStop => break,
                    // Fold into total_usage; parent session adds via SubagentResult.
                    StreamEvent::Usage {
                        input_tokens,
                        output_tokens,
                    } => {
                        total_usage.input_tokens += input_tokens;
                        total_usage.output_tokens += output_tokens;
                    }
                    StreamEvent::CacheUsage {
                        creation_input_tokens,
                        read_input_tokens,
                    } => {
                        *total_usage.cache_creation_input_tokens.get_or_insert(0) +=
                            creation_input_tokens;
                        *total_usage.cache_read_input_tokens.get_or_insert(0) += read_input_tokens;
                    }
                    StreamEvent::MessageStart { .. } => {}
                    StreamEvent::MessageDelta { .. } => {}
                    StreamEvent::Error { message } => {
                        return Err(whycode_core::Error::Llm(message));
                    }
                }
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
                    )
                }) {
                let futs: Vec<_> = tool_calls
                    .iter()
                    .map(|tc| self.tool_executor.execute(tc, &tool_ctx, permission))
                    .collect();
                futures::future::join_all(futs).await
            } else {
                let mut results = Vec::with_capacity(tool_calls.len());
                for tc in &tool_calls {
                    results.push(self.tool_executor.execute(tc, &tool_ctx, permission).await);
                }
                results
            };

            session.add_tool_results(results);
        }

        Ok((final_text, total_usage))
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
    if std::env::var("WHYCODE_NO_MEMORY")
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
    {
        return system_prompt.to_string();
    }

    let data_dir = directories::ProjectDirs::from("com", "whycorporation", "whycode")
        .map(|d| d.data_local_dir().to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    let mut settings = parent_memory.clone();
    // Env can force main bank even if config has subagent_banks=true.
    let banks_off = std::env::var("WHYCODE_SUBAGENT_BANKS")
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

    whycode_memory::apply_memory_prompt(
        system_prompt,
        project_path,
        &data_dir,
        &settings,
        Some(query),
    )
}
