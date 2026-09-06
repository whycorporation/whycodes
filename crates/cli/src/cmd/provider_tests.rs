#[test]
fn default_config_has_named_agents() {
    let cfg = whycodes_config::Config::default();
    assert!(cfg.get_agent("build").is_some());
    assert!(cfg.get_agent("plan").is_some());
}
