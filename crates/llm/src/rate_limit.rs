//! Rate limiting with a token-bucket algorithm for LLM API calls.
//!
//! Handles HTTP 429 responses by parsing `Retry-After` headers and provides
//! a token bucket rate limiter to proactively avoid hitting limits.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Parse a Retry-After header value (can be seconds as an integer or an HTTP date).
pub fn parse_retry_after(header_value: &str) -> Duration {
    // Try parsing as integer seconds first
    if let Ok(seconds) = header_value.trim().parse::<u64>() {
        return Duration::from_secs(seconds);
    }

    // Try HTTP date parsing (e.g., "Wed, 21 Oct 2015 07:28:00 GMT")
    if let Ok(date) = chrono::DateTime::parse_from_rfc2822(header_value.trim()) {
        let retry_time = date.with_timezone(&chrono::Utc);
        let now = chrono::Utc::now();
        let delta = retry_time.signed_duration_since(now);
        let seconds = delta.num_seconds().max(0) as u64;
        return Duration::from_secs(seconds);
    }

    // Default fallback
    Duration::from_secs(5)
}

/// Check if an HTTP status code indicates a rate limit (429 Too Many Requests).
pub fn is_rate_limited(status: u16) -> bool {
    status == 429
}

/// A simple token-bucket rate limiter.
///
/// Controls the rate of API calls to avoid hitting provider rate limits.
/// Thread-safe — can be shared across tasks.
pub struct RateLimiter {
    /// Requests per second capacity.
    rps: f64,
    /// Tokens available (fractional).
    tokens: Mutex<f64>,
    /// Last token refill time.
    last_refill: Mutex<Instant>,
    /// Maximum burst size (2x RPS).
    max_tokens: f64,
    /// Whether this limiter is paused due to a 429.
    paused: AtomicBool,
    /// When the pause ends.
    pause_until: Mutex<Option<Instant>>,
}

impl RateLimiter {
    /// Create a new rate limiter with the given requests-per-second capacity.
    pub fn new(rps: f64) -> Self {
        let max_tokens = (rps * 2.0).max(1.0);
        Self {
            rps,
            tokens: Mutex::new(max_tokens),
            last_refill: Mutex::new(Instant::now()),
            max_tokens,
            paused: AtomicBool::new(false),
            pause_until: Mutex::new(None),
        }
    }

    /// Acquire a token, returning how long to wait before proceeding.
    ///
    /// Returns `Duration::ZERO` if a token is immediately available.
    pub fn acquire(&self) -> Duration {
        // If paused due to a 429, return the remaining pause time
        if self.paused.load(Ordering::SeqCst) {
            let pause = *self.pause_until.lock().unwrap();
            if let Some(until) = pause {
                let now = Instant::now();
                if now < until {
                    return until - now;
                }
                // Pause expired
                self.paused.store(false, Ordering::SeqCst);
            }
        }

        let mut tokens = self.tokens.lock().unwrap();
        let mut last = self.last_refill.lock().unwrap();

        let now = Instant::now();
        let elapsed = now.duration_since(*last).as_secs_f64();

        // Refill tokens
        *tokens = (*tokens + elapsed * self.rps).min(self.max_tokens);
        *last = now;

        if *tokens >= 1.0 {
            *tokens -= 1.0;
            Duration::ZERO
        } else {
            // Calculate wait time until next token
            let wait = (1.0 - *tokens) / self.rps;
            Duration::from_secs_f64(wait)
        }
    }

    /// Pause the rate limiter after receiving a 429 response.
    ///
    /// `retry_after` should be the parsed `Retry-After` duration.
    pub fn pause(&self, retry_after: Duration) {
        let until = Instant::now() + retry_after;
        *self.pause_until.lock().unwrap() = Some(until);
        self.paused.store(true, Ordering::SeqCst);
    }

    /// Check if the limiter is currently paused.
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// Get the configured requests-per-second rate.
    pub fn rps(&self) -> f64 {
        self.rps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_retry_after_seconds() {
        assert_eq!(parse_retry_after("30"), Duration::from_secs(30));
        assert_eq!(parse_retry_after(" 60 "), Duration::from_secs(60));
    }

    #[test]
    fn test_parse_retry_after_fallback() {
        assert_eq!(parse_retry_after("invalid"), Duration::from_secs(5));
    }

    #[test]
    fn test_is_rate_limited() {
        assert!(is_rate_limited(429));
        assert!(!is_rate_limited(200));
        assert!(!is_rate_limited(500));
    }

    #[test]
    fn test_rate_limiter_acquire_fast() {
        let limiter = RateLimiter::new(1_000_000.0);
        assert_eq!(limiter.acquire(), Duration::ZERO);
    }

    #[test]
    fn test_rate_limiter_pause() {
        let limiter = RateLimiter::new(100.0);
        limiter.pause(Duration::from_secs(1));
        assert!(limiter.is_paused());
        let wait = limiter.acquire();
        assert!(wait > Duration::ZERO);
    }
}
