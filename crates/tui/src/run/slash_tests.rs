use super::*;
use crate::config::TuiAppConfig;

#[test]
fn slash_command_from_prompt_and_consume() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    assert!(slash_command_from_prompt(&app).is_none());
    app.input_buffer = "/help".into();
    assert_eq!(slash_command_from_prompt(&app).as_deref(), Some("/help"));
    consume_slash_draft(&mut app);
    assert!(app.input_buffer.is_empty());
    assert_eq!(app.input_cursor, 0);
    assert!(slash_command_from_prompt(&app).is_none());

    app.input_buffer = "/he".into();
    app.slash_suggest.refresh(&app.input_buffer);
    assert!(app.slash_suggest.active);
    let from_suggest = slash_command_from_prompt(&app).expect("slash suggest current");
    assert!(
        from_suggest.starts_with('/'),
        "active slash suggest must replace the draft with the highlighted command, got {from_suggest}"
    );
}

#[test]
fn expand_at_files_inlines_and_keeps_bare_at() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "hello-at").unwrap();
    let out = expand_at_files("see @note.txt please", dir.path());
    assert!(out.contains("hello-at"), "{out}");
    assert!(out.contains("note.txt"), "{out}");
    assert_eq!(expand_at_files("keep @ alone", dir.path()), "keep @ alone");
    assert_eq!(expand_at_files("no mentions", dir.path()), "no mentions");
}

#[test]
fn format_bg_jobs_and_share_helpers() {
    use std::time::Duration;
    use whycodes_agent::{JobSnapshot, JobStatus};

    let jobs = vec![JobSnapshot {
        id: "bg-1".into(),
        label: "cargo test".into(),
        status: JobStatus::Running,
        elapsed: Duration::from_secs(3),
        output_len: 0,
        exit_code: None,
    }];
    let listed = format_bg_jobs(1, &jobs);
    assert!(listed.contains("Background jobs (1 running)"), "{listed}");
    assert!(listed.contains("bg-1"), "{listed}");
    assert!(listed.contains("cargo test"), "{listed}");
    assert!(listed.contains("Hint: /bg kill"), "{listed}");

    let mut app = TuiApp::from_config(TuiAppConfig::default());
    memory_err_toast(&mut app, "disk full");
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Memory: disk full"))
    );

    apply_share_ok(&mut app, "/tmp/s.json", "sid-1", |_| true);
    assert!(
        app.status_message.contains("Share:"),
        "{}",
        app.status_message
    );
    assert!(
        app.messages
            .iter()
            .any(|m| m.content.contains("server is up")),
        "{:?}",
        app.messages.iter().map(|m| &m.content).collect::<Vec<_>>()
    );

    apply_share_ok(&mut app, "/tmp/s.json", "sid-2", |_| false);
    assert!(
        app.status_message.contains("Exported"),
        "{}",
        app.status_message
    );

    export_failed_toast(&mut app, "disk full");
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Export failed: disk full"))
    );

    let prev = std::env::var_os("WHYCODES_SHARE_PORT");
    unsafe { std::env::set_var("WHYCODES_SHARE_PORT", "4040") };
    apply_share_ok(&mut app, "/tmp/s.json", "sid-3", |port| {
        assert_eq!(port, 4040);
        false
    });
    assert!(
        app.status_message.contains(":4040/"),
        "{}",
        app.status_message
    );
    match prev {
        Some(v) => unsafe { std::env::set_var("WHYCODES_SHARE_PORT", v) },
        None => unsafe { std::env::remove_var("WHYCODES_SHARE_PORT") },
    }
}

#[test]
fn suggestion_text_from_blocks_skips_empty_and_quotes() {
    use whycodes_core::types::ContentBlock;
    assert!(suggestion_text_from_blocks(&[]).is_empty());
    assert!(
        suggestion_text_from_blocks(&[ContentBlock::Text {
            text: "\n  \n".into(),
        }])
        .is_empty()
    );
    assert_eq!(
        suggestion_text_from_blocks(&[
            ContentBlock::Thinking {
                text: "ignore".into(),
                signature: None,
            },
            ContentBlock::Text {
                text: "\n\"next step\"\n".into()
            }
        ]),
        "next step"
    );
}

