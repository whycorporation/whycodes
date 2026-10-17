use super::*;
use std::fs;
use tempfile::TempDir;

fn ctx(dir: &std::path::Path) -> ToolContext {
    ToolContext::new(dir.to_string_lossy().into_owned())
}

fn project() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/a.rs"),
        "fn main() {\n    let v = a.unwrap();\n    b.unwrap();\n}\n",
    )
    .unwrap();
    fs::write(dir.path().join("b.py"), "x = foo.unwrap()\n").unwrap();
    // Not UTF-8: parsed as no file at all.
    fs::write(dir.path().join("c.rs"), [0xff, 0xfe]).unwrap();
    dir
}

#[test]
fn schema_names_the_tool() {
    let t = AstGrepTool;
    assert_eq!(t.name(), "ast_grep");
    assert!(t.description().contains("$$$"));
    let p = t.parameters();
    assert_eq!(p["required"], serde_json::json!(["pattern"]));
    assert!(
        p["properties"]["language"]["description"]
            .as_str()
            .unwrap()
            .contains("tsx")
    );
}

#[tokio::test]
async fn hits_carry_tags_and_captures() {
    let dir = project();
    let out = AstGrepTool
        .execute(
            serde_json::json!({"pattern": "$X.unwrap()"}),
            &ctx(dir.path()),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    let line = "    let v = a.unwrap();";
    let tag = tag_of(line, MIN_LEN);
    assert!(
        out.content
            .contains(&format!("src/a.rs:2 {tag}:{line}\n    $X = a")),
        "{}",
        out.content
    );
    assert!(out.content.contains("$X = b"), "{}", out.content);
    assert!(out.content.contains("b.py:1 "), "{}", out.content);
    assert!(out.content.contains("shown 3 of 3"), "{}", out.content);
    assert!(
        out.content.contains("3 files parsed (rust, python)"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn skipped_language_cap_and_single_file() {
    let dir = project();
    // `?` is not Python: only Rust files are searched.
    let out = AstGrepTool
        .execute(
            serde_json::json!({"pattern": "$X.unwrap()", "language": "rust", "max_results": 1}),
            &ctx(dir.path()),
        )
        .await;
    assert!(out.content.contains("shown 1 (cap hit"), "{}", out.content);
    assert!(out.content.contains("next: raise max_results"));

    let out = AstGrepTool
        .execute(
            serde_json::json!({"pattern": "$X.unwrap()?"}),
            &ctx(dir.path()),
        )
        .await;
    assert!(
        out.content.starts_with("No matches found."),
        "{}",
        out.content
    );
    assert!(out.content.contains("(rust)"), "{}", out.content);

    let out = AstGrepTool
        .execute(
            serde_json::json!({"pattern": "foo.unwrap()", "path": "b.py"}),
            &ctx(dir.path()),
        )
        .await;
    assert!(
        out.content.contains("1 file parsed (python)"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn errors_and_empty_trees() {
    let dir = project();
    let out = AstGrepTool
        .execute(serde_json::json!({}), &ctx(dir.path()))
        .await;
    assert!(out.is_error && out.content.starts_with("Error: Missing"));

    let empty = TempDir::new().unwrap();
    fs::write(empty.path().join("notes.md"), "# x").unwrap();
    let out = AstGrepTool
        .execute(serde_json::json!({"pattern": "x"}), &ctx(empty.path()))
        .await;
    assert!(!out.is_error);
    assert!(
        out.content.contains("0 files parsed (no supported files)"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn repeated_metavariable_must_match_the_same_code() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("m.go"),
        "package m\nfunc f(a, b int) bool { return a == a || a == b }\n",
    )
    .unwrap();
    let out = AstGrepTool
        .execute(serde_json::json!({"pattern": "$A == $A"}), &ctx(dir.path()))
        .await;
    assert!(out.content.contains("shown 1 of 1"), "{}", out.content);
    assert!(out.content.contains("$A = a"), "{}", out.content);
}
