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
    assert!(!is_local_llm_endpoint("http://172."));
    assert!(!is_local_llm_endpoint("http://172.abc"));
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
