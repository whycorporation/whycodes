//! Unit tests. Sibling file so llvm-cov can ignore tests.rs.

use super::*;

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
        Some(whycode_tui::RESUME_LATEST.to_string())
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
    assert!(command_needs_multi_thread(&cli(None)));
    assert!(command_needs_multi_thread(&cli(Some(Commands::Run {
        prompt: None,
        max_turns: 25,
        format: OutputFormat::Text,
    }))));
    assert!(command_needs_multi_thread(&cli(Some(Commands::Mcp {
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
    use whycode_agent::events::TurnEvent as TE;

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

    let usage = turn_event_to_ci(TE::Usage(whycode_core::types::Usage {
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
    let panel = turn_event_to_ci(TE::Panel(whycode_core::PanelUpdate::File {
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
    use whycode_auth::token::ProviderAuth;
    let auth = |token: whycode_auth::token::OAuthToken| ProviderAuth {
        method: "oauth".into(),
        token,
    };

    // no expiry
    let none = auth(whycode_auth::token::OAuthToken {
        access_token: "a".into(),
        refresh_token: None,
        expires_at: None,
        extra: Default::default(),
    });
    assert_eq!(auth_expiry_label(&none), "no expiry");

    // future expiry → "expires <at>"
    let future = chrono::Utc::now() + chrono::Duration::days(30);
    let tok = whycode_auth::token::OAuthToken {
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
    let tok = whycode_auth::token::OAuthToken {
        access_token: "a".into(),
        refresh_token: None,
        expires_at: Some(past),
        extra: Default::default(),
    };
    let label = auth_expiry_label(&auth(tok));
    assert!(label.starts_with("expired "), "{label}");
    assert!(label.contains("refreshes on next use"), "{label}");

    // derived expiry wins over access-token expiry
    let tok = whycode_auth::token::OAuthToken {
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
    let tok = whycode_auth::token::OAuthToken {
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
        max_turns: 25,
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
fn runtime_choice_covers_remaining_commands() {
    assert!(command_needs_multi_thread(&cli(Some(Commands::Generate {
        prompt: vec!["x".into()],
        max_turns: 1,
        jobs: 1,
        format: OutputFormat::Text,
    }))));
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
}

#[test]
fn turn_event_to_ci_remaining_variants() {
    use whycode_agent::events::TurnEvent as TE;
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
        turn_event_to_ci(TE::Panel(whycode_core::PanelUpdate::Clear)),
        Some(CiEvent::Status {
            message: "panel clear".into()
        })
    );
    assert_eq!(
        turn_event_to_ci(TE::Panel(whycode_core::PanelUpdate::Diff {
            path: "d.rs".into(),
            unified: String::new(),
        })),
        Some(CiEvent::Status {
            message: "panel diff=d.rs".into()
        })
    );
    assert_eq!(
        turn_event_to_ci(TE::Panel(whycode_core::PanelUpdate::Mermaid {
            source: "graph TD".into(),
        })),
        Some(CiEvent::Status {
            message: "panel mermaid".into()
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

#[cfg(feature = "self-update")]
mod upgrade_helpers {
    use crate::upgrade::*;
    use std::io::Write;

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
            "assets": [{"name": "whycode.tgz", "id": 9}]
        });
        assert_eq!(release_version(&body).unwrap(), "1.2.3");
        assert_eq!(find_asset_id(&body, "whycode.tgz").unwrap(), 9);
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
                b.append_data(&mut h, "whycode", &b"bin"[..]).unwrap();
                b.finish().unwrap();
            }
            let mut gz = Vec::new();
            let mut enc = flate2::write::GzEncoder::new(&mut gz, flate2::Compression::default());
            enc.write_all(&raw).unwrap();
            enc.finish().unwrap();
            gz
        };
        assert_eq!(extract(&tar_bytes, "whycode.tar.gz").unwrap(), b"bin");
        assert!(extract(b"not-a-tar", "whycode.tar.gz").is_err());

        let zip_bytes = {
            let mut buf = Vec::new();
            let cursor = std::io::Cursor::new(&mut buf);
            let mut zip = zip::ZipWriter::new(cursor);
            zip.start_file("whycode.exe", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"exe").unwrap();
            zip.finish().unwrap();
            buf
        };
        assert_eq!(extract(&zip_bytes, "whycode.zip").unwrap(), b"exe");
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
        assert!(extract(&empty_zip, "whycode.zip").is_err());
        assert!(extract(b"nope", "whycode.zip").is_err());
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
                    "whycode.tar.gz"
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
                "abc123  whycode-x86_64-unknown-linux-gnu.tar.gz\ndef456  whycode-x86_64-pc-windows-msvc.zip\n",
                "whycode-x86_64-pc-windows-msvc.zip"
            ),
            Some("def456".into())
        );
        assert_eq!(expected_digest("abc", "x"), None);
        assert_eq!(
            expected_digest("abc123 *whycode.tar.gz", "whycode.tar.gz"),
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
        let target = dir.path().join("whycode");
        replace_binary(&target, b"fresh").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"fresh");
        replace_binary(&target, b"new").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert!(!dir.path().join(".whycode.new").exists());
        assert!(!dir.path().join(".whycode.old").exists());
    }
}
