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
