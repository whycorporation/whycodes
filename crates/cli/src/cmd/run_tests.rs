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
}
