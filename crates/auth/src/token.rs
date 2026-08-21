//! Token types. `Debug` impls redact secret material so a stray log line
//! can never leak a credential.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// A stored OAuth credential for one provider.
#[derive(Clone, Serialize, Deserialize)]
pub struct OAuthToken {
    /// Access token sent as the bearer credential.
    pub access_token: String,
    /// Refresh token, when the provider issues one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// Expiry of the access token, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    /// Provider-specific extras (e.g. GitHub Copilot's short-lived API token,
    /// Google's project id). Never logged.
    #[serde(default)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl OAuthToken {
    /// True when the token is expired or within 60s of expiring.
    /// Tokens without an expiry are treated as valid.
    pub fn is_expired(&self) -> bool {
        match self.expires_at {
            Some(at) => Utc::now() + Duration::seconds(60) >= at,
            None => false,
        }
    }

    pub fn can_refresh(&self) -> bool {
        self.refresh_token.is_some()
    }
}

impl std::fmt::Debug for OAuthToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthToken")
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("expires_at", &self.expires_at)
            .field("extra_keys", &self.extra.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Per-provider auth record persisted in the token store.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderAuth {
    /// How the credential was obtained: "oauth" today; leaves room for
    /// "imported" (credential discovery) later.
    pub method: String,
    pub token: OAuthToken,
}

#[cfg(test)]
#[path = "token_tests.rs"]
mod tests;
