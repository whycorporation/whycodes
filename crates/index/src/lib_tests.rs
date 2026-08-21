use super::*;
use std::fs;

impl WorkspaceIndex {
    fn apply_test_changes(&self, changes: Vec<Change>) {
        apply_changes(&self.shared, changes);
    }

    fn cancel_only(&self) {
        self.shared.cancel.store(true, Ordering::Relaxed);
    }
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src/nested")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
    fs::write(root.join("src/nested/deep.rs"), "// deep").unwrap();
    fs::write(root.join("Cargo.toml"), "[package]").unwrap();
    fs::create_dir_all(root.join("target/debug")).unwrap();
    fs::write(root.join("target/debug/x.o"), "bin").unwrap();
    fs::write(root.join(".env"), "SECRET=1").unwrap();
    dir
}

#[test]
fn end_to_end_scan_query_browse() {
    let dir = fixture();
    let idx = WorkspaceIndex::start_with(
        vec![dir.path().to_path_buf()],
        IndexOptions {
            watch: false,
            ..Default::default()
        },
    );
    assert!(idx.wait_ready(Duration::from_secs(10)));
    match idx.status() {
        ScanStatus::Ready { total, truncated } => {
            assert!(total >= 5, "total={total}");
            assert!(!truncated);
        }
        other => panic!("expected Ready, got {other:?}"),
    }

    // Fuzzy.
    let hits = idx.query("main.rs", 10);
    assert!(!hits.is_empty());
    assert_eq!(hits[0].rel, "src/main.rs");
    assert_eq!(hits[0].root, 0);

    // Pruned entries never made it in.
    assert!(idx.query("x.o", 10).is_empty());
    assert!(idx.query(".env", 10).is_empty());

    // Browse: empty query → top level, dirs first.
    let top = idx.query("", 20);
    assert!(top.iter().any(|m| m.rel == "src" && m.is_dir));
    assert!(top.iter().any(|m| m.rel == "Cargo.toml" && !m.is_dir));
    assert!(top[0].is_dir, "dirs first: {top:?}");

    // Browse subdir via trailing slash.
    let src = idx.query("src/", 20);
    assert!(src.iter().any(|m| m.rel == "src/main.rs"));
    assert!(src.iter().all(|m| m.rel.starts_with("src/")));

    // Tools view.
    assert!(idx.entries().iter().any(|e| &*e.rel == "src/main.rs"));
    let mut seen = 0;
    idx.visit(&mut |_| seen += 1);
    assert!(seen >= 5);

    // Resolve.
    let m = &hits[0];
    assert!(idx.resolve(m).ends_with("src/main.rs"));
}

