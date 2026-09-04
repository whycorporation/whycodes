//! Shared URL helpers for LLM providers.
//!
//! Built-in clients used to ignore config `base_url` and always demand an API
//! key. Local proxies (LiteLLM, Ollama, a tunnel on `:4554`) live on loopback
//! and often need neither.

use whycodes_config::Config;
use whycodes_core::types::ProviderConfig;

/// Default Anthropic Messages API URL.
pub const DEFAULT_ANTHROPIC_MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";

/// Whether this provider needs a credential before the first request.
///
/// `ollama` never does. Any other id is skipped only when config points at a
/// loopback / private `base_url` (local proxy). Cloud Anthropic/OpenAI still
/// require a key.
pub fn provider_requires_api_key(provider: &str, config: Option<&Config>) -> bool {
    if provider.eq_ignore_ascii_case("ollama") {
        return false;
    }
    let Some(cfg) = config else {
        return true;
    };
    let Some(pc) = cfg.get_provider(provider) else {
        return true;
    };
    !provider_config_skips_api_key(pc)
}

/// True when this provider block talks to a local/private host.
pub fn provider_config_skips_api_key(pc: &ProviderConfig) -> bool {
    pc.base_url
        .as_deref()
        .or(pc.api_base.as_deref())
        .is_some_and(is_local_llm_endpoint)
}

/// Loopback, RFC1918, `*.localhost`, `*.local`, or `host.docker.internal`.
pub fn is_local_llm_endpoint(base: &str) -> bool {
    let raw = base.trim();
    if raw.is_empty() {
        return false;
    }
    let with_scheme = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("http://{raw}")
    };
    let host = match reqwest::Url::parse(&with_scheme) {
        Ok(url) => url.host_str().unwrap_or("").to_ascii_lowercase(),
        Err(e) => {
            tracing::debug!(error = %e, raw, "LLM endpoint URL parse failed; using host heuristic");
            let lower = raw.to_ascii_lowercase();
            let stripped = lower
                .rsplit_once("://")
                .map(|(_, rest)| rest)
                .unwrap_or(&lower);
            stripped
                .split(['/', '?', '#'])
                .next()
                .unwrap_or(stripped)
                .split(':')
                .next()
                .unwrap_or(stripped)
                .trim_matches(['[', ']'])
                .to_string()
        }
    };
    is_local_host(&host)
}

fn is_local_host(host: &str) -> bool {
    matches!(
        host,
        "localhost" | "127.0.0.1" | "::1" | "0.0.0.0" | "host.docker.internal"
    ) || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.starts_with("10.")
        || host.starts_with("192.168.")
        || is_rfc1918_172(host)
}

fn is_rfc1918_172(host: &str) -> bool {
    let Some(rest) = host.strip_prefix("172.") else {
        return false;
    };
    let Some((second, _)) = rest.split_once('.') else {
        return false;
    };
    second.parse::<u8>().is_ok_and(|n| (16..=31).contains(&n))
}

/// Turn a configured host into Anthropic's native `POST /v1/messages` URL.
pub fn normalize_anthropic_messages_url(base: Option<&str>) -> String {
    let Some(raw) = base.map(str::trim).filter(|s| !s.is_empty()) else {
        return DEFAULT_ANTHROPIC_MESSAGES_URL.to_string();
    };
    let url = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("http://{raw}")
    };
    let url = url.trim_end_matches('/');
    if url.ends_with("/v1/messages") || url.ends_with("/messages") {
        return url.to_string();
    }
    let host = url
        .trim_end_matches("/v1/chat/completions")
        .trim_end_matches("/chat/completions")
        .trim_end_matches("/v1");
    let host = host.trim_end_matches('/');
    format!("{host}/v1/messages")
}

#[cfg(test)]
mod tests {
    use super::*;
    use whycodes_core::types::ProviderConfig;

    fn pc(base: Option<&str>) -> ProviderConfig {
        ProviderConfig {
            name: "anthropic".into(),
            api_key: None,
            api_base: None,
            base_url: base.map(str::to_string),
            headers: None,
            models: vec![],
            tool_arguments: None,
            extra: Default::default(),
        }
    }

