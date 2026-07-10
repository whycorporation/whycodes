//! Model capabilities detection.
//!
//! Provides a static map of well-known model capabilities and can be
//! extended with dynamic detection from provider metadata.

use serde::{Deserialize, Serialize};

/// Capabilities of an LLM model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelCapabilities {
    /// Whether the model supports tool/function calling.
    pub tools: bool,
    /// Whether the model supports vision (image inputs).
    pub vision: bool,
    /// Whether the model supports extended thinking.
    pub thinking: bool,
    /// Maximum context window tokens for this model.
    pub max_tokens: u32,
    /// Whether the model supports prompt caching.
    pub caching: bool,
}

impl Default for ModelCapabilities {
    fn default() -> Self {
        Self {
            tools: true,
            vision: false,
            thinking: false,
            max_tokens: 128_000,
            caching: false,
        }
    }
}

/// Detect capabilities for a model by its provider and model identifier.
///
/// Uses a static map of well-known models. Falls back to reasonable defaults
/// based on the provider and model name heuristics.
pub fn detect_capabilities(provider: &str, model: &str) -> ModelCapabilities {
    let model_lower = model.to_lowercase();
    let provider_lower = provider.to_lowercase();

    // Check static map first
    if let Some(caps) = known_capabilities(&model_lower) {
        return caps;
    }

    // Heuristic fallback based on provider and model name
    let tools = matches!(
        provider_lower.as_str(),
        "openai" | "anthropic" | "google" | "gemini" | "deepseek" | "openrouter"
    );

    let vision = model_lower.contains("vision")
        || model_lower.contains("gpt-4o")
        || model_lower.contains("claude-3")
        || model_lower.contains("claude-sonnet")
        || model_lower.contains("gemini-2")
        || model_lower.contains("gemini-1.5");

    let thinking = model_lower.contains("claude-sonnet-4")
        || model_lower.contains("claude-3-7")
        || model_lower.contains("claude-3.7")
        || model_lower.contains("gpt-4.5")
        || model_lower.starts_with("o1")
        || model_lower.starts_with("o3")
        || model_lower.starts_with("o4");

    let caching = provider_lower == "anthropic"
        && (model_lower.contains("claude-3-5")
            || model_lower.contains("claude-3-7")
            || model_lower.contains("claude-3.7")
            || model_lower.contains("claude-3.5")
            || model_lower.contains("claude-sonnet-4"));

    let max_tokens = if model_lower.contains("gpt-4-32k") {
        32_000
    } else if model_lower.contains("gemini-2.5-pro") {
        1_000_000
    } else if model_lower.contains("gemini-2.0-flash") {
        1_000_000
    } else if model_lower.contains("gemini") {
        128_000
    } else if model_lower.contains("claude-sonnet-4") || model_lower.contains("claude-3-7") || model_lower.contains("claude-3.7") {
        200_000
    } else if model_lower.contains("claude") {
        200_000
    } else if model_lower.contains("deepseek-r1") {
        128_000
    } else if model_lower.contains("deepseek-v3") {
        128_000
    } else if model_lower.contains("deepseek") {
        64_000
    } else if model_lower.contains("gpt-4o") {
        128_000
    } else if model_lower.contains("gpt-4") {
        8_192
    } else {
        128_000
    };

    ModelCapabilities {
        tools,
        vision,
        thinking,
        max_tokens,
        caching,
    }
}

/// Static map of known models and their capabilities.
fn known_capabilities(model_lower: &str) -> Option<ModelCapabilities> {
    // OpenAI models
    if model_lower.starts_with("gpt-4o") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: false,
            max_tokens: 128_000,
            caching: false,
        });
    }
    if model_lower == "gpt-4.5" || model_lower.starts_with("gpt-4.5-") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: true,
            max_tokens: 128_000,
            caching: false,
        });
    }
    if model_lower.starts_with("gpt-4-turbo") || model_lower.starts_with("gpt-4-") {
        return Some(ModelCapabilities {
            tools: true,
            vision: model_lower.contains("vision"),
            thinking: false,
            max_tokens: if model_lower.contains("32k") { 32_000 } else { 128_000 },
            caching: false,
        });
    }
    if model_lower.starts_with("gpt-3.5") {
        return Some(ModelCapabilities {
            tools: true,
            vision: false,
            thinking: false,
            max_tokens: 16_384,
            caching: false,
        });
    }
    if model_lower.starts_with("o1") {
        return Some(ModelCapabilities {
            tools: true,
            vision: model_lower.contains("gpt-4o"),
            thinking: true,
            max_tokens: 200_000,
            caching: false,
        });
    }
    if model_lower.starts_with("o3") || model_lower.starts_with("o4") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: true,
            max_tokens: 200_000,
            caching: false,
        });
    }

    // Anthropic models
    if model_lower.contains("claude-sonnet-4") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: true,
            max_tokens: 200_000,
            caching: true,
        });
    }
    if model_lower.contains("claude-3-7") || model_lower.contains("claude-3.7") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: true,
            max_tokens: 200_000,
            caching: true,
        });
    }
    if model_lower.contains("claude-3-5") || model_lower.contains("claude-3.5") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: false,
            max_tokens: 200_000,
            caching: true,
        });
    }
    if model_lower.contains("claude-3-opus") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: false,
            max_tokens: 200_000,
            caching: false,
        });
    }
    if model_lower.contains("claude-3-haiku") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: false,
            max_tokens: 200_000,
            caching: false,
        });
    }

    // DeepSeek models
    if model_lower.contains("deepseek-r1") {
        return Some(ModelCapabilities {
            tools: false,
            vision: false,
            thinking: true,
            max_tokens: 128_000,
            caching: false,
        });
    }
    if model_lower.contains("deepseek-v3") {
        return Some(ModelCapabilities {
            tools: true,
            vision: false,
            thinking: false,
            max_tokens: 128_000,
            caching: false,
        });
    }
    if model_lower.contains("deepseek-chat") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: false,
            max_tokens: 128_000,
            caching: false,
        });
    }

    // Gemini models
    if model_lower.contains("gemini-2.5-pro") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: true,
            max_tokens: 1_000_000,
            caching: false,
        });
    }
    if model_lower.contains("gemini-2.0-flash") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: false,
            max_tokens: 1_000_000,
            caching: false,
        });
    }
    if model_lower.contains("gemini-2.5-flash") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: true,
            max_tokens: 1_000_000,
            caching: false,
        });
    }
    if model_lower.contains("gemini-1.5-pro") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: false,
            max_tokens: 2_000_000,
            caching: false,
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gpt4o() {
        let caps = detect_capabilities("openai", "gpt-4o");
        assert!(caps.tools);
        assert!(caps.vision);
        assert!(!caps.thinking);
        assert_eq!(caps.max_tokens, 128_000);
    }

    #[test]
    fn test_claude_sonnet_4() {
        let caps = detect_capabilities("anthropic", "claude-sonnet-4-20250514");
        assert!(caps.tools);
        assert!(caps.vision);
        assert!(caps.thinking);
        assert!(caps.caching);
        assert_eq!(caps.max_tokens, 200_000);
    }

    #[test]
    fn test_deepseek_r1() {
        let caps = detect_capabilities("deepseek", "deepseek-r1");
        assert!(!caps.tools);
        assert!(caps.thinking);
    }

    #[test]
    fn test_unknown_model() {
        let caps = detect_capabilities("openai", "some-new-model");
        assert!(caps.tools); // falls back to provider heuristic
    }
}
