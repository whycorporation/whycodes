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
fn test_generate_help_documents_format() {
    let o = run(&["generate", "--help"]);
    assert_ok(&["generate", "--help"], &o);
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(
        s.contains("--format") || s.contains("output-format"),
        "generate help should document --format: {s}"
    );
    assert!(
        s.contains("stream-json") || s.contains("json"),
        "generate help should mention json formats: {s}"
    );
}

#[test]
fn test_run_help_documents_format() {
    let o = run(&["run", "--help"]);
    assert_ok(&["run", "--help"], &o);
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(
        s.contains("--format") || s.contains("output-format"),
        "run help should document --format: {s}"
    );
}

#[test]
fn test_format_requires_prompt_on_run() {
    // Structured format without a prompt must fail fast (no API call).
    let o = run(&["run", "--format", "json"]);
    assert!(
        !o.status.success(),
        "run --format json without prompt should fail"
    );
    let err = format!(
        "{}{}",
        String::from_utf8_lossy(&o.stderr),
        String::from_utf8_lossy(&o.stdout)
    );
    assert!(
        err.contains("prompt") || err.contains("format"),
        "error should mention prompt/format requirement: {err}"
    );
}

#[test]
fn test_invalid_format_value() {
    let o = run(&["generate", "hi", "--format", "not-a-format"]);
    assert!(!o.status.success());
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
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(
        s.contains("JSONL log") || s.contains("Data dir"),
        "debug should report log paths: {s}"
    );
}

#[test]
fn test_global_debug_flag_in_help() {
    let o = run(&["--help"]);
    assert_ok(&["--help"], &o);
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(
        s.contains("--debug") || s.contains("debug logs"),
        "help should document --debug: {s}"
    );
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
