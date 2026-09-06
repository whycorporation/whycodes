use super::*;

#[test]
fn public_reexports_are_wired() {
    let _ = USER_AGENT;
    let _ = HTTP_REFERER;
    let _ = CATALOG_TTL;
    let _ = DEFAULT_ANTHROPIC_MESSAGES_URL;
    let _ = DEFAULT_OLLAMA_HOST;
    let registry = ProviderRegistry::new();
    assert!(registry.get("missing").is_none());
    let scripted = ScriptedProvider::text("ok");
    assert_eq!(scripted.name(), "script");
    assert!(!provider_requires_api_key("ollama", None));
}
