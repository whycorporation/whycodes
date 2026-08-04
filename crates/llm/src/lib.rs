pub mod anthropic;
pub mod capabilities;
pub mod client_identity;
pub mod custom;
pub mod deepseek;
pub mod fallback;
pub mod google;
pub mod groq;
pub mod mistral;
pub mod model_catalog;
pub mod ollama;
pub mod openai;
pub mod openai_compat;
pub mod openrouter;
pub mod provider;
pub mod retry;
pub mod together;
pub mod types;
pub mod xai;

pub use capabilities::{
    detect_capabilities, resolve_context_window, ModelCapabilities,
};
pub use client_identity::{HTTP_REFERER, USER_AGENT, X_TITLE};
pub use model_catalog::{
    base_url_from_provider_config, catalog_request_from_config, fetch_model_catalog,
    fetch_model_catalog_from_request, normalize_models_url, parse_models_json, CatalogFetchRequest,
    ModelCatalog, CATALOG_TTL,
};
pub use provider::{LlmProvider, ProviderRegistry};
pub use types::*;
