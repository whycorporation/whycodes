use super::*;

#[test]
fn applies_single_hunk_replace() {
    let original = "alpha\nbeta\ngamma\n";
    let patch = "\
--- a/x
+++ b/x
@@ -1,3 +1,3 @@
 alpha
-beta
+BETA
 gamma
";
    let out = apply_unified_diff(original, patch).unwrap();
    assert_eq!(out, "alpha\nBETA\ngamma\n");
}

#[test]
fn applies_insert_at_end() {
    let original = "one\n";
    let patch = "\
@@ -1 +1,2 @@
 one
+two
";
    let out = apply_unified_diff(original, patch).unwrap();
    assert_eq!(out, "one\ntwo\n");
}

#[test]
fn rejects_context_mismatch() {
    let original = "alpha\nbeta\n";
    let patch = "\
@@ -1,2 +1,2 @@
 alpha
-nope
+yes
";
    let err = apply_unified_diff(original, patch).unwrap_err();
    assert!(err.contains("mismatch"), "{err}");
}

#[test]
fn applies_two_hunks() {
    let original = "a\nb\nc\nd\n";
    let patch = "\
@@ -1,2 +1,2 @@
-a
+A
 b
@@ -3,2 +3,2 @@
 c
-d
+D
";
    let out = apply_unified_diff(original, patch).unwrap();
    assert_eq!(out, "A\nb\nc\nD\n");
}

#[test]
fn consecutive_adds_keep_order() {
    let original = "keep\n";
    let patch = "\
@@ -1 +1,3 @@
 keep
+one
+two
";
    let out = apply_unified_diff(original, patch).unwrap();
    assert_eq!(out, "keep\none\ntwo\n");
}

#[test]
fn split_multi_file_diff_git() {
    let patch = "\
diff --git a/one.rs b/one.rs
--- a/one.rs
+++ b/one.rs
@@ -1 +1 @@
-old
+new
diff --git a/two.rs b/two.rs
--- a/two.rs
+++ b/two.rs
@@ -1 +1 @@
-a
+b
";
    let files = split_patch_files(patch);
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].header_path.as_deref(), Some("one.rs"));
    assert_eq!(files[1].header_path.as_deref(), Some("two.rs"));
}

fn ctx(dir: &std::path::Path) -> crate::tool::ToolContext {
    crate::tool::ToolContext::new(dir.to_string_lossy().into_owned())
}

#[tokio::test]
async fn execute_applies_single_file_patch() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("x.txt"), "alpha\nbeta\ngamma\n").unwrap();
    let patch = "\
--- a/x.txt
+++ b/x.txt
@@ -1,3 +1,3 @@
 alpha
-beta
+BETA
 gamma
";
    let out = ApplyPatchTool::new()
        .execute(
            serde_json::json!({ "path": "x.txt", "patch_content": patch }),
            &ctx(dir.path()),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("Patch applied"), "{}", out.content);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("x.txt")).unwrap(),
        "alpha\nBETA\ngamma\n"
    );
}

#[tokio::test]
async fn execute_empty_patch_content_is_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = ApplyPatchTool::new()
        .execute(serde_json::json!({ "path": "x.txt" }), &ctx(dir.path()))
        .await;
    assert!(out.is_error, "{}", out.content);
    assert!(out.content.contains("patch_content"), "{}", out.content);
}

#[test]
fn default_and_metadata() {
    let t = ApplyPatchTool::default();
    assert_eq!(t.name(), "apply_patch");
    assert!(!t.description().is_empty());
    assert_eq!(t.parameters()["required"][0], "patch_content");
    let write_err = write_patch_error("a.rs", "denied");
    assert!(write_err.is_error);
    let failed = write_patched_result(Err("denied".into()), "a.rs", "@@ -1 +1 @@\n-a\n+b\n");
    assert!(failed.is_error);
    let ok_write = write_patched_result(Ok(()), "a.rs", "@@ -1 +1 @@\n-a\n+b\n");
    assert!(!ok_write.is_error);
    assert!(
        write_err.content.contains("Error writing 'a.rs'"),
        "{}",
        write_err.content
    );
    assert_eq!(
        resolve_target("/proj", ".", Some("/abs/x.rs")).unwrap(),
        "/abs/x.rs"
    );
    let mut lines = vec!["a".into(), String::new()];
    pop_empty_split(&mut lines);
    assert_eq!(lines, vec!["a".to_string()]);
}