    #[test]
    fn ollama_never_requires_key() {
        assert!(!provider_requires_api_key("ollama", None));
        assert!(!provider_requires_api_key("Ollama", None));
        assert!(provider_requires_api_key("anthropic", None));
        assert!(provider_requires_api_key("openai", None));
    }

    #[test]
    fn anthropic_cloud_still_requires_key() {
        let mut cfg = Config::default();
        cfg.providers.insert("anthropic".into(), pc(None));
        assert!(provider_requires_api_key("anthropic", Some(&cfg)));
    }

    #[test]
    fn anthropic_loopback_proxy_skips_key() {
        let mut cfg = Config::default();
        cfg.providers
            .insert("anthropic".into(), pc(Some("http://127.0.0.1:4554")));
        assert!(!provider_requires_api_key("anthropic", Some(&cfg)));
        cfg.providers
            .insert("anthropic".into(), pc(Some("localhost:8080")));
        assert!(!provider_requires_api_key("anthropic", Some(&cfg)));
    }

    #[test]
    fn public_proxy_still_requires_key() {
        let mut cfg = Config::default();
        cfg.providers
            .insert("anthropic".into(), pc(Some("https://proxy.example.com/v1")));
        assert!(provider_requires_api_key("anthropic", Some(&cfg)));
    }

    #[test]
    fn local_host_detection() {
        assert!(is_local_llm_endpoint("http://127.0.0.1:4554"));
        assert!(is_local_llm_endpoint("http://localhost:11434/v1"));
        assert!(is_local_llm_endpoint("10.0.0.5:9000"));
        assert!(is_local_llm_endpoint("http://192.168.1.2/v1"));
        assert!(is_local_llm_endpoint("http://172.16.0.2"));
        assert!(is_local_llm_endpoint("http://host.docker.internal:4000"));
        assert!(!is_local_llm_endpoint("https://api.anthropic.com"));
        assert!(!is_local_llm_endpoint("https://api.openai.com/v1"));
        assert!(!is_local_llm_endpoint(""));
        assert!(is_local_llm_endpoint("http://foo.localhost/v1"));
        assert!(is_local_llm_endpoint("http://printer.local"));
        assert!(is_local_llm_endpoint("http://0.0.0.0:9"));
        assert!(is_local_llm_endpoint("172.16.0.2"));
        assert!(!is_local_llm_endpoint("172.15.0.2"));
        assert!(!is_local_llm_endpoint("172.32.0.2"));
        assert!(!is_local_llm_endpoint("http://["));
    }

    #[test]
    fn anthropic_url_normalization() {
        assert_eq!(
            normalize_anthropic_messages_url(None),
            DEFAULT_ANTHROPIC_MESSAGES_URL
        );
        assert_eq!(
            normalize_anthropic_messages_url(Some("http://127.0.0.1:4554")),
            "http://127.0.0.1:4554/v1/messages"
        );
        assert_eq!(
            normalize_anthropic_messages_url(Some("127.0.0.1:4554")),
            "http://127.0.0.1:4554/v1/messages"
        );
        assert_eq!(
            normalize_anthropic_messages_url(Some("http://127.0.0.1:4554/v1")),
            "http://127.0.0.1:4554/v1/messages"
        );
        assert_eq!(
            normalize_anthropic_messages_url(Some("http://127.0.0.1:4554/v1/chat/completions")),
            "http://127.0.0.1:4554/v1/messages"
        );
        assert_eq!(
            normalize_anthropic_messages_url(Some("http://127.0.0.1:4554/v1/messages")),
            "http://127.0.0.1:4554/v1/messages"
        );
    }

    #[test]
    fn config_skip_uses_api_base_too() {
        let mut pc = pc(None);
        pc.api_base = Some("http://127.0.0.1:9".into());
        assert!(provider_config_skips_api_key(&pc));
    }

    #[test]
    fn missing_provider_in_config_still_requires_key() {
        let cfg = Config::default();
        assert!(provider_requires_api_key("anthropic", Some(&cfg)));
    }

    #[test]
    fn blank_anthropic_base_uses_default() {
        assert_eq!(
            normalize_anthropic_messages_url(Some("   ")),
            DEFAULT_ANTHROPIC_MESSAGES_URL
        );
        assert_eq!(
            normalize_anthropic_messages_url(Some("http://127.0.0.1:9/messages")),
            "http://127.0.0.1:9/messages"
        );
    }
}
