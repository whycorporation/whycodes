use super::*;

#[test]
fn get_unknown_key_is_none() {
    let cfg = whycodes_config::Config::default();
    assert!(get_config_value(&cfg, "nope.nope").is_none());
    assert_eq!(
        get_config_value(&cfg, "default_agent").as_deref(),
        Some(cfg.default_agent.as_str())
    );
    assert!(get_config_value(&cfg, "project_path").is_none());
    assert!(get_config_value(&cfg, "log_level").is_none());
}

#[test]
fn set_config_value_covers_known_keys() {
    let mut cfg = whycodes_config::Config::default();
    set_config_value(&mut cfg, "default_agent", "plan").unwrap();
    assert_eq!(cfg.default_agent, "plan");
    set_config_value(&mut cfg, "project_path", "/tmp/proj").unwrap();
    assert_eq!(
        get_config_value(&cfg, "project_path").as_deref(),
        Some("/tmp/proj")
    );
    set_config_value(&mut cfg, "log_level", "debug").unwrap();
    assert_eq!(
        get_config_value(&cfg, "log_level").as_deref(),
        Some("debug")
    );
    assert!(set_config_value(&mut cfg, "nope", "x").is_err());
}

#[test]
fn config_printer_helpers() {
    assert!(config_path_line("/tmp/config.toml").contains("/tmp/config.toml"));
    assert!(config_key_missing_line("nope").contains("nope"));
    assert!(config_set_line("log_level", "info").contains("log_level"));
}
