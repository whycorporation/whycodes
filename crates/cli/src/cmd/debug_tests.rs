use super::*;
use crate::args::Cli;
use clap::Parser;

#[test]
fn auto_update_off_when_cli_flag() {
    let cli = Cli::try_parse_from(["whycodes", "--no-auto-update"]).unwrap();
    assert!(!should_auto_update_with_env(
        &cli, true, false, false, false
    ));
}

#[test]
fn debug_dump_keys_are_camel_case() {
    let dump = collect_debug();
    let v = serde_json::to_value(&dump).unwrap();
    for key in [
        "version",
        "gitHash",
        "configPath",
        "configExists",
        "dataDir",
        "jsonlLog",
        "crashDir",
        "debugLog",
        "cwd",
        "env",
        "oauth",
    ] {
        assert!(v.get(key).is_some(), "missing {key} in {v}");
    }
    assert!(v.get("git_hash").is_none());
    let env = v["env"].as_array().expect("env array");
    assert!(!env.is_empty());
    for entry in env {
        assert!(entry.get("name").is_some());
        assert!(entry.get("set").and_then(|s| s.as_bool()).is_some());
        assert!(entry.get("value").is_none(), "env must not leak values");
    }
}

#[test]
fn auto_update_on_for_interactive_run_and_off_otherwise() {
    let run = Cli::try_parse_from(["whycodes", "run"]).unwrap();
    assert!(should_auto_update_with_env(&run, true, false, false, false));
    let json = Cli::try_parse_from(["whycodes", "run", "--format", "json", "hi"]).unwrap();
    assert!(!should_auto_update_with_env(
        &json, true, false, false, false
    ));
    let stats = Cli::try_parse_from(["whycodes", "stats"]).unwrap();
    assert!(!should_auto_update_with_env(
        &stats, true, false, false, false
    ));
    assert!(!should_auto_update_with_env(
        &run, false, false, false, false
    ));
    assert!(!should_auto_update_with_env(&run, true, true, false, false));
    assert!(!should_auto_update_with_env(&run, true, false, true, false));
    assert!(!should_auto_update_with_env(&run, true, false, false, true));
    let dump = collect_debug();
    let _ = cmd_version("rustc");
    let _ = cmd_version("definitely-missing-bin");
    assert!(!dump.version.is_empty());
}
