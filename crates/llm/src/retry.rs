//! Retry utility with exponential backoff for LLM provider calls.
//!
//! Retries on rate limit (HTTP 429) and server errors (5xx), including common
//! OpenAI-compatible proxy shapes (`(500)`, `[500]`, `server_error`, …).

use std::future::Future;
use std::time::Duration;
use tokio::time::sleep;
use tracing::warn;

/// Default attempts after the first failure (total tries = max_retries + 1).
pub const DEFAULT_MAX_RETRIES: usize = 3;
/// Base delay before the first retry; doubles each time (1s → 2s → 4s).
pub const DEFAULT_BASE_DELAY_MS: u64 = 1000;

/// Retry an async operation with exponential backoff.
///
/// Retries only when [`is_retryable`] says so (429 / 5xx / server_error).
///
/// - `max_retries`: Extra attempts after the first failure (3 ⇒ up to 4 tries).
/// - `base_delay_ms`: First backoff in ms; doubles each retry.
pub async fn retry_with_backoff<F, Fut, T>(
    f: F,
    max_retries: usize,
    base_delay_ms: u64,
) -> whycode_core::Result<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = whycode_core::Result<T>>,
{
    let mut attempt = 0;
    let mut delay = base_delay_ms;

    loop {
        attempt += 1;
        match f().await {
            Ok(result) => return Ok(result),
            Err(e) => {
                if attempt > max_retries || !is_retryable(&e) {
                    return Err(e);
                }
                warn!(
                    attempt,
                    max_tries = max_retries + 1,
                    delay_ms = delay,
                    error = %e,
                    "LLM call failed, retrying"
                );
                sleep(Duration::from_millis(delay)).await;
                delay = delay.saturating_mul(2);
            }
        }
    }
}

/// Whether an error should be retried (rate limit or server error).
pub fn is_retryable(err: &whycode_core::Error) -> bool {
    is_retryable_message(&err.to_string())
}

/// Public so tests and call sites can unit-check the same rules as production.
pub fn is_retryable_message(msg: &str) -> bool {
    let lower = msg.to_ascii_lowercase();

    // Explicit OpenAI-style codes (proxies often nest these in JSON bodies).
    if lower.contains("\"code\":\"internal_server_error\"")
        || lower.contains("\"type\":\"server_error\"")
        || lower.contains("internal_server_error")
        || lower.contains("server_error")
        || lower.contains("service unavailable")
        || lower.contains("temporarily unavailable")
        || lower.contains("overloaded")
        || lower.contains("rate limit")
        || lower.contains("too many requests")
    {
        // Don't retry auth / client mistakes even if message is noisy.
        if has_client_status(&lower) {
            return false;
        }
        return true;
    }

    has_status_code(msg, 429) || (500..600).any(|code| has_status_code(msg, code))
}

fn has_client_status(lower: &str) -> bool {
    // Hard client errors that must not loop.
    [400, 401, 403, 404, 422].iter().any(|&c| {
        let s = c.to_string();
        lower.contains(&format!("({s})"))
            || lower.contains(&format!("[{s}]"))
            || lower.contains(&format!(" {s} "))
            || lower.contains(&format!(" {s}:"))
            || lower.contains(&format!("status {s}"))
            || lower.contains(&format!("status: {s}"))
            || lower.contains(&format!("http {s}"))
    })
}

/// Detect HTTP status in several wire formats:
/// `(500)`, `[500]`, ` 500:`, `status 500`, `HTTP 500`.
fn has_status_code(msg: &str, code: u16) -> bool {
    let s = code.to_string();
    let patterns = [
        format!("({s})"),
        format!("[{s}]"),
        format!(" {s}:"),
        format!(":{s} "),
        format!("status {s}"),
        format!("status: {s}"),
        format!("http {s}"),
        format!("http/{s}"),
        // Bare JSON-ish: "status":500
        format!("\"status\":{s}"),
        format!("\"status\": {s}"),
        format!("\"code\":{s}"),
        format!("\"code\": {s}"),
    ];
    patterns.iter().any(|p| {
        if p.chars().all(|c| c.is_ascii_digit() || c.is_ascii_whitespace()) {
            return false;
        }
        msg.to_ascii_lowercase()
            .contains(&p.to_ascii_lowercase())
            || msg.contains(p.as_str())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_429_parens() {
        let err = whycode_core::Error::Llm("OpenAI API error (429): Rate limit".into());
        assert!(is_retryable(&err));
    }

    #[test]
    fn retryable_503_parens() {
        let err = whycode_core::Error::Llm("Provider API error (503): Service unavailable".into());
        assert!(is_retryable(&err));
    }

    #[test]
    fn retryable_omniroute_bracket_500_json() {
        // Real omniroute / OpenAI-compat proxy body whycode was failing on.
        let body = r#"{"error":{"message":"[500]: An internal server error occurred","type":"server_error","code":"internal_server_error"}}"#;
        assert!(
            is_retryable_message(body),
            "must retry omniroute-style [500] + server_error JSON"
        );
        let err = whycode_core::Error::Llm(body.into());
        assert!(is_retryable(&err));
    }

    #[test]
    fn retryable_bare_status_field() {
        assert!(is_retryable_message(r#"upstream failed "status":502"#));
    }

    #[test]
    fn non_retryable_400() {
        let err = whycode_core::Error::Llm("Provider API error (400): Bad request".into());
        assert!(!is_retryable(&err));
    }

    #[test]
    fn non_retryable_401() {
        let err = whycode_core::Error::Llm("Provider API error (401): Unauthorized".into());
        assert!(!is_retryable(&err));
    }

    #[test]
    fn client_error_wins_over_server_error_noise() {
        // If both appear, prefer not looping forever on bad requests.
        assert!(!is_retryable_message(
            r#"{"error":{"message":"(401) invalid key","type":"server_error"}}"#
        ));
    }
}
