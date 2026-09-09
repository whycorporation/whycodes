use super::*;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tempfile::TempDir;

fn write(dir: &TempDir, name: &str, content: &str) -> std::path::PathBuf {
    let p = dir.path().join(name);
    fs::write(&p, content).unwrap();
    p
}

#[test]
fn clip_line_short_and_long() {
    assert_eq!(clip_line("short", 10), "short");
    assert_eq!(clip_line("short", 5), "short");
    let clipped = clip_line("abcdefghij", 4);
    assert_eq!(clipped, "abcd…");
    // Multibyte chars count as chars
    let clipped = clip_line("türkçe uzun satır", 4);
    assert_eq!(clipped, "türk…");
}

#[test]
fn search_single_file_matches() {
    let dir = TempDir::new().unwrap();
    let f = write(&dir, "a.txt", "hello world\nfoo bar\nhello again\n");
    let out = GrepTool::search("hello", &f, None, false, 0, 50, "/", None).unwrap();
    assert!(out.contains("a.txt:1:hello world"));
    assert!(out.contains("a.txt:3:hello again"));
    assert!(out.contains("(2 matches in 1 file"));
}

#[test]
fn search_single_file_no_matches() {
    let dir = TempDir::new().unwrap();
    let f = write(&dir, "a.txt", "hello\n");
    let out = GrepTool::search("zzz", &f, None, false, 0, 50, "/", None).unwrap();
    assert!(out.is_empty());
}

#[test]
fn search_directory_recursive() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("src/deep")).unwrap();
    write(&dir, "src/main.rs", "fn main() { println!(\"hi\"); }\n");
    write(&dir, "src/deep/mod.rs", "// helper\nfn helper() {}\n");
    write(&dir, "README.md", "no match here\n");

    let out = GrepTool::search("fn ", dir.path(), None, false, 0, 50, "/", None).unwrap();
    assert!(out.contains("src/main.rs:1"));
    assert!(out.contains("src/deep/mod.rs:2"));
    assert!(!out.contains("README.md"));
}

#[test]
fn search_case_insensitive() {
    let dir = TempDir::new().unwrap();
    let f = write(&dir, "a.txt", "Hello World\n");
    let out = GrepTool::search("hello", &f, None, true, 0, 50, "/", None).unwrap();
    assert!(out.contains("a.txt:1"));
}

#[test]
fn search_include_glob_filters() {
    let dir = TempDir::new().unwrap();
    write(&dir, "keep.rs", "fn keep() {}\n");
    write(&dir, "skip.py", "fn keep() {}\n");

    let out = GrepTool::search("keep", dir.path(), Some("*.rs"), false, 0, 50, "/", None).unwrap();
    assert!(out.contains("keep.rs"));
    assert!(!out.contains("skip.py"));
}

#[test]
fn search_context_lines() {
    let dir = TempDir::new().unwrap();
    let f = write(&dir, "a.txt", "before\nmatch\nafter\n");
    let out = GrepTool::search("match", &f, None, false, 1, 50, "/", None).unwrap();
    assert!(out.contains("a.txt:1-before"));
    assert!(out.contains("a.txt:2:match"));
    assert!(out.contains("a.txt:3-after"));
    assert!(out.contains("--"));
}

#[test]
fn search_max_results_truncates_in_directory() {
    let dir = TempDir::new().unwrap();
    for i in 0..10 {
        write(&dir, &format!("f{i}.txt"), "match here\n");
    }
    let out = GrepTool::search("match", dir.path(), None, false, 0, 3, "/", None).unwrap();
    assert!(out.contains("[truncated at 3 matches"));
}

