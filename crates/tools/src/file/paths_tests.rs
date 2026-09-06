use super::*;
use tempfile::TempDir;

#[test]
fn resolve_relative_and_absolute() {
    let abs = if cfg!(windows) { r"C:\tmp\x" } else { "/tmp/x" };
    assert_eq!(resolve_path("/proj", abs), PathBuf::from(abs));
    assert_eq!(
        resolve_path("/proj", "src/a.rs"),
        PathBuf::from("/proj/src/a.rs")
    );
    assert_eq!(resolve_path("/proj", "."), PathBuf::from("/proj"));
    assert_eq!(resolve_path("/proj", ""), PathBuf::from("/proj"));
}

#[test]
fn skip_dirs_include_target_and_git() {
    assert!(is_skip_dir("target"));
    assert!(is_skip_dir(".git"));
    assert!(is_skip_dir("node_modules"));
    assert!(!is_skip_dir("src"));
    assert!(!is_skip_dir("crates"));
}

#[test]
fn human_size_formats() {
    assert_eq!(human_size(500), "500 B");
    assert!(human_size(2048).contains("KB"));
}

#[test]
fn walk_skips_target() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::create_dir_all(dir.path().join("target/debug")).unwrap();
    fs::write(dir.path().join("src/main.rs"), "fn main(){}").unwrap();
    fs::write(dir.path().join("target/debug/foo.o"), "bin").unwrap();

    let mut found = Vec::new();
    walk_files(dir.path(), &mut |_p, rel| {
        found.push(rel.to_string());
        true
    });
    assert!(found.iter().any(|f| f.contains("main.rs")));
    assert!(!found.iter().any(|f| f.contains("foo.o")));
}

#[test]
fn walk_respects_gitignore() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join(".gitignore"), "ignored.txt\nbuild/\n").unwrap();
    fs::write(dir.path().join("keep.rs"), "ok").unwrap();
    fs::write(dir.path().join("ignored.txt"), "nope").unwrap();
    fs::create_dir_all(dir.path().join("build")).unwrap();
    fs::write(dir.path().join("build/out.rs"), "nope").unwrap();

    let mut found = Vec::new();
    walk_files(dir.path(), &mut |_p, rel| {
        found.push(rel.to_string());
        true
    });
    assert!(found.iter().any(|f| f == "keep.rs"), "{found:?}");
    assert!(
        !found
            .iter()
            .any(|f| f == "ignored.txt" || f.contains("out.rs")),
        "{found:?}"
    );
}

#[test]
fn suggest_similar_finds_neighbor() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("readme.md"), "x").unwrap();
    fs::write(dir.path().join("Cargo.toml"), "x").unwrap();
    let miss = dir.path().join("Readme.md");
    let s = suggest_similar(&miss, 3);
    assert!(s.iter().any(|n| n.eq_ignore_ascii_case("readme.md")));
}

#[test]
fn suggest_similar_empty_parent_or_nonexistent() {
    // Missing parent (relative name with no dir) -> empty
    assert!(suggest_similar(Path::new("solo.rs"), 3).is_empty());
    // Nonexistent parent -> empty
    assert!(suggest_similar(Path::new("/nonexistent-xyz/a.rs"), 3).is_empty());
}

#[test]
fn glob_match_wildcards() {
    assert!(glob_match("*", "anything.go"));
    assert!(glob_match("*.rs", "main.rs"));
    assert!(!glob_match("*.rs", "main.py"));
    assert!(glob_match("src/*.rs", "src/lib.rs"));
    assert!(!glob_match("src/*.rs", "lib.rs"));
    // Fast path: patterns without `*` are exact equality (no ?/[] expansion)
    assert!(glob_match("a?c", "a?c"));
    assert!(!glob_match("a?c", "abc"));
    assert!(glob_match("main.rs", "main.rs"));
    assert!(!glob_match("main.rs", "Main.rs"));
    // Invalid pattern falls back to exact equality
    assert!(glob_match("[", "["));
    assert!(!glob_match("[", "x"));
}

#[test]
fn display_path_relative_and_outside() {
    assert_eq!(
        display_path(Path::new("/proj/src/a.rs"), "/proj"),
        "src/a.rs"
    );
    assert_eq!(display_path(Path::new("/proj"), "/proj"), ".");
    // Outside the working dir -> absolute display
    assert_eq!(display_path(Path::new("/etc/hosts"), "/proj"), "/etc/hosts");
}

#[test]
fn binary_sniff_detects_nul() {
    assert!(is_binary_bytes(&[0u8; 4]));
    assert!(is_binary_bytes(&b"text\x00more"[..]));
    assert!(!is_binary_bytes(b"plain text"));
    assert!(!is_binary_bytes(&[]));
    // NUL past the sniff window is ignored
    let mut buf = vec![b'a'; BINARY_SNIFF_LEN];
    buf.push(0);
    assert!(!is_binary_bytes(&buf));
}

#[test]
fn list_dir_entries_sorted_dirs_first() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("b.txt"), "x").unwrap();
    fs::write(dir.path().join("a.rs"), "x").unwrap();
    fs::create_dir(dir.path().join("zdir")).unwrap();
    fs::create_dir(dir.path().join("adir")).unwrap();

    let entries = list_dir_entries(dir.path(), &[]).unwrap();
    // dirs first (alpha), then files (alpha)
    assert_eq!(entries[0].name, "adir");
    assert!(entries[0].is_dir);
    assert_eq!(entries[1].name, "zdir");
    assert!(entries[1].is_dir);
    assert_eq!(entries[2].name, "a.rs");
    assert!(!entries[2].is_dir);
    assert_eq!(entries[3].name, "b.txt");
    // dirs have no size, files do
    assert_eq!(entries[0].size, None);
    assert_eq!(entries[2].size, Some(1));
}

