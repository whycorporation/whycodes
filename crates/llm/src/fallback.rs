/// Provider fallback chain — tries each provider in order until one succeeds.
///
/// A `FallbackChain` is a sequence of (provider_name, model) pairs.
/// When `complete` is called, each provider is tried in order;
/// the first successful response is returned.
use std::collections::HashMap;
use tracing::warn;

use whycode_core::types::{LlmRequest, LlmResponse};

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
    ) -> whycode_core::Result<LlmResponse> {
        let mut last_error: Option<whycode_core::Error> = None;

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
                            provider_name,
                            idx
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
            whycode_core::Error::Provider("All fallback providers exhausted".to_string())
        }))
    }
}
