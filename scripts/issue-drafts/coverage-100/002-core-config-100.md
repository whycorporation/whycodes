# test(core/config): verify whycodes-core + whycodes-config stay at 100% (core 5/14, config 1/6 gaps)

Parent: #57.

## Problem
Both crates are already in `FULL_COVER_CRATES` at 100% **line** coverage (`tests.rs` ignored). File-test ratio is still low: `core` 5/14, `config` 1/6 — most modules have no local `#[cfg(test)]` and rely on a single sibling `tests.rs`. That is brittle: a new branch in `load.rs` / `types.rs` / `logging.rs` can slip through until llvm-cov fails, and reviewers cannot see tests next to the code.

## Surfaces
- `crates/core/src/{error,file_claims,logging,network,panel,paths,sandbox,swarm_hub,todo,tokens,tool,types}.rs`
- `crates/config/src/{load,merge,types,validate}.rs`

## Proposal
Keep the 100% floor. Add missing branch tests (token counting, error mapping, loader/migration/policy, merge overrides) in the existing `tests.rs` files (or `mod tests` in the module). Do not drop `tests.rs` ignore. Confirm `check_coverage_floors.py` still prints OK for both.

## Acceptance
- [ ] `whycodes-core` and `whycodes-config` remain 100% under `CRATE_IGNORE`.
- [ ] File-test ratio >= 80% for each crate (sibling `tests.rs` + any new `mod tests`).
- [ ] `cargo test -p whycodes-core -p whycodes-config` green.

## Validation
```bash
cargo test -p whycodes-core
cargo test -p whycodes-config
scripts/coverage.sh
```
