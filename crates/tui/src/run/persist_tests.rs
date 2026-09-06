use super::*;
use whycodes_config::Config;
use whycodes_core::types::Usage;
use whycodes_session::session::Session;

#[test]
fn persist_session_best_effort_does_not_panic() {
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let prev = std::env::var_os("WHYCODES_HOME");
    unsafe { std::env::set_var("WHYCODES_HOME", home.path()) };
    reset_session_db_cache();
    let session = Session::new(home.path().to_path_buf(), "sys".into());
    persist_session_best_effort(&session, "test");
    reset_session_db_cache();
    unsafe {
        match prev {
            Some(v) => std::env::set_var("WHYCODES_HOME", v),
            None => std::env::remove_var("WHYCODES_HOME"),
        }
    }
}

#[test]
fn configured_models_includes_config_entries() {
    let mut cfg = Config::default();
    cfg.providers.insert(
        "local".into(),
        whycodes_core::types::ProviderConfig {
            name: "local".into(),
            api_key: None,
            api_base: None,
            base_url: None,
            headers: None,
            models: vec!["tiny-test".into()],
            tool_arguments: None,
            extra: Default::default(),
        },
    );
    let models = configured_models(&cfg);
    assert!(
        models.iter().any(|(p, m)| p == "local" && m == "tiny-test"),
        "{models:?}"
    );
}

#[test]
fn cost_report_empty_usage_is_estimated() {
    let session = Session::new("/tmp/p".into(), "sys".into());
    let app = TuiApp::new(TuiAppConfig::default());
    let out = cost_report(&session, &app);
    assert!(out.contains("Cost"), "{out}");
    assert!(
        out.contains("estimated") || out.contains("none yet"),
        "{out}"
    );
    let mut session = session;
    session.usage = Usage {
        input_tokens: 10,
        output_tokens: 4,
        cache_creation_input_tokens: Some(2),
        cache_read_input_tokens: Some(1),
    };
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.turn_usage = Some(Usage {
        input_tokens: 3,
        output_tokens: 1,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    });
    let out = cost_report(&session, &app);
    assert!(out.contains("last turn"), "{out}");
    assert!(out.contains("cache"), "{out}");
}
