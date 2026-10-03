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

fn parse_value(parser: impl clap::builder::TypedValueParser<Value = String>, raw: &str) -> String {
    let cmd = clap::Command::new("whycodes");
    let arg = clap::Arg::new("value");
    parser
        .parse_ref(&cmd, Some(&arg), std::ffi::OsStr::new(raw))
        .unwrap()
}

#[test]
fn parsers_accept_freeform_values() {
    assert_eq!(parse_value(ProviderValueParser, "openai"), "openai");
    assert_eq!(parse_value(ModelValueParser, "tiny"), "tiny");
    assert_eq!(
        parse_value(AuthProviderValueParser, "anthropic"),
        "anthropic"
    );
    assert_eq!(parse_value(SessionIdValueParser, "abc"), "abc");
}

const AUTH_PLUGIN: &str = r#"{
    "kind": "auth",
    "auth": {
        "provider": "cov-auth-demo",
        "label": "Coverage",
        "flow": "device-code",
        "client_id": "abc",
        "authorize_url": "https://example.com/device/code",
        "token_url": "https://example.com/token",
        "scopes": "read"
    }
}"#;

/// Custom providers, blank model ids, session prefixes, an unreadable config
/// file, and one auth plugin so completion does not fall back to builtins.
#[test]
fn completion_ids_follow_config_sessions_and_auth_plugins() {
    let _home = crate::cmd::helpers::IsolatedHome::new();
    let path = whycodes_config::Config::default_path().unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"
[providers.custom]
name = "custom"
models = ["", "custom-model"]

[providers.openai]
name = "openai"
models = ["skipped-builtin"]

[models.fast]
model_id = "fast-id"
provider_id = "custom"

[models.blank]
model_id = ""
provider_id = "custom"

[default_model]
model_id = ""
provider_id = "custom"
"#,
    )
    .unwrap();

    let providers = provider_ids();
    assert!(providers.iter().any(|n| n == "custom"), "{providers:?}");
    assert!(providers.iter().any(|n| n == "openai"), "{providers:?}");
    let models = model_ids();
    assert!(models.iter().any(|n| n == "fast"), "{models:?}");
    assert!(models.iter().any(|n| n == "fast-id"), "{models:?}");
    assert!(models.iter().any(|n| n == "custom-model"), "{models:?}");
    assert!(!models.iter().any(|n| n.is_empty()), "{models:?}");

    std::fs::write(
        &path,
        r#"
[default_model]
model_id = "seed-model"
provider_id = "custom"
"#,
    )
    .unwrap();
    let models = model_ids();
    assert!(models.iter().any(|n| n == "seed-model"), "{models:?}");

    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(load_config_readonly().providers.is_empty());
    std::fs::remove_dir(&path).unwrap();

    let data = whycodes_config::Config::data_dir().unwrap();
    let db_path = data.join("whycodes.db");
    let db = whycodes_storage::db::Database::open(&db_path.to_string_lossy()).unwrap();
    db.create_session("short", "s", ".").unwrap();
    db.create_session("0123456789abcdefEXTRA", "long", ".")
        .unwrap();
    drop(db);
    let ids = session_id_prefixes();
    assert!(ids.iter().any(|id| id == "short"), "{ids:?}");
    assert!(ids.iter().any(|id| id == "0123456789ab"), "{ids:?}");

    let plug = data.join("plugins").join("cov-auth");
    std::fs::create_dir_all(&plug).unwrap();
    std::fs::write(plug.join("plugin.json"), AUTH_PLUGIN).unwrap();
    let names = auth_provider_ids();
    assert!(names.iter().any(|n| n == "cov-auth-demo"), "{names:?}");
}
