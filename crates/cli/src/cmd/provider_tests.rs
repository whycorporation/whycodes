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
}
