# Coverage

How line coverage is measured, what CI enforces, and the last workspace
breakdown.

## Running it

```bash
scripts/coverage.sh
```

Agents must run this **locally and green** before pushing coverage-related
changes (`AGENTS.md`). Iterating on CI logs instead is not allowed.

Same flags as the CI `Coverage (line floor)` job: the instrumented compile
cache is `${CARGO_TARGET_DIR}-llvm-cov` (or `target/llvm-cov-target`), leftover
`*.profraw` / `*.profdata` under that cache and `CARGO_TARGET_DIR` are deleted
(rlibs stay), then one `cargo llvm-cov --workspace` instrumentation, a JSON
report, and `python3 scripts/check_coverage_floors.py`.

```bash
scripts/coverage.sh --dry-run          # print the cargo/python argv
FAIL_UNDER=90 scripts/coverage.sh      # override the workspace floor
COVERAGE_FEATURES=whycodes-storage/bundled scripts/coverage.sh
REPORT_JSON=/tmp/cov.json scripts/coverage.sh
```

CI sets `COVERAGE_FEATURES=whycodes-storage/bundled` because the self-hosted
runners have no `libsqlite3-dev`. Locally, omit it if system sqlite is
installed (`pkg-config sqlite3`).

- `--ignore-filename-regex '/usr/src/|/rustc-'` drops rustc sysroot files that
  leak into totals on Arch / system toolchains.
- `--skip tests::watcher_picks_up_changes --skip picker_flow_over_real_index`
  avoids notify-timing flakes under instrumentation (`crates/index`). The
  normal `test` job still runs them.
- `--skip provider_and_model_dialogs_load_custom_from_isolated_home` (and the
  sibling broken-toml catalog test) skip TUI tests that pin `WHYCODES_HOME`.
  `cargo test --workspace` and `cargo llvm-cov --workspace` are one process;
  other crates overwrite that env. Both CI jobs skip them.
- The workspace 98.5% floor is checked on the JSON export (`-skip-expansions`),
  not on `llvm-cov show` (`--fail-under-lines`). `show` counts serde /
  `format!` expansions and dropped the 0.6.2 patch to 81.4%.
- Crate floors at 100% also ignore `tests.rs` so host-only branches cannot
  sink the gate (`CRATE_IGNORE` in the wrapper).
- JSON crate floors inject `-skip-expansions` via
  `scripts/llvm_cov_skip_expansions.sh` on `cargo llvm-cov report --json`
  (`export` only). Pair `LLVM_COV` with `LLVM_PROFDATA` — cargo-llvm-cov
  otherwise ignores the wrapper. `LLVM_COV_FLAGS` is stripped from the
  child llvm-cov process; rustup llvm-cov 21 also rejects the flag on
  text `show`. Without the wrapper, serde/`format!` lines inflate 100%
  floors (config 59%, protocol 78% on 0.6.1).

Needs `cargo-llvm-cov` and `llvm-tools` (`llvm-cov`, `llvm-profdata`):

- rustup clone: `rust-toolchain.toml` lists `llvm-tools-preview`. After
  `rustup show`, `rustup component list --installed` should include it.
  Then `cargo install cargo-llvm-cov --locked`. rustup's `llvm-cov` is
  **not** on `PATH` (it lives under the sysroot `lib/rustlib/<host>/bin`);
  `scripts/coverage.sh` prepends that directory.
- Distro toolchain (no rustup): point at the system binaries:

  ```bash
  export LLVM_COV=$(command -v llvm-cov)
  export LLVM_PROFDATA=$(command -v llvm-profdata)
  ```

Do **not** loop `cargo llvm-cov -p <crate>` for each floor — that
re-instruments the workspace (~12×). The Python script reads one JSON
report.

[#82](https://github.com/whycorporation/whycodes/issues/82) closed with
ratchets instead of a 100% workspace gate: `tui` and `cli` keep live TTY,
OAuth, and download paths that CI cannot drive without a real terminal or
network. Move `whycodes-tui` / `whycodes-cli` to `FULL_COVER_CRATES` only
after a Linux skip-expansions run prints `covered == total` for them.

## Floors

| Gate | Floor | What it covers |
|---|---|---|
| Workspace | **98.5%** lines | Every crate (`tests.rs` ignored) |
| `function`, `schema`, `skill`, `sandbox`, `protocol`, `plugin`, `command-risk`, `storage`, `core`, `config`, `index`, `session`, `memory`, `llm`, `auth`, `agent`, `lsp`, `mcp`, `sdk`, `server`, `format`, `import`, `slop`, `tools` | **100%** lines, exact | Production files only (`tests.rs` ignored). `covered == total`; one missed line fails even though it rounds to 100.0% |
| `tui` | **96.5%** lines | Production files only |
| `cli` | **95.5%** lines | Production files only |

Every floor is a ratchet: CI fails below it. When a run lands comfortably
above a floor, raise it — `FAIL_UNDER` default in `scripts/coverage.sh` and
`check_coverage_floors.py` for the workspace, `FLOORS` for `tui` / `cli`.

## Last measurement

Linux x86_64, 2026-10-10, CI Coverage job on PR #154
([run 38063003726](https://github.com/whycorporation/whycodes/actions/runs/38063003726)).
Workspace **98.7%** (`79460/80486`). `tui` is **96.7%** (`22841/23617`) and
`cli` is **95.6%** (`5344/5592`). The checker prints their uncovered files on
every run.

`core` 100% floor covers `ErrorKind` / `TransportError` via `crates/core/src/tests.rs`
(#48). Production modules also have local `#[cfg(test)]` next to the code (`error`,
`network`, `paths`, `sandbox`, `todo`, `tool`, `types`, `panel`, `file_claims`,
`swarm_hub`, `logging`, `tokens`) so a new branch is reviewable without opening the
sibling file (#60). `config` mirrors that in `load` / `merge` / `types` / `validate`.
Swallow-budget numbers live in `scripts/swallowed_error_budget.json`, not in these
line floors.

Line coverage is the number CI gates on. Function and region rates are
informational.

### Crate breakdown

| Crate | Lines |
|---|---|
| function, schema, skill, sandbox, protocol, plugin, command-risk, storage, core, config, index | **100%** |
| session, memory, llm, auth, agent, lsp, mcp, sdk, server, format, import, slop, tools | **100%** (`tools` `8716/8716`) |
| tui | floor 96.5% (**96.7%**, `22841/23617`) |
| cli | floor 95.5% (**95.6%**, `5344/5592`) |

When re-measuring, update this breakdown and the dated workspace total here,
then copy only the workspace percent into any README claim if it is mentioned.
