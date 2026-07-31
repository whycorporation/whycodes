//! Benchmarks for the markdown path the TUI runs on every frame.
//!
//! This is not micro-optimisation for its own sake. `crates/tui/src/ui/chat.rs`
//! calls `parse_markdown` and `highlight_code_spans` inside the render loop, so
//! their cost is paid per frame, per visible message — not once per response.
//! A response containing a long code block is the worst case and is measured
//! here explicitly.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use whycode_format::markdown::{highlight_code_spans, parse_markdown, render_markdown};

/// A response of the shape a model actually produces: prose, a list, inline
/// emphasis and a fenced block.
fn typical_response() -> String {
    r#"Here is what I found.

The problem is in **`parse_config`** — it reads the file before checking
whether the *path* exists, so a missing file surfaces as a read error
rather than as a clear message.

- Check the path first
- Return a typed error
- Keep the `?` at the call site

```rust
fn parse_config(path: &Path) -> Result<Config> {
    if !path.exists() {
        return Err(Error::Missing(path.to_path_buf()));
    }
    let text = std::fs::read_to_string(path)?;
    toml::from_str(&text).map_err(Error::Parse)
}
```

That turns the failure into something the caller can act on.
"#
    .to_string()
}

fn code_block(lines: usize) -> String {
    let body: String = (0..lines)
        .map(|i| format!("    let value_{i} = compute({i}) + offset;\n"))
        .collect();
    format!("```rust\nfn generated() {{\n{body}}}\n```\n")
}

fn bench_parse(c: &mut Criterion) {
    let response = typical_response();
    let mut group = c.benchmark_group("parse_markdown");

    group.bench_function("typical_response", |b| {
        b.iter(|| parse_markdown(black_box(&response)))
    });

    // Streaming re-parses the whole message every frame as it grows, so the
    // cost at each length matters, not just the final one.
    for chars in [200usize, 1000, 4000] {
        let prefix: String = response.chars().cycle().take(chars).collect();
        group.bench_with_input(
            BenchmarkId::new("streaming_prefix", chars),
            &prefix,
            |b, s| b.iter(|| parse_markdown(black_box(s))),
        );
    }
    group.finish();
}

fn bench_highlight(c: &mut Criterion) {
    let mut group = c.benchmark_group("highlight_code_spans");

    // First call pays for loading syntect's grammar set. It is behind a
    // OnceLock, so warm it here rather than measuring it in every sample.
    let _ = highlight_code_spans("fn main() {}", Some("rust"));

    for lines in [10usize, 100, 500] {
        let code: String = (0..lines)
            .map(|i| format!("let value_{i} = compute({i}) + offset;\n"))
            .collect();
        group.bench_with_input(BenchmarkId::new("rust", lines), &code, |b, code| {
            b.iter(|| highlight_code_spans(black_box(code), Some("rust")))
        });
    }

    // An unknown language skips syntect entirely; the gap between this and the
    // line above is what highlighting actually costs.
    let code: String = (0..100)
        .map(|i| format!("line {i} of something untagged\n"))
        .collect();
    group.bench_function("untagged_100", |b| {
        b.iter(|| highlight_code_spans(black_box(&code), None))
    });
    group.finish();
}

fn bench_full_render(c: &mut Criterion) {
    let response = typical_response();
    let heavy = code_block(200);
    let mut group = c.benchmark_group("render_markdown_ansi");

    group.bench_function("typical_response", |b| {
        b.iter(|| render_markdown(black_box(&response)))
    });
    group.bench_function("200_line_code_block", |b| {
        b.iter(|| render_markdown(black_box(&heavy)))
    });
    group.finish();
}

criterion_group!(benches, bench_parse, bench_highlight, bench_full_render);
criterion_main!(benches);
