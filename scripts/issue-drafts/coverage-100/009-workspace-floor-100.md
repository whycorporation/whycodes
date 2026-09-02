# chore(coverage): raise workspace floor 82 -> 100 and lock all crates

Parent: #57. **Merge last.** Depends on all Phase 0–3 coverage sub-issues.

## Problem
CI still uses `--fail-under-lines 82` and `FLOORS` has 12 entries (11×100% + `whycodes-format` 95%). After every crate is 100%, the workspace gate and the Python ratchet must match so a one-line uncovered branch fails CI.

## Surfaces
- `.github/workflows/ci.yml` (`--fail-under-lines 82` → `100`)
- `scripts/check_coverage_floors.py` — 24 crates at 100%; remove `whycodes-format` 95% special-case
- `scripts/coverage.sh` default fail-under
- `docs/coverage.md`, `CONTRIBUTING.md`

## Proposal
Only raise the numbers. No new tests in this PR unless a crate regressed. Parent #57 closes when this lands.

## Acceptance
- [ ] `cargo llvm-cov --workspace --ignore-filename-regex "$IGNORE" --fail-under-lines 100` passes on CI.
- [ ] `python scripts/check_coverage_floors.py /tmp/cov.json` exit 0 with **24** crates at 100%.
- [ ] Existing skips preserved: `--skip tests::watcher_picks_up_changes --skip picker_flow_over_real_index`.
- [ ] Close #57 after merge.

## Validation
```bash
scripts/coverage.sh   # fail-under 100
python scripts/check_coverage_floors.py /tmp/cov.json
```
