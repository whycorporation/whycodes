/// Integration tests for the whycode CLI binary.
///
/// These tests use `std::process::Command` to invoke the built binary directly,
/// verifying the CLI argument parsing and subcommand behavior.
use std::process::Command;

/// Path to the built whycode binary, resolved at test compile time.
fn whycode_binary() -> &'static str {
    env!("CARGO_BIN_EXE_whycode")
}

// ─── CLI help ──────────────────────────────────────────────────────────────

#[test]
fn test_cli_help() {
    let output = Command::new(whycode_binary())
        .arg("--help")
        .output()
        .expect("Failed to run whycode --help");

    assert!(output.status.success(), "whycode --help should exit 0");

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Should contain the binary name and common flags
    assert!(stdout.contains("whycode"), "help should contain binary name");
    assert!(
        stdout.contains("--prompt") || stdout.contains("--agent") || stdout.contains("--help"),
        "help should list common flags"
    );
    assert!(
        stdout.contains("Usage:") || stdout.contains("USAGE:"),
        "help should show usage"
    );
}

#[test]
fn test_cli_help_short_flag() {
    let output = Command::new(whycode_binary())
        .arg("-h")
        .output()
        .expect("Failed to run whycode -h");

    assert!(output.status.success(), "whycode -h should exit 0");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.is_empty(),
        "-h should produce output"
    );
}

// ─── CLI version ───────────────────────────────────────────────────────────

#[test]
fn test_cli_version() {
    let output = Command::new(whycode_binary())
        .arg("--version")
        .output()
        .expect("Failed to run whycode --version");

    assert!(output.status.success(), "whycode --version should exit 0");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.is_empty(),
        "version output should not be empty"
    );
    assert!(
        stdout.contains("whycode"),
        "version output should contain binary name: {stdout}"
    );
}

#[test]
fn test_cli_version_short_flag() {
    let output = Command::new(whycode_binary())
        .arg("-V")
        .output()
        .expect("Failed to run whycode -V");

    assert!(output.status.success(), "whycode -V should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.is_empty(), "-V output should not be empty");
}

// ─── List tools ────────────────────────────────────────────────────────────

#[test]
fn test_list_tools() {
    let output = Command::new(whycode_binary())
        .arg("tools")
        .output()
        .expect("Failed to run whycode tools");

    assert!(output.status.success(), "whycode tools should exit 0");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("read") || stdout.contains("Read a file") || stdout.contains("Available tools"),
        "tools output should list at least some tools. Got: {stdout}"
    );
    assert!(
        stdout.contains("write") || stdout.contains("Write"),
        "tools should list write tool"
    );
    assert!(
        stdout.contains("shell") || stdout.contains("Shell"),
        "tools should list shell tool"
    );
}

#[test]
fn test_list_tools_output_not_empty() {
    let output = Command::new(whycode_binary())
        .arg("tools")
        .output()
        .expect("Failed to run whycode tools");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should have real output
    assert!(!stdout.is_empty(), "stdout should not be empty. stderr: {stderr}");
    assert!(stderr.is_empty(), "stderr should be empty but got: {stderr}");
}

// ─── List models ───────────────────────────────────────────────────────────

#[test]
fn test_list_models() {
    let output = Command::new(whycode_binary())
        .arg("models")
        .output()
        .expect("Failed to run whycode models");

    assert!(output.status.success(), "whycode models should exit 0");

    let stdout = String::from_utf8_lossy(&output.stdout);
    // The models command lists configured models. It might show "No models configured"
    // or actual model entries. Either case is valid.
    assert!(
        !stdout.is_empty(),
        "models output should not be empty"
    );
    assert!(
        stdout.contains("Configured models")
            || stdout.contains("No models configured")
            || stdout.contains("/"),
        "models should show header or content. Got: {stdout}"
    );
}

// ─── Subcommand routing ────────────────────────────────────────────────────

#[test]
fn test_config_subcommand() {
    let output = Command::new(whycode_binary())
        .arg("config")
        .output()
        .expect("Failed to run whycode config");

    assert!(output.status.success(), "whycode config should exit 0");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Config path") || stdout.contains("[") || !stdout.is_empty(),
        "config should produce output"
    );
}

// ─── Error handling ────────────────────────────────────────────────────────

#[test]
fn test_invalid_subcommand_shows_error() {
    let output = Command::new(whycode_binary())
        .arg("nonexistent_command_xyz")
        .output()
        .expect("Failed to run whycode with invalid subcommand");

    // Invalid subcommands should fail
    assert!(
        !output.status.success(),
        "invalid subcommand should exit non-zero"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.is_empty(),
        "invalid subcommand should print error to stderr"
    );
}

#[test]
fn test_unknown_flag_shows_error() {
    let output = Command::new(whycode_binary())
        .arg("--nonexistent-flag")
        .output()
        .expect("Failed to run whycode with unknown flag");

    // Unknown flags should fail
    assert!(
        !output.status.success(),
        "unknown flag should exit non-zero"
    );
}
