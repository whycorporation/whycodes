pub mod cache;
pub mod capabilities;
pub mod client_identity;
pub mod endpoint;
pub mod error_class;
pub mod fallback;
pub mod model_catalog;
pub mod oauth_refresh;
pub mod openai_compat;
pub mod provider;
pub mod providers;
pub mod race;
pub mod rate_limit;
pub mod response_cache;
pub mod retry;
pub mod scripted;
pub mod thinking;
pub mod token_counter;
pub mod transport;
pub mod types;
pub mod usage_dump;

pub use providers::{
    anthropic, antigravity, codeassist, codex, copilot, custom, deepseek, google, groq, mistral,
    ollama, openai, openrouter, together, xai,
};

pub use cache::{CacheConfig, CachePolicy, apply_anthropic_cache_policy};
pub use capabilities::{ModelCapabilities, detect_capabilities, resolve_context_window};
pub use client_identity::{
    HTTP_REFERER, USER_AGENT, X_TITLE, post_for_provider, with_plugin_identity,
};
pub use endpoint::{
    DEFAULT_ANTHROPIC_MESSAGES_URL, is_local_llm_endpoint, normalize_anthropic_messages_url,
    provider_config_skips_api_key, provider_requires_api_key,
};
pub use error_class::{ClassifiedError, ErrorKind, classify, classify_message};
pub use model_catalog::{
    CATALOG_TTL, CatalogFetchRequest, ModelCatalog, base_url_from_provider_config,
    catalog_request_from_config, context_window_for_model_id, fetch_model_catalog,
    fetch_model_catalog_from_request, fetch_model_context_window, normalize_models_url,
    parse_models_json,
};
pub use openai_compat::{error_source_chain, stream_chunk_error};
pub use provider::{LlmProvider, ProviderRegistry};
pub use providers::ollama::{DEFAULT_OLLAMA_HOST, normalize_ollama_chat_url};
pub use race::{RaceOutcome, StreamTarget, stream_raced};
pub use response_cache::{CachedText, ResponseCache, text_only_response};
pub use retry::{RetryPolicy, execute_with_policy, retry_with_backoff};
pub use scripted::{ScriptedProvider, ScriptedStep};
pub use thinking::{ReasoningEffort, ThinkingConfig};
pub use transport::{
    LlmTransport, StreamTurn, StreamTurnOpts, default_transport, format_turn_error,
    user_facing_error,
};
pub use types::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_reexports_are_wired() {
        let _ = USER_AGENT;
        let _ = HTTP_REFERER;
        let _ = CATALOG_TTL;
        let _ = DEFAULT_ANTHROPIC_MESSAGES_URL;
        let _ = DEFAULT_OLLAMA_HOST;
        let registry = ProviderRegistry::new();
        assert!(registry.get("missing").is_none());
        let scripted = ScriptedProvider::text("ok");
        assert_eq!(scripted.name(), "script");
        assert!(!provider_requires_api_key("ollama", None));
    }
}
