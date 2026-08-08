use thiserror::Error;

#[derive(Debug, Error)]
pub enum AuthError {
    // NOTE: the supported list is tripwire-tested against OAUTH_PROVIDERS
    // (conformance_error_message_lists_every_provider) — keep it in sync.
    #[error(
        "provider `{0}` does not support OAuth login (supported: anthropic, openai, github-copilot, google)"
    )]
    UnsupportedProvider(String),

    #[error("not logged in for provider `{0}` — run `whycode auth login {0}`")]
    NotLoggedIn(String),

    #[error("token store has insecure permissions; expected 0600 on {0}")]
    InsecureStorePermissions(String),

    #[error("OAuth flow was cancelled or timed out: {0}")]
    FlowCancelled(String),

    #[error("OAuth provider returned an error: {0}")]
    Provider(String),

    #[error("token exchange failed: {0}")]
    TokenExchange(String),

    #[error("token refresh failed for `{0}`: {1}")]
    Refresh(String, String),

    #[error("could not open a browser; visit this URL manually: {0}")]
    BrowserUnavailable(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, AuthError>;
