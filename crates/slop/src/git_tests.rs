use super::*;
use std::path::Path;
use std::process::Command;

#[test]
fn numstat_counts_delta_and_skips_binary() {
    let raw = "10\t2\tsrc/a.rs\n-\t-\tpic.png\n3\t0\tsrc/b.ts\n";
    let (delta, files, paths) = parse_numstat(raw);
    assert_eq!(delta, 11);
    assert_eq!(files, 3);
    assert_eq!(paths, vec!["src/a.rs", "pic.png", "src/b.ts"]);
}

#[test]
fn numstat_ignores_blank_and_empty_path() {
    let (delta, files, paths) = parse_numstat("\n1\t1\t\n");
    assert_eq!(delta, 0);
    assert_eq!(files, 0);
    assert!(paths.is_empty());
}

#[test]
fn numstat_non_numeric_added() {
    let (delta, files, paths) = parse_numstat("x\ty\tz.rs\n");
    assert_eq!(delta, 0);
    assert_eq!(files, 1);
    assert_eq!(paths, vec!["z.rs"]);
}

#[test]
fn snapshot_rejects_non_git() {
    let dir = tempfile::tempdir().unwrap();
    let err = snapshot(dir.path(), None).unwrap_err();
    assert!(err.message.contains("not a git"), "{err}");
}

#[test]
fn snapshot_vs_head_on_dirty_tree() {
    let dir = tempfile::tempdir().unwrap();
    git_init(dir.path());
    std::fs::write(dir.path().join("a.rs"), "fn a() {}\n").unwrap();
    git_add_commit(dir.path(), "init");
    std::fs::write(
        dir.path().join("a.rs"),
        "fn a() { if true { if true { 1 } else { 0 } } else { 2 } }\n",
    )
    .unwrap();
    let snap = snapshot(dir.path(), Some("HEAD")).unwrap();
    assert_eq!(snap.head, "working-tree");
    assert!(!snap.base.is_empty());
    assert_eq!(snap.files_changed, 1);
    assert_eq!(snap.contents.len(), 1);
    assert_eq!(snap.contents[0].0, "a.rs");
}

#[test]
fn snapshot_skips_deleted_and_unknown_lang_includes_untracked() {
    let dir = tempfile::tempdir().unwrap();
    git_init(dir.path());
    std::fs::write(dir.path().join("keep.rs"), "fn k() {}\n").unwrap();
    std::fs::write(dir.path().join("gone.rs"), "fn g() {}\n").unwrap();
    std::fs::write(dir.path().join("notes.md"), "hi\n").unwrap();
    git_add_commit(dir.path(), "init");
    std::fs::remove_file(dir.path().join("gone.rs")).unwrap();
    std::fs::write(dir.path().join("notes.md"), "bye\n").unwrap();
    std::fs::write(dir.path().join("keep.rs"), "fn k() { 1 }\n").unwrap();
    std::fs::write(dir.path().join("new.rs"), "fn n() { 1 }\n").unwrap();
    let snap = snapshot(dir.path(), Some("HEAD")).unwrap();
    assert!(snap.files_changed >= 3, "{}", snap.files_changed);
    assert!(snap.contents.iter().any(|(p, _)| p == "keep.rs"));
    assert!(snap.contents.iter().any(|(p, _)| p == "new.rs"));
    assert!(!snap.contents.iter().any(|(p, _)| p == "gone.rs"));
}

#[test]
fn resolve_base_explicit_and_default() {
    let dir = tempfile::tempdir().unwrap();
    git_init(dir.path());
    std::fs::write(dir.path().join("a.rs"), "fn a() {}\n").unwrap();
    git_add_commit(dir.path(), "init");
    let head = git_rev_parse(dir.path(), "HEAD").unwrap();
    assert_eq!(resolve_base(dir.path(), Some("HEAD")).unwrap(), head);
    let auto = resolve_base(dir.path(), Some("")).unwrap();
    assert_eq!(auto.len(), head.len());
    let err = resolve_base(dir.path(), Some("no-such-ref")).unwrap_err();
    assert!(err.message.contains("rev-parse"), "{err}");
}

