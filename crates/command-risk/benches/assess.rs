//! Benchmarks for the shell risk gate.
//!
//! `assess` runs before every `bash` tool call, on the path between the model
//! deciding to run something and it running. It is pure and does no I/O, so it
//! should be far below the cost of spawning a process — but "should be" is not
//! a measurement, and a classifier that grew slowly would show up as the agent
//! feeling sluggish rather than as an obvious regression.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use std::path::Path;
use whycodes_command_risk::assess_with_home;

fn project() -> &'static Path {
    Path::new("/work/proj")
}
fn home() -> Option<&'static Path> {
    Some(Path::new("/home/user"))
}

/// The commands an agent actually issues, weighted toward the common case.
const COMMANDS: &[(&str, &str)] = &[
    ("safe_short", "ls -la"),
    ("safe_build", "cargo test --workspace --no-fail-fast"),
    ("caution", "rm -rf target"),
    ("destructive", "rm -rf /tmp/scratch"),
    ("catastrophic", "rm -rf /etc"),
    (
        "pipeline",
        "cargo build 2>&1 | rg --line-number error | head -20 > errors.txt",
    ),
];

fn bench_assess(c: &mut Criterion) {
    let mut group = c.benchmark_group("assess");
    for (label, command) in COMMANDS {
        group.bench_with_input(BenchmarkId::from_parameter(label), command, |b, cmd| {
            b.iter(|| assess_with_home(black_box(cmd), project(), home()))
        });
    }
    group.finish();
}

/// A model can emit a long chained command. Cost should grow with its length,
/// not faster than it.
fn bench_long_chains(c: &mut Criterion) {
    let mut group = c.benchmark_group("assess_chain");
    for segments in [1usize, 10, 50] {
        let command = (0..segments)
            .map(|i| format!("echo step{i} && rm -rf build/{i}"))
            .collect::<Vec<_>>()
            .join(" && ");
        group.bench_with_input(BenchmarkId::from_parameter(segments), &command, |b, cmd| {
            b.iter(|| assess_with_home(black_box(cmd), project(), home()))
        });
    }
    group.finish();
}

criterion_group!(benches, bench_assess, bench_long_chains);
criterion_main!(benches);
