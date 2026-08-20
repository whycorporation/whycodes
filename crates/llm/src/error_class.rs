//! Structured classification of LLM / HTTP transport failures.
//!
//! Providers and proxies return a mess of shapes: `(500)`, `[500]`, nested JSON
//! `server_error`, raw connection strings. Classification is the single source
//! of truth for **retry** and **user-facing** copy.

use std::time::Duration;

use whycode_core::Error;

use crate::rate_limit::parse_retry_after;

/// Coarse kind of an LLM failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// HTTP 429 / explicit rate limit language.
    RateLimited,
    /// HTTP 5xx or proxy `server_error` / overloaded.
    Server,
    /// DNS, TCP, TLS, connection reset, "error sending request".
    Network,
    /// Client-side deadline exceeded.
    Timeout,
    /// HTTP 401 / 403 / missing key language.
    Auth,
    /// HTTP 400 / 404 / 422 — do not retry.
    Client,
    /// User or agent cancelled the turn.
    Cancelled,
    /// Unclassified.
    Unknown,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RateLimited => "rate_limited",
            Self::Server => "server",
            Self::Network => "network",
            Self::Timeout => "timeout",
            Self::Auth => "auth",
            Self::Client => "client",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
        }
    }
}

/// Classified error with retry policy hints.
#[derive(Debug, Clone)]
pub struct ClassifiedError {
    pub kind: ErrorKind,
    /// Whether another attempt may succeed without changing the request.
    pub retryable: bool,
    /// Preferred wait before the next attempt (from `Retry-After` or body).
    pub retry_after: Option<Duration>,
    /// Parsed HTTP status when present in the message.
    pub status: Option<u16>,
    /// Original display string (unchanged for logs).
    pub message: String,
}

impl ClassifiedError {
    /// Short, user-facing line for TUI / CLI (no raw JSON dump when possible).
    pub fn user_message(&self) -> String {
        match self.kind {
            ErrorKind::RateLimited => {
                if let Some(d) = self.retry_after {
                    format!("Rate limited — retry in {}s", d.as_secs().max(1))
                } else {
                    "Rate limited by the provider".into()
                }
            }
            ErrorKind::Server => {
                if let Some(s) = self.status {
                    format!("Provider server error (HTTP {s})")
                } else {
                    "Provider server error".into()
                }
            }
            ErrorKind::Network => "Network error reaching the provider".into(),
            ErrorKind::Timeout => "Request timed out".into(),
            ErrorKind::Auth => {
                let lower = self.message.to_ascii_lowercase();
                if lower.contains("xai") {
                    return "xAI authentication failed — run `whycode auth login xai` (or set XAI_API_KEY)".into();
                }
                if lower.contains("not eligible") && lower.contains("code assist") {
                    // Gemini Code Assist free-tier eligibility is account-based;
                    // the credential is fine, so "check API key" misleads.
                    return "Google account is not eligible for Gemini Code Assist (free tier) — use an AI Studio API key (GOOGLE_API_KEY) or a different account".into();
                }
                if self.status == Some(403) {
                    // A 403 usually carries the actionable reason (plan, region,
                    // eligibility) in the provider body — surface it.
                    let m = self.message.trim().trim_start_matches("LLM error:").trim();
                    let snippet = if m.len() > 160 {
                        format!("{}…", m.chars().take(159).collect::<String>())
                    } else {
                        m.to_string()
                    };
                    return format!("Forbidden by the provider (HTTP 403): {snippet}");
                }
                "Authentication failed — check API key".into()
            }
            ErrorKind::Client => {
                if let Some(s) = self.status {
                    format!("Request rejected (HTTP {s})")
                } else {
                    "Request rejected by the provider".into()
                }
            }
            ErrorKind::Cancelled => "Cancelled".into(),
            ErrorKind::Unknown => {
                // Keep a compact snippet of the original message.
                let m = self.message.trim();
                if m.len() > 160 {
                    format!("{}…", m.chars().take(159).collect::<String>())
                } else if m.is_empty() {
                    "LLM request failed".into()
                } else {
                    m.to_string()
                }
            }
        }
    }
}

