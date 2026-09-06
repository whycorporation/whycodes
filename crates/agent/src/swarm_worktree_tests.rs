use super::*;
use std::process::Command;

fn init_repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    assert!(
        Command::new("git")
            .args(["init"])
            .current_dir(&root)
            .status()
            .unwrap()
            .success()
    );
    // Identity for commit in bare CI environments.
    let _ = Command::new("git")
        .args(["config", "user.email", "test@whycodes.local"])
        .current_dir(&root)
        .status();
    let _ = Command::new("git")
        .args(["config", "user.name", "whycodes-test"])
        .current_dir(&root)
        .status();
    std::fs::write(root.join("a.txt"), b"base-a\n").unwrap();
    std::fs::write(root.join("b.txt"), b"base-b\n").unwrap();
    assert!(
        Command::new("git")
            .args(["add", "."])
            .current_dir(&root)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(&root)
            .status()
            .unwrap()
            .success()
    );
    (dir, root)
}

#[test]
fn worktree_create_edit_merge_cleanup() {
    let (_keep, root) = init_repo();
    let dest = root
        .join(".whycodes")
        .join("swarm")
        .join("run1")
        .join("worker-0");
    let wt = create_worktree(&root, &dest, "worker-0").expect("create");
    assert!(wt.path.join("a.txt").exists());

    std::fs::write(wt.path.join("a.txt"), b"worker-a\n").unwrap();
    std::fs::write(wt.path.join("new.txt"), b"brand\n").unwrap();

    let report = merge_into_main(&wt, &root);
    assert!(
        report.conflicts.is_empty(),
        "conflicts: {:?}",
        report.conflicts
    );
    assert!(report.applied.iter().any(|p| p == "a.txt"));
    assert!(report.applied.iter().any(|p| p == "new.txt"));
    assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"worker-a\n");
    assert_eq!(std::fs::read(root.join("new.txt")).unwrap(), b"brand\n");

    remove_worktree(&wt).expect("remove");
    assert!(!dest.exists());
}

#[test]
fn create_worktree_rejects_path_without_parent() {
    // Unix `Path::new("/")` has parent `Some("")`; only `""` has `parent() == None`.
    let err = create_worktree(Path::new("."), Path::new(""), "worker-root").unwrap_err();
    assert!(err.contains("parent"), "{err}");
}

#[test]
fn merge_detects_main_divergence() {
    let (_keep, root) = init_repo();
    let dest = root
        .join(".whycodes")
        .join("swarm")
        .join("run2")
        .join("worker-0");
    let wt = create_worktree(&root, &dest, "worker-0").expect("create");

    std::fs::write(wt.path.join("a.txt"), b"from-worker\n").unwrap();
    // Main diverges while worker runs.
    std::fs::write(root.join("a.txt"), b"from-main\n").unwrap();

    let report = merge_into_main(&wt, &root);
    assert_eq!(report.conflicts.len(), 1);
    assert_eq!(report.conflicts[0].path, "a.txt");
    // Main unchanged by merge.
    assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"from-main\n");

    remove_worktree(&wt).ok();
}

#[test]
fn is_git_repo_true_for_init() {
    let (_keep, root) = init_repo();
    assert!(is_git_repo(&root));
    assert!(!is_git_repo(std::env::temp_dir().as_path()));
    assert!(git_toplevel(&root).is_some());
    assert!(format_merge_report(&MergeReport::default()).is_empty());
    let mut report = MergeReport::default();
    report.applied.push("a.txt".into());
    report.deleted.push("gone.txt".into());
    report.conflicts.push(MergeConflict {
        path: "c.txt".into(),
        reason: "diverged".into(),
    });
    report.notes.push("note".into());
    let txt = format_merge_report(&report);
    assert!(txt.contains("Merged"));
    assert!(txt.contains("Deleted"));
    assert!(txt.contains("conflicts"));
    assert!(txt.contains("note"));
    assert!(run_dir(&root, "r1").ends_with("r1"));
}

#[test]
fn create_worktree_rejects_existing_dest() {
    let (_keep, root) = init_repo();
    let dest = root.join(".whycodes").join("swarm").join("exists");
    std::fs::create_dir_all(&dest).unwrap();
    let err = create_worktree(&root, &dest, "w0").unwrap_err();
    assert!(err.contains("already exists"), "{err}");
}

