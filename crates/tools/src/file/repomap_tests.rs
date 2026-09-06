use super::*;
use crate::tool::ToolContext;

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
    let t = RepoMapTool::new();
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
}
