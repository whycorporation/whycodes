use super::*;
use whycodes_config::Config;
use whycodes_core::types::Usage;
use whycodes_session::session::Session;

#[test]
fn persist_session_best_effort_does_not_panic() {
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let prev = std::env::var_os("WHYCODES_HOME");
    unsafe { std::env::set_var("WHYCODES_HOME", home.path()) };
    reset_session_db_cache();
    let session = Session::new(home.path().to_path_buf(), "sys".into());
    persist_session_best_effort(&session, "test");
    reset_session_db_cache();
    unsafe {
        match prev {
            Some(v) => std::env::set_var("WHYCODES_HOME", v),
            None => std::env::remove_var("WHYCODES_HOME"),
        }
    }
}

#[test]
fn configured_models_includes_config_entries() {
    let mut cfg = Config::default();
    cfg.providers.insert(
        "local".into(),
        whycodes_core::types::ProviderConfig {
            name: "local".into(),
            api_key: None,
            api_base: None,
            base_url: None,
            headers: None,
            models: vec!["tiny-test".into()],
            tool_arguments: None,
            extra: Default::default(),
        },
    );
    let models = configured_models(&cfg);
    assert!(
        models.iter().any(|(p, m)| p == "local" && m == "tiny-test"),
        "{models:?}"
    );
}

#[test]
fn cost_report_empty_usage_is_estimated() {
    let session = Session::new("/tmp/p".into(), "sys".into());
    let app = TuiApp::new(TuiAppConfig::default());
    let out = cost_report(&session, &app);
    assert!(out.contains("Cost"), "{out}");
    assert!(
        out.contains("estimated") || out.contains("none yet"),
        "{out}"
    );
    let mut session = session;
    session.usage = Usage {
        input_tokens: 10,
        output_tokens: 4,
        cache_creation_input_tokens: Some(2),
        cache_read_input_tokens: Some(1),
    };
    let mut app = TuiApp::new(TuiAppConfig::default());
    app.turn_usage = Some(Usage {
        input_tokens: 3,
        output_tokens: 1,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    });
    let out = cost_report(&session, &app);
    assert!(out.contains("last turn"), "{out}");
    assert!(out.contains("cache"), "{out}");
}

#[test]
fn persist_outcome_helpers_cover_err_none_and_ok() {
    let session = Session::new("/tmp/p".into(), "sys".into());
    persist_session_outcome(&session, "ok", Some(Ok(())));
    persist_session_outcome(&session, "fail", Some(Err("disk full".into())));
    persist_session_outcome(&session, "nodb", None);
    save_backfilled_title(Ok(()));
    save_backfilled_title(Err("locked".into()));
    assert!(session_list_rows::<Vec<()>>(None).is_none());
    assert!(session_list_rows(Some(vec![1])).is_some());
    let err = loaded_session_or_unavailable(None).expect_err("unavailable");
    assert!(err.to_string().contains("unavailable"));
    assert!(
        loaded_session_or_unavailable(Some(Ok(None)))
            .unwrap()
            .is_none()
    );
}

fn cmd_output(script: &str) -> std::process::Output {
    #[cfg(windows)]
    {
        std::process::Command::new("cmd")
            .args(["/C", script])
            .output()
            .unwrap()
    }
    #[cfg(unix)]
    {
        std::process::Command::new("sh")
            .args(["-c", script])
            .output()
            .unwrap()
    }
}

#[test]
fn append_git_status_covers_unavailable_failed_and_clean() {
    let mut out = String::from("Diff\n");
    assert!(
        append_git_status(
            &mut out,
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "git")),
        )
        .is_err()
    );
    assert!(out.contains("git unavailable"), "{out}");

    let failed = cmd_output("exit 1");
    let mut out = String::from("Diff\n");
    assert!(append_git_status(&mut out, Ok(failed)).is_err());
    assert!(out.contains("git status failed"), "{out}");

    let clean = cmd_output("exit 0");
    let mut out = String::from("Diff\n");
    assert!(append_git_status(&mut out, Ok(clean)).is_ok());
    assert!(out.contains("clean working tree"), "{out}");

    let dirty = cmd_output("echo branch-status");
    let mut out = String::from("Diff\n");
    assert!(append_git_status(&mut out, Ok(dirty)).is_ok());
    assert!(out.contains("status:"), "{out}");

    let mut out = String::new();
    append_git_stat_block(
        &mut out,
        "  staged only:\n",
        Err(std::io::Error::new(std::io::ErrorKind::NotFound, "git")),
        40,
    );
    assert!(out.is_empty());
    let listed = cmd_output("echo file.rs");
    append_git_stat_block(&mut out, "  staged only:\n", Ok(listed), 40);
    assert!(out.contains("staged only"), "{out}");
    assert!(out.contains("file.rs") || out.contains("file"), "{out}");
}

