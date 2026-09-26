//! History-free side turn (`/btw`): answer on screen, never enter the parent session.

use whycodes_core::types::{ContentBlock, Role, ToolCall};
use whycodes_session::session::Session;

use super::agent::Agent;

const SIDE_TURN_MAX_TOKENS: usize = 2_000;
const SIDE_TURN_MAX_STEPS: usize = 4;

pub async fn run(
    agent: &Agent,
    parent: &Session,
    provider_name: &str,
    model: &str,
    api_key: &str,
    question: &str,
) -> whycodes_core::Result<String> {
    let question = question.trim();
    if question.is_empty() {
        return Err(whycodes_core::Error::Agent("Usage: /btw <question>".into()));
    }

    let mut scratch = Session::new(parent.project_path.clone(), parent.system_prompt.clone());
    let mut prior = Vec::new();
    for message in &parent.messages {
        if message.role != Role::Tool {
            prior.push(message.clone());
        }
    }
    scratch.set_messages(prior);
    scratch.add_user_message(&format!(
        "Side question (read-only; do not call bash, write, edit, or apply_patch):\n{question}"
    ));

    let mut last = String::new();
    for _ in 0..SIDE_TURN_MAX_STEPS {
        let mut tools = Vec::new();
        for def in agent.tool_executor_defs_readonly() {
            if is_side_readonly(&def.name) {
                tools.push(def);
            }
        }
        let request = scratch.build_request(tools, Some(1_024), None, Some(false));
        let provider = match agent.provider_registry_get(provider_name) {
            Some(provider) => provider,
            None => {
                return Err(whycodes_core::Error::llm(format!(
                    "Unknown provider: {provider_name}"
                )));
            }
        };
        let transport = whycodes_llm::default_transport();
        let completed = transport.complete(provider, &request, api_key, model).await;
        let response = match completed {
            Ok(response) => response,
            Err(err) => return Err(err),
        };
        let mut calls: Vec<ToolCall> = Vec::new();
        let mut text = String::new();
        for block in response.content {
            match block {
                ContentBlock::Text { text: t } => text.push_str(&t),
                ContentBlock::ToolUse { id, name, input } => {
                    calls.push(ToolCall {
                        id,
                        name,
                        arguments: input,
                    });
                }
                other => {
                    let _ignored = other;
                }
            }
        }
        last = text;
        if calls.is_empty() {
            break;
        }
        let mut results = Vec::new();
        for tc in calls {
            if !is_side_readonly(&tc.name) {
                results.push(whycodes_core::types::ToolResult {
                    tool_call_id: tc.id,
                    content: format!(
                        "/btw cannot call `{}`. Only read, grep, and glob are allowed.",
                        tc.name
                    ),
                    is_error: true,
                });
                continue;
            }
            let ctx = agent.tool_context(&scratch);
            results.push(agent.execute_readonly_tool(&tc, &ctx).await);
        }
        scratch.add_tool_results(results);
    }
    Ok(truncate_tokens(&last, SIDE_TURN_MAX_TOKENS))
}

fn is_side_readonly(name: &str) -> bool {
    match name {
        "read" | "grep" | "glob" => true,
        other => other == "\0",
    }
}

fn truncate_tokens(text: &str, max: usize) -> String {
    let n = whycodes_core::tokens::estimate_tokens(text);
    if n <= max {
        return text.to_string();
    }
    let keep = text.chars().take(max.saturating_mul(4)).collect::<String>();
    format!("{keep}\n…(truncated at {max} tokens)")
}

#[cfg(test)]
#[path = "side_turn_tests.rs"]
mod tests;