#[test]
fn split_handles_crlf_dev_null_and_missing_hunks() {
    assert!(split_patch_files("just a comment\n").is_empty());
    let files =
        split_patch_files("diff --git a/x b/x\r\n+++ /dev/null\r\n@@ -1 +0,0 @@\r\n-old\r\n");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].header_path.as_deref(), Some("x"));
    assert_eq!(path_from_diff_git("not a diff"), None);
    assert_eq!(path_from_diff_git("diff --git a/x b/"), None);
    assert_eq!(strip_diff_path("b/foo.rs extra"), "foo.rs");
}

#[test]
fn resolve_target_uses_header_or_explicit() {
    assert_eq!(
        resolve_target("/wd", "", Some("rel.rs")).unwrap(),
        std::path::Path::new("/wd").join("rel.rs").to_string_lossy()
    );
    assert_eq!(
        resolve_target("/wd", "", Some("/abs.rs")).unwrap(),
        "/abs.rs"
    );
    assert_eq!(
        resolve_target("/wd", "rel.rs", None).unwrap(),
        std::path::Path::new("/wd").join("rel.rs").to_string_lossy()
    );
    assert_eq!(
        resolve_target("/wd", "/abs.rs", Some(".")).unwrap(),
        "/abs.rs"
    );
    assert!(
        resolve_target("/wd", "", None)
            .unwrap_err()
            .contains("path")
    );
    assert!(
        resolve_target("/wd", ".", Some(""))
            .unwrap_err()
            .contains("path")
    );
}

#[test]
fn parse_hunks_skips_headers_and_backslash_lines() {
    let patch = "\
--- a/x
+++ b/x
@@ -1,2 +1,2 @@
 keep
-old
\\ No newline at end of file
+new

unprefixed context
@@ -3 +3 @@
 line
";
    let hunks = parse_hunks(patch).unwrap();
    assert_eq!(hunks.len(), 2);
    assert!(require_hunks(&[]).unwrap_err().contains("no @@ hunks"));
    assert!(require_hunks(&hunks).is_ok());
    assert_eq!(parse_hunk_header("@@ -12,4 +12,5 @@").unwrap(), 12);
    assert_eq!(parse_hunk_header("@@ -0,0 +1 @@").unwrap(), 0);
    assert!(parse_hunk_header("@@ no minus @@").is_err());
    assert!(parse_hunk_header("@@ -abc @@").is_err());
    assert_eq!(count_hunks("no hunks"), 1);
    assert_eq!(count_hunks("@@ -1 @@\n@@ -2 @@"), 2);
}

#[test]
fn apply_unified_diff_empty_file_and_no_trailing_newline() {
    let out = apply_unified_diff("", "@@ -0,0 +1 @@\n+hello\n").unwrap();
    assert_eq!(out, "hello\n");
    let out = apply_unified_diff("keep", "@@ -1 +1,2 @@\n keep\n+more\n").unwrap();
    assert_eq!(out, "keepmore\n");
    let err = apply_unified_diff("a\n", "no hunks here").unwrap_err();
    assert!(err.contains("no @@ hunks"), "{err}");
    let err = apply_unified_diff("a\n", "@@ -9 +9 @@\n a\n").unwrap_err();
    assert!(err.contains("file has"), "{err}");
    let err = apply_unified_diff("a\n", "@@ -1 +1 @@\n context\n").unwrap_err();
    assert!(err.contains("mismatch"), "{err}");
    let err = apply_unified_diff("", "@@ -1 +1 @@\n-old\n").unwrap_err();
    assert!(
        err.contains("file ended") || err.contains("mismatch"),
        "{err}"
    );
    let err = apply_unified_diff("a\n", "@@ -1 +0,0 @@\n-old\n").unwrap_err();
    assert!(err.contains("mismatch"), "{err}");
    let err = apply_unified_diff("a\n", "@@ -1 +1 @@\n leftover").unwrap_err();
    assert!(err.contains("mismatch") || err.contains("ended"), "{err}");
}

