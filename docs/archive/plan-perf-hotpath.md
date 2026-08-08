# Plan — Performance hot path (binary, hash, math)

**Status:** done · **Depends on:** crate boundaries (shipped) · **Related:** [plan-performance.md](../plan-performance.md), [benchmarks.md](../benchmarks.md)

## Diagnosis

| Area | Finding | Action |
|------|---------|--------|
| **Binary** | Release `whycode` ≈ **23 MB** unstripped, **18 MB** stripped; no `[profile.release]` LTO/strip | Workspace release profile: `strip`, `lto = "thin"`, `codegen-units = 1` |
| **Hash (crypto)** | `sha2` used only in `upgrade` for **SHA256SUMS** verification — correct, keep | No change; do not replace with Fx for integrity |
| **Hash (internal)** | Highlight + mermaid memo keys use **SipHash** (`DefaultHasher`) over full source **every frame** on the TUI path | Switch cache keys to **FxHash** (`rustc-hash`); use `FxHashMap` for hot registries (tools, providers) |
| **Math / heuristics** | Token fallback uses `chars().count() / 4`; no BPE cache; tests swap args | `div_ceil(4)` heuristic; cache BPE in `OnceLock`; fix tests |
| **Parse hot path** | `parse_inline` `find()` recounts needle chars every call | ASCII needle fast-path (byte/char length 1–2) |
| **Boot/TTFF (2026-08-05)** | Tokio-before-parse, full RELRO, 16 MB, mermaid+two-face+tiktoken always linked | Early version path, full LTO, partial RELRO, slim default features → **~1.0 ms / 12 MB** on Linux |

## Rationale

- **SipHash** is DoS-resistant for untrusted map keys; our memo keys and tool name maps are **local/trusted** — FxHash is the usual Rustc choice for that.
- **strip + thin LTO** is the highest ROI binary win without changing behavior.
- **BPE OnceLock** avoids reloading tiktoken tables if/when counting is wired into sessions.

## Tasks

1. [x] Add workspace `[profile.release]` (`strip = true`, `lto = "thin"`, `codegen-units = 1`)
2. [x] Add `rustc-hash` workspace dep; use for format cache keys + tool/provider maps
3. [x] Token counter: BPE cache, `div_ceil` heuristic, fix test arg order
4. [x] `parse_inline` / `find` ASCII needle optimisation
5. [x] Document in `docs/benchmarks.md` + mark residual notes
6. [x] `cargo check --workspace`, targeted tests, release build size check, commit + push

## Non-goals

- Replacing SHA-256 for upgrades
- Micro-crate explosion for hashing
- Changing default debug profile
- CI size gate (noise on shared runners)

## Success

- Release binary **~12 MB** stripped+full LTO+`panic=abort`+slim features (was 23 → 18 → 16 → 14 MB)
- Highlight/mermaid unit tests green (cache key semantics preserved)
- Workspace builds green; token_counter tests green
- Closed memo returns `Arc` — warm `highlight` **~261 ns** / 100 lines (was ~84 µs deep-clone)
- Bench split cold vs warm; numbers in [benchmarks.md](../benchmarks.md)