#[test]
fn toast_index_and_doctor_key_and_tool_chars() {
    let mut app = TuiApp::new(TuiAppConfig::default());
    toast_indexed_chunks(&mut app, 0);
    assert!(app.toasts.is_empty());
    toast_indexed_chunks(&mut app, 4);
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Indexed 4"))
    );
    assert_eq!(doctor_key_label(true), "set");
    assert!(doctor_key_label(false).contains("MISSING"));

    use whycodes_core::types::{ContentBlock, MessageContent};
    assert_eq!(tool_result_chars(&MessageContent::Text("abcd".into())), 4);
    assert_eq!(
        tool_result_chars(&MessageContent::Blocks(vec![
            ContentBlock::Text { text: "hi".into() },
            ContentBlock::ToolResult {
                tool_use_id: "t".into(),
                content: "xyz".into(),
                is_error: Some(false),
            },
            ContentBlock::Image {
                source: whycodes_core::types::ImageSource::Url {
                    url: "https://x".into()
                }
            },
            ContentBlock::Thinking {
                text: "secret".into(),
                signature: None,
            },
        ])),
        5
    );
}

#[test]
fn session_details_lists_optional_models_and_usage() {
    let session = Session::new("/tmp/p".into(), "sys".into());
    let app = TuiApp::new(TuiAppConfig::default());
    let mut config = Config::default();
    let empty = session_details(&session, "build", &app, &config);
    assert!(empty.contains("model_fast"), "{empty}");
    assert!(empty.contains("estimated"), "{empty}");

    config.session.model_fast = Some("fast".into());
    config.session.model_smol = Some("smol".into());
    config.session.model_plan = Some("plan".into());
    config.session.stream_rules.push(Default::default());
    let mut session = session;
    session.usage = Usage {
        input_tokens: 10,
        output_tokens: 4,
        cache_creation_input_tokens: Some(2),
        cache_read_input_tokens: Some(1),
    };
    let filled = session_details(&session, "build", &app, &config);
    assert!(filled.contains("model_fast: fast"), "{filled}");
    assert!(filled.contains("model_smol: smol"), "{filled}");
    assert!(filled.contains("model_plan: plan"), "{filled}");
    assert!(filled.contains("stream_rules: 1"), "{filled}");
    assert!(filled.contains("cache write"), "{filled}");
    assert!(filled.contains("cache read"), "{filled}");
}

#[test]
fn upgraded_title_from_load_skips_missing_and_err() {
    assert!(upgraded_title_from_load(Ok(None)).is_none());
    assert!(upgraded_title_from_load(Err("locked".into())).is_none());
    let session = Session::new("/tmp/p".into(), "sys".into());
    assert!(upgraded_title_from_load(Ok(Some(session))).is_some());
}

#[test]
fn upgrade_loaded_title_skips_unchanged_and_saves_heuristic() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::new(dir.path().to_path_buf(), "sys".into());
    assert!(upgrade_loaded_title(Some(session.clone()), |_| Ok(())).is_none());

    session.add_user_message("please fix the login flow");
    let upgraded = upgrade_loaded_title(Some(session), |_| Err("locked".into()));
    assert!(upgraded.is_some(), "{upgraded:?}");
    assert!(upgrade_loaded_title(None, |_| Ok(())).is_none());
}

#[test]
fn doctor_helpers_and_short_id() {
    assert_eq!(doctor_key_label(true), "set");
    assert!(doctor_key_label(false).contains("MISSING"));
    assert_eq!(short_session_id("abc"), "abc");
    assert!(short_session_id("abcdefghijklmnop").starts_with("abcdefgh"));
    let mut app = TuiApp::from_config(TuiAppConfig::default());
    toast_indexed_chunks(&mut app, 0);
    toast_indexed_chunks(&mut app, 3);
    assert!(
        app.toasts
            .visible()
            .iter()
            .any(|t| t.message.contains("Indexed 3"))
    );
    let chars = tool_result_chars(&whycodes_core::types::MessageContent::Text("hi".into()));
    assert_eq!(chars, 2);
    assert!(!share_server_up(1));
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(unshare_session(dir.path(), "nope"), 0);
}
