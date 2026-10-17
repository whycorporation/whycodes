use super::*;
use std::fs;
use tempfile::TempDir;

fn ctx(dir: &std::path::Path) -> ToolContext {
    ToolContext::new(dir.to_string_lossy().into_owned())
}

fn tool(format: bool) -> AstEditTool {
    AstEditTool::new(Arc::new(AtomicBool::new(format)))
}

const SRC: &str = "fn main() {\n    let v = a.unwrap();\n    b.unwrap();\n}\n";

fn project() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a.rs"), SRC).unwrap();
    fs::write(dir.path().join("b.py"), "x = 1\n").unwrap();
    dir
}

/// Run `f` while holding `ENV_LOCK` (sibling tests empty `PATH`; the
/// formatter runs `rustfmt` from `PATH`).
fn with_env_lock<F: std::future::Future>(f: F) -> F::Output {
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

#[test]
fn schema_names_the_tool() {
    let t = tool(true);
    assert_eq!(t.name(), "ast_edit");
    assert!(t.description().contains("dry_run: false"));
    let p = t.parameters();
    assert_eq!(p["required"], serde_json::json!(["pattern", "rewrite"]));
    assert!(p["properties"]["dry_run"].is_object());
    assert!(
        p["properties"]["language"]["description"]
            .as_str()
            .unwrap()
            .contains("go")
    );
}

#[tokio::test]
async fn dry_run_is_the_default_and_writes_nothing() {
    let dir = project();
    let out = tool(true)
        .execute(
            serde_json::json!({"pattern": "$X.unwrap()", "rewrite": "$X.expect(\"x\")"}),
            &ctx(dir.path()),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(
        out.content.starts_with(
            "ast_edit · pattern `$X.unwrap()` → `$X.expect(\"x\")` · 2 replacements in 1 file · dry run, nothing written\n--- a/a.rs\n+++ b/a.rs\n@@ -1,4 +1,4 @@\n"
        ),
        "{}",
        out.content
    );
    assert!(
        out.content
            .contains("-    b.unwrap();\n+    b.expect(\"x\");\n")
    );
    assert!(
        out.content
            .ends_with("dry_run: false to write these changes")
    );
    assert_eq!(fs::read_to_string(dir.path().join("a.rs")).unwrap(), SRC);
}

#[tokio::test]
async fn writes_when_dry_run_is_false() {
    let dir = project();
    let out = tool(false)
        .execute(
            serde_json::json!({
                "pattern": "let $V = $X.unwrap();",
                "rewrite": "let $V = $X?;",
                "language": "rust",
                "dry_run": false
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(
        out.content.contains("1 replacement in 1 file · 1 written"),
        "{}",
        out.content
    );
    assert!(out.content.ends_with("\nwrote a.rs"), "{}", out.content);
    assert_eq!(
        fs::read_to_string(dir.path().join("a.rs")).unwrap(),
        "fn main() {\n    let v = a?;\n    b.unwrap();\n}\n"
    );
}

#[test]
fn written_files_run_the_project_formatter() {
    with_env_lock(written_files_run_the_project_formatter_body());
}

async fn written_files_run_the_project_formatter_body() {
    let dir = project();
    fs::write(dir.path().join("rustfmt.toml"), "edition = \"2024\"\n").unwrap();
    fs::write(dir.path().join("c.rs"), "fn c() { z.unwrap(); }\n").unwrap();
    let out = tool(true)
        .execute(
            serde_json::json!({
                "pattern": "$X.unwrap()",
                "rewrite": "$X  .  unwrap_or_default()",
                "dry_run": false
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    let formatted = out.content.matches("Formatted with rustfmt").count();
    assert_eq!(formatted, 1, "one note for both files: {}", out.content);
    assert_eq!(
        fs::read_to_string(dir.path().join("c.rs")).unwrap(),
        "fn c() {\n    z.unwrap_or_default();\n}\n"
    );
}

#[tokio::test]
async fn nothing_to_change_and_errors() {
    let dir = project();
    let c = ctx(dir.path());
    let same = tool(true)
        .execute(
            serde_json::json!({"pattern": "$X.unwrap()", "rewrite": "$X.unwrap()"}),
            &c,
        )
        .await;
    assert!(!same.is_error);
    assert!(
        same.content.starts_with("No changes.") && same.content.contains("2 files parsed"),
        "{}",
        same.content
    );
    let one = tool(true)
        .execute(
            serde_json::json!({"pattern": "nope()", "rewrite": "x", "path": "a.rs"}),
            &c,
        )
        .await;
    assert!(one.content.contains("1 file parsed"), "{}", one.content);

    let missing = tool(true)
        .execute(serde_json::json!({"pattern": "$X.unwrap()"}), &c)
        .await;
    assert!(missing.is_error && missing.content.contains("`rewrite`"));
    let bad = tool(true)
        .execute(serde_json::json!({"rewrite": "x"}), &c)
        .await;
    assert!(bad.is_error && bad.content.starts_with("Error: Missing"));

    let leak = "x\nanalysis to=functions.edit code \u{8d4c}\u{535a}\u{7f51}\u{7ad9}\u{63a8}\u{8350}\u{4ee3}\u{7406}";
    let refused = tool(true)
        .execute(
            serde_json::json!({"pattern": "$X.unwrap()", "rewrite": leak, "dry_run": false}),
            &c,
        )
        .await;
    assert!(refused.is_error && refused.content.contains("Harmony"));
    assert_eq!(fs::read_to_string(dir.path().join("a.rs")).unwrap(), SRC);
}

#[tokio::test]
async fn claimed_file_is_reported_not_written() {
    let dir = project();
    let path = dir.path().join("a.rs");
    let claims = whycodes_core::file_claims::FileClaimRegistry::new();
    claims.try_claim("other", "other-agent", &path);
    let mut c = ctx(dir.path());
    c.file_claims = Some(claims);
    c.agent_id = Some("me".into());
    let out = tool(false)
        .execute(
            serde_json::json!({"pattern": "$X.unwrap()", "rewrite": "$X", "dry_run": false}),
            &c,
        )
        .await;
    assert!(out.is_error);
    assert!(out.content.contains("0 written"), "{}", out.content);
    assert!(
        out.content.contains("not written:\na.rs: File conflict"),
        "{}",
        out.content
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), SRC);
}

#[test]
fn io_errors_render_as_text() {
    let e = std::io::Error::other("disk full");
    assert_eq!(io_error(e), "disk full");
}
