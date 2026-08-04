//! Token counting using tiktoken-rs with model-based encoding selection.
//! Falls back to a char/4 heuristic when encoding is unavailable.
//!
//! Uses tiktoken's process-wide **singleton** BPE tables (`cl100k_base_singleton`,
//! `o200k_base_singleton`) so vocab load is paid once, not per call.

use anyhow::Result;
use tiktoken_rs::{cl100k_base_singleton, o200k_base_singleton};

/// Which BPE table a model family maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EncodingKind {
    Cl100k,
    O200k,
}

/// Map a model name to its tiktoken encoding family.
fn model_to_encoding(model: &str) -> Option<EncodingKind> {
    let model_lower = model.to_ascii_lowercase();
    // GPT-4 family → o200k_base
    if model_lower.contains("gpt-4o") || model_lower.contains("gpt-4.5") {
        return Some(EncodingKind::O200k);
    }
    if model_lower.contains("gpt-4") {
        return Some(EncodingKind::Cl100k);
    }
    // GPT-3.5 family
    if model_lower.contains("gpt-3.5") {
        return Some(EncodingKind::Cl100k);
    }
    // o-series models
    if model_lower.starts_with("o1")
        || model_lower.starts_with("o3")
        || model_lower.starts_with("o4")
    {
        return Some(EncodingKind::O200k);
    }
    // Claude / Anthropic
    if model_lower.contains("claude") {
        return Some(EncodingKind::Cl100k);
    }
    // DeepSeek
    if model_lower.contains("deepseek") {
        return Some(EncodingKind::Cl100k);
    }
    // Gemini
    if model_lower.contains("gemini") {
        return Some(EncodingKind::Cl100k);
    }
    // Generic fallback for common model families
    if model_lower.contains("llama")
        || model_lower.contains("mistral")
        || model_lower.contains("mixtral")
    {
        return Some(EncodingKind::Cl100k);
    }

    None
}

fn encode_len(kind: EncodingKind, text: &str) -> usize {
    match kind {
        EncodingKind::Cl100k => {
            let bpe = cl100k_base_singleton();
            bpe.lock().encode_with_special_tokens(text).len()
        }
        EncodingKind::O200k => {
            let bpe = o200k_base_singleton();
            bpe.lock().encode_with_special_tokens(text).len()
        }
    }
}

/// Count tokens in the given text using tiktoken for the specified model.
/// Falls back to char/4 heuristic if encoding is not found.
pub fn count_tokens(text: &str, model: &str) -> Result<usize> {
    match model_to_encoding(model) {
        Some(kind) => Ok(encode_len(kind, text)),
        None => Ok(chars_to_tokens_fallback(text)),
    }
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
    fn test_gpt4o_encoding() {
        let text = "Hello, world!";
        let count = count_tokens(text, "gpt-4o").unwrap();
        assert!(count > 0);
    }

    #[test]
    fn test_claude_encoding() {
        let text = "Hello, world!";
        let count = count_tokens(text, "claude-sonnet-4-20250514").unwrap();
        assert!(count > 0);
    }

    #[test]
    fn singleton_is_stable_across_calls() {
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
