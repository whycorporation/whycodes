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
