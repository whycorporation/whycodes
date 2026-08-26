//! OAuth login engine and token storage for WhyCodes.
//!
//! Built-in WhyCodes has **no** subscription-login clients. OAuth specs come
//! from auth plugins (`plugin.json` with `"kind": "auth"`). Tokens live in
//! `auth.json` under the data directory with `0600` permissions on Unix.
//!
//! Security rules enforced here:
//! - The token store is never world-readable; we chmod 0600 and refuse to
//!   use a store with looser permissions.
//! - Secrets never appear in logs at any tracing level.
//! - `Debug` impls redact token material.

pub mod cca;
pub mod discover;
pub mod error;
pub mod flow;
pub mod pkce;
pub mod plugin;
pub mod providers;
pub mod spec;
pub mod store;
pub mod token;

pub use error::AuthError;
pub use spec::{
    FlowKind, InferenceIdentity, ProviderSpec, TokenEncoding, clear_registry, inference_identity,
    register_spec, registered_providers, spec_for, spec_get, suggested_models, supports_oauth,
    validate,
};
pub use store::TokenStore;
pub use token::{OAuthToken, ProviderAuth};

/// Names of currently loaded auth plugins, sorted. Empty until plugins load.
pub fn oauth_providers() -> Vec<String> {
    registered_providers()
}
