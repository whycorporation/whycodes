//! Index hot paths: cold walk (serial vs parallel), warm fuzzy query, browse.
//!
//! ```sh
//! cargo bench -p whycodes-index
//! ```

use std::sync::atomic::{AtomicBool, AtomicUsize};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

use whycodes_index::{IndexOptions, WorkspaceIndex, walk_root};

/// Build a synthetic repo: `dirs` top dirs × `files` files each, nested 2 deep.
fn synthetic_repo(dirs: usize, files: usize) -> tempfile::TempDir {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
    std::fs::create_dir_all(root.join("target/debug")).unwrap();
    for i in 0..200 {
        std::fs::write(root.join(format!("target/debug/build{i}.o")), "bin").unwrap();
    }
    for d in 0..dirs {
        let dir = root.join(format!("crate{d}/src"));
        std::fs::create_dir_all(&dir).unwrap();
        for f in 0..files {
            std::fs::write(dir.join(format!("mod{f}.rs")), "fn f() {}").unwrap();
        }
    }
    tmp
}

fn bench_walk(c: &mut Criterion) {
    let tmp = synthetic_repo(20, 100); // ~2000 files + 200 pruned
    let root = tmp.path().to_path_buf();
    let mut g = c.benchmark_group("walk");
    for threads in [1, 4, 8] {
        g.bench_with_input(
            BenchmarkId::new("walk_root", format!("t{threads}")),
            &threads,
            |b, &threads| {
                b.iter(|| {
                    let scanned = AtomicUsize::new(0);
                    let cancel = AtomicBool::new(false);
                    let count = AtomicUsize::new(0);
                    walk_root(&root, threads, usize::MAX, &scanned, &cancel, &|_| {
                        count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    });
                    count
                })
            },
        );
    }
    g.finish();
}

fn bench_query(c: &mut Criterion) {
    let tmp = synthetic_repo(20, 100);
    let idx = WorkspaceIndex::start_with(
        vec![tmp.path().to_path_buf()],
        IndexOptions {
            watch: false,
            ..Default::default()
        },
    );
    assert!(idx.wait_ready(std::time::Duration::from_secs(30)));
    let mut g = c.benchmark_group("index");
    g.bench_function("query_warm", |b| b.iter(|| idx.query("mod42.rs", 20)));
    g.bench_function("browse_top", |b| b.iter(|| idx.browse(0, "")));
    g.bench_function("entries_snapshot", |b| b.iter(|| idx.entries()));
    g.finish();
}

/// Keystroke cost + full settle at 20k entries — the scale story of the
/// async query path (UI must stay non-blocking even when rematches take ms).
fn bench_query_scale(c: &mut Criterion) {
    let tmp = synthetic_repo(200, 100); // ~20k files
    let idx = WorkspaceIndex::start_with(
        vec![tmp.path().to_path_buf()],
        IndexOptions {
            watch: false,
            ..Default::default()
        },
    );
    assert!(idx.wait_ready(std::time::Duration::from_secs(60)));
    let mut g = c.benchmark_group("index-20k");
    g.bench_function("query_now_nonblocking", |b| {
        b.iter(|| idx.query_now("mod42.rs", 20))
    });
    g.bench_function("query_settle", |b| {
        b.iter(|| {
            idx.query_now("mod42.rs", 20);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while idx.matching() && std::time::Instant::now() < deadline {
                if idx.take_results_dirty() {
                    std::hint::black_box(idx.read_matches(20));
                }
                std::thread::yield_now();
            }
        })
    });
    g.finish();
}

criterion_group!(benches, bench_walk, bench_query, bench_query_scale);
criterion_main!(benches);
