use async_trait::async_trait;
use futures::stream::Stream;
use std::pin::Pin;
use whycode_core::types::{LlmRequest, LlmResponse, StreamEvent};

/// Trait for LLM providers (Anthropic, OpenAI, Google, etc.)
#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;
    fn default_base_url(&self) -> &str;

    /// Send a request and get a complete response
    async fn complete(
        &self,
        request: &LlmRequest,
        api_key: &str,
        model: &str,
    ) -> whycode_core::Result<LlmResponse>;

    /// Send a request and stream the response
    async fn stream(
        &self,
        request: &LlmRequest,
        api_key: &str,
        model: &str,
    ) -> whycode_core::Result<Pin<Box<dyn Stream<Item = whycode_core::Result<StreamEvent>> + Send>>>;
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

    /// Register a custom provider from config.
    /// This enables dynamically-added providers from config.toml.
    pub fn register_from_config(&mut self, config: &whycode_config::Config) {
        for (name, pc) in &config.providers {
            // Skip built-in providers that already exist
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
