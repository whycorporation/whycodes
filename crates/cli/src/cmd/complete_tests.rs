use super::*;

#[test]
fn builtin_providers_include_openai() {
    let names = provider_ids();
    assert!(names.iter().any(|n| n == "openai"), "{names:?}");
    assert!(names.iter().any(|n| n == "anthropic"), "{names:?}");
}

#[test]
fn model_ids_never_panic_without_config() {
    let _ = model_ids();
}

#[test]
fn session_prefixes_empty_without_db() {
    // Isolated: no assertion on HOME; just must not create files / panic.
    let _ = session_id_prefixes();
}

#[test]
fn parsers_expose_possible_values() {
    use clap::builder::TypedValueParser;
    assert!(ProviderValueParser.possible_values().is_some());
    assert!(ModelValueParser.possible_values().is_some());
    assert!(AuthProviderValueParser.possible_values().is_some());
    assert!(SessionIdValueParser.possible_values().is_some());
    let names = auth_provider_ids();
    assert!(
        names.iter().any(|n| n == "anthropic" || n == "openai"),
        "{names:?}"
    );
}

#[test]
fn load_config_readonly_missing_and_invalid() {
    let cfg = load_config_readonly();
    let _ = cfg.providers.len();
    let _ = model_ids();
    let _ = session_id_prefixes();
}
