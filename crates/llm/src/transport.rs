//! High-level LLM transport: one place for policy, retry, and open semantics.
//!
//! Call sites (agent, subagent, title, memory) should use these helpers instead
//! of invoking `provider.stream` / `complete` bare — keeps behaviour uniform.

use std::pin::Pin;
use std::time::Duration;

use futures::Stream;
use tracing::debug;
use whycodes_core::types::{LlmRequest, LlmResponse, StreamEvent};

use crate::error_class::{ClassifiedError, classify};
use crate::provider::LlmProvider;
use crate::race::{EventStream, RaceOutcome, StreamTarget, stream_raced};
use crate::response_cache::{ResponseCache, text_only_response};
use crate::retry::{RetryPolicy, execute_with_policy};

/// Bundle of transport defaults used across the product.
#[derive(Debug, Clone)]
pub struct LlmTransport {
    pub retry: RetryPolicy,
    /// Optional wall-clock timeout around a **complete** call (not stream body).
    pub complete_timeout: Option<Duration>,
}

impl Default for LlmTransport {
    fn default() -> Self {
        Self {
            retry: RetryPolicy::default(),
            // Completions (title, retain) stay bounded; agent stream has no outer cap.
            complete_timeout: Some(Duration::from_secs(120)),
        }
    }
}

impl LlmTransport {
    pub fn with_retry(mut self, policy: RetryPolicy) -> Self {
        self.retry = policy;
        self
    }

    /// Open a streaming response with retry on the HTTP open only.
    pub async fn stream(
        &self,
        provider: &dyn LlmProvider,
        request: &LlmRequest,
        api_key: &str,
        model: &str,
    ) -> whycodes_core::Result<Pin<Box<dyn Stream<Item = whycodes_core::Result<StreamEvent>> + Send>>>
    {
        let name = provider.name();
        let max_retries = self.retry.max_retries;
        debug!("llm.stream_open provider={name} model={model} max_retries={max_retries}");
        execute_with_policy(&self.retry, "stream_open", || {
            provider.stream(request, api_key, model)
        })
        .await
    }

    /// Non-streaming completion with retry + optional timeout.
    ///
    /// Tools-free requests consult the process-local response cache (exact,
    /// then semantic) so title/compact/retain retries skip a second prefill.
    pub async fn complete(
        &self,
        provider: &dyn LlmProvider,
        request: &LlmRequest,
        api_key: &str,
        model: &str,
    ) -> whycodes_core::Result<LlmResponse> {
        if let Some(hit) = ResponseCache::global().lookup(request, model) {
            debug!("llm.complete_cache_hit model={model}");
            return Ok(ResponseCache::to_response(&hit, model));
        }
        let name = provider.name();
        let max_retries = self.retry.max_retries;
        debug!("llm.complete provider={name} model={model} max_retries={max_retries}");
        let retry = self.retry.clone();
        let timeout = self.complete_timeout;

        let work = execute_with_policy(&retry, "complete", || {
            provider.complete(request, api_key, model)
        });

        let resp = match timeout {
            Some(t) => match tokio::time::timeout(t, work).await {
                Ok(r) => r,
                Err(elapsed) => Err(whycodes_core::Error::llm_kind(
                    whycodes_core::ErrorKind::Timeout,
                    format!("complete timed out after {}s ({elapsed})", t.as_secs()),
                )),
            },
            None => work.await,
        }?;
        if let Some(text) = text_only_response(&resp) {
            ResponseCache::global().store(request, model, &text);
        }
        Ok(resp)
    }

    /// Stream a turn: optional response-cache replay, then first-token race.
    pub async fn stream_turn(
        &self,
        primary: StreamTarget<'_>,
        request: &LlmRequest,
        opts: StreamTurnOpts<'_>,
    ) -> whycodes_core::Result<StreamTurn> {
        if opts.cache
            && let Some(hit) = ResponseCache::global().lookup(request, primary.model)
        {
            debug!("llm.stream_cache_hit model={}", primary.model);
            let text = hit.text;
            let events: EventStream = Box::pin(CachedReplay {
                text: Some(text),
                stop: false,
            });
            return Ok(StreamTurn {
                events,
                cache_hit: true,
                race: RaceOutcome::PrimaryOnly,
            });
        }

        let (events, race) =
            stream_raced(self, primary, opts.race, request, opts.race_after).await?;
        Ok(StreamTurn {
            events,
            cache_hit: false,
            race,
        })
    }
}

/// Options for [`LlmTransport::stream_turn`].
pub struct StreamTurnOpts<'a> {
    pub cache: bool,
    pub race: Option<StreamTarget<'a>>,
    pub race_after: Duration,
}

/// Opened turn stream plus how it was sourced.
pub struct StreamTurn {
    pub events: EventStream,
    pub cache_hit: bool,
    pub race: RaceOutcome,
}

struct CachedReplay {
    text: Option<String>,
    stop: bool,
}

impl Stream for CachedReplay {
    type Item = whycodes_core::Result<StreamEvent>;

    fn poll_next(
        self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if let Some(text) = this.text.take() {
            return std::task::Poll::Ready(Some(Ok(StreamEvent::TextDelta { text })));
        }
        if !this.stop {
            this.stop = true;
            return std::task::Poll::Ready(Some(Ok(StreamEvent::MessageStop)));
        }
        std::task::Poll::Ready(None)
    }
}

/// Global default transport (cheap to construct; no shared state yet).
pub fn default_transport() -> LlmTransport {
    LlmTransport::default()
}

/// Classify and rephrase an error for UI display without losing the raw string
/// in logs (caller still logs the original).
pub fn user_facing_error(err: &whycodes_core::Error) -> String {
    let c: ClassifiedError = classify(err);
    // Prefer clean copy; append short kind tag for power users.
    let base = c.user_message();
    if c.kind.as_str() == "unknown" || base.contains(c.kind.as_str()) {
        base
    } else {
        base.to_string()
    }
}

/// Richer line for turn errors: clean summary + optional detail suffix.
pub fn format_turn_error(err: &whycodes_core::Error) -> String {
    let c = classify(err);
    let summary = c.user_message();
    // If classification already cleaned it, use that; else keep original Llm payload trimmed.
    match c.kind {
        crate::error_class::ErrorKind::Unknown => {
            // Strip redundant "LLM error: " prefix if present.
            let s = err.to_string();
            s.strip_prefix("LLM error: ").unwrap_or(&s).to_string()
        }
        _ => summary,
    }
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
