use super::*;

#[test]
fn snapshot_chunk_does_not_duplicate() {
    let mut acc = ThinkingAccumulator::new();
    acc.push_text("ab");
    acc.push_text("abcd");
    let blocks = acc.into_blocks();
    let ContentBlock::Thinking { text, .. } = &blocks[0] else {
        panic!("expected thinking");
    };
    assert_eq!(text, "abcd");
}

#[test]
fn signature_attaches_to_open_block() {
    let mut acc = ThinkingAccumulator::new();
    acc.push_text("plan");
    acc.push_signature("sig-1");
    let blocks = acc.into_blocks();
    let ContentBlock::Thinking { text, signature } = &blocks[0] else {
        panic!("expected thinking");
    };
    assert_eq!(text, "plan");
    assert_eq!(signature.as_deref(), Some("sig-1"));
}

#[test]
fn attach_sets_budget_and_effort_for_grok() {
    let mut req = whycodes_core::types::LlmRequest {
        system: String::new(),
        messages: std::sync::Arc::from(Vec::new()),
        tools: std::sync::Arc::from([]),
        max_tokens: Some(1024),
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: None,
        thinking: None,
        use_prompt_cache: false,
    };
    attach_thinking_request(&mut req, "xai", "grok-4", None, None);
    {
        let t = req.thinking.as_ref().unwrap();
        assert_eq!(t["enabled"], true);
        assert_eq!(t["budget_tokens"], 4000);
        assert_eq!(t["reasoning_effort"], "medium");
    }
    req.thinking = None;
    attach_thinking_request(&mut req, "xai", "grok-4.6", None, Some("xhigh"));
    assert_eq!(req.thinking.as_ref().unwrap()["reasoning_effort"], "xhigh");
    req.thinking = None;
    attach_thinking_request(&mut req, "xai", "grok-4", None, Some("xhigh"));
    assert_eq!(req.thinking.as_ref().unwrap()["reasoning_effort"], "high");
    apply_ultrathink(&mut req);
    let t = req.thinking.as_ref().unwrap();
    assert_eq!(t["budget_tokens"], 16_000);
    assert_eq!(t["reasoning_effort"], "high");
}

#[test]
fn empty_text_and_signature_only_block() {
    let mut acc = ThinkingAccumulator::new();
    acc.push_text("");
    acc.push_signature("");
    acc.push_signature("sig-only");
    acc.push_redacted("");
    acc.push_redacted("redacted-data");
    let blocks = acc.into_blocks();
    assert!(
        blocks.iter().any(|b| matches!(
            b,
            ContentBlock::Thinking {
                signature: Some(s),
                ..
            } if s == "sig-only"
        )),
        "{blocks:?}"
    );
    assert!(
        blocks.iter().any(
            |b| matches!(b, ContentBlock::RedactedThinking { data } if data == "redacted-data")
        ),
        "{blocks:?}"
    );
}

#[test]
fn attach_skips_when_already_set_or_unsupported() {
    let mut req = whycodes_core::types::LlmRequest {
        system: String::new(),
        messages: std::sync::Arc::from(Vec::new()),
        tools: std::sync::Arc::from([]),
        max_tokens: Some(1024),
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: None,
        thinking: Some(serde_json::json!({"enabled": true, "budget_tokens": 9})),
        use_prompt_cache: false,
    };
    attach_thinking_request(&mut req, "xai", "grok-4", None, None);
    assert_eq!(req.thinking.as_ref().unwrap()["budget_tokens"], 9);

    let cfg = whycodes_core::types::ModelConfig {
        model_id: "grok-4".into(),
        provider_id: "xai".into(),
        max_tokens: None,
        context_window: None,
        temperature: None,
        top_p: None,
        thinking: Some(false),
        supports_tools: None,
        supports_images: None,
    };
    req.thinking = None;
    attach_thinking_request(&mut req, "xai", "grok-4", Some(&cfg), None);
    assert!(req.thinking.is_none());
}

#[test]
fn ultrathink_enables_thinking_when_absent() {
    let mut req = whycodes_core::types::LlmRequest {
        system: String::new(),
        messages: std::sync::Arc::from(Vec::new()),
        tools: std::sync::Arc::from([]),
        max_tokens: Some(1024),
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: None,
        thinking: None,
        use_prompt_cache: false,
    };
    apply_ultrathink(&mut req);
    let t = req.thinking.unwrap();
    assert_eq!(t["enabled"], true);
    assert_eq!(t["budget_tokens"], 16_000);
    assert_eq!(t["reasoning_effort"], "high");
}

fn assert_thinking(blocks: &[ContentBlock]) {
    let ContentBlock::Thinking { .. } = &blocks[0] else {
        panic!("expected thinking");
    };
}

#[test]
fn leftover_panic_arms_are_reachable() {
    let mut acc = ThinkingAccumulator::new();
    acc.push_text("hello");
    assert_thinking(&acc.into_blocks());

    let mut acc = ThinkingAccumulator::new();
    acc.push_text("plan");
    acc.push_signature("sig");
    assert_thinking(&acc.into_blocks());
}
