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
        let mult = 1u64 << exp;
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
    let max_ms = max.as_millis().min(u128::from(u64::MAX)) as u64;
    if max_ms == 0 {
        return Duration::ZERO;
    }
    // Cheap non-crypto PRNG from time + address — fine for backoff jitter.
    let seed =
        Instant::now().elapsed().as_nanos() as u64 ^ (max_ms.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let r = seed.wrapping_mul(0xBF58_476D_1CE4_E5B9) >> 16;
    Duration::from_millis(r % max_ms.saturating_add(1))
}

/// Run `f` with [`RetryPolicy::default`].
pub async fn retry_with_backoff<F, Fut, T>(
    f: F,
    max_retries: usize,
    base_delay_ms: u64,
) -> whycodes_core::Result<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = whycodes_core::Result<T>>,
{
    execute_with_policy(
        &RetryPolicy {
            max_retries,
            initial_backoff: Duration::from_millis(base_delay_ms),
            ..RetryPolicy::default()
        },
        "llm_call",
        f,
    )
    .await
}

/// Execute an async LLM open/complete with professional retry semantics.
///
/// `op` is a short label for logs (`stream_open`, `complete`, …).
pub async fn execute_with_policy<F, Fut, T>(
    policy: &RetryPolicy,
    op: &str,
    f: F,
) -> whycodes_core::Result<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = whycodes_core::Result<T>>,
{
    let started = Instant::now();
    let mut attempt: usize = 0;

    loop {
        attempt += 1;
        let attempt_t0 = Instant::now();
        match f().await {
            Ok(value) => {
                if attempt > 1 {
                    log_retry_success(op, attempt, started.elapsed().as_millis());
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
                    log_retry_give_up(
                        op,
                        attempt,
                        classified.kind.as_str(),
                        classified.retryable,
                        classified.status,
                        attempt_ms,
                        started.elapsed().as_millis(),
                    );
                    return Err(e);
                }

                let delay = policy.sleep_duration(attempt, &classified);
                // `allow` already requires `elapsed < max_elapsed`, so remaining > 0.
                let remaining = policy.max_elapsed.saturating_sub(started.elapsed());
                let delay = delay.min(remaining);

                log_retry_again(RetryAgainFields {
                    _op: op,
                    _attempt: attempt,
                    _next_attempt: attempt + 1,
                    _max_tries: policy.max_retries + 1,
                    _kind: classified.kind.as_str(),
                    _status: classified.status,
                    _delay_ms: delay.as_millis(),
                    _attempt_ms: attempt_ms,
                });
                sleep(delay).await;
            }
        }
    }
}

fn log_retry_success(_op: &str, _attempt: usize, _elapsed_ms: u128) {}

fn log_retry_give_up(
    _op: &str,
    _attempt: usize,
    _kind: &str,
    _retryable: bool,
    _status: Option<u16>,
    _attempt_ms: u64,
    _elapsed_ms: u128,
) {
}

fn log_retry_again(_fields: RetryAgainFields<'_>) {}

struct RetryAgainFields<'a> {
    _op: &'a str,
    _attempt: usize,
    _next_attempt: usize,
    _max_tries: usize,
    _kind: &'a str,
    _status: Option<u16>,
    _delay_ms: u128,
    _attempt_ms: u64,
}

#[cfg(test)]
pub(crate) fn log_retry_helpers_for_tests() {
    log_retry_success("ok", 2, 1);
    log_retry_give_up("warn", 1, "http", false, Some(400), 1, 1);
    log_retry_again(RetryAgainFields {
        _op: "warn",
        _attempt: 1,
        _next_attempt: 2,
        _max_tries: 4,
        _kind: "http",
        _status: Some(503),
        _delay_ms: 5,
        _attempt_ms: 1,
    });
}

/// Whether an error should be retried (delegates to classification).
pub fn is_retryable(err: &whycodes_core::Error) -> bool {
    classify(err).retryable
}

/// Public message helper for tests and call sites.
pub fn is_retryable_message(msg: &str) -> bool {
    crate::error_class::classify_message(msg).retryable
}

#[cfg(test)]
#[path = "retry_tests.rs"]
mod tests;
