//! OAuth subscription login for Whycode.
//!
//! Lets users authenticate with an existing provider subscription (Claude
//! Pro/Max, ChatGPT Plus/Pro, GitHub Copilot, Google/Gemini) instead of an
//! API key, using the public PKCE OAuth client ids that first-party and
//! community terminal agents use. Tokens live in `auth.json` under the
//! whycode data directory with `0600` permissions on Unix.
//!
//! Security rules enforced here:
//! - The token store is never world-readable; we chmod 0600 and refuse to
//!   use a store with looser permissions.
//! - Secrets never appear in logs at any tracing level.
//! - `Debug` impls redact token material.

pub mod error;
pub mod flow;
pub mod pkce;
pub mod providers;
pub mod store;
pub mod token;

pub use error::AuthError;
pub use store::TokenStore;
pub use token::{OAuthToken, ProviderAuth};

/// Providers that support OAuth subscription login.
pub const OAUTH_PROVIDERS: &[&str] = &["anthropic", "openai", "github-copilot", "google"];
