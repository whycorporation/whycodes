//! Client identity headers for LLM HTTP requests.
//!
//! Gateways (OmniRoute, OpenRouter, LiteLLM, etc.) use these to label traffic
//! as coming from whycode rather than a generic HTTP client.

use reqwest::RequestBuilder;

/// `User-Agent` value, e.g. `whycode/0.1.0`.
pub const USER_AGENT: &str = concat!("whycode/", env!("CARGO_PKG_VERSION"));

/// OpenRouter-style app title (`X-Title`).
pub const X_TITLE: &str = "whycode";

/// App / project URL (`HTTP-Referer`).
pub const HTTP_REFERER: &str = "https://github.com/whycorporation/whycode";

/// Shared HTTP client with the whycode `User-Agent` as the default.
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Attach whycode identity headers used by OpenRouter, OmniRoute, and similar gateways.
///
/// Callers that need to override (e.g. custom provider `headers`, OpenRouter
/// `with_site`) should set their headers *after* this.
pub fn with_identity(req: RequestBuilder) -> RequestBuilder {
    req.header("User-Agent", USER_AGENT)
        .header("X-Title", X_TITLE)
        .header("HTTP-Referer", HTTP_REFERER)
}

/// Start a POST with whycode identity headers already applied.
pub fn post(url: &str) -> RequestBuilder {
    with_identity(http_client().post(url))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_starts_with_whycode() {
        assert!(
            USER_AGENT.starts_with("whycode/"),
            "USER_AGENT={USER_AGENT}"
        );
        assert!(!USER_AGENT.ends_with('/'));
        assert!(USER_AGENT.len() > "whycode/".len());
    }

    #[test]
    fn identity_constants() {
        assert_eq!(X_TITLE, "whycode");
        assert!(HTTP_REFERER.contains("whycode"));
    }

    #[test]
    fn http_client_builds() {
        let _ = http_client();
    }
}
