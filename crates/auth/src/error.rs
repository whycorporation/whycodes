use thiserror::Error;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error(
        "provider `{0}` does not support OAuth login (install an auth plugin for that provider)"
    )]
    UnsupportedProvider(String),

    #[error("not logged in for provider `{0}` — run `whycodes auth login {0}`")]
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

    #[error("credential import needs explicit approval for this source path: {0}")]
    ConsentRequired(String),

    #[error("refusing to import a symlinked credential file: {0}")]
    SymlinkRejected(String),

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

#[cfg(test)]
mod tests {
    #[test]
    fn error_module_loads() {
        assert!(!module_path!().is_empty());
    }
}