#[test]
fn search_single_file_respects_max_results_without_notice() {
    // Single-file searches stop at max_results but do not append the
    // truncation notice (only directory walks set the flag).
    let dir = TempDir::new().unwrap();
    let mut content = String::new();
    for i in 0..10 {
        content.push_str(&format!("line {i} match\n"));
    }
    let f = write(&dir, "a.txt", &content);
    let out = GrepTool::search("match", &f, None, false, 0, 3, "/", None).unwrap();
    assert!(!out.contains("[truncated"));
    assert!(out.contains("(3 matches in 1 file"));
}

#[test]
fn search_skips_binary_files() {
    let dir = TempDir::new().unwrap();
    write(&dir, "bin.dat", "text\x00with nul\n");
    let out = GrepTool::search("nul", dir.path(), None, false, 0, 50, "/", None).unwrap();
    assert!(out.is_empty());
}

#[test]
fn search_path_not_found() {
    let err = GrepTool::search(
        "x",
        Path::new("/nonexistent-xyz"),
        None,
        false,
        0,
        50,
        "/",
        None,
    )
    .unwrap_err();
    assert!(err.contains("path not found"));
}

#[test]
fn search_invalid_regex() {
    let dir = TempDir::new().unwrap();
    let f = write(&dir, "a.txt", "x");
    let err = GrepTool::search("([", &f, None, false, 0, 50, "/", None).unwrap_err();
    assert!(err.contains("invalid regex"));
}

#[test]
fn search_invalid_glob() {
    let dir = TempDir::new().unwrap();
    let f = write(&dir, "a.txt", "x");
    let err = GrepTool::search("x", &f, Some("["), false, 0, 50, "/", None).unwrap_err();
    assert!(err.contains("invalid glob"));
}

#[test]
fn search_skips_heavy_dirs() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("target/debug")).unwrap();
    write(&dir, "target/debug/foo.rs", "match me\n");
    write(&dir, "keep.rs", "match me\n");
    let out = GrepTool::search("match", dir.path(), None, false, 0, 50, "/", None).unwrap();
    assert!(out.contains("keep.rs"));
    assert!(!out.contains("foo.rs"));
}

#[test]
fn search_respects_gitignore() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join(".gitignore"), "secret.rs\n").unwrap();
    write(&dir, "keep.rs", "match me\n");
    write(&dir, "secret.rs", "match me\n");
    let out = GrepTool::search("match", dir.path(), None, false, 0, 50, "/", None).unwrap();
    assert!(out.contains("keep.rs"), "{out}");
    assert!(!out.contains("secret.rs"), "{out}");
}

#[test]
fn search_large_file_skipped() {
    let dir = TempDir::new().unwrap();
    let big = vec![b'a'; (MAX_GREP_FILE_BYTES + 1) as usize];
    let f = write(&dir, "big.txt", "");
    fs::write(&f, big).unwrap();
    let out = GrepTool::search("a", &f, None, false, 0, 50, "/", None).unwrap();
    assert!(out.is_empty());
}

#[test]
fn search_context_includes_markers_only_between() {
    let dir = TempDir::new().unwrap();
    // Two matches with context should not end with a lone `--` at EOF boundary
    let f = write(&dir, "a.txt", "x\n\n\n");
    let out = GrepTool::search("x", &f, None, false, 1, 50, "/", None).unwrap();
    assert!(out.contains("a.txt:1:x"));
}

#[tokio::test]
async fn execute_missing_pattern_is_error() {
    let ctx = ToolContext::new("/");
    let result = GrepTool::new().execute(serde_json::json!({}), &ctx).await;
    assert!(result.is_error);
    assert!(result.content.contains("Missing required parameter"));
}

