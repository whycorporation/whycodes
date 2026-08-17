//! Extended thinking support for LLM providers (Anthropic Claude, etc.).
//!
//! Provides a `ThinkingConfig` struct that can be attached to `LlmRequest`
//! via the `thinking` field (`Option<serde_json::Value>`).

use serde::{Deserialize, Serialize};

/// Configuration for extended thinking features.
///
/// Supported by Anthropic Claude 3.7+ and some other providers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThinkingConfig {
    /// Maximum tokens to allocate for the thinking process.
    /// Must be less than `max_tokens` (which includes both thinking and visible output).
    pub budget_tokens: u32,
    /// Whether extended thinking is enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// OpenAI / Grok / o-series: `low` | `medium` | `high`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

fn default_enabled() -> bool {
    true
}

impl Default for ThinkingConfig {
    fn default() -> Self {
        Self {
            budget_tokens: 4000,
            enabled: true,
            reasoning_effort: None,
        }
    }
}

impl ThinkingConfig {
    /// Create a new thinking config with the given budget.
    pub fn new(budget_tokens: u32) -> Self {
        Self {
            budget_tokens,
            enabled: true,
            reasoning_effort: None,
        }
    }

    /// Disable thinking.
    pub fn disabled() -> Self {
        Self {
            budget_tokens: 0,
            enabled: false,
            reasoning_effort: None,
        }
    }

    /// Convert to the JSON value expected by the Anthropic API.
    ///
    /// Returns `{"type": "enabled", "budget_tokens": N}` when enabled,
    /// or `{"type": "disabled"}` when disabled.
    pub fn to_anthropic_value(&self) -> serde_json::Value {
        if self.enabled {
            serde_json::json!({
                "type": "enabled",
                "budget_tokens": self.budget_tokens,
            })
        } else {
            serde_json::json!({"type": "disabled"})
        }
    }

    /// Build a ThinkingConfig from the `thinking` field of an LlmRequest.
    pub fn from_request_value(value: Option<&serde_json::Value>) -> Option<Self> {
        let value = value?;
        // Legacy boolean form: thinking: true/false
        if let Some(enabled) = value.as_bool() {
            return Some(if enabled {
                Self::default()
            } else {
                Self::disabled()
            });
        }
        // Structured form — Anthropic wire uses `type: enabled|disabled`.
        let enabled = if let Some(kind) = value.get("type").and_then(|v| v.as_str()) {
            kind != "disabled"
        } else {
            value
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true)
        };
        let budget_tokens = value
            .get("budget_tokens")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .unwrap_or(4000);

        let budget = if budget_tokens == 0 {
            4000
        } else {
            budget_tokens
        };
        let reasoning_effort = value
            .get("reasoning_effort")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| matches!(s.as_str(), "low" | "medium" | "high" | "minimal"));
        Some(Self {
            budget_tokens: budget,
            enabled,
            reasoning_effort,
        })
    }

    /// Default `reasoning_effort` for OpenAI-compat families that accept it.
    pub fn default_effort(provider: &str, model: &str) -> Option<&'static str> {
        let p = provider.to_ascii_lowercase();
        let m = model.to_ascii_lowercase();
        if matches!(p.as_str(), "openai" | "xai" | "openrouter" | "deepseek")
            || m.starts_with("o1")
            || m.starts_with("o3")
            || m.starts_with("o4")
            || m.contains("gpt-5")
            || m.contains("grok-4")
            || m.contains("grok-3-mini")
            || m.contains("reasoner")
        {
            Some("medium")
        } else {
            None
        }
    }

    /// Anthropic Messages `thinking` object, or `None` when unset.
    pub fn apply_anthropic(
        body: &mut serde_json::Value,
        request_thinking: Option<&serde_json::Value>,
    ) {
        let Some(cfg) = Self::from_request_value(request_thinking) else {
            return;
        };
        body["thinking"] = cfg.to_anthropic_value();
        // `max_tokens` includes thinking + visible output. A default 4k cap
        // with a 4k budget leaves almost no room for the answer.
        if cfg.enabled {
            let current_max = body
                .get("max_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(4096);
            let min_max = u64::from(cfg.budget_tokens).saturating_add(4096);
            if current_max < min_max {
                body["max_tokens"] = serde_json::json!(min_max);
            }
        }
    }

    /// OpenAI-compat `reasoning_effort` when the request asks for thinking.
    pub fn apply_openai_effort(
        body: &mut serde_json::Value,
        request_thinking: Option<&serde_json::Value>,
    ) {
        let Some(cfg) = Self::from_request_value(request_thinking) else {
            return;
        };
        if !cfg.enabled {
            return;
        }
        if let Some(effort) = cfg.reasoning_effort {
            body["reasoning_effort"] = serde_json::Value::String(effort);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default() {
        let cfg = ThinkingConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.budget_tokens, 4000);
    }

    #[test]
    fn test_to_anthropic_enabled() {
        let cfg = ThinkingConfig::new(2000);
        let v = cfg.to_anthropic_value();
        assert_eq!(v["type"], "enabled");
        assert_eq!(v["budget_tokens"], 2000);
    }

    #[test]
    fn test_to_anthropic_disabled() {
        let cfg = ThinkingConfig::disabled();
        let v = cfg.to_anthropic_value();
        assert_eq!(v["type"], "disabled");
    }

    #[test]
    fn test_from_legacy_bool() {
        let v = serde_json::json!(true);
        let cfg = ThinkingConfig::from_request_value(Some(&v)).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.budget_tokens, 4000);
    }

    #[test]
    fn test_from_structured() {
        let v = serde_json::json!({"enabled": true, "budget_tokens": 8000});
        let cfg = ThinkingConfig::from_request_value(Some(&v)).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.budget_tokens, 8000);
    }

    #[test]
    fn from_structured_reads_reasoning_effort() {
        let v = serde_json::json!({"enabled": true, "reasoning_effort": "high"});
        let cfg = ThinkingConfig::from_request_value(Some(&v)).unwrap();
        assert_eq!(cfg.reasoning_effort.as_deref(), Some("high"));
        let mut body = serde_json::json!({});
        ThinkingConfig::apply_openai_effort(&mut body, Some(&v));
        assert_eq!(body["reasoning_effort"], "high");
    }

    #[test]
    fn type_disabled_disables_thinking() {
        let v = serde_json::json!({"type": "disabled"});
        let cfg = ThinkingConfig::from_request_value(Some(&v)).unwrap();
        assert!(!cfg.enabled);
    }

    #[test]
    fn apply_anthropic_raises_max_tokens_above_budget() {
        let v = serde_json::json!({"enabled": true, "budget_tokens": 8000});
        let mut body = serde_json::json!({"max_tokens": 1024});
        ThinkingConfig::apply_anthropic(&mut body, Some(&v));
        assert_eq!(body["thinking"]["budget_tokens"], 8000);
        assert_eq!(body["max_tokens"], 8000 + 4096);
    }

    #[test]
    fn default_effort_for_grok_and_openai() {
        assert_eq!(
            ThinkingConfig::default_effort("xai", "grok-4"),
            Some("medium")
        );
        assert_eq!(
            ThinkingConfig::default_effort("openai", "gpt-5"),
            Some("medium")
        );
        assert_eq!(
            ThinkingConfig::default_effort("anthropic", "claude-sonnet-4"),
            None
        );
    }
}