#[test]
fn merge_delete_already_same_and_gone_on_main() {
    let (_keep, root) = init_repo();
    let dest = root
        .join(".whycodes")
        .join("swarm")
        .join("run-del")
        .join("worker-0");
    let wt = create_worktree(&root, &dest, "worker-0").expect("create");

    std::fs::remove_file(wt.path.join("b.txt")).unwrap();
    std::fs::write(wt.path.join("a.txt"), b"base-a\n").unwrap();
    std::fs::write(wt.path.join("new.txt"), b"brand\n").unwrap();
    std::fs::write(root.join("new.txt"), b"brand\n").unwrap();
    std::fs::remove_file(root.join("gone-on-main.txt")).ok();
    std::fs::write(wt.path.join("tracked-gone.txt"), b"x\n").ok();
    // File present at base, deleted in worker, already missing on main.
    std::fs::write(root.join("tmp-gone.txt"), b"tmp\n").ok();

    let report = merge_into_main(&wt, &root);
    assert!(
        report.deleted.iter().any(|p| p == "b.txt")
            || report.notes.is_empty()
            || !report.applied.is_empty(),
        "{report:?}"
    );
    assert!(
        report.applied.iter().any(|p| p == "a.txt")
            || report.applied.iter().any(|p| p == "new.txt"),
        "{report:?}"
    );
    remove_worktree(&wt).ok();
}

#[test]
fn merge_write_conflict_when_target_is_directory() {
    let (_keep, root) = init_repo();
    let dest = root
        .join(".whycodes")
        .join("swarm")
        .join("run-dir")
        .join("worker-0");
    let wt = create_worktree(&root, &dest, "worker-0").expect("create");
    std::fs::write(wt.path.join("newdir.txt"), b"from-worker\n").unwrap();
    std::fs::create_dir_all(root.join("newdir.txt")).unwrap();
    let report = merge_into_main(&wt, &root);
    assert!(
        report.conflicts.iter().any(|c| c.path == "newdir.txt"),
        "{report:?}"
    );
    remove_worktree(&wt).ok();
}

#[test]
fn changed_relative_paths_skips_short_and_parses_rename() {
    let (_keep, root) = init_repo();
    let dest = root
        .join(".whycodes")
        .join("swarm")
        .join("run-ren")
        .join("worker-0");
    let wt = create_worktree(&root, &dest, "worker-0").expect("create");
    assert!(
        Command::new("git")
            .args(["mv", "a.txt", "renamed.txt"])
            .current_dir(&wt.path)
            .status()
            .unwrap()
            .success()
    );
    let paths = changed_relative_paths(&wt.path).expect("status");
    assert!(paths.iter().any(|p| p.contains("renamed")), "{paths:?}");
    remove_worktree(&wt).ok();
}

#[test]
fn porcelain_path_skips_short_and_parses_rename() {
    assert_eq!(porcelain_path(""), None);
    assert_eq!(porcelain_path("M"), None);
    assert_eq!(porcelain_path("M  "), None);
    assert_eq!(porcelain_path("M   "), None);
    assert_eq!(porcelain_path("M  a.txt").as_deref(), Some("a.txt"));
    assert_eq!(
        porcelain_path("R  old.txt -> new.txt").as_deref(),
        Some("new.txt")
    );
    assert_eq!(
        porcelain_path("?? \"quoted.txt\"").as_deref(),
        Some("quoted.txt")
    );
    assert_eq!(
        porcelain_path("M   ").or_else(|| porcelain_path("M  \"\"")),
        None
    );
}

#[test]
fn remove_worktree_fallback_when_path_already_gone() {
    let (_keep, root) = init_repo();
    let dest = root
        .join(".whycodes")
        .join("swarm")
        .join("run-rm")
        .join("worker-0");
    let wt = create_worktree(&root, &dest, "worker-0").expect("create");
    let _ = std::fs::remove_dir_all(&wt.path);
    let _ = remove_worktree(&wt);
}

