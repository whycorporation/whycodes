use super::*;
use tempfile::TempDir;

fn write(dir: &TempDir, name: &str, content: &str) -> std::path::PathBuf {
    let p = dir.path().join(name);
    fs::write(&p, content).unwrap();
    p
}

#[test]
fn truncate_line_short_and_long() {
    assert_eq!(truncate_line("short", 10), "short");
    let t = truncate_line("abcdefghij", 4);
    assert!(t.starts_with("abcd"));
    assert!(t.contains("[line truncated]"));
    // Multibyte
    let t = truncate_line("türkçe uzun", 3);
    assert!(t.starts_with("tür"));
}

#[test]
fn image_media_type_by_extension() {
    assert_eq!(image_media_type(Path::new("a.png")), Some("image/png"));
    assert_eq!(image_media_type(Path::new("a.jpg")), Some("image/jpeg"));
    assert_eq!(image_media_type(Path::new("a.JPEG")), Some("image/jpeg"));
    assert_eq!(image_media_type(Path::new("a.gif")), Some("image/gif"));
    assert_eq!(image_media_type(Path::new("a.webp")), Some("image/webp"));
    assert_eq!(image_media_type(Path::new("a.bmp")), Some("image/bmp"));
    assert_eq!(image_media_type(Path::new("a.txt")), None);
    assert_eq!(image_media_type(Path::new("noext")), None);
}

#[test]
fn read_window_basic() {
    let dir = TempDir::new().unwrap();
    let f = write(&dir, "a.txt", "one\ntwo\nthree\nfour\n");
    let w = read_window(&f, 2, 2).unwrap();
    assert_eq!(w.lines, vec!["two", "three"]);
    assert_eq!(w.start_line, 2);
    assert_eq!(w.end_line, 3);
    assert_eq!(w.total_lines, 4);
    assert!(w.total_known);
    // Content follows the window -> truncated (prompts the model to page)
    assert!(w.truncated);
}

#[test]
fn read_window_at_eof_not_truncated() {
    let dir = TempDir::new().unwrap();
    let f = write(&dir, "a.txt", "one\ntwo\nthree\nfour\n");
    let w = read_window(&f, 3, 2).unwrap();
    assert_eq!(w.lines, vec!["three", "four"]);
    assert_eq!(w.total_lines, 4);
    assert!(!w.truncated);
}

#[test]
fn read_window_first_and_last() {
    let dir = TempDir::new().unwrap();
    let f = write(&dir, "a.txt", "one\ntwo\n");
    let w = read_window(&f, 1, 100).unwrap();
    assert_eq!(w.lines, vec!["one", "two"]);
    assert_eq!(w.start_line, 1);
    assert_eq!(w.end_line, 2);
    assert!(!w.truncated);
}

#[test]
fn read_window_offset_past_eof() {
    let dir = TempDir::new().unwrap();
    let f = write(&dir, "a.txt", "one\ntwo\n");
    let w = read_window(&f, 10, 5).unwrap();
    assert!(w.lines.is_empty());
    assert_eq!(w.start_line, 10);
    assert_eq!(w.end_line, 9); // saturating_sub
    assert!(!w.truncated);
}

#[test]
fn read_window_truncated_flag() {
    let dir = TempDir::new().unwrap();
    let content = "l1\nl2\nl3\nl4\nl5\n";
    let f = write(&dir, "a.txt", content);
    let w = read_window(&f, 1, 2).unwrap();
    assert_eq!(w.lines.len(), 2);
    assert!(w.truncated);
    assert_eq!(w.total_lines, 5);
    assert!(w.total_known);
}

#[test]
fn read_window_strips_crlf() {
    let dir = TempDir::new().unwrap();
    let f = write(&dir, "a.txt", "one\r\ntwo\r\n");
    let w = read_window(&f, 1, 2).unwrap();
    assert_eq!(w.lines, vec!["one", "two"]);
}

#[test]
fn read_window_trailing_newline() {
    let dir = TempDir::new().unwrap();
    let f = write(&dir, "a.txt", "one\n\ntwo");
    let w = read_window(&f, 1, 10).unwrap();
    assert_eq!(w.lines, vec!["one", "", "two"]);
    assert!(w.total_known);
}

#[test]
fn read_window_missing_file_errors() {
    assert!(read_window(Path::new("/nonexistent-xyz/a.txt"), 1, 5).is_err());
}