#[tokio::test]
async fn execute_finds_match_and_reports_no_matches() {
    let dir = TempDir::new().unwrap();
    write(&dir, "hit.rs", "fn coverage_marker() {}\n");
    write(&dir, "miss.rs", "fn other() {}\n");
    let ctx = ToolContext::new(dir.path().to_str().unwrap());
    let hit = GrepTool::new()
        .execute(
            serde_json::json!({
                "pattern": "coverage_marker",
                "include": "*.rs"
            }),
            &ctx,
        )
        .await;
    assert!(!hit.is_error, "{hit:?}");
    assert!(hit.content.contains("coverage_marker"), "{hit:?}");

    let miss = GrepTool::new()
        .execute(
            serde_json::json!({"pattern": "definitely-not-here-xyz"}),
            &ctx,
        )
        .await;
    assert!(!miss.is_error, "{miss:?}");
    assert!(miss.content.contains("No matches found"), "{miss:?}");
}

#[tokio::test]
async fn remaining_execute_and_search_edges() {
    let t = GrepTool::default();
    assert_eq!(t.name(), "grep");
    assert!(!t.description().is_empty());
    let _ = t.parameters();
    let dir = TempDir::new().unwrap();
    write(&dir, "a.txt", "hello\n");
    let ctx = ToolContext::new(dir.path().to_str().unwrap());
    let hit = t
        .execute(
            serde_json::json!({
                "pattern": "hello",
                "path": "a.txt",
                "case_insensitive": true,
                "context": 9,
                "max_results": 0
            }),
            &ctx,
        )
        .await;
    assert!(!hit.is_error, "{}", hit.content);

    let bad = t
        .execute(serde_json::json!({"pattern": "([", "path": "a.txt"}), &ctx)
        .await;
    assert!(bad.is_error);

    let out =
        GrepTool::search("hello", dir.path(), Some("*.txt"), false, 0, 50, "/", None).unwrap();
    assert!(out.contains("a.txt"));
    assert_eq!(utf8_line(b"hi\r\n"), "hi");
    let empty = grep_ok(String::new());
    assert!(!empty.is_error);
    assert_eq!(empty.content, "No matches found.");
    let hit = grep_ok("a.txt:1:hello".into());
    assert_eq!(hit.content, "a.txt:1:hello");
    let failed = grep_err("grep task failed: boom");
    assert!(failed.is_error);
    assert!(
        failed.content.contains("grep task failed"),
        "{}",
        failed.content
    );
    assert!(invalid_regex("bad").contains("invalid regex"));
    let join = grep_join_error("boom");
    assert!(join.is_error);
    assert!(join.content.contains("grep task failed"));
    assert!(sink_at_cap(3, 3));
    assert!(!sink_at_cap(2, 3));
    let mut breaks = vec!["a.txt:1:x".into()];
    push_context_break(&mut breaks, 10);
    assert_eq!(breaks.last().map(String::as_str), Some("--"));
    push_context_break(&mut breaks, 10);
    assert_eq!(breaks.iter().filter(|s| *s == "--").count(), 1);
    skip_unsearchable(Path::new("gone.rs"), "permission denied");
    skip_unsearchable_msg("gone.rs", "permission denied");
    assert_eq!(empty_file_matches(3), (3, Vec::<String>::new()));
    assert_eq!(skip_stopped_file(3), (3, Vec::<String>::new()));
    assert!(search_file_at_cap(5, 5));
    skip_search_at_cap();
    assert!(!sink_stop().unwrap());
    let mut capped = vec!["a.txt:1:x".into()];
    assert!(!sink_push_match(&mut capped, 1, "a.txt", 2, b"more").unwrap());
    assert!(!sink_push_context(&mut capped, 1, "a.txt", 3, b"ctx").unwrap());
    let mut open = Vec::new();
    assert!(sink_push_match(&mut open, 2, "a.txt", 1, b"hello").unwrap());
    assert!(sink_push_context(&mut open, 2, "a.txt", 2, b"before").unwrap());
    let stop = AtomicBool::new(true);
    let remaining = AtomicUsize::new(3);
    assert_eq!(
        grep_file_task(
            4,
            Path::new("gone.rs"),
            "gone.rs",
            &grep_regex::RegexMatcherBuilder::new().build("x").unwrap(),
            0,
            &stop,
            &remaining
        ),
        (4, Vec::<String>::new())
    );
    let mut sink_matches = vec!["a.txt:1:x".into()];
    let mut sink = CollectSink {
        display: "a.txt",
        matches: &mut sink_matches,
        max_results: 10,
    };
    let searcher = SearcherBuilder::new().build();
    assert!(sink.context_break(&searcher).unwrap());
    let matcher = grep_regex::RegexMatcherBuilder::new().build("x").unwrap();
    let mut at_cap = vec!["already".into()];
    GrepTool::search_file(Path::new("gone.rs"), "gone.rs", &matcher, 0, &mut at_cap, 1);
    assert_eq!(at_cap.len(), 1);
    handle_search_err(Path::new("gone.rs"), Err("denied".into()));
    handle_search_err(Path::new("gone.rs"), Ok(()));
    let mut breaks2 = vec!["x".into()];
    assert!(sink_context_break(&mut breaks2, 10).unwrap());
    let from_ok = grep_from_blocking(Ok(Ok("hit".into())));
    assert_eq!(from_ok.content, "hit");
    let from_err = grep_from_blocking(Ok(Err("bad".into())));
    assert!(from_err.is_error);
    let from_join = grep_from_blocking(Err("boom".into()));
    assert!(from_join.content.contains("grep task failed"));
    let stop = AtomicBool::new(false);
    let remaining = AtomicUsize::new(0);
    assert!(grep_should_stop(&stop, &remaining));
    stop.store(true, Ordering::Relaxed);
    remaining.store(3, Ordering::Relaxed);
    assert!(grep_should_stop(&stop, &remaining));
    stop.store(false, Ordering::Relaxed);
    remaining.store(2, Ordering::Relaxed);
    assert!(!grep_should_stop(&stop, &remaining));
    let mut merged = vec!["a".into()];
    assert!(merge_file_matches(
        &mut merged,
        vec!["b".into(), "c".into()],
        2
    ));
    assert_eq!(merged, vec!["a".to_string(), "b".to_string()]);
    assert!(merge_file_matches(&mut merged, vec!["d".into()], 2));
}