#[test]
fn watcher_picks_up_changes() {
    let dir = fixture();
    let idx = WorkspaceIndex::start(vec![dir.path().to_path_buf()]);
    assert!(idx.wait_ready(Duration::from_secs(10)));
    let before = idx.len();

    // Create → appears. `wait_ready` now means the watcher is armed, so
    // this write cannot race the first `watch()`. Still poll: debounce
    // is 250 ms and CI can starve the apply thread.
    fs::write(dir.path().join("src/new_file.rs"), "// new").unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    let appeared = loop {
        let hit = idx
            .query("new_file", 10)
            .iter()
            .any(|m| m.rel == "src/new_file.rs");
        if hit || Instant::now() >= deadline {
            break hit;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(appeared, "create must be indexed");
    assert_eq!(idx.len(), before + 1, "create adds one store entry");

    // Delete → disappears from the store (fuzzy engine rebuilds).
    fs::remove_file(dir.path().join("src/new_file.rs")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let gone = {
            let mut found = false;
            idx.visit(&mut |e| found |= &*e.rel == "src/new_file.rs");
            !found
        };
        if gone || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut found = false;
    idx.visit(&mut |e| found |= &*e.rel == "src/new_file.rs");
    assert!(!found, "delete must be removed from store");
}

#[test]
fn sanitize_roots_dedups_nested() {
    let dir = fixture();
    let root = dir.path().canonicalize().unwrap();
    let roots = sanitize_roots(vec![
        root.clone(),
        root.join("src"),     // nested → dropped
        root.join("missing"), // nonexistent → dropped
        root.clone(),         // dup → dropped
    ]);
    assert_eq!(roots, vec![root]);
}

#[test]
fn project_roots_without_allowlist() {
    let dir = fixture();
    let roots = WorkspaceIndex::project_roots(dir.path());
    assert_eq!(roots, vec![dir.path().to_path_buf()]);
}

#[test]
fn project_roots_reads_allowlist() {
    let dir = fixture();
    let ext = tempfile::TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join(".whycode")).unwrap();
    fs::write(
        dir.path().join(".whycode/external_dirs_allowed"),
        format!("# comment\n{}\n\n", ext.path().display()),
    )
    .unwrap();
    let roots = WorkspaceIndex::project_roots(dir.path());
    assert_eq!(roots.len(), 2);
    assert_eq!(roots[0], dir.path());
    assert_eq!(roots[1], ext.path());
}

#[test]
fn empty_roots_is_safe() {
    let idx = WorkspaceIndex::start_with(vec![], IndexOptions::default());
    assert!(idx.roots().is_empty());
    assert!(idx.query("x", 5).is_empty());
}

/// The UI contract: query_now never blocks on a rematch, fresh results
/// arrive via the dirty flag, and matching eventually settles.
#[test]
fn async_query_eventually_consistent() {
    // 5k files — big enough that a full rematch is measurable.
    let dir = tempfile::TempDir::new().unwrap();
    for d in 0..50 {
        let p = dir.path().join(format!("pkg{d}/src"));
        fs::create_dir_all(&p).unwrap();
        for f in 0..100 {
            fs::write(p.join(format!("file{f}.rs")), "x").unwrap();
        }
    }
    fs::write(dir.path().join("pkg7/src/needle.rs"), "x").unwrap();
    let idx = WorkspaceIndex::start_with(
        vec![dir.path().to_path_buf()],
        IndexOptions {
            watch: false,
            ..Default::default()
        },
    );
    assert!(idx.wait_ready(Duration::from_secs(30)));

    // set path returns fast even with a full rematch queued.
    let t = Instant::now();
    let _ = idx.query_now("needle.rs", 10);
    let first_ms = t.elapsed().as_secs_f64() * 1000.0;
    assert!(first_ms < 50.0, "query_now blocked {first_ms:.1}ms");

    // Dirty flag flips and results converge without another keystroke.
    // Always nudge (`matching`) so a missed nucleo notify cannot stall
    // the snapshot — same contract the TUI picker now uses.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut found = false;
    while Instant::now() < deadline {
        let dirty = idx.take_results_dirty();
        let running = idx.matching();
        if dirty || !running {
            let hits = idx.read_matches(10);
            if hits.iter().any(|m| m.rel.ends_with("needle.rs")) {
                found = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(found, "needle.rs never appeared via dirty-flag polling");
    // …and matching reports quiescence once converged.
    let deadline = Instant::now() + Duration::from_secs(5);
    while idx.matching() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(!idx.matching());
}

#[test]
fn cancel_aborts_scan_and_loop() {
    let dir = fixture();
    let idx = WorkspaceIndex::start_with(
        vec![dir.path().to_path_buf()],
        IndexOptions {
            watch: true,
            threads: 1,
            ..Default::default()
        },
    );
    assert!(idx.wait_ready(Duration::from_secs(10)));
    std::thread::sleep(Duration::from_millis(300));
    idx.cancel_only();
    std::thread::sleep(Duration::from_millis(300));
    let _ = idx.query_now("rs", 20);
    let _ = idx.matching();
    let hits = idx.read_matches(20);
    let _ = hits.len();

    let dir = fixture();
    let idx = WorkspaceIndex::start_with(
        vec![dir.path().to_path_buf()],
        IndexOptions {
            watch: false,
            threads: 1,
            ..Default::default()
        },
    );
    idx.cancel_only();
    let _ = idx.wait_ready(Duration::from_millis(200));
}

#[test]
fn query_now_same_pattern_after_blocking_still_hits() {
    let dir = fixture();
    let idx = WorkspaceIndex::start_with(
        vec![dir.path().to_path_buf()],
        IndexOptions {
            watch: false,
            threads: 1,
            ..Default::default()
        },
    );
    assert!(idx.wait_ready(Duration::from_secs(10)));
    assert!(
        idx.query("main", 10).iter().any(|m| m.rel == "src/main.rs"),
        "blocking fuzzy missed src/main.rs"
    );
    // `set_query` used to return immediately on the same pattern; a missed
    // notify then left `query_now` on an empty snapshot (CI picker flake).
    assert!(
        idx.query_now("main", 10)
            .iter()
            .any(|m| m.rel == "src/main.rs"),
        "same-pattern query_now missed src/main.rs"
    );
    assert!(idx.query_now("zzzznotapath", 10).is_empty());
}

#[test]
fn query_now_sorts_multiple_hits() {
    let dir = fixture();
    let idx = WorkspaceIndex::start_with(
        vec![dir.path().to_path_buf()],
        IndexOptions {
            watch: false,
            threads: 1,
            ..Default::default()
        },
    );
    assert!(idx.wait_ready(Duration::from_secs(10)));
    let _ = idx.query_now("rs", 20);
    idx.rearm_fuzzy();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut hits = Vec::new();
    while Instant::now() < deadline {
        let _ = idx.matching();
        hits = idx.read_matches(20);
        if hits.len() >= 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(hits.len() >= 2, "{hits:?}");
}

#[test]
fn blocking_query_merges_and_sorts_across_roots() {
    let a = fixture();
    let b = fixture();
    let idx = WorkspaceIndex::start_with(
        vec![a.path().to_path_buf(), b.path().to_path_buf()],
        IndexOptions {
            watch: false,
            threads: 1,
            ..Default::default()
        },
    );
    assert!(idx.wait_ready(Duration::from_secs(10)));

    // Distinct scores: the comparator's score branch runs on >=2 hits.
    let hits = idx.query("rs", 20);
    assert!(hits.len() >= 4, "{hits:?}");
    for w in hits.windows(2) {
        assert!(
            w[0].score > w[1].score
                || (w[0].score == w[1].score && w[0].rel.len() <= w[1].rel.len()),
            "unsorted: {w:?}"
        );
    }

    // Same file in both roots -> equal scores, exercising the rel-length
    // tie-break in the sort comparator.
    let tied = idx.query("main.rs", 10);
    assert_eq!(tied.len(), 2, "{tied:?}");
    assert_eq!(tied[0].score, tied[1].score);
    assert!(tied.iter().any(|m| m.root == 0));
    assert!(tied.iter().any(|m| m.root == 1));
}

#[test]
fn recv_should_stop_only_on_disconnect() {
    assert!(!recv_should_stop(
        std::sync::mpsc::RecvTimeoutError::Timeout
    ));
    assert!(recv_should_stop(
        std::sync::mpsc::RecvTimeoutError::Disconnected
    ));
    assert!(matches!(
        classify_recv(Err(std::sync::mpsc::RecvTimeoutError::Timeout)),
        RecvAct::Idle
    ));
    assert!(matches!(
        classify_recv(Err(std::sync::mpsc::RecvTimeoutError::Disconnected)),
        RecvAct::Stop
    ));
    assert!(matches!(
        classify_recv(Ok(Command::Shutdown)),
        RecvAct::Stop
    ));
    assert!(matches!(
        classify_recv(Ok(Command::Rescan)),
        RecvAct::Rescan
    ));
    assert!(matches!(
        classify_recv(Ok(Command::Batch(vec![]))),
        RecvAct::Batch(_)
    ));
}

#[test]
fn start_helpers_status_browse_and_rescan() {
    let dir = fixture();
    let file = dir.path().join("not-a-dir.txt");
    fs::write(&file, "x").unwrap();
    let idx = WorkspaceIndex::start(vec![
        dir.path().to_path_buf(),
        file,
        PathBuf::from("/no/such/index/root"),
    ]);
    assert!(idx.wait_ready(Duration::from_secs(10)));
    assert_eq!(idx.primary_root(), dir.path().canonicalize().unwrap());
    assert!(!idx.is_empty());
    let _ = format!("{:?}", idx);
    match idx.status() {
        ScanStatus::Ready { .. } => {}
        other => panic!("{other:?}"),
    }
    assert!(idx.browse(99, "").is_empty());
    let now = idx.query_now("", 10);
    assert!(now.iter().any(|m| m.rel == "src" && m.is_dir));
    let src = idx.query_now("src/", 10);
    assert!(src.iter().any(|m| m.rel.starts_with("src/")));
    idx.rescan();
    let deadline = Instant::now() + Duration::from_secs(10);
    while idx.is_ready() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(idx.wait_ready(Duration::from_secs(10)));
    assert!(idx.entries().iter().any(|e| &*e.rel == "src/main.rs"));
    let _ = idx.query_now("rs", 20);
    let _ = idx.read_matches(20);
}

#[test]
fn scanning_status_before_ready_and_empty_visit() {
    let dir = fixture();
    let idx = WorkspaceIndex::start_with(
        vec![dir.path().to_path_buf()],
        IndexOptions {
            watch: false,
            threads: 1,
            max_entries: DEFAULT_MAX_ENTRIES,
        },
    );
    let _ = idx.status();
    let _ = idx.is_empty();
    assert!(idx.wait_ready(Duration::from_secs(10)));

    let empty = WorkspaceIndex::start_with(vec![], IndexOptions::default());
    assert!(empty.is_empty());
    empty.visit(&mut |_| panic!("no entries"));
    assert!(empty.entries().is_empty());
    assert!(empty.query_now("x", 5).is_empty());
    assert!(empty.query("x", 5).is_empty());
}

#[test]
fn max_entries_one_truncates_walk() {
    let dir = fixture();
    let idx = WorkspaceIndex::start_with(
        vec![dir.path().to_path_buf()],
        IndexOptions {
            watch: false,
            max_entries: 1,
            threads: 1,
        },
    );
    assert!(idx.wait_ready(Duration::from_secs(10)));
    match idx.status() {
        ScanStatus::Ready { truncated, .. } => assert!(truncated),
        other => panic!("{other:?}"),
    }
}

#[test]
fn max_entries_zero_truncates() {
    let dir = fixture();
    let idx = WorkspaceIndex::start_with(
        vec![dir.path().to_path_buf()],
        IndexOptions {
            watch: false,
            max_entries: 0,
            threads: 1,
        },
    );
    assert!(idx.wait_ready(Duration::from_secs(10)));
    match idx.status() {
        ScanStatus::Ready { truncated, .. } => assert!(truncated),
        other => panic!("{other:?}"),
    }
}

#[test]
fn apply_changes_upsert_and_remove() {
    let dir = fixture();
    let idx = WorkspaceIndex::start_with(
        vec![dir.path().to_path_buf()],
        IndexOptions {
            watch: false,
            threads: 1,
            ..Default::default()
        },
    );
    assert!(idx.wait_ready(Duration::from_secs(10)));
    let new = dir.path().join("src/extra.rs");
    fs::write(&new, "fn extra() {}").unwrap();
    idx.apply_test_changes(vec![Change {
        root: 0,
        rel: "src/extra.rs".into(),
        kind: ChangeKind::Upsert,
    }]);
    let mut found = false;
    idx.visit(&mut |e| found |= &*e.rel == "src/extra.rs");
    assert!(found, "upsert must land in the store");

    fs::remove_file(&new).unwrap();
    idx.apply_test_changes(vec![Change {
        root: 0,
        rel: "src/extra.rs".into(),
        kind: ChangeKind::Upsert,
    }]);
    found = false;
    idx.visit(&mut |e| found |= &*e.rel == "src/extra.rs");
    assert!(!found, "gone file collapses to remove");

    idx.apply_test_changes(vec![Change {
        root: 0,
        rel: "src/main.rs".into(),
        kind: ChangeKind::Remove,
    }]);
    found = false;
    idx.visit(&mut |e| found |= &*e.rel == "src/main.rs");
    assert!(!found);

    idx.apply_test_changes(vec![Change {
        root: 9,
        rel: "nope.rs".into(),
        kind: ChangeKind::Remove,
    }]);
}

#[test]
fn lock_helpers_survive_poison() {
    fn poison<T: Send + 'static>(make: fn() -> T) {
        let m = std::sync::Arc::new(std::sync::Mutex::new(make()));
        let m2 = std::sync::Arc::clone(&m);
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("poison");
        })
        .join();
        let _g = lock(&m);
    }
    poison(|| 1u8);
    let r = std::sync::Arc::new(std::sync::RwLock::new(0u8));
    let r2 = std::sync::Arc::clone(&r);
    let _ = std::thread::spawn(move || {
        let _g = r2.write().unwrap();
        panic!("poison");
    })
    .join();
    drop(read(&r));
    let w = std::sync::Arc::new(std::sync::RwLock::new(0u8));
    let w2 = std::sync::Arc::clone(&w);
    let _ = std::thread::spawn(move || {
        let _g = w2.write().unwrap();
        panic!("poison");
    })
    .join();
    drop(write(&w));
    poison(FuzzyEngine::default);
    poison(Vec::<WalkEntry>::new);
    let store = std::sync::Arc::new(std::sync::RwLock::new(IndexStore::new()));
    let store2 = std::sync::Arc::clone(&store);
    let _ = std::thread::spawn(move || {
        let _g = store2.write().unwrap();
        panic!("poison");
    })
    .join();
    drop(read(&store));
    let store = std::sync::Arc::new(std::sync::RwLock::new(IndexStore::new()));
    let store2 = std::sync::Arc::clone(&store);
    let _ = std::thread::spawn(move || {
        let _g = store2.write().unwrap();
        panic!("poison");
    })
    .join();
    drop(write(&store));
}
