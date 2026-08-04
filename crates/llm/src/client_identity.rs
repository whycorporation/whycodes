//! Client identity headers for LLM HTTP requests.
//!
//! Gateways (OmniRoute, OpenRouter, LiteLLM, etc.) use these to label traffic
//! as coming from whycode rather than a generic HTTP client.

use std::sync::OnceLock;

use reqwest::RequestBuilder;

/// `User-Agent` value, e.g. `whycode/0.1.0`.
pub const USER_AGENT: &str = concat!("whycode/", env!("CARGO_PKG_VERSION"));

/// OpenRouter-style app title (`X-Title`).
pub const X_TITLE: &str = "whycode";

/// App / project URL (`HTTP-Referer`).
pub const HTTP_REFERER: &str = "https://github.com/whycorporation/whycode";

/// Process-wide HTTP client. Reusing one `reqwest::Client` keeps the connection
/// pool and TLS sessions warm across LLM turns (title refine, multi-step tools,
/// catalog fetch). Building a new client per request forces a full handshake
/// every time and can add hundreds of ms–seconds of TTFT.
fn shared_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .pool_max_idle_per_host(8)
            .tcp_nodelay(true)
            .tcp_keepalive(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

/// Shared HTTP client with the whycode `User-Agent` as the default.
///
/// Always returns a clone of the process-wide client (cheap; Arc under the hood).
pub fn http_client() -> reqwest::Client {
    shared_client().clone()
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
    with_identity(shared_client().post(url))
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

    #[test]
    fn http_client_is_shared() {
        // Process-wide client: same static reference on every call.
        assert!(std::ptr::eq(shared_client(), shared_client()));
        // Clones are cheap and keep the pool warm.
        let _a = http_client();
        let _b = http_client();
    }
}