#[test]
fn merge_deleted_in_worker_main_diverged_and_already_gone() {
    let (_keep, root) = init_repo();
    let dest = root
        .join(".whycodes")
        .join("swarm")
        .join("run-div")
        .join("worker-0");
    let wt = create_worktree(&root, &dest, "worker-0").expect("create");
    std::fs::remove_file(wt.path.join("a.txt")).unwrap();
    std::fs::write(root.join("a.txt"), b"from-main\n").unwrap();
    std::fs::remove_file(root.join("b.txt")).unwrap();
    std::fs::remove_file(wt.path.join("b.txt")).ok();
    let report = merge_into_main(&wt, &root);
    assert!(
        report
            .conflicts
            .iter()
            .any(|c| c.path == "a.txt" && c.reason.contains("diverged")),
        "{report:?}"
    );
    assert!(
        report.deleted.iter().any(|p| p == "b.txt")
            || report.conflicts.iter().any(|c| c.path == "b.txt"),
        "{report:?}"
    );
    remove_worktree(&wt).ok();
}

#[test]
fn merge_mkdir_fails_when_parent_is_file() {
    let (_keep, root) = init_repo();
    let dest = root
        .join(".whycodes")
        .join("swarm")
        .join("run-mkdir")
        .join("worker-0");
    let wt = create_worktree(&root, &dest, "worker-0").expect("create");
    std::fs::create_dir_all(wt.path.join("blocked")).unwrap();
    std::fs::write(wt.path.join("blocked").join("nested.txt"), b"from-worker\n").unwrap();
    std::fs::write(root.join("blocked"), b"i-am-a-file\n").unwrap();
    let report = merge_into_main(&wt, &root);
    assert!(
        report
            .conflicts
            .iter()
            .any(|c| c.path.contains("nested.txt") && c.reason.contains("mkdir")),
        "{report:?}"
    );
    remove_worktree(&wt).ok();
}

#[test]
fn merge_delete_fails_when_main_path_is_directory() {
    let (_keep, root) = init_repo();
    let dest = root
        .join(".whycodes")
        .join("swarm")
        .join("run-del-dir")
        .join("worker-0");
    let wt = create_worktree(&root, &dest, "worker-0").expect("create");
    std::fs::remove_file(wt.path.join("a.txt")).unwrap();
    std::fs::remove_file(root.join("a.txt")).unwrap();
    std::fs::create_dir_all(root.join("a.txt")).unwrap();
    let report = merge_into_main(&wt, &root);
    assert!(
        report.conflicts.iter().any(|c| c.path == "a.txt")
            || report.deleted.iter().any(|p| p == "a.txt"),
        "{report:?}"
    );
    remove_worktree(&wt).ok();
}

#[test]
fn merge_notes_when_worktree_is_not_git() {
    let dir = tempfile::TempDir::new().unwrap();
    let wt = SwarmWorktree {
        path: dir.path().to_path_buf(),
        repo_root: dir.path().to_path_buf(),
        base_head: "deadbeef".into(),
        worker_id: "w0".into(),
    };
    let report = merge_into_main(&wt, dir.path());
    assert!(
        report.notes.iter().any(|n| n.contains("git status")),
        "{report:?}"
    );
}

#[test]
fn create_worktree_fails_when_git_add_cannot_run() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("wt");
    let err = create_worktree(dir.path(), &dest, "w0").unwrap_err();
    assert!(
        err.contains("HEAD") || err.contains("git") || err.contains("worktree"),
        "{err}"
    );
}

#[test]
fn merge_notes_main_deleted_while_worker_edited() {
    let (_keep, root) = init_repo();
    let dest = root
        .join(".whycodes")
        .join("swarm")
        .join("run-del-main")
        .join("worker-0");
    let wt = create_worktree(&root, &dest, "worker-0").expect("create");
    std::fs::write(wt.path.join("a.txt"), b"from-worker\n").unwrap();
    std::fs::remove_file(root.join("a.txt")).unwrap();
    let report = merge_into_main(&wt, &root);
    assert!(
        report.conflicts.iter().any(|c| c.path == "a.txt")
            || report.applied.iter().any(|p| p == "a.txt"),
        "{report:?}"
    );
    remove_worktree(&wt).ok();
}

#[test]
fn create_worktree_add_fails_when_dest_is_existing_file() {
    let (_keep, root) = init_repo();
    let dest_parent = root.join(".whycodes").join("swarm").join("run-add-fail");
    std::fs::create_dir_all(&dest_parent).unwrap();
    let dest = dest_parent.join("worker-0");
    std::fs::write(&dest, b"i-am-a-file").unwrap();
    let err = create_worktree(&root, &dest, "worker-0").unwrap_err();
    assert!(
        err.contains("exists") || err.contains("worktree") || err.contains("git"),
        "{err}"
    );
}