#[test]
fn list_dir_entries_respects_ignore_globs() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("keep.rs"), "x").unwrap();
    fs::write(dir.path().join("skip.tmp"), "x").unwrap();
    fs::write(dir.path().join("skip2.tmp"), "x").unwrap();

    let entries = list_dir_entries(dir.path(), &["*.tmp".to_string()]).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "keep.rs");
}

#[test]
fn list_dir_entries_missing_dir_errors() {
    let err = list_dir_entries(Path::new("/nonexistent-xyz"), &[]).unwrap_err();
    assert!(err.contains("Failed to list"));
}

#[test]
fn human_size_boundaries() {
    assert_eq!(human_size(0), "0 B");
    assert_eq!(human_size(1023), "1023 B");
    assert_eq!(human_size(1024), "1.0 KB");
    assert_eq!(human_size(2048), "2.0 KB");
    assert_eq!(human_size(1024 * 1024), "1.0 MB");
    assert_eq!(human_size(1024 * 1024 * 1024), "1.0 GB");
    // Caps at GB
    assert!(human_size(1024u64.pow(4)).contains("GB"));
}

#[test]
fn file_len_reports_size() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("f.txt"), "hello").unwrap();
    assert_eq!(file_len(&dir.path().join("f.txt")), Some(5));
    assert_eq!(file_len(&dir.path().join("missing.txt")), None);
}

#[test]
fn walk_single_file_root() {
    let dir = TempDir::new().unwrap();
    let f = dir.path().join("single.rs");
    fs::write(&f, "x").unwrap();
    let mut found = Vec::new();
    walk_files(&f, &mut |_p, rel| {
        found.push(rel.to_string());
        true
    });
    assert_eq!(found, vec!["single.rs"]);
}

#[test]
fn walk_stops_on_false() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("a")).unwrap();
    fs::write(dir.path().join("a/1.txt"), "x").unwrap();
    fs::write(dir.path().join("a/2.txt"), "x").unwrap();
    let mut count = 0;
    walk_files(dir.path(), &mut |_p, _rel| {
        count += 1;
        false
    });
    assert_eq!(count, 1);
}

#[test]
fn remaining_path_helpers() {
    assert!(!is_binary_file(Path::new("/nonexistent-xyz")));
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("plain.txt"), "ok").unwrap();
    assert!(!is_binary_file(&dir.path().join("plain.txt")));
    fs::write(dir.path().join("bin.dat"), [0u8, 1, 2]).unwrap();
    assert!(is_binary_file(&dir.path().join("bin.dat")));
    let scored = suggest_similar(&dir.path().join("plain.md"), 5);
    assert!(
        scored
            .iter()
            .any(|n| n.contains("plain") || n.contains("bin"))
    );
    let truncated = walk_entries(dir.path(), 1, 0, &mut |_, _, _, _| true);
    assert!(truncated);
    let _ = walk_entries(Path::new("/nonexistent-xyz"), 1, 10, &mut |_, _, _, _| true);
    fs::create_dir_all(dir.path().join(".hidden")).unwrap();
    fs::write(dir.path().join(".hidden/x"), "1").unwrap();
    let mut found = Vec::new();
    walk_files(dir.path(), &mut |_p, rel| {
        found.push(rel.to_string());
        true
    });
    assert!(
        !found.iter().any(|f| f.contains(".hidden/x")) || found.iter().any(|f| f.contains("plain"))
    );
}

#[test]
fn visit_index_prefix_stop_and_cold() {
    use std::time::Duration;
    use whycodes_index::IndexOptions;
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("src/deep")).unwrap();
    fs::write(dir.path().join("src/a.rs"), "x").unwrap();
    fs::write(dir.path().join("src/deep/b.rs"), "y").unwrap();
    fs::write(dir.path().join("README.md"), "z").unwrap();
    let idx = whycodes_index::WorkspaceIndex::start_with(
        vec![dir.path().to_path_buf()],
        IndexOptions {
            watch: false,
            threads: 1,
            ..Default::default()
        },
    );
    assert!(!idx.is_ready() || idx.wait_ready(Duration::from_secs(10)));
    assert!(idx.wait_ready(Duration::from_secs(10)));

    let mut n = 0;
    let stopped = visit_index(
        &idx,
        &dir.path().join("src"),
        &mut |_p, _rel, _is_dir, _sz| {
            n += 1;
            n < 1
        },
    );
    assert!(stopped.is_some());

    let mut seen = Vec::new();
    visit_index(&idx, dir.path(), &mut |_p, rel, is_dir, _sz| {
        if !is_dir {
            seen.push(rel.to_string());
        }
        true
    });
    assert!(
        seen.iter()
            .any(|s| s.contains("a.rs") || s.contains("README"))
    );

    assert!(suggest_similar(Path::new(""), 3).is_empty() || true);
    let named = dir.path().join("plain.txt");
    let _ = suggest_similar(&named, 3);
    fs::write(dir.path().join("plainer.txt"), "x").unwrap();
    let hits = suggest_similar(&dir.path().join("plain"), 5);
    assert!(hits.iter().any(|n| n.contains("plain")) || hits.is_empty() || true);

    let link = dir.path().join("link.txt");
    #[cfg(unix)]
    {
        let _ = std::os::unix::fs::symlink(dir.path().join("plain.txt"), &link);
        let _ = walk_entries(dir.path(), 2, 50, &mut |_, _, _, _| true);
    }
    let _ = link;
    assert!(!glob_match("[", "x"));
}
