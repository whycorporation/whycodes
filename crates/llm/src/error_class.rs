//! Structured classification of LLM / HTTP transport failures.
//!
//! Providers and proxies return a mess of shapes: `(500)`, `[500]`, nested JSON
//! `server_error`, raw connection strings. Classification is the single source
//! of truth for **retry** and **user-facing** copy.

use std::time::Duration;

use serde::Deserialize;
use whycodes_core::Error;

use crate::rate_limit::parse_retry_after;

// Re-export so `whycodes_llm::ErrorKind` stays the public alias.
pub use whycodes_core::ErrorKind;

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
                    return "xAI authentication failed — run `whycodes auth login xai` (or set XAI_API_KEY)".into();
                }
                if lower.contains("not eligible") && lower.contains("code assist") {
                    // Gemini Code Assist free-tier eligibility is account-based;
                    // the credential is fine, so "check API key" misleads.
                    return "Google account is not eligible for Gemini Code Assist (free tier) — use an AI Studio API key (GOOGLE_API_KEY) or a different account".into();
                }
                if self.status == Some(403) {
                    // A 403 usually carries the actionable reason (plan, region,
                    // eligibility) in the provider body — surface it before
                    // the generic "run login" hint.
                    let m = self.message.trim().trim_start_matches("LLM error:").trim();
                    let reason = extract_provider_reason(m)
                        .map(|r| compact_reason(&r, 160))
                        .unwrap_or_else(|| compact_reason(m, 160));
                    return format!("Forbidden by the provider (HTTP 403): {reason}");
                }
                if lower.contains("code assist") {
                    return "Google authentication failed — run `whycodes auth login google-antigravity` (or `google`)".into();
                }
                "Authentication failed — check API key".into()
            }
            ErrorKind::Client => {
                if let Some(s) = self.status {
                    // Code Assist wraps Anthropic JSON in `error.message`.
                    // Dumping the first 80 chars after `model=` produced
                    // `model=claude-sonnet-4-6: {"error":{"message":"{\"type`
                    // instead of the actual rejection reason.
                    let m = self.message.trim();
                    let model = extract_model_label(m);
                    let reason = extract_provider_reason(m).map(|r| compact_reason(&r, 180));
                    return match (model, reason) {
                        (Some(model), Some(reason)) => {
                            format!("Request rejected (HTTP {s}) · {model}: {reason}")
                        }
                        (Some(model), None) => {
                            format!("Request rejected (HTTP {s}): model={model}")
                        }
                        (None, Some(reason)) => {
                            format!("Request rejected (HTTP {s}): {reason}")
                        }
                        (None, None) => format!("Request rejected (HTTP {s})"),
                    };
                }
                "Request rejected by the provider".into()
            }
            ErrorKind::ContextOverflow => {
                "Context window exceeded — compacting history and retrying".into()
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
///
/// When `err` already carries a structured [`ErrorKind`] other than
/// [`ErrorKind::Unknown`], retry/TUI use that kind and do not parse the
/// display string. Unknown kinds (and non-transport errors) still go through
/// [`classify_message`] so provider wire bodies keep working.
pub fn classify(err: &Error) -> ClassifiedError {
    if let Some(kind) = err.transport_kind()
        && kind != ErrorKind::Unknown
    {
        let mut classified = classify_message(&err.to_string());
        classified.kind = kind;
        classified.retryable = kind.retryable();
        return classified;
    }
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
        || looks_authentication_failure(&lower)
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

    // Context window / prompt too large — never retry the same payload.
    if looks_context_overflow(&lower) {
        return ClassifiedError {
            kind: ErrorKind::ContextOverflow,
            retryable: false,
            retry_after: None,
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

fn looks_authentication_failure(lower: &str) -> bool {
    if !lower.contains("authentication") {
        return false;
    }
    lower.contains("fail") || lower.contains("error") || lower.contains("invalid")
}

fn looks_context_overflow(lower: &str) -> bool {
    lower.contains("context_length_exceeded")
        || lower.contains("context length")
        || lower.contains("maximum context")
        || lower.contains("prompt is too long")
        || lower.contains("prompt too long")
        || lower.contains("input is too long")
        || lower.contains("request too large")
        || (lower.contains("too many tokens") && !lower.contains("rate"))
        || (lower.contains("token limit") && lower.contains("exceed"))
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

/// `model=claude-sonnet-4-6` from a Code Assist error line.
fn extract_model_label(msg: &str) -> Option<String> {
    let idx = msg.find("model=")?;
    let rest = &msg[idx + "model=".len()..];
    let label: String = rest
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != ':' && *c != ',' && *c != '"' && *c != '{')
        .collect();
    let label = label.trim().trim_matches(['.', ';']);
    if label.is_empty() {
        None
    } else {
        Some(label.to_string())
    }
}

/// Innermost provider `message` from Google / Anthropic JSON, if any.
pub fn extract_provider_reason(raw: &str) -> Option<String> {
    let mut current = first_json_value(raw)?;
    for _ in 0..4 {
        let msg = current
            .pointer("/error/error/message")
            .or_else(|| current.pointer("/error/message"))
            .or_else(|| current.pointer("/message"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())?;
        match first_json_value(msg).filter(|_| msg.starts_with('{')) {
            Some(nested) => current = nested,
            None => return Some(msg.to_string()),
        }
    }
    None
}

fn first_json_value(s: &str) -> Option<serde_json::Value> {
    let start = s.find('{')?;
    let mut de = serde_json::Deserializer::from_str(&s[start..]);
    Deserialize::deserialize(&mut de).ok()
}

fn compact_reason(s: &str, max_chars: usize) -> String {
    let squashed: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if squashed.chars().count() <= max_chars {
        return squashed;
    }
    let mut out: String = squashed.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
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
#[path = "error_class_tests.rs"]
mod tests;
