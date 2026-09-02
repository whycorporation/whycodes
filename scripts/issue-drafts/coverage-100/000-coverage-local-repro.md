# chore(coverage): local repro — coverage.sh + toolchain + docs

Parent: #57

## Problem
`cargo llvm-cov` fails on a vanilla clone (`failed to find llvm-tools-preview`). Contributors cannot reproduce the CI `Coverage (line floor)` job locally. `docs/coverage.md` documents a 12-crate loop of `cargo llvm-cov -p` that CI no longer runs (replaced by one workspace JSON + `scripts/check_coverage_floors.py`).

## Proposal
- Add `rust-toolchain.toml` with `llvm-tools-preview` (plus clippy/rustfmt) so rustup clones match CI.
- Add `scripts/coverage.sh` wrapping the exact CI flags: `IGNORE='/usr/src/|/rustc-'`, `--skip tests::watcher_picks_up_changes --skip picker_flow_over_real_index`, then JSON report + `python3 scripts/check_coverage_floors.py`.
- Point `docs/coverage.md` and a short `CONTRIBUTING.md` snippet at that one command. Do **not** add a `xtask` crate.

## Acceptance
- [ ] `rustup component list --installed` includes `llvm-tools-preview` after toolchain file is applied.
- [ ] `scripts/coverage.sh` (default `summary`) matches CI: workspace `--fail-under-lines` + per-crate floors script.
- [ ] `docs/coverage.md` / `CONTRIBUTING.md` document the wrapper; no crate floor change in this issue.
- [ ] Distro toolchains still work via `LLVM_COV` / `LLVM_PROFDATA`.

## Validation
```bash
scripts/coverage.sh
cargo fmt --all --check
```

## Non-goals
No `--fail-under-lines 100` yet (that is the Phase 4 gate). No codecov/coveralls.