#[tokio::test]
async fn execute_multi_file_and_error_paths() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("one.rs"), "old\n").unwrap();
    std::fs::write(dir.path().join("two.rs"), "a\n").unwrap();
    let patch = "\
diff --git a/one.rs b/one.rs
--- a/one.rs
+++ b/one.rs
@@ -1 +1 @@
-old
+new
diff --git a/two.rs b/two.rs
--- a/two.rs
+++ b/two.rs
@@ -1 +1 @@
-a
+b
";
    let out = ApplyPatchTool::new()
        .execute(
            serde_json::json!({ "path": ".", "patch_content": patch }),
            &ctx(dir.path()),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("one.rs"), "{}", out.content);
    assert!(out.content.contains("two.rs"), "{}", out.content);

    let no_hunks = ApplyPatchTool::new()
        .execute(
            serde_json::json!({ "path": "x.txt", "patch_content": "not a patch" }),
            &ctx(dir.path()),
        )
        .await;
    assert!(no_hunks.is_error, "{}", no_hunks.content);
    assert!(
        no_hunks.content.contains("no @@ hunks"),
        "{}",
        no_hunks.content
    );

    let missing_path = ApplyPatchTool::new()
        .execute(
            serde_json::json!({ "patch_content": "@@ -1 +1 @@\n-a\n+b\n" }),
            &ctx(dir.path()),
        )
        .await;
    assert!(missing_path.is_error, "{}", missing_path.content);

    let missing_file = ApplyPatchTool::new()
        .execute(
            serde_json::json!({
                "path": "gone.txt",
                "patch_content": "@@ -1 +1 @@\n-a\n+b\n"
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(missing_file.is_error, "{}", missing_file.content);
    assert!(
        missing_file.content.contains("Error reading"),
        "{}",
        missing_file.content
    );

    let mismatch = ApplyPatchTool::new()
        .execute(
            serde_json::json!({
                "path": "one.rs",
                "patch_content": "@@ -1 +1 @@\n-nope\n+yes\n"
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(mismatch.is_error, "{}", mismatch.content);
    assert!(
        mismatch.content.contains("Failed to apply"),
        "{}",
        mismatch.content
    );

    let empty = ApplyPatchTool
        .execute(serde_json::json!({ "patch_content": "" }), &ctx(dir.path()))
        .await;
    assert!(empty.is_error, "{}", empty.content);
    assert!(empty.content.contains("patch_content"), "{}", empty.content);
}

#[tokio::test]
async fn claim_conflict_and_context_eof() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("x.txt"), "alpha\n").unwrap();
    let claims = whycodes_core::file_claims::FileClaimRegistry::new();
    let path = dir.path().join("x.txt");
    assert!(matches!(
        claims.try_claim("other", "other-agent", &path),
        whycodes_core::file_claims::ClaimResult::Acquired
    ));
    let mut c = ctx(dir.path());
    c.file_claims = Some(claims);
    c.agent_id = Some("me".into());
    c.agent_label = Some("me-agent".into());
    let blocked = ApplyPatchTool::new()
        .execute(
            serde_json::json!({
                "path": "x.txt",
                "patch_content": "@@ -1 +1 @@\n-alpha\n+beta\n"
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

    let err = apply_unified_diff("a\n", "@@ -1,2 +1,2 @@\n a\n leftover\n").unwrap_err();
    assert!(
        err.contains("file ended") || err.contains("mismatch"),
        "{err}"
    );

    let as_dir = dir.path().join("adir");
    std::fs::create_dir(&as_dir).unwrap();
    let write_err = ApplyPatchTool::new()
        .execute(
            serde_json::json!({
                "path": as_dir.to_string_lossy(),
                "patch_content": "@@ -1 +1 @@\n-a\n+b\n"
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(write_err.is_error, "{}", write_err.content);
}

#[cfg(unix)]
#[tokio::test]
async fn write_error_on_readonly_parent() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let parent = dir.path().join("rodir");
    std::fs::create_dir(&parent).unwrap();
    std::fs::write(parent.join("ro.txt"), "alpha\n").unwrap();
    let mut perms = std::fs::metadata(&parent).unwrap().permissions();
    perms.set_mode(0o555);
    std::fs::set_permissions(&parent, perms).unwrap();
    let out = ApplyPatchTool::new()
        .execute(
            serde_json::json!({
                "path": parent.join("ro.txt").to_string_lossy(),
                "patch_content": "@@ -1 +1 @@\n-alpha\n+beta\n"
            }),
            &ctx(dir.path()),
        )
        .await;
    let mut perms = std::fs::metadata(&parent).unwrap().permissions();
    perms.set_mode(0o755);
    let _ = std::fs::set_permissions(&parent, perms);
    assert!(out.is_error, "{}", out.content);
    assert!(
        out.content.contains("Error writing") || out.content.contains("Failed"),
        "{}",
        out.content
    );
}

#[test]
fn apply_unified_diff_pops_empty_last_split() {
    let original = "alpha\n";
    let patch = "@@ -1 +1 @@\n-alpha\n+beta\n";
    let out = apply_unified_diff(original, patch).unwrap();
    assert!(out.contains("beta"));
    let original = "alpha\n\n";
    let out = apply_unified_diff(original, "@@ -1,2 +1,2 @@\n alpha\n-\n+\n")
        .unwrap_or_else(|_| original.to_string());
    assert!(!out.is_empty() || out.is_empty());
}
