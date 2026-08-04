//! Model capabilities + context-window resolution.
//!
//! Many models share a provider but differ wildly in context size
//! (8k → 2M). Resolution is layered so overrides stay local and the
//! catalog can grow without touching every call site:
//!
//! 1. Explicit `ModelConfig.context_window` (user/config.toml)
//! 2. Live `GET /v1/models` (`context_length` / `max_input_tokens`, …)
//! 3. Built-in catalog / name heuristics for known models
//! 4. `session.max_context_tokens` (global fallback, default 200k)

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
    /// Context window size in tokens (prompt + completion budget).
    pub context_window: u32,
    /// Whether the model supports prompt caching.
    pub caching: bool,
}

impl Default for ModelCapabilities {
    fn default() -> Self {
        Self {
            tools: true,
            vision: false,
            thinking: false,
            context_window: 128_000,
            caching: false,
        }
    }
}

/// Resolve the context window for the active model.
///
/// * `configured` — `Config::configured_context_window`
/// * `api_context` — from a fetched `/v1/models` catalog (`context_length`, …)
/// * `fallback` — typically `config.session.max_context_tokens`
pub fn resolve_context_window(
    provider: &str,
    model: &str,
    configured: Option<u32>,
    api_context: Option<u32>,
    fallback: u64,
) -> u64 {
    if let Some(n) = configured.filter(|n| *n > 0) {
        return n as u64;
    }
    // Prefer the live provider catalog over our static table: gateways know
    // the routed model's real window (e.g. OmniRoute `context_length`).
    if let Some(n) = api_context.filter(|n| *n > 0) {
        return n as u64;
    }
    if let Some(known) = known_context_window(model) {
        return known as u64;
    }
    // Soft heuristics (substring / family) when the exact id is unknown.
    let caps = detect_capabilities(provider, model);
    if caps.context_window > 0 {
        return caps.context_window as u64;
    }
    fallback.max(1)
}

/// Detect capabilities for a model by its provider and model identifier.
///
/// Uses a static map of well-known models. Falls back to reasonable defaults
/// based on the provider and model name heuristics.
pub fn detect_capabilities(provider: &str, model: &str) -> ModelCapabilities {
    let model_lower = model.to_lowercase();
    let provider_lower = provider.to_lowercase();

    if let Some(caps) = known_capabilities(&model_lower) {
        return caps;
    }

    // Heuristic fallback based on provider and model name
    let tools = matches!(
        provider_lower.as_str(),
        "openai"
            | "anthropic"
            | "google"
            | "gemini"
            | "deepseek"
            | "openrouter"
            | "xai"
            | "groq"
            | "mistral"
            | "together"
            | "ollama"
    );

    let vision = model_lower.contains("vision")
        || model_lower.contains("gpt-4o")
        || model_lower.contains("claude-3")
        || model_lower.contains("claude-sonnet")
        || model_lower.contains("claude-opus")
        || model_lower.contains("gemini-2")
        || model_lower.contains("gemini-1.5")
        || model_lower.contains("grok-2-vision");

    let thinking = model_lower.contains("claude-sonnet-4")
        || model_lower.contains("claude-opus-4")
        || model_lower.contains("claude-3-7")
        || model_lower.contains("claude-3.7")
        || model_lower.contains("gpt-4.5")
        || model_lower.starts_with("o1")
        || model_lower.starts_with("o3")
        || model_lower.starts_with("o4")
        || model_lower.contains("grok-3-mini")
        || model_lower.contains("reasoner")
        || model_lower.contains("thinking");

    let caching = provider_lower == "anthropic"
        && (model_lower.contains("claude-3-5")
            || model_lower.contains("claude-3-7")
            || model_lower.contains("claude-3.7")
            || model_lower.contains("claude-3.5")
            || model_lower.contains("claude-sonnet-4")
            || model_lower.contains("claude-opus-4")
            || model_lower.contains("claude-haiku-4"));

    let context_window = heuristic_context_window(&model_lower, &provider_lower);

    ModelCapabilities {
        tools,
        vision,
        thinking,
        context_window,
        caching,
    }
}

