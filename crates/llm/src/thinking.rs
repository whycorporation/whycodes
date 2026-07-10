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
}

fn default_enabled() -> bool {
    true
}

impl Default for ThinkingConfig {
    fn default() -> Self {
        Self {
            budget_tokens: 4000,
            enabled: true,
        }
    }
}

impl ThinkingConfig {
    /// Create a new thinking config with the given budget.
    pub fn new(budget_tokens: u32) -> Self {
        Self {
            budget_tokens,
            enabled: true,
        }
    }

    /// Disable thinking.
    pub fn disabled() -> Self {
        Self {
            budget_tokens: 0,
            enabled: false,
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
        // Structured form
        let enabled = value
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let budget_tokens = value
            .get("budget_tokens")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .unwrap_or(4000);

        let budget = if budget_tokens == 0 { 4000 } else { budget_tokens };
        Some(Self {
            budget_tokens: budget,
            enabled,
        })
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
}
