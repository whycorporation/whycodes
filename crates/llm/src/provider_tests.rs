use super::*;
use whycodes_config::Config;
use whycodes_core::types::ProviderConfig;

fn config_entry(name: &str) -> ProviderConfig {
    ProviderConfig {
        name: name.to_string(),
        api_key: None,
        api_base: None,
        base_url: None,
        headers: None,
        models: vec![],
        tool_arguments: None,
        extra: Default::default(),
    }
}

#[test]
fn default_registry_exposes_builtin_providers() {
    let registry = ProviderRegistry::default();
    for name in [
        "anthropic",
        "openai",
        "github-copilot",
        "google",
        "google-antigravity",
        "deepseek",
        "openrouter",
        "ollama",
        "xai",
        "mistral",
        "together",
        "groq",
    ] {
        assert!(registry.get(name).is_some(), "{name} missing");
    }
    assert!(registry.get("nope").is_none());
    let names = registry.names();
    assert!(names.contains(&"google-antigravity".to_string()));
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
}

#[test]
fn register_from_config_adds_custom_and_keeps_builtin() {
    let mut registry = ProviderRegistry::default();
    let mut config = Config::default();
    config
        .providers
        .insert("anthropic".to_string(), config_entry("anthropic"));
    config
        .providers
        .insert("acme".to_string(), config_entry("acme"));

    registry.register_from_config(&config);

    assert!(registry.get("acme").is_some(), "custom provider not added");
    assert_eq!(registry.get("anthropic").unwrap().name(), "anthropic");
}

#[test]
fn register_from_config_applies_ollama_base_url() {
    let mut registry = ProviderRegistry::default();
    let mut config = Config::default();
    let mut pc = config_entry("ollama");
    pc.base_url = Some("http://127.0.0.1:4554".into());
    config.providers.insert("ollama".to_string(), pc);
    registry.register_from_config(&config);
    assert_eq!(
        registry.get("ollama").unwrap().default_base_url(),
        "http://127.0.0.1:4554/api/chat"
    );
}

#[test]
fn register_from_config_applies_anthropic_base_url() {
    let mut registry = ProviderRegistry::default();
    let mut config = Config::default();
    let mut pc = config_entry("anthropic");
    pc.base_url = Some("http://127.0.0.1:4554".into());
    config.providers.insert("anthropic".to_string(), pc);
    registry.register_from_config(&config);
    assert_eq!(
        registry.get("anthropic").unwrap().default_base_url(),
        "http://127.0.0.1:4554/v1/messages"
    );
}

#[test]
fn empty_config_registers_nothing_new() {
    let mut registry = ProviderRegistry::new();
    registry.register_from_config(&Config::default());
    assert!(registry.get("anything").is_none());
}

#[test]
fn register_from_config_applies_openai_base_url() {
    let mut registry = ProviderRegistry::default();
    let mut config = Config::default();
    let mut pc = config_entry("openai");
    pc.base_url = Some("http://127.0.0.1:4554/v1".into());
    config.providers.insert("openai".to_string(), pc);
    registry.register_from_config(&config);
    let openai_url = registry.get("openai").unwrap().default_base_url();
    assert!(openai_url.contains("127.0.0.1:4554"), "{openai_url}");
}

#[test]
fn register_from_config_skips_existing_builtin_wrappers() {
    let mut registry = ProviderRegistry::default();
    let before = registry.get("groq").unwrap().default_base_url().to_string();
    let mut config = Config::default();
    let mut pc = config_entry("groq");
    pc.base_url = Some("http://127.0.0.1:9/v1".into());
    config.providers.insert("groq".to_string(), pc);
    registry.register_from_config(&config);
    assert_eq!(registry.get("groq").unwrap().default_base_url(), before);
}
