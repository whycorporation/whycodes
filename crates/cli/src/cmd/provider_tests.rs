use super::*;

#[test]
fn default_config_has_named_agents() {
    let cfg = whycodes_config::Config::default();
    assert!(cfg.get_agent("build").is_some());
    assert!(cfg.get_agent("plan").is_some());
}

#[test]
fn provider_printer_helpers() {
    assert_eq!(provider_key_status(true), "set");
    assert_eq!(provider_key_status(false), "not set");
    assert_eq!(
        provider_base_url(Some("https://a"), Some("https://b")),
        "https://a"
    );
    assert_eq!(provider_base_url(None, Some("https://b")), "https://b");
    assert_eq!(provider_base_url(None, None), "(default)");
    assert!(provider_add_summary(true).contains("API key"));
    assert!(provider_add_summary(false).contains("no API key"));
    let headers = parse_provider_headers("A=1, B=2, skip, C=");
    assert_eq!(headers.get("A").map(String::as_str), Some("1"));
    assert_eq!(headers.get("B").map(String::as_str), Some("2"));
    assert!(!headers.contains_key("skip"));
    assert!(headers.contains_key("C"));
    assert!(agent_default_marker("build", "build").contains("default"));
    assert!(agent_default_marker("plan", "build").is_empty());
    assert!(provider_updating_line("acme").contains("already exists"));
    assert!(provider_saved_line("acme", "added with API key").contains("acme"));
    assert!(provider_removed_line("acme").contains("removed"));
    assert!(provider_not_found_line("acme").contains("not found"));
    assert!(provider_default_set_line("acme").contains("Default"));
    assert!(provider_default_use_line("acme").contains("-P acme"));
    assert!(provider_default_missing_line("acme").contains("provider add"));
    assert!(model_default_set_line("openai", "gpt").contains("openai"));
    assert!(agent_not_found_line("plan").contains("plan"));
}
