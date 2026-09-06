use super::*;

#[test]
fn test_fallback_counting() {
    let text = "Hello, world! This is a test.";
    let count = count_tokens(text, "unknown-model").unwrap();
    assert!(count > 0);
    assert_eq!(count, text.chars().count().div_ceil(4).max(1));
}

#[test]
fn test_gpt4o_encoding_uses_heuristic() {
    let text = "Hello, world!";
    let count = count_tokens(text, "gpt-4o").unwrap();
    assert_eq!(count, text.chars().count().div_ceil(4).max(1));
}

#[test]
fn test_claude_encoding_uses_heuristic() {
    let text = "Hello, world!";
    let count = count_tokens(text, "claude-sonnet-4-20250514").unwrap();
    assert_eq!(count, text.chars().count().div_ceil(4).max(1));
}

#[test]
fn heuristic_is_stable_across_calls() {
    let a = count_tokens("cache me", "gpt-4o").unwrap();
    let b = count_tokens("cache me", "gpt-4o").unwrap();
    assert_eq!(a, b);
}

#[test]
fn div_ceil_heuristic_on_short_strings() {
    assert_eq!(chars_to_tokens_fallback("a"), 1);
    assert_eq!(chars_to_tokens_fallback("abcd"), 1);
    assert_eq!(chars_to_tokens_fallback("abcde"), 2);
}

#[test]
fn count_message_tokens_sums_system_text_and_blocks() {
    use whycodes_core::types::{ContentBlock, Message, MessageContent, Role};
    let messages = [
        Message {
            role: Role::User,
            content: MessageContent::Text("hello world".into()),
            tool_call_id: None,
            name: None,
            created_at: None,
        },
        Message {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![
                ContentBlock::Text {
                    text: "reply".into(),
                },
                ContentBlock::Image {
                    source: whycodes_core::types::ImageSource::Url {
                        url: "http://example.invalid/x.png".into(),
                    },
                },
            ]),
            tool_call_id: None,
            name: None,
            created_at: None,
        },
    ];
    let n = count_message_tokens("sys", &messages, "gpt-4o").unwrap();
    assert!(n >= 3, "{n}");
    assert_eq!(chars_to_tokens_raw(""), 0);
    assert_eq!(chars_to_tokens_raw("abcd"), 1);
}
