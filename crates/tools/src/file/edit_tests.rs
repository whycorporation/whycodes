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
    assert!(miss.content.contains("Nearby lines:"), "{}", miss.content);

    // First token exists, the rest does not: snippet centers on that line.
    let partial = tool
        .execute(
            serde_json::json!({
                "path": "a.rs",
                "old_string": "let missing-tail",
                "new_string": "x"
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(partial.is_error, "{}", partial.content);
    assert!(
        partial.content.contains("Could not find"),
        "{}",
        partial.content
    );
}

#[tokio::test]
async fn execute_refuses_harmony_leak_in_new_string() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("a.rs");
    std::fs::write(&path, "fn run() {}\n").unwrap();
    let leak = "fn run() { 1 }\nanalysis to=functions.edit code \u{8d4c}\u{535a}\u{7f51}\u{7ad9}\u{63a8}\u{8350}\u{4ee3}\u{7406}";
    let out = EditTool::new()
        .execute(
            serde_json::json!({
                "path": "a.rs",
                "old_string": "fn run() {}",
                "new_string": leak
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(out.is_error, "{}", out.content);
    assert!(
        out.content.contains("Harmony protocol leak"),
        "{}",
        out.content
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "fn run() {}\n");
}

#[tokio::test]
async fn execute_replaces_by_from_to_tags() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("a.rs");
    std::fs::write(&path, "fn run() {\n    let x = 1;\n}\n").unwrap();
    let from = crate::file::line_tag::tag_of("    let x = 1;", 2);
    let tool = EditTool::new();
    let ok = tool
        .execute(
            serde_json::json!({
                "path": "a.rs",
                "from": from,
                "new_string": "    let x = 2;\n"
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(!ok.is_error, "{}", ok.content);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "fn run() {\n    let x = 2;\n}\n"
    );
}

#[tokio::test]
async fn execute_insert_after_tag() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("a.rs");
    std::fs::write(&path, "fn run() {\n    let x = 1;\n}\n").unwrap();
    let after = crate::file::line_tag::tag_of("    let x = 1;", 2);
    let ok = EditTool::new()
        .execute(
            serde_json::json!({
                "path": "a.rs",
                "insert_after": after,
                "new_string": "    let y = 3;"
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(!ok.is_error, "{}", ok.content);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "fn run() {\n    let x = 1;\n    let y = 3;\n}\n"
    );
}

#[tokio::test]
async fn execute_insert_after_last_line_without_newline() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("a.rs");
    std::fs::write(&path, "fn run() {}").unwrap();
    let after = crate::file::line_tag::tag_of("fn run() {}", 2);
    let ok = EditTool::new()
        .execute(
            serde_json::json!({
                "path": "a.rs",
                "insert_after": after,
                "new_string": "fn go() {}"
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(!ok.is_error, "{}", ok.content);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "fn run() {}\nfn go() {}\n"
    );
}

#[tokio::test]
async fn execute_replaces_from_to_range() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("a.rs");
    std::fs::write(&path, "fn run() {\n    let x = 1;\n    let y = 2;\n}\n").unwrap();
    let from = crate::file::line_tag::tag_of("    let x = 1;", 2);
    let to = crate::file::line_tag::tag_of("    let y = 2;", 2);
    let ok = EditTool::new()
        .execute(
            serde_json::json!({
                "path": "a.rs",
                "from": from,
                "to": to,
                "new_string": "    let z = 3;\n"
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(!ok.is_error, "{}", ok.content);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "fn run() {\n    let z = 3;\n}\n"
    );
}

#[tokio::test]
async fn execute_tagged_error_paths() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("a.rs");
    std::fs::write(&path, "fn run() {\n    let x = 1;\n    let x = 1;\n}\n").unwrap();
    let tag = crate::file::line_tag::tag_of("    let x = 1;", 2);
    let both = EditTool::new()
        .execute(
            serde_json::json!({
                "path": "a.rs",
                "from": tag,
                "insert_after": tag,
                "new_string": "nope"
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(both.is_error, "{}", both.content);
    assert!(both.content.contains("not both"), "{}", both.content);

    let amb = EditTool::new()
        .execute(
            serde_json::json!({
                "path": "a.rs",
                "from": tag,
                "new_string": "    let y = 2;\n"
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(amb.is_error, "{}", amb.content);
    assert!(amb.content.contains("matches"), "{}", amb.content);

    let close = crate::file::line_tag::tag_of("}", 2);
    let order = EditTool::new()
        .execute(
            serde_json::json!({
                "path": "a.rs",
                "from": close,
                "to": crate::file::line_tag::tag_of("fn run() {", 2),
                "new_string": "x"
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(order.is_error, "{}", order.content);
    assert!(
        order.content.contains("must not precede"),
        "{}",
        order.content
    );

    std::fs::write(&path, "").unwrap();
    let empty = EditTool::new()
        .execute(
            serde_json::json!({
                "path": "a.rs",
                "from": "aa",
                "new_string": "x"
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(empty.is_error, "{}", empty.content);
    assert!(empty.content.contains("empty"), "{}", empty.content);

    std::fs::write(&path, "fn run() {}\n").unwrap();
    let missing_tag = EditTool::new()
        .execute(
            serde_json::json!({
                "path": "a.rs",
                "from": "zz",
                "new_string": "x"
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(missing_tag.is_error, "{}", missing_tag.content);
    assert!(
        missing_tag.content.contains("not found") || missing_tag.content.contains("Nearby"),
        "{}",
        missing_tag.content
    );
}

#[tokio::test]
async fn execute_stale_tag_does_not_write() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("a.rs");
    std::fs::write(&path, "fn run() {}\n").unwrap();
    let before = std::fs::read_to_string(&path).unwrap();
    let miss = EditTool::new()
        .execute(
            serde_json::json!({
                "path": "a.rs",
                "from": "zz",
                "new_string": "nope"
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(miss.is_error, "{}", miss.content);
    assert!(miss.content.contains("was not found"), "{}", miss.content);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
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

#[test]
fn io_and_missing_from_tag_are_named() {
    assert_eq!(
        io_error_string(std::io::Error::other("disk full")),
        "disk full"
    );
    assert_eq!(
        display_error(std::io::Error::other("disk full")),
        "disk full"
    );
    let missing = require_from_tag(None).unwrap_err();
    assert!(missing.contains("from is required"), "{missing}");
    assert_eq!(require_from_tag(Some("abc")).unwrap(), "abc");
}

#[test]
fn preview_edit_shows_the_pending_diff_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.rs");
    std::fs::write(&file, "fn run() {\n    let x = 1;\n}\n").unwrap();
    let wd = dir.path().to_string_lossy().into_owned();

    let by_text = preview_edit(
        &serde_json::json!({"path": "a.rs", "old_string": "let x = 1;", "new_string": "let x = 2;"}),
        &wd,
    )
    .unwrap();
    assert!(by_text.starts_with("Edit a.rs\n"), "{by_text}");
    assert!(
        by_text.contains("2|-let x = 1;\n   2|+let x = 2;"),
        "{by_text}"
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "fn run() {\n    let x = 1;\n}\n"
    );

    let tag = crate::file::line_tag::tag_of("    let x = 1;", 2);
    let abs = file.to_string_lossy().into_owned();
    let by_tag = preview_edit(
        &serde_json::json!({"path": abs, "from": tag, "new_string": "    let x = 3;"}),
        &wd,
    )
    .unwrap();
    assert!(by_tag.contains("+    let x = 3;"), "{by_tag}");

    for args in [
        serde_json::json!({"path": "a.rs", "old_string": "missing", "new_string": "x"}),
        serde_json::json!({"path": "gone.rs", "old_string": "x", "new_string": "y"}),
        serde_json::json!({"path": "a.rs", "old_string": "let x = 1;"}),
        serde_json::json!({"old_string": "x", "new_string": "y"}),
    ] {
        assert_eq!(preview_edit(&args, &wd), None, "{args}");
    }
}

#[test]
fn crlf_file_matches_lf_old_string_and_keeps_crlf() {
    let original = "fn run() {\r\n    let x = 1;\r\n}\r\n";
    let target = EditTarget {
        old_string: "fn run() {\n    let x = 1;",
        replace_all: false,
        from: None,
        to: None,
        insert_after: None,
    };
    let plan = plan_edit(original, &target, "fn run() {\n    let x = 2;").unwrap();
    assert_eq!(plan.modified, "fn run() {\r\n    let x = 2;\r\n}\r\n");
    assert_eq!(plan.new_text, "fn run() {\r\n    let x = 2;");
}

#[test]
fn crlf_file_insert_after_tag_writes_crlf() {
    let original = "a\r\nb\r\n";
    let target = EditTarget {
        old_string: "",
        replace_all: false,
        from: None,
        to: None,
        insert_after: Some(&crate::file::line_tag::tag_of("a", 2)),
    };
    let plan = plan_edit(original, &target, "x\ny").unwrap();
    assert_eq!(plan.modified, "a\r\nx\r\ny\r\nb\r\n");
}

#[test]
fn typographic_punctuation_matches_ascii_both_ways() {
    let file = "let s = \u{201C}it\u{2019}s\u{201D}; // a \u{2014} b\n";
    match locate_spans(file, "\"it's\"", false) {
        Locate::Hits(spans) => {
            assert_eq!(&file[spans[0].0..spans[0].1], "\u{201C}it\u{2019}s\u{201D}");
        }
        _ => panic!("expected folded hit"),
    }
    let ascii = "x = 'a' - b\n";
    match locate_spans(ascii, "\u{2018}a\u{2019} \u{2013} b", false) {
        Locate::Hits(spans) => assert_eq!(&ascii[spans[0].0..spans[0].1], "'a' - b"),
        _ => panic!("expected folded hit"),
    }
}

#[test]
fn folded_match_also_tolerates_whitespace() {
    let file = "say(\u{201C}hi\u{201D},    name)\n";
    match locate_spans(file, "say(\"hi\", name)", false) {
        Locate::Hits(spans) => {
            assert_eq!(
                &file[spans[0].0..spans[0].1],
                "say(\u{201C}hi\u{201D},    name)"
            );
        }
        _ => panic!("expected folded whitespace-tolerant hit"),
    }
    assert!(matches!(
        locate_spans(file, "nothing here", false),
        Locate::None
    ));
}

const NOTEBOOK: &str = "{\n \"cells\": [\n  {\n   \"cell_type\": \"code\",\n   \"execution_count\": 1,\n   \"metadata\": {},\n   \"outputs\": [\n    {\n     \"output_type\": \"stream\"\n    }\n   ],\n   \"source\": [\n    \"x = 1\"\n   ]\n  }\n ],\n \"metadata\": {},\n \"nbformat\": 4,\n \"nbformat_minor\": 4\n}\n";

#[tokio::test]
async fn execute_edits_a_notebook_through_its_rendering() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("n.ipynb");
    std::fs::write(&path, NOTEBOOK).unwrap();
    let args = json!({"path": "n.ipynb", "old_string": "x = 1", "new_string": "x = 2"});
    let preview = preview_edit(&args, &dir.path().to_string_lossy()).unwrap();
    assert!(preview.contains("x = 2"), "{preview}");
    let result = EditTool::new().execute(args, &ctx(dir.path())).await;
    assert!(!result.is_error, "{}", result.content);
    let expected = NOTEBOOK
        .replace("\"execution_count\": 1", "\"execution_count\": null")
        .replace(
            "[\n    {\n     \"output_type\": \"stream\"\n    }\n   ]",
            "[]",
        )
        .replace("x = 1", "x = 2");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
}

#[tokio::test]
async fn execute_refuses_a_notebook_edit_that_damages_markers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("n.ipynb");
    std::fs::write(&path, NOTEBOOK).unwrap();
    let args = json!({"path": "n.ipynb", "old_string": "cell:1", "new_string": "cell:7"});
    assert!(preview_edit(&args, &dir.path().to_string_lossy()).is_none());
    let result = EditTool::new().execute(args, &ctx(dir.path())).await;
    assert!(result.is_error);
    assert!(result.content.contains("`cell:7`"), "{}", result.content);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), NOTEBOOK);
}
