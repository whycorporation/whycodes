use std::process::Command;

fn whycode_binary() -> &'static str {
    env!("CARGO_BIN_EXE_whycode")
}

#[test]
fn test_cli_help() {
    let o = Command::new(whycode_binary()).arg("--help").output().unwrap();
    assert!(o.status.success());
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(s.contains("run") && s.contains("generate"), "help: {}", s);
}

#[test]
fn test_cli_help_short_flag() {
    let o = Command::new(whycode_binary()).arg("-h").output().unwrap();
    assert!(o.status.success());
    assert!(!String::from_utf8_lossy(&o.stdout).is_empty());
}

#[test]
fn test_cli_version() {
    let o = Command::new(whycode_binary()).arg("--version").output().unwrap();
    assert!(o.status.success());
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(s.contains("whycode"), "version: {}", s);
}

#[test]
fn test_config_subcommand() {
    let o = Command::new(whycode_binary()).arg("config").output().unwrap();
    assert!(o.status.success());
}

#[test]
fn test_debug_subcommand() {
    let o = Command::new(whycode_binary()).arg("debug").output().unwrap();
    assert!(o.status.success());
}

#[test]
fn test_stats_subcommand() {
    let o = Command::new(whycode_binary()).arg("stats").output().unwrap();
    assert!(o.status.success());
}

#[test]
fn test_provider_list() {
    let o = Command::new(whycode_binary()).args(["provider", "list"]).output().unwrap();
    assert!(o.status.success());
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(!s.is_empty(), "provider list output empty");
}

#[test]
fn test_session_list() {
    let o = Command::new(whycode_binary()).args(["session", "list"]).output().unwrap();
    assert!(o.status.success());
}

#[test]
fn test_invalid_subcommand() {
    let o = Command::new(whycode_binary()).arg("nonexistent_xyz").output().unwrap();
    assert!(!o.status.success());
}

#[test]
fn test_unknown_flag() {
    let o = Command::new(whycode_binary()).arg("--nonexistent-flag").output().unwrap();
    assert!(!o.status.success());
}
