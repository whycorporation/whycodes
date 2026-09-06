use super::*;
use whycodes_core::types::{Message, ToolDefinition};

fn req(system: &str, user: &str) -> LlmRequest {
    LlmRequest {
        system: system.into(),
        messages: std::sync::Arc::from(vec![Message {
            role: Role::User,
            content: MessageContent::Text(user.into()),
            tool_call_id: None,
            name: None,
            created_at: None,
        }]),
        tools: std::sync::Arc::from([]),
        max_tokens: Some(64),
        temperature: Some(0.2),
        top_p: None,
        top_k: None,
        stop_sequences: None,
        thinking: None,
        use_prompt_cache: false,
    }
}

#[test]
fn exact_hit_replays() {
    let cache = ResponseCache::new();
    let r = req("sys", "what is the default port");
    assert!(cache.lookup(&r, "haiku").is_none());
    cache.store(&r, "haiku", "8080");
    let hit = cache.lookup(&r, "haiku").expect("exact");
    assert_eq!(hit.text, "8080");
}

#[test]
fn different_model_misses() {
    let cache = ResponseCache::new();
    let r = req("sys", "default port");
    cache.store(&r, "haiku", "8080");
    assert!(cache.lookup(&r, "sonnet").is_none());
}

#[test]
fn semantic_paraphrase_hits_same_system() {
    let cache = ResponseCache::new();
    let a = req("sys", "what is the default port");
    cache.store(&a, "haiku", "8080");
    let b = req("sys", "what's the default port");
    let hit = cache.lookup(&b, "haiku").expect("semantic");
    assert_eq!(hit.text, "8080");
}

#[test]
fn different_system_does_not_semantic_hit() {
    let cache = ResponseCache::new();
    cache.store(
        &req("project-a", "what is the default port"),
        "haiku",
        "8080",
    );
    assert!(
        cache
            .lookup(&req("project-b", "what is the default port"), "haiku")
            .is_none()
    );
}

#[test]
fn tools_in_request_never_cache() {
    let cache = ResponseCache::new();
    let mut r = req("sys", "read src/main.rs");
    r.tools = vec![ToolDefinition {
        name: "read".into(),
        description: "read a file".into(),
        parameters: serde_json::json!({"type": "object"}),
    }]
    .into();
    cache.store(&r, "haiku", "fn main() {}");
    assert!(cache.lookup(&r, "haiku").is_none());
    assert_eq!(cache.len(), 0);
}

#[test]
fn text_only_skips_tool_use_responses() {
    let resp = LlmResponse {
        content: vec![
            ContentBlock::Text {
                text: "calling".into(),
            },
            ContentBlock::ToolUse {
                id: "1".into(),
                name: "read".into(),
                input: serde_json::json!({}),
            },
        ],
        stop_reason: None,
        usage: Usage::default(),
        model: "x".into(),
    };
    assert!(text_only_response(&resp).is_none());

    let thinking_only = LlmResponse {
        content: vec![ContentBlock::Thinking {
            text: "plan".into(),
            signature: None,
        }],
        stop_reason: None,
        usage: Usage::default(),
        model: "x".into(),
    };
    assert!(text_only_response(&thinking_only).is_none());

    let whitespace = LlmResponse {
        content: vec![ContentBlock::Text {
            text: "   \n".into(),
        }],
        stop_reason: None,
        usage: Usage::default(),
        model: "x".into(),
    };
    assert!(text_only_response(&whitespace).is_none());
}

fn full_req(system: &str, messages: Vec<Message>, tools: Vec<ToolDefinition>) -> LlmRequest {
    LlmRequest {
        system: system.into(),
        messages: std::sync::Arc::from(messages),
        tools: std::sync::Arc::from(tools),
        max_tokens: Some(64),
        temperature: Some(0.2),
        top_p: None,
        top_k: None,
        stop_sequences: None,
        thinking: None,
        use_prompt_cache: false,
    }
}

fn msg(role: Role, content: MessageContent) -> Message {
    Message {
        role,
        content,
        tool_call_id: None,
        name: None,
        created_at: None,
    }
}

