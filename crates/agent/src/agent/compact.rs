//! Grok-style full-replace session compaction.

use whycodes_core::types::ContentBlock;
use whycodes_session::session::Session;

use super::Agent;

/// Structured compact prompt (Grok full-replace), with optional `/compact` note.
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

impl Agent {
    /// Grok-style full-replace compact: summarize the whole conversation,
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
        session.prune_old_tool_results();
        if session.messages.is_empty() {
            return whycodes_session::CompactOutcome::default();
        }

        let transcript = session.transcript_for_full_summary(0);
        let local = session.local_full_replace_summary();
        let want_llm = self.compaction_llm && !api_key.is_empty() && !transcript.trim().is_empty();
        let summary = if want_llm {
            self.llm_compact_summary(&transcript, provider_name, model, api_key, user_context)
                .await
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(local)
        } else {
            local
        };
        session.apply_full_replace(&summary)
    }

    /// Structured summarizer used by full-replace compact (session model).
    async fn llm_compact_summary(
        &self,
        transcript: &str,
        provider_name: &str,
        model: &str,
        api_key: &str,
        user_context: Option<&str>,
    ) -> Option<String> {
        if transcript.trim().is_empty() {
            return None;
        }
        let provider = self.provider_registry.get(provider_name)?;
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
mod tests {
    use super::*;

    #[test]
    fn compact_prompt_includes_transcript_and_optional_context() {
        let bare = build_compact_summary_prompt("hello transcript", None);
        assert!(bare.contains("hello transcript"));
        assert!(!bare.contains("User-provided context"));
        let with = build_compact_summary_prompt("hello transcript", Some(" keep auth.rs "));
        assert!(with.contains("keep auth.rs"));
        assert!(with.contains("<summary>"));
        let empty_ctx = build_compact_summary_prompt("t", Some("   "));
        assert!(!empty_ctx.contains("User-provided context"));
    }
}
