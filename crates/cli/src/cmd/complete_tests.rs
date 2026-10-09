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
    // Auth plugins register process-wide. A parallel test may already have
    // loaded `cov-auth-demo`, and a non-empty registry skips the builtin
    // fallback. Drop it so this assertion sees `anthropic` / `openai`.
    let _guard = crate::cmd::helpers::lock_env();
    whycodes_auth::clear_registry();
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
    assert!(
        load_config_at(Err(whycodes_core::Error::Config("no config path".into())))
            .providers
            .is_empty()
    );
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
    let path = whycodes_config::Config::default_path();
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

    // A file named `whycodes.db` that is not a database makes the read-only
    // open fail. Drop the WAL sidecars first or SQLite recovers the sessions
    // written above and the error arm never runs.
    drop(std::fs::remove_file(format!("{}-wal", db_path.display())));
    drop(std::fs::remove_file(format!("{}-shm", db_path.display())));
    std::fs::write(&db_path, b"not a sqlite database").unwrap();
    assert!(session_id_prefixes().is_empty());
}

/// Linux lets a process delete its own cwd; `current_dir` then fails and
/// completion falls back to `.`. Windows refuses to remove the cwd, so
/// this arm is host-only (the coverage job is Linux).
#[cfg(unix)]
#[test]
fn auth_completion_uses_dot_when_cwd_is_gone() {
    let _home = crate::cmd::helpers::IsolatedHome::new();
    let prev = std::env::current_dir().expect("cwd");
    let dir = tempfile::tempdir().expect("tempdir");
    let gone = dir.path().join("missing-cwd");
    std::fs::create_dir(&gone).unwrap();
    std::env::set_current_dir(&gone).unwrap();
    std::fs::remove_dir(&gone).unwrap();
    whycodes_auth::clear_registry();
    let names = auth_provider_ids();
    if let Err(err) = std::env::set_current_dir(&prev) {
        panic!("restore cwd: {err}");
    }
    assert!(
        names.iter().any(|n| n == "openai" || n == "anthropic"),
        "{names:?}"
    );
}

/// A config that exists but is not TOML takes the parse fallback. Two auth
/// plugins with the same provider id take `dedup`. A database file whose
/// session table is gone makes `list_sessions` fail without creating a new db.
#[test]
fn completion_covers_bad_toml_duplicate_auth_and_broken_sessions() {
    let _home = crate::cmd::helpers::IsolatedHome::new();
    let path = whycodes_config::Config::default_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "this is not toml {{{").unwrap();
    assert!(load_config_readonly().providers.is_empty());
    std::fs::remove_file(&path).unwrap();

    let data = whycodes_config::Config::data_dir().unwrap();
    let plug = data.join("plugins").join("dup-auth");
    std::fs::create_dir_all(&plug).unwrap();
    std::fs::write(plug.join("plugin.json"), AUTH_PLUGIN).unwrap();
    let cwd = std::env::current_dir().unwrap();
    let project = whycodes_plugin::project_plugins_dir(&cwd).join("dup-auth");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("plugin.json"), AUTH_PLUGIN).unwrap();
    whycodes_auth::clear_registry();
    let names = auth_provider_ids();
    let hits = names.iter().filter(|n| *n == "cov-auth-demo").count();
    assert_eq!(hits, 1, "{names:?}");
    let _ = std::fs::remove_dir_all(whycodes_plugin::project_plugins_dir(&cwd));

    let db_path = data.join("whycodes.db");
    let db = whycodes_storage::db::Database::open(&db_path.to_string_lossy()).unwrap();
    db.create_session("kept", "s", ".").unwrap();
    db.drop_sessions_table_for_test().unwrap();
    drop(db);
    assert!(session_id_prefixes().is_empty());
    assert!(
        session_prefixes_in(Err(whycodes_core::Error::Config("no data dir".into()))).is_empty()
    );
}

/// A config file with mode `000` cannot be read, so completion falls back to
/// the default config. An empty sessions table is a successful query that
/// still yields no prefixes. Linux-only: Windows ignores Unix mode bits.
#[cfg(unix)]
#[test]
fn completion_unreadable_config_and_empty_sessions() {
    let _home = crate::cmd::helpers::IsolatedHome::new();
    let path = whycodes_config::Config::default_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"
[providers.hidden]
name = "hidden"
"#,
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    let cfg = load_config_readonly();
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644));
    assert!(
        !cfg.providers.contains_key("hidden"),
        "unreadable config must not load providers"
    );

    let data = whycodes_config::Config::data_dir().unwrap();
    let db_path = data.join("whycodes.db");
    let db = whycodes_storage::db::Database::open(&db_path.to_string_lossy()).unwrap();
    drop(db);
    assert!(session_id_prefixes().is_empty());
}
