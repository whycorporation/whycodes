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
        || model_lower.contains("claude-4")
        || model_lower.contains("claude-3-7")
        || model_lower.contains("claude-3.7")
        || model_lower.contains("gpt-4.5")
        || model_lower.contains("gpt-5")
        || model_lower.starts_with("o1")
        || model_lower.starts_with("o3")
        || model_lower.starts_with("o4")
        || model_lower.contains("grok-4")
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
    if model_lower.starts_with("gpt-5") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: true,
            context_window: 200_000,
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
        assert!(caps.thinking);
        let caps = detect_capabilities("xai", "grok-3");
        assert_eq!(caps.context_window, 131_072);
    }

    #[test]
    fn gpt5_and_claude_4_think() {
        assert!(detect_capabilities("openai", "gpt-5").thinking);
        assert!(detect_capabilities("openai", "gpt-5-mini").thinking);
        assert!(detect_capabilities("anthropic", "claude-opus-4-1").thinking);
        assert!(detect_capabilities("anthropic", "claude-4-sonnet").thinking);
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

    #[test]
    fn resolve_unknown_model_uses_heuristic_default() {
        assert_eq!(
            resolve_context_window("vendor", "mystery", None, None, 4096),
            128_000
        );
    }

    #[test]
    fn openai_family_catalog_entries() {
        let g45 = detect_capabilities("openai", "GPT-4.5-Preview");
        assert!(g45.thinking && g45.vision && g45.context_window == 128_000);

        let turbo = detect_capabilities("openai", "gpt-4-turbo-2024");
        assert!(turbo.tools && !turbo.thinking && turbo.context_window == 128_000);

        let k32 = detect_capabilities("openai", "gpt-4-32k");
        assert_eq!(k32.context_window, 32_000);

        let legacy = detect_capabilities("openai", "gpt-3.5-turbo");
        assert!(legacy.tools && !legacy.vision && legacy.context_window == 16_384);

        let o1 = detect_capabilities("openai", "o1-mini");
        assert!(o1.thinking && !o1.vision && o1.context_window == 200_000);

        let o4 = detect_capabilities("openai", "o4-mini");
        assert!(o4.thinking && o4.vision && o4.context_window == 200_000);
    }

    #[test]
    fn anthropic_family_catalog_entries() {
        let haiku4 = detect_capabilities("anthropic", "claude-haiku-4-5");
        assert!(haiku4.tools && haiku4.caching && !haiku4.thinking);

        let c37 = detect_capabilities("anthropic", "claude-3-7-sonnet-latest");
        assert!(c37.thinking && c37.caching);

        let c35 = detect_capabilities("anthropic", "claude-3.5-haiku-latest");
        assert!(!c35.thinking && c35.caching);

        let opus3 = detect_capabilities("anthropic", "claude-3-opus-20240229");
        assert!(opus3.vision && !opus3.caching && !opus3.thinking);
        assert_eq!(opus3.context_window, 200_000);
    }

    #[test]
    fn grok_family_catalog_entries() {
        let g4 = detect_capabilities("xai", "grok-4-fast");
        assert!(g4.thinking && g4.vision && g4.context_window == 256_000);

        let g3 = detect_capabilities("xai", "grok-3-beta");
        assert!(!g3.thinking && g3.context_window == 131_072);

        let g3mini = detect_capabilities("xai", "grok-3-mini-reasoner");
        assert!(g3mini.thinking);

        let g2v = detect_capabilities("xai", "grok-2-vision");
        assert!(g2v.vision && !g2v.thinking);

        let bare = detect_capabilities("xai", "grok-beta");
        assert_eq!(bare.context_window, 131_072);
    }

    #[test]
    fn deepseek_and_gemini_catalog_entries() {
        let v3 = detect_capabilities("deepseek", "deepseek-v3");
        assert!(v3.tools && !v3.thinking);

        let chat = detect_capabilities("deepseek", "deepseek-chat");
        assert!(chat.vision);

        let p25 = detect_capabilities("google", "gemini-2.5-pro");
        assert!(p25.thinking && p25.context_window == 1_000_000);

        let f20 = detect_capabilities("google", "gemini-2.0-flash");
        assert!(!f20.thinking && f20.context_window == 1_000_000);

        let p15 = detect_capabilities("google", "gemini-1.5-pro");
        assert_eq!(p15.context_window, 2_000_000);

        let f15 = detect_capabilities("google", "gemini-1.5-flash");
        assert_eq!(f15.context_window, 1_000_000);
    }

    #[test]
    fn mistral_family_catalog_entries() {
        let large = detect_capabilities("mistral", "mistral-large-latest");
        assert!(large.tools && large.context_window == 128_000);

        let pix = detect_capabilities("mistral", "pixtral-large");
        assert!(pix.vision);

        let code = detect_capabilities("mistral", "codestral-latest");
        assert_eq!(code.context_window, 32_000);
    }

    #[test]
    fn heuristics_cover_models_missing_from_the_catalog() {
        let cases = [
            ("gateway", "custom-gpt-4-32k-endpoint", 32_000),
            ("google", "gemini-flash-experimental", 128_000),
            ("anthropic", "claude-future", 200_000),
            ("deepseek", "deepseek-coder-v2", 64_000),
            ("mistral", "mistral-medium-latest", 32_000),
            ("xai", "unknown-xai-model", 131_072),
            ("vendor", "totally-mystery", 128_000),
        ];
        for (provider, model, want) in cases {
            assert_eq!(
                detect_capabilities(provider, model).context_window,
                want,
                "{model}"
            );
        }
        assert!(detect_capabilities("vendor", "llava-vision").vision);
        assert!(detect_capabilities("ollama", "plain-model").tools);
        assert!(!detect_capabilities("vendor", "plain-model").tools);
    }
}
