//! Google Antigravity subscription LLM provider.
//!
//! Always routes through the Antigravity Code Assist control plane
//! (`daily-cloudcode-pa.googleapis.com`). OAuth tokens come from
//! `whycodes auth login google-antigravity`.

use super::codeassist;
use crate::provider::{LlmProvider, ProviderResponseFuture, ProviderStreamFuture};
use whycodes_core::types::LlmRequest;

pub struct AntigravityProvider {
    name: String,
}

impl AntigravityProvider {
    pub fn new() -> Self {
        Self {
            name: "google-antigravity".to_string(),
        }
    }
}

impl LlmProvider for AntigravityProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_base_url(&self) -> &str {
        "https://daily-cloudcode-pa.googleapis.com/v1internal"
    }

    fn complete<'a>(
        &'a self,
        request: &'a LlmRequest,
        api_key: &'a str,
        model: &'a str,
    ) -> ProviderResponseFuture<'a> {
        Box::pin(async move { codeassist::complete_antigravity(request, api_key, model).await })
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
        api_key: &'a str,
        model: &'a str,
    ) -> ProviderStreamFuture<'a> {
        Box::pin(async move { codeassist::stream_antigravity(request, api_key, model).await })
    }
}

impl Default for AntigravityProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity() {
        let p = AntigravityProvider::new();
        assert_eq!(p.name(), "google-antigravity");
        assert_eq!(
            p.default_base_url(),
            "https://daily-cloudcode-pa.googleapis.com/v1internal"
        );
    }
}
