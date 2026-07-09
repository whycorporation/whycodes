use futures::StreamExt;
use std::sync::Arc;
use whycode_core::types::{
    AgentInfo, ContentBlock, StreamEvent, ToolCall,
};
use whycode_llm::provider::ProviderRegistry;
use whycode_tools::executor::ToolExecutor;
use whycode_tools::tool::ToolContext;

use whycode_session::session::Session;

use super::subagent::{SubagentRunner, SubagentTask};

/// Default system prompt (loaded from prompts/build.txt at compile time)
pub const DEFAULT_SYSTEM_PROMPT: &str = include_str!("../prompts/build.txt");

/// Main agent orchestrating the conversation loop
pub struct Agent {
    pub info: AgentInfo,
    provider_registry: Arc<ProviderRegistry>,
    tool_executor: Arc<ToolExecutor>,
}

impl Agent {
    pub fn new(info: AgentInfo) -> Self {
        Self {
            info,
            provider_registry: Arc::new(ProviderRegistry::default()),
            tool_executor: Arc::new(ToolExecutor::new()),
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

    /// Get the system prompt for this agent
    pub fn system_prompt(&self) -> String {
        self.info
            .system_prompt
            .clone()
            .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string())
    }

    /// Get the system prompt for a named agent.
    ///
    /// If the agent has an explicit `system_prompt` set in its info, that wins.
    /// Otherwise falls back to loading the matching prompt file from
    /// `crates/agent/prompts/<name>.txt` at compile time.
    pub fn system_prompt_for(agent_name: &str) -> String {
        match agent_name {
            "build" => include_str!("../prompts/build.txt").to_string(),
            "plan" => include_str!("../prompts/plan.txt").to_string(),
            "explore" => include_str!("../prompts/explore.txt").to_string(),
            "general" => include_str!("../prompts/general.txt").to_string(),
            _ => DEFAULT_SYSTEM_PROMPT.to_string(),
        }
    }

    /// Run a single conversation turn: send LLM request, process tool calls, return response
    pub async fn run_turn(
        &self,
        session: &mut Session,
        provider_name: &str,
        model: &str,
        api_key: &str,
        max_turns: usize,
    ) -> whycode_core::Result<String> {
        // Get tool definitions
        let tools = self
            .tool_executor
            .get_definitions(&self.info.permission);

        let tool_ctx = ToolContext {
            working_dir: session.project_path.to_string_lossy().to_string(),
            session_id: Some(session.id.clone()),
        };

        let provider = self
            .provider_registry
            .get(provider_name)
            .ok_or_else(|| {
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
                None,    // max_tokens
                self.info.temperature,
                Some(true), // thinking: enabled for complex tasks
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
                        // Thinking content is not shown to user by default
                        tracing::debug!("Thinking: {}", text);
                    }
                    StreamEvent::ThinkingDelta { text } => {
                        tracing::debug!("Thinking: {}", text);
                    }
                    StreamEvent::MessageStop => break,
                    StreamEvent::Usage { .. } => {}
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
                let result = self
                    .tool_executor
                    .execute(tc, &tool_ctx, &self.info.permission)
                    .await;
                results.push(result);
            }

            session.add_tool_results(results.clone());

            // Error recovery: if any tool failed, inject a user-like message
            // giving the LLM a chance to self-correct in the same turn
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

    /// Spawn a single subagent to accomplish a goal.
    ///
    /// The subagent runs in a fresh session with its own conversation loop.
    /// Returns the subagent's textual output.
    pub async fn spawn_subagent(
        &self,
        goal: String,
        context: Option<String>,
        tools: Option<Vec<String>>,
        max_turns: usize,
        provider_name: &str,
        model: &str,
        api_key: &str,
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
            std::path::PathBuf::new(), // subagent uses the same working dir logic via the ToolContext
        );

        // Actually we need the project path — let's get it from the AgentInfo or default.
        // SubagentRunner::run will use it from info, which is cloned above. We pass cwd from
        // the environment; the runner creates a Session whose project_path is that value.
        // Here we default to ".". The important piece is that ToolContext.working_dir
        // inside run_turn_inner comes from the session's project_path.
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
            std::path::PathBuf::new(),
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

