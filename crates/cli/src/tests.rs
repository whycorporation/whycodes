//! Unit tests. Sibling file so llvm-cov can ignore tests.rs.

use super::*;
use crate::cmd::auth::auth_expiry_label;
use crate::cmd::config::{get_config_value, set_config_value};
use crate::cmd::debug::should_auto_update_with_env;
use whycodes_agent::agent::Agent;
use whycodes_core::types::{ModelConfig, ProviderConfig};
use whycodes_protocol::{CiEvent, ResultMeta};

/// Serializes tests that mutate process-global env (`WHYCODES_HOME`, …).
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Clear test-only env vars and REPL queue even if a test panics.
struct TestLlmEnv;

impl Drop for TestLlmEnv {
    fn drop(&mut self) {
        unsafe {
            std::env::remove_var("WHYCODES_TEST_LLM");
            std::env::remove_var("WHYCODES_TEST_SKIP_UPGRADE");
            std::env::remove_var("ANTHROPIC_API_KEY");
        }
        clear_test_repl_lines();
    }
}

/// Point `WHYCODES_HOME` at a temp dir until dropped. Safe inside `#[tokio::test]`.
struct IsolatedHome {
    _guard: std::sync::MutexGuard<'static, ()>,
    dir: tempfile::TempDir,
    prev: Option<std::ffi::OsString>,
    prev_home: Option<std::ffi::OsString>,
}

impl IsolatedHome {
    fn new() -> Self {
        let guard = lock_env();
        let dir = tempfile::tempdir().expect("tempdir");
        let prev = std::env::var_os("WHYCODES_HOME");
        let prev_home = std::env::var_os("HOME");
        unsafe { std::env::set_var("WHYCODES_HOME", dir.path()) };
        unsafe { std::env::set_var("HOME", dir.path()) };
        Self {
            _guard: guard,
            dir,
            prev,
            prev_home,
        }
    }

    fn path(&self) -> &std::path::Path {
        self.dir.path()
    }
}

impl Drop for IsolatedHome {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => unsafe { std::env::set_var("WHYCODES_HOME", v) },
            None => unsafe { std::env::remove_var("WHYCODES_HOME") },
        }
        match &self.prev_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }
}

/// Prepend a stub `gh` that prints its argv and exits `exit`. Restores `PATH` on drop.
/// Does not take `ENV_LOCK` — compose with [`IsolatedHome`] which already serializes env.
struct IsolatedGh {
    _bin: tempfile::TempDir,
    prev_path: Option<std::ffi::OsString>,
}

impl IsolatedGh {
    fn new(exit: i32) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let gh = dir.path().join("gh");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(
                &gh,
                format!("#!/bin/sh\necho fake-gh \"$@\"\nexit {exit}\n"),
            )
            .unwrap();
            std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let prev_path = std::env::var_os("PATH");
        let mut path = dir.path().display().to_string();
        if let Some(rest) = &prev_path {
            path.push(':');
            path.push_str(&rest.to_string_lossy());
        }
        unsafe { std::env::set_var("PATH", &path) };
        Self {
            _bin: dir,
            prev_path,
        }
    }
}

impl Drop for IsolatedGh {
    fn drop(&mut self) {
        match &self.prev_path {
            Some(v) => unsafe { std::env::set_var("PATH", v) },
            None => unsafe { std::env::remove_var("PATH") },
        }
    }
}

fn cli(command: Option<Commands>) -> Cli {
    Cli {
        command,
        provider: None,
        model: None,
        agent_flag: None,
        dir: None,
        plain: false,
        continue_session: false,
        resume: None,
        debug: false,
        no_auto_update: false,
        no_memory: false,
    }
}

#[test]
fn splits_slash_commands() {
    assert_eq!(split_slash_command("/exit"), ("/exit", ""));
    assert_eq!(
        split_slash_command("/rename new name"),
        ("/rename", "new name")
    );
    assert_eq!(split_slash_command("  /help   arg  "), ("/help", "arg"));
    assert_eq!(split_slash_command("  /q"), ("/q", ""));
    assert_eq!(split_slash_command(""), ("", ""));
}

#[test]
fn provider_env_var_name() {
    assert_eq!(provider_env_var("anthropic"), "ANTHROPIC_API_KEY");
    assert_eq!(provider_env_var("openai"), "OPENAI_API_KEY");
    assert_eq!(provider_env_var("Grok"), "GROK_API_KEY");
}

#[test]
fn mask_secret_does_not_slice_mid_utf8() {
    assert_eq!(mask_secret("short"), "***");
    assert_eq!(mask_secret("abcdefghij"), "abcd...ghij");
    // `ö` is 2 bytes; `&val[..4]` would panic when it straddles offset 4.
    let s = format!("abcö{}xy", "d".repeat(8));
    assert!(!s.is_char_boundary(4));
    let masked = mask_secret(&s);
    assert!(masked.contains("..."), "{masked}");
    assert!(masked.is_char_boundary(masked.len()));
    assert!(masked.starts_with("abc"), "{masked}");
}

#[test]
fn missing_database_detection() {
    let not_found = anyhow::anyhow!(std::io::Error::new(std::io::ErrorKind::NotFound, "gone"));
    assert!(is_missing_database(&not_found));

    let permission = anyhow::anyhow!(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "nope"
    ));
    assert!(!is_missing_database(&permission));

    let other = anyhow::anyhow!("boom");
    assert!(!is_missing_database(&other));
}

#[test]
#[allow(clippy::field_reassign_with_default)]
fn resolves_provider_from_flag_then_default_model() {
    let config = Config::default();
    let c = cli(None);
    assert_eq!(resolve_provider(&c, &config), "anthropic");

    let mut flagged = cli(None);
    flagged.provider = Some("openai".into());
    assert_eq!(resolve_provider(&flagged, &config), "openai");

    let mut config2 = Config::default();
    config2.default_model = Some(ModelConfig {
        model_id: "claude-sonnet-4-20250514".into(),
        provider_id: "anthropic".into(),
        max_tokens: None,
        context_window: None,
        temperature: None,
        top_p: None,
        thinking: None,
        supports_tools: None,
        supports_images: None,
    });
    assert_eq!(resolve_provider(&cli(None), &config2), "anthropic");
}

#[test]
fn resolves_model_from_flag_or_default() {
    let config = Config::default();
    let c = cli(None);
    assert_eq!(resolve_model(&c, &config), "claude-sonnet-4-20250514");

    let mut flagged = cli(None);
    flagged.model = Some("gpt-4o".into());
    assert_eq!(resolve_model(&flagged, &config), "gpt-4o");
}

#[test]
fn resolves_agent_from_flag_or_config_default() {
    let config = Config::default();
    assert_eq!(config.default_agent, "build");
    assert_eq!(resolve_agent(&cli(None), &config), "build");

    let mut flagged = cli(None);
    flagged.agent_flag = Some("plan".into());
    assert_eq!(resolve_agent(&flagged, &config), "plan");
}

#[test]
fn resolves_dir_flag_but_dot_means_cwd() {
    let dir = Cli {
        dir: Some("src".into()),
        ..cli(None)
    };
    assert_eq!(resolve_dir(&dir), PathBuf::from("src"));

    let dot = Cli {
        dir: Some(".".into()),
        ..cli(None)
    };
    assert_eq!(resolve_dir(&dot), std::env::current_dir().unwrap());

    assert_eq!(resolve_dir(&cli(None)), std::env::current_dir().unwrap());
}

#[test]
fn resume_flag_wins_over_continue() {
    let c = cli(None);
    assert_eq!(resolve_resume_want(&c), None);

    let mut cont = cli(None);
    cont.continue_session = true;
    assert_eq!(
        resolve_resume_want(&cont),
        Some(whycodes_tui::RESUME_LATEST.to_string())
    );

    let mut both = cli(None);
    both.continue_session = true;
    both.resume = Some("  abc123  ".into());
    assert_eq!(resolve_resume_want(&both), Some("abc123".to_string()));

    let blank = Cli {
        resume: Some("   ".into()),
        continue_session: false,
        ..cli(None)
    };
    assert_eq!(resolve_resume_want(&blank), None);
}

#[test]
fn runtime_choice_per_command() {
    // Interactive TUI / `run` need the multi-thread pool: the loop blocks on
    // `event::poll`, so current_thread starves `tokio::spawn` turns.
    assert!(command_needs_multi_thread(&cli(None)));
    assert!(!command_needs_full_worker_pool(&cli(None)));
    assert!(command_needs_multi_thread(&cli(Some(Commands::Run {
        prompt: None,
        max_turns: None,
        format: OutputFormat::Text,
    }))));
    assert!(!command_needs_full_worker_pool(&cli(Some(Commands::Run {
        prompt: None,
        max_turns: None,
        format: OutputFormat::Text,
    }))));
    assert!(command_needs_multi_thread(&cli(Some(Commands::Mcp {
        cmd: McpCmd::List
    }))));
    assert!(!command_needs_full_worker_pool(&cli(Some(Commands::Mcp {
        cmd: McpCmd::List
    }))));
    assert!(!command_needs_multi_thread(&cli(Some(Commands::Session {
        cmd: SessionCmd::List
    }))));
    assert!(!command_needs_multi_thread(&cli(Some(Commands::Config {
        cmd: ConfigCmd::Show
    }))));
}

#[test]
fn auto_update_only_interactive_text_sessions() {
    // Do not call `should_auto_update` for the "on" cases: GitHub Actions
    // sets `CI=true`, which is a real production opt-out.
    assert!(should_auto_update_with_env(
        &cli(None),
        true,
        false,
        false,
        false
    ));
    assert!(should_auto_update_with_env(
        &cli(Some(Commands::Run {
            prompt: None,
            max_turns: None,
            format: OutputFormat::Text,
        })),
        true,
        false,
        false,
        false
    ));
    assert!(!should_auto_update_with_env(
        &cli(Some(Commands::Run {
            prompt: Some("hi".into()),
            max_turns: None,
            format: OutputFormat::Json,
        })),
        true,
        false,
        false,
        false
    ));
    assert!(!should_auto_update_with_env(
        &cli(Some(Commands::Generate {
            prompt: vec!["x".into()],
            max_turns: None,
            jobs: 1,
            format: OutputFormat::Text,
        })),
        true,
        false,
        false,
        false
    ));
    let mut off = cli(None);
    off.no_auto_update = true;
    assert!(!should_auto_update_with_env(
        &off, true, false, false, false
    ));
    assert!(!should_auto_update_with_env(
        &cli(None),
        false,
        false,
        false,
        false
    ));
    assert!(!should_auto_update_with_env(
        &cli(None),
        true,
        true,
        false,
        false
    ));
    assert!(!should_auto_update_with_env(
        &cli(None),
        true,
        false,
        true,
        false
    ));
    // First-frame / idle harness: no GitHub, no home update dialog.
    assert!(!should_auto_update_with_env(
        &cli(None),
        true,
        false,
        false,
        true
    ));
}

#[test]
fn parses_output_formats() {
    assert_eq!(parse_output_format("text"), Ok(OutputFormat::Text));
    assert_eq!(parse_output_format("json"), Ok(OutputFormat::Json));
    assert_eq!(
        parse_output_format("stream-json"),
        Ok(OutputFormat::StreamJson)
    );
    assert_eq!(parse_output_format("ndjson"), Ok(OutputFormat::StreamJson));
    assert!(parse_output_format("bogus").is_err());
}

