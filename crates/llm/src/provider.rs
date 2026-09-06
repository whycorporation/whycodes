use futures::stream::Stream;
use std::future::Future;
use std::pin::Pin;
use whycodes_core::types::{LlmRequest, LlmResponse, StreamEvent};

/// Boxed, sendable future returned by [`LlmProvider::complete`].
pub type ProviderResponseFuture<'a> =
    Pin<Box<dyn Future<Output = whycodes_core::Result<LlmResponse>> + Send + 'a>>;

/// Stream of provider events returned after [`LlmProvider::stream`] opens successfully.
pub type ProviderEventStream =
    Pin<Box<dyn Stream<Item = whycodes_core::Result<StreamEvent>> + Send>>;

/// Boxed, sendable future returned by [`LlmProvider::stream`].
pub type ProviderStreamFuture<'a> =
    Pin<Box<dyn Future<Output = whycodes_core::Result<ProviderEventStream>> + Send + 'a>>;

/// Trait for LLM providers (Anthropic, OpenAI, Google, etc.)
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;
    fn default_base_url(&self) -> &str;

    /// Send a request and get a complete response
    fn complete<'a>(
        &'a self,
        request: &'a LlmRequest,
        api_key: &'a str,
        model: &'a str,
    ) -> ProviderResponseFuture<'a>;

    /// Send a request and stream the response
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
        api_key: &'a str,
        model: &'a str,
    ) -> ProviderStreamFuture<'a>;
}

/// Registry of available LLM providers
pub struct ProviderRegistry {
    /// Provider id → implementation. Local config keys only (FxHash).
    providers: rustc_hash::FxHashMap<String, Box<dyn LlmProvider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self {
            providers: rustc_hash::FxHashMap::default(),
        }
    }

    pub fn register(&mut self, provider: Box<dyn LlmProvider>) {
        self.providers.insert(provider.name().to_string(), provider);
    }

    pub fn get(&self, name: &str) -> Option<&dyn LlmProvider> {
        self.providers.get(name).map(|p| p.as_ref())
    }

    /// Sorted built-in (and later config-registered) provider ids.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.providers.keys().cloned().collect();
        names.sort();
        names
    }

    /// Register a custom provider from config.
    /// This enables dynamically-added providers from config.toml.
    pub fn register_from_config(&mut self, config: &whycodes_config::Config) {
        for (name, pc) in &config.providers {
            match name.as_str() {
                "ollama" => {
                    self.providers.insert(
                        name.clone(),
                        Box::new(super::providers::ollama::OllamaProvider::from_config(pc)),
                    );
                    continue;
                }
                "anthropic" => {
                    self.providers.insert(
                        name.clone(),
                        Box::new(super::providers::anthropic::AnthropicProvider::from_config(
                            pc,
                        )),
                    );
                    continue;
                }
                "openai" => {
                    self.providers.insert(
                        name.clone(),
                        Box::new(super::providers::openai::OpenAiProvider::from_config(pc)),
                    );
                    continue;
                }
                _ => {}
            }
            // Skip other built-in providers that already exist
            if self.providers.contains_key(name) {
                continue;
            }
            // Create a CustomProvider for this config entry
            let custom = Box::new(super::providers::custom::CustomProvider::from_config(pc));
            self.providers.insert(name.clone(), custom);
        }
    }
    ///
    /// `primary` is a `(provider_name, model)` pair to try first.
    /// `fallbacks` is a list of fallback `(provider_name, model)` pairs.
    /// Returns a `FallbackChain` ready to call `.complete()` on.
    pub fn get_with_fallback(
        &self,
        primary: (String, String),
        fallbacks: Vec<(String, String)>,
        api_keys: std::collections::HashMap<String, String>,
    ) -> super::fallback::FallbackChain {
        let mut entries = Vec::with_capacity(1 + fallbacks.len());
        entries.push(primary);
        entries.extend(fallbacks);
        super::fallback::FallbackChain::new(entries, api_keys)
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(
            super::providers::anthropic::AnthropicProvider::new(),
        ));
        registry.register(Box::new(super::providers::openai::OpenAiProvider::new()));
        registry.register(Box::new(super::providers::copilot::CopilotProvider::new()));
        registry.register(Box::new(super::providers::google::GoogleProvider::new()));
        registry.register(Box::new(
            super::providers::antigravity::AntigravityProvider::new(),
        ));
        registry.register(Box::new(super::providers::deepseek::DeepSeekProvider::new()));
        registry.register(Box::new(
            super::providers::openrouter::OpenRouterProvider::new(),
        ));
        registry.register(Box::new(super::providers::ollama::OllamaProvider::new()));
        registry.register(Box::new(super::providers::xai::XaiProvider::new()));
        registry.register(Box::new(super::providers::mistral::MistralProvider::new()));
        registry.register(Box::new(super::providers::together::TogetherProvider::new()));
        registry.register(Box::new(super::providers::groq::GroqProvider::new()));
        registry
    }
}

#[cfg(test)]
#[path = "provider_tests.rs"]
mod tests;
