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
for crate in whycodes-function whycodes-schema whycodes-skill whycodes-sandbox \
             whycodes-protocol whycodes-plugin whycodes-command-risk whycodes-storage \
             whycodes-core whycodes-config whycodes-format whycodes-index; do
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
| Workspace | **82%** lines | Every crate, including tests in the same `.rs` files |
| `function`, `schema`, `skill`, `sandbox`, `protocol`, `plugin`, `command-risk`, `storage`, `core`, `config`, `format`, `index` | **100%** lines | Production files only (`tests.rs` ignored) |

The workspace number is a ratchet: CI fails below the floor. When a run lands
comfortably above it, raise `--fail-under-lines` in
[`.github/workflows/ci.yml`](../.github/workflows/ci.yml).

## Last measurement

Linux x86_64, 2026-08-21 (`cargo llvm-cov --workspace`, flags above).
Workspace line coverage **85.58%**.

`core` 100% floor still covers `ErrorKind` / `TransportError` via `crates/core/src/tests.rs`
(2026-09-01, #48). Swallow-budget numbers live in `scripts/swallowed_error_budget.json`,
not in these line floors. Agent facade split (same date, #48) is a file move only —
re-measure llvm-cov when the next coverage PR lands; floors unchanged.

Line coverage is the number CI gates on. Function and region rates are
informational.

### Crate breakdown

| Crate | Lines |
|---|---|
| function, schema, skill, sandbox, protocol, plugin, command-risk, storage, core, config, format, index | **100%** |
| session | **100%** |
| auth | **99.2%** |
| memory | 87.7% |
| tui | 86.3% |
| tools | 86.0% |
| llm | 83.6% |
| mcp | 80.8% |
| sdk | 80.4% |
| server | 79.1% |
| cli | 69.6% |
| agent | 65.4% |
| lsp | 64.1% |

When re-measuring, update this breakdown and the dated workspace total here,
then copy only the workspace total to the
[README](../README.md#coverage). A stale percentage is worse than no
percentage.
