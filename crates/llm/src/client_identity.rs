//! Client identity headers for LLM HTTP requests.
//!
//! Core traffic identifies as WhyCodes. Auth plugins may attach extra
//! inference headers (or a different User-Agent) when the user has
//! installed them; the default binary never impersonates another product.

use std::sync::OnceLock;

use reqwest::RequestBuilder;

/// `User-Agent` value, e.g. `whycodes/0.4.0`.
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
            .unwrap_or_else(|e| {
                tracing::debug!("shared HTTP client builder failed, using default: {e}");
                reqwest::Client::new()
            })
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

/// Identity for a named LLM provider.
///
/// With no auth plugin, this is the honest WhyCodes identity. A loaded
/// plugin's `inference` object may replace the User-Agent and add headers
/// (unofficial subscription plugins live outside the default install).
pub fn with_plugin_identity(req: RequestBuilder, provider: &str) -> RequestBuilder {
    match whycodes_auth::inference_identity(provider) {
        Some(id)
            if id.user_agent.as_deref().is_some_and(|s| !s.is_empty())
                || !id.headers.is_empty() =>
        {
            let mut req = match id.user_agent.as_deref().filter(|s| !s.is_empty()) {
                Some(ua) => req.header("User-Agent", ua),
                None => req.header("User-Agent", USER_AGENT),
            };
            for (k, v) in id.headers {
                if k.eq_ignore_ascii_case("user-agent") {
                    continue;
                }
                req = req.header(k, v);
            }
            req
        }
        _ => with_identity(req),
    }
}

/// POST with [`with_plugin_identity`] already applied.
pub fn post_for_provider(url: &str, provider: &str) -> RequestBuilder {
    with_plugin_identity(shared_client().post(url), provider)
}

#[cfg(test)]
#[path = "client_identity_tests.rs"]
mod tests;