#[test]
fn cli_parser_maps_global_flags_and_nested_commands() {
    let parsed = Cli::try_parse_from([
        "whycodes",
        "--provider",
        "openai",
        "--model",
        "gpt-5",
        "generate",
        "fix it",
        "--max-turns",
        "7",
        "--jobs",
        "2",
        "--output-format",
        "ndjson",
    ])
    .unwrap();

    assert_eq!(parsed.provider.as_deref(), Some("openai"));
    assert_eq!(parsed.model.as_deref(), Some("gpt-5"));
    match parsed.command {
        Some(Commands::Generate {
            prompt,
            max_turns,
            jobs,
            format,
        }) => {
            assert_eq!(prompt, ["fix it"]);
            assert_eq!(max_turns, Some(7));
            assert_eq!(jobs, 2);
            assert_eq!(format, OutputFormat::StreamJson);
        }
        other => panic!("unexpected parsed command: {other:?}"),
    }

    let nested = Cli::try_parse_from(["whycodes", "github", "pr", "view", "42"]).unwrap();
    assert!(matches!(
        nested.command,
        Some(Commands::Github {
            cmd: GithubCmd::Pr {
                action: Some(PrAction::View { number: 42 })
            }
        })
    ));
}

#[test]
fn cli_parser_rejects_invalid_commands_before_dispatch() {
    use clap::error::ErrorKind;

    for (args, kind) in [
        (
            vec!["whycodes", "generate"],
            ErrorKind::MissingRequiredArgument,
        ),
        (
            vec!["whycodes", "run", "--format", "yaml"],
            ErrorKind::ValueValidation,
        ),
        (
            vec!["whycodes", "github", "pr", "view", "not-a-number"],
            ErrorKind::ValueValidation,
        ),
        (
            vec!["whycodes", "not-a-command"],
            ErrorKind::InvalidSubcommand,
        ),
    ] {
        let error = Cli::try_parse_from(args).unwrap_err();
        assert_eq!(error.kind(), kind);
    }
}

#[test]
fn cli_parser_maps_mcp_add_without_interpreting_values() {
    let parsed = Cli::try_parse_from([
        "whycodes",
        "mcp",
        "add",
        "docs",
        "node",
        "--args",
        "server.js --quiet",
        "--type",
        "local",
        "--header",
        "Authorization: Bearer token",
        "--header",
        "X-Trace: test",
    ])
    .unwrap();

    match parsed.command {
        Some(Commands::Mcp {
            cmd:
                McpCmd::Add {
                    name,
                    command,
                    args,
                    url,
                    transport,
                    headers,
                },
        }) => {
            assert_eq!(name, "docs");
            assert_eq!(command.as_deref(), Some("node"));
            assert_eq!(args.as_deref(), Some("server.js --quiet"));
            assert_eq!(url, None);
            assert_eq!(transport.as_deref(), Some("local"));
            assert_eq!(headers, ["Authorization: Bearer token", "X-Trace: test"]);
        }
        other => panic!("unexpected parsed command: {other:?}"),
    }
}

#[test]
fn runtime_for_builds_the_selected_runtime_flavor() {
    use tokio::runtime::RuntimeFlavor;

    // Interactive TUI needs MultiThread so spawned turns run during poll.
    // Worker count is `TUI_WORKER_THREADS` (2); tokio's public handle
    // does not expose `metrics_num_workers`, so flavor is the contract.
    let interactive = runtime_for(&cli(None)).unwrap();
    assert_eq!(
        interactive.handle().runtime_flavor(),
        RuntimeFlavor::MultiThread
    );

    let local = runtime_for(&cli(Some(Commands::Config {
        cmd: ConfigCmd::Path,
    })))
    .unwrap();
    assert_eq!(
        local.handle().runtime_flavor(),
        RuntimeFlavor::CurrentThread
    );
}

#[test]
fn truncate_str_short_and_long() {
    assert_eq!(truncate_str("hello", 10), "hello");
    assert_eq!(truncate_str("hello world", 8), "hello...");
    assert_eq!(truncate_str("", 3), "");
    // exact boundary stays untouched
    assert_eq!(truncate_str("abcdef", 6), "abcdef");
    // multibyte chars are counted as chars, not bytes: keeps max_len - 3
    let t = truncate_str("héllo wörld", 6);
    assert_eq!(t, "hél...");
}

#[test]
fn expand_user_input_inlines_existing_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "hello file").unwrap();
    let out = expand_user_input("read @note.txt please", dir.path());
    assert!(out.contains("--- file: note.txt ---"), "{out}");
    assert!(out.contains("hello file"), "{out}");
    assert!(out.contains("--- end file ---"), "{out}");
    // the surrounding text survives
    assert!(out.starts_with("read "), "{out}");
    assert!(out.ends_with(" please"), "{out}");
}

#[test]
fn expand_user_input_keeps_missing_files_literal() {
    let dir = tempfile::tempdir().unwrap();
    let out = expand_user_input("see @nope.txt now", dir.path());
    assert_eq!(out, "see @nope.txt now");
}

#[test]
fn expand_user_input_truncates_huge_files() {
    let dir = tempfile::tempdir().unwrap();
    let big = "x".repeat(AT_FILE_MAX_CHARS + 50);
    std::fs::write(dir.path().join("big.txt"), &big).unwrap();
    let out = expand_user_input("@big.txt", dir.path());
    assert!(
        out.contains("characters omitted"),
        "{} chars",
        out.chars().count()
    );
    // the file body is capped: fewer 'x' chars than the source file holds
    assert!(
        out.matches('x').count() < big.matches('x').count(),
        "file body must be truncated"
    );
}

#[test]
fn expand_user_input_bare_at_and_multiple() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "A").unwrap();
    std::fs::write(dir.path().join("b.txt"), "B").unwrap();
    // bare @ stays literal
    assert_eq!(expand_user_input("just @", dir.path()), "just @");
    // two files in one prompt
    let out = expand_user_input("@a.txt and @b.txt", dir.path());
    assert!(out.contains("--- file: a.txt ---"), "{out}");
    assert!(out.contains("--- file: b.txt ---"), "{out}");
    // absolute path is used as-is
    let abs = dir.path().join("a.txt");
    let out = expand_user_input(&format!("@{} hi", abs.display()), dir.path());
    assert!(out.contains("--- file:"), "{out}");
    assert!(out.contains("A"), "{out}");
}

#[test]
fn turn_event_to_ci_maps_all_variants() {
    use whycodes_agent::events::TurnEvent as TE;

    assert_eq!(
        turn_event_to_ci(TE::TextDelta("hi".into())),
        Some(CiEvent::TextDelta { text: "hi".into() })
    );
    assert_eq!(
        turn_event_to_ci(TE::ThinkingDelta("think".into())),
        Some(CiEvent::ThinkingDelta {
            text: "think".into()
        })
    );
    assert_eq!(
        turn_event_to_ci(TE::Status("busy".into())),
        Some(CiEvent::Status {
            message: "busy".into()
        })
    );
    assert_eq!(turn_event_to_ci(TE::Cancelled), Some(CiEvent::Cancelled));

    let tool_start = turn_event_to_ci(TE::ToolStart {
        id: "t1".into(),
        name: "read".into(),
        input: serde_json::json!({}),
    });
    assert_eq!(
        tool_start,
        Some(CiEvent::ToolStart {
            id: "t1".into(),
            name: "read".into(),
            input: serde_json::json!({})
        })
    );

    let usage = turn_event_to_ci(TE::Usage(whycodes_core::types::Usage {
        input_tokens: 10,
        output_tokens: 20,
        cache_creation_input_tokens: Some(30),
        cache_read_input_tokens: None,
    }));
    assert_eq!(
        usage,
        Some(CiEvent::Usage {
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_input_tokens: Some(30),
            cache_read_input_tokens: None
        })
    );

    // intent surfaces as a status line
    let intent = turn_event_to_ci(TE::Intent {
        kind: "question".into(),
        confidence: 0.9,
        badge: "Q".into(),
        notice_kind: "info".into(),
        notice: "ask first".into(),
    });
    assert_eq!(
        intent,
        Some(CiEvent::Status {
            message: "intent:question conf=0.90 badge=Q [info] ask first".into()
        })
    );

    // panel updates collapse to status lines
    let panel = turn_event_to_ci(TE::Panel(whycodes_core::PanelUpdate::File {
        path: "x.rs".into(),
        text: "...".into(),
    }));
    assert_eq!(
        panel,
        Some(CiEvent::Status {
            message: "panel file=x.rs".into()
        })
    );

    // swarm / background / permission notifications surface as status
    let swarm = turn_event_to_ci(TE::SwarmStatus {
        active: 1,
        total: 2,
        message: "".into(),
    });
    assert_eq!(
        swarm,
        Some(CiEvent::Status {
            message: "swarm active=1 total=2".into()
        })
    );
    let bg = turn_event_to_ci(TE::Background {
        id: "b1".into(),
        status: "done".into(),
        summary: "ok".into(),
    });
    assert_eq!(
        bg,
        Some(CiEvent::Status {
            message: "bg b1 done: ok".into()
        })
    );
    let perm = turn_event_to_ci(TE::PermissionAsk {
        request_id: "r1".into(),
        tool_name: "write".into(),
        detail: "d".into(),
    });
    assert_eq!(
        perm,
        Some(CiEvent::Status {
            message: "permission_request id=r1 tool=write".into()
        })
    );
    let child = turn_event_to_ci(TE::Subagent {
        id: "task-1".into(),
        kind: "explore".into(),
        description: "scan".into(),
        status: "running".into(),
        activity: "Thinking".into(),
        elapsed_ms: 0,
        output: String::new(),
    });
    assert_eq!(
        child,
        Some(CiEvent::Status {
            message: "subagent task-1 running (explore): scan".into()
        })
    );
}

#[test]
fn get_and_set_config_value_roundtrip() {
    let mut config = Config::default();
    assert_eq!(
        get_config_value(&config, "default_agent"),
        Some("build".into())
    );
    assert_eq!(get_config_value(&config, "bogus"), None);

    set_config_value(&mut config, "default_agent", "plan").unwrap();
    assert_eq!(config.default_agent, "plan");
    set_config_value(&mut config, "project_path", "/tmp/proj").unwrap();
    assert_eq!(
        config.general.project_path.as_deref(),
        Some(std::path::Path::new("/tmp/proj"))
    );
    set_config_value(&mut config, "log_level", "debug").unwrap();
    assert_eq!(config.general.log_level.as_deref(), Some("debug"));
    assert!(set_config_value(&mut config, "bogus", "x").is_err());
}

#[test]
fn auth_expiry_labels() {
    use whycodes_auth::token::ProviderAuth;
    let auth = |token: whycodes_auth::token::OAuthToken| ProviderAuth {
        method: "oauth".into(),
        token,
    };

    // no expiry
    let none = auth(whycodes_auth::token::OAuthToken {
        access_token: "a".into(),
        refresh_token: None,
        expires_at: None,
        extra: Default::default(),
    });
    assert_eq!(auth_expiry_label(&none), "no expiry");

    // future expiry → "expires <at>"
    let future = chrono::Utc::now() + chrono::Duration::days(30);
    let tok = whycodes_auth::token::OAuthToken {
        access_token: "a".into(),
        refresh_token: None,
        expires_at: Some(future),
        extra: Default::default(),
    };
    let label = auth_expiry_label(&auth(tok));
    assert!(label.starts_with("expires "), "{label}");
    assert!(!label.contains("expired"), "{label}");

    // past expiry → "expired <at> (refreshes...)"
    let past = chrono::Utc::now() - chrono::Duration::days(1);
    let tok = whycodes_auth::token::OAuthToken {
        access_token: "a".into(),
        refresh_token: None,
        expires_at: Some(past),
        extra: Default::default(),
    };
    let label = auth_expiry_label(&auth(tok));
    assert!(label.starts_with("expired "), "{label}");
    assert!(label.contains("refreshes on next use"), "{label}");

    // derived expiry wins over access-token expiry
    let tok = whycodes_auth::token::OAuthToken {
        access_token: "a".into(),
        refresh_token: None,
        expires_at: Some(future),
        extra: serde_json::json!({ "derived_expires_at": "2030-01-01T00:00:00Z" })
            .as_object()
            .unwrap()
            .clone(),
    };
    let label = auth_expiry_label(&auth(tok));
    assert_eq!(label, "derived API token expires 2030-01-01T00:00:00Z");

    // legacy copilot key still resolves
    let tok = whycodes_auth::token::OAuthToken {
        access_token: "a".into(),
        refresh_token: None,
        expires_at: None,
        extra: serde_json::json!({ "copilot_expires_at": "2031-01-01T00:00:00Z" })
            .as_object()
            .unwrap()
            .clone(),
    };
    let label = auth_expiry_label(&auth(tok));
    assert_eq!(label, "derived API token expires 2031-01-01T00:00:00Z");
}