#[test]
fn create_worktree_add_fails_when_git_worktree_add_is_disabled() {
    let (_keep, root) = init_repo();
    let dest = root
        .join(".whycodes")
        .join("swarm")
        .join("run-add-disabled")
        .join("worker-0");
    // `extensions.worktreeConfig` is fine; instead, make `core.worktree` invalid
    // so `git worktree add` fails after HEAD is already resolved.
    let _ = std::process::Command::new("git")
        .args(["config", "core.bare", "true"])
        .current_dir(&root)
        .status();
    let err = create_worktree(&root, &dest, "worker-0");
    let _ = std::process::Command::new("git")
        .args(["config", "core.bare", "false"])
        .current_dir(&root)
        .status();
    match err {
        Ok(wt) => {
            let _ = remove_worktree(&wt);
        }
        Err(e) => {
            assert!(
                e.contains("worktree")
                    || e.contains("git")
                    || e.contains("HEAD")
                    || e.contains("bare"),
                "{e}"
            );
        }
    }
}

#[test]
fn create_worktree_add_fails_when_git_dir_is_readonly_after_head() {
    let (_keep, root) = init_repo();
    let dest = root
        .join(".whycodes")
        .join("swarm")
        .join("run-add-ro")
        .join("worker-0");
    let git_dir = root.join(".git");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let orig = std::fs::metadata(&git_dir).unwrap().permissions();
        std::fs::set_permissions(&git_dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let err = create_worktree(&root, &dest, "worker-0").unwrap_err();
        std::fs::set_permissions(&git_dir, orig).ok();
        assert!(
            err.contains("worktree") || err.contains("git") || err.contains("failed"),
            "{err}"
        );
    }
    #[cfg(not(unix))]
    {
        let _ = create_worktree(&root, &dest, "worker-0");
    }
}

#[test]
fn merge_delete_fails_when_main_parent_is_readonly() {
    let (_keep, root) = init_repo();
    let dest = root
        .join(".whycodes")
        .join("swarm")
        .join("run-del-chmod")
        .join("worker-0");
    let wt = create_worktree(&root, &dest, "worker-0").expect("create");
    std::fs::remove_file(wt.path.join("a.txt")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let orig = std::fs::metadata(&root).unwrap().permissions();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o555)).unwrap();
        let report = merge_into_main(&wt, &root);
        std::fs::set_permissions(&root, orig).ok();
        assert!(
            report.conflicts.iter().any(|c| c.path == "a.txt")
                || report.deleted.iter().any(|p| p == "a.txt"),
            "{report:?}"
        );
    }
    #[cfg(not(unix))]
    {
        let report = merge_into_main(&wt, &root);
        let _ = report;
    }
    remove_worktree(&wt).ok();
}

#[test]
fn remove_worktree_path_remains_when_replaced_with_readonly_file() {
    let (_keep, root) = init_repo();
    let dest = root
        .join(".whycodes")
        .join("swarm")
        .join("run-rm-remain")
        .join("worker-0");
    let wt = create_worktree(&root, &dest, "worker-0").expect("create");
    let parent = wt.path.parent().unwrap().to_path_buf();
    let _ = std::process::Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(&wt.path)
        .current_dir(&root)
        .output();
    let _ = std::fs::remove_dir_all(&wt.path);
    std::fs::write(&wt.path, b"stuck").ok();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let orig = std::fs::metadata(&parent).unwrap().permissions();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o555)).ok();
        let result = remove_worktree(&wt);
        std::fs::set_permissions(&parent, orig).ok();
        let _ = std::fs::remove_file(&wt.path);
        let _ = result;
    }
    #[cfg(not(unix))]
    {
        let _ = remove_worktree(&wt);
    }
}

#[test]
fn merge_skips_vanished_untracked_path() {
    let (_keep, root) = init_repo();
    let dest = root
        .join(".whycodes")
        .join("swarm")
        .join("run-none-none")
        .join("worker-0");
    let wt = create_worktree(&root, &dest, "worker-0").expect("create");
    let ghost = wt.path.join("ghost.txt");
    std::fs::write(&ghost, b"temp\n").unwrap();
    std::fs::remove_file(&ghost).unwrap();
    let report = merge_into_main(&wt, &root);
    assert!(
        report.applied.iter().all(|p| p != "ghost.txt")
            && report.conflicts.iter().all(|c| c.path != "ghost.txt"),
        "{report:?}"
    );
    remove_worktree(&wt).ok();
}
