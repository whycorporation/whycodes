use super::*;
use crate::tool::ToolContext;
use std::path::Path;

fn ctx(dir: &std::path::Path) -> ToolContext {
    ToolContext::new(dir.to_string_lossy().into_owned())
}

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(path, body).expect("write");
}

#[tokio::test]
async fn metadata_describes_repomap() {
    let t = RepoMapTool::default();
    assert_eq!(t.name(), "repomap");
    assert!(t.description().to_ascii_lowercase().contains("signature"));
    let params = t.parameters();
    assert!(params["properties"]["path"].is_object());
    assert!(params["properties"]["max_tokens"].is_object());
}

#[tokio::test]
async fn missing_root_is_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = RepoMapTool::new()
        .execute(json!({ "path": "nope" }), &ctx(dir.path()))
        .await;
    assert!(out.is_error);
    assert!(out.content.contains("Path not found"), "{}", out.content);
    let file = dir.path().join("solo.rs");
    std::fs::write(&file, "fn solo() {}\n").unwrap();
    let mapped = RepoMapTool::new()
        .execute(json!({ "path": file.to_string_lossy() }), &ctx(dir.path()))
        .await;
    assert!(!mapped.is_error, "{}", mapped.content);
    assert!(
        mapped.content.contains("fn solo") || mapped.content.contains("solo.rs"),
        "{}",
        mapped.content
    );
    assert!(skip_repomap_file(Path::new("/nonexistent-xyz")));
    assert!(read_repomap_text(Path::new("/nonexistent-xyz")).is_none());
    assert!(file_block_from(Path::new("/nonexistent-xyz"), "x.rs".into()).is_none());
    assert!(!collect_from_index(None, Path::new("."), &mut |_, _| true));
    assert!(visit_repomap_entry(
        Path::new("."),
        ".",
        true,
        &mut |_, _| false
    ));
    assert!(!visit_repomap_entry(
        Path::new("."),
        ".",
        false,
        &mut |_, _| false
    ));
    walk_repomap_files(Path::new("/nonexistent-xyz"), &mut |_, _| true);
    assert!(!skip_repomap_bytes(Path::new("/nonexistent-xyz")));
    let idx = whycodes_index::WorkspaceIndex::start_with(
        vec![dir.path().to_path_buf()],
        whycodes_index::IndexOptions {
            watch: false,
            threads: 1,
            ..Default::default()
        },
    );
    let _ = idx.wait_ready(std::time::Duration::from_secs(10));
    collect_from_index(Some(&idx), dir.path(), &mut |_, _| true);
    let bin = dir.path().join("skip.bin");
    std::fs::write(&bin, [0u8, 1, 2]).unwrap();
    assert!(skip_repomap_bytes(&bin));
    assert!(read_repomap_text(&bin).is_none());
    let empty = dir.path().join("empty.rs");
    std::fs::write(&empty, "// none\n").unwrap();
    assert!(file_block_from(&empty, "empty.rs".into()).is_none());
}

#[tokio::test]
async fn maps_rust_signatures_and_respects_budget() {
    let dir = tempfile::tempdir().expect("tempdir");
    write(
        dir.path(),
        "src/lib.rs",
        "pub fn greet(name: &str) -> String {\n    format!(\"hi {name}\")\n}\n\
         pub struct Person { pub name: String }\n\
         impl Person {\n    pub fn new(name: String) -> Self { Self { name } }\n}\n",
    );
    write(dir.path(), "src/skip.bin", "not source");
    write(
        dir.path(),
        "tests/extra.rs",
        "fn helper() {}\nfn other() {}\n",
    );

    let out = RepoMapTool::new()
        .execute(json!({ "max_tokens": 2000 }), &ctx(dir.path()))
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("# repomap"), "{}", out.content);
    assert!(out.content.contains("src/lib.rs"), "{}", out.content);
    assert!(out.content.contains("fn greet"), "{}", out.content);
    assert!(out.content.contains("struct Person"), "{}", out.content);
    assert!(
        !out.content.contains("format!"),
        "bodies must be omitted: {}",
        out.content
    );
    // src/ ranks ahead of tests/
    let src = out.content.find("src/lib.rs").expect("src");
    if let Some(tests) = out.content.find("tests/extra.rs") {
        assert!(src < tests, "{}", out.content);
    }
}