/// Classify any [`Error`] that may wrap an LLM transport failure.
pub fn classify(err: &Error) -> ClassifiedError {
    classify_message(&err.to_string())
}

/// Classify a free-form error string (provider body, reqwest text, …).
pub fn classify_message(raw: &str) -> ClassifiedError {
    let message = raw.to_string();
    let lower = raw.to_ascii_lowercase();
    let status = extract_http_status(raw);
    let retry_after = extract_retry_after(raw);

    // Cancellation first.
    if lower.contains("cancelled") || lower.contains("canceled") {
        return ClassifiedError {
            kind: ErrorKind::Cancelled,
            retryable: false,
            retry_after: None,
            status,
            message,
        };
    }

    // Auth before generic 4xx.
    if matches!(status, Some(401 | 403))
        || lower.contains("unauthorized")
        || lower.contains("forbidden")
        || lower.contains("invalid api key")
        || lower.contains("incorrect api key")
        || lower.contains("authentication")
            && (lower.contains("fail") || lower.contains("error") || lower.contains("invalid"))
    {
        return ClassifiedError {
            kind: ErrorKind::Auth,
            retryable: false,
            retry_after: None,
            status,
            message,
        };
    }

    // Rate limit.
    if status == Some(429)
        || lower.contains("rate limit")
        || lower.contains("too many requests")
        || lower.contains("\"code\":\"rate_limit")
        || lower.contains("rate_limit_exceeded")
    {
        return ClassifiedError {
            kind: ErrorKind::RateLimited,
            retryable: true,
            retry_after: retry_after.or(Some(Duration::from_secs(5))),
            status: status.or(Some(429)),
            message,
        };
    }

    // Timeout.
    if lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("deadline exceeded")
        || lower.contains("operation timed out")
    {
        return ClassifiedError {
            kind: ErrorKind::Timeout,
            retryable: true,
            retry_after: None,
            status,
            message,
        };
    }

    // Network / transport (reqwest often: "error sending request for url").
    if lower.contains("error sending request")
        || lower.contains("connection refused")
        || lower.contains("connection reset")
        || lower.contains("connection closed")
        || lower.contains("broken pipe")
        || lower.contains("network unreachable")
        || lower.contains("name or service not known")
        || lower.contains("dns error")
        || lower.contains("tls handshake")
        || lower.contains("certificate")
        || lower.contains("tcp connect")
        || lower.contains("temporarily unavailable") && status.is_none()
    {
        return ClassifiedError {
            kind: ErrorKind::Network,
            retryable: true,
            retry_after: None,
            status,
            message,
        };
    }

    // Server errors (5xx + proxy language).
    let looks_server = matches!(status, Some(s) if (500..600).contains(&s))
        || lower.contains("\"type\":\"server_error\"")
        || lower.contains("\"code\":\"internal_server_error\"")
        || lower.contains("internal_server_error")
        || lower.contains("\"type\":\"api_error\"")
        || lower.contains("overloaded")
        || lower.contains("service unavailable")
        || lower.contains("bad gateway")
        || lower.contains("gateway timeout");

    if looks_server {
        // Hard client status wins if both appear (noisy bodies).
        if matches!(status, Some(s) if (400..500).contains(&s) && s != 429) {
            return ClassifiedError {
                kind: ErrorKind::Client,
                retryable: false,
                retry_after: None,
                status,
                message,
            };
        }
        return ClassifiedError {
            kind: ErrorKind::Server,
            retryable: true,
            retry_after,
            status,
            message,
        };
    }

    // Other 4xx.
    if matches!(status, Some(s) if (400..500).contains(&s)) {
        return ClassifiedError {
            kind: ErrorKind::Client,
            retryable: false,
            retry_after: None,
            status,
            message,
        };
    }

    ClassifiedError {
        kind: ErrorKind::Unknown,
        retryable: false,
        retry_after: None,
        status,
        message,
    }
}

