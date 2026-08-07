//! Production retry policy for LLM transport opens.
//!
//! - **Full jitter** exponential backoff (AWS architecture blog style)
//! - Honours **Retry-After** when classification finds it
//! - Structured tracing: attempt / kind / delay / error
//! - Hard cap on total wall time spent retrying
//!
//! Mid-stream failures are **not** retried here — only the operation you wrap
//! (typically `provider.stream` HTTP open or `provider.complete`).

use std::future::Future;
use std::time::{Duration, Instant};

use tokio::time::sleep;
use tracing::{info, warn};

use crate::error_class::{ClassifiedError, classify};

/// Default extra attempts after the first failure (4 tries total).
pub const DEFAULT_MAX_RETRIES: usize = 3;
/// Base backoff before the first retry.
pub const DEFAULT_BASE_DELAY_MS: u64 = 500;
/// Cap on a single backoff sleep.
pub const DEFAULT_MAX_DELAY_MS: u64 = 20_000;
/// Cap on cumulative time spent sleeping + failing (not counting success work).
pub const DEFAULT_MAX_ELAPSED_MS: u64 = 60_000;

/// Tunable retry policy.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Failures after the first try (0 = no retry).
    pub max_retries: usize,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
    /// Stop retrying after this much wall clock since the first attempt.
    pub max_elapsed: Duration,
    /// Full jitter: sleep uniform random in `[0, computed_backoff]`.
    pub full_jitter: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: DEFAULT_MAX_RETRIES,
            initial_backoff: Duration::from_millis(DEFAULT_BASE_DELAY_MS),
            max_backoff: Duration::from_millis(DEFAULT_MAX_DELAY_MS),
            max_elapsed: Duration::from_millis(DEFAULT_MAX_ELAPSED_MS),
            full_jitter: true,
        }
    }
}

impl RetryPolicy {
    /// Fast policy for unit tests (tiny delays).
    pub fn test_fast() -> Self {
        Self {
            max_retries: 3,
            initial_backoff: Duration::from_millis(5),
            max_backoff: Duration::from_millis(20),
            max_elapsed: Duration::from_secs(2),
            full_jitter: false,
        }
    }

    /// Backoff for attempt `n` (1-based failed attempt count before sleep).
    pub fn backoff_for_attempt(&self, failed_attempts: usize) -> Duration {
        let exp = failed_attempts.saturating_sub(1).min(16) as u32;
        let mult = 1u64.checked_shl(exp).unwrap_or(u64::MAX);
        let base_ms = self.initial_backoff.as_millis() as u64;
        let raw = base_ms.saturating_mul(mult);
        let capped = raw.min(self.max_backoff.as_millis() as u64);
        Duration::from_millis(capped.max(1))
    }

    /// Apply full jitter and optional Retry-After floor.
    pub fn sleep_duration(&self, failed_attempts: usize, classified: &ClassifiedError) -> Duration {
        let mut d = self.backoff_for_attempt(failed_attempts);
        if let Some(ra) = classified.retry_after {
            // Never sleep less than Retry-After when the provider asked us to wait.
            d = d.max(ra).min(self.max_backoff.max(ra));
        }
        if self.full_jitter {
            d = full_jitter(d);
        }
        d
    }
}