#[test]
fn memory_settings_for_sets_agent_bank() {
    let config = Config::default();
    let base = memory_settings_for(&config, None);
    assert_eq!(base.agent_bank, None);
    let with_bank = memory_settings_for(&config, Some("build".into()));
    assert_eq!(with_bank.agent_bank.as_deref(), Some("build"));
    assert_eq!(base.enabled, with_bank.enabled);
}

#[test]
fn headless_setup_error_is_always_err() {
    let err = emit_headless_setup_error(OutputFormat::Text, "missing key").unwrap_err();
    assert!(err.to_string().contains("missing key"));
    let err = emit_headless_setup_error(OutputFormat::Json, "empty prompt").unwrap_err();
    assert!(err.to_string().contains("empty prompt"));
    let err = emit_headless_setup_error(OutputFormat::StreamJson, "nope").unwrap_err();
    assert!(err.to_string().contains("nope"));
}

#[test]
fn version_only_argv_and_tui_invoke() {
    assert!(is_version_only_argv(["--version"]));
    assert!(is_version_only_argv(["-V"]));
    assert!(!is_version_only_argv(Vec::<&str>::new()));
    assert!(!is_version_only_argv(["--version", "--debug"]));
    assert!(!is_version_only_argv(["config"]));
    assert!(!is_version_only_argv(["-V", "extra"]));

    assert!(is_tui_invoke(&cli(None)));
    assert!(is_tui_invoke(&cli(Some(Commands::Run {
        prompt: None,
        max_turns: None,
        format: OutputFormat::Text,
    }))));
    assert!(is_tui_invoke(&cli(Some(Commands::Connect {
        addr: "127.0.0.1:3030".into(),
        session: None,
    }))));
    let mut plain = cli(None);
    plain.plain = true;
    assert!(!is_tui_invoke(&plain));
    assert!(!is_tui_invoke(&cli(Some(Commands::Stats))));
}

#[test]
fn interactive_mode_drops_max_turns() {
    assert_eq!(ignore_max_turns_interactive(None), None);
    assert_eq!(ignore_max_turns_interactive(Some(25)), None);
    assert_eq!(ignore_max_turns_interactive(Some(1)), None);
}

#[test]
fn runtime_choice_covers_remaining_commands() {
    assert!(command_needs_multi_thread(&cli(Some(Commands::Generate {
        prompt: vec!["x".into()],
        max_turns: Some(1),
        jobs: 1,
        format: OutputFormat::Text,
    }))));
    assert!(command_needs_full_worker_pool(&cli(Some(
        Commands::Generate {
            prompt: vec!["x".into()],
            max_turns: Some(1),
            jobs: 1,
            format: OutputFormat::Text,
        }
    ))));
    assert!(command_needs_multi_thread(&cli(Some(Commands::Acp))));
    assert!(command_needs_multi_thread(&cli(Some(Commands::Pr {
        title: None,
        base: None,
    }))));
    assert!(command_needs_multi_thread(&cli(Some(Commands::Github {
        cmd: GithubCmd::Pr { action: None },
    }))));
    assert!(command_needs_multi_thread(&cli(Some(Commands::Web))));
    assert!(command_needs_multi_thread(&cli(Some(Commands::Connect {
        addr: "x".into(),
        session: None,
    }))));
    assert!(command_needs_multi_thread(&cli(Some(Commands::Auth {
        cmd: AuthCmd::Status,
    }))));
    #[cfg(feature = "server")]
    assert!(command_needs_multi_thread(&cli(Some(Commands::Serve {
        port: 1
    }))));
    #[cfg(feature = "server")]
    assert!(command_needs_full_worker_pool(&cli(Some(
        Commands::Serve { port: 1 }
    ))));
    #[cfg(feature = "self-update")]
    assert!(command_needs_multi_thread(&cli(Some(Commands::Upgrade))));
    assert!(!command_needs_multi_thread(&cli(Some(
        Commands::Provider {
            cmd: ProviderCmd::List,
        }
    ))));
    assert!(!command_needs_multi_thread(&cli(Some(Commands::Model {
        cmd: ModelCmd::List,
    }))));
    assert!(!command_needs_multi_thread(&cli(Some(Commands::Agent {
        name: None
    }))));
    assert!(!command_needs_multi_thread(&cli(Some(Commands::Plugins {
        cmd: None
    }))));
    assert!(!command_needs_multi_thread(&cli(Some(Commands::Memory {
        cmd: MemoryCmd::Path,
    }))));
    assert!(!command_needs_multi_thread(&cli(Some(Commands::Stats))));
    assert!(!command_needs_multi_thread(&cli(Some(Commands::Debug))));
    assert!(!command_needs_multi_thread(&cli(Some(
        Commands::Completions {
            shell: clap_complete::Shell::Bash,
        }
    ))));
}

#[test]
fn turn_event_to_ci_remaining_variants() {
    use whycodes_agent::events::TurnEvent as TE;
    assert_eq!(
        turn_event_to_ci(TE::ToolEnd {
            id: "t".into(),
            content: "ok".into(),
            is_error: false,
        }),
        Some(CiEvent::ToolEnd {
            id: "t".into(),
            content: "ok".into(),
            is_error: false,
        })
    );
    assert_eq!(
        turn_event_to_ci(TE::FileConflict {
            path: "a.rs".into(),
            claimant: "w1".into(),
            owner: "w0".into(),
        }),
        Some(CiEvent::Status {
            message: "file_conflict path=a.rs claimant=w1 owner=w0".into()
        })
    );
    assert_eq!(
        turn_event_to_ci(TE::SwarmStatus {
            active: 1,
            total: 2,
            message: "busy".into(),
        }),
        Some(CiEvent::Status {
            message: "busy".into()
        })
    );
    assert_eq!(
        turn_event_to_ci(TE::EnqueuePrompt {
            text: "next".into()
        }),
        Some(CiEvent::Status {
            message: "enqueue_prompt: next".into()
        })
    );
    assert_eq!(
        turn_event_to_ci(TE::SwarmMessage {
            from: "a".into(),
            to: "b".into(),
            text: "hi".into(),
        }),
        Some(CiEvent::Status {
            message: "swarm_msg from=a to=b: hi".into()
        })
    );
    assert_eq!(
        turn_event_to_ci(TE::FileStale {
            path: "x".into(),
            reader: "r".into(),
            writer: "w".into(),
        }),
        Some(CiEvent::Status {
            message: "file_stale path=x reader=r writer=w".into()
        })
    );
    assert_eq!(
        turn_event_to_ci(TE::QuestionAsk {
            request_id: "q1".into(),
            questions: serde_json::json!([]),
        }),
        Some(CiEvent::Status {
            message: "question_request id=q1".into()
        })
    );
    assert_eq!(
        turn_event_to_ci(TE::Panel(whycodes_core::PanelUpdate::Clear)),
        Some(CiEvent::Status {
            message: "panel clear".into()
        })
    );
    assert_eq!(
        turn_event_to_ci(TE::Panel(whycodes_core::PanelUpdate::Diff {
            path: "d.rs".into(),
            unified: String::new(),
        })),
        Some(CiEvent::Status {
            message: "panel diff=d.rs".into()
        })
    );
    assert_eq!(
        turn_event_to_ci(TE::Panel(whycodes_core::PanelUpdate::Mermaid {
            source: "graph TD".into(),
        })),
        Some(CiEvent::Status {
            message: "panel mermaid".into()
        })
    );
    assert_eq!(
        turn_event_to_ci(TE::Todos {
            todos: vec![
                whycodes_core::TodoItem::new("a", "one", whycodes_core::TodoStatus::Completed),
                whycodes_core::TodoItem::new("b", "two", whycodes_core::TodoStatus::Pending),
            ]
        }),
        Some(CiEvent::Status {
            message: "todos 1/2".into()
        })
    );
    let intent = turn_event_to_ci(TE::Intent {
        kind: "k".into(),
        confidence: 1.0,
        badge: String::new(),
        notice_kind: "n".into(),
        notice: String::new(),
    });
    assert_eq!(
        intent,
        Some(CiEvent::Status {
            message: "intent:k conf=1.00".into()
        })
    );
}

#[test]
fn get_config_value_project_path() {
    let mut config = Config::default();
    assert_eq!(get_config_value(&config, "project_path"), None);
    config.general.project_path = Some(PathBuf::from("/tmp/p"));
    assert_eq!(
        get_config_value(&config, "project_path"),
        Some("/tmp/p".into())
    );
}

#[test]
fn run_shell_capture_stdout_stderr_and_empty() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_shell_capture("printf hi", dir.path());
    assert!(out.contains("hi"), "{out}");
    let mixed = run_shell_capture("printf out; printf err >&2", dir.path());
    assert!(mixed.contains("out"), "{mixed}");
    assert!(mixed.contains("err"), "{mixed}");
    let empty = run_shell_capture("true", dir.path());
    assert!(empty.contains("exit"), "{empty}");
}

#[test]
fn print_slash_help_and_switch_agent() {
    print_slash_help();
    let config = Config::default();
    let dir = tempfile::tempdir().unwrap();
    let (name, _agent, prompt) = switch_agent("build", &config, dir.path()).unwrap();
    assert_eq!(name, "build");
    assert!(!prompt.is_empty());
    assert!(switch_agent("nope", &config, dir.path()).is_err());
}

#[test]
fn early_print_version_from_prints_and_returns() {
    assert!(early_print_version_from(["--version"]));
    assert!(!early_print_version_from(["config"]));
}