/// Extract an HTTP status code from common wire formats.
pub fn extract_http_status(msg: &str) -> Option<u16> {
    // Prefer explicit patterns over bare digits.
    let patterns: &[(char, char)] = &[('(', ')'), ('[', ']')];
    for (open, close) in patterns {
        let mut rest = msg;
        while let Some(start) = rest.find(*open) {
            let after = &rest[start + 1..];
            if let Some(end) = after.find(*close) {
                // Accept a leading numeric code with optional text: "(403)",
                // "(403 Forbidden)". Non-numeric parens are skipped.
                let inner = after[..end].trim();
                let digits: String = inner.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(code) = digits.parse::<u16>()
                    && (100..600).contains(&code)
                {
                    return Some(code);
                }
                rest = &after[end + 1..];
            } else {
                break;
            }
        }
    }

    // "HTTP 502", "status: 503", "status 500"
    for prefix in ["http ", "status:", "status ", "status code "] {
        if let Some(idx) = msg.to_ascii_lowercase().find(prefix) {
            let after = msg[idx + prefix.len()..].trim_start();
            let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(code) = digits.parse::<u16>()
                && (100..600).contains(&code)
            {
                return Some(code);
            }
        }
    }

    // JSON "status": 502
    for key in ["\"status\":", "\"code\":"] {
        if let Some(idx) = msg.find(key) {
            let after = msg[idx + key.len()..].trim_start();
            // Skip quoted string codes like "internal_server_error"
            if after.starts_with('"') {
                continue;
            }
            let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(code) = digits.parse::<u16>()
                && (100..600).contains(&code)
            {
                return Some(code);
            }
        }
    }

    // Leading "[500]:" style already handled; also "500 Internal"
    let trimmed = msg.trim_start();
    if trimmed.len() >= 3 {
        let head: String = trimmed.chars().take(3).collect();
        if let Ok(code) = head.parse::<u16>()
            && (100..600).contains(&code)
        {
            let next = trimmed.chars().nth(3);
            if next.is_none_or(|c| c == ' ' || c == ':' || c == ',') {
                return Some(code);
            }
        }
    }

    None
}

