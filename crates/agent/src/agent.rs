use futures::StreamExt;
use std::sync::Arc;
use whycode_core::types::{
    AgentInfo, ContentBlock, StreamEvent, ToolCall,
};
use whycode_llm::provider::ProviderRegistry;
use whycode_tools::executor::ToolExecutor;
use whycode_tools::tool::ToolContext;

use whycode_session::session::Session;

/// Default system prompt
pub const DEFAULT_SYSTEM_PROMPT: &str = include_str!("../prompt.txt");

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
        }

        Ok(final_text)
    }
}

