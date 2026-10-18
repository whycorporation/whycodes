use super::*;

fn trees(texts: &[(&'static str, &str)]) -> Vec<(&'static str, Value)> {
    texts
        .iter()
        .map(|(layer, text)| (*layer, Value::Table(toml::from_str(text).unwrap())))
        .collect()
}

fn path(key: &str) -> Vec<String> {
    dotted::parse_key(key).unwrap()
}

#[test]
fn legacy_keys_map_to_dotted_paths() {
    assert_eq!(canonical_key("log_level"), "general.log_level");
    assert_eq!(canonical_key("project_path"), "general.project_path");
    assert_eq!(
        canonical_key("session.tool_profile"),
        "session.tool_profile"
    );
}

#[test]
fn config_printer_helpers() {
    assert!(config_path_line("/tmp/config.toml").contains("/tmp/config.toml"));
    assert!(config_key_missing_line("nope").contains("nope"));
    let line = config_set_line("log_level", "info", "/h/config.toml");
    assert!(line.contains("log_level") && line.contains("in /h/config.toml"));
    assert!(config_get_line("plan", "global").contains("(from: global)"));
}

#[test]
fn resolve_config_value_names_the_last_layer_that_changed_it() {
    let layers = trees(&[
        ("default", "default_agent = \"build\"\n[general]\n"),
        (
            "global",
            "default_agent = \"plan\"\n[providers.p]\napi_key = \"sk\"\n",
        ),
        (
            "project",
            "default_agent = \"plan\"\n[general]\nlog_level = \"debug\"\n[providers.p]\napi_key = \"sk\"\n",
        ),
        (
            "env",
            "default_agent = \"plan\"\n[general]\nlog_level = \"trace\"\n[providers.p]\napi_key = \"sk\"\n",
        ),
    ]);
    assert_eq!(
        resolve_config_value(&layers, &path("default_agent")),
        Some(("plan".into(), "global"))
    );
    assert_eq!(
        resolve_config_value(&layers, &path("general.log_level")),
        Some(("trace".into(), "env"))
    );
    assert_eq!(
        resolve_config_value(&layers, &path("providers.p.api_key")),
        Some((REDACTED.into(), "global"))
    );
    // A whole table is redacted inside too.
    let (table, _) = resolve_config_value(&layers, &path("providers.p")).unwrap();
    assert_eq!(table.get("api_key").and_then(Value::as_str), Some(REDACTED));
    assert_eq!(resolve_config_value(&layers, &path("nope")), None);
    assert_eq!(resolve_config_value(&[], &path("nope")), None);
}

#[test]
fn config_keys_lists_leaves_under_a_prefix() {
    let layers = trees(&[
        (
            "default",
            "[session]\ntool_profile = \"core\"\nauto_title = true\n",
        ),
        (
            "global",
            "[session]\ntool_profile = \"full\"\nauto_title = true\n[notify]\ndiscord_webhook = \"https://d\"\n",
        ),
    ]);
    let rows = config_keys(&layers, Some("session")).unwrap();
    assert_eq!(
        rows,
        [
            (
                "session.auto_title".to_string(),
                Value::Boolean(true),
                "default"
            ),
            ("session.tool_profile".to_string(), "full".into(), "global"),
        ]
    );
    let all = config_keys(&layers, None).unwrap();
    assert!(all.contains(&(
        "notify.discord_webhook".to_string(),
        REDACTED.into(),
        "global"
    )));
    assert!(config_keys(&layers, Some("nope")).unwrap().is_empty());
    assert!(config_keys(&layers, Some("a..b")).is_err());
}

#[test]
fn get_json_keeps_the_value_type() {
    let out = get_json(
        "memory.auto_inject",
        Some(&(Value::Boolean(false), "project")),
    )
    .unwrap();
    assert_eq!(
        out,
        serde_json::json!({ "key": "memory.auto_inject", "value": false, "overriddenBy": "project" })
    );
    let missing = get_json("nope", None).unwrap();
    assert_eq!(
        missing,
        serde_json::json!({ "key": "nope", "value": null, "overriddenBy": null })
    );
}
