//! Post-turn memory retain: heuristic + optional small-LLM extract.
//!
//! Heuristic retain is cheap (sync). LLM extract can take many seconds and must
//! **not** block turn completion — otherwise the TUI stays on `generating`
//! after the assistant text is already fully streamed (same class of bug as
//! awaiting title refine).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use whycode_core::types::{LlmRequest, Message, MessageContent, Role};
use whycode_llm::provider::{LlmProvider, ProviderRegistry};
use whycode_memory::{MemoryService, MemorySettings, llm_retain_prompt};
use whycode_session::session::Session;

use crate::events::{EventSink, TurnEvent, emit};
use crate::title::resolve_title_model;

/// Snapshot of session fields needed after the turn returns (no `&Session`).
#[derive(Debug, Clone)]
pub struct RetainSnapshot {
    pub project_path: PathBuf,
    pub session_id: String,
    pub turn_index: usize,
    pub user_text: String,
    pub assistant_text: String,
}

impl RetainSnapshot {
    pub fn from_session(session: &Session, assistant_text: &str) -> Self {
        let user_text = session
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .and_then(|m| m.content.as_text().map(|s| s.to_string()))
            .unwrap_or_default();
        Self {
            project_path: session.project_path.clone(),
            session_id: session.id.clone(),
            turn_index: session.user_message_count().max(1),
            user_text,
            assistant_text: assistant_text.to_string(),
        }
    }
}

/// Run heuristic (+ optional LLM) retain after a successful turn.
///
/// Prefer [`spawn_post_turn_retain`] from interactive UIs so the agent turn
/// can return immediately. This full await is for headless / tests.
///
/// Returns saved fact strings (for UI toast/status). Failures are logged only.
#[allow(clippy::too_many_arguments)]
pub async fn run_post_turn_retain(
    session: &Session,
    assistant_text: &str,
    settings: &MemorySettings,
    provider: &dyn LlmProvider,
    provider_name: &str,
    model: &str,
    api_key: &str,
    data_dir: &Path,
) -> Vec<String> {
    if !settings.enabled || !settings.auto_retain {
        return Vec::new();
    }

    let snap = RetainSnapshot::from_session(session, assistant_text);
    let mut saved = run_heuristic_retain(&snap, settings, data_dir);

    if should_llm_retain(settings, saved.len(), snap.turn_index)
        && let Ok(more) = run_llm_retain_facts(
            &snap,
            settings,
            provider,
            provider_name,
            model,
            api_key,
            data_dir,
        )
        .await
    {
        merge_facts(&mut saved, more);
    }

    saved
}

/// Fire-and-forget post-turn retain (heuristic sync-in-task + optional LLM).
///
/// Never blocks the agent turn. Emits `TurnEvent::Status` when facts are saved
/// so the TUI can show a quiet status/toast after `Idle`.
#[allow(clippy::too_many_arguments)]
pub fn spawn_post_turn_retain(
    session: &Session,
    assistant_text: &str,
    settings: &MemorySettings,
    registry: Arc<ProviderRegistry>,
    provider_name: &str,
    model: &str,
    api_key: &str,
    events: Option<EventSink>,
) {
    if !settings.enabled {
        return;
    }
    if !settings.auto_retain && !settings.session_inject {
        return;
    }

    let snap = RetainSnapshot::from_session(session, assistant_text);
    let settings = settings.clone();
    let provider_name = provider_name.to_string();
    let model = model.to_string();
    let api_key = api_key.to_string();
    let data_dir = whycode_core::paths::data_dir();

    tokio::spawn(async move {
        let mut saved = Vec::new();
        if settings.auto_retain {
            saved = run_heuristic_retain(&snap, &settings, &data_dir);

            if should_llm_retain(&settings, saved.len(), snap.turn_index)
                && let Some(provider) = registry.get(&provider_name)
            {
                match run_llm_retain_facts(
                    &snap,
                    &settings,
                    provider,
                    &provider_name,
                    &model,
                    &api_key,
                    &data_dir,
                )
                .await
                {
                    Ok(more) => merge_facts(&mut saved, more),
                    Err(e) => tracing::debug!("llm retain skipped: {e}"),
                }
            }
        }

        if let Ok(svc) = MemoryService::open(&snap.project_path, &data_dir, settings.clone()) {
            if let Err(e) = svc.index_session_turn(
                &snap.session_id,
                snap.turn_index,
                &snap.user_text,
                &snap.assistant_text,
            ) {
                tracing::debug!("session chunk skip: {e}");
            }
            if let Err(e) = svc.consolidate() {
                tracing::debug!("memory consolidate skip: {e}");
            }
        }

        if !saved.is_empty() {
            tracing::info!(count = saved.len(), "auto-retained memories");
            emit(
                &events,
                TurnEvent::Status(format!("Remembered {} durable fact(s)", saved.len())),
            );
        }
    });
}

fn should_llm_retain(settings: &MemorySettings, heuristic_saved: usize, turn_index: usize) -> bool {
    if !settings.enabled || !settings.auto_retain || !settings.retain_llm {
        return false;
    }
    let every = settings.retain_every_n.max(1);
    if turn_index > 0 && !turn_index.is_multiple_of(every) {
        return false;
    }
    settings.retain_llm_always || heuristic_saved == 0
}

fn run_heuristic_retain(
    snap: &RetainSnapshot,
    settings: &MemorySettings,
    data_dir: &Path,
) -> Vec<String> {
    let Ok(svc) = MemoryService::open(&snap.project_path, data_dir, settings.clone()) else {
        return Vec::new();
    };
    svc.auto_retain(
        &snap.user_text,
        Some(&snap.assistant_text),
        Some(&snap.session_id),
        snap.turn_index,
    )
    .unwrap_or_default()
}

