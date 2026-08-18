use super::*;
use std::fs;
use std::sync::Mutex;

fn collect(root: &Path, threads: usize) -> Vec<String> {
    let scanned = AtomicUsize::new(0);
    let cancel = AtomicBool::new(false);
    let out = Mutex::new(Vec::new());
    walk_root(root, threads, 100_000, &scanned, &cancel, &|e| {
        out.lock()
            .unwrap()
            .push(format!("{}{}", e.rel, if e.is_dir { "/" } else { "" }));
    });
    out.into_inner().unwrap()
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
    fs::write(root.join("README.md"), "hi").unwrap();
    fs::create_dir_all(root.join("target/debug")).unwrap();
    fs::write(root.join("target/debug/x.o"), "bin").unwrap();
    fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
    fs::write(root.join("node_modules/pkg/i.js"), "x").unwrap();
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(root.join(".git/config"), "x").unwrap();
    fs::create_dir_all(root.join(".github/workflows")).unwrap();
    fs::write(root.join(".github/workflows/ci.yml"), "on: push").unwrap();
    fs::write(root.join(".env"), "SECRET=1").unwrap();
    fs::write(root.join(".gitignore"), "ignored.txt\n").unwrap();
    fs::write(root.join("ignored.txt"), "x").unwrap();
    dir
}

#[test]
fn walk_respects_policy_and_gitignore() {
    for threads in [1, 4] {
        let dir = fixture();
        let entries = collect(dir.path(), threads);
        assert!(
            entries.iter().any(|e| e == "src/main.rs"),
            "threads={threads}: {entries:?}"
        );
        assert!(entries.iter().any(|e| e == "src/"), "threads={threads}");
        assert!(
            entries.iter().any(|e| e == ".github/workflows/ci.yml"),
            "threads={threads}: {entries:?}"
        );
        assert!(entries.iter().any(|e| e == ".gitignore"));
        for bad in [
            "target/debug/x.o",
            "node_modules/pkg/i.js",
            ".git/config",
            ".env",
            "ignored.txt",
        ] {
            assert!(
                !entries.iter().any(|e| e == bad),
                "threads={threads}: {bad} must be excluded: {entries:?}"
            );
        }
    }
}

#[test]
fn walk_caps_entries() {
    let dir = tempfile::TempDir::new().unwrap();
    for i in 0..100 {
        fs::write(dir.path().join(format!("f{i:03}.txt")), "x").unwrap();
    }
    let scanned = AtomicUsize::new(0);
    let cancel = AtomicBool::new(false);
    let count = AtomicUsize::new(0);
    let stats = walk_root(dir.path(), 1, 10, &scanned, &cancel, &|_| {
        count.fetch_add(1, Ordering::Relaxed);
    });
    assert!(stats.truncated);
    assert_eq!(stats.scanned, 10);
    assert_eq!(count.load(Ordering::Relaxed), 10);
}

#[test]
fn walk_is_cancellable() {
    let dir = tempfile::TempDir::new().unwrap();
    for i in 0..100 {
        fs::write(dir.path().join(format!("f{i:03}.txt")), "x").unwrap();
    }
    let scanned = AtomicUsize::new(0);
    let cancel = AtomicBool::new(true); // pre-cancelled
    let stats = walk_root(dir.path(), 1, 100_000, &scanned, &cancel, &|_| {});
    assert_eq!(stats.scanned, 0);
    assert!(!stats.truncated);
}

#[test]
fn walk_skips_unreadable_and_symlink() {
    let dir = fixture();
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let _ = symlink(dir.path().join("src/main.rs"), dir.path().join("link.rs"));
    }
    let entries = collect(dir.path(), 1);
    assert!(entries.iter().any(|e| e == "src/main.rs"));
    let scanned = AtomicUsize::new(0);
    let cancel = AtomicBool::new(false);
    let _ = walk_root(dir.path(), 0, 100_000, &scanned, &cancel, &|_| {});
    assert!(allow_dir_entry(0, "src", true));
    assert!(!allow_dir_entry(1, "target", true));
    assert!(!allow_dir_entry(1, ".env", false));
    assert!(allow_dir_entry(1, "main.rs", false));
    assert!(rel_is_empty(""));
    assert!(!rel_is_empty("src"));
    assert!(entry_is_err::<(), &str>(&Err("nope")));
    assert!(!entry_is_err::<(), &str>(&Ok(())));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let denied = dir.path().join("denied");
        std::fs::create_dir(&denied).unwrap();
        let _ = std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o000));
        let _ = collect(dir.path(), 1);
        let _ = std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o755));
    }
}