fn extract_retry_after(msg: &str) -> Option<Duration> {
    // Header-ish: "retry-after: 12" or "Retry-After: Wed, …"
    let lower = msg.to_ascii_lowercase();
    if let Some(idx) = lower.find("retry-after") {
        let after = msg[idx + "retry-after".len()..].trim_start();
        let after = after.strip_prefix(':').unwrap_or(after).trim_start();
        // Take until newline or comma-ish end of value.
        let value: String = after
            .chars()
            .take_while(|c| *c != '\n' && *c != '\r' && *c != '"' && *c != '}')
            .collect();
        let value = value.trim();
        if !value.is_empty() {
            return Some(parse_retry_after(value));
        }
    }
    // "retry in 5 seconds" / "wait 10s"
    for (pat, mult) in [("retry in ", 1u64), ("wait ", 1u64)] {
        if let Some(idx) = lower.find(pat) {
            let after = &lower[idx + pat.len()..];
            let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = digits.parse::<u64>() {
                if after.contains("ms") {
                    return Some(Duration::from_millis(n * mult));
                }
                return Some(Duration::from_secs(n * mult));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_omniroute_500_json() {
        let body = r#"{"error":{"message":"[500]: An internal server error occurred","type":"server_error","code":"internal_server_error"}}"#;
        let c = classify_message(body);
        assert_eq!(c.kind, ErrorKind::Server);
        assert!(c.retryable);
        assert_eq!(c.status, Some(500));
    }

    #[test]
    fn classifies_429() {
        let c = classify_message("OpenAI API error (429): Rate limit exceeded");
        assert_eq!(c.kind, ErrorKind::RateLimited);
        assert!(c.retryable);
    }

    #[test]
    fn classifies_network() {
        let c = classify_message(
            "LLM error: models list HTTP: error sending request for url (http://x/v1/models)",
        );
        assert_eq!(c.kind, ErrorKind::Network);
        assert!(c.retryable);
    }

    #[test]
    fn auth_403_surfaces_the_provider_reason() {
        let c = classify_message(
            "LLM error: Code Assist onboardUser (403 Forbidden): account lacks region X",
        );
        assert_eq!(c.kind, ErrorKind::Auth);
        let msg = c.user_message();
        assert!(msg.contains("403"), "{msg}");
        assert!(msg.contains("account lacks region X"), "{msg}");
        assert!(!msg.contains("check API key"), "{msg}");
    }

    #[test]
    fn code_assist_ineligibility_gets_actionable_copy() {
        let c = classify_message(
            "LLM error: Code Assist onboardUser (403 Forbidden): Your account is not eligible for Gemini Code Assist for individuals at this time",
        );
        let msg = c.user_message();
        assert!(msg.contains("not eligible"), "{msg}");
        assert!(msg.contains("GOOGLE_API_KEY"), "{msg}");
    }

    #[test]
    fn auth_401_keeps_the_key_hint() {
        let c = classify_message("Provider API error (401): Unauthorized");
        assert_eq!(c.user_message(), "Authentication failed — check API key");
    }

    #[test]
    fn xai_401_points_at_login_not_api_key() {
        let c = classify_message("xAI API error (401): Unauthorized");
        let msg = c.user_message();
        assert!(msg.contains("auth login xai"), "{msg}");
        assert!(!msg.contains("check API key"), "{msg}");
    }

    #[test]
    fn classifies_auth() {
        let c = classify_message("Provider API error (401): Unauthorized");
        assert_eq!(c.kind, ErrorKind::Auth);
        assert!(!c.retryable);
    }

    #[test]
    fn classifies_client_400() {
        let c = classify_message("Provider API error (400): Bad request");
        assert_eq!(c.kind, ErrorKind::Client);
        assert!(!c.retryable);
    }

    #[test]
    fn user_message_hides_json_blob() {
        let body = r#"{"error":{"message":"[500]: An internal server error occurred","type":"server_error","code":"internal_server_error"}}"#;
        let c = classify_message(body);
        let u = c.user_message();
        assert!(!u.contains('{'), "{u}");
        assert!(u.contains("500") || u.to_ascii_lowercase().contains("server"));
    }

    #[test]
    fn extract_bracket_and_paren_status() {
        assert_eq!(extract_http_status("[500]: boom"), Some(500));
        assert_eq!(extract_http_status("API error (503): x"), Some(503));
        assert_eq!(extract_http_status(r#""status": 502"#), Some(502));
        assert_eq!(extract_http_status("(403 Forbidden): nope"), Some(403));
        assert_eq!(extract_http_status("(next page)"), None);
    }

    #[test]
    fn extract_http_prefix_and_leading_forms() {
        assert_eq!(extract_http_status("HTTP 502 bad gateway"), Some(502));
        assert_eq!(extract_http_status("status: 503"), Some(503));
        assert_eq!(extract_http_status("status 500 boom"), Some(500));
        assert_eq!(extract_http_status("status code 500 boom"), Some(500));
        assert_eq!(extract_http_status("500 Internal Server Error"), Some(500));
        assert_eq!(
            extract_http_status(r#""code":"internal_server_error""#),
            None
        );
        assert_eq!(extract_http_status(r#""code":502"#), Some(502));
    }

    #[test]
    fn retry_after_header_seconds_is_parsed() {
        let c = classify_message("Provider API error (429): slow down; retry-after: 12");
        assert_eq!(c.kind, ErrorKind::RateLimited);
        assert_eq!(c.retry_after, Some(Duration::from_secs(12)));
    }

    #[test]
    fn retry_in_seconds_phrase_sets_wait() {
        let c = classify_message("(429) rate limit hit, retry in 7 seconds");
        assert_eq!(c.kind, ErrorKind::RateLimited);
        assert_eq!(c.retry_after, Some(Duration::from_secs(7)));
    }

    #[test]
    fn wait_ms_phrase_is_parsed_as_millis() {
        let c = classify_message("(429) too many requests, wait 250ms");
        assert_eq!(c.retry_after, Some(Duration::from_millis(250)));
    }

    #[test]
    fn rate_limit_without_hint_defaults_to_five_seconds() {
        let c = classify_message("rate_limit_exceeded for model X");
        assert_eq!(c.kind, ErrorKind::RateLimited);
        assert_eq!(c.retry_after, Some(Duration::from_secs(5)));
        assert_eq!(c.status, Some(429));
    }

    #[test]
    fn timeout_copy() {
        let c = classify_message("request timed out after 30s");
        assert_eq!(c.kind, ErrorKind::Timeout);
        assert!(c.retryable);
        assert_eq!(c.user_message(), "Request timed out");
    }

    #[test]
    fn deadline_exceeded_is_timeout() {
        let c = classify_message("operation deadline exceeded while waiting");
        assert_eq!(c.kind, ErrorKind::Timeout);
        assert!(c.retryable);
    }

    #[test]
    fn network_copy() {
        let c = classify_message("tcp connect error: connection refused");
        assert_eq!(c.kind, ErrorKind::Network);
        assert!(c.retryable);
        assert_eq!(c.user_message(), "Network error reaching the provider");
    }

    #[test]
    fn cancelled_copy_in_both_spellings() {
        let c = classify_message("stream cancelled by user");
        assert_eq!(c.kind, ErrorKind::Cancelled);
        assert!(!c.retryable);
        assert_eq!(c.user_message(), "Cancelled");
        let us = classify_message("request canceled");
        assert_eq!(us.kind, ErrorKind::Cancelled);
    }

    #[test]
    fn bad_gateway_is_server_with_status() {
        let c = classify_message("502 bad gateway from proxy");
        assert_eq!(c.kind, ErrorKind::Server);
        assert!(c.retryable);
        assert_eq!(c.status, Some(502));
        assert_eq!(c.user_message(), "Provider server error (HTTP 502)");
    }

    #[test]
    fn hard_client_status_wins_over_server_language() {
        let c = classify_message("overloaded (422): validation failed");
        assert_eq!(c.kind, ErrorKind::Client);
        assert!(!c.retryable);
        assert_eq!(c.user_message(), "Request rejected (HTTP 422)");
    }

    #[test]
    fn authentication_language_is_auth_without_status() {
        let c = classify_message("authentication failed for provider");
        assert_eq!(c.kind, ErrorKind::Auth);
        assert!(!c.retryable);
    }

    #[test]
    fn unclassified_message_passes_through_compactly() {
        let c = classify_message("something odd happened");
        assert_eq!(c.kind, ErrorKind::Unknown);
        assert_eq!(c.user_message(), "something odd happened");
    }

    #[test]
    fn unknown_long_message_is_truncated_with_ellipsis() {
        let m = format!("prefix {}", "x".repeat(200));
        let u = classify_message(&m).user_message();
        assert!(u.ends_with('…'), "{u}");
        assert_eq!(u.chars().count(), 160);
    }

    #[test]
    fn unknown_blank_message_gets_generic_copy() {
        let u = classify_message("   ").user_message();
        assert_eq!(u, "LLM request failed");
    }

    #[test]
    fn server_without_status_copy() {
        let ce = ClassifiedError {
            kind: ErrorKind::Server,
            retryable: true,
            retry_after: None,
            status: None,
            message: "boom".into(),
        };
        assert_eq!(ce.user_message(), "Provider server error");
    }

    #[test]
    fn client_without_status_copy() {
        let ce = ClassifiedError {
            kind: ErrorKind::Client,
            retryable: false,
            retry_after: None,
            status: None,
            message: "boom".into(),
        };
        assert_eq!(ce.user_message(), "Request rejected by the provider");
    }

    #[test]
    fn rate_limited_copy_reflects_retry_after() {
        let with_hint = ClassifiedError {
            kind: ErrorKind::RateLimited,
            retryable: true,
            retry_after: Some(Duration::from_secs(9)),
            status: Some(429),
            message: String::new(),
        };
        assert_eq!(with_hint.user_message(), "Rate limited — retry in 9s");
        let without = ClassifiedError {
            kind: ErrorKind::RateLimited,
            retryable: true,
            retry_after: None,
            status: None,
            message: String::new(),
        };
        assert_eq!(without.user_message(), "Rate limited by the provider");
    }

    #[test]
    fn classify_wraps_core_error_display() {
        let err = whycode_core::Error::Llm("Provider API error (500): internal".into());
        let c = classify(&err);
        assert_eq!(c.kind, ErrorKind::Server);
        assert_eq!(c.message, "LLM error: Provider API error (500): internal");
    }
}
