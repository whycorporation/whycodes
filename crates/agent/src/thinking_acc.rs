//! Accumulate streamed thinking + Anthropic signatures into persistable blocks.

use whycodes_core::types::ContentBlock;

#[derive(Default)]
pub struct ThinkingAccumulator {
    open: Option<OpenThinking>,
    closed: Vec<ContentBlock>,
}

struct OpenThinking {
    text: String,
    signature: Option<String>,
}

impl ThinkingAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        match &mut self.open {
            Some(open) => {
                if !open.text.is_empty() && text.starts_with(open.text.as_str()) {
                    let rest = &text[open.text.len()..];
                    open.text.push_str(rest);
                } else {
                    open.text.push_str(text);
                }
            }
            None => {
                self.open = Some(OpenThinking {
                    text: text.to_string(),
                    signature: None,
                });
            }
        }
    }

    pub fn push_signature(&mut self, signature: &str) {
        if signature.is_empty() {
            return;
        }
        match &mut self.open {
            Some(open) => open.signature = Some(signature.to_string()),
            None => {
                self.open = Some(OpenThinking {
                    text: String::new(),
                    signature: Some(signature.to_string()),
                });
            }
        }
    }

    pub fn push_redacted(&mut self, data: &str) {
        self.flush();
        if !data.is_empty() {
            self.closed.push(ContentBlock::RedactedThinking {
                data: data.to_string(),
            });
        }
    }

    /// Close the open block so a following text/tool starts a new thought.
    pub fn flush(&mut self) {
        if let Some(open) = self.open.take()
            && (!open.text.is_empty() || open.signature.is_some())
        {
            self.closed.push(ContentBlock::Thinking {
                text: open.text,
                signature: open.signature,
            });
        }
    }

    pub fn into_blocks(mut self) -> Vec<ContentBlock> {
        self.flush();
        self.closed
    }
}

/// Enable extended thinking on the request when the model/config supports it.
pub fn attach_thinking_request(
    request: &mut whycodes_core::types::LlmRequest,
    provider: &str,
    model: &str,
    model_cfg: Option<&whycodes_core::types::ModelConfig>,
    effort_override: Option<&str>,
) {
    let want = match model_cfg.and_then(|m| m.thinking) {
        Some(flag) => flag,
        None => whycodes_llm::capabilities::detect_capabilities(provider, model).thinking,
    };
    if !want {
        return;
    }
    if request.thinking.is_some() {
        return;
    }
    let mut payload = serde_json::json!({
        "enabled": true,
        "budget_tokens": 4000,
    });
    if let Some(effort) =
        whycodes_llm::thinking::ThinkingConfig::resolve_effort(provider, model, effort_override)
    {
        payload["reasoning_effort"] = serde_json::Value::String(effort.as_str().to_string());
    }
    request.thinking = Some(payload);
}

/// Raise thinking budget / reasoning effort for an `ultrathink` turn.
pub fn apply_ultrathink(request: &mut whycodes_core::types::LlmRequest) {
    let mut payload = request.thinking.take().unwrap_or_else(|| {
        serde_json::json!({
            "enabled": true,
            "budget_tokens": 4000,
        })
    });
    payload["enabled"] = serde_json::json!(true);
    let budget = payload
        .get("budget_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(4000);
    if budget < 16_000 {
        payload["budget_tokens"] = serde_json::json!(16_000);
    }
    payload["reasoning_effort"] = serde_json::json!("high");
    request.thinking = Some(payload);
}

#[cfg(test)]
#[path = "thinking_acc_tests.rs"]
mod tests;
