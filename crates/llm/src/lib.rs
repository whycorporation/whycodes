pub mod cache;
pub mod capabilities;
pub mod client_identity;
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
pub use client_identity::{HTTP_REFERER, USER_AGENT, X_TITLE};
pub use error_class::{ClassifiedError, ErrorKind, classify, classify_message};
pub use model_catalog::{
    CATALOG_TTL, CatalogFetchRequest, ModelCatalog, base_url_from_provider_config,
    catalog_request_from_config, context_window_for_model_id, fetch_model_catalog,
    fetch_model_catalog_from_request, fetch_model_context_window, normalize_models_url,
    parse_models_json,
};
pub use provider::{LlmProvider, ProviderRegistry};
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
