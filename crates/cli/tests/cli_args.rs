use std::process::{Command, Output};

/// Run the whycodes binary with `args` and capture its output.
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_whycodes"))
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn `whycodes {}`: {}", args.join(" "), e))
}

/// Assert the command succeeded, reporting stdout and stderr when it did not.
/// Without this the only signal on CI is `assertion failed: o.status.success()`.
fn assert_ok(args: &[&str], o: &Output) {
    assert!(
        o.status.success(),
        "`whycodes {}` exited with {:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
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
    assert!(s.contains("import"), "help should list import: {s}");
    assert!(s.contains("connect"), "help should list connect: {s}");
    assert!(
        s.contains("completions"),
        "help should list completions: {s}"
    );
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
    // clap prints: "whycodes 0.4.0 (abc1234 2026-09-02)"
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
fn test_completions_bash_emits_script() {
    let o = run(&["completions", "bash"]);
    assert_ok(&["completions", "bash"], &o);
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(
        s.contains("whycodes") && (s.contains("_whycodes") || s.contains("complete")),
        "bash completions should mention whycodes: {s}"
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
    assert!(s.contains("whycodes"), "version: {}", s);
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
fn test_debug_json_subcommand() {
    let o = run(&["debug", "--json"]);
    assert_ok(&["debug", "--json"], &o);
    let s = String::from_utf8_lossy(&o.stdout);
    let v: serde_json::Value = serde_json::from_str(&s).expect("debug --json");
    assert!(v.get("version").is_some(), "{s}");
    assert!(v.get("gitHash").is_some(), "{s}");
    assert!(v.get("configPath").is_some(), "{s}");
    assert!(v.get("git_hash").is_none(), "{s}");
    let env = v["env"].as_array().expect("env");
    for entry in env {
        assert!(
            entry.get("value").is_none(),
            "must not leak env values: {s}"
        );
    }
}

#[test]
fn test_unique_subcommand_prefix() {
    let o = run(&["sess", "--help"]);
    assert_ok(&["sess", "--help"], &o);
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(
        s.contains("List all sessions") || s.contains("session"),
        "{s}"
    );
}

#[test]
fn test_ambiguous_subcommand_prefix_fails() {
    let o = run(&["s"]);
    assert!(!o.status.success());
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
    if s.contains("Built-in providers supported:") {
        assert!(
            s.contains("google-antigravity"),
            "empty provider list should name google-antigravity: {s}"
        );
    }
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
    let home = std::env::temp_dir().join(format!("whycodes-test-home-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_whycodes"))
        .args(["auth", "import"])
        .env("HOME", &home)
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .output()
        .expect("spawn whycodes auth import");
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

/// Isolated `WHYCODES_HOME` so config/session/memory commands never touch
/// the developer's real store.
fn run_home(home: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_whycodes"))
        .args(args)
        .env("WHYCODES_HOME", home)
        .env("HOME", home)
        .current_dir(home)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn `whycodes {}`: {e}", args.join(" ")))
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
    assert!(s.contains("whycodes serve"), "{s}");
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
    let o = Command::new(env!("CARGO_BIN_EXE_whycodes"))
        .args(["pr", "--title", "t", "--base", "main"])
        .env("WHYCODES_HOME", home.path())
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

    let gh = Command::new(env!("CARGO_BIN_EXE_whycodes"))
        .args(["github", "pr", "list"])
        .env("WHYCODES_HOME", home.path())
        .env("HOME", home.path())
        .env("PATH", empty_path)
        .current_dir(home.path())
        .output()
        .expect("github pr");
    assert_ok(&["github", "pr", "list"], &gh);

    let view = Command::new(env!("CARGO_BIN_EXE_whycodes"))
        .args(["github", "pr", "view", "1"])
        .env("WHYCODES_HOME", home.path())
        .env("HOME", home.path())
        .env("PATH", empty_path)
        .current_dir(home.path())
        .output()
        .expect("github view");
    assert_ok(&["github", "pr", "view"], &view);

    let issue = Command::new(env!("CARGO_BIN_EXE_whycodes"))
        .args(["github", "issue", "2"])
        .env("WHYCODES_HOME", home.path())
        .env("HOME", home.path())
        .env("PATH", empty_path)
        .current_dir(home.path())
        .output()
        .expect("github issue");
    assert_ok(&["github", "issue"], &issue);
}

#[test]
fn test_import_help_and_dry_run() {
    let o = run(&["import", "--help"]);
    assert_ok(&["import", "--help"], &o);
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(s.contains("--from"), "import help: {s}");
    assert!(s.contains("--dry-run"), "import help: {s}");

    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join(".claude.json"),
        r#"{"mcpServers":{"fs":{"command":"npx"}}}"#,
    )
    .unwrap();
    let dry = run_home(
        home.path(),
        &["import", "--yes", "--dry-run", "--from", "claude"],
    );
    assert_ok(&["import", "--yes", "--dry-run", "--from", "claude"], &dry);
    let out = String::from_utf8_lossy(&dry.stdout);
    assert!(
        out.contains("Dry run") || out.contains("MCP"),
        "import dry-run stdout: {out}"
    );
}

#[test]
fn test_session_import_share_rename_and_memory_files() {
    let home = tempfile::tempdir().unwrap();
    let transcript = home.path().join("chat.json");
    std::fs::write(
        &transcript,
        r#"{"messages":[{"role":"user","content":"imported hello"}]}"#,
    )
    .unwrap();
    let imp = run_home(
        home.path(),
        &["session", "import", transcript.to_str().unwrap()],
    );
    assert_ok(&["session", "import"], &imp);
    let out = String::from_utf8_lossy(&imp.stdout);
    assert!(out.contains("Imported"), "{out}");
    let id = out
        .lines()
        .find_map(|l| l.split("session ").nth(1))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .expect("session id")
        .to_string();

    let share = run_home(home.path(), &["session", "share", &id]);
    assert_ok(&["session", "share"], &share);
    assert!(
        String::from_utf8_lossy(&share.stdout).contains("exported")
            || home
                .path()
                .join("shares")
                .join(format!("{id}.json"))
                .exists()
    );
    let share_miss = run_home(home.path(), &["session", "share", "nope"]);
    assert_ok(&["session", "share", "nope"], &share_miss);

    let rename = run_home(home.path(), &["session", "rename", &id, "new title"]);
    assert_ok(&["session", "rename"], &rename);
    let rename_miss = run_home(home.path(), &["session", "rename", "nope", "x"]);
    assert_ok(&["session", "rename", "nope"], &rename_miss);

    let view = run_home(home.path(), &["session", "view", &id]);
    assert_ok(&["session", "view"], &view);

    let exp_path = home.path().join("mem.json");
    let _ = run_home(
        home.path(),
        &["memory", "add", "exportable fact about rustfmt"],
    );
    let exp = run_home(
        home.path(),
        &["memory", "export", "-o", exp_path.to_str().unwrap()],
    );
    assert_ok(&["memory", "export", "-o"], &exp);
    assert!(exp_path.exists());
    let imp_mem = run_home(
        home.path(),
        &["memory", "import", exp_path.to_str().unwrap()],
    );
    assert_ok(&["memory", "import"], &imp_mem);

    let code = run_home(home.path(), &["memory", "code-search", "fn main"]);
    assert_ok(&["memory", "code-search"], &code);
    let sess = run_home(home.path(), &["memory", "session-search", "hello"]);
    assert_ok(&["memory", "session-search"], &sess);

    let onnx = run_home(home.path(), &["memory", "onnx-smoke"]);
    if onnx.status.success() {
        assert!(
            String::from_utf8_lossy(&onnx.stdout).contains("embedding dim="),
            "successful ONNX smoke should report its embedding dimension"
        );
    } else {
        assert!(
            !onnx.stderr.is_empty(),
            "failed ONNX smoke should explain why it could not run"
        );
    }

    let add_stdio = run_home(
        home.path(),
        &["mcp", "add", "echoer", "echo", "--args", "hi"],
    );
    assert_ok(&["mcp", "add", "echoer"], &add_stdio);

    let def_miss = run_home(home.path(), &["provider", "default", "nope"]);
    assert_ok(&["provider", "default", "nope"], &def_miss);
    let add_p = run_home(
        home.path(),
        &["provider", "add", "local", "--base-url", "http://127.0.0.1"],
    );
    assert_ok(&["provider", "add"], &add_p);
    let def = run_home(home.path(), &["provider", "default", "local"]);
    assert_ok(&["provider", "default", "local"], &def);
}

#[test]
fn test_config_dispatch_roundtrips_supported_values_and_reports_errors() {
    let home = tempfile::tempdir().unwrap();

    for (key, value) in [("project_path", "/workspace/demo"), ("log_level", "trace")] {
        let set = run_home(home.path(), &["config", "set", key, value]);
        assert_ok(&["config", "set", key, value], &set);

        let get = run_home(home.path(), &["config", "get", key]);
        assert_ok(&["config", "get", key], &get);
        assert_eq!(String::from_utf8_lossy(&get.stdout).trim(), value);
    }

    let show = run_home(home.path(), &["config", "show"]);
    assert_ok(&["config", "show"], &show);
    let shown = String::from_utf8_lossy(&show.stdout);
    assert!(shown.contains("Config path:"), "{shown}");
    assert!(
        shown.contains("project_path = \"/workspace/demo\""),
        "{shown}"
    );
    assert!(shown.contains("log_level = \"trace\""), "{shown}");

    let rejected = run_home(home.path(), &["config", "set", "unknown", "value"]);
    assert!(!rejected.status.success());
    let error = format!(
        "{}{}",
        String::from_utf8_lossy(&rejected.stderr),
        String::from_utf8_lossy(&rejected.stdout)
    );
    assert!(error.contains("Unknown config key: unknown"), "{error}");
    assert!(
        error.contains("default_agent, project_path, log_level"),
        "{error}"
    );
}

#[test]
fn test_mcp_dispatch_persists_remote_headers_and_lists_stdio_details() {
    let home = tempfile::tempdir().unwrap();

    let remote = run_home(
        home.path(),
        &[
            "mcp",
            "add",
            "remote",
            "--url",
            "https://example.com/rpc",
            "--type",
            "streamable-http",
            "--header",
            "Authorization: Bearer test",
        ],
    );
    assert_ok(&["mcp", "add", "remote"], &remote);

    let local = run_home(
        home.path(),
        &["mcp", "add", "local", "node", "--args", "server.js --quiet"],
    );
    assert_ok(&["mcp", "add", "local"], &local);

    let listed = run_home(home.path(), &["mcp", "list"]);
    assert_ok(&["mcp", "list"], &listed);
    let output = String::from_utf8_lossy(&listed.stdout);
    assert!(output.contains("remote"), "{output}");
    assert!(output.contains("http https://example.com/rpc"), "{output}");
    assert!(output.contains("local"), "{output}");
    assert!(output.contains("node server.js --quiet"), "{output}");

    let config = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
    assert!(
        config.contains("Authorization = \"Bearer test\""),
        "{config}"
    );
}

fn run_plain(home: &std::path::Path, stdin: &str) -> Output {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new(env!("CARGO_BIN_EXE_whycodes"))
        .args(["--plain", "--no-memory"])
        .env("WHYCODES_HOME", home)
        .env("HOME", home)
        .env("WHYCODES_PLAIN", "1")
        .current_dir(home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn whycodes --plain");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("wait --plain")
}

#[test]
fn test_plain_repl_slash_commands_without_api_key() {
    let home = tempfile::tempdir().unwrap();
    let o = run_plain(
        home.path(),
        "\n\
         /help\n\
         /info\n\
         /rename\n\
         /rename testhome\n\
         /undo\n\
         /redo\n\
         /new\n\
         /share\n\
         /compact\n\
         /fresh\n\
         /diff\n\
         /cost\n\
         /context\n\
         /doctor\n\
         /sessions\n\
         /resume\n\
         /continue\n\
         /models\n\
         /models gpt-test\n\
         /agent\n\
         /agent nope\n\
         /agent plan\n\
         /connect\n\
         /login\n\
         /login nope\n\
         /thinking\n\
         /themes\n\
         /tools\n\
         /remember\n\
         /remember keep this\n\
         /memory\n\
         /effort\n\
         /effort high\n\
         /h\n\
         /details\n\
         /export\n\
         /summarize\n\
         /usage\n\
         /agents\n\
         /init\n\
         /clear\n\
         /quit\n\
         hello world this is a prompt without a key\n\
         !echo hi\n\
         /unknowncmd\n\
         /q\n",
    );
    assert_ok(&["--plain"], &o);
    let s = format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(s.contains("Slash commands") || s.contains("/help"), "{s}");
    assert!(
        s.contains("Interactive mode") || s.contains("WhyCodes"),
        "{s}"
    );
}

#[test]
fn test_generate_without_key_and_empty_prompt() {
    let home = tempfile::tempdir().unwrap();
    let missing = run_home(home.path(), &["generate", "hello", "--format", "json"]);
    assert!(!missing.status.success());
    let err = format!(
        "{}{}",
        String::from_utf8_lossy(&missing.stderr),
        String::from_utf8_lossy(&missing.stdout)
    );
    assert!(
        err.contains("API key") || err.contains("ANTHROPIC_API_KEY"),
        "{err}"
    );

    let empty = run_home(home.path(), &["generate", "", "--format", "text"]);
    assert!(!empty.status.success());
    let err = format!(
        "{}{}",
        String::from_utf8_lossy(&empty.stderr),
        String::from_utf8_lossy(&empty.stdout)
    );
    assert!(
        err.contains("empty prompt") || err.contains("API key"),
        "{err}"
    );

    let structured_run = run_home(home.path(), &["run", "hi", "--format", "stream-json"]);
    assert!(!structured_run.status.success());
}

#[test]
fn test_plain_repl_shell_bang_and_exit_aliases() {
    let home = tempfile::tempdir().unwrap();
    let o = run_plain(home.path(), "!\n!printf hi\n/quit\n");
    assert_ok(&["--plain bang"], &o);
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(s.contains("Usage: !") || s.contains("hi"), "{s}");
}

#[test]
fn test_plain_repl_ollama_prompt_without_server() {
    let home = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_whycodes"))
        .args(["--plain", "--no-memory", "-P", "ollama", "-m", "tiny"])
        .env("WHYCODES_HOME", home.path())
        .env("HOME", home.path())
        .env("WHYCODES_PLAIN", "1")
        .current_dir(home.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().expect("stdin");
        stdin.write_all(b"hello from ollama path\n/q\n").unwrap();
    }
    let o = child.wait_with_output().expect("wait");
    assert_ok(&["--plain ollama"], &o);
}
