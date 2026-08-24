//! Google Antigravity subscription LLM provider.
//!
//! Always routes through the Antigravity Code Assist control plane
//! (`daily-cloudcode-pa.googleapis.com`). OAuth tokens come from
//! `whycode auth login google-antigravity`.

use super::codeassist;
use crate::provider::LlmProvider;
use async_trait::async_trait;
use futures::stream::Stream;
use std::pin::Pin;
use whycode_core::types::{LlmRequest, LlmResponse, StreamEvent};

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

#[async_trait]
impl LlmProvider for AntigravityProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_base_url(&self) -> &str {
        "https://daily-cloudcode-pa.googleapis.com/v1internal"
    }

    async fn complete(
        &self,
        request: &LlmRequest,
        api_key: &str,
        model: &str,
    ) -> whycode_core::Result<LlmResponse> {
        codeassist::complete_antigravity(request, api_key, model).await
    }

    async fn stream(
        &self,
        request: &LlmRequest,
        api_key: &str,
        model: &str,
    ) -> whycode_core::Result<Pin<Box<dyn Stream<Item = whycode_core::Result<StreamEvent>> + Send>>>
    {
        codeassist::stream_antigravity(request, api_key, model).await
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
