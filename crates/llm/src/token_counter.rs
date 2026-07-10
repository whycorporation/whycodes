//! Token counting using tiktoken-rs with model-based encoding selection.
//! Falls back to a char/4 heuristic when encoding is unavailable.

use anyhow::{Context, Result};

/// Map a model name to its tiktoken encoding name.
fn model_to_encoding(model: &str) -> Option<&'static str> {
    let model_lower = model.to_lowercase();
    // GPT-4 family → o200k_base
    if model_lower.contains("gpt-4o") || model_lower.contains("gpt-4.5") {
        return Some("o200k_base");
    }
    if model_lower.contains("gpt-4") {
        return Some("cl100k_base");
    }
    // GPT-3.5 family
    if model_lower.contains("gpt-3.5") {
        return Some("cl100k_base");
    }
    // o-series models
    if model_lower.starts_with("o1") || model_lower.starts_with("o3") || model_lower.starts_with("o4") {
        return Some("o200k_base");
    }
    // Claude / Anthropic
    if model_lower.contains("claude") {
        return Some("cl100k_base");
    }
    // DeepSeek
    if model_lower.contains("deepseek") {
        return Some("cl100k_base");
    }
    // Gemini
    if model_lower.contains("gemini") {
        return Some("cl100k_base");
    }
    // Generic fallback for common model families
    if model_lower.contains("llama") || model_lower.contains("mistral") || model_lower.contains("mixtral") {
        return Some("cl100k_base");
    }

    None
}

/// Count tokens in the given text using tiktoken for the specified model.
/// Falls back to char/4 heuristic if encoding is not found.
pub fn count_tokens(text: &str, model: &str) -> Result<usize> {
    let encoding_name = match model_to_encoding(model) {
        Some(name) => name,
        None => return Ok(chars_to_tokens_fallback(text)),
    };

    match tiktoken_rs::get_bpe_from_encoding(encoding_name) {
        Ok(bpe) => {
            let tokens = bpe.encode_with_special_tokens(text);
            Ok(tokens.len())
        }
        Err(_) => Ok(chars_to_tokens_fallback(text)),
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
        let text = match &msg.content {
            whycode_core::types::MessageContent::Text(t) => t.clone(),
            whycode_core::types::MessageContent::Blocks(blocks) => {
                blocks
                    .iter()
                    .filter_map(|b| match b {
                        whycode_core::types::ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        };
        total += count_tokens(&text, model)?;
    }
    Ok(total)
}

/// Simple fallback: ~4 characters per token (common heuristic).
fn chars_to_tokens_fallback(text: &str) -> usize {
    let chars = text.chars().count();
    (chars / 4).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fallback_counting() {
        let text = "Hello, world! This is a test.";
        let count = count_tokens(text, "unknown-model").unwrap();
        assert!(count > 0);
    }

    #[test]
    fn test_gpt4o_encoding() {
        let text = "Hello, world!";
        let count = count_tokens("gpt-4o", &text).unwrap();
        assert!(count > 0);
    }

    #[test]
    fn test_claude_encoding() {
        let text = "Hello, world!";
        let count = count_tokens("claude-sonnet-4-20250514", &text).unwrap();
        assert!(count > 0);
    }
}
