## What

<!-- One or two sentences. Commit style: `area: what changed`. -->

## Why

<!-- Bug, missing docs, or a roadmap item. Link the issue if there is one. -->

## Checks

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace` (or the narrowest crate tests that cover the change)
- [ ] Budget scripts if you touched `.rs` or `Cargo.toml` (`python scripts/check_panic_budget.py` and the other ratchets in [CONTRIBUTING.md](../CONTRIBUTING.md))
- [ ] TypeScript SDK tests if you changed `crates/protocol/src/sdk.rs` or `sdk/typescript`
