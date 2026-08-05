//! Token counting for local estimates when the provider does not report usage.
//!
//! Uses a char/4 heuristic only. Shipping tiktoken-rs BPE tables inflated the
//! binary for a path the product rarely needs (provider `Usage` events and
//! session totals already drive `/info` and `stats`). Re-add a real encoder
//! behind a feature if offline BPE ever becomes a product requirement.

use anyhow::Result;

/// Count tokens in the given text for the specified model.
///
/// Model is accepted for API stability; the heuristic is model-agnostic.
pub fn count_tokens(text: &str, _model: &str) -> Result<usize> {
    Ok(chars_to_tokens_fallback(text))
}

/// Count tokens across multiple messages.
pub fn count_message_tokens(
    system: &str,
    messages: &[whycode_core::types::Message],
    model: &str,
) -> Result<usize> {
    let mut total = count_tokens(system, model)?;
    for msg in messages {
        match &msg.content {
            whycode_core::types::MessageContent::Text(t) => {
                total += count_tokens(t, model)?;
            }
            whycode_core::types::MessageContent::Blocks(blocks) => {
                // Avoid joining into a temporary when most blocks are text.
                for b in blocks {
                    if let whycode_core::types::ContentBlock::Text { text } = b {
                        total += count_tokens(text, model)?;
                    }
                }
            }
        }
    }
    Ok(total)
}

/// Simple fallback: ~4 characters per token (common heuristic).
///
/// Uses Unicode scalar count (not UTF-8 bytes) so CJK is not under-counted, then
/// `div_ceil(4)` instead of truncating division so short strings never report 0.
fn chars_to_tokens_fallback(text: &str) -> usize {
    let chars = text.chars().count();
    chars.div_ceil(4).max(1)
}

#[cfg(test)]
mod tests {
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
}
