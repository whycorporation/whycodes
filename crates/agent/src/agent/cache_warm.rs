//! Keep the provider's prompt cache warm while the user is idle.
//!
//! Providers drop a cached prefix a few minutes after its last use, so the
//! first turn after a pause pays full price for the whole history again.
//! A keep-alive repeats the last step's request (same tools, system prompt
//! and history, without the reply it produced) and hangs up as soon as the
//! provider reports usage: the prefix is read, its TTL restarts, and only a
//! few output tokens are generated. Opt-in through
//! `session.cache_warm_minutes`, since every keep-alive is a paid request.

use std::time::Duration;

use futures::StreamExt;
use whycodes_core::types::{Role, StreamEvent, Usage};
use whycodes_session::session::Session;

use super::{Agent, append_turn_context};

/// Gap between keep-alives: inside the common five-minute cache TTL.
pub const WARM_EVERY: Duration = Duration::from_secs(270);

/// Whether a keep-alive is due. `ended` is when the last turn finished,
/// `warmed` the last keep-alive; none goes out once the idle `window` is over.
pub fn warm_due(
    now: std::time::Instant,
    ended: std::time::Instant,
    warmed: Option<std::time::Instant>,
    window: Duration,
) -> bool {
    let since = warmed.unwrap_or(ended).max(ended);
    !window.is_zero()
        && now.duration_since(since) >= WARM_EVERY
        && now.duration_since(ended) < window
}

impl Agent {
    /// A keep-alive for `session`'s cached prefix, as a future that owns what
    /// it needs (spawn it). `None` when prompt caching is off or there is no
    /// history to keep. The future yields the usage the provider reported.
    pub fn cache_warm_job(
        &self,
        session: &Session,
        provider_name: &str,
        model: &str,
        api_key: &str,
    ) -> Option<impl std::future::Future<Output = Option<Usage>> + Send + 'static> {
        if !self.use_prompt_cache {
            return None;
        }
        let mut request =
            session.build_request(self.step_tools(), None, self.info.temperature, Some(true));
        // The last step's request ended before the reply it produced.
        let keep = request
            .messages
            .iter()
            .rposition(|m| m.role != Role::Assistant)?;
        request.messages = std::sync::Arc::from(&request.messages[..=keep]);
        append_turn_context(&mut request, session.turn_context.as_deref());
        request.use_prompt_cache = true;
        crate::thinking_acc::attach_thinking_request(
            &mut request,
            provider_name,
            model,
            self.info.model.as_ref(),
            self.reasoning_effort.as_deref(),
        );
        let registry = std::sync::Arc::clone(&self.provider_registry);
        let (provider_name, model, api_key) = (
            provider_name.to_string(),
            model.to_string(),
            api_key.to_string(),
        );
        Some(async move {
            let provider = registry.get(&provider_name)?;
            let transport = whycodes_llm::default_transport();
            let stream = transport.stream(provider, &request, &api_key, &model).await;
            let mut stream = stream
                .map_err(|e| tracing::debug!(error = %e, "cache keep-alive failed"))
                .ok()?;
            let mut usage = Usage::default();
            while let Some(Ok(event)) = stream.next().await {
                match event {
                    StreamEvent::Usage {
                        input_tokens,
                        output_tokens,
                    } => {
                        usage.input_tokens += input_tokens;
                        usage.output_tokens += output_tokens;
                    }
                    StreamEvent::CacheUsage {
                        creation_input_tokens,
                        read_input_tokens,
                    } => {
                        usage.cache_creation_input_tokens = Some(creation_input_tokens);
                        usage.cache_read_input_tokens = Some(read_input_tokens);
                    }
                    // The prefix is processed once output starts; hang up.
                    _ => break,
                }
            }
            Some(usage)
        })
    }

    /// Fold a keep-alive's usage and price into the session totals.
    pub fn record_cache_warm(
        &self,
        session: &mut Session,
        provider: &str,
        model: &str,
        usage: &Usage,
    ) {
        session.add_usage(usage);
        session.cost.add(self.step_cost(provider, model, usage));
    }
}

#[cfg(test)]
#[path = "cache_warm_tests.rs"]
mod tests;
