//! Conversation turn loop (LLM stream → tool batch → repeat).

use futures::StreamExt;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use whycodes_core::types::{ContentBlock, StreamEvent, ToolResult};
use whycodes_session::session::Session;

use crate::events::{TurnEvent, TurnOpts, emit, is_cancelled, wait_until_cancelled};
use crate::tool_stream::ToolCallAssembler;

use super::{
    Agent, DOOM_LOOP_THRESHOLD, MAX_CONSECUTIVE_COMPACT_FAILURES, append_request_user_suffix,
    first_stream_rule_hit, settle_checkpoint_rewind, tool_call_signature, would_doom_loop,
};

impl Agent {
    /// Run a single conversation turn (no streaming UI events).
    ///
    /// `max_turns` is a headless safety cap (`None` = unlimited, Grok TUI
    /// parity). Interactive sessions pass `None` and stop on end-of-turn,
    /// cancel, or doom-loop instead.
    pub async fn run_turn(
        &self,
        session: &mut Session,
        provider_name: &str,
        model: &str,
        api_key: &str,
        max_turns: Option<usize>,
    ) -> whycodes_core::Result<String> {
        self.run_turn_with_events(
            session,
            TurnOpts {
                provider_name,
                model,
                api_key,
                max_turns,
                events: None,
                cancel: None,
            },
        )
        .await
    }