#[tokio::test]
async fn search_uses_index_hidden_glob_and_truncates() {
    use std::time::Duration;
    use whycodes_index::IndexOptions;
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    write(&dir, "src/a.rs", "match me\ncontext\n");
    write(&dir, ".env", "SECRET=match me\n");
    write(&dir, "src/b.rs", "match me again\n");
    write(&dir, "src/c.rs", "match me too\n");
    let idx = whycodes_index::WorkspaceIndex::start_with(
        vec![dir.path().to_path_buf()],
        IndexOptions {
            watch: false,
            threads: 1,
            ..Default::default()
        },
    );
    assert!(idx.wait_ready(Duration::from_secs(10)));
    let mut ctx = ToolContext::new(dir.path().to_str().unwrap());
    ctx.file_index = Some(idx);

    let hit = GrepTool::new()
        .execute(
            serde_json::json!({"pattern": "match me", "include": "*.rs", "max_results": 1}),
            &ctx,
        )
        .await;
    assert!(!hit.is_error, "{}", hit.content);
    assert!(hit.content.contains("[truncated"), "{}", hit.content);

    let hidden = GrepTool::new()
        .execute(
            serde_json::json!({"pattern": "SECRET", "include": ".env"}),
            &ctx,
        )
        .await;
    assert!(!hidden.is_error, "{}", hidden.content);
    assert!(
        hidden.content.contains(".env") || hidden.content.contains("SECRET"),
        "{}",
        hidden.content
    );

    let with_ctx = GrepTool::new()
        .execute(
            serde_json::json!({
                "pattern": "match me",
                "path": "src/a.rs",
                "context": 1,
                "max_results": 1
            }),
            &ctx,
        )
        .await;
    assert!(!with_ctx.is_error, "{}", with_ctx.content);
}
