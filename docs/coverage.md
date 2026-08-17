# Coverage

How line coverage is measured, what CI enforces, and the last workspace
breakdown.

## Running it

```bash
export LLVM_COV=$(command -v llvm-cov)
export LLVM_PROFDATA=$(command -v llvm-profdata)
IGNORE='/usr/src/|/rustc-'
cargo llvm-cov --workspace --ignore-filename-regex "$IGNORE" --summary-only \
  -- --skip tests::watcher_picks_up_changes
```

Same flags as the CI `Coverage (line floor)` job.

- `--ignore-filename-regex '/usr/src/|/rustc-'` drops rustc sysroot files that
  leak into totals on Arch / system toolchains.
- `--skip tests::watcher_picks_up_changes` avoids a notify-timing flake under
  instrumentation (`crates/index`). The normal `test` job still runs it.

Crate floors at 100% also ignore `tests.rs` so host-only branches cannot sink
the gate:

```bash
CRATE_IGNORE="$IGNORE|tests\\.rs$"
for crate in whycode-function whycode-schema whycode-skill whycode-sandbox \
             whycode-protocol whycode-plugin whycode-command-risk whycode-storage \
             whycode-core; do
  cargo llvm-cov -p "$crate" --ignore-filename-regex "$CRATE_IGNORE" \
    --fail-under-lines 100 --show-missing-lines --summary-only
done
```

Needs `cargo-llvm-cov` and `llvm-tools` (`llvm-cov`, `llvm-profdata`). On a
rustup toolchain: `rustup component add llvm-tools-preview`. On a distro
toolchain, point `LLVM_COV` / `LLVM_PROFDATA` at the system binaries.

## Floors

| Gate | Floor | What it covers |
|---|---|---|
| Workspace | **78%** lines | Every crate, including tests in the same `.rs` files |
| `function`, `schema`, `skill`, `sandbox`, `protocol`, `plugin`, `command-risk`, `storage`, `core` | **100%** lines | Production files only (`tests.rs` ignored) |

The workspace number is a ratchet: CI fails below the floor. When a run lands
comfortably above it, raise `--fail-under-lines` in
[`.github/workflows/ci.yml`](../.github/workflows/ci.yml).

## Last measurement

Linux x86_64, 2026-08-17 (`cargo llvm-cov --workspace`, flags above).
Workspace line coverage **81.30%**.

Line coverage is the number CI gates on. Function and region rates are
informational.

The crate table lives in the [README](../README.md#coverage). Re-measure and
update both files together — a stale percentage in the README is worse than
no percentage.