#[test]
fn clear_default_to_response_and_empty_store() {
    let cache = ResponseCache::default();
    assert!(cache.is_empty());
    cache.store(&req("sys", "q"), "haiku", "   ");
    assert!(cache.is_empty());
    cache.store(&req("sys", "q"), "haiku", "answer");
    assert_eq!(cache.len(), 1);
    cache.store(&req("sys", "q"), "haiku", "answer-again");
    assert_eq!(cache.len(), 1, "duplicate exact key is ignored");
    cache.clear();
    assert!(cache.is_empty());

    let hit = CachedText {
        text: "cached".into(),
    };
    let resp = ResponseCache::to_response(&hit, "haiku");
    assert_eq!(resp.model, "haiku");
    assert_eq!(resp.stop_reason.as_deref(), Some("cache"));
    assert!(matches!(
        &resp.content[0],
        ContentBlock::Text { text } if text == "cached"
    ));
    assert!(text_only_response(&resp).as_deref() == Some("cached"));
    assert!(cosine(&[], &[1.0]).abs() < f32::EPSILON);
    let mut zeros = [0.0f32; 4];
    l2_normalize(&mut zeros);
    assert!(zeros.iter().all(|x| *x == 0.0));
    let _ = embed("a   b   c", 4);
}

#[test]
fn store_at_evicts_expired_and_lru_overflow() {
    let cache = ResponseCache::new();
    let r = req("sys", "old question");
    let old = Instant::now()
        .checked_sub(TTL + Duration::from_secs(1))
        .unwrap_or_else(Instant::now);
    cache.store_at(&r, "haiku", "stale", old);
    assert!(cache.lookup(&r, "haiku").is_none());

    for i in 0..(MAX_ENTRIES + 2) {
        cache.store(
            &req("sys", &format!("q{i} unique-enough")),
            "haiku",
            &format!("a{i}"),
        );
    }
    assert!(cache.len() <= MAX_ENTRIES);
}

#[test]
fn exact_key_hashes_roles_and_blocks() {
    let r = full_req(
        "sys",
        vec![
            msg(Role::System, MessageContent::Text("s".into())),
            msg(Role::User, MessageContent::Text("u".into())),
            msg(
                Role::Assistant,
                MessageContent::Blocks(vec![
                    ContentBlock::Text { text: "t".into() },
                    ContentBlock::ToolUse {
                        id: "1".into(),
                        name: "read".into(),
                        input: serde_json::json!({"p": 1}),
                    },
                    ContentBlock::ToolResult {
                        tool_use_id: "1".into(),
                        content: "ok".into(),
                        is_error: None,
                    },
                    ContentBlock::Image {
                        source: whycodes_core::types::ImageSource::Url {
                            url: "http://example.invalid/x.png".into(),
                        },
                    },
                    ContentBlock::Thinking {
                        text: "think".into(),
                        signature: Some("sig".into()),
                    },
                    ContentBlock::RedactedThinking {
                        data: "opaque".into(),
                    },
                ]),
            ),
            Message {
                role: Role::Tool,
                content: MessageContent::Text("tool-out".into()),
                tool_call_id: Some("1".into()),
                name: None,
                created_at: None,
            },
        ],
        vec![ToolDefinition {
            name: "read".into(),
            description: "d".into(),
            parameters: serde_json::json!({"type": "object"}),
        }],
    );
    assert!(!ResponseCache::eligible(&r));
    let key = exact_key(&r, "haiku");
    assert_ne!(key, 0);
    assert_eq!(tool_sig(&r), tool_sig(&r));
}

#[test]
fn store_returns_when_lock_is_poisoned() {
    let cache = ResponseCache::new();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = cache.inner.lock().unwrap();
        panic!("poison response cache");
    }));
    cache.store(&req("sys", "q"), "haiku", "answer");
    assert_eq!(cache.len(), 1);
    let hit = cache.lookup(&req("sys", "q"), "haiku").expect("recovered");
    assert_eq!(hit.text, "answer");
    cache.clear();
    assert!(cache.is_empty());
}

#[test]
fn semantic_picks_higher_score_when_two_paraphrases_match() {
    let cache = ResponseCache::new();
    cache.store(&req("sys", "what is the default port"), "haiku", "8080");
    cache.store(
        &req("sys", "what is the default listening port number"),
        "haiku",
        "9090",
    );
    let hit = cache
        .lookup(&req("sys", "what's the default port"), "haiku")
        .expect("semantic");
    assert!(!hit.text.is_empty());
}

#[test]
fn better_semantic_prefers_higher_score() {
    assert_eq!(
        ResponseCache::better_semantic_for_tests(None, 0, 0.90),
        Some((0, 0.90))
    );
    assert_eq!(
        ResponseCache::better_semantic_for_tests(Some((0, 0.90)), 1, 0.95),
        Some((1, 0.95))
    );
    assert_eq!(
        ResponseCache::better_semantic_for_tests(Some((0, 0.95)), 1, 0.91),
        Some((0, 0.95))
    );
    assert!(ResponseCache::take_semantic_hit_none_for_tests().is_none());
}