#[test]
fn key_from_env_config_and_missing_message() {
    let config = Config::default();
    assert_eq!(
        key_from_env_and_config("anthropic", &config, |_| None),
        None
    );
    assert_eq!(
        key_from_env_and_config("anthropic", &config, |k| {
            (k == "ANTHROPIC_API_KEY").then(|| "sk-env".into())
        }),
        Some("sk-env".into())
    );
    assert_eq!(
        key_from_env_and_config("anthropic", &config, |k| {
            (k == "ANTHROPIC_API_KEY").then(|| "".into())
        }),
        None
    );

    let mut with_key = Config::default();
    with_key.providers.insert(
        "grok".into(),
        ProviderConfig {
            name: "grok".into(),
            api_key: Some("cfg-key".into()),
            api_base: None,
            base_url: None,
            headers: None,
            models: vec![],
            tool_arguments: None,
            extra: Default::default(),
        },
    );
    assert_eq!(
        key_from_env_and_config("grok", &with_key, |_| None),
        Some("cfg-key".into())
    );
    assert_eq!(
        key_from_env_and_config("openai", &config, |k| {
            (k == "OPENAI_API_KEY").then(|| "".into())
        }),
        Some("".into())
    );

    let anthropic = missing_api_key_message_for("anthropic", None);
    assert!(anthropic.contains("ANTHROPIC_API_KEY"), "{anthropic}");
    // Subscription login is plugin-loaded; default installs have an empty
    // OAuth registry so the hint is absent unless an auth plugin registered.
    assert_eq!(
        anthropic.contains("auth login"),
        whycodes_auth::providers::supports_oauth("anthropic"),
        "{anthropic}"
    );
    let custom = missing_api_key_message_for("acme", None);
    assert!(custom.contains("ACME_API_KEY"), "{custom}");
    assert!(!custom.contains("auth login"), "{custom}");

    // Cover the hint branch without depending on extras plugins.
    whycodes_auth::register_spec(whycodes_auth::ProviderSpec {
        name: "cli-oauth-hint-demo".into(),
        label: "Demo".into(),
        flow: whycodes_auth::FlowKind::DeviceCode,
        client_id: "cid".into(),
        client_secret: None,
        authorize_url: "https://example.com/auth".into(),
        token_url: "https://example.com/token".into(),
        scopes: "read".into(),
        token_encoding: whycodes_auth::TokenEncoding::Form,
        redirect_uri: None,
        loopback_port: None,
        loopback_host: None,
        callback_path: String::new(),
        extra_authorize: vec![],
        derived: None,
        suggested_models: vec![],
        inference: None,
    });
    let hinted = missing_api_key_message_for("cli-oauth-hint-demo", None);
    assert!(hinted.contains("CLI-OAUTH-HINT-DEMO_API_KEY"), "{hinted}");
    assert!(hinted.contains("auth login"), "{hinted}");

    let ollama = missing_api_key_message_for("ollama", None);
    assert!(
        ollama.contains("local host") || ollama.contains("Ollama"),
        "{ollama}"
    );
    assert!(!ollama.contains("OLLAMA_API_KEY"), "{ollama}");

    let mut cfg = Config::default();
    cfg.providers.insert(
        "anthropic".into(),
        ProviderConfig {
            name: "anthropic".into(),
            api_key: None,
            api_base: None,
            base_url: Some("http://127.0.0.1:4554".into()),
            headers: None,
            models: vec![],
            tool_arguments: None,
            extra: Default::default(),
        },
    );
    let proxied = missing_api_key_message_for("anthropic", Some(&cfg));
    assert!(proxied.contains("local host"), "{proxied}");
    assert!(!proxied.contains("ANTHROPIC_API_KEY"), "{proxied}");
    let cloud = missing_api_key_message_for("anthropic", None);
    assert!(cloud.contains("ANTHROPIC_API_KEY"), "{cloud}");
}

#[test]
fn agent_info_for_known_and_fallback() {
    let config = Config::default();
    let known = agent_info_for(&cli(None), &config);
    assert_eq!(known.name, "build");
    let mut flagged = cli(None);
    flagged.agent_flag = Some("does-not-exist".into());
    let fallback = agent_info_for(&flagged, &config);
    assert_eq!(fallback.name, "build");
    assert_eq!(fallback.description, "Default build agent");
}

fn dummy_meta() -> ResultMeta {
    ResultMeta {
        session_id: "s".into(),
        provider: "p".into(),
        model: "m".into(),
        agent: "a".into(),
        usage: whycodes_core::types::Usage::default(),
        duration_ms: 1,
    }
}

#[test]
fn emit_turn_and_parallel_outcomes() {
    assert!(emit_turn_outcome(OutputFormat::Text, Ok("hi".into()), dummy_meta()).is_ok());
    assert!(emit_turn_outcome(OutputFormat::Text, Ok(String::new()), dummy_meta()).is_ok());
    assert!(emit_turn_outcome(OutputFormat::Json, Ok("j".into()), dummy_meta()).is_ok());
    assert!(emit_turn_outcome(OutputFormat::StreamJson, Ok("s".into()), dummy_meta()).is_ok());
    assert!(emit_turn_outcome(OutputFormat::Text, Err("boom".into()), dummy_meta()).is_err());
    assert!(emit_turn_outcome(OutputFormat::Json, Err("jerr".into()), dummy_meta()).is_err());
    assert!(
        emit_turn_outcome(
            OutputFormat::StreamJson,
            Err("cancelled".into()),
            dummy_meta()
        )
        .is_err()
    );
    assert!(
        emit_turn_outcome(
            OutputFormat::StreamJson,
            Err("offline".into()),
            dummy_meta()
        )
        .is_err()
    );

    let wrap = |ev: CiEvent| ev;
    assert!(!emit_parallel_outcome(
        OutputFormat::Text,
        Ok("p".into()),
        dummy_meta(),
        &wrap
    ));
    assert!(!emit_parallel_outcome(
        OutputFormat::Text,
        Ok(String::new()),
        dummy_meta(),
        &wrap
    ));
    assert!(!emit_parallel_outcome(
        OutputFormat::Json,
        Ok("p".into()),
        dummy_meta(),
        &wrap
    ));
    assert!(!emit_parallel_outcome(
        OutputFormat::StreamJson,
        Ok("p".into()),
        dummy_meta(),
        &wrap
    ));
    assert!(emit_parallel_outcome(
        OutputFormat::Text,
        Err("x".into()),
        dummy_meta(),
        &wrap
    ));
    assert!(emit_parallel_outcome(
        OutputFormat::Json,
        Err("x".into()),
        dummy_meta(),
        &wrap
    ));
    assert!(emit_parallel_outcome(
        OutputFormat::StreamJson,
        Err("cancel now".into()),
        dummy_meta(),
        &wrap
    ));
    assert!(emit_parallel_outcome(
        OutputFormat::StreamJson,
        Err("fail".into()),
        dummy_meta(),
        &wrap
    ));
}

#[test]
fn fold_parallel_joins_ok_fail_and_panic() {
    assert!(fold_parallel_joins([Ok(false), Ok(false)], false).is_ok());
    assert!(fold_parallel_joins([Ok(false), Ok(true)], false).is_err());
    assert!(fold_parallel_joins([Err("worker panicked: boom".into())], false).is_err());
    assert!(fold_parallel_joins([Err("worker panicked: boom".into())], true).is_err());
}

#[tokio::test]
async fn headless_turn_with_scripted_provider() {
    let dir = tempfile::tempdir().unwrap();
    let mut registry = whycodes_llm::ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::text("hello")));
    let agent =
        Agent::new(agent_info_for(&cli(None), &Config::default())).with_provider_registry(registry);
    let mut session =
        whycodes_session::session::Session::new(dir.path().to_path_buf(), "sys".into());
    session.add_user_message("hi");
    run_headless_turn(
        &agent,
        &mut session,
        "script",
        "m",
        "k",
        "build",
        Some(4),
        OutputFormat::Text,
    )
    .await
    .unwrap();

    let mut registry = whycodes_llm::ProviderRegistry::new();
    registry.register(Box::new(whycodes_llm::ScriptedProvider::new([
        whycodes_llm::ScriptedStep::FailOpen("cancelled by test".into()),
    ])));
    let agent =
        Agent::new(agent_info_for(&cli(None), &Config::default())).with_provider_registry(registry);
    let mut session =
        whycodes_session::session::Session::new(dir.path().to_path_buf(), "sys".into());
    session.add_user_message("please fail");
    assert!(
        run_headless_turn(
            &agent,
            &mut session,
            "script",
            "m",
            "k",
            "build",
            Some(4),
            OutputFormat::StreamJson,
        )
        .await
        .is_err()
    );
}

#[test]
fn generate_prompt_helpers() {
    assert!(all_prompts_empty(&["".into(), "".into()]));
    assert!(!all_prompts_empty(&["".into(), "x".into()]));
    assert!(!should_fan_out(&["one".into()]));
    assert!(should_fan_out(&["a".into(), "b".into()]));
}

#[tokio::test]
async fn cmd_generate_single_and_parallel_unknown_provider() {
    let dir = tempfile::tempdir().unwrap();
    let prev_home = std::env::var_os("WHYCODES_HOME");
    let prev_key = std::env::var_os("SCRIPT_API_KEY");
    unsafe { std::env::set_var("WHYCODES_HOME", dir.path()) };
    unsafe { std::env::set_var("SCRIPT_API_KEY", "k") };
    let mut c = cli(None);
    c.provider = Some("script".into());
    c.dir = Some(dir.path().display().to_string());
    c.no_memory = true;
    let single = cmd_generate(&c, &["hello".into()], Some(1), 1, OutputFormat::Text).await;
    let parallel = cmd_generate(
        &c,
        &["a".into(), "b".into(), "".into()],
        Some(1),
        2,
        OutputFormat::Json,
    )
    .await;
    match prev_home {
        Some(v) => unsafe { std::env::set_var("WHYCODES_HOME", v) },
        None => unsafe { std::env::remove_var("WHYCODES_HOME") },
    }
    match prev_key {
        Some(v) => unsafe { std::env::set_var("SCRIPT_API_KEY", v) },
        None => unsafe { std::env::remove_var("SCRIPT_API_KEY") },
    }
    assert!(single.is_err(), "{single:?}");
    assert!(parallel.is_err(), "{parallel:?}");
}

#[tokio::test]
async fn run_one_parallel_turn_unknown_provider_fails() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::default();
    let info = agent_info_for(&cli(None), &config);
    let failed = run_one_parallel_turn(
        "hi",
        &config,
        info,
        "script",
        "m",
        "build",
        "k",
        Some(1),
        OutputFormat::Text,
        dir.path(),
        false,
    )
    .await;
    assert!(failed);
}

#[test]
fn strip_agents_fence_and_cancel_message() {
    assert_eq!(strip_agents_fence("```markdown\nHi\n```"), "Hi");
    assert_eq!(strip_agents_fence("```md\nHi\n```"), "Hi");
    assert_eq!(strip_agents_fence("```\nHi\n```"), "Hi");
    assert_eq!(strip_agents_fence("  already clean  "), "already clean");
    assert!(is_cancel_message("Cancelled by user"));
    assert!(is_cancel_message("request cancel"));
    assert!(!is_cancel_message("timeout"));
}

#[cfg(feature = "self-update")]
mod upgrade_helpers {
    use crate::upgrade::*;
    use std::io::Write;
    use std::path::Path;

    #[test]
    fn archive_for_every_published_target_and_unknown() {
        assert!(archive_for("linux", "x86_64").unwrap().contains("linux"));
        assert!(archive_for("macos", "aarch64").unwrap().contains("darwin"));
        assert!(archive_for("macos", "x86_64").unwrap().contains("darwin"));
        assert!(archive_for("windows", "x86_64").unwrap().ends_with(".zip"));
        assert!(archive_for("plan9", "arm").is_err());
        assert!(target_archive().is_ok());
    }

    #[test]
    fn token_from_env_prefers_github_then_gh() {
        assert_eq!(token_from_env(|_| None), None);
        assert_eq!(
            token_from_env(|k| (k == "GITHUB_TOKEN").then(|| "".into())),
            None
        );
        assert_eq!(
            token_from_env(|k| (k == "GITHUB_TOKEN").then(|| "ghs".into())),
            Some("ghs".into())
        );
        assert_eq!(
            token_from_env(|k| (k == "GH_TOKEN").then(|| "gh".into())),
            Some("gh".into())
        );
    }

    #[test]
    fn status_and_asset_hints() {
        assert!(!status_hint(404, false).is_empty());
        assert!(status_hint(404, true).is_empty());
        assert!(status_hint(500, false).is_empty());
        assert!(!asset_download_hint(404, false).is_empty());
        assert!(asset_download_hint(403, false).is_empty());
    }

