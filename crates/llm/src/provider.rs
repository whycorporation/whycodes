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
    providers: std::collections::HashMap<String, Box<dyn LlmProvider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self {
            providers: std::collections::HashMap::new(),
        }
    }

    pub fn register(&mut self, provider: Box<dyn LlmProvider>) {
        self.providers.insert(provider.name().to_string(), provider);
    }

    pub fn get(&self, name: &str) -> Option<&dyn LlmProvider> {
        self.providers.get(name).map(|p| p.as_ref())
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(super::anthropic::AnthropicProvider::new()));
        registry.register(Box::new(super::openai::OpenAiProvider::new()));
        registry.register(Box::new(super::google::GoogleProvider::new()));
        registry
    }
}
