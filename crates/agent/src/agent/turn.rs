//! Conversation turn loop (LLM stream → tool batch → repeat).

use futures::StreamExt;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use whycodes_core::types::{ContentBlock, StreamEvent, ToolResult};
use whycodes_session::session::Session;

use crate::events::{TurnEvent, TurnOpts, emit, is_cancelled, wait_until_cancelled};
use crate::stream_rules::StreamChannel;
use crate::tool_stream::ToolCallAssembler;

use super::{
    Agent, DOOM_LOOP_THRESHOLD, MAX_CONSECUTIVE_COMPACT_FAILURES, append_request_user_suffix,
    append_turn_context, settle_checkpoint_rewind, tool_call_signature, would_doom_loop,
};

impl Agent {
    /// Tool schemas sent with a step (cached; extra activations still apply).
    pub(crate) fn step_tools(&self) -> std::sync::Arc<[whycodes_core::types::ToolDefinition]> {
        let extra = self.activated_tools_snapshot();
        let defs = self.tool_executor.get_definitions_profile_extra(
            &self.info.permission,
            self.tool_profile,
            &extra,
        );
        if !self.swarm_enabled && defs_include_swarm(&defs) {
            defs_without_swarm(&defs).into()
        } else {
            defs
        }
    }

    /// Run a single conversation turn (no streaming UI events).
    ///
    /// `max_turns` is a headless safety cap (`None` = unlimited, as in
    /// the TUI). Interactive sessions pass `None` and stop on end-of-turn,
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
    ///
    /// `session_start` / `user_prompt_submit` hooks run first; `stop` hooks
    /// run after a reply.
    pub async fn run_turn_with_events(
        &self,
        session: &mut Session,
        opts: TurnOpts<'_>,
    ) -> whycodes_core::Result<String> {
        let prompt = session
            .messages
            .iter()
            .rev()
            .find(|m| m.role == whycodes_core::types::Role::User)
            .and_then(|m| m.content.as_text().map(str::to_string))
            .unwrap_or_default();
        self.run_prompt_hooks(session, &prompt).await?;
        let usage_before = session.usage.clone();
        let reply = self.run_turn_steps(session, opts).await?;
        self.run_stop_hooks(session, &reply, &usage_before).await;
        Ok(reply)
    }