#[test]
fn default_branch_origin_head_and_main() {
    let dir = tempfile::tempdir().unwrap();
    git_init(dir.path());
    std::fs::write(dir.path().join("a.rs"), "fn a() {}\n").unwrap();
    git_add_commit(dir.path(), "init");
    assert_eq!(default_branch(dir.path()).unwrap(), "main");
    let _ = Command::new("git")
        .args(["remote", "add", "origin", "."])
        .current_dir(dir.path())
        .status();
    let _ = Command::new("git")
        .args([
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ])
        .current_dir(dir.path())
        .status();
    let b = default_branch(dir.path()).unwrap();
    assert!(b.contains("main"), "{b}");
    let mb = merge_base(dir.path(), "main").unwrap();
    assert_eq!(mb.len(), 40);
}

#[test]
fn git_ok_error_paths() {
    let dir = tempfile::tempdir().unwrap();
    git_init(dir.path());
    let err = git_ok(
        dir.path(),
        &["rev-parse", "--verify", "nope"],
        "git rev-parse",
    )
    .unwrap_err();
    assert!(err.message.contains("git rev-parse"), "{err}");
}

#[test]
fn git_unavailable_or_failed_cwd() {
    let missing = Path::new("/this/whycodes-slop-cwd-does-not-exist");
    let err = git_ok(missing, &["status"], "git status").unwrap_err();
    assert!(
        err.message.contains("git status") || err.message.contains("git unavailable"),
        "{err}"
    );
}

#[test]
fn default_branch_origin_non_origin_prefix() {
    let dir = tempfile::tempdir().unwrap();
    git_init(dir.path());
    std::fs::write(dir.path().join("a.rs"), "fn a() {}\n").unwrap();
    git_add_commit(dir.path(), "init");
    let _ = Command::new("git")
        .args(["remote", "add", "origin", "."])
        .current_dir(dir.path())
        .status();
    let _ = Command::new("git")
        .args([
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/heads/main",
        ])
        .current_dir(dir.path())
        .status();
    let b = default_branch(dir.path()).unwrap();
    assert!(!b.is_empty(), "{b}");
}

#[test]
fn default_branch_master_and_empty_repo() {
    let dir = tempfile::tempdir().unwrap();
    let st = Command::new("git")
        .args(["init", "-b", "master"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(st.status.success());
    let _ = Command::new("git")
        .args(["config", "user.email", "t@t"])
        .current_dir(dir.path())
        .status();
    let _ = Command::new("git")
        .args(["config", "user.name", "t"])
        .current_dir(dir.path())
        .status();
    std::fs::write(dir.path().join("a.rs"), "fn a() {}\n").unwrap();
    git_add_commit(dir.path(), "init");
    assert_eq!(default_branch(dir.path()).unwrap(), "master");

    let empty = tempfile::tempdir().unwrap();
    let _ = Command::new("git")
        .args(["init", "--initial-branch=nope"])
        .current_dir(empty.path())
        .status();
    let err = default_branch(empty.path()).unwrap_err();
    assert!(
        err.message.contains("default branch") || err.message.contains("rev-parse"),
        "{err}"
    );
    let err = snapshot(empty.path(), None).unwrap_err();
    assert!(!err.message.is_empty());
}

fn git_init(dir: &Path) {
    let st = Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        st.status.success(),
        "{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let _ = Command::new("git")
        .args(["config", "user.email", "t@t"])
        .current_dir(dir)
        .status();
    let _ = Command::new("git")
        .args(["config", "user.name", "t"])
        .current_dir(dir)
        .status();
}

fn git_add_commit(dir: &Path, msg: &str) {
    let _ = Command::new("git")
        .args(["add", "."])
        .current_dir(dir)
        .status();
    let st = Command::new("git")
        .args(["commit", "-m", msg, "--allow-empty"])
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(
        st.status.success(),
        "{}",
        String::from_utf8_lossy(&st.stderr)
    );
}
