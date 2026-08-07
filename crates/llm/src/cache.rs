//! Prompt caching — OpenCode-compatible policy for Anthropic Messages.
//!
//! OpenCode `packages/llm/src/cache-policy.ts` default `"auto"` places:
//! 1. last tool definition
//! 2. last system content block
//! 3. latest user message (last content part)
//!
//! That boundary stays fixed while a single user turn fans out into many
//! assistant/tool API calls, so every intra-turn step can hit the prefix cache.
//!
//! Anthropic allows at most **4** explicit `cache_control` breakpoints per request.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How aggressively to mark cache breakpoints on Anthropic/Bedrock bodies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CachePolicy {
    /// Tools + system + latest user message (OpenCode auto). Default.
    #[default]
    Auto,
    /// No automatic markers (manual / disabled).
    None,
}

impl CachePolicy {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" | "off" | "false" | "0" => Self::None,
            _ => Self::Auto,
        }
    }
}

/// Legacy config shape (system + trailing message count). Prefer [`CachePolicy`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CacheConfig {
    #[serde(default = "default_true")]
    pub system: bool,
    /// When > 0, mark the latest user message (OpenCode auto). Count is ignored
    /// beyond “enabled”; kept for backward-compatible TOML.
    #[serde(default = "default_messages")]
    pub messages: usize,
    #[serde(default = "default_true")]
    pub tools: bool,
}

fn default_true() -> bool {
    true
}
fn default_messages() -> usize {
    1
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            system: true,
            messages: 1,
            tools: true,
        }
    }
}

impl CacheConfig {
    pub fn disabled() -> Self {
        Self {
            system: false,
            messages: 0,
            tools: false,
        }
    }

    pub fn from_policy(policy: CachePolicy) -> Self {
        match policy {
            CachePolicy::Auto => Self::default(),
            CachePolicy::None => Self::disabled(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.system || self.tools || self.messages > 0
    }

    pub fn cache_control_value() -> Value {
        serde_json::json!({ "type": "ephemeral" })
    }
}

const EPHEMERAL: &str = r#"{"type":"ephemeral"}"#;

fn ephemeral() -> Value {
    serde_json::from_str(EPHEMERAL).unwrap_or_else(|_| serde_json::json!({"type": "ephemeral"}))
}

/// Apply OpenCode-style cache breakpoints onto a fully built Anthropic request body.
///
/// Expects `body` shaped like Messages API: optional `system` (string or array),
/// `tools` array, `messages` array of `{role, content: [...]}`.
///
/// Caps total markers at 4 (Anthropic limit).
pub fn apply_anthropic_cache_policy(body: &mut Value, cfg: &CacheConfig) {
    if !cfg.is_enabled() {
        return;
    }
    let mut budget: u8 = 4;
    let hint = ephemeral();

    // 1) Last tool
    if cfg.tools && budget > 0 {
        if let Some(tools) = body.get_mut("tools").and_then(|t| t.as_array_mut())
            && let Some(last) = tools.last_mut()
            && let Some(obj) = last.as_object_mut()
            && !obj.contains_key("cache_control")
        {
            obj.insert("cache_control".into(), hint.clone());
            budget = budget.saturating_sub(1);
        }
    }

    // 2) System — promote string → text block array, mark last part
    if cfg.system && budget > 0 {
        match body.get("system") {
            Some(Value::String(s)) if !s.is_empty() => {
                body["system"] = serde_json::json!([{
                    "type": "text",
                    "text": s,
                    "cache_control": hint,
                }]);
                budget = budget.saturating_sub(1);
            }
            Some(Value::Array(_)) => {
                if let Some(arr) = body.get_mut("system").and_then(|s| s.as_array_mut())
                    && let Some(last) = arr.last_mut()
                    && let Some(obj) = last.as_object_mut()
                    && !obj.contains_key("cache_control")
                {
                    obj.insert("cache_control".into(), hint.clone());
                    budget = budget.saturating_sub(1);
                }
            }
            _ => {}
        }
    }

    // 3) Latest user message — last content part (text preferred)
    if cfg.messages > 0 && budget > 0 {
        mark_latest_user_message(body, &hint);
    }
}

fn mark_latest_user_message(body: &mut Value, hint: &Value) {
    let Some(messages) = body.get_mut("messages").and_then(|m| m.as_array_mut()) else {
        return;
    };
    let Some(user_idx) = messages
        .iter()
        .rposition(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
    else {
        return;
    };
    let Some(content) = messages[user_idx]
        .get_mut("content")
        .and_then(|c| c.as_array_mut())
    else {
        // String content — upgrade to block so we can attach cache_control
        if let Some(Value::String(text)) = messages[user_idx].get("content").cloned() {
            messages[user_idx]["content"] = serde_json::json!([{
                "type": "text",
                "text": text,
                "cache_control": hint,
            }]);
        }
        return;
    };
    if content.is_empty() {
        return;
    }
    // Prefer last text part; else last part of any type (tool_result-only turns).
    let mark_at = content
        .iter()
        .rposition(|p| p.get("type").and_then(|t| t.as_str()) == Some("text"))
        .unwrap_or(content.len() - 1);
    if let Some(obj) = content[mark_at].as_object_mut()
        && !obj.contains_key("cache_control")
    {
        obj.insert("cache_control".into(), hint.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_marks_system_tools_and_latest_user() {
        let mut body = serde_json::json!({
            "system": "You are Whycode.",
            "tools": [
                {"name": "read", "description": "r", "input_schema": {"type": "object"}},
                {"name": "grep", "description": "g", "input_schema": {"type": "object"}},
            ],
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "first"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "ok"}]},
                {"role": "user", "content": [{"type": "text", "text": "latest"}]},
            ]
        });
        apply_anthropic_cache_policy(&mut body, &CacheConfig::default());

        assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
        assert!(body["tools"][0].get("cache_control").is_none());
        assert_eq!(body["tools"][1]["cache_control"]["type"], "ephemeral");
        assert!(
            body["messages"][0]["content"][0]
                .get("cache_control")
                .is_none()
        );
        assert_eq!(
            body["messages"][2]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
    }

    #[test]
    fn none_policy_leaves_body() {
        let mut body = serde_json::json!({
            "system": "x",
            "tools": [{"name": "a", "input_schema": {}}],
            "messages": [{"role": "user", "content": [{"type": "text", "text": "hi"}]}],
        });
        apply_anthropic_cache_policy(&mut body, &CacheConfig::disabled());
        assert_eq!(body["system"], "x");
        assert!(body["tools"][0].get("cache_control").is_none());
    }

    #[test]
    fn parse_policy() {
        assert_eq!(CachePolicy::parse("auto"), CachePolicy::Auto);
        assert_eq!(CachePolicy::parse("none"), CachePolicy::None);
        assert_eq!(CachePolicy::parse("off"), CachePolicy::None);
    }
}
