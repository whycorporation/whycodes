/// Retry utility with exponential backoff for LLM provider calls.
///
/// Only retries on rate limit (HTTP 429) and server errors (5xx).
use std::future::Future;
use std::time::Duration;
use tokio::time::sleep;
use tracing::warn;

/// Retry an async operation with exponential backoff.
///
/// Retries only on rate limit (429) and server errors (5xx),
/// provided the error is an `Llm` error containing one of those status codes.
///
/// - `max_retries`: Maximum number of retry attempts (default: 3).
/// - `base_delay_ms`: Base delay in milliseconds, doubles each retry (default: 1000).
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
                    "LLM call failed (attempt {}/{}), retrying in {}ms: {e}",
                    attempt,
                    max_retries + 1,
                    delay,
                );
                sleep(Duration::from_millis(delay)).await;
                delay *= 2; // Exponential backoff: 1s, 2s, 4s, 8s, ...
            }
        }
    }
}

/// Check whether an error is retryable (rate limit or server error).
fn is_retryable(err: &whycode_core::Error) -> bool {
    let msg = err.to_string();
    // Check for status codes 429 (rate limit) and 5xx (server errors)
    has_status_code(&msg, 429) || (500..600).any(|code| has_status_code(&msg, code))
}

/// Check if a string contains a given HTTP status code in the format "(NNN)".
fn has_status_code(msg: &str, code: u16) -> bool {
    let pattern = format!("({})", code);
    msg.contains(&pattern)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retryable_429() {
        let err = whycode_core::Error::Llm("OpenAI API error (429): Rate limit".to_string());
        assert!(is_retryable(&err));
    }

    #[test]
    fn test_retryable_503() {
        let err =
            whycode_core::Error::Llm("Provider API error (503): Service unavailable".to_string());
        assert!(is_retryable(&err));
    }

    #[test]
    fn test_non_retryable_400() {
        let err = whycode_core::Error::Llm("Provider API error (400): Bad request".to_string());
        assert!(!is_retryable(&err));
    }

    #[test]
    fn test_non_retryable_401() {
        let err = whycode_core::Error::Llm("Provider API error (401): Unauthorized".to_string());
        assert!(!is_retryable(&err));
    }
}
