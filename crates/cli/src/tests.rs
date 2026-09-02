//! Unit tests. Sibling file so llvm-cov can ignore tests.rs.

use super::*;
use crate::cmd::auth::auth_expiry_label;
use crate::cmd::config::{get_config_value, set_config_value};
use crate::cmd::debug::should_auto_update_with_env;
use whycodes_agent::agent::Agent;
use whycodes_core::types::{ModelConfig, ProviderConfig};
use whycodes_protocol::{CiEvent, ResultMeta};

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
