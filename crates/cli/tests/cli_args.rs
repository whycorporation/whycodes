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
    assert!(s.contains("connect"), "help should list connect: {s}");
    assert!(
        s.contains("--continue") || s.contains("-c"),
        "help should document --continue: {s}"
    );
    assert!(
        s.contains("--resume") || s.contains("-r"),
        "help should document --resume: {s}"
    );
}

#[test]
fn test_version_includes_semver_and_build_meta() {
    let o = run(&["--version"]);
    assert_ok(&["--version"], &o);
    let s = String::from_utf8_lossy(&o.stdout);
    // clap prints: "whycode 0.1.0 (abc1234 2026-08-04)"
    assert!(
        s.contains(env!("CARGO_PKG_VERSION")),
        "version should include crate semver: {s}"
    );
    assert!(
        s.contains('(') && s.contains(')'),
        "version should include (git-hash build-date): {s}"
    );
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

#[test]
fn test_auth_help_lists_subcommands() {
    let o = run(&["auth", "--help"]);
    assert_ok(&["auth", "--help"], &o);
    let s = String::from_utf8_lossy(&o.stdout);
    for sub in ["login", "logout", "status", "import"] {
        assert!(s.contains(sub), "auth help should list `{sub}`: {s}");
    }
}

#[test]
fn test_auth_import_runs_offline() {
    // Isolate HOME so the scan finds nothing and never touches the real
    // consent store — the command must exit cleanly without prompting.
    let home = std::env::temp_dir().join(format!("whycode-test-home-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_whycode"))
        .args(["auth", "import"])
        .env("HOME", &home)
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .output()
        .expect("spawn whycode auth import");
    assert_ok(&["auth", "import"], &o);
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(
        s.contains("No credentials"),
        "empty HOME should report no findings: {s}"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn test_auth_status_runs_offline() {
    // Works with or without an existing store; never prints token material.
    let o = run(&["auth", "status"]);
    assert_ok(&["auth", "status"], &o);
}

#[test]
fn test_auth_login_rejects_unknown_provider_without_network() {
    // Must fail fast, before any browser/listener/token-endpoint step.
    let o = run(&["auth", "login", "definitely-not-a-provider"]);
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("does not support OAuth login"),
        "stderr should name the supported set: {err}"
    );
}

/// Isolated `WHYCODE_HOME` so config/session/memory commands never touch
/// the developer's real store.
fn run_home(home: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_whycode"))
        .args(args)
        .env("WHYCODE_HOME", home)
        .env("HOME", home)
        .current_dir(home)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn `whycode {}`: {e}", args.join(" ")))
}

#[test]
fn test_acp_and_web_are_offline_stubs() {
    let home = tempfile::tempdir().unwrap();
    let acp = run_home(home.path(), &["acp"]);
    assert_ok(&["acp"], &acp);
    let s = String::from_utf8_lossy(&acp.stdout);
    assert!(s.contains("not yet implemented"), "{s}");

    let web = run_home(home.path(), &["web"]);
    assert_ok(&["web"], &web);
    let s = String::from_utf8_lossy(&web.stdout);
    assert!(s.contains("whycode serve"), "{s}");
}

#[test]
fn test_config_get_path_and_set() {
    let home = tempfile::tempdir().unwrap();
    let path = run_home(home.path(), &["config", "path"]);
    assert_ok(&["config", "path"], &path);
    let p = String::from_utf8_lossy(&path.stdout);
    assert!(p.contains("config.toml"), "{p}");

    let get = run_home(home.path(), &["config", "get", "default_agent"]);
    assert_ok(&["config", "get", "default_agent"], &get);
    assert!(
        String::from_utf8_lossy(&get.stdout).contains("build"),
        "{}",
        String::from_utf8_lossy(&get.stdout)
    );

    let missing = run_home(home.path(), &["config", "get", "nope"]);
    assert_ok(&["config", "get", "nope"], &missing);
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("not found")
            || String::from_utf8_lossy(&missing.stdout).contains("not found")
    );

    let set = run_home(home.path(), &["config", "set", "default_agent", "plan"]);
    assert_ok(&["config", "set", "default_agent", "plan"], &set);
    let get2 = run_home(home.path(), &["config", "get", "default_agent"]);
    assert!(
        String::from_utf8_lossy(&get2.stdout).contains("plan"),
        "{}",
        String::from_utf8_lossy(&get2.stdout)
    );
}

#[test]
fn test_mcp_add_list_remove_and_validation() {
    let home = tempfile::tempdir().unwrap();
    let empty = run_home(home.path(), &["mcp", "list"]);
    assert_ok(&["mcp", "list"], &empty);
    assert!(
        String::from_utf8_lossy(&empty.stdout).contains("No MCP servers"),
        "{}",
        String::from_utf8_lossy(&empty.stdout)
    );

    let add = run_home(
        home.path(),
        &["mcp", "add", "demo", "--url", "https://example.com/mcp"],
    );
    assert_ok(&["mcp", "add", "demo"], &add);

    let listed = run_home(home.path(), &["mcp", "list"]);
    let s = String::from_utf8_lossy(&listed.stdout);
    assert!(s.contains("demo"), "{s}");
    assert!(s.contains("example.com"), "{s}");

    let both = run_home(
        home.path(),
        &[
            "mcp",
            "add",
            "bad",
            "npx",
            "--url",
            "https://example.com/mcp",
        ],
    );
    assert!(!both.status.success());

    let bad_type = run_home(
        home.path(),
        &[
            "mcp",
            "add",
            "badtype",
            "--url",
            "https://example.com/mcp",
            "--type",
            "ftp",
        ],
    );
    assert!(!bad_type.status.success());

    let bad_header = run_home(
        home.path(),
        &[
            "mcp",
            "add",
            "hdr",
            "--url",
            "https://example.com/mcp",
            "--header",
            "NotAHeader",
        ],
    );
    assert!(!bad_header.status.success());

    let neither = run_home(home.path(), &["mcp", "add", "empty"]);
    assert!(!neither.status.success());

    let rm = run_home(home.path(), &["mcp", "remove", "demo"]);
    assert_ok(&["mcp", "remove", "demo"], &rm);
    let missing = run_home(home.path(), &["mcp", "remove", "demo"]);
    assert_ok(&["mcp", "remove", "demo"], &missing);
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("not found")
            || String::from_utf8_lossy(&missing.stdout).contains("not found")
    );
}

