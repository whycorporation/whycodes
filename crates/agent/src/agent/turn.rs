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