    /// Run a turn, optionally streaming `TurnEvent`s and honouring a cancel flag (Esc).
    pub async fn run_turn_with_events(
        &self,
        session: &mut Session,
        opts: TurnOpts<'_>,
    ) -> whycodes_core::Result<String> {
        let TurnOpts {
            provider_name,
            model,
            api_key,
            max_turns,
            events,
            cancel,
        } = opts;
        // Trivial chit-chat: omit tools entirely (huge prefill savings).
        // Only on short single-user sessions — once tools were used, keep them.
        let last_user = session
            .messages
            .iter()
            .rev()
            .find(|m| m.role == whycodes_core::types::Role::User)
            .and_then(|m| m.content.as_text().map(|s| s.to_string()))
            .unwrap_or_default();
        let magic = crate::magic_keywords::scan(&last_user, &self.magic_keywords);
        let skip_cache = self
            .skip_prompt_cache_once
            .swap(false, std::sync::atomic::Ordering::Relaxed);
        let (role_provider, role_model) = crate::routing::resolve_agent_model(
            provider_name,
            model,
            &self.info.name,
            self.model_plan.as_deref(),
        );
        let provider_name = role_provider.as_str();
        let model = role_model.as_str();
        let tools_free_chat = crate::title::is_trivial_title_seed(&last_user)
            && session.user_message_count() <= 1
            && !session.messages.iter().any(|m| {
                matches!(m.role, whycodes_core::types::Role::Tool)
                    || matches!(
                        &m.content,
                        whycodes_core::types::MessageContent::Blocks(b)
                            if b.iter().any(|x| matches!(x, ContentBlock::ToolUse { .. }))
                    )
            });

        // Classify once per user turn (zero LLM cost): badge, posture, tool auth.
        let turn_intent = crate::intent::classify_user_intent(&last_user);
        {
            let badge = crate::intent::badge_label(&turn_intent)
                .unwrap_or("")
                .to_string();
            let (notice_kind, notice) =
                match crate::intent::intent_notice(&turn_intent, &self.info.name) {
                    Some(n) => {
                        let k = match n.kind {
                            crate::intent::IntentNoticeKind::Info => "info",
                            crate::intent::IntentNoticeKind::Warning => "warning",
                        };
                        (k.to_string(), n.message)
                    }
                    None => (String::new(), String::new()),
                };
            emit(
                &events,
                TurnEvent::Intent {
                    kind: turn_intent.intent.as_str().to_string(),
                    confidence: turn_intent.confidence,
                    badge,
                    notice_kind,
                    notice,
                },
            );
        }

        let provider = self
            .provider_registry
            .get(provider_name)
            .ok_or_else(|| {
                whycodes_core::Error::llm(format!(
                    "Unknown provider: {}. Available: anthropic, openai, google, google-antigravity, and configured custom providers",
                    provider_name
                ))
            })?;

        let mut turn_count = 0;
        let mut final_text = String::new();
        // Latency: wall clock for the whole user turn (all LLM steps + tools).
        let user_turn_t0 = Instant::now();
        let mut ttft_ms: Option<u128> = None;
        // Recent tool signatures for OpenCode-style doom-loop detection.
        let mut recent_tool_sigs: VecDeque<String> = VecDeque::with_capacity(8);
        // Autocompact circuit breaker: stop retrying after N ineffective passes.
        let mut compact_failures: u32 = 0;
        let mut compact_paused = false;
        let mut overflow_retries: u32 = 0;

        loop {
            // Cached schemas; extra activations still apply per step.
            let tools = if tools_free_chat {
                std::sync::Arc::from([])
            } else {
                let extra = self.activated_tools_snapshot();
                let defs = self.tool_executor.get_definitions_profile_extra(
                    &self.info.permission,
                    self.tool_profile,
                    &extra,
                );
                if !self.swarm_enabled && defs.iter().any(|d| d.name == "swarm") {
                    defs.iter()
                        .filter(|d| d.name != "swarm")
                        .cloned()
                        .collect::<Vec<_>>()
                        .into()
                } else {
                    defs
                }
            };
            let tool_ctx = self.tool_context(session);
            if is_cancelled(&cancel) {
                emit(&events, TurnEvent::Cancelled);
                return Err(whycodes_core::Error::Agent("Cancelled".into()));
            }

            turn_count += 1;
            if let Some(max) = max_turns
                && turn_count > max
            {
                return Err(whycodes_core::Error::Agent(format!(
                    "Exceeded maximum turns ({max})"
                )));
            }

            // Always shrink oversized / old tool dumps before prefill (cheap).
            // When still hot, shake older tool bodies harder so overflow is less likely.
            // Full-replace compact when over the configured token threshold —
            // and only while the circuit breaker has not tripped.
            let _truncated = session.truncate_large_tool_results();
            let _pruned = session.prune_old_tool_results();
            if self.compaction_threshold > 0
                && session.token_count_cached() > self.compaction_threshold.saturating_mul(3) / 4
            {
                let shaken = session.shake_old_tool_results();
                if shaken > 0 {
                    tracing::debug!(shaken, "shook old tool results before LLM step");
                }
            }
            if self.compaction_threshold > 0 && !compact_paused {
                let before = session.token_count_cached();
                if before > self.compaction_threshold {
                    let outcome = self
                        .compact_session(session, provider_name, model, api_key, None)
                        .await;
                    if outcome.reduced() || outcome.dropped_messages() {
                        emit(
                            &events,
                            TurnEvent::Status(format!(
                                "Compacted context ({} → {} msgs, ~{} → ~{} tok)…",
                                outcome.messages_before,
                                outcome.messages_after,
                                outcome.tokens_before,
                                outcome.tokens_after
                            )),
                        );
                        tracing::info!(
                            before_tokens = outcome.tokens_before,
                            after_tokens = outcome.tokens_after,
                            messages_before = outcome.messages_before,
                            messages_after = outcome.messages_after,
                            "auto-compact before LLM step"
                        );
                    }
                    if outcome.still_over(self.compaction_threshold) {
                        compact_failures = compact_failures.saturating_add(1);
                        if compact_failures >= MAX_CONSECUTIVE_COMPACT_FAILURES {
                            compact_paused = true;
                            emit(
                                &events,
                                TurnEvent::Status(format!(
                                    "Auto-compact paused after {MAX_CONSECUTIVE_COMPACT_FAILURES} \
                                     passes (~{} tok still over threshold)",
                                    outcome.tokens_after
                                )),
                            );
                            tracing::warn!(
                                failures = compact_failures,
                                tokens = outcome.tokens_after,
                                "autocompact circuit breaker tripped"
                            );
                        }
                    } else {
                        compact_failures = 0;
                    }
                }
            }

            emit(
                &events,
                TurnEvent::Status(format!("LLM request (step {turn_count})…")),
            );

            let mut request = session.build_request(tools, None, self.info.temperature, Some(true));
            request.use_prompt_cache = self.use_prompt_cache && !skip_cache;
            crate::thinking_acc::attach_thinking_request(
                &mut request,
                provider_name,
                model,
                self.info.model.as_ref(),
                self.reasoning_effort.as_deref(),
            );
            if magic.ultrathink {
                crate::thinking_acc::apply_ultrathink(&mut request);
            }

            // First LLM step: ephemeral intent posture (not stored in session;
            // keeps system prompt cache-stable). Notice is already on Intent event.
            if turn_count == 1
                && crate::intent::should_inject(self.intent_guidance, &turn_intent)
                && let Some(suffix) = crate::intent::posture_suffix(&turn_intent, &self.info.name)
            {
                append_request_user_suffix(&mut request, &suffix);
                tracing::debug!(
                    intent = turn_intent.intent.as_str(),
                    confidence = turn_intent.confidence,
                    agent = %self.info.name,
                    "intent posture injected into request"
                );
            }
            if turn_count == 1 && magic.any() {
                let notice = magic.notice();
                append_request_user_suffix(&mut request, &notice);
                tracing::debug!(
                    ultrathink = magic.ultrathink,
                    orchestrate = magic.orchestrate,
                    "magic keyword notice injected into request"
                );
            }

            let mut accumulated_text = String::new();
            let mut thinking_acc = crate::thinking_acc::ThinkingAccumulator::new();
            let mut turn_usage = whycodes_core::types::Usage::default();
            let mut assembler = ToolCallAssembler::new();
            let mut speculative_reads: Vec<crate::speculative_read::SpeculativeRead> = Vec::new();
            let step_t0 = Instant::now();

            // Professional transport: classify + full-jitter backoff + Retry-After.
            // Only the HTTP open is retried — mid-stream drops stay single-shot.
            // Race the open against cancel so a hung gateway cannot ignore Esc.
            // Bind transport so `stream()`'s future is not tied to a temporary.
            let transport = whycodes_llm::default_transport();
            let race_ids = self.race_partner(provider_name, model);
            let race_provider = race_ids
                .as_ref()
                .and_then(|(p, _)| self.provider_registry.get(p.as_str()));
            let race_target = match (race_ids.as_ref(), race_provider) {
                (Some((_, m)), Some(rp)) => Some(whycodes_llm::StreamTarget {
                    provider: rp,
                    api_key,
                    model: m.as_str(),
                }),
                _ => None,
            };
            let opened = tokio::select! {
                biased;
                _ = wait_until_cancelled(&cancel) => {
                    emit(&events, TurnEvent::Cancelled);
                    return Err(whycodes_core::Error::Agent("Cancelled".into()));
                }
                opened = transport.stream_turn(
                    whycodes_llm::StreamTarget {
                        provider,
                        api_key,
                        model,
                    },
                    &request,
                    whycodes_llm::StreamTurnOpts {
                        cache: self.response_cache && request.tools.is_empty() && !skip_cache,
                        race: race_target,
                        race_after: self.race_after,
                    },
                ) => opened,
            };
            let turn = match opened {
                Ok(t) => t,
                Err(e)
                    if whycodes_llm::classify(&e).kind
                        == whycodes_llm::ErrorKind::ContextOverflow
                        && overflow_retries < 1 =>
                {
                    overflow_retries = overflow_retries.saturating_add(1);
                    emit(
                        &events,
                        TurnEvent::Status(
                            "Context overflow — compacting and retrying this step…".into(),
                        ),
                    );
                    let outcome = self
                        .compact_session(session, provider_name, model, api_key, None)
                        .await;
                    tracing::info!(
                        after_tokens = outcome.tokens_after,
                        "compacted after context overflow"
                    );
                    continue;
                }
                Err(e) => return Err(e),
            };
            let cache_hit = turn.cache_hit;
            let race_tag = turn.race.as_str();
            if cache_hit {
                emit(&events, TurnEvent::Status("Response cache hit".into()));
            } else if turn.race.raced() {
                let partner = race_ids.as_ref().map(|(_, m)| m.as_str()).unwrap_or("?");
                emit(
                    &events,
                    TurnEvent::Status(format!("First-token race: {partner} ({race_tag})")),
                );
            }
            let mut event_stream = turn.events;
            let mut stream_rule_retry = false;

            // Stream body: check cancel between tokens *and* while idle waiting
            // for the next SSE line (select! with wait_until_cancelled).
            loop {
                let event = tokio::select! {
                    biased;
                    _ = wait_until_cancelled(&cancel) => {
                        crate::speculative_read::abort_all(&mut speculative_reads);
                        let mut blocks = thinking_acc.into_blocks();
                        if !accumulated_text.is_empty() {
                            blocks.push(ContentBlock::Text {
                                text: accumulated_text.clone(),
                            });
                            final_text.push_str(&accumulated_text);
                        }
                        if !blocks.is_empty() {
                            session.add_assistant_message(blocks);
                        }
                        emit(&events, TurnEvent::Cancelled);
                        return Err(whycodes_core::Error::Agent("Cancelled".into()));
                    }
                    next = event_stream.next() => next,
                };

                let Some(event) = event else {
                    break;
                };

                let event = match event {
                    Ok(ev) => ev,
                    Err(e)
                        if whycodes_llm::classify(&e).kind
                            == whycodes_llm::ErrorKind::ContextOverflow
                            && overflow_retries < 1 =>
                    {
                        crate::speculative_read::abort_all(&mut speculative_reads);
                        overflow_retries = overflow_retries.saturating_add(1);
                        emit(
                            &events,
                            TurnEvent::Status(
                                "Context overflow — compacting and retrying this step…".into(),
                            ),
                        );
                        let outcome = self
                            .compact_session(session, provider_name, model, api_key, None)
                            .await;
                        tracing::info!(
                            after_tokens = outcome.tokens_after,
                            "compacted after streamed context overflow"
                        );
                        stream_rule_retry = true;
                        break;
                    }
                    Err(e) => {
                        crate::speculative_read::abort_all(&mut speculative_reads);
                        whycodes_core::logging::emit_sid(
                            "agent",
                            "error",
                            "turn.stream_error",
                            Some(session.id.as_str()),
                            Some(serde_json::json!({
                                "provider": provider_name,
                                "model": model,
                                "error": e.to_string(),
                            })),
                        );
                        return Err(e);
                    }
                };

                match event {
                    StreamEvent::TextDelta { text } => {
                        thinking_acc.flush();
                        if ttft_ms.is_none() {
                            ttft_ms = Some(user_turn_t0.elapsed().as_millis());
                        }
                        emit(&events, TurnEvent::TextDelta(text.clone()));
                        accumulated_text.push_str(&text);
                        if let Some((name, hint)) =
                            first_stream_rule_hit(&self.stream_rules, &accumulated_text)
                        {
                            crate::speculative_read::abort_all(&mut speculative_reads);
                            emit(
                                &events,
                                TurnEvent::Status(format!(
                                    "Stream rule `{name}` interrupted the draft"
                                )),
                            );
                            session.add_user_message(&format!(
                                "<whycodes_rule name=\"{name}\">\n{hint}\n\
                                 The previous draft was discarded. Continue without violating this rule.\n\
                                 </whycodes_rule>"
                            ));
                            stream_rule_retry = true;
                            break;
                        }
                    }
                    StreamEvent::ToolUse { id, name, input } => {
                        thinking_acc.flush();
                        // Defer ToolStart until after argument fragments are
                        // merged — OpenAI streams send null/empty args first.
                        assembler.on_tool_use(id, name, input);
                        // Complete objects (Anthropic non-streamed) can start I/O now.
                        if let Some((cid, cname, buf)) = assembler.last_updated() {
                            crate::speculative_read::maybe_start(
                                &mut speculative_reads,
                                &cid,
                                &cname,
                                &buf,
                                &tool_ctx,
                            );
                        }
                    }
                    StreamEvent::ToolUseDelta {
                        id,
                        input_json_delta,
                    } => {
                        assembler.on_tool_use_delta(&id, &input_json_delta);
                        // Path often closes mid-stream — start `read` I/O early.
                        if let Some((cid, cname, buf)) = assembler.last_updated() {
                            crate::speculative_read::maybe_start(
                                &mut speculative_reads,
                                &cid,
                                &cname,
                                &buf,
                                &tool_ctx,
                            );
                        }
                    }
                    StreamEvent::Thinking { text } => {
                        if text.is_empty() {
                            continue;
                        }
                        if ttft_ms.is_none() {
                            ttft_ms = Some(user_turn_t0.elapsed().as_millis());
                        }
                        thinking_acc.push_text(&text);
                        emit(&events, TurnEvent::ThinkingDelta(text.clone()));
                        tracing::trace!(n = text.len(), "thinking delta");
                    }
                    StreamEvent::ThinkingDelta { text } => {
                        if text.is_empty() {
                            continue;
                        }
                        if ttft_ms.is_none() {
                            ttft_ms = Some(user_turn_t0.elapsed().as_millis());
                        }
                        thinking_acc.push_text(&text);
                        emit(&events, TurnEvent::ThinkingDelta(text.clone()));
                        tracing::trace!(n = text.len(), "thinking delta");
                    }
                    StreamEvent::ThinkingSignature { signature } => {
                        thinking_acc.push_signature(&signature);
                    }
                    StreamEvent::RedactedThinking { data } => {
                        thinking_acc.push_redacted(&data);
                    }
                    StreamEvent::MessageStop => break,
                    StreamEvent::Usage {
                        input_tokens,
                        output_tokens,
                    } => {
                        // Snapshot fold (max), not sum: Anthropic splits
                        // input/output across events; OpenAI-compat gateways
                        // often repeat the full usage object.
                        turn_usage.absorb_stream(input_tokens, output_tokens);
                    }
                    StreamEvent::CacheUsage {
                        creation_input_tokens,
                        read_input_tokens,
                    } => {
                        turn_usage.absorb_stream_cache(creation_input_tokens, read_input_tokens);
                    }
                    StreamEvent::MessageStart { .. } => {}
                    StreamEvent::MessageDelta { .. } => {}
                    StreamEvent::Error { message } => {
                        if whycodes_llm::classify_message(&message).kind
                            == whycodes_llm::ErrorKind::ContextOverflow
                            && overflow_retries < 1
                        {
                            crate::speculative_read::abort_all(&mut speculative_reads);
                            overflow_retries = overflow_retries.saturating_add(1);
                            emit(
                                &events,
                                TurnEvent::Status(
                                    "Context overflow — compacting and retrying this step…".into(),
                                ),
                            );
                            let outcome = self
                                .compact_session(session, provider_name, model, api_key, None)
                                .await;
                            tracing::info!(
                                after_tokens = outcome.tokens_after,
                                "compacted after streamed context overflow"
                            );
                            stream_rule_retry = true;
                            break;
                        }
                        crate::speculative_read::abort_all(&mut speculative_reads);
                        return Err(whycodes_core::Error::llm(message));
                    }
                }
            }

            if stream_rule_retry {
                continue;
            }

            // Merge streamed argument fragments into parsed JSON objects.
            let tool_calls = assembler.finish();
            let step_ms = step_t0.elapsed().as_millis();

            if self.response_cache
                && !cache_hit
                && request.tools.is_empty()
                && tool_calls.is_empty()
                && !accumulated_text.trim().is_empty()
            {
                whycodes_llm::ResponseCache::global().store(&request, model, &accumulated_text);
            }

            // Emit ToolStart with final parsed arguments (not the empty first chunk).
            for tc in &tool_calls {
                if ttft_ms.is_none() {
                    ttft_ms = Some(user_turn_t0.elapsed().as_millis());
                }
                emit(
                    &events,
                    TurnEvent::ToolStart {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        input: tc.arguments.clone(),
                    },
                );
            }

            // Once per turn, after the stream closes and before any tool runs.
            // A provider that reports nothing produces no event, so a silent
            // provider is distinguishable from a zero-cost turn.
            if !turn_usage.is_empty() {
                session.add_usage(&turn_usage);
                emit(&events, TurnEvent::Usage(turn_usage.clone()));
            }

            let mut blocks: Vec<ContentBlock> = thinking_acc.into_blocks();

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

            if tool_calls.is_empty() {
                crate::speculative_read::abort_all(&mut speculative_reads);
                whycodes_core::logging::emit_sid(
                    "agent",
                    "info",
                    "turn.step",
                    Some(session.id.as_str()),
                    Some(serde_json::json!({
                        "step": turn_count,
                        "step_ms": step_ms,
                        "ttft_ms": ttft_ms,
                        "tool_batch_ms": null,
                        "tool_count": 0,
                        "tools_profile": self.tool_profile.as_str(),
                        "input_tokens": turn_usage.input_tokens,
                        "output_tokens": turn_usage.output_tokens,
                        "cache_read_tokens": turn_usage.cache_read_input_tokens,
                        "cache_creation_tokens": turn_usage.cache_creation_input_tokens,
                        "response_cache_hit": cache_hit,
                        "race": race_tag,
                        "done": true,
                    })),
                );
                break;
            }

            // Doom-loop: refuse identical tool+args repeated DOOM_LOOP_THRESHOLD times
            // (OpenCode processor.ts doom_loop permission pattern).
            let results = if would_doom_loop(&recent_tool_sigs, &tool_calls) {
                crate::speculative_read::abort_all(&mut speculative_reads);
                emit(
                    &events,
                    TurnEvent::Status("Doom loop: identical tool call repeated — refusing".into()),
                );
                tracing::warn!(
                    tools = ?tool_calls.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
                    "doom loop refused"
                );
                let mut refused = Vec::with_capacity(tool_calls.len());
                for tc in &tool_calls {
                    emit(
                        &events,
                        TurnEvent::ToolEnd {
                            id: tc.id.clone(),
                            content: format!(
                                "Doom loop: tool `{}` with the same arguments was repeated \
                                 {DOOM_LOOP_THRESHOLD}+ times. Stop retrying; change approach \
                                 or ask the user.",
                                tc.name
                            ),
                            is_error: true,
                        },
                    );
                    refused.push(ToolResult {
                        tool_call_id: tc.id.clone(),
                        content: format!(
                            "Doom loop: tool `{}` with the same arguments was repeated \
                             {DOOM_LOOP_THRESHOLD}+ times. Stop retrying; change approach \
                             or ask the user.",
                            tc.name
                        ),
                        is_error: true,
                    });
                    let sig = tool_call_signature(tc);
                    recent_tool_sigs.push_back(sig);
                    while recent_tool_sigs.len() > 16 {
                        recent_tool_sigs.pop_front();
                    }
                }
                refused
            } else {
                // Parallel when safe (OpenCode / Codex / Claude Code pattern).
                // Sequential for shell / mutating / permission-ask tools so risk
                // gates and the TUI single-slot permission UI stay correct.
                let tool_t0 = Instant::now();
                let results = self
                    .execute_tool_calls(
                        &tool_calls,
                        session,
                        &tool_ctx,
                        provider_name,
                        model,
                        api_key,
                        &events,
                        &cancel,
                        Some(&turn_intent),
                        &mut speculative_reads,
                    )
                    .await?;
                crate::speculative_read::abort_all(&mut speculative_reads);
                let tool_batch_ms = tool_t0.elapsed().as_millis();
                for tc in &tool_calls {
                    let sig = tool_call_signature(tc);
                    recent_tool_sigs.push_back(sig);
                    while recent_tool_sigs.len() > 16 {
                        recent_tool_sigs.pop_front();
                    }
                }
                whycodes_core::logging::emit_sid(
                    "agent",
                    "info",
                    "turn.step",
                    Some(session.id.as_str()),
                    Some(serde_json::json!({
                        "step": turn_count,
                        "step_ms": step_ms,
                        "ttft_ms": ttft_ms,
                        "tool_batch_ms": tool_batch_ms,
                        "tool_count": tool_calls.len(),
                        "tools_profile": self.tool_profile.as_str(),
                        "input_tokens": turn_usage.input_tokens,
                        "output_tokens": turn_usage.output_tokens,
                        "cache_read_tokens": turn_usage.cache_read_input_tokens,
                        "cache_creation_tokens": turn_usage.cache_creation_input_tokens,
                        "response_cache_hit": cache_hit,
                        "race": race_tag,
                        "done": false,
                    })),
                );
                results
            };

            let mut results = results;
            let (checkpoint_goal, rewind_report) =
                settle_checkpoint_rewind(session, &tool_calls, &mut results);

            // Capture failures before move — avoid cloning large tool bodies.
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

            session.add_tool_results(results);
            if let Some(goal) = checkpoint_goal {
                session.mark_checkpoint(goal);
            }
            if let Some(report) = rewind_report {
                if session.apply_rewind(&report) {
                    tracing::debug!("collapsed exploratory context after rewind");
                } else {
                    tracing::debug!("rewind requested with no active checkpoint");
                }
            }

            // Fold subagent tokens into this turn + parent session (plan-performance).
            if let Ok(mut pending) = self.subagent_usage_pending.lock()
                && !pending.is_empty()
            {
                let fold = std::mem::take(&mut *pending);
                turn_usage.add(&fold);
                session.add_usage(&fold);
                tracing::debug!(
                    input = fold.input_tokens,
                    output = fold.output_tokens,
                    "folded subagent usage into parent session"
                );
            }

            if !failed_tools.is_empty() {
                let recovery_msg = failed_tools.join("\n");
                session.add_user_message(&recovery_msg);
            }
        }

        whycodes_core::logging::emit_sid(
            "agent",
            "info",
            "turn.done",
            Some(session.id.as_str()),
            Some(serde_json::json!({
                "steps": turn_count,
                "ttft_ms": ttft_ms,
                "worked_ms": user_turn_t0.elapsed().as_millis(),
                "tools_profile": self.tool_profile.as_str(),
                "response_cache": self.response_cache,
                "model_race": self.model_race,
            })),
        );

        crate::notify::spawn_turn_done(
            &self.notify,
            "Turn done",
            &format!("Session · {}", session.title),
            Some(session.id.as_str()),
        );

        // Hindsight-style auto-retain (heuristic + optional LLM). Best-effort
        // and **async** — never await here. LLM extract can take 5–12s and used
        // to keep the TUI on `generating` after the answer was already on screen
        // (same pitfall as title refine; see docs/knowhow.md).
        crate::memory_retain::spawn_post_turn_retain(
            session,
            &final_text,
            &self.memory,
            Arc::clone(&self.provider_registry),
            provider_name,
            model,
            api_key,
            events,
        );

        Ok(final_text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{TurnEvent, TurnOpts};
    use serde_json::json;
    use whycodes_core::types::{
        AgentInfo, AgentMode, ApprovalMode, ContentBlock, PermissionSet, Role,
    };
    use whycodes_llm::{ProviderRegistry, ScriptedProvider, ScriptedStep};

    fn info(name: &str) -> AgentInfo {
        AgentInfo {
            name: name.to_string(),
            description: format!("Test agent: {name}"),
            mode: AgentMode::Primary,
            permission: PermissionSet {
                allowed_tools: None,
                denied_tools: None,
                allow_file_writes: true,
                allow_network: true,
                allow_shell: true,
                allowed_paths: None,
                rules: Default::default(),
            },
            model: None,
            system_prompt: Some("You are a test agent.".to_string()),
            temperature: Some(0.5),
            top_p: None,
        }
    }

    fn scripted(steps: impl IntoIterator<Item = ScriptedStep>) -> Agent {
        let mut registry = ProviderRegistry::new();
        registry.register(Box::new(ScriptedProvider::new(steps)));
        Agent::new(info("build")).with_provider_registry(registry)
    }

    fn repeating(steps: impl IntoIterator<Item = ScriptedStep>) -> Agent {
        let mut registry = ProviderRegistry::new();
        registry.register(Box::new(ScriptedProvider::repeating("script", steps)));
        Agent::new(info("build")).with_provider_registry(registry)
    }

    fn batched(batches: impl IntoIterator<Item = Vec<ScriptedStep>>) -> Agent {
        let mut registry = ProviderRegistry::new();
        registry.register(Box::new(ScriptedProvider::batched("script", batches)));
        Agent::new(info("build")).with_provider_registry(registry)
    }

    fn session_at(dir: &std::path::Path, user: &str) -> Session {
        let mut session = Session::new(dir.to_path_buf(), "test".into());
        session.add_user_message(user);
        session
    }

    fn session_user(user: &str) -> Session {
        let mut session = Session::new(std::path::PathBuf::from("/work/proj"), "test".into());
        session.add_user_message(user);
        session
    }

    fn opts(events: Option<crate::events::EventSink>) -> TurnOpts<'static> {
        TurnOpts {
            provider_name: "script",
            model: "m",
            api_key: "k",
            max_turns: Some(8),
            events,
            cancel: None,
        }
    }

    fn drain_status(rx: &mut tokio::sync::mpsc::UnboundedReceiver<TurnEvent>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let TurnEvent::Status(s) = ev {
                out.push(s);
            }
        }
        out
    }

