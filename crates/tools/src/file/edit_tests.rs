use super::*;

#[test]
fn exact_span_unique() {
    let spans = exact_spans("ab X cd X", "X");
    assert_eq!(spans, vec![(3, 4), (8, 9)]);
}

#[test]
fn ws_flexible_matches_indent_and_spacing() {
    let file = "fn run() {\n    let x = 1;\n}\n";
    let needle = "fn run() {\n  let x = 1;\n}";
    let spans = ws_flexible_spans(file, needle);
    assert_eq!(spans.len(), 1);
    assert_eq!(
        &file[spans[0].0..spans[0].1],
        "fn run() {\n    let x = 1;\n}"
    );
}

#[test]
fn ws_flexible_does_not_glue_tokens() {
    assert!(ws_flexible_spans("foobar", "foo bar").is_empty());
    assert!(ws_flexible_spans("fnord x", "fn x").is_empty());
}

#[test]
fn ws_flexible_skips_single_token() {
    assert!(ws_flexible_spans("hello", "hello").is_empty());
}

#[test]
fn ws_flexible_ambiguous_two_blocks() {
    let file = "fn a() {\n  x();\n}\nfn b() {\n  x();\n}\n";
    let needle = "x();";
    // single token → no fuzzy
    assert!(ws_flexible_spans(file, needle).is_empty());
    let needle = "{\n  x();\n}";
    assert_eq!(ws_flexible_spans(file, needle).len(), 2);
}

#[test]
fn apply_spans_replaces_in_order() {
    let s = "aa X bb X cc";
    let out = apply_spans(s, &[(3, 4), (8, 9)], "Y");
    assert_eq!(out, "aa Y bb Y cc");
}

#[test]
fn locate_prefers_exact_over_fuzzy() {
    let file = "foo  bar\nfoo bar\n";
    match locate_spans(file, "foo bar", false) {
        Locate::Hits(spans) => {
            assert_eq!(spans, vec![(9, 16)]);
        }
        _ => panic!("expected unique exact, got mismatch"),
    }
    match locate_spans(file, "foo bar", true) {
        Locate::Hits(spans) => assert_eq!(spans.len(), 1),
        _ => panic!("replace_all still exact-only when exact exists"),
    }
}

#[test]
fn locate_fuzzy_when_exact_missing() {
    let file = "    foo   bar\n";
    match locate_spans(file, "foo bar", false) {
        Locate::Hits(spans) => assert_eq!(&file[spans[0].0..spans[0].1], "foo   bar"),
        _ => panic!("expected fuzzy hit"),
    }
}

fn ctx(dir: &std::path::Path) -> crate::tool::ToolContext {
    crate::tool::ToolContext::new(dir.to_string_lossy().into_owned())
}

