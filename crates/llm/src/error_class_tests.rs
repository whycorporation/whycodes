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
fn code_assist_401_points_at_login_not_api_key() {
    let c = classify_message(
        "LLM error: Code Assist loadCodeAssist (401 Unauthorized): Request had invalid authentication credentials.",
    );
    let msg = c.user_message();
    assert!(msg.contains("auth login google-antigravity"), "{msg}");
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
    assert_eq!(extract_http_status(r#""status": 99"#), None);
    assert_eq!(extract_http_status("500x boom"), None);
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
fn context_overflow_is_not_retryable() {
    for msg in [
        "invalid_request_error (400): context_length_exceeded",
        "prompt is too long for this model",
        "maximum context length exceeded",
        "input is too long",
        "request too large",
        "too many tokens in the prompt",
        "token limit exceeded",
    ] {
        let c = classify_message(msg);
        assert_eq!(c.kind, ErrorKind::ContextOverflow, "{msg}");
        assert!(!c.retryable, "{msg}");
        assert!(c.user_message().contains("Context window"), "{msg}");
    }
    assert_eq!(ErrorKind::ContextOverflow.as_str(), "context_overflow");
}

#[test]
fn hard_client_status_wins_over_server_language() {
    let c = classify_message("overloaded (422): validation failed");
    assert_eq!(c.kind, ErrorKind::Client);
    assert!(!c.retryable);
    assert_eq!(c.user_message(), "Request rejected (HTTP 422)");
}

#[test]
fn code_assist_404_surfaces_the_model() {
    let c = classify_message("LLM error: Code Assist error (404) model=gemini-3.1-pro: Not Found");
    assert_eq!(c.kind, ErrorKind::Client);
    assert_eq!(c.status, Some(404));
    let msg = c.user_message();
    assert!(msg.contains("404"), "{msg}");
    assert!(msg.contains("gemini-3.1-pro"), "{msg}");
    assert!(!msg.contains('{'), "{msg}");
}

#[test]
fn code_assist_400_unwraps_nested_anthropic_json() {
    let raw = r#"LLM error: Code Assist error (400 Bad Request) model=claude-sonnet-4-6: {"error":{"code":400,"message":"{\"type\":\"error\",\"error\":{\"type\":\"invalid_request_error\",\"message\":\"messages.2: The final block in an assistant message cannot be `thinking`.\"}}","status":"INVALID_ARGUMENT"}}"#;
    let c = classify_message(raw);
    assert_eq!(c.kind, ErrorKind::Client);
    assert_eq!(c.status, Some(400));
    let msg = c.user_message();
    assert!(msg.contains("400"), "{msg}");
    assert!(msg.contains("claude-sonnet-4-6"), "{msg}");
    assert!(
        msg.contains("cannot be `thinking`"),
        "expected inner Anthropic reason, got {msg}"
    );
    assert!(
        !msg.contains("{\"type"),
        "must not dump the nested JSON blob: {msg}"
    );
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
    let err = whycodes_core::Error::llm("Provider API error (500): internal");
    let c = classify(&err);
    assert_eq!(c.kind, ErrorKind::Server);
    assert_eq!(c.message, "LLM error: Provider API error (500): internal");
}

#[test]
fn classify_prefers_structured_kind_over_display_string() {
    let err = whycodes_core::Error::llm_kind(
        ErrorKind::Timeout,
        "rate limit exceeded (looks like 429 but kind is timeout)",
    );
    let c = classify(&err);
    assert_eq!(c.kind, ErrorKind::Timeout);
    assert!(c.retryable);
    assert!(c.message.contains("rate limit"));
}

#[test]
fn auth_403_long_message_is_truncated() {
    let long = "x".repeat(200);
    let c = classify_message(&format!("LLM error: Provider (403 Forbidden): {long}"));
    let msg = c.user_message();
    assert!(msg.contains("403"), "{msg}");
    assert!(msg.contains('…'), "{msg}");
}

#[test]
fn client_reason_without_model_label() {
    let c = classify_message("Request rejected (400): {\"error\":{\"message\":\"bad schema\"}}");
    let msg = c.user_message();
    assert!(msg.contains("400"), "{msg}");
    assert!(msg.contains("bad schema"), "{msg}");
}

#[test]
fn extract_unclosed_paren_and_empty_model() {
    assert_eq!(extract_http_status("(500 never closed"), None);
    assert_eq!(extract_model_label("model="), None);
    assert_eq!(extract_model_label("model= :"), None);
    fn nest_message(inner: &str, n: usize) -> String {
        let mut s = inner.to_string();
        for _ in 0..n {
            s = serde_json::json!({"message": s}).to_string();
        }
        s
    }
    assert_eq!(
        extract_provider_reason(&nest_message("leaf", 3)).as_deref(),
        Some("leaf")
    );
    assert!(extract_provider_reason(&nest_message("deep", 5)).is_none());
    assert_eq!(compact_reason("one   two", 10), "one two");
    assert!(compact_reason("abcdefghijklmnop", 8).ends_with('…'));
    let empty_retry = classify_message("(429) retry-after:");
    assert_eq!(empty_retry.kind, ErrorKind::RateLimited);
    let wait_secs = classify_message("(429) please wait 10 before retrying");
    assert_eq!(wait_secs.retry_after, Some(Duration::from_secs(10)));
    assert_eq!(extract_http_status(r#"{"status": 502}"#), Some(502));
    assert_eq!(extract_http_status("500 Internal Server Error"), Some(500));
    let no_digits = classify_message("(429) please wait before retrying");
    assert_eq!(no_digits.kind, ErrorKind::RateLimited);
    assert_ne!(no_digits.retry_after, Some(Duration::from_secs(0)));
}