#[test]
fn suggestion_prompt_and_oauth_hint_helpers() {
    let body = suggestion_prompt_body("do the thing", "ok");
    assert!(body.contains("do the thing"), "{body}");
    assert!(body.contains("ok"), "{body}");
    let req = suggestion_llm_request("user", "asst");
    assert_eq!(req.max_tokens, Some(40));
    assert!(!req.system.is_empty());
    let t = suggestion_transport();
    assert_eq!(t.retry.max_retries, 0);
    assert_eq!(oauth_unavailable_hint(Vec::new()), "install an auth plugin");
    assert_eq!(
        oauth_unavailable_hint(vec!["anthropic".into(), "openai".into()]),
        "anthropic, openai"
    );
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    send_suggestion_text(&[], &tx);
    assert!(rx.try_recv().is_err());
    send_suggestion_text(
        &[whycodes_core::types::ContentBlock::Text {
            text: "try cargo test".into(),
        }],
        &tx,
    );
    assert_eq!(rx.try_recv().unwrap(), "try cargo test");
}

#[tokio::test]
async fn complete_prompt_suggestion_sends_scripted_text() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let prov = whycodes_llm::ScriptedProvider::repeating(
        "acme",
        [whycodes_llm::ScriptedStep::Text("try cargo test".into())],
    );
    complete_prompt_suggestion(&prov, "do the next step", "ok", "sk", "m-suggest-ok", tx).await;
    let got = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .ok()
        .flatten()
        .expect("suggestion text");
    assert_eq!(got, "try cargo test");
}

#[tokio::test]
async fn complete_prompt_suggestion_logs_fail_open_without_sending() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let prov = whycodes_llm::ScriptedProvider::repeating(
        "acme",
        [whycodes_llm::ScriptedStep::FailOpen("scripted-fail".into())],
    );
    // Distinct model so the sibling success test's process-wide response
    // cache cannot replay "try cargo test" here.
    complete_prompt_suggestion(&prov, "do the next step", "ok", "sk", "m-suggest-fail", tx).await;
    assert!(
        rx.try_recv().is_err(),
        "fail-open must not enqueue a suggestion"
    );
}

#[test]
fn send_suggestion_text_drops_when_loop_closed() {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    drop(rx);
    send_suggestion_text(
        &[whycodes_core::types::ContentBlock::Text {
            text: "try cargo test".into(),
        }],
        &tx,
    );
}

#[test]
fn expand_at_files_truncates_long_and_keeps_absolute() {
    let dir = tempfile::tempdir().unwrap();
    let long: String = "x".repeat(AT_FILE_MAX_CHARS + 40);
    std::fs::write(dir.path().join("big.txt"), &long).unwrap();
    let out = expand_at_files("see @big.txt please", dir.path());
    assert!(out.contains("characters omitted from @big.txt"), "{out}");
    assert!(out.contains("--- file: big.txt ---"), "{out}");

    let abs = dir.path().join("note.txt");
    std::fs::write(&abs, "abs-body").unwrap();
    let mention = format!("see @{} end", abs.display());
    let out = expand_at_files(&mention, dir.path());
    assert!(out.contains("abs-body"), "{out}");
}

#[test]
fn resolve_tell_matches_a_prefix_or_the_only_running_subagent() {
    let two = vec!["task-abc".to_string(), "worker-0".to_string()];
    assert_eq!(
        resolve_tell(&two, "task focus here"),
        Some(("task-abc".into(), "focus here".into()))
    );
    assert_eq!(resolve_tell(&two, "worker-0"), None, "no message");
    assert_eq!(resolve_tell(&two, "just a message"), None, "ambiguous");
    assert_eq!(resolve_tell(&two, "   "), None);
    let one = vec!["task-abc".to_string()];
    assert_eq!(
        resolve_tell(&one, "stop now"),
        Some(("task-abc".into(), "stop now".into()))
    );
}