    #[test]
    fn find_asset_id_and_release_version() {
        let body = serde_json::json!({
            "tag_name": "v1.2.3",
            "assets": [{"name": "whycodes.tgz", "id": 9}]
        });
        assert_eq!(release_version(&body).unwrap(), "1.2.3");
        assert_eq!(find_asset_id(&body, "whycodes.tgz").unwrap(), 9);
        assert!(find_asset_id(&body, "missing").is_err());
        assert!(find_asset_id(&serde_json::json!({}), "x").is_err());
        assert!(find_asset_id(&serde_json::json!({"assets": [{"name": "x"}]}), "x").is_err());
        assert!(release_version(&serde_json::json!({})).is_err());
        let msg = checksum_mismatch("a.tar.gz", "aa", "bb");
        assert!(msg.contains("expected aa"));
        assert!(msg.contains("actual   bb"));
    }

    #[test]
    fn extracts_targz_and_zip_and_rejects_empty() {
        let tar_bytes = {
            let mut raw = Vec::new();
            {
                let mut b = tar::Builder::new(&mut raw);
                let mut h = tar::Header::new_gnu();
                h.set_size(3);
                h.set_cksum();
                b.append_data(&mut h, "whycodes", &b"bin"[..]).unwrap();
                b.finish().unwrap();
            }
            let mut gz = Vec::new();
            let mut enc = flate2::write::GzEncoder::new(&mut gz, flate2::Compression::default());
            enc.write_all(&raw).unwrap();
            enc.finish().unwrap();
            gz
        };
        assert_eq!(extract(&tar_bytes, "whycodes.tar.gz").unwrap(), b"bin");
        assert!(extract(b"not-a-tar", "whycodes.tar.gz").is_err());

        let zip_bytes = {
            let mut buf = Vec::new();
            let cursor = std::io::Cursor::new(&mut buf);
            let mut zip = zip::ZipWriter::new(cursor);
            zip.start_file("whycodes.exe", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"exe").unwrap();
            zip.finish().unwrap();
            buf
        };
        assert_eq!(extract(&zip_bytes, "whycodes.zip").unwrap(), b"exe");
        let empty_zip = {
            let mut buf = Vec::new();
            let cursor = std::io::Cursor::new(&mut buf);
            let mut zip = zip::ZipWriter::new(cursor);
            zip.start_file("readme.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"no").unwrap();
            zip.finish().unwrap();
            buf
        };
        assert!(extract(&empty_zip, "whycodes.zip").is_err());
        assert!(extract(b"nope", "whycodes.zip").is_err());
        assert!(
            extract(&tar_bytes, "other.tar.gz").is_ok()
                || extract(
                    &{
                        let mut raw = Vec::new();
                        {
                            let mut b = tar::Builder::new(&mut raw);
                            let mut h = tar::Header::new_gnu();
                            h.set_size(1);
                            h.set_cksum();
                            b.append_data(&mut h, "readme", &b"x"[..]).unwrap();
                            b.finish().unwrap();
                        }
                        let mut gz = Vec::new();
                        let mut enc =
                            flate2::write::GzEncoder::new(&mut gz, flate2::Compression::default());
                        enc.write_all(&raw).unwrap();
                        enc.finish().unwrap();
                        gz
                    },
                    "whycodes.tar.gz"
                )
                .is_err()
        );
    }

    #[test]
    fn newer_versions_and_digests() {
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("v0.1.1", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.1"));
        assert!(!is_newer("0.9.9", "1.0.0"));
        assert!(!is_newer("1.0", "1.0.0"));
        assert!(is_newer("1.0.1", "1.0"));
        assert!(!is_newer("nightly", "0.1.0"));
        assert!(!is_newer("", "0.1.0"));
        assert!(!is_newer("0.1.x", "0.1.0"));
        assert!(is_newer("0.2.0-rc1", "0.1.0"));
        assert!(!is_newer("0.1.0-rc1", "0.1.0"));
        assert_eq!(
            expected_digest(
                "abc123  whycodes-x86_64-unknown-linux-gnu.tar.gz\ndef456  whycodes-x86_64-pc-windows-msvc.zip\n",
                "whycodes-x86_64-pc-windows-msvc.zip"
            ),
            Some("def456".into())
        );
        assert_eq!(expected_digest("abc", "x"), None);
        assert_eq!(
            expected_digest("abc123 *whycodes.tar.gz", "whycodes.tar.gz"),
            Some("abc123".into())
        );
        assert_eq!(
            digest_of(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn replace_binary_fresh_and_existing() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("whycodes");
        replace_binary(&target, b"fresh").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"fresh");
        replace_binary(&target, b"new").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert!(!dir.path().join(".whycodes.new").exists());
        assert!(!dir.path().join(".whycodes.old").exists());
    }

    #[test]
    fn decide_upgrade_up_to_date_mismatch_and_install() {
        let name = "whycodes.tar.gz";
        let tar_bytes = {
            let mut raw = Vec::new();
            {
                let mut b = tar::Builder::new(&mut raw);
                let mut h = tar::Header::new_gnu();
                h.set_size(3);
                h.set_cksum();
                b.append_data(&mut h, "whycodes", &b"bin"[..]).unwrap();
                b.finish().unwrap();
            }
            let mut gz = Vec::new();
            let mut enc = flate2::write::GzEncoder::new(&mut gz, flate2::Compression::default());
            enc.write_all(&raw).unwrap();
            enc.finish().unwrap();
            gz
        };
        let digest = digest_of(&tar_bytes);
        let sums = format!("{digest}  {name}\n");
        assert_eq!(
            decide_upgrade("1.0.0", "1.0.0", &sums, name, &tar_bytes).unwrap(),
            UpgradeDecision::UpToDate
        );
        assert!(decide_upgrade("0.1.0", "0.2.0", "nope", name, &tar_bytes).is_err());
        assert!(
            decide_upgrade(
                "0.1.0",
                "0.2.0",
                "deadbeef  whycodes.tar.gz\n",
                name,
                &tar_bytes
            )
            .unwrap_err()
            .to_string()
            .contains("checksum mismatch")
        );
        match decide_upgrade("0.1.0", "0.2.0", &sums, name, &tar_bytes).unwrap() {
            UpgradeDecision::Install(b) => assert_eq!(b, b"bin"),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            format_upgrade_outcome("0.1.0", Ok(Some("0.2.0".into()))),
            "Upgraded 0.1.0 → 0.2.0"
        );
        assert_eq!(
            format_upgrade_outcome("0.1.0", Ok(None)),
            "Already on the latest release."
        );
        assert!(format_upgrade_outcome("0.1.0", Err("offline".into())).contains("offline"));
    }

    #[test]
    fn current_binary_and_asset_url() {
        let p = current_binary().unwrap();
        assert!(p.is_absolute() || p.file_name().is_some());
        assert!(release_asset_url(42).ends_with("/assets/42"));
    }

    #[test]
    fn homebrew_prefix_is_not_self_updated() {
        assert!(path_looks_like_homebrew(
            "/opt/homebrew/Cellar/whycodes/0.1.0/bin/whycodes"
        ));
        assert!(path_looks_like_homebrew("/opt/homebrew/bin/whycodes"));
        assert!(path_looks_like_homebrew(
            "/home/linuxbrew/.linuxbrew/Cellar/whycodes/0.1.0/bin/whycodes"
        ));
        assert!(path_looks_like_homebrew(
            "/usr/local/Cellar/whycodes/0.1.0/bin/whycodes"
        ));
        assert!(path_looks_like_homebrew(r"C:\opt\homebrew\bin\whycodes"));
        assert!(!path_looks_like_homebrew("/home/me/.local/bin/whycodes"));
        assert!(!path_looks_like_homebrew("/home/me/.cargo/bin/whycodes"));
        assert!(!path_looks_like_homebrew("/usr/bin/whycodes"));
        assert!(
            package_manager_upgrade_hint(Path::new("/opt/homebrew/bin/whycodes"))
                .unwrap()
                .contains("brew upgrade whycodes")
        );
    }

    #[test]
    fn homebrew_bin_symlink_into_cellar_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let cellar = dir
            .path()
            .join("Cellar")
            .join("whycodes")
            .join("0.1.0")
            .join("bin");
        std::fs::create_dir_all(&cellar).unwrap();
        let real = cellar.join("whycodes");
        std::fs::write(&real, b"x").unwrap();
        let bindir = dir.path().join("bin");
        std::fs::create_dir_all(&bindir).unwrap();
        let link = bindir.join("whycodes");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&real, &link).unwrap();
            assert!(
                package_manager_upgrade_hint(&link).is_some(),
                "symlink {} -> {}",
                link.display(),
                real.display()
            );
        }
        assert!(package_manager_upgrade_hint(&real).is_some());
        let script_install = dir.path().join(".local").join("bin").join("whycodes");
        std::fs::create_dir_all(script_install.parent().unwrap()).unwrap();
        std::fs::write(&script_install, b"x").unwrap();
        assert!(package_manager_upgrade_hint(&script_install).is_none());
    }

    #[tokio::test]
    async fn download_bytes_ok_and_404() {
        let url = serve_http(200, "blob").await;
        let client = reqwest::Client::new();
        assert_eq!(
            download_bytes(&client, &url, "x.bin").await.unwrap(),
            b"blob"
        );
        let url = serve_http(404, "no").await;
        let err = download_bytes(&client, &url, "x.bin")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("404"), "{err}");
    }

    async fn serve_http(status: u16, body: &'static str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = sock.read(&mut buf).await;
            let resp = format!(
                "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        });
        format!("http://{addr}/")
    }

    #[tokio::test]
    async fn get_local_ok_and_404() {
        let url = serve_http(200, "{\"ok\":true}").await;
        let client = reqwest::Client::new();
        let resp = get(&client, &url).await.unwrap();
        assert!(resp.status().is_success());

        let url = serve_http(404, "missing").await;
        let err = get(&client, &url).await.unwrap_err().to_string();
        assert!(err.contains("404"), "{err}");
    }
}

#[tokio::test]
async fn stub_commands_web_acp_pr_github() {
    let _home = IsolatedHome::new();
    let _gh = IsolatedGh::new(0);
    let c = cli(None);
    cmd_web().await.unwrap();
    cmd_acp(&c).await.unwrap();
    cmd_pr(&c, None, None).await.unwrap();
    cmd_pr(&c, Some("feat"), Some("dev")).await.unwrap();
    cmd_github(&c, &GithubCmd::Pr { action: None })
        .await
        .unwrap();
    cmd_github(
        &c,
        &GithubCmd::Pr {
            action: Some(PrAction::List),
        },
    )
    .await
    .unwrap();
    cmd_github(
        &c,
        &GithubCmd::Pr {
            action: Some(PrAction::View { number: 1 }),
        },
    )
    .await
    .unwrap();
    cmd_github(
        &c,
        &GithubCmd::Pr {
            action: Some(PrAction::Create {
                title: None,
                base: None,
            }),
        },
    )
    .await
    .unwrap();
    cmd_github(
        &c,
        &GithubCmd::Pr {
            action: Some(PrAction::Create {
                title: Some("t".into()),
                base: Some("main".into()),
            }),
        },
    )
    .await
    .unwrap();
    cmd_github(&c, &GithubCmd::Issue { number: None })
        .await
        .unwrap();
    cmd_github(&c, &GithubCmd::Issue { number: Some(7) })
        .await
        .unwrap();
}

#[tokio::test]
async fn after_tui_exit_quit() {
    after_tui_exit(whycodes_tui::TuiExit::Quit).await.unwrap();
}