#[test]
fn pack_blocks_stops_at_budget() {
    let blocks: Vec<FileBlock> = (0..20)
        .map(|i| FileBlock {
            rel: format!("src/f{i}.rs"),
            sigs: vec!["fn quite_a_long_function_name_for_budget()".into(); 12],
        })
        .collect();
    let (body, used, omitted) = pack_blocks(&blocks, 500);
    assert!(!body.is_empty());
    assert!(used < 20, "used={used}");
    assert!(omitted > 0, "omitted={omitted}");
}

#[test]
fn rust_and_js_extractors() {
    let rust = extract_signatures(
        "/// docs\npub fn foo() {\n    1\n}\nimpl Bar {\n    fn baz(&self) {}\n}\n",
        "rs",
    );
    assert!(rust.iter().any(|s| s.contains("fn foo")), "{rust:?}");
    assert!(rust.iter().any(|s| s.contains("impl Bar")), "{rust:?}");
    assert!(rust.iter().all(|s| !s.contains("1")), "{rust:?}");

    let js = extract_signatures(
        "export function ping() { return 1 }\nconst x = 1;\nexport class Box {}\n",
        "ts",
    );
    assert!(js.iter().any(|s| s.contains("function ping")), "{js:?}");
    assert!(js.iter().any(|s| s.contains("class Box")), "{js:?}");
}

#[test]
fn rank_prefers_src_over_tests() {
    assert!(rank_key("src/lib.rs") < rank_key("tests/foo.rs"));
    assert!(rank_key("Cargo.toml") < rank_key("src/lib.rs"));
    assert_eq!(rank_key("pkg/src/foo.rs").0, 1);
    assert_eq!(rank_key("foo_test.rs").0, 3);
    assert_eq!(rank_key("pkg/foo_test.rs").0, 3);
    assert_eq!(rank_key("README.md").0, 2);
    assert_eq!(render_repomap("# h", ""), "# h\n(no signatures in scope)");
    assert!(is_signature_line("func Foo() {", "swift"));
    assert!(is_signature_line("public void Foo() {", "java"));
    assert!(is_signature_line("export class Foo {", "cs"));
    assert!(is_signature_line("enum Foo(int x) {", "c"));
    assert!(is_signature_line("typedef struct Foo(int x) {", "h"));
    assert!(is_signature_line("namespace Foo(int x) {", "cpp"));
    let many = (0..MAX_SIGS_PER_FILE + 3)
        .map(|i| format!("fn f{i}() {{}}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(extract_signatures(&many, "rs").len(), MAX_SIGS_PER_FILE);
    assert!(
        extract_signatures("/* comment\n * still\nfn foo() {}\n", "rs")
            .iter()
            .any(|s| s.contains("fn foo"))
    );
}

#[test]
fn signature_line_covers_remaining_languages() {
    assert!(is_signature_line("def foo():", "py"));
    assert!(is_signature_line("async def bar():", "py"));
    assert!(is_signature_line("class C:", "py"));
    assert!(!is_signature_line("x = 1", "py"));

    assert!(is_signature_line("func main() {", "go"));
    assert!(is_signature_line("type T struct {", "go"));
    assert!(is_signature_line("package main", "go"));

    assert!(is_signature_line("public class Foo {", "java"));
    assert!(is_signature_line("interface Bar {", "kt"));
    assert!(is_signature_line("enum Kind {", "cs"));
    assert!(is_signature_line("fun baz() {", "kts"));

    assert!(is_signature_line("struct Foo(int x) {", "c"));
    assert!(is_signature_line("class Bar(int x) {", "cpp"));
    assert!(!is_signature_line("int x;", "h"));
    assert!(!is_signature_line("if (x) {", "cc"));

    assert!(is_signature_line("[package]", "toml"));
    assert!(!is_signature_line("name = \"x\"", "toml"));
    assert!(is_signature_line("# Title", "md"));
    assert!(is_signature_line("## Sub", "md"));
    assert!(!is_signature_line("#### Deep", "md"));
    assert!(!is_signature_line("#NoSpace", "md"));
    assert!(!is_signature_line("anything", "xyz"));
    assert!(is_signature_line("export Foo", "swift"));

    assert!(!is_source_path(Path::new("README")));
    assert!(is_source_path(Path::new("src/lib.rs")));
    assert!(!is_source_path(Path::new("blob.bin")));

    let short = truncate_sig("fn foo() {");
    assert_eq!(short, "fn foo()");
    let long = "a".repeat(200);
    let cut = truncate_sig(&long);
    assert_eq!(cut.chars().count(), 120);
    assert!(cut.ends_with('…'));
}
