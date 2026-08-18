# Contributing

Thanks for taking the time. This file is the short path from clone to a merged
change.

## Setup

Stable Rust is the only requirement:

```bash
git clone https://github.com/whycorporation/whycode.git
cd whycode
cargo build -p whycode-cli
```

## Before opening a pull request

CI runs exactly these, and fails on any of them:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
python scripts/check_panic_budget.py
python scripts/check_swallowed_error_budget.py
python scripts/check_dependency_boundaries.py
python scripts/check_sdk_protocol.py   # Rust ↔ TypeScript protocol v1 tags
```

TypeScript SDK (`sdk/typescript`): `npm ci && npm test` after changing
`crates/protocol/src/sdk.rs` or the TS client.

Notes:

- Clippy runs with `--all-targets`, so test code is linted under `-D warnings`
  too.
- The budget scripts are ratchets: counts may only go down. If your change
  legitimately needs a new panic site or a swallowed error, the budget file
  change is reviewed as part of the PR.
- CI also runs `cargo llvm-cov` and fails the workspace below 78% line
  coverage (twelve crates are locked at 100%). See
  [docs/coverage.md](docs/coverage.md).
- Internal crate edges are allowlisted in
  `scripts/dependency_boundaries.json`. Adding an edge is a deliberate
  decision — see [docs/architecture.md](docs/architecture.md).

## Conventions

- Commit messages look like `area: what changed` — e.g. `auth: refresh OAuth
  tokens on 401`, `docs(readme): fix stale slash-command list`. One logical
  change per commit.
- Leaf types and traits live in `crates/core`; user-config loading lives in
  `crates/config`. `core` never depends on `config`.
- Behaviour changes to the TUI event loop, mouse handling or terminal setup
  should read [`docs/knowhow.md`](docs/knowhow.md) first, and extend it when
  they fix a non-obvious bug. After those edits, run the manual host pass in
  [`docs/tui-term-matrix.md`](docs/tui-term-matrix.md)
  (`scripts/tui_term_matrix.sh`) on at least Alacritty and one VTE terminal.
- Platform-specific behaviour needs a `#[cfg]` branch, not a Unix-only
  assumption: tagged releases build for Linux, Windows and macOS.
- Agent-facing repo rules are in [AGENTS.md](AGENTS.md); they apply to human
  contributors just the same.
- Naming: Rust files and modules are `snake_case`; crate directories are
  short `kebab-case` (package name `whycode-<dir>`); docs outside the repo
  root are `kebab-case.md`; scripts are `snake_case` (`check_*`, `bench_*`,
  `update_*`). Do not repeat a parent directory in the file name
  (`git/blame.rs`, not `git/git_blame.rs`). Root meta files (`README.md`,
  `LICENSE`, `CONTRIBUTING.md`, `SECURITY.md`, `AGENTS.md`) stay uppercase.
  Agent-facing tool *string* names (`webfetch`, `git_status`) are a stable
  product API — do not rename them to match Rust modules.

## Reporting bugs

Open an issue with the command you ran, what you expected, and what happened.
For TUI problems, the last lines of
`~/.local/share/whycode/logs/unified.jsonl` are the fastest diagnostic — see
[docs/knowhow.md](docs/knowhow.md) for what the lifecycle events mean.

Security issues: please use the process in [SECURITY.md](SECURITY.md) instead
of a public issue.
