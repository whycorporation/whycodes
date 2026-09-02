# Coverage

How line coverage is measured, what CI enforces, and the last workspace
breakdown.

## Running it

```bash
scripts/coverage.sh
```

Same flags as the CI `Coverage (line floor)` job: one `cargo llvm-cov
--workspace` instrumentation, then a JSON report and
`python3 scripts/check_coverage_floors.py`.

```bash
scripts/coverage.sh --dry-run          # print the cargo/python argv
FAIL_UNDER=90 scripts/coverage.sh      # override the workspace floor
COVERAGE_FEATURES=whycodes-storage/bundled scripts/coverage.sh
REPORT_JSON=/tmp/cov.json scripts/coverage.sh
```

CI sets `COVERAGE_FEATURES=whycodes-storage/bundled` because the self-hosted
runner has no `libsqlite3-dev`. Locally, omit it if system sqlite is
installed (`pkg-config sqlite3`).

- `--ignore-filename-regex '/usr/src/|/rustc-'` drops rustc sysroot files that
  leak into totals on Arch / system toolchains.
- `--skip tests::watcher_picks_up_changes --skip picker_flow_over_real_index`
  avoids notify-timing flakes under instrumentation (`crates/index`). The
  normal `test` job still runs them.
- Crate floors at 100% also ignore `tests.rs` so host-only branches cannot
  sink the gate (`CRATE_IGNORE` in the wrapper).

Needs `cargo-llvm-cov` and `llvm-tools` (`llvm-cov`, `llvm-profdata`):

- rustup clone: `rust-toolchain.toml` lists `llvm-tools-preview`. After
  `rustup show`, `rustup component list --installed` should include it.
  Then `cargo install cargo-llvm-cov --locked`.
- Distro toolchain (no rustup): point at the system binaries:

  ```bash
  export LLVM_COV=$(command -v llvm-cov)
  export LLVM_PROFDATA=$(command -v llvm-profdata)
  ```

Do **not** loop `cargo llvm-cov -p <crate>` for each floor — that
re-instruments the workspace (~12×). The Python script reads one JSON
report.

The 100% workspace raise is tracked as [#57](https://github.com/whycorporation/whycodes/issues/57)
(sub-issues #58–#67). This file’s floors stay at the current CI values until
that work lands.

## Floors

| Gate | Floor | What it covers |
|---|---|---|
| Workspace | **82%** lines | Every crate, including tests in the same `.rs` files |
| `function`, `schema`, `skill`, `sandbox`, `protocol`, `plugin`, `command-risk`, `storage`, `core`, `config`, `index` | **100%** lines | Production files only (`tests.rs` ignored) |
| `format` | **95%** lines | Production files only (`tests.rs` ignored) |

The workspace number is a ratchet: CI fails below the floor. When a run lands
comfortably above it, raise `--fail-under-lines` in
`scripts/coverage.sh` (`FAIL_UNDER` default) — CI calls that wrapper.

## Last measurement

Linux x86_64, 2026-09-01 (`cargo llvm-cov --workspace`, flags above — re-measured after #48).
Workspace line coverage **85.58%** (was 85.58% on 2026-08-21; delta within noise — #48 was
`core::ErrorKind`/`TransportError` + `ToolExecutor` cache + swallow ratchet + `agent/{mod,turn,gate,dispatch,compact}` file move, no line-coverage change).

`core` 100% floor covers `ErrorKind` / `TransportError` via `crates/core/src/tests.rs`
(#48). Production modules also have local `#[cfg(test)]` next to the code (`error`,
`network`, `paths`, `sandbox`, `todo`, `tool`, `types`, `panel`, `file_claims`,
`swarm_hub`, `logging`, `tokens`) so a new branch is reviewable without opening the
sibling file (#60). `config` mirrors that in `load` / `merge` / `types` / `validate`.
Swallow-budget numbers live in `scripts/swallowed_error_budget.json`, not in these
line floors. Floors unchanged (workspace ≥82%).

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
then copy only the workspace percent into any README claim if it is mentioned.
