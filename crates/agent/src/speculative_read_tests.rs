use super::*;

#[test]
fn parse_path_only() {
    let (p, o, l) = try_parse_read_args(r#"{"path": "crates/foo/src/lib.rs"}"#).unwrap();
    assert_eq!(p, "crates/foo/src/lib.rs");
    assert_eq!(o, 1);
    assert_eq!(l, DEFAULT_LIMIT);
}

#[test]
fn parse_with_window() {
    let (p, o, l) = try_parse_read_args(r#"{"path":"a.rs","offset":10,"limit":50}"#).unwrap();
    assert_eq!(p, "a.rs");
    assert_eq!(o, 10);
    assert_eq!(l, 50);
}

#[test]
fn incomplete_path_returns_none() {
    assert!(try_parse_read_args(r#"{"path": "crates/fo"#).is_none());
}

#[test]
fn escaped_path() {
    let (p, _, _) = try_parse_read_args(r#"{"path": "dir\\file.rs"}"#).unwrap();
    assert_eq!(p, "dir\\file.rs");
}

#[test]
fn field_order_path_last() {
    let (p, o, l) = try_parse_read_args(r#"{"offset": 2, "limit": 10, "path": "z.rs"}"#).unwrap();
    assert_eq!(p, "z.rs");
    assert_eq!(o, 2);
    assert_eq!(l, 10);
}

#[test]
fn window_clamped_to_hard_limit() {
    let (_, _, l) = try_parse_read_args(r#"{"path": "a.rs", "limit": 99999}"#).unwrap();
    assert_eq!(l, HARD_LIMIT);
    let (_, _, l) = try_parse_read_args(r#"{"path": "a.rs", "limit": 0}"#).unwrap();
    assert_eq!(l, 1);
    let (_, o, _) = try_parse_read_args(r#"{"path": "a.rs", "offset": 0}"#).unwrap();
    assert_eq!(o, 1, "offset floors at 1");
}

#[test]
fn unicode_escape_incomplete_waits() {
    // `\u` escape unfinished → path still streaming.
    assert!(try_parse_read_args(r#"{"path": "caf\u"#).is_none());
}

#[test]
fn trailing_backslash_waits_for_escape() {
    assert!(try_parse_read_args(r#"{"path": "dir\"#).is_none());
}

fn ctx(dir: &std::path::Path) -> ToolContext {
    ToolContext {
        working_dir: dir.to_string_lossy().into_owned(),
        session_id: None,
        sandbox: whycodes_core::SandboxSettings::off(),
        network: whycodes_core::NetworkPolicy::unrestricted(),
        file_claims: None,
        agent_id: None,
        agent_label: None,
        file_index: None,
        panel: None,
        todo_sink: None,
        swarm_hub: None,
    }
}

#[test]
fn spawn_skips_virtual_and_missing() {
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(dir.path());
    assert!(spawn_speculative_read("1".into(), "skill://x".into(), 1, 10, &c).is_none());
    assert!(spawn_speculative_read("1".into(), "missing.txt".into(), 1, 10, &c).is_none());
}

#[tokio::test]
async fn spawn_and_take_matching_hit_and_miss() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("n.txt"), "payload").unwrap();
    let c = ctx(dir.path());
    let mut jobs = Vec::new();
    maybe_start(&mut jobs, "c1", "read", r#"{"path":"n.txt"}"#, &c);
    assert_eq!(jobs.len(), 1);
    maybe_start(&mut jobs, "c1", "read", r#"{"path":"n.txt"}"#, &c);
    assert_eq!(jobs.len(), 1, "existing call_id skipped");
    maybe_start(&mut jobs, "c2", "bash", r#"{"path":"n.txt"}"#, &c);
    assert_eq!(jobs.len(), 1, "non-read skipped");

    let miss = take_matching(
        &mut jobs,
        "c1",
        "other.txt",
        1,
        DEFAULT_LIMIT,
        &c.working_dir,
    )
    .await;
    assert!(miss.is_none());
    assert_eq!(jobs.len(), 1);

    let hit = take_matching(&mut jobs, "c1", "n.txt", 1, DEFAULT_LIMIT, &c.working_dir)
        .await
        .expect("hit");
    assert!(hit.content.contains("payload"), "{hit:?}");
    assert!(jobs.is_empty());
}

#[tokio::test]
async fn maybe_start_pretty_json_fallback() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("n.txt"), "payload").unwrap();
    let c = ctx(dir.path());
    let mut jobs = Vec::new();
    maybe_start(
        &mut jobs,
        "c1",
        "read",
        "{\n  \"path\": \"n.txt\",\n  \"offset\": 1\n}",
        &c,
    );
    assert_eq!(jobs.len(), 1);
    abort_all(&mut jobs);
    assert!(jobs.is_empty());
}

#[tokio::test]
async fn take_matching_join_err_after_abort() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("n.txt"), "payload").unwrap();
    let c = ctx(dir.path());
    let mut jobs = Vec::new();
    let job =
        spawn_speculative_read("c1".into(), "n.txt".into(), 1, DEFAULT_LIMIT, &c).expect("spawn");
    job.handle.abort();
    jobs.push(job);
    let miss = take_matching(&mut jobs, "c1", "n.txt", 1, DEFAULT_LIMIT, &c.working_dir).await;
    assert!(miss.is_none());
    assert!(jobs.is_empty());
}

#[test]
fn window_from_args_defaults_and_clamp() {
    assert_eq!(window_from_args(&json!({})), (1, DEFAULT_LIMIT));
    let (_, l) = window_from_args(&json!({"limit": 99999}));
    assert_eq!(l, HARD_LIMIT);
    let (o, _) = window_from_args(&json!({"offset": 0}));
    assert_eq!(o, 1);
}

#[test]
fn non_string_path_returns_none() {
    assert!(try_parse_read_args(r#"{"path": 123}"#).is_none());
}

#[test]
fn extract_json_u64_empty_digits_after_colon_is_none() {
    assert!(extract_json_u64_field(r#"{"offset":}"#, "offset").is_none());
    assert!(extract_json_u64_field(r#"{"offset": }"#, "offset").is_none());
    assert_eq!(
        extract_json_u64_field(r#"{"offset": 12"#, "offset"),
        Some(12)
    );
}

#[tokio::test]
async fn maybe_start_pretty_json_window_from_args() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("n.txt"), "payload").unwrap();
    let c = ctx(dir.path());
    let mut jobs = Vec::new();
    maybe_start(
        &mut jobs,
        "c1",
        "read",
        "{\n  \"path\": \"n.txt\",\n  \"offset\": 2,\n  \"limit\": 9\n}",
        &c,
    );
    assert_eq!(jobs.len(), 1);
    abort_all(&mut jobs);
}

#[test]
fn window_from_args_matches_tool_semantics() {
    let v = serde_json::json!({"path": "a.rs"});
    assert_eq!(window_from_args(&v), (1, DEFAULT_LIMIT));
    let v = serde_json::json!({"path": "a.rs", "offset": 5, "limit": 100});
    assert_eq!(window_from_args(&v), (5, 100));
    let v = serde_json::json!({"offset": 0, "limit": 0});
    let (o, l) = window_from_args(&v);
    assert_eq!(o, 1);
    assert_eq!(l, 1);
}

#[test]
fn abort_all_drains_jobs() {
    let mut jobs = Vec::new();
    abort_all(&mut jobs);
    assert!(jobs.is_empty());
}

#[test]
fn spawn_skips_internal_schemes() {
    let ctx = whycodes_core::ToolContext::new("/tmp");
    assert!(spawn_speculative_read("c1".into(), "skill://demo".into(), 1, 10, &ctx).is_none());
    assert!(spawn_speculative_read("c1".into(), "agent://task-1".into(), 1, 10, &ctx).is_none());
}

#[test]
fn empty_path_and_escape_variants() {
    assert!(try_parse_read_args(r#"{"path": ""}"#).is_none());
    assert!(try_parse_read_args(r#"{"path": "   "}"#).is_none());
    let (p, _, _) = try_parse_read_args(r#"{"path": "a\nb"}"#).unwrap();
    assert_eq!(p, "a\nb");
    let (p, _, _) = try_parse_read_args(r#"{"path": "a\rb"}"#).unwrap();
    assert_eq!(p, "a\rb");
    let (p, _, _) = try_parse_read_args(r#"{"path": "a\tb"}"#).unwrap();
    assert_eq!(p, "a\tb");
    let (p, _, _) = try_parse_read_args(r#"{"path": "a\/b"}"#).unwrap();
    assert_eq!(p, "a/b");
    let (p, _, _) = try_parse_read_args(r#"{"path": "a\qb"}"#).unwrap();
    assert_eq!(p, "aqb");
    assert!(try_parse_read_args(r#"{"path": "caf\u"}"#).is_none());
    assert!(try_parse_read_args(r#"{"offset": x, "path": "a.rs"}"#).is_some());
    assert!(extract_json_u64_field(r#"{"offset":}"#, "offset").is_none());
    assert!(extract_json_u64_field("nope", "offset").is_none());
    assert!(extract_json_u64_field(r#"{"offset":"#, "offset").is_none());
    assert_eq!(
        extract_json_u64_field(r#"{"offset": 12"#, "offset"),
        Some(12)
    );
}

#[tokio::test]
async fn maybe_start_pretty_json_empty_path_is_none() {
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(dir.path());
    let mut jobs = Vec::new();
    maybe_start(&mut jobs, "c1", "read", r#"{"path": ""}"#, &c);
    assert!(jobs.is_empty());
    maybe_start(&mut jobs, "c1", "read", "{\n  \"path\": \"\"\n}", &c);
    assert!(jobs.is_empty());
    maybe_start(
        &mut jobs,
        "c2",
        "read",
        "{\n  \"path\": \"\",\n  \"offset\": 1\n}",
        &c,
    );
    assert!(jobs.is_empty());
}

#[tokio::test]
async fn maybe_start_unicode_escape_falls_back_to_serde() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("n.txt"), "payload").unwrap();
    let c = ctx(dir.path());
    let mut jobs = Vec::new();
    maybe_start(
        &mut jobs,
        "c1",
        "read",
        r#"{"path": "\u006e.txt", "offset": 1, "limit": 9}"#,
        &c,
    );
    assert_eq!(jobs.len(), 1, "serde path \\u006e.txt should spawn");
    abort_all(&mut jobs);
}

#[test]
fn maybe_start_ignores_non_read_tools() {
    let mut jobs = Vec::new();
    let ctx = whycodes_core::ToolContext {
        working_dir: "/work/proj".into(),
        session_id: None,
        sandbox: whycodes_core::SandboxSettings::off(),
        network: whycodes_core::NetworkPolicy::unrestricted(),
        file_claims: None,
        agent_id: None,
        agent_label: None,
        file_index: None,
        panel: None,
        todo_sink: None,
        swarm_hub: None,
    };
    maybe_start(&mut jobs, "tc-1", "grep", r#"{"pattern": "x"}"#, &ctx);
    assert!(jobs.is_empty());
}