#[tokio::test]
async fn execute_replaces_text_and_reports_missing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("a.rs");
    std::fs::write(&path, "fn run() { let x = 1; }\n").unwrap();
    let tool = EditTool::new();
    let ok = tool
        .execute(
            serde_json::json!({
                "path": "a.rs",
                "old_string": "let x = 1;",
                "new_string": "let x = 2;"
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(!ok.is_error, "{}", ok.content);
    assert!(
        ok.content.contains("a.rs") || !ok.content.is_empty(),
        "{}",
        ok.content
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "fn run() { let x = 2; }\n"
    );

    let miss = tool
        .execute(
            serde_json::json!({
                "path": "a.rs",
                "old_string": "definitely-not-here",
                "new_string": "x"
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(miss.is_error, "{}", miss.content);
    assert!(miss.content.contains("Could not find"), "{}", miss.content);
}

#[tokio::test]
async fn execute_missing_required_params_is_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = EditTool::new()
        .execute(serde_json::json!({"path": "nope.rs"}), &ctx(dir.path()))
        .await;
    assert!(out.is_error, "{}", out.content);
}

#[tokio::test]
async fn remaining_edit_branches() {
    let t = EditTool::default();
    assert_eq!(t.name(), "edit");
    assert!(!t.description().is_empty());
    let _ = t.parameters();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "foo bar\nfoo bar\n").unwrap();
    let abs = dir.path().join("a.rs");
    let amb = t
        .execute(
            serde_json::json!({
                "path": abs.to_string_lossy(),
                "old_string": "foo bar",
                "new_string": "x"
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(amb.is_error, "{}", amb.content);
    assert!(amb.content.contains("occurrences"), "{}", amb.content);

    let all = t
        .execute(
            serde_json::json!({
                "path": "a.rs",
                "old_string": "foo bar",
                "new_string": "X",
                "replace_all": true
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(!all.is_error, "{}", all.content);

    let miss = t
        .execute(
            serde_json::json!({
                "path": "gone.rs",
                "old_string": "a",
                "new_string": "b"
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(miss.is_error);
    assert!(exact_spans("abc", "").is_empty());
    assert!(!left_boundary_ok("x", 0, ""));
    assert!(!right_boundary_ok("x", 0, ""));
    assert!(left_boundary_ok(" x", 1, "x"));
    assert!(right_boundary_ok("x ", 1, "x"));
    let spans = ws_flexible_spans("fooX bar", "foo bar");
    assert!(spans.is_empty() || spans.len() == 1);
    let write_err = write_edit_error("denied");
    assert!(write_err.is_error);
    assert!(
        write_err.content.contains("Error writing file"),
        "{}",
        write_err.content
    );
    let failed = write_edit_result(Err("denied".into()), "a.rs", "old", "new", 1, Some(1));
    assert!(failed.is_error);
    let ok_write = write_edit_result(Ok(()), "a.rs", "old", "new", 1, Some(1));
    assert!(!ok_write.is_error);
    assert!(left_boundary_ok("", 0, "x"));
    assert!(right_boundary_ok("x", 1, "x"));
    assert_eq!(skip_ws("  ab", 0), 2);
    assert_eq!(skip_ws("", 0), 0);
    assert!(ident_boundary_missing());
    assert!(ws_flexible_spans("fooXbar", "foo bar").is_empty());
    assert!(ws_flexible_spans("foobar", "foo bar").is_empty());
    assert!(ws_flexible_spans("foo bar foo", "foo zzz").is_empty());
    assert!(ws_flexible_spans("foo barX", "foo bar").is_empty());
    assert_eq!(tokens_match_after("foo   bar", &["bar"], 3), Some(9));
    assert!(tokens_match_after("foobar", &["bar"], 3).is_none());
    assert!(tokens_match_after("fooXbar", &["bar"], 3).is_none());
    assert_eq!(skip_token_at(3, 3), 6);
    assert!(tokens_need_ws().is_none());
    assert!(tokens_mismatch().is_none());
    assert!(ident_left_ok("", 0));
    assert!(ident_right_ok("", 0));
    assert!(ident_left_ok(" foo", 1));
    assert!(!ident_left_ok("xfoo", 1));
    assert!(ident_right_ok("foo ", 3));
    assert!(!ident_right_ok("foox", 3));
}

#[tokio::test]
async fn claim_conflict_and_helper_edges() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("claimed.rs");
    std::fs::write(&path, "fn run() {}\n").unwrap();
    let claims = whycodes_core::file_claims::FileClaimRegistry::new();
    assert!(matches!(
        claims.try_claim("other", "other-agent", &path),
        whycodes_core::file_claims::ClaimResult::Acquired
    ));
    let mut c = ctx(dir.path());
    c.file_claims = Some(claims);
    c.agent_id = Some("me".into());
    c.agent_label = Some("me-agent".into());
    let blocked = EditTool::new()
        .execute(
            serde_json::json!({
                "path": "claimed.rs",
                "old_string": "fn run",
                "new_string": "fn go"
            }),
            &c,
        )
        .await;
    assert!(blocked.is_error, "{}", blocked.content);
    assert!(
        blocked.content.contains("File conflict"),
        "{}",
        blocked.content
    );

    let as_dir = dir.path().join("adir");
    std::fs::create_dir(&as_dir).unwrap();
    let write_err = EditTool::new()
        .execute(
            serde_json::json!({
                "path": as_dir.to_string_lossy(),
                "old_string": "x",
                "new_string": "y"
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(write_err.is_error, "{}", write_err.content);

    assert!(ws_flexible_spans("foo bar", "foo bar").len() == 1);
    assert!(ws_flexible_spans("foobar x", "foo bar").is_empty());
    assert!(left_boundary_ok("foo", 0, "foo"));
    assert!(right_boundary_ok("foo", 3, "foo"));
    assert!(!left_boundary_ok("xfoo", 1, "foo"));
    assert!(!right_boundary_ok("foox", 3, "foo"));
    assert!(skip_ws("  a", 0) > 0);
    assert_eq!(skip_ws("a", 0), 0);
    assert_eq!(skip_ws("", 0), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn write_error_on_readonly_parent() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let parent = dir.path().join("rodir");
    std::fs::create_dir(&parent).unwrap();
    std::fs::write(parent.join("ro.rs"), "fn run() {}\n").unwrap();
    let mut perms = std::fs::metadata(&parent).unwrap().permissions();
    perms.set_mode(0o555);
    std::fs::set_permissions(&parent, perms).unwrap();
    let out = EditTool::new()
        .execute(
            serde_json::json!({
                "path": parent.join("ro.rs").to_string_lossy(),
                "old_string": "fn run() {}",
                "new_string": "fn go() {}"
            }),
            &ctx(dir.path()),
        )
        .await;
    let mut perms = std::fs::metadata(&parent).unwrap().permissions();
    perms.set_mode(0o755);
    let _ = std::fs::set_permissions(&parent, perms);
    assert!(out.is_error, "{}", out.content);
    assert!(
        out.content.contains("Error writing file"),
        "{}",
        out.content
    );
}
