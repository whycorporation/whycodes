use super::*;

#[test]
fn fold_parallel_joins_ok_and_fail() {
    fold_parallel_joins([Ok(false)], false).unwrap();
    assert!(fold_parallel_joins([Ok(true)], false).is_err());
    assert!(fold_parallel_joins([Err("x".into())], true).is_err());
    assert!(fold_parallel_joins([Err("x".into())], false).is_err());
}

#[test]
fn emit_parallel_outcome_covers_formats() {
    let meta = ResultMeta {
        session_id: "s".into(),
        provider: "p".into(),
        model: "m".into(),
        agent: "a".into(),
        usage: Default::default(),
        duration_ms: 1,
    };
    let wrap = |ev: CiEvent| ev;
    assert!(!emit_parallel_outcome(
        OutputFormat::Text,
        Ok("ok".into()),
        meta.clone(),
        &wrap
    ));
    assert!(!emit_parallel_outcome(
        OutputFormat::Text,
        Ok(String::new()),
        meta.clone(),
        &wrap
    ));
    assert!(!emit_parallel_outcome(
        OutputFormat::Json,
        Ok("j".into()),
        meta.clone(),
        &wrap
    ));
    assert!(!emit_parallel_outcome(
        OutputFormat::StreamJson,
        Ok("s".into()),
        meta.clone(),
        &wrap
    ));
    assert!(emit_parallel_outcome(
        OutputFormat::Text,
        Err("boom".into()),
        meta.clone(),
        &wrap
    ));
    assert!(emit_parallel_outcome(
        OutputFormat::Json,
        Err("jerr".into()),
        meta.clone(),
        &wrap
    ));
    assert!(emit_parallel_outcome(
        OutputFormat::StreamJson,
        Err("cancel me".into()),
        meta.clone(),
        &wrap
    ));
    assert!(emit_parallel_outcome(
        OutputFormat::StreamJson,
        Err("provider down".into()),
        meta,
        &wrap
    ));
}

#[test]
fn helpers_all_prompts_and_fan_out() {
    assert!(all_prompts_empty(&[String::new(), String::new()]));
    assert!(!all_prompts_empty(&["x".into()]));
    assert!(!should_fan_out(&["a".into()]));
    assert!(should_fan_out(&["a".into(), "b".into()]));
    let mapped = map_tui_run_error(anyhow::anyhow!("os error 6"));
    assert!(mapped.to_string().contains("TUI needs a real terminal"));
    let mapped = map_tui_run_error(anyhow::anyhow!("No such device"));
    assert!(mapped.to_string().contains("TUI needs a real terminal"));
    let mapped = map_tui_run_error(anyhow::anyhow!("not a terminal"));
    assert!(mapped.to_string().contains("TUI needs a real terminal"));
    let passthrough = map_tui_run_error(anyhow::anyhow!("bind failed"));
    assert_eq!(passthrough.to_string(), "bind failed");
}

#[test]
fn tui_plain_and_resume_helpers() {
    assert!(!force_plain_mode(false));
    assert!(force_plain_mode(true));
    let prev = std::env::var_os("WHYCODES_PLAIN");
    unsafe { std::env::set_var("WHYCODES_PLAIN", "1") };
    assert!(force_plain_mode(false));
    match prev {
        Some(v) => unsafe { std::env::set_var("WHYCODES_PLAIN", v) },
        None => unsafe { std::env::remove_var("WHYCODES_PLAIN") },
    }

    assert!(!should_use_tui(true, true, true));
    assert!(should_use_tui(false, true, false));
    assert!(should_use_tui(false, false, true));
    assert!(!should_use_tui(false, false, false));

    assert!(is_repl_interactive(None, false));
    assert!(is_repl_interactive(Some(""), false));
    assert!(!is_repl_interactive(Some("hi"), false));
    assert!(!is_repl_interactive(None, true));

    assert_eq!(
        resume_missing_label(whycodes_tui::RESUME_LATEST),
        "none saved yet"
    );
    assert_eq!(resume_missing_label("sess-1"), "sess-1");
}
