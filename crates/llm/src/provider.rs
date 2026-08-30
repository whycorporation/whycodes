use async_trait::async_trait;
use futures::stream::Stream;
use std::pin::Pin;
use whycodes_core::types::{LlmRequest, LlmResponse, StreamEvent};

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
    ) -> whycodes_core::Result<LlmResponse>;

    /// Send a request and stream the response
    async fn stream(
        &self,
        request: &LlmRequest,
        api_key: &str,
        model: &str,
    ) -> whycodes_core::Result<Pin<Box<dyn Stream<Item = whycodes_core::Result<StreamEvent>> + Send>>>;
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
mod tests {
    use super::*;
    use whycodes_config::Config;
    use whycodes_core::types::ProviderConfig;

    fn config_entry(name: &str) -> ProviderConfig {
        ProviderConfig {
            name: name.to_string(),
            api_key: None,
            api_base: None,
            base_url: None,
            headers: None,
            models: vec![],
            tool_arguments: None,
            extra: Default::default(),
        }
    }

    #[test]
    fn default_registry_exposes_builtin_providers() {
        let registry = ProviderRegistry::default();
        for name in [
            "anthropic",
            "openai",
            "github-copilot",
            "google",
            "google-antigravity",
            "deepseek",
            "openrouter",
            "ollama",
            "xai",
            "mistral",
            "together",
            "groq",
        ] {
            assert!(registry.get(name).is_some(), "{name} missing");
        }
        assert!(registry.get("nope").is_none());
        let names = registry.names();
        assert!(names.contains(&"google-antigravity".to_string()));
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    #[test]
    fn register_from_config_adds_custom_and_keeps_builtin() {
        let mut registry = ProviderRegistry::default();
        let mut config = Config::default();
        config
            .providers
            .insert("anthropic".to_string(), config_entry("anthropic"));
        config
            .providers
            .insert("acme".to_string(), config_entry("acme"));

        registry.register_from_config(&config);

        assert!(registry.get("acme").is_some(), "custom provider not added");
        assert_eq!(registry.get("anthropic").unwrap().name(), "anthropic");
    }

    #[test]
    fn register_from_config_applies_ollama_base_url() {
        let mut registry = ProviderRegistry::default();
        let mut config = Config::default();
        let mut pc = config_entry("ollama");
        pc.base_url = Some("http://127.0.0.1:4554".into());
        config.providers.insert("ollama".to_string(), pc);
        registry.register_from_config(&config);
        assert_eq!(
            registry.get("ollama").unwrap().default_base_url(),
            "http://127.0.0.1:4554/api/chat"
        );
    }

    #[test]
    fn register_from_config_applies_anthropic_base_url() {
        let mut registry = ProviderRegistry::default();
        let mut config = Config::default();
        let mut pc = config_entry("anthropic");
        pc.base_url = Some("http://127.0.0.1:4554".into());
        config.providers.insert("anthropic".to_string(), pc);
        registry.register_from_config(&config);
        assert_eq!(
            registry.get("anthropic").unwrap().default_base_url(),
            "http://127.0.0.1:4554/v1/messages"
        );
    }

    #[test]
    fn empty_config_registers_nothing_new() {
        let mut registry = ProviderRegistry::new();
        registry.register_from_config(&Config::default());
        assert!(registry.get("anything").is_none());
    }
}
