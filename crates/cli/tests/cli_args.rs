use std::process::{Command, Output};

/// Run the whycode binary with `args` and capture its output.
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_whycode"))
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn `whycode {}`: {}", args.join(" "), e))
}

/// Assert the command succeeded, reporting stdout and stderr when it did not.
/// Without this the only signal on CI is `assertion failed: o.status.success()`.
fn assert_ok(args: &[&str], o: &Output) {
    assert!(
        o.status.success(),
        "`whycode {}` exited with {:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        args.join(" "),
        o.status.code(),
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr),
    );
}

#[test]
fn test_cli_help() {
    let o = run(&["--help"]);
    assert_ok(&["--help"], &o);
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(s.contains("run") && s.contains("generate"), "help: {}", s);
}

#[test]
fn test_cli_help_short_flag() {
    let o = run(&["-h"]);
    assert_ok(&["-h"], &o);
    assert!(!String::from_utf8_lossy(&o.stdout).is_empty());
}

#[test]
fn test_cli_version() {
    let o = run(&["--version"]);
    assert_ok(&["--version"], &o);
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(s.contains("whycode"), "version: {}", s);
}

#[test]
fn test_config_subcommand() {
    let o = run(&["config", "show"]);
    assert_ok(&["config", "show"], &o);
}

#[test]
fn test_debug_subcommand() {
    let o = run(&["debug"]);
    assert_ok(&["debug"], &o);
}

#[test]
fn test_stats_subcommand() {
    let o = run(&["stats"]);
    assert_ok(&["stats"], &o);
}

#[test]
fn test_provider_list() {
    let o = run(&["provider", "list"]);
    assert_ok(&["provider", "list"], &o);
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(!s.is_empty(), "provider list output empty");
}

#[test]
fn test_session_list() {
    let o = run(&["session", "list"]);
    assert_ok(&["session", "list"], &o);
}

#[test]
fn test_invalid_subcommand() {
    let o = run(&["nonexistent_xyz"]);
    assert!(!o.status.success());
}

#[test]
fn test_unknown_flag() {
    let o = run(&["--nonexistent-flag"]);
    assert!(!o.status.success());
}