    /// The model/tool step loop of one user turn.
    async fn run_turn_steps(
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
        session.stream_rules.begin_turn();
        let watch_tool_args = self.stream_rules.iter().any(|r| {
            r.scopes
                .contains(&whycodes_config::StreamRuleScope::ToolArgs)
        });
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
        // Prewalk plans on `model_plan`, like the `plan` agent.
        let plan_role = if self.prewalk {
            "plan"
        } else {
            &self.info.name
        };
        let (role_provider, role_model) = crate::routing::resolve_agent_model(
            provider_name,
            model,
            plan_role,
            self.model_plan.as_deref(),
        );
        let provider_name = role_provider.as_str();
        let model = role_model.as_str();
        let tools_free_chat = crate::title::is_trivial_title_seed(&last_user)
            && session.user_message_count() <= 1
            && !transcript_has_tool_work(&session.messages);

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

        // Completed LLM steps. A step that is retried before its stream finished
        // (overflow, credential rotation, fallback, stream rule, harmony leak)
        // is not counted, so `max_turns` and the first-step suffixes see the
        // retry as the same step.
        let mut turn_count = 0;
        let mut final_text = String::new();
        // Latency: wall clock for the whole user turn (all LLM steps + tools).
        let user_turn_t0 = Instant::now();
        let mut ttft_ms: Option<u128> = None;
        // Recent tool signatures for doom-loop detection.
        let mut recent_tool_sigs: VecDeque<String> = VecDeque::with_capacity(8);
        // Autocompact circuit breaker: stop retrying after N ineffective passes.
        let mut compact_failures: u32 = 0;
        let mut compact_paused = false;
        let mut overflow_retries: u32 = 0;
        let mut harmony_retries: u32 = 0;
        // Steps in a row cut off at the output token limit.
        let mut cut_offs: u32 = 0;
        // Mid-stream drop retry (`stream_retry_secs`): first drop + count.
        let mut stream_drop_t0: Option<Instant> = None;
        let mut stream_drops: usize = 0;
        let mut api_key = api_key.to_string();
        // Set once the primary fails to open, or at the prewalk hand-off;
        // sticks for the rest of this turn. The index is the next
        // `fallback_models` entry to try.
        let mut fallback: Option<(usize, super::FallbackTarget, &dyn whycodes_llm::LlmProvider)> =
            None;
        let (strong_provider, strong_model) = (provider_name, model);
        if let Some((target, next)) = self.prewalk_resume(strong_provider, strong_model) {
            emit(
                &events,
                TurnEvent::Status(format!(
                    "Prewalk: continuing on {}/{} (/prewalk restart plans again)",
                    target.provider, target.model
                )),
            );
            api_key = target.api_key.clone();
            fallback = Some((0, target, next));
        }

        loop {
            let (provider_name, model, provider) = match &fallback {
                Some((_, target, p)) => (target.provider.as_str(), target.model.as_str(), *p),
                None => (provider_name, model, provider),
            };
            let harmony = whycodes_llm::uses_harmony_dialect(provider_name, model);
            self.inject_background_completions(session);
            self.inject_steering(session, &events);
            // Cached schemas; extra activations still apply per step.
            let tools = if tools_free_chat {
                std::sync::Arc::from([])
            } else {
                self.step_tools()
            };
            let tool_ctx = self.tool_context(session);
            if is_cancelled(&cancel) {
                emit(&events, TurnEvent::Cancelled);
                return Err(whycodes_core::Error::Agent("Cancelled".into()));
            }

            let max = max_turns.unwrap_or(0);
            if max_turns.is_some() && turn_count >= max {
                return Err(whycodes_core::Error::Agent(format!(
                    "Exceeded maximum turns ({max})"
                )));
            }

            // Always shrink oversized / old tool dumps before prefill (cheap).
            // When still hot, shake older tool bodies harder so overflow is less likely.
            // Full-replace compact when over the configured token threshold —
            // and only while the circuit breaker has not tripped.
            let _truncated = session.truncate_large_tool_results();
            let spilled = session.spill_old_tool_results(&mut super::spill::artifact_store(
                session.project_path.clone(),
            ));
            if spilled > 0 {
                tracing::debug!(spilled, "spilled old tool results to scratch artifacts");
            }
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
                    let outcome = match self.take_speculative_summary(session).await {
                        Some(summary) => {
                            tracing::info!(
                                "compacting with the summary prepared before the threshold"
                            );
                            self.compact_with_summary(session, &summary).await
                        }
                        None => {
                            self.compact_session(session, provider_name, model, &api_key, None)
                                .await
                        }
                    };
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
                    compact_failures = next_compact_failures(
                        compact_failures,
                        outcome.still_over(self.compaction_threshold),
                    );
                    if compact_failures >= MAX_CONSECUTIVE_COMPACT_FAILURES && !compact_paused {
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
                } else if before.saturating_mul(100)
                    >= self
                        .compaction_threshold
                        .saturating_mul(super::compact::SPECULATIVE_LEAD_PERCENT)
                    && self.maybe_spawn_speculative_summary(session, provider_name, model, &api_key)
                {
                    tracing::debug!(
                        tokens = before,
                        "preparing a compaction summary ahead of the threshold"
                    );
                }
            }

            emit(
                &events,
                TurnEvent::Status(format!("LLM request (step {})…", turn_count + 1)),
            );

            let mut request = session.build_request(tools, None, self.info.temperature, Some(true));
            append_turn_context(&mut request, session.turn_context.as_deref());
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
            if turn_count == 0
                && crate::intent::should_inject(self.intent_guidance, &turn_intent)
                && let Some(suffix) = crate::intent::posture_suffix(&turn_intent, &self.info.name)
            {
                append_request_user_suffix(&mut request, &suffix);
                let intent = turn_intent.intent.as_str();
                let confidence = turn_intent.confidence;
                tracing::debug!(
                    intent,
                    confidence,
                    agent = %self.info.name,
                    "intent posture injected into request"
                );
            }
            if turn_count == 0 && magic.any() {
                let notice = magic.notice();
                append_request_user_suffix(&mut request, &notice);
                tracing::debug!(
                    ultrathink = magic.ultrathink,
                    orchestrate = magic.orchestrate,
                    "magic keyword notice injected into request"
                );
            }

            let mut accumulated_text = String::new();
            let mut accumulated_thinking = String::new();
            let mut thinking_acc = crate::thinking_acc::ThinkingAccumulator::new();
            let mut turn_usage = whycodes_core::types::Usage::default();
            let mut assembler = ToolCallAssembler::new();
            let mut speculative_reads: Vec<crate::speculative_read::SpeculativeRead> = Vec::new();
            let mut hit_output_limit = false;
            let step_t0 = Instant::now();

            // Professional transport: classify + full-jitter backoff + Retry-After.
            // Only the HTTP open is retried — mid-stream drops stay single-shot.
            // Race the open against cancel so a hung gateway cannot ignore Esc.
            // Bind transport so `stream()`'s future is not tied to a temporary.
            let transport = whycodes_llm::default_transport().with_redactor(self.secrets.clone());
            let race_ids = self.race_partner(provider_name, model);
            let race_provider = race_ids
                .as_ref()
                .and_then(|(p, _)| self.provider_registry.get(p.as_str()));
            let race_target = match (race_ids.as_ref(), race_provider) {
                (Some((_, m)), Some(rp)) => Some(whycodes_llm::StreamTarget {
                    provider: rp,
                    api_key: &api_key,
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
                        api_key: &api_key,
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
                Err(e) => {
                    let kind = whycodes_llm::classify(&e).kind;
                    if kind == whycodes_llm::ErrorKind::ContextOverflow && overflow_retries < 1 {
                        overflow_retries = overflow_retries.saturating_add(1);
                        emit(
                            &events,
                            TurnEvent::Status(
                                "Context overflow — compacting and retrying this step…".into(),
                            ),
                        );
                        let outcome = self
                            .compact_session(session, provider_name, model, &api_key, None)
                            .await;
                        tracing::info!(
                            after_tokens = outcome.tokens_after,
                            "compacted after context overflow"
                        );
                        continue;
                    }
                    if kind == whycodes_llm::ErrorKind::RateLimited {
                        let next = self.failover_api_key(provider_name, &api_key);
                        let (name, secret) = match &next {
                            Some((name, secret)) => (name.clone(), secret.clone()),
                            None => (String::new(), String::new()),
                        };
                        if !secret.is_empty() && secret != api_key {
                            let mut status = String::from("Rate limited — retrying ");
                            status.push_str(provider_name);
                            status.push('/');
                            status.push_str(model);
                            status.push_str(" on credential `");
                            status.push_str(&name);
                            status.push('`');
                            emit(&events, TurnEvent::Status(status));
                            api_key = secret;
                            self.set_sticky_credential(Some(name));
                            continue;
                        }
                    }
                    let from = fallback.as_ref().map_or(0, |(next, _, _)| *next);
                    if kind.retryable()
                        && let Some((idx, target)) =
                            self.next_fallback(from, provider_name, model, &api_key)
                        && let Some(next) = self.provider_registry.get(&target.provider)
                    {
                        let from_ref = format!("{provider_name}/{model}");
                        let to_ref = format!("{}/{}", target.provider, target.model);
                        emit(
                            &events,
                            TurnEvent::Status(format!(
                                "{from_ref} unavailable ({kind}) — falling back to {to_ref}"
                            )),
                        );
                        tracing::warn!(from = %from_ref, to = %to_ref, %kind, "model fallback");
                        api_key = target.api_key.clone();
                        fallback = Some((idx + 1, target, next));
                        continue;
                    }
                    return Err(e);
                }
            };
            // The request holds a full copy of the transcript. Once the stream is
            // open it is only needed to store a tools-free reply in the response
            // cache; drop it now otherwise instead of holding it while streaming.
            let cacheable = (self.response_cache && request.tools.is_empty()).then_some(request);
            let cache_hit = turn.cache_hit;
            if self.sticky_credential().is_none() {
                self.set_sticky_credential(Some("default".into()));
            }
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
                            append_step_text(&mut final_text, &accumulated_text);
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
                            .compact_session(session, provider_name, model, &api_key, None)
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
                        let first_drop = *stream_drop_t0.get_or_insert_with(Instant::now);
                        let Some(delay) =
                            stream_retry_delay(self.stream_retry, first_drop, stream_drops, &e)
                        else {
                            return Err(e);
                        };
                        stream_drops += 1;
                        emit(
                            &events,
                            TurnEvent::Status(format!(
                                "Stream dropped ({}) — retrying this step…",
                                whycodes_llm::classify(&e).kind
                            )),
                        );
                        let delay_ms = delay.as_millis() as u64;
                        tracing::warn!(stream_drops, delay_ms, error = %e, "stream drop retry");
                        tokio::select! {
                            biased;
                            _ = wait_until_cancelled(&cancel) => {
                                emit(&events, TurnEvent::Cancelled);
                                return Err(whycodes_core::Error::Agent("Cancelled".into()));
                            }
                            _ = tokio::time::sleep(delay) => {}
                        }
                        stream_rule_retry = true;
                        break;
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
                        if check_stream_rules(
                            &self.stream_rules,
                            StreamChannel::Text,
                            &accumulated_text,
                            &mut speculative_reads,
                            &events,
                            session,
                        ) {
                            stream_rule_retry = true;
                            break;
                        }
                    }
                    StreamEvent::ToolUse { id, name, input } => {
                        thinking_acc.flush();
                        // Defer ToolStart until after argument fragments are
                        // merged — OpenAI streams send null/empty args first.
                        assembler.on_tool_use(id, name, input);
                        if watch_tool_args
                            && check_tool_args_rules(
                                &self.stream_rules,
                                &assembler,
                                &mut speculative_reads,
                                &events,
                                session,
                            )
                        {
                            stream_rule_retry = true;
                            break;
                        }
                        // Complete objects (Anthropic non-streamed) can start I/O now.
                        maybe_start_speculative(&mut speculative_reads, &assembler, &tool_ctx);
                    }
                    StreamEvent::ToolUseDelta {
                        id,
                        input_json_delta,
                    } => {
                        assembler.on_tool_use_delta(&id, &input_json_delta);
                        if watch_tool_args
                            && check_tool_args_rules(
                                &self.stream_rules,
                                &assembler,
                                &mut speculative_reads,
                                &events,
                                session,
                            )
                        {
                            stream_rule_retry = true;
                            break;
                        }
                        // Path often closes mid-stream — start `read` I/O early.
                        maybe_start_speculative(&mut speculative_reads, &assembler, &tool_ctx);
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
                        accumulated_thinking.push_str(&text);
                        if check_stream_rules(
                            &self.stream_rules,
                            StreamChannel::Thinking,
                            &accumulated_thinking,
                            &mut speculative_reads,
                            &events,
                            session,
                        ) {
                            stream_rule_retry = true;
                            break;
                        }
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
                        accumulated_thinking.push_str(&text);
                        if check_stream_rules(
                            &self.stream_rules,
                            StreamChannel::Thinking,
                            &accumulated_thinking,
                            &mut speculative_reads,
                            &events,
                            session,
                        ) {
                            stream_rule_retry = true;
                            break;
                        }
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
                    StreamEvent::MessageStart { .. } => skip_event(),
                    StreamEvent::MessageDelta { delta } => {
                        hit_output_limit |= is_output_limit_stop(&delta);
                    }
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
                                .compact_session(session, provider_name, model, &api_key, None)
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
            let mut tool_calls = assembler.finish();
            let step_ms = step_t0.elapsed().as_millis();
            // A response cut off at the output token limit is not an answer,
            // and its last tool call may hold half its arguments: run none of
            // them and ask the model to go on from where it stopped.
            let cut_off = hit_output_limit && cut_offs < MAX_CUT_OFFS;
            if cut_off {
                cut_offs += 1;
                let dropped = tool_calls.len();
                tracing::warn!(
                    dropped_tool_calls = dropped,
                    cut_offs,
                    "response cut off at the output token limit; continuing"
                );
                emit(
                    &events,
                    TurnEvent::Status("Response hit the output token limit — continuing…".into()),
                );
                tool_calls.clear();
            } else {
                cut_offs = 0;
            }

            if harmony
                && let Some(hit) = harmony_leak(&accumulated_text, &thinking_acc, &tool_calls)
            {
                crate::speculative_read::abort_all(&mut speculative_reads);
                let summary = hit.summary();
                let co_signal = hit.co_signal.as_str();
                tracing::warn!(
                    co_signal,
                    retry = harmony_retries,
                    "harmony protocol leak; discarding draft"
                );
                emit(
                    &events,
                    TurnEvent::Status(format!("{summary} — retrying this step…")),
                );
                if harmony_retries >= 2 {
                    return Err(whycodes_core::Error::Agent(format!(
                        "{summary}; stopped after {harmony_retries} retries"
                    )));
                }
                harmony_retries = harmony_retries.saturating_add(1);
                continue;
            }

            if let Some(request) = &cacheable
                && !cache_hit
                && !cut_off
                && tool_calls.is_empty()
                && !accumulated_text.trim().is_empty()
            {
                whycodes_llm::ResponseCache::global().store(request, model, &accumulated_text);
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
                session
                    .cost
                    .add(self.step_cost(provider_name, model, &turn_usage));
                emit(&events, TurnEvent::Usage(turn_usage.clone()));
                if let Some(note) = self.note_cache_read(&session.id, &turn_usage, skip_cache) {
                    tracing::info!(session = %session.id, "{note}");
                    emit(&events, TurnEvent::Status(note));
                }
            }

            // The stream finished: this step counts.
            turn_count += 1;

            let mut blocks: Vec<ContentBlock> = thinking_acc.into_blocks();

            if !accumulated_text.is_empty() {
                blocks.push(ContentBlock::Text {
                    text: accumulated_text.clone(),
                });
                append_step_text(&mut final_text, &accumulated_text);
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

            if cut_off {
                crate::speculative_read::abort_all(&mut speculative_reads);
                session.add_user_message(CUT_OFF_NOTE);
                continue;
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
            let results = if would_doom_loop(&recent_tool_sigs, &tool_calls) {
                crate::speculative_read::abort_all(&mut speculative_reads);
                emit(
                    &events,
                    TurnEvent::Status("Doom loop: identical tool call repeated — refusing".into()),
                );
                let tools: Vec<&str> = tool_calls.iter().map(|t| t.name.as_str()).collect();
                tracing::warn!(tools = ?tools, "doom loop refused");
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
                // Parallel when safe.
                // Sequential for shell / mutating / permission-ask tools so risk
                // gates and the TUI single-slot permission UI stay correct.
                let tool_t0 = Instant::now();
                let executed = self
                    .execute_tool_calls(
                        &tool_calls,
                        session,
                        &tool_ctx,
                        provider_name,
                        model,
                        &api_key,
                        &events,
                        &cancel,
                        Some(&turn_intent),
                        &mut speculative_reads,
                    )
                    .await;
                let results = match executed {
                    Ok(results) => results,
                    Err(e) => return Err(close_unanswered_calls(session, &tool_calls, e)),
                };
                crate::speculative_read::abort_all(&mut speculative_reads);
                // Prewalk: the strong model planned and made the first edit;
                // the remaining steps run on the cheap model.
                if fallback.is_none()
                    && let Some((target, next)) = self.prewalk_target(
                        &tool_calls,
                        &tool_ctx.working_dir,
                        strong_provider,
                        strong_model,
                        &api_key,
                    )
                {
                    emit(
                        &events,
                        TurnEvent::Status(format!(
                            "Prewalk: handing off to {}/{} after the plan",
                            target.provider, target.model
                        )),
                    );
                    api_key = target.api_key.clone();
                    fallback = Some((0, target, next));
                }
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
            attach_nested_instructions(session, &tool_calls, &mut results);
            let (checkpoint_goal, rewind_report) =
                settle_checkpoint_rewind(session, &tool_calls, &mut results);

            // Capture failures before move — avoid cloning large tool bodies.
            let mut failed_tools = Vec::new();
            for result in &results {
                if result.is_error {
                    failed_tools.push(format!(
                        "The tool failed with error: {content}. Please correct your approach.",
                        content = result.content
                    ));
                }
            }

            session.record_edit_results(&tool_calls, &results);
            session.add_tool_results(results);
            let goal = checkpoint_goal.unwrap_or_default();
            if !goal.is_empty() {
                session.mark_checkpoint(&goal);
            }
            let report = rewind_report.unwrap_or_default();
            if !report.is_empty() {
                // `settle_checkpoint_rewind` only yields a report when a checkpoint
                // is already active, so apply always succeeds.
                let applied = session.apply_rewind(&report);
                tracing::debug!(applied, "collapsed exploratory context after rewind");
            }

            // Fold subagent tokens into this turn + parent session (plan-performance).
            let fold = take_pending_usage(&self.subagent_usage_pending).unwrap_or_default();
            if !fold.is_empty() {
                turn_usage.add(&fold);
                session.add_usage(&fold);
                let fold_cost =
                    std::mem::take(&mut *super::recover_lock(&self.subagent_cost_pending));
                session.cost.merge(&fold_cost);
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
            &api_key,
            events,
        );

        Ok(final_text)
    }
}

/// Add one step's text to the turn's text, a blank line apart: each step is
/// its own message, and gluing them read "…in parallel.The stubs match…".
fn append_step_text(final_text: &mut String, step: &str) {
    if step.trim().is_empty() {
        return;
    }
    if final_text.trim().is_empty() {
        final_text.clear();
        final_text.push_str(step);
        return;
    }
    let end = final_text.trim_end().len();
    final_text.truncate(end);
    final_text.push_str("\n\n");
    final_text.push_str(step.trim_start());
}

fn take_pending_usage(
    pending: &std::sync::Mutex<whycodes_core::types::Usage>,
) -> Option<whycodes_core::types::Usage> {
    let mut pending = super::recover_lock(pending);
    if pending.is_empty() {
        return None;
    }
    Some(std::mem::take(&mut *pending))
}

fn next_compact_failures(failures: u32, still_over: bool) -> u32 {
    if still_over {
        failures.saturating_add(1)
    } else {
        0
    }
}

fn harmony_leak(
    visible: &str,
    thinking: &crate::thinking_acc::ThinkingAccumulator,
    tool_calls: &[whycodes_core::types::ToolCall],
) -> Option<whycodes_core::harmony::Hit> {
    match whycodes_core::harmony::scan_text(visible) {
        Some(hit) => Some(hit),
        None => harmony_leak_thinking(thinking, tool_calls),
    }
}

fn harmony_leak_thinking(
    thinking: &crate::thinking_acc::ThinkingAccumulator,
    tool_calls: &[whycodes_core::types::ToolCall],
) -> Option<whycodes_core::harmony::Hit> {
    match thinking.harmony_leak() {
        Some(hit) => Some(hit),
        None => harmony_leak_tools(tool_calls),
    }
}

fn harmony_leak_tools(
    tool_calls: &[whycodes_core::types::ToolCall],
) -> Option<whycodes_core::harmony::Hit> {
    for tc in tool_calls {
        match whycodes_core::harmony::scan_json(&tc.arguments) {
            Some(hit) => return Some(hit),
            None => note_clean_tool_args(),
        }
    }
    None
}

fn note_clean_tool_args() {}

fn maybe_start_speculative(
    speculative_reads: &mut Vec<crate::speculative_read::SpeculativeRead>,
    assembler: &crate::tool_stream::ToolCallAssembler,
    tool_ctx: &whycodes_core::tool::ToolContext,
) {
    if let Some((cid, cname, buf)) = assembler.last_updated() {
        crate::speculative_read::maybe_start(speculative_reads, &cid, &cname, &buf, tool_ctx);
    }
}

fn skip_event() {}

/// Backoff before re-sending a step whose stream failed with `err`, or `None`
/// to fail the turn: retry is off (`window` zero), the error is not
/// transient, or `window` has passed since `first_drop`. `drops` is the
/// number of retries already spent in this turn.
fn stream_retry_delay(
    window: std::time::Duration,
    first_drop: Instant,
    drops: usize,
    err: &whycodes_core::Error,
) -> Option<std::time::Duration> {
    let remaining = window.checked_sub(first_drop.elapsed())?;
    if remaining.is_zero() || !whycodes_llm::classify(err).retryable {
        return None;
    }
    let backoff = whycodes_llm::RetryPolicy::default().backoff_for_attempt(drops + 1);
    Some(backoff.min(remaining))
}

/// Match `text` on `channel` against the stream rules. On a hit: abort
/// speculative reads, record the hit for `repeat`, and inject the hint. The
/// caller then discards the draft and retries the step.
fn check_stream_rules(
    rules: &[crate::stream_rules::StreamRule],
    channel: StreamChannel<'_>,
    text: &str,
    speculative_reads: &mut Vec<crate::speculative_read::SpeculativeRead>,
    events: &Option<crate::events::EventSink>,
    session: &mut whycodes_session::Session,
) -> bool {
    let Some(rule) = crate::stream_rules::first_hit(rules, channel, text, &session.stream_rules)
    else {
        return false;
    };
    let (name, hint) = (rule.name.as_str(), rule.hint.as_str());
    crate::speculative_read::abort_all(speculative_reads);
    emit(
        events,
        TurnEvent::Status(format!("Stream rule `{name}` interrupted the draft")),
    );
    session.stream_rules.record(name);
    session.add_user_message(&format!(
        "<whycodes_rule name=\"{name}\">\n{hint}\n\
         The previous draft was discarded. Continue without violating this rule.\n\
         </whycodes_rule>"
    ));
    true
}

/// [`check_stream_rules`] on the arguments of the tool call that just streamed.
fn check_tool_args_rules(
    rules: &[crate::stream_rules::StreamRule],
    assembler: &ToolCallAssembler,
    speculative_reads: &mut Vec<crate::speculative_read::SpeculativeRead>,
    events: &Option<crate::events::EventSink>,
    session: &mut whycodes_session::Session,
) -> bool {
    let Some((_, tool, args)) = assembler.last_updated() else {
        return false;
    };
    check_stream_rules(
        rules,
        StreamChannel::ToolArgs(&tool),
        &args,
        speculative_reads,
        events,
        session,
    )
}

fn transcript_has_tool_work(messages: &[whycodes_core::types::Message]) -> bool {
    messages.iter().any(|message| {
        if matches!(message.role, whycodes_core::types::Role::Tool) {
            return true;
        }
        let whycodes_core::types::MessageContent::Blocks(blocks) = &message.content else {
            return false;
        };
        blocks
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolUse { .. }))
    })
}

fn defs_include_swarm(defs: &[whycodes_core::types::ToolDefinition]) -> bool {
    defs.iter().any(|def| def.name == "swarm")
}

fn defs_without_swarm(
    defs: &[whycodes_core::types::ToolDefinition],
) -> Vec<whycodes_core::types::ToolDefinition> {
    defs.iter()
        .filter(|def| def.name != "swarm")
        .cloned()
        .collect()
}

/// Append the instruction files of a nested directory (its `AGENTS.md`, …)
/// to the successful `read` that first reached it this session.
///
/// Attached to the tool result, not the system prompt, so the cached prefix
/// stays byte-stable. See [`crate::context_files::nested_for`].
fn attach_nested_instructions(
    session: &mut Session,
    calls: &[whycodes_core::types::ToolCall],
    results: &mut [whycodes_core::types::ToolResult],
) {
    for result in results.iter_mut() {
        let path = read_path_for(calls, result);
        if path.is_empty() {
            continue;
        }
        let file = session.project_path.join(path);
        let found = crate::context_files::nested_for(
            &session.project_path,
            &file,
            &mut session.instruction_dirs,
        );
        if !found.is_empty() {
            result
                .content
                .push_str(&crate::context_files::render_nested(&found));
        }
    }
}

/// The `path` of the successful `read` call that produced `result`, or "".
fn read_path_for<'a>(
    calls: &'a [whycodes_core::types::ToolCall],
    result: &whycodes_core::types::ToolResult,
) -> &'a str {
    let mut path = "";
    for call in calls {
        if call.id == result.tool_call_id && call.name == "read" && !result.is_error {
            path = call.arguments["path"].as_str().unwrap_or("");
        }
    }
    path
}

/// Result body for a call the turn stopped before it could answer.
const NOT_RUN: &str = "Tool not run: the turn was stopped before this call finished.";

/// Continue after this many cut-off steps in a row, then accept the reply.
const MAX_CUT_OFFS: u32 = 3;

const CUT_OFF_NOTE: &str = "Your last response was cut off at the output token limit, so none of \
     its tool calls ran. Continue from where you stopped. Keep each response shorter: split a \
     large file write into several smaller edits.";

/// Whether a stream's stop reason says the output token limit cut it off:
/// `length` (OpenAI-compatible), `max_tokens` (Anthropic), `MAX_TOKENS` (Google).
fn is_output_limit_stop(delta: &serde_json::Value) -> bool {
    ["stop_reason", "finishReason"].iter().any(|key| {
        delta[key].as_str().is_some_and(|r| {
            r.eq_ignore_ascii_case("length") || r.eq_ignore_ascii_case("max_tokens")
        })
    })
}

/// Answer every call of an interrupted batch, then hand back `err`.
///
/// The assistant turn holding these calls is already in the transcript.
/// Strict APIs reject the next request if any call lacks a result (OpenAI:
/// tool_calls must be followed by tool messages; Anthropic: tool_use needs a
/// tool_result), so a cancel mid-tool used to break the following message.
fn close_unanswered_calls(
    session: &mut Session,
    calls: &[whycodes_core::types::ToolCall],
    err: whycodes_core::Error,
) -> whycodes_core::Error {
    let mut results = Vec::with_capacity(calls.len());
    for call in calls {
        results.push(whycodes_core::types::ToolResult {
            tool_call_id: call.id.clone(),
            content: NOT_RUN.to_string(),
            is_error: true,
        });
    }
    session.add_tool_results(results);
    err
}

#[cfg(test)]
#[path = "turn_tests.rs"]
mod tests;
