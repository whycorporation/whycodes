//! Smoke/bench helper: scan the cwd, print timing + a sample query.
//!
//! ```sh
//! cargo run -p whycode-index --example scan --release
//! ```

use std::time::Instant;
use whycode_index::WorkspaceIndex;

fn main() {
    let cwd = std::env::current_dir().unwrap();
    let t0 = Instant::now();
    let idx = WorkspaceIndex::start(vec![cwd]);
    idx.wait_ready(std::time::Duration::from_secs(60));
    let scan_ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("status : {:?} in {scan_ms:.1}ms", idx.status());

    for q in ["main.rs", "tuiapp", "cargo.toml", "agent"] {
        let t = Instant::now();
        let hits = idx.query(q, 10);
        let us = t.elapsed().as_micros();
        println!(
            "query {q:?}: {} hits in {us}µs — top: {}",
            hits.len(),
            hits.first().map(|m| m.rel.as_str()).unwrap_or("-")
        );
    }
    let t = Instant::now();
    let top = idx.browse(0, "");
    println!("browse : {} entries in {}µs", top.len(), t.elapsed().as_micros());
}
