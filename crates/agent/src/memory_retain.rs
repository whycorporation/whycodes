//! Post-turn memory retain: heuristic + optional small-LLM extract.

use whycode_core::types::{LlmRequest, Message, MessageContent, Role};
use whycode_llm::provider::LlmProvider;
use whycode_memory::{MemoryService, MemorySettings, llm_retain_prompt};
use whycode_session::session::Session;

use crate::title::resolve_title_model;

/// Run heuristic (+ optional LLM) retain after a successful turn.
///
/// Returns saved fact strings (for UI toast/status). Failures are logged only.
pub async fn run_post_turn_retain(
    session: &Session,
    assistant_text: &str,
    settings: &MemorySettings,
    provider: &dyn LlmProvider,
    provider_name: &str,
    model: &str,
    api_key: &str,
    data_dir: &std::path::Path,
) -> Vec<String> {
    if !settings.enabled || !settings.auto_retain {
        return Vec::new();
    }

    let Ok(svc) = MemoryService::open(&session.project_path, data_dir, settings.clone()) else {
        return Vec::new();
    };

    let turn_index = session.user_message_count().max(1);
    let user_text = session
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .and_then(|m| m.content.as_text().map(|s| s.to_string()))
        .unwrap_or_default();

    let mut saved = svc
        .auto_retain(
            &user_text,
            Some(assistant_text),
            Some(&session.id),
            turn_index,
        )
        .unwrap_or_default();

    if svc.should_run_llm_retain(saved.len(), turn_index) {
        match llm_extract_facts(
            provider,
            provider_name,
            model,
            api_key,
            &user_text,
            assistant_text,
        )
        .await
        {
            Ok(raw) => {
                if let Ok(more) = svc.retain_llm_facts(&raw, Some(&session.id)) {
                    for f in more {
                        if !saved.iter().any(|s| s.eq_ignore_ascii_case(&f)) {
                            saved.push(f);
                        }
                    }
                }
            }
            Err(e) => {
                tracing::debug!("llm retain skipped: {e}");
            }
        }
    }

    saved
}

async fn llm_extract_facts(
    provider: &dyn LlmProvider,
    provider_name: &str,
    model: &str,
    api_key: &str,
    user: &str,
    assistant: &str,
) -> whycode_core::Result<String> {
    // Prefer a cheap sibling model (same strategy as title refine).
    let (p_name, m_id) = resolve_title_model(provider_name, model, None);
    let use_provider = if p_name == provider_name {
        provider
    } else {
        // Caller only passed one provider handle; stick to the session provider.
        let _ = p_name;
        provider
    };
    let use_model = if m_id != model {
        m_id
    } else {
        model.to_string()
    };

    let user_clip: String = user.chars().take(1200).collect();
    let asst_clip: String = assistant.chars().take(800).collect();
    let prompt = llm_retain_prompt(&user_clip, &asst_clip);

    let request = LlmRequest {
        system: "You extract durable coding-project facts only. No secrets. If none: NONE."
            .into(),
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text(prompt),
            tool_call_id: None,
            name: None,
        }],
        tools: Vec::new(),
        max_tokens: Some(200),
        temperature: Some(0.1),
        top_p: None,
        top_k: None,
        stop_sequences: None,
        thinking: None,
        use_prompt_cache: false,
    };

    let transport = whycode_llm::LlmTransport {
        complete_timeout: Some(std::time::Duration::from_secs(12)),
        retry: whycode_llm::RetryPolicy {
            max_retries: 1,
            initial_backoff: std::time::Duration::from_millis(300),
            max_backoff: std::time::Duration::from_secs(3),
            max_elapsed: std::time::Duration::from_secs(12),
            full_jitter: true,
        },
    };
    let response = transport
        .complete(use_provider, &request, api_key, &use_model)
        .await?;

    let raw = response
        .content
        .iter()
        .filter_map(|b| match b {
            whycode_core::types::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(raw)
}
