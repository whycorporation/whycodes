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
    /// Whether the provider can compact a conversation server-side for this
    /// model (Anthropic on-demand compaction, beta `compact-2026-09-04`).
    pub compaction: bool,
}

impl Default for ModelCapabilities {
    fn default() -> Self {
        Self {
            tools: true,
            vision: false,
            thinking: false,
            context_window: 128_000,
            caching: false,
            compaction: false,
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

    let compaction = native_compaction(&provider_lower, &model_lower);
    if let Some(caps) = known_capabilities(&model_lower) {
        return ModelCapabilities { compaction, ..caps };
    }

    // Heuristic fallback based on provider and model name
    let tools = matches!(
        provider_lower.as_str(),
        "openai"
            | "anthropic"
            | "google"
            | "google-antigravity"
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
        compaction,
    }
}

/// Anthropic models documented for on-demand compaction
/// (<https://platform.claude.com/docs/en/build-with-claude/compaction-on-demand>).
const ANTHROPIC_COMPACTION_MODELS: &[&str] = &[
    "claude-fable-5-1",
    "claude-mythos-5-1",
    "claude-fable-5",
    "claude-mythos-5",
    "claude-mythos-preview",
    "claude-opus-5-5",
    "claude-opus-5",
    "claude-opus-4-8",
    "claude-opus-4-7",
    "claude-opus-4-6",
    "claude-sonnet-5-5",
    "claude-sonnet-5",
    "claude-sonnet-4-6",
    "claude-haiku-5-5",
];

/// Native compaction: only the first-party `anthropic` provider, and only a
/// listed model id, bare or with a `-YYYYMMDD` snapshot suffix (so
/// `claude-opus-5` does not also admit an unlisted `claude-opus-5-1`).
fn native_compaction(provider_lower: &str, model_lower: &str) -> bool {
    provider_lower == "anthropic"
        && ANTHROPIC_COMPACTION_MODELS.iter().any(|id| {
            model_lower.strip_prefix(id).is_some_and(|rest| {
                rest.is_empty()
                    || rest
                        .strip_prefix('-')
                        .is_some_and(|d| d.len() == 8 && d.bytes().all(|b| b.is_ascii_digit()))
            })
        })
}

/// Exact / family catalog hit only (no generic 128k default).
fn known_context_window(model: &str) -> Option<u32> {
    known_capabilities(&model.to_lowercase()).map(|c| c.context_window)
}

#[cfg(test)]
pub(crate) fn heuristic_context_window_for_tests(model: &str, provider: &str) -> u32 {
    heuristic_context_window(&model.to_lowercase(), &provider.to_lowercase())
}

fn heuristic_context_window(model_lower: &str, provider_lower: &str) -> u32 {
    #[cfg(test)]
    if model_lower == "__force_zero_window__" {
        return 0;
    }
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
            compaction: false,
        });
    }
    if model_lower == "gpt-4.5" || model_lower.starts_with("gpt-4.5-") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: true,
            context_window: 128_000,
            caching: false,
            compaction: false,
        });
    }
    if model_lower.starts_with("gpt-5") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: true,
            context_window: 200_000,
            caching: false,
            compaction: false,
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
            compaction: false,
        });
    }
    if model_lower.starts_with("gpt-3.5") {
        return Some(ModelCapabilities {
            tools: true,
            vision: false,
            thinking: false,
            context_window: 16_384,
            caching: false,
            compaction: false,
        });
    }
    if model_lower.starts_with("o1") {
        return Some(ModelCapabilities {
            tools: true,
            vision: model_lower.contains("gpt-4o"),
            thinking: true,
            context_window: 200_000,
            caching: false,
            compaction: false,
        });
    }
    if model_lower.starts_with("o3") || model_lower.starts_with("o4") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: true,
            context_window: 200_000,
            caching: false,
            compaction: false,
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
            compaction: false,
        });
    }
    if model_lower.contains("claude-3-7") || model_lower.contains("claude-3.7") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: true,
            context_window: 200_000,
            caching: true,
            compaction: false,
        });
    }
    if model_lower.contains("claude-3-5") || model_lower.contains("claude-3.5") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: false,
            context_window: 200_000,
            caching: true,
            compaction: false,
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
            compaction: false,
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
            compaction: false,
        });
    }
    if model_lower.contains("grok-3") {
        return Some(ModelCapabilities {
            tools: true,
            vision: false,
            thinking: model_lower.contains("mini") || model_lower.contains("reason"),
            context_window: 131_072,
            caching: false,
            compaction: false,
        });
    }
    if model_lower.contains("grok-2") {
        return Some(ModelCapabilities {
            tools: true,
            vision: model_lower.contains("vision"),
            thinking: false,
            context_window: 131_072,
            caching: false,
            compaction: false,
        });
    }
    if model_lower.contains("grok") {
        return Some(ModelCapabilities {
            tools: true,
            vision: false,
            thinking: false,
            context_window: 131_072,
            caching: false,
            compaction: false,
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
            compaction: false,
        });
    }
    if model_lower.contains("deepseek-v3") || model_lower.contains("deepseek-chat") {
        return Some(ModelCapabilities {
            tools: true,
            vision: model_lower.contains("chat"),
            thinking: false,
            context_window: 128_000,
            caching: false,
            compaction: false,
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
            compaction: false,
        });
    }
    if model_lower.contains("gemini-1.5-pro") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: false,
            context_window: 2_000_000,
            caching: false,
            compaction: false,
        });
    }
    if model_lower.contains("gemini-1.5-flash") {
        return Some(ModelCapabilities {
            tools: true,
            vision: true,
            thinking: false,
            context_window: 1_000_000,
            caching: false,
            compaction: false,
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
            compaction: false,
        });
    }
    if model_lower.contains("codestral") || model_lower.contains("mistral-small") {
        return Some(ModelCapabilities {
            tools: true,
            vision: false,
            thinking: false,
            context_window: 32_000,
            caching: false,
            compaction: false,
        });
    }

    None
}

#[cfg(test)]
#[path = "capabilities_tests.rs"]
mod tests;
