//! Full-replace session compaction.

use std::sync::Arc;

use whycodes_core::types::ContentBlock;
use whycodes_llm::provider::ProviderRegistry;
use whycodes_session::session::Session;

use super::Agent;

/// Structured compact prompt (full-replace), with optional `/compact` note.
fn build_compact_summary_prompt(transcript: &str, user_context: Option<&str>) -> String {
    let user_context_section = match user_context.map(str::trim).filter(|s| !s.is_empty()) {
        Some(context) => format!(
            "\n\n**User-provided context for this compaction:**\n{context}\n\n\
             Please incorporate this context into your summary, ensuring it is \
             prominently addressed in the relevant sections.\n"
        ),
        None => String::new(),
    };
    format!(
        "Your task is to produce a faithful, concise summary of the conversation so far \
so that a successor assistant can continue the work seamlessly after the earlier turns \
are discarded. The successor will see the user's original query plus this summary. \
Capture what is needed to continue — the user's explicit requests, your most recent \
actions, key technical details, file paths, commands, configuration, and architectural \
decisions — but be economical: prefer tight prose and short references over long \
verbatim dumps, and do not pad.
{user_context_section}
CRITICAL: If earlier turns include a prior compaction summary (marked with a \
\"This session is being continued\" preamble or \"[Compacted\" stub), treat it as \
authoritative for the early history and carry its still-relevant information forward.

Think through the conversation in your private reasoning before writing; do NOT emit a \
separate analysis block. Output the final summary inside a single <summary>...</summary> \
block, organized into the following numbered sections. Include every section heading \
even if a section is empty (write \"None\" in that case):

1. Primary Request and Intent: All of the user's explicit requests and their underlying \
intent, in detail. Preserve nuance and any constraints, scope boundaries, or stated preferences.
2. Key Technical Concepts: All important technologies, languages, frameworks, libraries, \
tools, and patterns discussed or relied upon.
3. Files and Code Sections: Every file examined, created, or modified. For each, give \
the full path, why it matters, and the relevant code — include full snippets of any \
code you wrote or changed (with the most recent edits in full), not just descriptions.
4. Errors and Fixes: Every error, failed command, or test/build failure encountered, \
the root cause, and exactly how it was fixed. Note any fix that came from user feedback verbatim.
5. Problem Solving: Problems already solved and any in-progress diagnosis or troubleshooting.
6. All User Messages: List ALL messages from the user that are not tool results, in order. \
Do NOT include this summarization instruction itself.
7. Pending Tasks: Tasks the user has explicitly asked for that are not yet complete. \
Do not invent tasks the user never requested.
8. Current Work: Precisely what you were doing immediately before this summary request.
9. Optional Next Step: The single next step that directly continues the most recent work.

IMPORTANT: Do NOT call or use any tools. Respond with ONLY the <summary>...</summary> \
block as your text output.

Conversation:
{transcript}"
    )
}

/// Share of `compaction_threshold` at which a summary of the current prefix
/// is prepared in the background, so the compaction at the threshold does not
/// wait for a model call.
pub(crate) const SPECULATIVE_LEAD_PERCENT: usize = 85;

/// A background summary of the messages before the last user turn.
pub(crate) struct SpeculativeSummary {
    session_id: String,
    prefix_end: usize,
    handle: tokio::task::JoinHandle<Option<String>>,
}

impl Agent {
    /// Start summarizing the prefix in the background. Returns whether a job
    /// was started: not when compaction is local-only, when a job for this
    /// prefix already exists, or when there is no prefix.
    pub(crate) fn maybe_spawn_speculative_summary(
        &self,
        session: &Session,
        provider_name: &str,
        model: &str,
        api_key: &str,
    ) -> bool {
        if !self.compaction_llm || api_key.is_empty() {
            return false;
        }
        let Some((prefix_end, transcript)) = session.prefix_transcript_for_summary() else {
            return false;
        };
        let mut slot = super::recover_lock(&self.speculative_summary);
        if let Some(job) = slot.as_ref()
            && job.session_id == session.id
            && job.prefix_end == prefix_end
        {
            return false;
        }
        if let Some(stale) = slot.take() {
            stale.handle.abort();
        }
        let registry = Arc::clone(&self.provider_registry);
        let redactor = self.secrets.clone();
        let provider_name = provider_name.to_string();
        let model = model.to_string();
        let api_key = api_key.to_string();
        let handle = tokio::spawn(async move {
            summarize_with_model(
                &registry,
                redactor,
                &transcript,
                &provider_name,
                &model,
                &api_key,
                None,
            )
            .await
        });
        *slot = Some(SpeculativeSummary {
            session_id: session.id.clone(),
            prefix_end,
            handle,
        });
        true
    }

    /// The prepared summary when it still describes the session's prefix.
    /// Consumes the job; a stale one (other session, or the prefix moved) is
    /// aborted and `None` is returned.
    pub(crate) async fn take_speculative_summary(&self, session: &Session) -> Option<String> {
        let job = super::recover_lock(&self.speculative_summary).take()?;
        let fresh =
            job.session_id == session.id && session.compaction_prefix_end() == Some(job.prefix_end);
        if !fresh {
            job.handle.abort();
            return None;
        }
        match job.handle.await {
            Ok(Some(summary)) if !summary.trim().is_empty() => Some(summary),
            Ok(_) => None,
            Err(err) => {
                tracing::debug!(error = %err, "speculative summary task did not finish");
                None
            }
        }
    }

    /// [`Self::compact_session`] with a summary prepared earlier: the same
    /// cheap passes and `pre_compact` hooks, no model call.
    pub(crate) async fn compact_with_summary(
        &self,
        session: &mut Session,
        summary: &str,
    ) -> whycodes_session::CompactOutcome {
        session.truncate_large_tool_results();
        session.spill_old_tool_results(&mut super::spill::artifact_store(
            session.project_path.clone(),
        ));
        session.prune_old_tool_results();
        if session.messages.is_empty() {
            return whycodes_session::CompactOutcome::default();
        }
        self.run_pre_compact_hooks(session).await;
        session.apply_full_replace(summary)
    }

    /// Full-replace compact: summarize the whole conversation,
    /// keep the last real user query + current-turn tail, replace the rest
    /// with a continuation carrier.
    ///
    /// Manual `/compact [context]` always runs this (no token threshold).
    /// Auto-compact uses the same path when over `compaction_threshold`.
    /// Falls back to a local stub when LLM is off, the key is missing, or
    /// sampling fails.
    pub async fn compact_session(
        &self,
        session: &mut Session,
        provider_name: &str,
        model: &str,
        api_key: &str,
        user_context: Option<&str>,
    ) -> whycodes_session::CompactOutcome {
        session.truncate_large_tool_results();
        session.spill_old_tool_results(&mut super::spill::artifact_store(
            session.project_path.clone(),
        ));
        session.prune_old_tool_results();
        if session.messages.is_empty() {
            return whycodes_session::CompactOutcome::default();
        }
        self.run_pre_compact_hooks(session).await;

        let transcript = session.transcript_for_full_summary(0);
        let local = session.local_full_replace_summary();
        let want_llm = self.compaction_llm && !api_key.is_empty() && !transcript.trim().is_empty();
        let summary = if want_llm {
            summarize_with_model(
                &self.provider_registry,
                self.secrets.clone(),
                &transcript,
                provider_name,
                model,
                api_key,
                user_context,
            )
            .await
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(local)
        } else {
            local
        };
        session.apply_full_replace(&summary)
    }
}

/// Structured summarizer used by full-replace compact (session model). Free
/// of `Agent` so a speculative job can run it from a background task.
async fn summarize_with_model(
    registry: &ProviderRegistry,
    redactor: Option<Arc<whycodes_llm::Redactor>>,
    transcript: &str,
    provider_name: &str,
    model: &str,
    api_key: &str,
    user_context: Option<&str>,
) -> Option<String> {
    {
        if transcript.trim().is_empty() {
            return None;
        }
        let provider = registry.get(provider_name)?;
        use whycodes_core::types::{LlmRequest, Message, MessageContent, Role};
        let request = LlmRequest {
            system: String::new(),
            messages: std::sync::Arc::from(vec![Message {
                role: Role::User,
                content: MessageContent::Text(build_compact_summary_prompt(
                    transcript,
                    user_context,
                )),
                tool_call_id: None,
                name: None,
                created_at: None,
            }]),
            tools: std::sync::Arc::from([]),
            max_tokens: Some(4_096),
            temperature: Some(0.2),
            top_p: None,
            top_k: None,
            stop_sequences: None,
            thinking: None,
            use_prompt_cache: false,
        };
        let transport = whycodes_llm::LlmTransport {
            redactor,
            complete_timeout: Some(std::time::Duration::from_secs(60)),
            retry: whycodes_llm::RetryPolicy {
                max_retries: 2,
                initial_backoff: std::time::Duration::from_millis(200),
                max_backoff: std::time::Duration::from_secs(3),
                max_elapsed: std::time::Duration::from_secs(90),
                full_jitter: true,
            },
        };
        match transport.complete(provider, &request, api_key, model).await {
            Ok(resp) => {
                let text = resp
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
                    .trim()
                    .to_string();
                if text.is_empty() { None } else { Some(text) }
            }
            Err(e) => {
                tracing::warn!(error = %e, "LLM compact summary failed");
                None
            }
        }
    }
}

#[cfg(test)]
#[path = "compact_tests.rs"]
mod tests;
