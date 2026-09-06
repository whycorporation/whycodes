use super::*;
use crate::args::Cli;
use clap::Parser;
use whycodes_config::Config;

#[test]
fn resolve_dir_falls_back_to_cwd() {
    let cli = Cli::try_parse_from(["whycodes"]).unwrap();
    let dir = resolve_dir(&cli);
    assert!(dir.is_absolute() || dir.as_os_str() == ".");
    let cfg = Config::default();
    assert!(!resolve_provider(&cli, &cfg).is_empty());
    assert!(!resolve_model(&cli, &cfg).is_empty());
    assert!(!resolve_agent(&cli, &cfg).is_empty());
}

#[test]
fn turn_event_to_ci_covers_remaining_arms() {
    use whycodes_core::types::Usage;
    assert!(matches!(
        turn_event_to_ci(TurnEvent::TextDelta("x".into())),
        Some(CiEvent::TextDelta { .. })
    ));
    assert!(matches!(
        turn_event_to_ci(TurnEvent::ThinkingDelta("t".into())),
        Some(CiEvent::ThinkingDelta { .. })
    ));
    assert!(matches!(
        turn_event_to_ci(TurnEvent::ToolStart {
            id: "1".into(),
            name: "read".into(),
            input: serde_json::json!({}),
        }),
        Some(CiEvent::ToolStart { .. })
    ));
    assert!(matches!(
        turn_event_to_ci(TurnEvent::ToolEnd {
            id: "1".into(),
            content: "ok".into(),
            is_error: false,
        }),
        Some(CiEvent::ToolEnd { .. })
    ));
    assert!(matches!(
        turn_event_to_ci(TurnEvent::Usage(Usage::default())),
        Some(CiEvent::Usage { .. })
    ));
    assert!(matches!(
        turn_event_to_ci(TurnEvent::Status("s".into())),
        Some(CiEvent::Status { .. })
    ));
    let intent = turn_event_to_ci(TurnEvent::Intent {
        kind: "change".into(),
        confidence: 0.9,
        badge: "C".into(),
        notice_kind: "info".into(),
        notice: "n".into(),
    })
    .unwrap();
    if let CiEvent::Status { message } = intent {
        assert!(message.contains("intent:change"), "{message}");
        assert!(message.contains("badge=C"), "{message}");
    } else {
        panic!("{intent:?}");
    }
    assert!(matches!(
        turn_event_to_ci(TurnEvent::Cancelled),
        Some(CiEvent::Cancelled)
    ));
    let conflict = turn_event_to_ci(TurnEvent::FileConflict {
        path: "a.rs".into(),
        claimant: "w1".into(),
        owner: "w2".into(),
    })
    .unwrap();
    if let CiEvent::Status { message } = conflict {
        assert!(message.contains("file_conflict"), "{message}");
    }
    let swarm = turn_event_to_ci(TurnEvent::SwarmStatus {
        active: 1,
        total: 2,
        message: String::new(),
    })
    .unwrap();
    if let CiEvent::Status { message } = swarm {
        assert!(message.contains("swarm active=1"), "{message}");
    }
    let swarm_msg = turn_event_to_ci(TurnEvent::SwarmStatus {
        active: 1,
        total: 2,
        message: "custom".into(),
    })
    .unwrap();
    if let CiEvent::Status { message } = swarm_msg {
        assert_eq!(message, "custom");
    }
    let bg = turn_event_to_ci(TurnEvent::Background {
        id: "j1".into(),
        status: "running".into(),
        summary: "echo".into(),
    })
    .unwrap();
    if let CiEvent::Status { message } = bg {
        assert!(message.contains("bg j1"), "{message}");
    }
    assert!(matches!(
        turn_event_to_ci(TurnEvent::EnqueuePrompt {
            text: "next".into()
        }),
        Some(CiEvent::Status { .. })
    ));
    assert!(matches!(
        turn_event_to_ci(TurnEvent::SwarmMessage {
            from: "a".into(),
            to: "b".into(),
            text: "hi".into(),
        }),
        Some(CiEvent::Status { .. })
    ));
    assert!(matches!(
        turn_event_to_ci(TurnEvent::FileStale {
            path: "x".into(),
            reader: "r".into(),
            writer: "w".into(),
        }),
        Some(CiEvent::Status { .. })
    ));
    assert!(matches!(
        turn_event_to_ci(TurnEvent::PermissionAsk {
            request_id: "p".into(),
            tool_name: "bash".into(),
            detail: String::new(),
        }),
        Some(CiEvent::Status { .. })
    ));
    assert!(matches!(
        turn_event_to_ci(TurnEvent::QuestionAsk {
            request_id: "q".into(),
            questions: serde_json::json!([]),
        }),
        Some(CiEvent::Status { .. })
    ));
    for update in [
        whycodes_core::PanelUpdate::Clear,
        whycodes_core::PanelUpdate::File {
            path: "a.rs".into(),
            text: String::new(),
        },
        whycodes_core::PanelUpdate::Diff {
            path: "a.rs".into(),
            unified: String::new(),
        },
        whycodes_core::PanelUpdate::Mermaid {
            source: String::new(),
        },
    ] {
        assert!(matches!(
            turn_event_to_ci(TurnEvent::Panel(update)),
            Some(CiEvent::Status { .. })
        ));
    }
    assert!(matches!(
        turn_event_to_ci(TurnEvent::Subagent {
            id: "s".into(),
            kind: "explore".into(),
            description: "d".into(),
            status: "done".into(),
            activity: String::new(),
            elapsed_ms: 0,
            output: String::new(),
        }),
        Some(CiEvent::Status { .. })
    ));
    let todos = turn_event_to_ci(TurnEvent::Todos { todos: vec![] }).unwrap();
    if let CiEvent::Status { message } = todos {
        assert!(message.contains("todos"), "{message}");
    }
}

#[test]
fn emit_headless_setup_error_formats() {
    assert!(emit_headless_setup_error(OutputFormat::Text, "nope").is_err());
    assert!(emit_headless_setup_error(OutputFormat::Json, "nope").is_err());
    assert!(emit_headless_setup_error(OutputFormat::StreamJson, "nope").is_err());
}

#[test]
fn strip_agents_fence_variants() {
    assert_eq!(strip_agents_fence("```markdown\nhi\n```"), "hi");
    assert_eq!(strip_agents_fence("```md\nhi\n```"), "hi");
    assert_eq!(strip_agents_fence("```\nhi\n```"), "hi");
    assert_eq!(strip_agents_fence("plain"), "plain");
}

#[test]
fn read_repl_line_queue_and_eof() {
    install_test_repl_lines(["hello"]);
    let mut buf = String::new();
    let n = read_repl_line(&mut buf).unwrap();
    assert!(n > 0);
    assert!(buf.contains("hello"));
    buf.clear();
    assert_eq!(read_repl_line(&mut buf).unwrap(), 0);
    clear_test_repl_lines();
    cmd_completions(clap_complete::Shell::Fish).unwrap();
    cmd_completions(clap_complete::Shell::Elvish).unwrap();
    cmd_completions(clap_complete::Shell::PowerShell).unwrap();
    let cfg = Config::default();
    assert!(!missing_api_key_message_for("groq", Some(&cfg)).is_empty());
    assert!(!oauth_provider_list().is_empty() || oauth_provider_list().contains("none"));
}
