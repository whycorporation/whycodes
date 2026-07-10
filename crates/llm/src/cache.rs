//! Prompt caching support for Anthropic's API.
//!
//! Anthropic supports cache breakpoints in messages — adding `cache_control: {"type": "ephemeral"}`
//! to content blocks. The system prompt and the last N messages can be cached.

use serde::{Deserialize, Serialize};

/// Configuration for prompt caching.
///
/// Controls which parts of a request get cached by the provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CacheConfig {
    /// Whether to cache the system prompt.
    #[serde(default)]
    pub system: bool,
    /// Number of trailing messages to cache (e.g., 2 = cache last 2 messages).
    /// 0 means no message caching.
    #[serde(default)]
    pub messages: usize,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            system: true,
            messages: 2,
        }
    }
}

impl CacheConfig {
    /// Create a config that caches nothing.
    pub fn disabled() -> Self {
        Self {
            system: false,
            messages: 0,
        }
    }

    /// Create a config caching system prompt + last N messages.
    pub fn new(system: bool, messages: usize) -> Self {
        Self { system, messages }
    }

    /// Returns the cache_control annotation value for Anthropic.
    pub fn cache_control_value() -> serde_json::Value {
        serde_json::json!({"type": "ephemeral"})
    }

    /// Whether any caching is enabled at all.
    pub fn is_enabled(&self) -> bool {
        self.system || self.messages > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default() {
        let cfg = CacheConfig::default();
        assert!(cfg.system);
        assert_eq!(cfg.messages, 2);
        assert!(cfg.is_enabled());
    }

    #[test]
    fn test_disabled() {
        let cfg = CacheConfig::disabled();
        assert!(!cfg.system);
        assert_eq!(cfg.messages, 0);
        assert!(!cfg.is_enabled());
    }

    #[test]
    fn test_cache_control_value() {
        let v = CacheConfig::cache_control_value();
        assert_eq!(v["type"], "ephemeral");
    }
}
