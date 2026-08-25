/// Provider fallback chain — tries each provider in order until one succeeds.
///
/// A `FallbackChain` is a sequence of (provider_name, model) pairs.
/// When `complete` is called, each provider is tried in order;
/// the first successful response is returned.
use std::collections::HashMap;
use tracing::warn;

use whycodes_core::types::{LlmRequest, LlmResponse};

use super::provider::ProviderRegistry;

/// A list of fallback providers, each with a model to use.
pub struct FallbackChain {
    /// List of (provider_name, model) pairs to try in order.
    entries: Vec<(String, String)>,
    /// Shared API keys: provider_name -> api_key.
    api_keys: HashMap<String, String>,
}

impl FallbackChain {
    /// Create a new fallback chain.
    ///
    /// `entries` is a list of `(provider_name, model)` pairs — primary first, then fallbacks.
    /// `api_keys` maps provider names to API keys.
    pub fn new(entries: Vec<(String, String)>, api_keys: HashMap<String, String>) -> Self {
        Self { entries, api_keys }
    }

    /// Send a complete request through the fallback chain.
    ///
    /// Tries each provider in order. Returns the first successful response.
    /// Returns an error if all providers fail.
    pub async fn complete(
        &self,
        request: &LlmRequest,
        registry: &ProviderRegistry,
    ) -> whycodes_core::Result<LlmResponse> {
        let mut last_error: Option<whycodes_core::Error> = None;

        for (idx, (provider_name, model)) in self.entries.iter().enumerate() {
            let provider = match registry.get(provider_name) {
                Some(p) => p,
                None => {
                    warn!(
                        "Fallback: provider '{}' not found in registry (entry {}/{})",
                        provider_name,
                        idx + 1,
                        self.entries.len()
                    );
                    continue;
                }
            };

            let api_key = self
                .api_keys
                .get(provider_name)
                .map(|s| s.as_str())
                .unwrap_or("");

            match provider.complete(request, api_key, model).await {
                Ok(response) => {
                    if idx > 0 {
                        warn!(
                            "Fallback succeeded on provider '{}' (tried {} providers first)",
                            provider_name, idx
                        );
                    }
                    return Ok(response);
                }
                Err(e) => {
                    warn!(
                        "Fallback: provider '{}' failed (entry {}/{}): {e}",
                        provider_name,
                        idx + 1,
                        self.entries.len()
                    );
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            whycodes_core::Error::Provider("All fallback providers exhausted".to_string())
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{LlmProvider, ProviderRegistry};
    use crate::scripted::{ScriptedProvider, ScriptedStep};
    use whycodes_core::types::{LlmRequest, Message, MessageContent, Role};

    fn req() -> LlmRequest {
        LlmRequest {
            system: "s".into(),
            messages: std::sync::Arc::from(vec![Message {
                role: Role::User,
                content: MessageContent::Text("hi".into()),
                tool_call_id: None,
                name: None,
                created_at: None,
            }]),
            tools: vec![],
            max_tokens: Some(8),
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            thinking: None,
            use_prompt_cache: false,
        }
    }

    fn keys(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[tokio::test]
    async fn empty_chain_exhausts() {
        let chain = FallbackChain::new(vec![], HashMap::new());
        let err = chain
            .complete(&req(), &ProviderRegistry::new())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("exhausted"), "{err}");
    }

    #[tokio::test]
    async fn missing_provider_is_skipped() {
        let chain = FallbackChain::new(vec![("nope".into(), "m".into())], keys(&[("nope", "k")]));
        let err = chain
            .complete(&req(), &ProviderRegistry::new())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("exhausted"), "{err}");
    }

    #[tokio::test]
    async fn first_success_short_circuits() {
        let mut registry = ProviderRegistry::new();
        registry.register(Box::new(ScriptedProvider::named(
            "primary",
            [ScriptedStep::Text("ok".into())],
        )));
        registry.register(Box::new(ScriptedProvider::named(
            "backup",
            [ScriptedStep::Text("unused".into())],
        )));
        let chain = FallbackChain::new(
            vec![
                ("primary".into(), "m1".into()),
                ("backup".into(), "m2".into()),
            ],
            keys(&[("primary", "k"), ("backup", "k")]),
        );
        let resp = chain.complete(&req(), &registry).await.unwrap();
        assert_eq!(resp.model, "m1");
        let text = match &resp.content[0] {
            whycodes_core::types::ContentBlock::Text { text } => text.as_str(),
            other => panic!("expected text, got {other:?}"),
        };
        assert_eq!(text, "ok");
    }

    #[tokio::test]
    async fn second_provider_used_after_first_fails() {
        let mut registry = ProviderRegistry::new();
        registry.register(Box::new(ScriptedProvider::named(
            "primary",
            [ScriptedStep::Error("down".into())],
        )));
        registry.register(Box::new(ScriptedProvider::named(
            "backup",
            [ScriptedStep::Text("recovered".into())],
        )));
        let chain = registry.get_with_fallback(
            ("primary".into(), "m1".into()),
            vec![("backup".into(), "m2".into())],
            keys(&[("primary", "k"), ("backup", "k")]),
        );
        let resp = chain.complete(&req(), &registry).await.unwrap();
        assert_eq!(resp.model, "m2");
        let text = match &resp.content[0] {
            whycodes_core::types::ContentBlock::Text { text } => text.as_str(),
            other => panic!("expected text, got {other:?}"),
        };
        assert_eq!(text, "recovered");
    }

    #[tokio::test]
    async fn all_failing_providers_surface_last_error() {
        let mut registry = ProviderRegistry::new();
        registry.register(Box::new(ScriptedProvider::named(
            "a",
            [ScriptedStep::Error("first".into())],
        )));
        registry.register(Box::new(ScriptedProvider::named(
            "b",
            [ScriptedStep::Error("second".into())],
        )));
        let chain = FallbackChain::new(
            vec![("a".into(), "m".into()), ("b".into(), "m".into())],
            keys(&[("a", "k"), ("b", "k")]),
        );
        let err = chain.complete(&req(), &registry).await.unwrap_err();
        assert!(err.to_string().contains("second"), "{err}");
    }

    #[test]
    fn provider_name_and_default_url_on_scripted() {
        let p = ScriptedProvider::text("x");
        assert_eq!(p.name(), "script");
        assert_eq!(p.default_base_url(), "http://script.invalid");
    }
}
