//! Smoke/bench helper: scan the cwd, print timing + a sample query.
//!
//! ```sh
//! cargo run -p whycode-index --example scan --release
//! ```

use std::time::Instant;
use whycode_index::WorkspaceIndex;

fn vmrss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find(|l| l.starts_with("VmRSS:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

fn main() {
    let cwd = std::env::current_dir().unwrap();
    let t0 = Instant::now();
    let idx = WorkspaceIndex::start(vec![cwd]);
    idx.wait_ready(std::time::Duration::from_secs(60));
    let scan_ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("status : {:?} in {scan_ms:.1}ms", idx.status());
    if let Some(kb) = vmrss_kb() {
        println!("rss    : {:.1} MB (whole process)", kb as f64 / 1024.0);
    }

    // Async UI pattern: query_now never blocks; results settle via polling.
    for q in ["main.rs", "tuiapp", "cargo.toml", "agent"] {
        let t = Instant::now();
        let hits = idx.query_now(q, 10);
        let first_us = t.elapsed().as_micros();
        let mut final_hits = hits.len();
        let mut top = hits
            .first()
            .map(|m| m.rel.clone())
            .unwrap_or_else(|| "-".into());
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        while idx.matching() && Instant::now() < deadline {
            if idx.take_results_dirty() {
                let h = idx.read_matches(10);
                final_hits = h.len();
                if let Some(m) = h.first() {
                    top = m.rel.clone();
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        // Final read: the last publish can land after the loop's last poll.
        let h = idx.read_matches(10);
        final_hits = h.len();
        if let Some(m) = h.first() {
            top = m.rel.clone();
        }
        let settle_ms = t.elapsed().as_secs_f64() * 1000.0;
        println!(
            "query {q:?}: first {first_us}µs (non-blocking), settled in {settle_ms:.1}ms — {final_hits} hits, top: {top}"
        );
    }
    let t = Instant::now();
    let top = idx.browse(0, "");
    println!("browse : {} entries in {}µs", top.len(), t.elapsed().as_micros());
}