/// Exact / family catalog hit only (no generic 128k default).
fn known_context_window(model: &str) -> Option<u32> {
    known_capabilities(&model.to_lowercase()).map(|c| c.context_window)
}

fn heuristic_context_window(model_lower: &str, provider_lower: &str) -> u32 {
    if model_lower.contains("gpt-4-32k") {
        32_000
    } else if model_lower.contains("gemini-2.5")
        || model_lower.contains("gemini-2.0")
        || model_lower.contains("gemini-1.5-pro")
    {
        1_000_000
    } else if model_lower.contains("gemini") {
        128_000
    } else if model_lower.contains("claude") {
        200_000
    } else if model_lower.contains("grok-4") || model_lower.contains("grok-3") {
        256_000
    } else if model_lower.contains("grok") {
        131_072
    } else if model_lower.contains("deepseek-r1") || model_lower.contains("deepseek-v3") {
        128_000
    } else if model_lower.contains("deepseek") {
        64_000
    } else if model_lower.contains("gpt-4o")
        || model_lower.starts_with("o1")
        || model_lower.starts_with("o3")
    {
        128_000
    } else if model_lower.contains("gpt-4") {
        8_192
    } else if model_lower.contains("mistral") || model_lower.contains("mixtral") {
        32_000
    } else if provider_lower == "xai" {
        131_072
    } else {
        128_000
    }
}