#[test]
fn test_model_provider_agent_plugins_offline() {
    let home = tempfile::tempdir().unwrap();

    let models = run_home(home.path(), &["model", "list"]);
    assert_ok(&["model", "list"], &models);

    let def = run_home(home.path(), &["model", "default", "openai", "gpt-4o"]);
    assert_ok(&["model", "default"], &def);

    let providers = run_home(home.path(), &["provider", "list"]);
    assert_ok(&["provider", "list"], &providers);

    let add = run_home(
        home.path(),
        &[
            "provider",
            "add",
            "local",
            "--api-key",
            "sk-test",
            "--base-url",
            "http://127.0.0.1:9/v1",
        ],
    );
    assert_ok(&["provider", "add", "local"], &add);
    let listed = run_home(home.path(), &["provider", "list"]);
    assert!(
        String::from_utf8_lossy(&listed.stdout).contains("local"),
        "{}",
        String::from_utf8_lossy(&listed.stdout)
    );
    let rm = run_home(home.path(), &["provider", "remove", "local"]);
    assert_ok(&["provider", "remove", "local"], &rm);
    let rm_missing = run_home(home.path(), &["provider", "remove", "local"]);
    assert_ok(&["provider", "remove", "local"], &rm_missing);

    let agents = run_home(home.path(), &["agent"]);
    assert_ok(&["agent"], &agents);
    let one = run_home(home.path(), &["agent", "build"]);
    assert_ok(&["agent", "build"], &one);
    let missing = run_home(home.path(), &["agent", "no-such-agent"]);
    assert_ok(&["agent", "no-such-agent"], &missing);
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("not found")
            || String::from_utf8_lossy(&missing.stdout).contains("Available")
    );

    let plugins = run_home(home.path(), &["plugins"]);
    assert_ok(&["plugins"], &plugins);
    assert!(
        String::from_utf8_lossy(&plugins.stdout).contains("plugin"),
        "{}",
        String::from_utf8_lossy(&plugins.stdout)
    );
}