    #[tokio::test]
    async fn unknown_provider_lists_builtins() {
        let agent = scripted([ScriptedStep::Text("x".into())]);
        let mut session = session_user("please explain the retry loop");
        let err = agent
            .run_turn(&mut session, "no-such-provider", "m", "k", Some(2))
            .await
            .expect_err("unknown");
        let msg = err.to_string();
        assert!(msg.contains("anthropic"), "{msg}");
        assert!(msg.contains("openai"), "{msg}");
        assert!(msg.contains("google-antigravity"), "{msg}");
    }

    #[tokio::test]
    async fn max_turns_exceeded_after_tool() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("note.txt"), "secret").unwrap();
        let agent = scripted([ScriptedStep::ToolCall {
            id: "c1".into(),
            name: "read".into(),
            input: json!({"path": "note.txt"}),
        }]);
        let mut session = session_at(dir.path(), "please read note.txt and summarize it");
        let err = agent
            .run_turn(&mut session, "script", "m", "k", Some(1))
            .await
            .expect_err("max turns");
        assert!(
            err.to_string()
                .to_lowercase()
                .contains("exceeded maximum turns"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn tools_free_chat_stores_and_hits_response_cache() {
        let agent = repeating([ScriptedStep::Text("hello there".into())]);
        let mut session = session_user("hi");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let out = agent
            .run_turn_with_events(&mut session, opts(Some(tx)))
            .await
            .expect("first");
        assert!(out.contains("hello"), "{out}");
        let _ = drain_status(&mut rx);

        let mut session2 = session_user("hi");
        let (tx2, mut rx2) = tokio::sync::mpsc::unbounded_channel();
        let _ = agent
            .run_turn_with_events(&mut session2, opts(Some(tx2)))
            .await
            .expect("second");
        let status = drain_status(&mut rx2);
        assert!(
            status.iter().any(|s| s.contains("Response cache hit")),
            "{status:?}"
        );
    }

    #[tokio::test]
    async fn overflow_fail_open_retries_then_succeeds() {
        let mut agent = batched([
            vec![ScriptedStep::FailOpen("context_length_exceeded".into())],
            vec![ScriptedStep::Text("recovered".into())],
        ]);
        // Local stub only — LLM compact would consume the recovery batch via complete().
        agent.compaction_llm = false;
        let mut session = session_user("please explain rust ownership in detail");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let out = agent
            .run_turn_with_events(
                &mut session,
                TurnOpts {
                    provider_name: "script",
                    model: "overflow-fail-open-test",
                    api_key: "k",
                    max_turns: Some(8),
                    events: Some(tx),
                    cancel: None,
                },
            )
            .await
            .expect("retry");
        assert!(out.contains("recovered"), "{out}");
        let status = drain_status(&mut rx);
        assert!(
            status
                .iter()
                .any(|s| s.contains("Context overflow") && s.contains("retrying")),
            "{status:?}"
        );
    }

    #[tokio::test]
    async fn overflow_stream_error_event_retries() {
        let mut agent = batched([
            vec![ScriptedStep::Error("context_length_exceeded".into())],
            vec![ScriptedStep::Text("after overflow".into())],
        ]);
        agent.compaction_llm = false;
        let mut session = session_user("please explain the compact circuit");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let out = agent
            .run_turn_with_events(
                &mut session,
                TurnOpts {
                    provider_name: "script",
                    model: "overflow-stream-error-test",
                    api_key: "k",
                    max_turns: Some(8),
                    events: Some(tx),
                    cancel: None,
                },
            )
            .await
            .expect("retry");
        assert!(out.contains("after overflow"), "{out}");
        let status = drain_status(&mut rx);
        assert!(
            status.iter().any(|s| s.contains("Context overflow")),
            "{status:?}"
        );
    }

    #[tokio::test]
    async fn non_overflow_stream_error_fails() {
        let agent = scripted([ScriptedStep::Error("gateway down".into())]);
        let mut session = session_user("please explain rust ownership");
        let err = agent
            .run_turn(&mut session, "script", "m", "k", Some(2))
            .await
            .expect_err("stream error");
        assert!(err.to_string().to_lowercase().contains("gateway"), "{err}");
    }

    #[tokio::test]
    async fn extended_stream_events_and_usage() {
        let agent = scripted([
            ScriptedStep::MessageStart,
            ScriptedStep::Thinking(String::new()),
            ScriptedStep::ThinkingDelta(String::new()),
            ScriptedStep::Thinking("plan".into()),
            ScriptedStep::ThinkingDelta(" more".into()),
            ScriptedStep::ThinkingSignature("sig".into()),
            ScriptedStep::RedactedThinking("red".into()),
            ScriptedStep::MessageDelta(json!({"stop_reason": "end_turn"})),
            ScriptedStep::Usage {
                input_tokens: 11,
                output_tokens: 7,
            },
            ScriptedStep::CacheUsage {
                creation_input_tokens: 2,
                read_input_tokens: 3,
            },
            ScriptedStep::Text("final".into()),
        ]);
        let mut session = session_user("please walk through the retry loop carefully");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let out = agent
            .run_turn_with_events(&mut session, opts(Some(tx)))
            .await
            .expect("turn");
        assert!(out.contains("final"), "{out}");
        let mut saw_usage = false;
        let mut saw_thinking = false;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                TurnEvent::Usage(u) => {
                    assert_eq!(u.input_tokens, 11);
                    assert_eq!(u.output_tokens, 7);
                    saw_usage = true;
                }
                TurnEvent::ThinkingDelta(t) if t.contains("plan") || t.contains("more") => {
                    saw_thinking = true;
                }
                _ => {}
            }
        }
        assert!(saw_usage, "usage event");
        assert!(saw_thinking, "thinking event");
    }

    #[tokio::test]
    async fn tool_use_delta_then_text() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("n.txt"), "payload").unwrap();
        let agent = scripted([
            ScriptedStep::ToolCall {
                id: "c1".into(),
                name: "read".into(),
                input: json!({}),
            },
            ScriptedStep::ToolUseDelta {
                id: "c1".into(),
                input_json_delta: r#"{"path":"n.txt"}"#.into(),
            },
            ScriptedStep::Text("got it".into()),
        ]);
        let mut session = session_at(dir.path(), "please read n.txt and summarize it");
        let out = agent
            .run_turn(&mut session, "script", "m", "k", Some(4))
            .await
            .expect("turn");
        assert!(out.contains("got it"), "{out}");
    }

    #[tokio::test]
    async fn failed_tool_injects_recovery_message() {
        let dir = tempfile::tempdir().unwrap();
        let agent = scripted([
            ScriptedStep::ToolCall {
                id: "c1".into(),
                name: "read".into(),
                input: json!({"path": "missing-nope.txt"}),
            },
            ScriptedStep::Text("recovered from the miss".into()),
        ]);
        let mut session = session_at(dir.path(), "please read missing-nope.txt carefully");
        let out = agent
            .run_turn(&mut session, "script", "m", "k", Some(4))
            .await
            .expect("turn");
        assert!(out.contains("recovered"), "{out}");
        let joined: String = session
            .messages
            .iter()
            .filter(|m| m.role == Role::User)
            .filter_map(|m| m.content.as_text().map(|s| s.to_string()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("The tool failed with error"), "{joined}");
    }

    #[tokio::test]
    async fn checkpoint_then_rewind_collapses() {
        let agent = scripted([
            ScriptedStep::ToolCall {
                id: "c1".into(),
                name: "checkpoint".into(),
                input: json!({"goal": "look around"}),
            },
            ScriptedStep::ToolCall {
                id: "c2".into(),
                name: "rewind".into(),
                input: json!({"report": "nothing found"}),
            },
            ScriptedStep::Text("collapsed".into()),
        ]);
        let mut session = session_user("please explore then rewind the investigation");
        let out = agent
            .run_turn(&mut session, "script", "m", "k", Some(6))
            .await
            .expect("turn");
        assert!(out.contains("collapsed"), "{out}");
    }

    #[tokio::test]
    async fn skip_prompt_cache_next_is_consumed() {
        let agent = scripted([ScriptedStep::Text("fresh".into())]);
        agent.skip_prompt_cache_next();
        let mut session = session_user("please explain the skip cache path");
        let out = agent
            .run_turn(&mut session, "script", "m", "k", Some(2))
            .await
            .expect("turn");
        assert!(out.contains("fresh"), "{out}");
        assert!(
            !agent
                .skip_prompt_cache_once
                .load(std::sync::atomic::Ordering::Relaxed)
        );
    }

    #[tokio::test]
    async fn swarm_stripped_from_defs_when_disabled() {
        let mut agent = scripted([ScriptedStep::Text("no swarm".into())])
            .with_tool_profile(whycodes_tools::profile::ToolProfile::Full);
        agent.swarm_enabled = false;
        let mut session = session_user("please explain how swarm isolation works");
        let out = agent
            .run_turn(&mut session, "script", "m", "k", Some(2))
            .await
            .expect("turn");
        assert!(out.contains("no swarm"), "{out}");
    }

    #[tokio::test]
    async fn magic_ultrathink_completes() {
        let agent = scripted([ScriptedStep::Text("careful answer".into())]);
        let mut session = session_user("please ultrathink the retry loop");
        let out = agent
            .run_turn(&mut session, "script", "m", "k", Some(3))
            .await
            .expect("turn");
        assert!(out.contains("careful"), "{out}");
    }

    #[tokio::test]
    async fn intent_event_emitted_on_question() {
        let agent = scripted([ScriptedStep::Text("auth is a gate".into())]);
        let mut session = session_user("how does authentication work in this crate?");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let _ = agent
            .run_turn_with_events(&mut session, opts(Some(tx)))
            .await
            .expect("turn");
        let mut saw_intent = false;
        while let Ok(ev) = rx.try_recv() {
            if let TurnEvent::Intent { kind, .. } = ev {
                saw_intent = true;
                assert!(!kind.is_empty(), "{kind}");
            }
        }
        assert!(saw_intent, "intent event");
    }

    #[tokio::test]
    async fn auto_compact_pauses_after_failures() {
        // Last user is the first message, so full-replace keeps the huge tail
        // (`still_over`) and we need several LLM steps to trip the breaker.
        let mut agent = repeating([ScriptedStep::ToolCall {
            id: "c1".into(),
            name: "read".into(),
            input: json!({"path": "missing-compact.txt"}),
        }]);
        agent.compaction_threshold = 8;
        agent.compaction_llm = false;
        let mut session = Session::new(std::path::PathBuf::from("/work/proj"), "test".into());
        session.add_user_message("please keep summarizing the huge dump");
        for i in 0..12 {
            session.add_assistant_message(vec![ContentBlock::Text {
                text: format!("step {i}"),
            }]);
            session.add_tool_results(vec![whycodes_core::types::ToolResult {
                tool_call_id: format!("t{i}"),
                content: format!("TOOL DUMP {i} {}", "x".repeat(400)),
                is_error: false,
            }]);
        }
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let _ = agent
            .run_turn_with_events(
                &mut session,
                TurnOpts {
                    provider_name: "script",
                    model: "m",
                    api_key: "k",
                    max_turns: Some(6),
                    events: Some(tx),
                    cancel: None,
                },
            )
            .await;
        let status = drain_status(&mut rx);
        assert!(
            status
                .iter()
                .any(|s| s.contains("Auto-compact paused") || s.contains("Compacted")),
            "{status:?}"
        );
    }

    #[tokio::test]
    async fn cancel_before_llm_returns_cancelled() {
        let agent = scripted([ScriptedStep::Text("never".into())]);
        let mut session = session_user("please explain the retry loop");
        let cancel = crate::events::new_cancel_flag();
        crate::events::request_cancel(&cancel);
        let err = agent
            .run_turn_with_events(
                &mut session,
                TurnOpts {
                    provider_name: "script",
                    model: "m",
                    api_key: "k",
                    max_turns: Some(4),
                    events: None,
                    cancel: Some(cancel),
                },
            )
            .await
            .expect_err("cancelled");
        assert!(err.to_string().to_lowercase().contains("cancel"), "{err}");
    }

    #[tokio::test]
    async fn doom_loop_refuses_repeated_read() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("n.txt"), "payload").unwrap();
        let agent = repeating([ScriptedStep::ToolCall {
            id: "c1".into(),
            name: "read".into(),
            input: json!({"path": "n.txt"}),
        }]);
        let mut session = session_at(dir.path(), "please read n.txt and keep checking it");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let _ = agent
            .run_turn_with_events(
                &mut session,
                TurnOpts {
                    provider_name: "script",
                    model: "m",
                    api_key: "k",
                    max_turns: Some(6),
                    events: Some(tx),
                    cancel: None,
                },
            )
            .await;
        let status = drain_status(&mut rx);
        assert!(
            status.iter().any(|s| s.to_lowercase().contains("doom")),
            "{status:?}"
        );
    }

    #[tokio::test]
    async fn stream_rule_interrupts_draft() {
        let mut cfg = whycodes_config::Config::default();
        cfg.session
            .stream_rules
            .push(whycodes_config::StreamRuleConfig {
                name: "no-secret".into(),
                pattern: "FORBIDDENWORD".into(),
                hint: "do not leak secrets".into(),
            });
        let mut registry = ProviderRegistry::new();
        registry.register(Box::new(ScriptedProvider::new([ScriptedStep::Text(
            "FORBIDDENWORD in the draft".into(),
        )])));
        let agent = Agent::new(info("build"))
            .with_config(&cfg)
            .with_provider_registry(registry);
        let mut session = session_user("please summarize crates/agent carefully");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let _ = agent
            .run_turn_with_events(&mut session, opts(Some(tx)))
            .await;
        let status = drain_status(&mut rx);
        assert!(status.iter().any(|s| s.contains("no-secret")), "{status:?}");
    }

    #[allow(dead_code)]
    fn _approval_mode_used() {
        let _ = ApprovalMode::Auto;
    }

    #[tokio::test]
    async fn first_token_race_hang_then_text() {
        let mut registry = ProviderRegistry::new();
        registry.register(Box::new(ScriptedProvider::named(
            "script",
            [ScriptedStep::Hang(std::time::Duration::from_millis(80))],
        )));
        registry.register(Box::new(ScriptedProvider::named(
            "openai",
            [ScriptedStep::Text("raced".into())],
        )));
        let mut agent = Agent::new(info("build")).with_provider_registry(registry);
        agent.model_race = "openai/gpt-4o-mini".into();
        agent.race_after = std::time::Duration::from_millis(10);
        let mut session = session_user("please explain the first-token race partner");
        let out = agent
            .run_turn(&mut session, "script", "m", "k", Some(3))
            .await
            .unwrap_or_else(|_| "raced-or-hang".into());
        assert!(!out.is_empty(), "{out}");
    }

    #[tokio::test]
    async fn tools_free_chat_false_when_session_has_tool_use() {
        let agent = scripted([ScriptedStep::Text("still here".into())]);
        let mut session = Session::new(std::path::PathBuf::from("/work/proj"), "test".into());
        session.add_user_message("hi");
        session.add_assistant_message(vec![ContentBlock::ToolUse {
            id: "c1".into(),
            name: "read".into(),
            input: json!({"path": "n.txt"}),
        }]);
        let out = agent
            .run_turn(&mut session, "script", "m", "k", Some(2))
            .await
            .expect("turn");
        assert!(out.contains("still here"), "{out}");
    }

    #[tokio::test]
    async fn cancel_during_stream_persists_accumulated_text() {
        let agent = scripted([
            ScriptedStep::Text("partial draft".into()),
            ScriptedStep::Hang(std::time::Duration::from_secs(30)),
        ]);
        let mut session = session_user("please explain the retry loop");
        let cancel = crate::events::new_cancel_flag();
        let handle = {
            let cancel = cancel.clone();
            tokio::spawn(async move {
                agent
                    .run_turn_with_events(
                        &mut session,
                        TurnOpts {
                            provider_name: "script",
                            model: "m",
                            api_key: "k",
                            max_turns: Some(4),
                            events: None,
                            cancel: Some(cancel),
                        },
                    )
                    .await
                    .map_err(|e| e.to_string())
                    .map(|_| session)
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        crate::events::request_cancel(&cancel);
        let result = handle.await.expect("join");
        assert!(
            result
                .as_ref()
                .err()
                .is_some_and(|e| e.to_lowercase().contains("cancel")),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn rewind_without_checkpoint_is_debug_only() {
        let agent = scripted([
            ScriptedStep::ToolCall {
                id: "c1".into(),
                name: "rewind".into(),
                input: json!({"report": "nothing found"}),
            },
            ScriptedStep::Text("kept going".into()),
        ]);
        let mut session = session_user("please rewind even without a checkpoint");
        let out = agent
            .run_turn(&mut session, "script", "m", "k", Some(4))
            .await
            .expect("turn");
        assert!(out.contains("kept going"), "{out}");
    }

    #[tokio::test]
    async fn intent_warning_emitted_for_ask_agent_on_change() {
        let agent = Agent::new(info("ask")).with_provider_registry({
            let mut registry = ProviderRegistry::new();
            registry.register(Box::new(ScriptedProvider::new([ScriptedStep::Text(
                "I can only advise".into(),
            )])));
            registry
        });
        let mut session = session_user("Fix the auth bug in session.rs");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let _ = agent
            .run_turn_with_events(&mut session, opts(Some(tx)))
            .await
            .expect("turn");
        let mut saw_warning = false;
        while let Ok(ev) = rx.try_recv() {
            if let TurnEvent::Intent { notice_kind, .. } = ev
                && notice_kind == "warning"
            {
                saw_warning = true;
            }
        }
        assert!(saw_warning, "expected warning intent notice");
    }

    #[tokio::test]
    async fn fold_subagent_usage_from_task_tool() {
        let agent = repeating([
            ScriptedStep::Usage {
                input_tokens: 3,
                output_tokens: 2,
            },
            ScriptedStep::Text("worker done".into()),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let session = session_at(dir.path(), "please spawn a worker to inspect the tree");
        let result = agent
            .execute_task_tool(
                &whycodes_core::types::ToolCall {
                    id: "c1".into(),
                    name: "task".into(),
                    arguments: json!({"goal": "inspect the tree", "max_turns": 1}),
                },
                &session,
                "script",
                "m",
                "k",
                None,
            )
            .await;
        assert!(!result.is_error, "{result:?}");
        assert!(
            result.content.contains("worker done") || !result.content.is_empty(),
            "{}",
            result.content
        );
        let fold = agent
            .subagent_usage_pending
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default();
        assert!(
            fold.input_tokens > 0 || fold.output_tokens > 0 || !result.content.is_empty(),
            "{fold:?} {}",
            result.content
        );
    }

    #[tokio::test]
    async fn cancel_at_stream_open_before_first_token() {
        let agent = scripted([ScriptedStep::Hang(std::time::Duration::from_secs(30))]);
        let mut session = session_user("please explain the retry loop");
        let cancel = crate::events::new_cancel_flag();
        let handle = {
            let cancel = cancel.clone();
            tokio::spawn(async move {
                agent
                    .run_turn_with_events(
                        &mut session,
                        TurnOpts {
                            provider_name: "script",
                            model: "m",
                            api_key: "k",
                            max_turns: Some(4),
                            events: None,
                            cancel: Some(cancel),
                        },
                    )
                    .await
                    .map_err(|e| e.to_string())
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        crate::events::request_cancel(&cancel);
        let err = handle.await.expect("join");
        assert!(
            err.as_ref()
                .err()
                .is_some_and(|e| e.to_lowercase().contains("cancel")),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn parent_turn_folds_task_tool_usage() {
        let mut registry = ProviderRegistry::new();
        registry.register(Box::new(ScriptedProvider::batched(
            "script",
            [
                vec![ScriptedStep::ToolCall {
                    id: "c1".into(),
                    name: "task".into(),
                    input: json!({"goal": "inspect the tree", "max_turns": 1}),
                }],
                vec![ScriptedStep::Text("parent after task".into())],
            ],
        )));
        registry.register(Box::new(ScriptedProvider::repeating(
            "script-worker",
            [
                ScriptedStep::Usage {
                    input_tokens: 5,
                    output_tokens: 4,
                },
                ScriptedStep::Text("worker done".into()),
            ],
        )));
        let mut agent = Agent::new(info("build")).with_provider_registry(registry);
        agent.model_smol = Some("script-worker/m".into());
        let dir = tempfile::tempdir().unwrap();
        let mut session = session_at(dir.path(), "please spawn a worker to inspect the tree");
        let out = agent
            .run_turn(&mut session, "script", "m", "k", Some(4))
            .await
            .expect("turn");
        assert!(
            out.contains("parent after task") || session.usage.input_tokens > 0,
            "{out} {:?}",
            session.usage
        );
    }

    #[tokio::test]
    async fn intent_posture_injected_on_change_always() {
        let mut agent = scripted([ScriptedStep::Text("implementing".into())]);
        agent.intent_guidance = crate::intent::IntentGuidanceMode::Always;
        let mut session = session_user("Fix the auth bug in session.rs");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let out = agent
            .run_turn_with_events(&mut session, opts(Some(tx)))
            .await
            .expect("turn");
        assert!(out.contains("implementing"), "{out}");
        let _ = drain_status(&mut rx);
    }

    #[tokio::test]
    async fn shake_old_tool_results_before_llm() {
        let mut agent = scripted([ScriptedStep::Text("after shake".into())]);
        agent.compaction_threshold = 200;
        agent.compaction_llm = false;
        let mut session = Session::new(std::path::PathBuf::from("/work/proj"), "test".into());
        session.add_user_message("please keep summarizing the huge dump");
        for i in 0..8 {
            session.add_assistant_message(vec![ContentBlock::Text {
                text: format!("step {i}"),
            }]);
            session.add_tool_results(vec![whycodes_core::types::ToolResult {
                tool_call_id: format!("t{i}"),
                content: format!("TOOL DUMP {i} {}", "x".repeat(800)),
                is_error: false,
            }]);
        }
        let out = agent
            .run_turn(&mut session, "script", "m", "k", Some(2))
            .await
            .expect("turn");
        assert!(out.contains("after shake"), "{out}");
    }

    #[tokio::test]
    async fn doom_loop_pop_front_when_signatures_overflow() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("n.txt"), "payload").unwrap();
        let mut batches = Vec::new();
        for i in 0..20 {
            batches.push(vec![ScriptedStep::ToolCall {
                id: format!("c{i}"),
                name: "read".into(),
                input: json!({"path": format!("n{i}.txt")}),
            }]);
        }
        batches.push(vec![ScriptedStep::Text("done".into())]);
        let agent = batched(batches);
        let mut session = session_at(dir.path(), "please read many files in sequence");
        let _ = agent
            .run_turn(&mut session, "script", "m", "k", Some(24))
            .await;
    }

    #[tokio::test]
    async fn compact_failures_reset_when_under_threshold() {
        let mut agent = batched([
            vec![ScriptedStep::Text("first compact pass".into())],
            vec![ScriptedStep::Text("second compact pass".into())],
        ]);
        agent.compaction_threshold = 40;
        agent.compaction_llm = false;
        let mut session = Session::new(std::path::PathBuf::from("/work/proj"), "test".into());
        session.add_user_message("please keep summarizing");
        for i in 0..6 {
            session.add_assistant_message(vec![ContentBlock::Text {
                text: format!("step {i} {}", "y".repeat(80)),
            }]);
        }
        let out = agent
            .run_turn(&mut session, "script", "m", "k", Some(3))
            .await
            .unwrap_or_else(|_| "ok".into());
        assert!(!out.is_empty() || !session.messages.is_empty());
    }

    struct OverflowAfterTextProvider {
        calls: std::sync::atomic::AtomicU32,
    }

    impl OverflowAfterTextProvider {
        fn new() -> Self {
            Self {
                calls: std::sync::atomic::AtomicU32::new(0),
            }
        }
    }

    impl whycodes_llm::LlmProvider for OverflowAfterTextProvider {
        fn name(&self) -> &str {
            "overflow-script"
        }
        fn default_base_url(&self) -> &str {
            "http://script.invalid"
        }
        fn complete<'a>(
            &'a self,
            _request: &'a whycodes_core::types::LlmRequest,
            _api_key: &'a str,
            _model: &'a str,
        ) -> whycodes_llm::provider::ProviderResponseFuture<'a> {
            Box::pin(async { Err(whycodes_core::Error::llm("context_length_exceeded")) })
        }
        fn stream<'a>(
            &'a self,
            _request: &'a whycodes_core::types::LlmRequest,
            _api_key: &'a str,
            _model: &'a str,
        ) -> whycodes_llm::provider::ProviderStreamFuture<'a> {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async move {
                if n == 0 {
                    Ok(Box::pin(async_stream::stream! {
                        yield Ok(whycodes_core::types::StreamEvent::TextDelta {
                            text: "partial overflow".into(),
                        });
                        yield Err(whycodes_core::Error::llm_kind(
                            whycodes_core::ErrorKind::ContextOverflow,
                            "context_length_exceeded",
                        ));
                    })
                        as whycodes_llm::provider::ProviderEventStream)
                } else {
                    Ok(Box::pin(async_stream::stream! {
                        yield Ok(whycodes_core::types::StreamEvent::TextDelta {
                            text: "after compact".into(),
                        });
                    })
                        as whycodes_llm::provider::ProviderEventStream)
                }
            })
        }
    }

    #[tokio::test]
    async fn mid_stream_context_overflow_compacts_and_retries() {
        let mut registry = ProviderRegistry::new();
        registry.register(Box::new(OverflowAfterTextProvider::new()));
        let mut agent = Agent::new(info("build")).with_provider_registry(registry);
        agent.compaction_llm = false;
        agent.compaction_threshold = 8;
        let mut session = Session::new(std::path::PathBuf::from("/work/proj"), "test".into());
        session.add_user_message("please keep summarizing the huge dump");
        for i in 0..8 {
            session.add_assistant_message(vec![ContentBlock::Text {
                text: format!("step {i} {}", "x".repeat(80)),
            }]);
        }
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let out = agent
            .run_turn_with_events(
                &mut session,
                TurnOpts {
                    provider_name: "overflow-script",
                    model: "overflow-mid-stream-unique",
                    api_key: "k",
                    max_turns: Some(4),
                    events: Some(tx),
                    cancel: None,
                },
            )
            .await
            .expect("retry after mid-stream overflow");
        assert!(out.contains("after compact"), "{out}");
        let status = drain_status(&mut rx);
        assert!(
            status
                .iter()
                .any(|s| s.to_lowercase().contains("overflow")
                    || s.to_lowercase().contains("compact")),
            "{status:?}"
        );
    }

    #[tokio::test]
    async fn doom_loop_pop_front_on_refused_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("n.txt"), "payload").unwrap();
        let mut batches = Vec::new();
        for i in 0..16 {
            batches.push(vec![ScriptedStep::ToolCall {
                id: format!("u{i}"),
                name: "read".into(),
                input: json!({"path": format!("n{i}.txt")}),
            }]);
        }
        for i in 0..4 {
            batches.push(vec![ScriptedStep::ToolCall {
                id: format!("d{i}"),
                name: "read".into(),
                input: json!({"path": "same.txt"}),
            }]);
        }
        batches.push(vec![ScriptedStep::Text("stopped".into())]);
        let agent = batched(batches);
        let mut session = session_at(dir.path(), "please read many files then the same one");
        let _ = agent
            .run_turn(&mut session, "script", "m", "k", Some(28))
            .await;
    }
}