async fn run_llm_retain_facts(
    snap: &RetainSnapshot,
    settings: &MemorySettings,
    provider: &dyn LlmProvider,
    provider_name: &str,
    model: &str,
    api_key: &str,
    data_dir: &Path,
) -> whycode_core::Result<Vec<String>> {
    let raw = llm_extract_facts(
        provider,
        provider_name,
        model,
        api_key,
        &snap.user_text,
        &snap.assistant_text,
    )
    .await?;
    let Ok(svc) = MemoryService::open(&snap.project_path, data_dir, settings.clone()) else {
        return Ok(Vec::new());
    };
    Ok(svc
        .retain_llm_facts(&raw, Some(&snap.session_id))
        .unwrap_or_default())
}

fn merge_facts(saved: &mut Vec<String>, more: Vec<String>) {
    for f in more {
        if !saved.iter().any(|s| s.eq_ignore_ascii_case(&f)) {
            saved.push(f);
        }
    }
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
        system: "You extract durable coding-project facts only. No secrets. If none: NONE.".into(),
        messages: std::sync::Arc::from(vec![Message {
            role: Role::User,
            content: MessageContent::Text(prompt),
            tool_call_id: None,
            name: None,
            created_at: None,
        }]),
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

#[cfg(test)]
mod tests {
    use super::*;
    use whycode_core::types::ContentBlock;
    use whycode_memory::MemorySettings;

    fn make_session() -> Session {
        let mut s = Session::new(PathBuf::from("/work/proj"), "sys".into());
        s.add_user_message("fix the auth bug");
        s.add_assistant_message(vec![ContentBlock::Text {
            text: "I patched it".into(),
        }]);
        s.add_user_message("also add retries");
        s.add_assistant_message(vec![ContentBlock::Text {
            text: "done, plus retries".into(),
        }]);
        s
    }

    #[test]
    fn snapshot_captures_last_user_turn() {
        let session = make_session();
        let snap = RetainSnapshot::from_session(&session, "final answer");
        assert_eq!(snap.project_path, PathBuf::from("/work/proj"));
        assert_eq!(snap.session_id, session.id);
        assert_eq!(snap.user_text, "also add retries");
        assert_eq!(snap.assistant_text, "final answer");
        assert_eq!(snap.turn_index, 2);
    }

    #[test]
    fn snapshot_empty_user_text_when_no_user_message() {
        let session = Session::new(PathBuf::from("/work/proj"), "sys".into());
        let snap = RetainSnapshot::from_session(&session, "hi");
        assert_eq!(snap.user_text, "");
        assert_eq!(snap.turn_index, 1, "turn_index floors at 1");
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn llm_retain_gate_respects_settings() {
        let mut s = MemorySettings::default();
        // Disabled → never.
        s.enabled = false;
        assert!(!should_llm_retain(&s, 0, 1));
        s.enabled = true;
        // auto_retain off → never.
        s.auto_retain = false;
        assert!(!should_llm_retain(&s, 0, 1));
        s.auto_retain = true;
        // retain_llm off → never.
        s.retain_llm = false;
        assert!(!should_llm_retain(&s, 0, 1));
        s.retain_llm = true;
        // Every-N: turn 1 with N=2 skips, turn 2 runs.
        s.retain_every_n = 2;
        assert!(!should_llm_retain(&s, 0, 1));
        assert!(should_llm_retain(&s, 0, 2));
        // Heuristic found facts and always=false → skip.
        s.retain_every_n = 1;
        assert!(!should_llm_retain(&s, 3, 1));
        // retain_llm_always overrides heuristic.
        s.retain_llm_always = true;
        assert!(should_llm_retain(&s, 3, 1));
    }

    #[test]
    fn merge_facts_dedupes_case_insensitively() {
        let mut saved = vec!["Fix auth".to_string()];
        merge_facts(
            &mut saved,
            vec!["fix AUTH".to_string(), "new fact".to_string()],
        );
        assert_eq!(saved, vec!["Fix auth", "new fact"]);
    }

    #[tokio::test]
    async fn post_turn_retain_skips_when_disabled() {
        let session = make_session();
        let settings = MemorySettings::disabled();
        let registry = whycode_llm::ProviderRegistry::default();
        let provider = registry.get("anthropic").expect("built-in provider");
        let saved = run_post_turn_retain(
            &session,
            "answer",
            &settings,
            provider,
            "anthropic",
            "claude-sonnet-4-5",
            "",
            std::path::Path::new("/tmp"),
        )
        .await;
        assert!(saved.is_empty());
    }

    #[test]
    fn spawn_retain_is_noop_when_disabled() {
        let session = make_session();
        let settings = MemorySettings::disabled();
        let registry = Arc::new(whycode_llm::ProviderRegistry::default());
        // Must not panic or spawn anything — just returns early.
        spawn_post_turn_retain(
            &session,
            "answer",
            &settings,
            registry,
            "anthropic",
            "m",
            "k",
            None,
        );
    }

    #[test]
    fn spawn_retain_skips_when_no_retain_and_no_inject() {
        let session = make_session();
        let settings = MemorySettings {
            enabled: true,
            auto_retain: false,
            session_inject: false,
            ..MemorySettings::default()
        };
        let registry = Arc::new(whycode_llm::ProviderRegistry::default());
        spawn_post_turn_retain(
            &session,
            "answer",
            &settings,
            registry,
            "anthropic",
            "m",
            "k",
            None,
        );
    }
}
