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

#[test]
fn resolve_config_value_names_the_last_layer_that_changed_it() {
    let defaults = whycodes_config::Config::default();
    let mut global = defaults.clone();
    global.default_agent = "plan".into();
    let mut project = global.clone();
    project.general.log_level = Some("debug".into());
    let mut env = project.clone();
    env.general.log_level = Some("trace".into());
    let layers = [
        ("default", defaults.clone()),
        ("global", global),
        ("project", project),
        ("env", env),
    ];
    assert_eq!(
        resolve_config_value(&layers, "default_agent"),
        Some(("plan".into(), "global"))
    );
    assert_eq!(
        resolve_config_value(&layers, "log_level"),
        Some(("trace".into(), "env"))
    );
    assert_eq!(resolve_config_value(&layers, "project_path"), None);
    assert_eq!(resolve_config_value(&layers, "nope"), None);
    // Nothing changed it: the built-in default wins.
    let untouched = [("default", defaults.clone()), ("global", defaults)];
    assert_eq!(
        resolve_config_value(&untouched, "default_agent"),
        Some((whycodes_config::Config::default().default_agent, "default"))
    );
    assert!(config_get_line("plan", "global").contains("(from: global)"));
}