/// Full jitter: uniform random duration in `[0, max]`.
fn full_jitter(max: Duration) -> Duration {
    let max_ms = max.as_millis() as u64;
    if max_ms == 0 {
        return Duration::ZERO;
    }
    // Cheap non-crypto PRNG from time + address — fine for backoff jitter.
    let seed =
        Instant::now().elapsed().as_nanos() as u64 ^ (max_ms.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let r = seed.wrapping_mul(0xBF58_476D_1CE4_E5B9) >> 16;
    Duration::from_millis(r % (max_ms + 1))
}

/// Run `f` with [`RetryPolicy::default`].
pub async fn retry_with_backoff<F, Fut, T>(
    f: F,
    max_retries: usize,
    base_delay_ms: u64,
) -> whycode_core::Result<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = whycode_core::Result<T>>,
{
    let policy = RetryPolicy {
        max_retries,
        initial_backoff: Duration::from_millis(base_delay_ms),
        ..RetryPolicy::default()
    };
    execute_with_policy(&policy, "llm_call", f).await
}

/// Execute an async LLM open/complete with professional retry semantics.
///
/// `op` is a short label for logs (`stream_open`, `complete`, …).
pub async fn execute_with_policy<F, Fut, T>(
    policy: &RetryPolicy,
    op: &str,
    f: F,
) -> whycode_core::Result<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = whycode_core::Result<T>>,
{
    let started = Instant::now();
    let mut attempt: usize = 0;

    loop {
        attempt += 1;
        let attempt_t0 = Instant::now();
        match f().await {
            Ok(value) => {
                if attempt > 1 {
                    info!(
                        op,
                        attempt,
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        "LLM call succeeded after retry"
                    );
                }
                return Ok(value);
            }
            Err(e) => {
                let classified = classify(&e);
                let attempt_ms = attempt_t0.elapsed().as_millis() as u64;
                // `attempt` is 1-based try count; retries already used = attempt - 1.
                let allow = classified.retryable
                    && (attempt - 1) < policy.max_retries
                    && started.elapsed() < policy.max_elapsed;

                if !allow {
                    warn!(
                        op,
                        attempt,
                        kind = classified.kind.as_str(),
                        retryable = classified.retryable,
                        status = ?classified.status,
                        attempt_ms,
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        error = %e,
                        "LLM call failed (no more retries)"
                    );
                    return Err(e);
                }

                let delay = policy.sleep_duration(attempt, &classified);
                let remaining = policy.max_elapsed.saturating_sub(started.elapsed());
                let delay = delay.min(remaining);
                if remaining.is_zero() {
                    return Err(e);
                }

                warn!(
                    op,
                    attempt,
                    next_attempt = attempt + 1,
                    max_tries = policy.max_retries + 1,
                    kind = classified.kind.as_str(),
                    status = ?classified.status,
                    delay_ms = delay.as_millis() as u64,
                    attempt_ms,
                    error = %e,
                    "LLM call failed, retrying"
                );
                sleep(delay).await;
            }
        }
    }
}

/// Whether an error should be retried (delegates to classification).
pub fn is_retryable(err: &whycode_core::Error) -> bool {
    classify(err).retryable
}

/// Public message helper for tests and call sites.
pub fn is_retryable_message(msg: &str) -> bool {
    crate::error_class::classify_message(msg).retryable
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn retryable_omniroute_bracket_500_json() {
        let body = r#"{"error":{"message":"[500]: An internal server error occurred","type":"server_error","code":"internal_server_error"}}"#;
        assert!(is_retryable_message(body));
    }

    #[test]
    fn non_retryable_401() {
        assert!(!is_retryable_message(
            "Provider API error (401): Unauthorized"
        ));
    }

    #[test]
    fn backoff_grows_and_caps() {
        let p = RetryPolicy {
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_millis(1000),
            full_jitter: false,
            ..RetryPolicy::default()
        };
        assert_eq!(p.backoff_for_attempt(1), Duration::from_millis(100));
        assert_eq!(p.backoff_for_attempt(2), Duration::from_millis(200));
        assert_eq!(p.backoff_for_attempt(3), Duration::from_millis(400));
        assert_eq!(p.backoff_for_attempt(10), Duration::from_millis(1000));
    }

    #[tokio::test]
    async fn retries_then_succeeds() {
        let n = Arc::new(AtomicUsize::new(0));
        let c = n.clone();
        let policy = RetryPolicy::test_fast();
        let out = execute_with_policy(&policy, "test", || {
            let c = c.clone();
            async move {
                let i = c.fetch_add(1, Ordering::SeqCst);
                if i < 2 {
                    Err(whycode_core::Error::Llm(
                        "API error (503): unavailable".into(),
                    ))
                } else {
                    Ok(42)
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(out, 42);
        assert_eq!(n.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn does_not_retry_client_errors() {
        let n = Arc::new(AtomicUsize::new(0));
        let c = n.clone();
        let policy = RetryPolicy::test_fast();
        let err = execute_with_policy(&policy, "test", || {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>(whycode_core::Error::Llm(
                    "API error (400): bad request".into(),
                ))
            }
        })
        .await
        .unwrap_err();
        assert!(err.to_string().contains("400"));
        assert_eq!(n.load(Ordering::SeqCst), 1);
    }
}