#[test]
fn busy_tell_queues_for_the_subagent_or_explains() {
    let inbox = whycodes_agent::steer::SteerInbox::default();
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.input_buffer = "plain prompt".into();
    assert!(!busy_tell(&mut app, &inbox));
    assert_eq!(app.input_buffer, "plain prompt");

    app.input_buffer = "/tell hi".into();
    assert!(busy_tell(&mut app, &inbox));
    assert_eq!(app.status_message, "No subagent is running");
    assert!(app.input_buffer.is_empty());

    for id in ["task-abc", "worker-0"] {
        app.upsert_subagent(crate::app::SubagentUpdate {
            id: id.into(),
            kind: "general".into(),
            description: "d".into(),
            status: "running".into(),
            activity: String::new(),
            elapsed_ms: 0,
            output: String::new(),
        });
    }
    app.input_buffer = "/tell ambiguous".into();
    assert!(busy_tell(&mut app, &inbox));
    assert!(
        app.status_message.contains("task-abc, worker-0"),
        "{}",
        app.status_message
    );

    app.input_buffer = "/tell work check the logs".into();
    assert!(busy_tell(&mut app, &inbox));
    assert_eq!(app.status_message, "sent to worker-0");
    let got = inbox.drain("worker-0");
    assert_eq!(
        whycodes_agent::steer::content_text(&got[0]),
        "check the logs"
    );
}

#[test]
fn terminal_setup_asks_before_editing_tmux_conf() {
    use crate::terminal_setup::{TerminalEnv, tmux_conf_path};
    let home = tempfile::tempdir().unwrap();
    let tmux = TerminalEnv {
        tmux: true,
        ..TerminalEnv::default()
    };

    // Outside tmux: report only.
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    handle_terminal_setup_slash(&mut app, &TerminalEnv::default(), Some(home.path()));
    assert!(!app.dialogs.is_open());
    assert!(
        app.messages
            .last()
            .is_some_and(|m| m.content.contains("inside tmux: no"))
    );

    // Inside tmux with settings missing: a confirm, and no file yet.
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    handle_terminal_setup_slash(&mut app, &tmux, Some(home.path()));
    assert!(matches!(
        app.dialogs.active(),
        Some(DialogKind::Confirm {
            on_confirm: ConfirmAction::TerminalSetup,
            ..
        })
    ));
    let conf = tmux_conf_path(home.path());
    assert!(
        !conf.exists(),
        "nothing is written before the user says yes"
    );

    // Confirmed: the run loop writes it; a second run has nothing to offer.
    apply_terminal_setup(&mut app, Some(home.path()));
    assert!(
        std::fs::read_to_string(&conf)
            .unwrap()
            .contains("extended-keys")
    );
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    handle_terminal_setup_slash(&mut app, &tmux, Some(home.path()));
    assert!(!app.dialogs.is_open(), "already configured: no confirm");
    assert!(
        app.messages
            .last()
            .is_some_and(|m| m.content.contains("already configured"))
    );

    // No home: no confirm, and applying says why.
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    handle_terminal_setup_slash(&mut app, &tmux, None);
    assert!(!app.dialogs.is_open());
    apply_terminal_setup(&mut app, None);
    assert!(app.status_message.contains("No home directory"));
}

#[test]
fn mode_commands_leave_the_message_and_return_the_mode() {
    use whycodes_agent::steer::SteerMode;
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.input_buffer = "  /now  fix it ".into();
    assert_eq!(take_mode_command(&mut app), Some(SteerMode::Urgent));
    assert_eq!(app.input_buffer, "fix it");
    assert_eq!(app.input_cursor, "fix it".len());
    app.input_buffer = "/queue then tests".into();
    assert_eq!(take_mode_command(&mut app), Some(SteerMode::Queue));
    assert_eq!(app.input_buffer, "then tests");
    app.input_buffer = "/now".into();
    assert_eq!(take_mode_command(&mut app), Some(SteerMode::Urgent));
    assert_eq!(app.status_message, "Usage: /now <message>");
    app.input_buffer = "/nowhere".into();
    assert_eq!(take_mode_command(&mut app), None);
    assert_eq!(app.input_buffer, "/nowhere");
}

#[test]
fn busy_agents_opens_the_hub_mid_turn() {
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    app.input_buffer = "/agentsx".into();
    assert!(!busy_agents(&mut app));
    app.input_buffer = " /agents ".into();
    assert!(busy_agents(&mut app));
    assert!(app.input_buffer.is_empty());
    assert!(matches!(
        app.dialogs.active(),
        Some(crate::app::DialogKind::Agents)
    ));
}
