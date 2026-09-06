use super::*;

#[test]
fn get_unknown_key_is_none() {
    let cfg = whycodes_config::Config::default();
    assert!(get_config_value(&cfg, "nope.nope").is_none());
}
