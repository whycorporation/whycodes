//! Auto session titles via a small/fast model (industry standard + refine).
//!
//! Flow elsewhere:
//! 1. Placeholder `{project}-{ab}` at create
//! 2. Offline heuristic on first user message
//! 3. This module upgrades the title after the first successful turn using the
//!    user prompt + a short assistant excerpt (better than first-message-only).

use whycode_core::types::{LlmRequest, Message, MessageContent, Role};
use whycode_llm::provider::LlmProvider;
use whycode_session::Session;
use whycode_session::title::sanitize_title;

const TITLE_SYSTEM: &str = "\
You name coding-agent chat sessions.
Reply with ONLY a short title (3–8 words). No quotes, no trailing punctuation, \
no markdown, no emoji unless essential to meaning. Prefer concrete nouns \
(files, features, bugs, APIs) over vague words like \"help\" or \"question\". \
Match the user's language.";

/// Resolve which model id to use for title generation.
///
/// Prefer `override_model` (`provider/model` or bare model id). Otherwise pick
/// a known small/fast sibling for the active provider; fall back to `model`.
pub fn resolve_title_model(
    provider: &str,
    model: &str,
    override_model: Option<&str>,
) -> (String, String) {
    if let Some(raw) = override_model.map(str::trim).filter(|s| !s.is_empty()) {
        if let Some((p, m)) = raw.split_once('/') {
            return (p.to_string(), m.to_string());
        }
        return (provider.to_string(), raw.to_string());
    }
    let p = provider.to_ascii_lowercase();
    let m = model.to_ascii_lowercase();
    let small = small_model_for(&p, &m).unwrap_or(model);
    (provider.to_string(), small.to_string())
}

fn small_model_for(provider: &str, model: &str) -> Option<&'static str> {
    // Already a cheap model — keep it.
    if is_already_small(model) {
        return None;
    }
    match provider {
        "anthropic" => Some("claude-haiku-4-5-20251001"),
        "openai" => Some("gpt-4o-mini"),
        "google" => Some("gemini-2.0-flash"),
        "xai" => Some("grok-3-mini"),
        "groq" => Some("llama-3.1-8b-instant"),
        "mistral" => Some("mistral-small-latest"),
        "deepseek" => Some("deepseek-chat"),
        "together" => Some("meta-llama/Meta-Llama-3.1-8B-Instruct-Turbo"),
        "openrouter" => {
            // Prefer same family cheap routes when possible.
            if model.contains("claude") {
                Some("anthropic/claude-haiku-4.5")
            } else if model.contains("gpt") || model.contains("o1") || model.contains("o3") {
                Some("openai/gpt-4o-mini")
            } else if model.contains("gemini") {
                Some("google/gemini-2.0-flash-001")
            } else {
                Some("openai/gpt-4o-mini")
            }
        }
        "ollama" => None, // keep local model
        _ => None,
    }
}

fn is_already_small(model: &str) -> bool {
    const MARKERS: &[&str] = &[
        "haiku", "mini", "nano", "flash", "small", "lite", "8b", "7b", "3b", "instant",
    ];
    MARKERS.iter().any(|m| model.contains(m))
}

/// Generate a refined title; empty string on blank/useless output.
pub async fn generate_title(
    provider: &dyn LlmProvider,
    api_key: &str,
    model: &str,
    user_text: &str,
    assistant_snippet: Option<&str>,
) -> whycode_core::Result<String> {
    let user_text = truncate(user_text, 800);
    let mut body = format!("User request:\n{user_text}");
    if let Some(a) = assistant_snippet.map(str::trim).filter(|s| !s.is_empty()) {
        body.push_str("\n\nAssistant excerpt:\n");
        body.push_str(&truncate(a, 400));
    }
    body.push_str("\n\nSession title:");

    let request = LlmRequest {
        system: TITLE_SYSTEM.to_string(),
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text(body),
            tool_call_id: None,
            name: None,
        }],
        tools: Vec::new(),
        max_tokens: Some(48),
        temperature: Some(0.2),
        top_p: None,
        top_k: None,
        stop_sequences: None,
        thinking: None,
    };

    let response = provider.complete(&request, api_key, model).await?;
    let raw = response
        .content
        .iter()
        .filter_map(|b| match b {
            whycode_core::types::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ");
    // Models sometimes prefix "Title:" — strip once.
    let line = raw
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .trim_start_matches("Title:")
        .trim_start_matches("title:")
        .trim();
    Ok(sanitize_title(line))
}

fn truncate(s: &str, max_chars: usize) -> String {
    let n = s.chars().count();
    if n <= max_chars {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max_chars).collect();
        out.push('…');
        out
    }
}

/// Whether this session is eligible for an LLM title pass right now.
///
/// - First user turn (`user_message_count == 1`): normal path after heuristic.
/// - Still on [`TitleSource::Default`] with a longer transcript: one chance to
///   replace legacy `New session - …` / `project-ab` placeholders on resume.
pub fn should_refine_title(session: &Session) -> bool {
    if !session.title_source.allows_llm() || session.first_user_text().is_none() {
        return false;
    }
    session.user_message_count() == 1
        || session.title_source == whycode_session::TitleSource::Default
}

/// Apply a generated title string, logging success/empty results.
pub fn apply_refine_result(session: &mut Session, title: &str, model: &str) {
    if title.is_empty() {
        tracing::debug!("title model returned empty; keeping heuristic/default");
        return;
    }
    if session.apply_generated_title(title) {
        tracing::debug!(%title, %model, "session title refined");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_override_and_small_siblings() {
        let (p, m) = resolve_title_model("anthropic", "claude-sonnet-4-5", None);
        assert_eq!(p, "anthropic");
        assert!(m.contains("haiku"));

        let (p, m) = resolve_title_model("openai", "gpt-4o", Some("gpt-4.1-nano"));
        assert_eq!(p, "openai");
        assert_eq!(m, "gpt-4.1-nano");

        let (p, m) = resolve_title_model("xai", "grok-4", Some("openrouter/foo"));
        assert_eq!(p, "openrouter");
        assert_eq!(m, "foo");
    }

    #[test]
    fn keeps_already_small_model() {
        let (p, m) = resolve_title_model("openai", "gpt-4o-mini", None);
        assert_eq!(p, "openai");
        assert_eq!(m, "gpt-4o-mini");
    }

    #[test]
    fn refine_gate_allows_legacy_default_multi_turn() {
        let mut session = Session::new(std::path::PathBuf::from("/tmp/proj"), String::new());
        // Placeholder still Default after two user turns (pre-auto-title rows).
        session.title = "New session - 2026-01-01".into();
        session.title_source = whycode_session::TitleSource::Default;
        session.add_user_message("fix auth");
        session.add_user_message("also retries");
        assert!(should_refine_title(&session));

        session.title_source = whycode_session::TitleSource::Heuristic;
        assert!(!should_refine_title(&session)); // multi-turn + already heuristicked
    }
}
