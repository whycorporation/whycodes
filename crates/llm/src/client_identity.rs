//! Client identity headers for LLM HTTP requests.
//!
//! Gateways (OmniRoute, OpenRouter, LiteLLM, etc.) use these to label traffic
//! as coming from whycodes rather than a generic HTTP client.

use std::sync::OnceLock;

use reqwest::RequestBuilder;

/// `User-Agent` value, e.g. `whycodes/0.1.0`.
pub const USER_AGENT: &str = concat!("whycodes/", env!("CARGO_PKG_VERSION"));

/// OpenRouter-style app title (`X-Title`).
pub const X_TITLE: &str = "whycodes";

/// App / project URL (`HTTP-Referer`).
pub const HTTP_REFERER: &str = "https://why.codes";

/// TCP connect budget. Without this, a dead Tailscale/VPN hop can sit in SYN
/// retries for 20–75s and inflate "Worked for Xs" far above gateway Duration.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Process-wide HTTP client. Reusing one `reqwest::Client` keeps the connection
/// pool and TLS sessions warm across LLM turns (title refine, multi-step tools,
/// catalog fetch). Building a new client per request forces a full handshake
/// every time and can add hundreds of ms–seconds of TTFT.
///
/// No client-wide request timeout — streaming chat completions must be free to
/// run for minutes. Call sites that need a budget (catalog) set `.timeout()` on
/// the request builder.
fn shared_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .pool_max_idle_per_host(8)
            .tcp_nodelay(true)
            .tcp_keepalive(std::time::Duration::from_secs(30))
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

/// Shared HTTP client with the whycodes `User-Agent` as the default.
///
/// Always returns a clone of the process-wide client (cheap; Arc under the hood).
pub fn http_client() -> reqwest::Client {
    shared_client().clone()
}

/// Attach whycodes identity headers used by OpenRouter, OmniRoute, and similar gateways.
///
/// Callers that need to override (e.g. custom provider `headers`, OpenRouter
/// `with_site`) should set their headers *after* this.
pub fn with_identity(req: RequestBuilder) -> RequestBuilder {
    req.header("User-Agent", USER_AGENT)
        .header("X-Title", X_TITLE)
        .header("HTTP-Referer", HTTP_REFERER)
}

/// Start a POST with whycodes identity headers already applied.
pub fn post(url: &str) -> RequestBuilder {
    with_identity(shared_client().post(url))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_starts_with_whycodes() {
        assert!(
            USER_AGENT.starts_with("whycodes/"),
            "USER_AGENT={USER_AGENT}"
        );
        assert!(!USER_AGENT.ends_with('/'));
        assert!(USER_AGENT.len() > "whycodes/".len());
    }

    #[test]
    fn identity_constants() {
        assert_eq!(X_TITLE, "whycodes");
        assert!(HTTP_REFERER.contains("why.codes"));
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

    #[test]
    fn connect_timeout_is_finite() {
        // Guard against regressions that drop connect_timeout and re-inflate
        // "Worked for Xs" on dead VPN/Tailscale hops.
        assert!(CONNECT_TIMEOUT.as_secs() >= 1);
        assert!(CONNECT_TIMEOUT.as_secs() <= 10);
    }
}
