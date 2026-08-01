use std::sync::Arc;
use std::time::Instant;

use whycode_core::tool::ToolContext;
use whycode_core::types::{AgentInfo, PermissionSet};
use whycode_llm::provider::ProviderRegistry;
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
}

/// Runner that executes a subagent task by creating a fresh session and
/// delegating to `Agent::run_turn` with the goal as the initial user message.
pub struct SubagentRunner {
    provider_registry: Arc<ProviderRegistry>,
    tool_executor: Arc<ToolExecutor>,
    info: AgentInfo,
    project_path: std::path::PathBuf,
}

impl SubagentRunner {
    /// Create a new runner from the agent's shared state.
    pub fn new(
        provider_registry: Arc<ProviderRegistry>,
        tool_executor: Arc<ToolExecutor>,
        info: AgentInfo,
        project_path: std::path::PathBuf,
    ) -> Self {
        Self {
            provider_registry,
            tool_executor,
            info,
            project_path,
        }
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

        // Create a fresh session for this subagent
        let system_prompt = self
            .info
            .system_prompt
            .clone()
            .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string());

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
            Ok(text) => Ok(SubagentResult {
                goal: task.goal,
                output: text,
                success: true,
                duration,
            }),
            Err(e) => Ok(SubagentResult {
                goal: task.goal,
                output: format!("Subagent error: {}", e),
                success: false,
                duration,
            }),
        }
    }

    /// Internal turn loop — mirrors `Agent::run_turn` but accepts an overridden
    /// PermissionSet so that subagents can have a restricted tool set.
    async fn run_turn_inner(
        &self,
        session: &mut Session,
        provider_name: &str,
        model: &str,
        api_key: &str,
        max_turns: usize,
        permission: &PermissionSet,
    ) -> whycode_core::Result<String> {
        use futures::StreamExt;
        use whycode_core::types::{ContentBlock, StreamEvent, ToolCall};

        // Get tool definitions filtered by the subagent's permission set
        let tools = self.tool_executor.get_definitions(permission);

        let tool_ctx = ToolContext {
            working_dir: session.project_path.to_string_lossy().to_string(),
            session_id: Some(session.id.clone()),
        };

        let provider = self.provider_registry.get(provider_name).ok_or_else(|| {
            whycode_core::Error::Llm(format!(
                "Unknown provider: {}. Available: anthropic, openai, google",
                provider_name
            ))
        })?;

        let mut turn_count = 0;
        let mut final_text = String::new();

        loop {
            turn_count += 1;
            if turn_count > max_turns {
                return Err(whycode_core::Error::Agent(format!(
                    "Exceeded maximum turns ({})",
                    max_turns
                )));
            }

            // Build request
            let request = session.build_request(
                &tools,
                None, // max_tokens
                self.info.temperature,
                Some(true), // thinking
            );

            // Stream response
            let mut accumulated_text = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut current_tool_id = String::new();
            let mut _current_tool_name = String::new();
            let mut current_tool_args = String::new();

            let mut event_stream = provider.stream(&request, api_key, model).await?;

            while let Some(event) = event_stream.next().await {
                match event? {
                    StreamEvent::TextDelta { text } => {
                        accumulated_text.push_str(&text);
                    }
                    StreamEvent::ToolUse { id, name, input } => {
                        current_tool_id = id.clone();
                        _current_tool_name = name.clone();
                        tool_calls.push(ToolCall {
                            id,
                            name,
                            arguments: input,
                        });
                    }
                    StreamEvent::ToolUseDelta {
                        id,
                        input_json_delta,
                    } => {
                        if id == current_tool_id {
                            current_tool_args.push_str(&input_json_delta);
                        }
                    }
                    StreamEvent::Thinking { text } => {
                        tracing::debug!("Subagent thinking: {}", text);
                    }
                    StreamEvent::ThinkingDelta { text } => {
                        tracing::debug!("Subagent thinking: {}", text);
                    }
                    StreamEvent::MessageStop => break,
                    // A subagent's tokens are billed to the same session, but
                    // it does not own one — the parent's accounting is what the
                    // user sees, and routing these there needs a channel this
                    // does not have. Recorded as a known gap in docs/5.md.
                    StreamEvent::Usage { .. } | StreamEvent::CacheUsage { .. } => {}
                    StreamEvent::MessageStart { .. } => {}
                    StreamEvent::MessageDelta { .. } => {}
                    StreamEvent::Error { message } => {
                        return Err(whycode_core::Error::Llm(message));
                    }
                }
            }

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

            session.add_assistant_message(blocks);

            // If no tool calls, we're done
            if tool_calls.is_empty() {
                break;
            }

            // Execute tool calls
            let mut results = Vec::new();
            for tc in &tool_calls {
                let result = self.tool_executor.execute(tc, &tool_ctx, permission).await;
                results.push(result);
            }

            session.add_tool_results(results.clone());
        }

        Ok(final_text)
    }
}