#[test]
fn test_session_view_delete_missing_and_memory_roundtrip() {
    let home = tempfile::tempdir().unwrap();

    let view = run_home(home.path(), &["session", "view", "nope"]);
    assert_ok(&["session", "view", "nope"], &view);
    assert!(
        String::from_utf8_lossy(&view.stderr).contains("not found")
            || String::from_utf8_lossy(&view.stdout).contains("not found")
    );
    let del = run_home(home.path(), &["session", "delete", "nope"]);
    assert_ok(&["session", "delete", "nope"], &del);

    let path = run_home(home.path(), &["memory", "path"]);
    assert_ok(&["memory", "path"], &path);

    let list = run_home(home.path(), &["memory", "list"]);
    assert_ok(&["memory", "list"], &list);
    assert!(
        String::from_utf8_lossy(&list.stdout).contains("No memories"),
        "{}",
        String::from_utf8_lossy(&list.stdout)
    );

    let add = run_home(
        home.path(),
        &["memory", "add", "always run cargo test after edits"],
    );
    assert_ok(&["memory", "add"], &add);

    let search = run_home(home.path(), &["memory", "search", "cargo test"]);
    assert_ok(&["memory", "search"], &search);

    let export = run_home(home.path(), &["memory", "export"]);
    assert_ok(&["memory", "export"], &export);
    assert!(
        String::from_utf8_lossy(&export.stdout).contains("always run cargo"),
        "{}",
        String::from_utf8_lossy(&export.stdout)
    );

    let del_mem = run_home(home.path(), &["memory", "delete", "zzzzzzzz"]);
    assert_ok(&["memory", "delete"], &del_mem);

    let clear = run_home(home.path(), &["memory", "clear"]);
    assert_ok(&["memory", "clear"], &clear);
}

#[test]
fn test_pr_and_github_degrade_without_gh() {
    let home = tempfile::tempdir().unwrap();
    // PATH without `gh` so we hit the fallback print, not a real GitHub call.
    // Empty PATH so we never invoke a real `gh`.
    let empty_path = home.path();
    let o = Command::new(env!("CARGO_BIN_EXE_whycode"))
        .args(["pr", "--title", "t", "--base", "main"])
        .env("WHYCODE_HOME", home.path())
        .env("HOME", home.path())
        .env("PATH", empty_path)
        .current_dir(home.path())
        .output()
        .expect("pr");
    assert_ok(&["pr"], &o);
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(
        s.contains("pull request") || s.contains("GitHub CLI") || s.contains("gh pr"),
        "{s}"
    );

    let gh = Command::new(env!("CARGO_BIN_EXE_whycode"))
        .args(["github", "pr", "list"])
        .env("WHYCODE_HOME", home.path())
        .env("HOME", home.path())
        .env("PATH", empty_path)
        .current_dir(home.path())
        .output()
        .expect("github pr");
    assert_ok(&["github", "pr", "list"], &gh);

    let view = Command::new(env!("CARGO_BIN_EXE_whycode"))
        .args(["github", "pr", "view", "1"])
        .env("WHYCODE_HOME", home.path())
        .env("HOME", home.path())
        .env("PATH", empty_path)
        .current_dir(home.path())
        .output()
        .expect("github view");
    assert_ok(&["github", "pr", "view"], &view);
}
