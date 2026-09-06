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

#[test]
fn debug_printer_helpers() {
    assert!(debug_header_line().contains("Debug"));
    assert!(debug_exists_mark(true).contains("✓") || debug_exists_mark(true).contains("green"));
    assert!(debug_exists_mark(false).contains("not found"));
    assert!(!debug_dir_exists_mark(false).contains("not found"));
    assert!(debug_config_line("/tmp/c.toml", true).contains("/tmp/c.toml"));
    assert!(debug_data_dir_line("/tmp/data", false).contains("/tmp/data"));
    assert!(debug_jsonl_line("/tmp/log.jsonl", true).contains("JSONL"));
    assert!(debug_jsonl_line("/tmp/log.jsonl", false).contains("/tmp/log.jsonl"));
    assert!(debug_crash_dir_line("/tmp/crash").contains("/tmp/crash"));
    assert!(debug_log_line("/tmp/latest.log").contains("WHYCODES_LOG_FILE"));
    assert!(debug_path_error_line("Config", "boom").contains("boom"));
    assert!(debug_cwd_line("/tmp").contains("/tmp"));
    assert!(debug_home_line("/home/x").contains("/home/x"));
    assert!(debug_tool_line("Rust", "rustc 1").contains("rustc 1"));
    assert!(debug_env_set_line("FOO", "sk-xxxx").contains("set"));
    assert!(debug_env_unset_line("FOO").contains("not set"));
    assert!(debug_oauth_empty_line().contains("auth login"));
    assert!(debug_oauth_entry_line("acme", "oauth", "no expiry").contains("acme"));
    assert!(debug_oauth_store_error_line("denied").contains("denied"));
    assert!(debug_oauth_data_dir_error_line("missing").contains("missing"));
    assert!(after_tui_upgrade_skip_line().contains("latest"));
    assert!(after_tui_upgrade_ok_line("1.0", "1.1").contains("1.1"));
    assert!(after_tui_upgrade_failed_line("offline").contains("offline"));
}