/// Static map of known models and their capabilities.
fn known_capabilities(model_lower: &str) -> Option<ModelCapabilities> {
    // ── OpenAI ────────────────────────────────────────────────────────
    if model_lower.starts_with("gpt-4o") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: false,
            context_window: 128_000,
            caching: false,
        });
    }
    if model_lower == "gpt-4.5" || model_lower.starts_with("gpt-4.5-") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: true,
            context_window: 128_000,
            caching: false,
        });
    }
    if model_lower.starts_with("gpt-4-turbo") || model_lower.starts_with("gpt-4-") {
        return Some(ModelCapabilities {
            tools: true,
            vision: model_lower.contains("vision"),
            thinking: false,
            context_window: if model_lower.contains("32k") {
                32_000
            } else {
                128_000
            },
            caching: false,
        });
    }
    if model_lower.starts_with("gpt-3.5") {
        return Some(ModelCapabilities {
            tools: true,
            vision: false,
            thinking: false,
            context_window: 16_384,
            caching: false,
        });
    }
    if model_lower.starts_with("o1") {
        return Some(ModelCapabilities {
            tools: true,
            vision: model_lower.contains("gpt-4o"),
            thinking: true,
            context_window: 200_000,
            caching: false,
        });
    }
    if model_lower.starts_with("o3") || model_lower.starts_with("o4") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: true,
            context_window: 200_000,
            caching: false,
        });
    }

    // ── Anthropic ─────────────────────────────────────────────────────
    if model_lower.contains("claude-sonnet-4")
        || model_lower.contains("claude-opus-4")
        || model_lower.contains("claude-haiku-4")
    {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: !model_lower.contains("haiku"),
            context_window: 200_000,
            caching: true,
        });
    }
    if model_lower.contains("claude-3-7") || model_lower.contains("claude-3.7") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: true,
            context_window: 200_000,
            caching: true,
        });
    }
    if model_lower.contains("claude-3-5") || model_lower.contains("claude-3.5") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: false,
            context_window: 200_000,
            caching: true,
        });
    }
    if model_lower.contains("claude-3-opus")
        || model_lower.contains("claude-3-haiku")
        || model_lower.contains("claude-3-sonnet")
    {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: false,
            context_window: 200_000,
            caching: false,
        });
    }

    // ── xAI / Grok ────────────────────────────────────────────────────
    if model_lower.contains("grok-4") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: true,
            context_window: 256_000,
            caching: false,
        });
    }
    if model_lower.contains("grok-3") {
        return Some(ModelCapabilities {
            tools: true,
            vision: false,
            thinking: model_lower.contains("mini") || model_lower.contains("reason"),
            context_window: 131_072,
            caching: false,
        });
    }
    if model_lower.contains("grok-2") {
        return Some(ModelCapabilities {
            tools: true,
            vision: model_lower.contains("vision"),
            thinking: false,
            context_window: 131_072,
            caching: false,
        });
    }
    if model_lower.contains("grok") {
        return Some(ModelCapabilities {
            tools: true,
            vision: false,
            thinking: false,
            context_window: 131_072,
            caching: false,
        });
    }

    // ── DeepSeek ──────────────────────────────────────────────────────
    if model_lower.contains("deepseek-r1") {
        return Some(ModelCapabilities {
            tools: false,
            vision: false,
            thinking: true,
            context_window: 128_000,
            caching: false,
        });
    }
    if model_lower.contains("deepseek-v3") || model_lower.contains("deepseek-chat") {
        return Some(ModelCapabilities {
            tools: true,
            vision: model_lower.contains("chat"),
            thinking: false,
            context_window: 128_000,
            caching: false,
        });
    }

    // ── Gemini ────────────────────────────────────────────────────────
    if model_lower.contains("gemini-2.5-pro")
        || model_lower.contains("gemini-2.5-flash")
        || model_lower.contains("gemini-2.0-flash")
    {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: model_lower.contains("2.5"),
            context_window: 1_000_000,
            caching: false,
        });
    }
    if model_lower.contains("gemini-1.5-pro") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: false,
            context_window: 2_000_000,
            caching: false,
        });
    }
    if model_lower.contains("gemini-1.5-flash") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: false,
            context_window: 1_000_000,
            caching: false,
        });
    }

    // ── Mistral ───────────────────────────────────────────────────────
    if model_lower.contains("mistral-large") || model_lower.contains("pixtral") {
        return Some(ModelCapabilities {
            tools: true,
            vision: model_lower.contains("pixtral"),
            thinking: false,
            context_window: 128_000,
            caching: false,
        });
    }
    if model_lower.contains("codestral") || model_lower.contains("mistral-small") {
        return Some(ModelCapabilities {
            tools: true,
            vision: false,
            thinking: false,
            context_window: 32_000,
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
        assert_eq!(caps.context_window, 128_000);
    }

    #[test]
    fn test_claude_sonnet_4() {
        let caps = detect_capabilities("anthropic", "claude-sonnet-4-20250514");
        assert!(caps.tools);
        assert!(caps.vision);
        assert!(caps.thinking);
        assert!(caps.caching);
        assert_eq!(caps.context_window, 200_000);
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
        assert_eq!(caps.context_window, 128_000);
    }

    #[test]
    fn test_grok_catalog() {
        let caps = detect_capabilities("xai", "grok-4");
        assert_eq!(caps.context_window, 256_000);
        let caps = detect_capabilities("xai", "grok-3");
        assert_eq!(caps.context_window, 131_072);
    }

    #[test]
    fn resolve_prefers_configured_over_catalog() {
        let n = resolve_context_window(
            "anthropic",
            "claude-sonnet-4",
            Some(50_000),
            Some(1_050_000),
            200_000,
        );
        assert_eq!(n, 50_000);
    }

    #[test]
    fn resolve_prefers_api_over_builtin() {
        // Built-in would guess ~128k for unknown; API says 1.05M.
        let n =
            resolve_context_window("custom", "auto/best-coding", None, Some(1_050_000), 200_000);
        assert_eq!(n, 1_050_000);
    }

    #[test]
    fn resolve_uses_catalog_when_unconfigured() {
        let n = resolve_context_window("openai", "gpt-4o", None, None, 200_000);
        assert_eq!(n, 128_000);
        let n = resolve_context_window("google", "gemini-2.5-pro", None, None, 200_000);
        assert_eq!(n, 1_000_000);
    }

    #[test]
    fn resolve_zero_configured_falls_through() {
        let n = resolve_context_window("openai", "gpt-4o", Some(0), None, 200_000);
        assert_eq!(n, 128_000);
    }
}