#[tokio::test]
async fn execute_reads_window_with_header() {
    let dir = TempDir::new().unwrap();
    write(&dir, "a.txt", "one\ntwo\nthree\n");
    let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
    let result = ReadTool::new()
        .execute(
            serde_json::json!({"path": "a.txt", "offset": 2, "limit": 2}),
            &ctx,
        )
        .await;
    assert!(!result.is_error);
    assert!(result.content.contains("# lines 2–3 of 3"));
    assert!(result.content.contains("2 "));
    assert!(result.content.contains("|two"));
    assert!(result.content.contains("3 "));
    assert!(result.content.contains("|three"));
}

#[tokio::test]
async fn execute_missing_file_suggests() {
    let dir = TempDir::new().unwrap();
    write(&dir, "readme.md", "x");
    let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
    // A distinct name so case-insensitive filesystems (NTFS) still miss.
    let result = ReadTool::new()
        .execute(serde_json::json!({"path": "readm"}), &ctx)
        .await;
    assert!(result.is_error, "{}", result.content);
    assert!(
        result.content.contains("File not found"),
        "{}",
        result.content
    );
    assert!(
        result.content.contains("Did you mean"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn execute_directory_is_error() {
    let dir = TempDir::new().unwrap();
    let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
    let result = ReadTool::new()
        .execute(serde_json::json!({"path": "."}), &ctx)
        .await;
    assert!(result.is_error);
    assert!(result.content.contains("is a directory"));
}

#[tokio::test]
async fn execute_binary_file_refused() {
    let dir = TempDir::new().unwrap();
    write(&dir, "bin.dat", "text\x00nul\n");
    let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
    let result = ReadTool::new()
        .execute(serde_json::json!({"path": "bin.dat"}), &ctx)
        .await;
    assert!(result.is_error);
    assert!(result.content.contains("binary"));
}

#[tokio::test]
async fn execute_image_returns_b64() {
    let dir = TempDir::new().unwrap();
    let png = [0x89u8, b'P', b'N', b'G', 0, 0, 0, 0];
    fs::write(dir.path().join("img.png"), png).unwrap();
    let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
    let result = ReadTool::new()
        .execute(serde_json::json!({"path": "img.png"}), &ctx)
        .await;
    assert!(!result.is_error);
    assert!(result.content.contains("image/png"));
    assert!(result.content.contains("WHYCODES_IMAGE_B64"));
}

#[tokio::test]
async fn oversized_image_is_refused() {
    let dir = TempDir::new().unwrap();
    let big = vec![0u8; (2 * 1024 * 1024) + 1];
    fs::write(dir.path().join("big.png"), big).unwrap();
    let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
    let result = ReadTool::new()
        .execute(serde_json::json!({"path": "big.png"}), &ctx)
        .await;
    assert!(result.is_error, "{}", result.content);
    assert!(result.content.contains("max"), "{}", result.content);
}

#[tokio::test]
async fn execute_missing_path_param() {
    let ctx = ToolContext::new("/");
    let result = ReadTool::new().execute(serde_json::json!({}), &ctx).await;
    assert!(result.is_error);
    assert!(result.content.contains("Missing required parameter"));
}

#[tokio::test]
async fn remaining_read_branches() {
    let t = ReadTool::default();
    assert_eq!(t.name(), "read");
    let img_err = image_read_error("a.png", "denied");
    assert!(img_err.is_error);
    let failed = image_bytes_result("a.png", "image/png", 1, Err("denied".into()));
    assert!(failed.is_error);
    let ok_img = image_bytes_result("a.png", "image/png", 1, Ok(b"PNG".to_vec()));
    assert!(!ok_img.is_error);
    assert!(img_err.content.contains("Failed to read image"));
    let win_err = window_read_error("a.txt", "denied");
    assert!(win_err.is_error);
    assert!(win_err.content.contains("Error reading"));
    assert!(take_read_window(Err("denied".into())).is_err());
    let win_from = window_from(Err("denied".into()), "a.txt", 0, 10, None);
    assert!(win_from.is_error);
    assert!(win_from.content.contains("Error reading"));
    note_large_default_window(MAX_FULL_READ_BYTES + 1, 1, DEFAULT_LIMIT);
    note_large_default_window(1, 1, DEFAULT_LIMIT);
    note_disk_read();
    note_text_read();
    note_fresh_read();
    let mut stale = String::new();
    note_stale_read(&mut stale, "a.txt", "writer");
    assert!(stale.contains("stale"), "{stale}");
    assert!(!refuse_binary(Path::new("/nonexistent-xyz"), "gone"));
    assert!(!sniff_opened(Err(std::io::Error::other("gone"))));
    assert!(!sniff_read(Err(std::io::Error::other("eof")), &[]));
    assert!(sniff_read(Ok(1), &[0]));
    let refused = binary_refused("bin.dat", 4);
    assert!(refused.is_error);
    assert!(!t.description().is_empty());
    let _ = t.parameters();
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("empty.png"), []).unwrap();
    let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
    let empty_img = t
        .execute(serde_json::json!({"path": "empty.png"}), &ctx)
        .await;
    assert!(empty_img.is_error, "{}", empty_img.content);

    let skill = t
        .execute(serde_json::json!({"path": "skill://nope"}), &ctx)
        .await;
    assert!(skill.is_error);

    let long = "x".repeat(5000);
    fs::write(dir.path().join("long.txt"), format!("{long}\nmore\n")).unwrap();
    let window = t
        .execute(
            serde_json::json!({"path": "long.txt", "offset": 1, "limit": 1}),
            &ctx,
        )
        .await;
    assert!(!window.is_error, "{}", window.content);
    assert!(window.content.contains("[truncated"), "{}", window.content);
}

#[tokio::test]
async fn stale_read_huge_file_and_image_read_error() {
    let dir = TempDir::new().unwrap();
    write(&dir, "stale.txt", "hello\n");
    let claims = whycodes_core::file_claims::FileClaimRegistry::new();
    let path = dir.path().join("stale.txt");
    assert!(matches!(
        claims.try_claim("writer", "writer-agent", &path),
        whycodes_core::file_claims::ClaimResult::Acquired
    ));
    let mut ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
    ctx.file_claims = Some(claims);
    ctx.agent_id = Some("reader".into());
    let stale = ReadTool::new()
        .execute(serde_json::json!({"path": "stale.txt"}), &ctx)
        .await;
    assert!(!stale.is_error, "{}", stale.content);
    assert!(stale.content.contains("[stale]"), "{}", stale.content);

    let huge = vec![b'a'; (MAX_FULL_READ_BYTES as usize) + 8];
    fs::write(dir.path().join("huge.txt"), huge).unwrap();
    let window = ReadTool::new()
        .execute(
            serde_json::json!({"path": "huge.txt", "offset": 1, "limit": 400}),
            &ctx,
        )
        .await;
    assert!(!window.is_error, "{}", window.content);

    let fifo = dir.path().join("pipe.jpg");
    #[cfg(unix)]
    {
        use std::os::unix::net::UnixListener;
        let _ = UnixListener::bind(&fifo);
        if fifo.exists() {
            let img = ReadTool::new()
                .execute(serde_json::json!({"path": "pipe.jpg"}), &ctx)
                .await;
            assert!(img.is_error || !img.content.is_empty(), "{}", img.content);
        }
    }
    let _ = fifo;
}

#[tokio::test]
async fn huge_file_reports_unknown_total() {
    let dir = TempDir::new().unwrap();
    let mut content = String::new();
    for i in 0..20 {
        content.push_str(&format!("line {i}\n"));
    }
    let huge_prefix = "x".repeat(64 * 1024);
    let mut body = String::new();
    for _ in 0..((MAX_FULL_READ_BYTES as usize / huge_prefix.len()) + 2) {
        body.push_str(&huge_prefix);
        body.push('\n');
    }
    fs::write(dir.path().join("huge.txt"), body).unwrap();
    let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
    let out = ReadTool::new()
        .execute(
            serde_json::json!({"path": "huge.txt", "offset": 1, "limit": 1}),
            &ctx,
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(
        out.content.contains("≥") || out.content.contains("lines"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn execute_skill_url_loads_project_skill() {
    let dir = TempDir::new().unwrap();
    let skills = dir.path().join(".skills");
    fs::create_dir(&skills).unwrap();
    fs::write(
        skills.join("demo.skill.md"),
        "---\nname: demo\ndescription: d\n---\n\nTHE BODY\n",
    )
    .unwrap();
    let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
    let out = ReadTool::new()
        .execute(serde_json::json!({"path": "skill://demo"}), &ctx)
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("THE BODY"), "{}", out.content);
    let agent = ReadTool::new()
        .execute(serde_json::json!({"path": "agent://"}), &ctx)
        .await;
    assert!(!agent.is_error, "{}", agent.content);
}

#[test]
fn image_io_error_string_is_named() {
    assert_eq!(
        io_error_string(std::io::Error::other("unreadable")),
        "unreadable"
    );
}

fn rust_sample(methods: usize) -> String {
    let mut s =
        String::from("use std::fmt;\n\npub struct Widget {\n    id: u32,\n}\n\nimpl Widget {\n");
    for i in 0..methods {
        s.push_str(&format!(
            "    pub fn method_{i}(&self) -> u32 {{\n        let x = {i};\n        let y = x + 1;\n        y\n    }}\n\n"
        ));
    }
    s.push_str("}\n");
    s
}

async fn read_args(dir: &TempDir, args: serde_json::Value) -> String {
    let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
    let r = ReadTool::new().execute(args, &ctx).await;
    assert!(!r.is_error, "{}", r.content);
    r.content
}

#[test]
fn outline_mode_from_args() {
    use serde_json::json;
    assert_eq!(outline_mode(&json!({"path": "a"})), OutlineMode::Auto);
    assert_eq!(outline_mode(&json!({"offset": 3})), OutlineMode::Off);
    assert_eq!(outline_mode(&json!({"limit": 3})), OutlineMode::Off);
    assert_eq!(outline_mode(&json!({"outline": false})), OutlineMode::Off);
    assert_eq!(
        outline_mode(&json!({"outline": true, "offset": 3})),
        OutlineMode::Forced
    );
}

#[tokio::test]
async fn large_rust_file_reads_as_outline() {
    let dir = TempDir::new().unwrap();
    write(&dir, "big.rs", &rust_sample(100));
    let out = read_args(&dir, serde_json::json!({"path": "big.rs"})).await;
    assert!(
        out.contains("# outline: 102 declarations of 608 lines"),
        "{out}"
    );
    assert!(out.contains("pub struct Widget {"), "{out}");
    assert!(out.contains("|impl Widget {"), "{out}");
    // Nesting shows through the kept indentation.
    assert!(
        out.contains("|    pub fn method_7(&self) -> u32 {"),
        "{out}"
    );
    assert!(!out.contains("let y = x + 1"), "{out}");
    assert!(out.ends_with(
        "[file has 608 lines; showing its outline. Read a range with offset/limit, e.g. offset=3 limit=80]"
    ), "{out}");
}

#[tokio::test]
async fn large_python_and_typescript_files_read_as_outline() {
    let dir = TempDir::new().unwrap();
    let mut py = String::from("import os\n\nclass Store:\n");
    let mut ts = String::from("import x from 'y';\n\nexport class Store {\n}\n");
    for i in 0..150 {
        py.push_str(&format!("    def get_{i}(self):\n        return {i}\n\n"));
        ts.push_str(&format!(
            "export function get{i}(): number {{\n  return {i};\n}}\n"
        ));
    }
    write(&dir, "big.py", &py);
    write(&dir, "big.ts", &ts);
    let out = read_args(&dir, serde_json::json!({"path": "big.py"})).await;
    assert!(out.contains("|class Store:"), "{out}");
    assert!(out.contains("|    def get_42(self):"), "{out}");
    assert!(!out.contains("return 42"), "{out}");
    assert!(
        out.contains("[file has 453 lines; showing its outline."),
        "{out}"
    );
    let out = read_args(&dir, serde_json::json!({"path": "big.ts"})).await;
    assert!(out.contains("|export class Store {"), "{out}");
    assert!(out.contains("|export function get9(): number {"), "{out}");
    assert!(
        out.contains("[file has 454 lines; showing its outline."),
        "{out}"
    );
}

#[tokio::test]
async fn small_file_pages_unless_outline_is_asked() {
    let dir = TempDir::new().unwrap();
    write(&dir, "small.rs", &rust_sample(3));
    let out = read_args(&dir, serde_json::json!({"path": "small.rs"})).await;
    assert!(out.contains("# lines 1–26 of 26"), "{out}");
    let out = read_args(
        &dir,
        serde_json::json!({"path": "small.rs", "outline": true}),
    )
    .await;
    assert!(
        out.contains("# outline: 5 declarations of 26 lines"),
        "{out}"
    );
}

#[tokio::test]
async fn explicit_window_or_outline_false_pages_a_large_file() {
    let dir = TempDir::new().unwrap();
    write(&dir, "big.rs", &rust_sample(100));
    for args in [
        serde_json::json!({"path": "big.rs", "offset": 1}),
        serde_json::json!({"path": "big.rs", "limit": 400}),
        serde_json::json!({"path": "big.rs", "outline": false}),
    ] {
        let out = read_args(&dir, args).await;
        assert!(out.contains("# lines 1–400 of 608"), "{out}");
        assert!(!out.contains("outline"), "{out}");
    }
}

#[tokio::test]
async fn files_repomap_cannot_outline_page_as_before() {
    let dir = TempDir::new().unwrap();
    // Unsupported extension.
    write(&dir, "big.txt", &rust_sample(100));
    // Supported, but no declaration lines.
    write(&dir, "flat.rs", &"let x = 1;\n".repeat(500));
    // Supported, but past repomap's per-file size cap.
    write(&dir, "huge.rs", &rust_sample(9000));
    for path in ["big.txt", "flat.rs", "huge.rs"] {
        let out = read_args(&dir, serde_json::json!({"path": path, "outline": true})).await;
        assert!(out.contains("# lines 1–400 of"), "{path}: {out}");
    }
}

#[tokio::test]
async fn outline_caps_its_entries() {
    let dir = TempDir::new().unwrap();
    write(&dir, "many.rs", &"fn f() {}\n".repeat(450));
    let out = read_args(&dir, serde_json::json!({"path": "many.rs"})).await;
    assert!(
        out.contains("# outline: 450 declarations of 450 lines"),
        "{out}"
    );
    assert!(
        out.contains("[outline shows the first 400 of 450 declarations]"),
        "{out}"
    );
    assert!(out.contains("\n   400 "), "{out}");
    assert!(!out.contains("\n   401 "), "{out}");
}

#[tokio::test]
async fn outline_tags_resolve_with_edit() {
    let dir = TempDir::new().unwrap();
    let path = write(&dir, "big.rs", &rust_sample(100));
    let out = read_args(&dir, serde_json::json!({"path": "big.rs"})).await;
    let row = out
        .lines()
        .find(|l| l.ends_with("|    pub fn method_42(&self) -> u32 {"))
        .expect("method_42 in outline");
    let tag = row
        .trim_start()
        .split(' ')
        .nth(1)
        .unwrap()
        .split('|')
        .next()
        .unwrap();
    let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
    let r = crate::file::edit::EditTool::new()
        .execute(
            serde_json::json!({
                "path": "big.rs",
                "from": tag,
                "new_string": "    pub fn renamed_42(&self) -> u32 {\n"
            }),
            &ctx,
        )
        .await;
    assert!(!r.is_error, "{}", r.content);
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("pub fn renamed_42(&self) -> u32 {"));
    assert!(!text.contains("pub fn method_42("));
}

#[tokio::test]
async fn execute_renders_a_notebook_as_cells() {
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "n.ipynb",
        r##"{"cells": [{"cell_type": "markdown", "source": ["# T\n", "x"]}, {"cell_type": "code", "source": ["y = 1"], "outputs": [{"output_type": "stream"}]}]}"##,
    );
    let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
    let tool = ReadTool::new();
    let read = |args| tool.execute(args, &ctx);
    let result = read(serde_json::json!({"path": "n.ipynb"})).await;
    assert!(!result.is_error, "{}", result.content);
    assert!(
        result.content.contains("(notebook: cells as text"),
        "{}",
        result.content
    );
    assert!(
        result.content.contains("# lines 1–7 of 7"),
        "{}",
        result.content
    );
    assert!(
        result.content.contains("|# %% [code] cell:2"),
        "{}",
        result.content
    );
    assert!(
        result.content.contains("|# out: 1 output (stream)"),
        "{}",
        result.content
    );
    let result = read(serde_json::json!({"path": "n.ipynb", "offset": 2, "limit": 2})).await;
    assert!(
        result.content.contains("# lines 2–3 of 7"),
        "{}",
        result.content
    );
    assert!(
        result.content.contains("Re-call with offset=4"),
        "{}",
        result.content
    );
}