#[test]
fn spawn_update_check_disabled_returns_none() {
    let mut c = cli(None);
    c.no_auto_update = true;
    let config = Config::default();
    assert!(spawn_update_check(&c, &config).is_none());
}

#[test]
fn should_auto_update_reads_process_env() {
    let c = cli(None);
    let _ = should_auto_update(&c, true);
    let _ = should_auto_update(&c, false);
}

#[tokio::test]
async fn cmd_debug_and_stats_on_isolated_home() {
    let _home = IsolatedHome::new();
    cmd_debug().await.unwrap();
    cmd_stats().await.unwrap();
}

#[tokio::test]
async fn dispatch_offline_commands() {
    let _home = IsolatedHome::new();
    let mut c = cli(None);
    c.plain = true;
    c.no_auto_update = true;
    c.no_memory = true;

    dispatch_command(&Commands::Web, &c).await.unwrap();
    dispatch_command(&Commands::Acp, &c).await.unwrap();
    dispatch_command(
        &Commands::Pr {
            title: None,
            base: None,
        },
        &c,
    )
    .await
    .unwrap();
    let _gh = IsolatedGh::new(1);
    dispatch_command(
        &Commands::Github {
            cmd: GithubCmd::Issue { number: None },
        },
        &c,
    )
    .await
    .unwrap();
    dispatch_command(&Commands::Debug, &c).await.unwrap();
    dispatch_command(&Commands::Stats, &c).await.unwrap();
    dispatch_command(
        &Commands::Config {
            cmd: ConfigCmd::Path,
        },
        &c,
    )
    .await
    .unwrap();
    dispatch_command(
        &Commands::Config {
            cmd: ConfigCmd::Get {
                key: "default_agent".into(),
            },
        },
        &c,
    )
    .await
    .unwrap();
    dispatch_command(
        &Commands::Config {
            cmd: ConfigCmd::Show,
        },
        &c,
    )
    .await
    .unwrap();
    dispatch_command(&Commands::Mcp { cmd: McpCmd::List }, &c)
        .await
        .unwrap();
    dispatch_command(
        &Commands::Provider {
            cmd: ProviderCmd::List,
        },
        &c,
    )
    .await
    .unwrap();
    dispatch_command(
        &Commands::Model {
            cmd: ModelCmd::List,
        },
        &c,
    )
    .await
    .unwrap();
    dispatch_command(&Commands::Agent { name: None }, &c)
        .await
        .unwrap();
    dispatch_command(&Commands::Plugins { cmd: None }, &c)
        .await
        .unwrap();
    dispatch_command(
        &Commands::Session {
            cmd: SessionCmd::List,
        },
        &c,
    )
    .await
    .unwrap();
    dispatch_command(
        &Commands::Memory {
            cmd: MemoryCmd::List { limit: 5 },
        },
        &c,
    )
    .await
    .unwrap();
    dispatch_command(
        &Commands::Auth {
            cmd: AuthCmd::Status,
        },
        &c,
    )
    .await
    .unwrap();
    dispatch_command(
        &Commands::Auth {
            cmd: AuthCmd::Logout {
                provider: "nope".into(),
            },
        },
        &c,
    )
    .await
    .unwrap();
    dispatch_command(
        &Commands::Auth {
            cmd: AuthCmd::Import,
        },
        &c,
    )
    .await
    .unwrap();
    dispatch_command(
        &Commands::Completions {
            shell: clap_complete::Shell::Bash,
        },
        &c,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn cmd_run_empty_prompt_and_missing_key() {
    let _home = IsolatedHome::new();
    let mut c = cli(None);
    c.plain = true;
    c.no_auto_update = true;
    c.no_memory = true;
    cmd_run(&c, Some(""), None, OutputFormat::Text)
        .await
        .unwrap();
    cmd_run(&c, Some("hello"), Some(1), OutputFormat::Text)
        .await
        .unwrap();
    let err = cmd_run(&c, None, None, OutputFormat::Json).await;
    assert!(err.is_err(), "{err:?}");
}

#[tokio::test]
async fn cmd_connect_unreachable_host() {
    let c = cli(None);
    let err = cmd_connect(&c, "127.0.0.1:1", None).await;
    assert!(err.is_err(), "{err:?}");
}

#[tokio::test]
async fn cmd_generate_empty_prompt_errors() {
    let _home = IsolatedHome::new();
    let mut c = cli(None);
    c.plain = true;
    c.no_memory = true;
    let err = cmd_generate(&c, &[String::new()], Some(1), 1, OutputFormat::Text).await;
    assert!(err.is_err(), "{err:?}");
}

#[tokio::test]
async fn ensure_api_key_false_without_credentials() {
    let _home = IsolatedHome::new();
    let config = Config::default();
    let mut key = String::new();
    assert!(!ensure_api_key(&mut key, "anthropic", &config).await);
    key = "sk-test".into();
    assert!(ensure_api_key(&mut key, "anthropic", &config).await);
    print_slash_help();
}

#[test]
fn refresh_session_memory_and_open_memory_service() {
    let home = IsolatedHome::new();
    let dir = home.path();
    let config = Config::default();
    let mut session = whycodes_session::session::Session::new(dir.to_path_buf(), "sys".into());
    let agent = Agent::new(agent_info_for(&cli(None), &config));
    refresh_session_memory(&mut session, &agent, dir, &config, Some("query"));
    let mut c = cli(None);
    c.dir = Some(dir.to_string_lossy().into_owned());
    open_memory_service(&c, &config).unwrap();
    maybe_session_auto_index(dir, &config);
    load_auth_plugins(&c);
    let _ = oauth_provider_list();
    resume_session_into(&mut session, "no-such-session").unwrap();
}

#[tokio::test]
async fn get_api_key_env_wins() {
    let _home = IsolatedHome::new();
    let prev = std::env::var_os("ANTHROPIC_API_KEY");
    unsafe { std::env::set_var("ANTHROPIC_API_KEY", "sk-from-env") };
    let config = Config::default();
    let key = get_api_key("anthropic", &config).await;
    match prev {
        Some(v) => unsafe { std::env::set_var("ANTHROPIC_API_KEY", v) },
        None => unsafe { std::env::remove_var("ANTHROPIC_API_KEY") },
    }
    assert_eq!(key.as_deref(), Some("sk-from-env"));
}

#[tokio::test]
async fn memory_index_and_empty_add_on_isolated_home() {
    let home = IsolatedHome::new();
    let mut c = cli(None);
    c.dir = Some(home.path().to_string_lossy().into_owned());
    cmd_memory(
        &c,
        &MemoryCmd::Index {
            max_files: 2,
            max_chunks: 4,
        },
    )
    .await
    .unwrap();
    cmd_memory(
        &c,
        &MemoryCmd::SessionSearch {
            query: "nope".into(),
            limit: 3,
        },
    )
    .await
    .unwrap();
    let empty = cmd_memory(&c, &MemoryCmd::Add { text: vec![] }).await;
    assert!(empty.is_err());
    cmd_memory(
        &c,
        &MemoryCmd::Delete {
            id: "missing".into(),
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn provider_add_headers_and_model_default() {
    let _home = IsolatedHome::new();
    cmd_provider(&ProviderCmd::Add {
        name: "local".into(),
        api_key: Some("k".into()),
        base_url: Some("http://127.0.0.1:9".into()),
        headers: Some("X-A=1,X-B=2".into()),
    })
    .await
    .unwrap();
    cmd_provider(&ProviderCmd::Add {
        name: "local".into(),
        api_key: Some("k2".into()),
        base_url: Some("http://127.0.0.1:9".into()),
        headers: None,
    })
    .await
    .unwrap();
    cmd_provider(&ProviderCmd::List).await.unwrap();
    cmd_model(&ModelCmd::Default {
        provider: "local".into(),
        model: "m".into(),
    })
    .await
    .unwrap();
}

#[test]
fn init_logging_from_cli_debug() {
    let _home = IsolatedHome::new();
    let mut c = cli(None);
    c.debug = true;
    init_logging(&c);
    ignore_sigpipe();
}

#[tokio::test]
async fn cmd_auth_unknown_provider_and_status() {
    let _home = IsolatedHome::new();
    let err = cmd_auth(&AuthCmd::Login {
        provider: "not-a-provider".into(),
        no_browser: true,
    })
    .await;
    assert!(err.is_err(), "{err:?}");
    cmd_auth(&AuthCmd::Status).await.unwrap();
    cmd_auth_import(&Config::data_dir().unwrap()).await.unwrap();
}

#[tokio::test]
async fn mcp_add_list_remove_transports() {
    let _home = IsolatedHome::new();
    cmd_mcp(&McpCmd::Add {
        name: "local".into(),
        command: Some("npx".into()),
        args: Some("-y demo".into()),
        url: None,
        transport: Some("stdio".into()),
        headers: vec![],
    })
    .await
    .unwrap();
    cmd_mcp(&McpCmd::Add {
        name: "remote".into(),
        command: None,
        args: None,
        url: Some("https://example.com/mcp".into()),
        transport: Some("http".into()),
        headers: vec!["Authorization: Bearer x".into()],
    })
    .await
    .unwrap();
    cmd_mcp(&McpCmd::List).await.unwrap();
    let bad = cmd_mcp(&McpCmd::Add {
        name: "bad".into(),
        command: None,
        args: None,
        url: None,
        transport: Some("ftp".into()),
        headers: vec![],
    })
    .await;
    assert!(bad.is_err());
    cmd_mcp(&McpCmd::Remove {
        name: "local".into(),
    })
    .await
    .unwrap();
    cmd_mcp(&McpCmd::Remove {
        name: "missing".into(),
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn session_missing_ids_and_import_share() {
    let home = IsolatedHome::new();
    cmd_session(&SessionCmd::Delete { id: "nope".into() })
        .await
        .unwrap();
    cmd_session(&SessionCmd::Rename {
        id: "nope".into(),
        name: "x".into(),
    })
    .await
    .unwrap();
    cmd_session(&SessionCmd::Share { id: "nope".into() })
        .await
        .unwrap();
    cmd_session(&SessionCmd::View { id: "nope".into() })
        .await
        .unwrap();

    let import_path = home.path().join("chat.json");
    std::fs::write(
        &import_path,
        r#"[{"role":"user","content":"hi"},{"role":"assistant","content":"yo"}]"#,
    )
    .unwrap();
    cmd_session(&SessionCmd::Import {
        path: import_path.clone(),
        from: "json".into(),
    })
    .await
    .unwrap();
    cmd_session(&SessionCmd::List).await.unwrap();
    let db = open_db().unwrap();
    let sessions = db.list_sessions().unwrap();
    assert!(!sessions.is_empty());
    let id = sessions[0].id.clone();
    cmd_session(&SessionCmd::View { id: id.clone() })
        .await
        .unwrap();
    cmd_session(&SessionCmd::Rename {
        id: id.clone(),
        name: "imported".into(),
    })
    .await
    .unwrap();
    cmd_session(&SessionCmd::Share { id: id.clone() })
        .await
        .unwrap();
    cmd_session(&SessionCmd::Delete { id }).await.unwrap();
}

#[tokio::test]
async fn cmd_serve_bind_privileged_port_fails() {
    let _home = IsolatedHome::new();
    let err = cmd_serve(1).await;
    assert!(err.is_err(), "{err:?}");
}

#[tokio::test]
async fn dispatch_generate_empty_prompt() {
    let _home = IsolatedHome::new();
    let mut c = cli(None);
    c.plain = true;
    c.no_memory = true;
    let err = dispatch_command(
        &Commands::Generate {
            prompt: vec![String::new()],
            max_turns: Some(1),
            jobs: 1,
            format: OutputFormat::Text,
        },
        &c,
    )
    .await;
    assert!(err.is_err(), "{err:?}");
}

#[tokio::test]
async fn cmd_generate_ollama_reaches_llm_and_fails() {
    let _home = IsolatedHome::new();
    let mut c = cli(None);
    c.provider = Some("ollama".into());
    c.model = Some("tiny".into());
    c.plain = true;
    c.no_memory = true;
    c.no_auto_update = true;
    let err = cmd_generate(&c, &["say hi".into()], Some(1), 1, OutputFormat::Json).await;
    assert!(err.is_err(), "{err:?}");
    let err = cmd_generate(
        &c,
        &["one".into(), "two".into()],
        Some(1),
        2,
        OutputFormat::Text,
    )
    .await;
    assert!(err.is_err(), "{err:?}");
    let err = cmd_run(&c, Some("say hi"), Some(1), OutputFormat::Json).await;
    assert!(err.is_err(), "{err:?}");
}

#[tokio::test]
async fn cmd_serve_binds_then_abort() {
    let _home = IsolatedHome::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let handle = tokio::spawn(async move { cmd_serve(port).await });
    let url = format!("http://127.0.0.1:{port}/api/health");
    let mut ok = false;
    for _ in 0..80 {
        if reqwest::get(&url).await.is_ok() {
            ok = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    handle.abort();
    let _ = handle.await;
    assert!(ok, "serve never became healthy on {url}");
}

#[tokio::test]
async fn dispatch_serve_and_connect_error_arms() {
    let _home = IsolatedHome::new();
    let mut c = cli(None);
    c.plain = true;
    c.no_memory = true;
    let err = dispatch_command(&Commands::Serve { port: 1 }, &c).await;
    assert!(err.is_err(), "{err:?}");
    let err = dispatch_command(
        &Commands::Connect {
            addr: "127.0.0.1:1".into(),
            session: Some("s1".into()),
        },
        &c,
    )
    .await;
    assert!(err.is_err(), "{err:?}");
}

#[tokio::test]
async fn github_commands_with_failing_gh() {
    let _home = IsolatedHome::new();
    let _gh = IsolatedGh::new(1);
    let c = cli(None);
    cmd_github(
        &c,
        &GithubCmd::Pr {
            action: Some(PrAction::Create {
                title: Some("t".into()),
                base: Some("main".into()),
            }),
        },
    )
    .await
    .unwrap();
    cmd_github(&c, &GithubCmd::Issue { number: Some(3) })
        .await
        .unwrap();
    cmd_github(&c, &GithubCmd::Issue { number: None })
        .await
        .unwrap();
}

#[test]
fn init_logging_skips_config_under_bench() {
    let _home = IsolatedHome::new();
    let prev = std::env::var_os("WHYCODES_BENCH");
    unsafe { std::env::set_var("WHYCODES_BENCH", "1") };
    init_logging(&cli(None));
    match prev {
        Some(v) => unsafe { std::env::set_var("WHYCODES_BENCH", v) },
        None => unsafe { std::env::remove_var("WHYCODES_BENCH") },
    }
}

#[tokio::test]
async fn memory_add_path_export_import_clear() {
    let home = IsolatedHome::new();
    let mut c = cli(None);
    c.dir = Some(home.path().to_string_lossy().into_owned());
    cmd_memory(
        &c,
        &MemoryCmd::Add {
            text: vec!["remember this fact".into()],
        },
    )
    .await
    .unwrap();
    cmd_memory(&c, &MemoryCmd::Path).await.unwrap();
    cmd_memory(&c, &MemoryCmd::List { limit: 10 })
        .await
        .unwrap();
    cmd_memory(
        &c,
        &MemoryCmd::Search {
            query: "fact".into(),
            limit: 5,
        },
    )
    .await
    .unwrap();
    let out = home.path().join("mem.json");
    cmd_memory(
        &c,
        &MemoryCmd::Export {
            output: Some(out.clone()),
        },
    )
    .await
    .unwrap();
    cmd_memory(&c, &MemoryCmd::Export { output: None })
        .await
        .unwrap();
    cmd_memory(&c, &MemoryCmd::Import { path: out })
        .await
        .unwrap();
    cmd_memory(
        &c,
        &MemoryCmd::CodeSearch {
            query: "fn".into(),
            limit: 3,
        },
    )
    .await
    .unwrap();
    cmd_memory(&c, &MemoryCmd::Clear).await.unwrap();
}

#[tokio::test]
async fn provider_remove_default_and_agent_missing() {
    let _home = IsolatedHome::new();
    cmd_provider(&ProviderCmd::Add {
        name: "tmp".into(),
        api_key: None,
        base_url: Some("http://127.0.0.1:9".into()),
        headers: Some("badpair".into()),
    })
    .await
    .unwrap();
    cmd_provider(&ProviderCmd::Default { name: "tmp".into() })
        .await
        .unwrap();
    cmd_provider(&ProviderCmd::Default {
        name: "nope".into(),
    })
    .await
    .unwrap();
    cmd_provider(&ProviderCmd::Remove { name: "tmp".into() })
        .await
        .unwrap();
    cmd_provider(&ProviderCmd::Remove {
        name: "nope".into(),
    })
    .await
    .unwrap();
    cmd_agent(Some("no-such-agent")).await.unwrap();
    cmd_agent(None).await.unwrap();
    cmd_plugins(&cli(None), Some(&PluginsCmd::List))
        .await
        .unwrap();
}

#[tokio::test]
async fn cmd_run_tui_stub_quit() {
    let _home = IsolatedHome::new();
    unsafe { std::env::set_var("WHYCODES_TEST_TUI", "quit") };
    let mut c = cli(None);
    c.plain = false;
    c.no_memory = true;
    c.no_auto_update = true;
    let result = cmd_run(&c, None, None, OutputFormat::Text).await;
    unsafe { std::env::remove_var("WHYCODES_TEST_TUI") };
    result.unwrap();
}

#[tokio::test]
async fn cmd_run_tui_stub_upgrade_skips_network() {
    let _home = IsolatedHome::new();
    unsafe {
        std::env::set_var("WHYCODES_TEST_TUI", "upgrade");
        std::env::set_var("WHYCODES_TEST_SKIP_UPGRADE", "1");
    }
    let mut c = cli(None);
    c.plain = false;
    c.no_memory = true;
    after_tui_exit(whycodes_tui::TuiExit::Upgrade)
        .await
        .unwrap();
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::remove_var("WHYCODES_TEST_SKIP_UPGRADE");
    }
}

#[tokio::test]
async fn cmd_upgrade_uses_mock_latest_release() {
    let _home = IsolatedHome::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 1024];
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let n = stream.peek(&mut buf).await.unwrap_or(0);
        let _ = n;
        let mut stream = stream;
        let _ = stream.read(&mut buf).await;
        let body = br#"{"tag_name":"v0.0.0","assets":[]}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            std::str::from_utf8(body).unwrap()
        );
        let _ = stream.write_all(resp.as_bytes()).await;
    });
    let url = format!("http://127.0.0.1:{}/releases/latest", addr.port());
    unsafe { std::env::set_var("WHYCODES_UPGRADE_LATEST_URL", &url) };
    cmd_upgrade().await.unwrap();
    unsafe { std::env::remove_var("WHYCODES_UPGRADE_LATEST_URL") };
    let _ = server.await;
}

#[tokio::test]
async fn cmd_connect_health_ok_with_tui_stub() {
    let _home = IsolatedHome::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for _ in 0..8 {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let mut buf = vec![0u8; 2048];
            let n = stream.read(&mut buf).await.unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let body: &[u8] = if req.contains("/api/health") {
                br#"{"ok":true,"version":"0.3.0"}"#
            } else if req.contains("/api/session/new") {
                br#"{"session_id":"sess-test"}"#
            } else {
                br#"{"ok":true}"#
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                std::str::from_utf8(body).unwrap()
            );
            let _ = stream.write_all(resp.as_bytes()).await;
        }
    });
    unsafe { std::env::set_var("WHYCODES_TEST_TUI", "quit") };
    let c = cli(None);
    let result = cmd_connect(&c, &format!("127.0.0.1:{}", addr.port()), None).await;
    unsafe { std::env::remove_var("WHYCODES_TEST_TUI") };
    server.abort();
    result.unwrap();
}

#[tokio::test]
async fn cmd_auth_logout_status_and_oauth_error() {
    let home = IsolatedHome::new();
    let store = whycodes_auth::TokenStore::new(home.path());
    store
        .set(
            "openai",
            whycodes_auth::ProviderAuth {
                method: "oauth".into(),
                token: whycodes_auth::OAuthToken {
                    access_token: "tok".into(),
                    refresh_token: None,
                    expires_at: None,
                    extra: Default::default(),
                },
            },
        )
        .unwrap();
    cmd_auth(&AuthCmd::Status).await.unwrap();
    cmd_auth(&AuthCmd::Logout {
        provider: "openai".into(),
    })
    .await
    .unwrap();
    cmd_auth(&AuthCmd::Logout {
        provider: "missing".into(),
    })
    .await
    .unwrap();
    let err = cmd_auth(&AuthCmd::Login {
        provider: "openai".into(),
        no_browser: true,
    })
    .await;
    assert!(err.is_err(), "{err:?}");
}

#[tokio::test]
async fn cmd_run_force_plain_fallback_message() {
    let _home = IsolatedHome::new();
    let mut c = cli(None);
    c.plain = false;
    c.no_memory = true;
    // Without WHYCODES_TEST_TUI and without a TTY this hits the fallback eprintln.
    let err = cmd_run(&c, Some("hello"), Some(1), OutputFormat::Text).await;
    let _ = err;
}

#[tokio::test]
async fn dispatch_upgrade_and_web() {
    let _home = IsolatedHome::new();
    unsafe { std::env::set_var("WHYCODES_TEST_SKIP_UPGRADE", "1") };
    let c = cli(None);
    let _ = dispatch_command(&Commands::Upgrade, &c).await;
    dispatch_command(&Commands::Web, &c).await.unwrap();
    dispatch_command(&Commands::Acp, &c).await.unwrap();
    unsafe { std::env::remove_var("WHYCODES_TEST_SKIP_UPGRADE") };
}

#[test]
fn latest_release_url_env_override() {
    let _home = IsolatedHome::new();
    unsafe { std::env::set_var("WHYCODES_UPGRADE_LATEST_URL", "http://127.0.0.1:9/latest") };
    assert!(crate::upgrade::latest_release_url().contains("127.0.0.1"));
    unsafe { std::env::remove_var("WHYCODES_UPGRADE_LATEST_URL") };
    assert!(crate::upgrade::latest_release_url().contains("api.github.com"));
}

#[tokio::test]
async fn github_commands_with_ok_gh() {
    let _home = IsolatedHome::new();
    let _gh = IsolatedGh::new(0);
    let c = cli(None);
    cmd_pr(&c, Some("t"), Some("main")).await.unwrap();
    cmd_github(
        &c,
        &GithubCmd::Pr {
            action: Some(PrAction::Create {
                title: Some("t".into()),
                base: Some("main".into()),
            }),
        },
    )
    .await
    .unwrap();
    cmd_github(&c, &GithubCmd::Issue { number: Some(3) })
        .await
        .unwrap();
    cmd_github(&c, &GithubCmd::Issue { number: None })
        .await
        .unwrap();
}

#[tokio::test]
async fn cmd_session_list_when_empty() {
    let _home = IsolatedHome::new();
    cmd_session(&SessionCmd::List).await.unwrap();
}

#[tokio::test]
async fn spawn_update_check_skips_when_flagged() {
    let _home = IsolatedHome::new();
    unsafe { std::env::set_var("WHYCODES_TEST_SKIP_UPGRADE", "1") };
    let mut c = cli(None);
    c.no_auto_update = false;
    let cfg = Config::default();
    let rx = crate::cmd::debug::spawn_update_check(&c, &cfg);
    unsafe { std::env::remove_var("WHYCODES_TEST_SKIP_UPGRADE") };
    drop(rx);
}

#[tokio::test]
async fn cmd_run_plain_repl_slash_commands() {
    let home = IsolatedHome::new();
    let _llm = TestLlmEnv;
    unsafe {
        std::env::set_var("WHYCODES_TEST_LLM", "scripted-ok");
        std::env::set_var("WHYCODES_TEST_SKIP_UPGRADE", "1");
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test-repl");
    }
    let proj = home.path().join("proj");
    std::fs::create_dir_all(proj.join(".whycodes").join("commands")).unwrap();
    std::fs::write(proj.join("note.txt"), "hello file").unwrap();
    std::fs::write(
        proj.join(".whycodes").join("commands").join("hello.md"),
        "Say hello $ARGUMENTS",
    )
    .unwrap();

    install_test_repl_lines([
        "",
        "   ",
        "!",
        "!echo hello-shell",
        "/help",
        "/h",
        "/rename",
        "/rename coverage-session",
        "/info",
        "/details",
        "/new",
        "/clear",
        "/undo",
        "/redo",
        "/share",
        "/export",
        "/fresh",
        "/compact",
        "/summarize",
        "/diff",
        "/cost",
        "/usage",
        "/context",
        "/doctor",
        "/sessions",
        "/resume",
        "/continue",
        "/models",
        "/models ollama/tiny",
        "/models tiny-only",
        "/effort",
        "/effort high",
        "/effort nope",
        "/agent",
        "/agent plan",
        "/agent nope",
        "/connect",
        "/login",
        "/login nope",
        "/thinking",
        "/thinking",
        "/themes",
        "/tools",
        "/remember",
        "/remember keep this",
        "/memory",
        "/not-a-command",
        "/hello coverage",
        "read @note.txt please",
        "/compact extra note",
        "/review",
        "/security-review",
        "/commit",
        "/quit",
    ]);

    let mut c = cli(None);
    c.plain = true;
    c.no_memory = false;
    c.no_auto_update = true;
    c.dir = Some(proj.to_string_lossy().into_owned());
    c.provider = Some("anthropic".into());
    c.model = Some("claude-test".into());
    c.resume = Some("missing-session".into());
    let result = cmd_run(&c, None, Some(3), OutputFormat::Text).await;
    clear_test_repl_lines();
    unsafe {
        std::env::remove_var("WHYCODES_TEST_LLM");
        std::env::remove_var("WHYCODES_TEST_SKIP_UPGRADE");
        std::env::remove_var("ANTHROPIC_API_KEY");
    }
    result.unwrap();
}

#[tokio::test]
async fn cmd_run_plain_repl_eof_without_exit() {
    let _home = IsolatedHome::new();
    let _llm = TestLlmEnv;
    unsafe {
        std::env::set_var("WHYCODES_TEST_LLM", "ok");
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test-eof");
    }
    install_test_repl_lines(["/help"]);
    let mut c = cli(None);
    c.plain = true;
    c.no_memory = true;
    c.no_auto_update = true;
    c.provider = Some("anthropic".into());
    let result = cmd_run(&c, None, None, OutputFormat::Text).await;
    clear_test_repl_lines();
    unsafe {
        std::env::remove_var("WHYCODES_TEST_LLM");
        std::env::remove_var("ANTHROPIC_API_KEY");
    }
    result.unwrap();
}

#[tokio::test]
async fn cmd_run_one_shot_with_scripted_llm() {
    let home = IsolatedHome::new();
    let _llm = TestLlmEnv;
    unsafe {
        std::env::set_var("WHYCODES_TEST_LLM", "one-shot-ok");
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test-oneshot");
    }
    let mut c = cli(None);
    c.plain = true;
    c.no_memory = true;
    c.no_auto_update = true;
    c.dir = Some(home.path().to_string_lossy().into_owned());
    c.provider = Some("anthropic".into());
    let result = cmd_run(&c, Some("hello world"), Some(2), OutputFormat::Text).await;
    unsafe {
        std::env::remove_var("WHYCODES_TEST_LLM");
        std::env::remove_var("ANTHROPIC_API_KEY");
    }
    result.unwrap();
}

#[tokio::test]
async fn cmd_generate_with_scripted_llm() {
    let home = IsolatedHome::new();
    let _llm = TestLlmEnv;
    unsafe {
        std::env::set_var("WHYCODES_TEST_LLM", "gen-ok");
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test-gen");
    }
    let mut c = cli(None);
    c.plain = true;
    c.no_memory = true;
    c.no_auto_update = true;
    c.dir = Some(home.path().to_string_lossy().into_owned());
    c.provider = Some("anthropic".into());
    cmd_generate(&c, &["one".into()], Some(1), 1, OutputFormat::Text)
        .await
        .unwrap();
    cmd_generate(
        &c,
        &["a".into(), "b".into()],
        Some(1),
        2,
        OutputFormat::Json,
    )
    .await
    .unwrap();
    unsafe {
        std::env::remove_var("WHYCODES_TEST_LLM");
        std::env::remove_var("ANTHROPIC_API_KEY");
    }
}

#[tokio::test]
async fn cmd_run_plain_continue_empty_and_init_login() {
    let home = IsolatedHome::new();
    let _llm = TestLlmEnv;
    unsafe {
        std::env::set_var("WHYCODES_TEST_LLM", "init-ok");
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test-init");
    }
    let proj = home.path().join("p2");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
    install_test_repl_lines(["/init", "/login nope", "/q"]);
    let mut c = cli(None);
    c.plain = true;
    c.no_memory = true;
    c.no_auto_update = true;
    c.continue_session = true;
    c.dir = Some(proj.to_string_lossy().into_owned());
    c.provider = Some("anthropic".into());
    let result = cmd_run(&c, None, None, OutputFormat::Text).await;
    clear_test_repl_lines();
    unsafe {
        std::env::remove_var("WHYCODES_TEST_LLM");
        std::env::remove_var("ANTHROPIC_API_KEY");
    }
    result.unwrap();
}

#[test]
fn map_tui_run_error_rewrites_enxio() {
    let mapped = map_tui_run_error(anyhow::anyhow!("No such device (os error 6)"));
    let msg = mapped.to_string();
    assert!(msg.contains("TUI needs a real terminal"), "{msg}");
    let mapped = map_tui_run_error(anyhow::anyhow!("not a terminal"));
    assert!(mapped.to_string().contains("TUI needs a real terminal"));
    let other = map_tui_run_error(anyhow::anyhow!("boom"));
    assert_eq!(other.to_string(), "boom");
}

#[tokio::test]
async fn cmd_auth_import_approves_claude_code_from_home() {
    let home = IsolatedHome::new();
    let creds = home.path().join(".claude");
    std::fs::create_dir_all(&creds).unwrap();
    std::fs::write(
        creds.join(".credentials.json"),
        r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat-test","refreshToken":"r","expiresAt":4102444800000}}"#,
    )
    .unwrap();
    install_test_repl_lines(["y"]);
    cmd_auth_import(&Config::data_dir().unwrap()).await.unwrap();
    clear_test_repl_lines();
    // Denied path
    let gemini = home.path().join(".gemini");
    std::fs::create_dir_all(&gemini).unwrap();
    std::fs::write(gemini.join("oauth_creds.json"), r#"{"token":"x"}"#).unwrap();
    install_test_repl_lines(["n"]);
    cmd_auth_import(&Config::data_dir().unwrap()).await.unwrap();
    clear_test_repl_lines();
}

#[tokio::test]
async fn cmd_run_tui_path_hits_whycodes_tui_run() {
    let _home = IsolatedHome::new();
    let _llm = TestLlmEnv;
    unsafe {
        std::env::set_var("WHYCODES_TEST_TUI", "quit");
        std::env::set_var("WHYCODES_TEST_SKIP_UPGRADE", "1");
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test-tui");
    }
    let mut c = cli(None);
    c.plain = false;
    c.no_memory = true;
    c.no_auto_update = true;
    cmd_run(&c, None, None, OutputFormat::Text).await.unwrap();
    unsafe {
        std::env::remove_var("WHYCODES_TEST_TUI");
        std::env::remove_var("WHYCODES_TEST_SKIP_UPGRADE");
        std::env::remove_var("ANTHROPIC_API_KEY");
    }
}

#[tokio::test]
async fn cmd_upgrade_downloads_and_replaces_target() {
    let home = IsolatedHome::new();
    let target = home.path().join("whycodes-bin");
    std::fs::write(&target, b"old-binary").unwrap();

    let tar_bytes = {
        let mut raw = Vec::new();
        {
            let mut b = tar::Builder::new(&mut raw);
            let mut h = tar::Header::new_gnu();
            h.set_size(3);
            h.set_cksum();
            b.append_data(&mut h, "whycodes", &b"new"[..]).unwrap();
            b.finish().unwrap();
        }
        let mut gz = Vec::new();
        let mut enc = flate2::write::GzEncoder::new(&mut gz, flate2::Compression::default());
        use std::io::Write;
        enc.write_all(&raw).unwrap();
        enc.finish().unwrap();
        gz
    };
    let digest = crate::upgrade::digest_of(&tar_bytes);
    let archive_name = crate::upgrade::target_archive().unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let latest_url = format!("http://{addr}/latest");
    let asset_url = format!("http://{addr}/asset");
    let sums_url = format!("http://{addr}/sums");
    let tar_clone = tar_bytes.clone();
    let sums_body = format!("{digest}  {archive_name}\n");
    let latest_body = serde_json::json!({
        "tag_name": "v99.0.0",
        "assets": [
            {"name": archive_name, "id": 1, "browser_download_url": asset_url},
            {"name": "SHA256SUMS", "id": 2, "browser_download_url": sums_url},
        ]
    })
    .to_string();
    let server = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let mut buf = vec![0u8; 4096];
            let n = stream.read(&mut buf).await.unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let (status, body, ctype): (&str, Vec<u8>, &str) = if req.contains("GET /latest") {
                (
                    "200 OK",
                    latest_body.as_bytes().to_vec(),
                    "application/json",
                )
            } else if req.contains("GET /sums") {
                ("200 OK", sums_body.as_bytes().to_vec(), "text/plain")
            } else if req.contains("GET /asset") {
                ("200 OK", tar_clone.clone(), "application/octet-stream")
            } else {
                ("404 Not Found", b"nope".to_vec(), "text/plain")
            };
            let resp = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes()).await;
            let _ = stream.write_all(&body).await;
            let _ = stream.shutdown().await;
        }
    });

    unsafe {
        std::env::set_var("WHYCODES_UPGRADE_LATEST_URL", &latest_url);
        std::env::set_var("WHYCODES_UPGRADE_TARGET", &target);
    }
    let result =
        tokio::time::timeout(std::time::Duration::from_secs(8), crate::upgrade::run()).await;
    unsafe {
        std::env::remove_var("WHYCODES_UPGRADE_LATEST_URL");
        std::env::remove_var("WHYCODES_UPGRADE_TARGET");
    }
    server.abort();
    let result = result.expect("upgrade::run timed out").unwrap();
    assert_eq!(result.as_deref(), Some("99.0.0"));
    assert_eq!(std::fs::read(&target).unwrap(), b"new");
}
