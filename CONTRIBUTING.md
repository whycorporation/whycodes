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
```

Notes:

- Clippy runs with `--all-targets`, so test code is linted under `-D warnings`
  too.
- The budget scripts are ratchets: counts may only go down. If your change
  legitimately needs a new panic site or a swallowed error, the budget file
  change is reviewed as part of the PR.
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
  should read [`docs/KNOWHOW.md`](docs/KNOWHOW.md) first, and extend it when
  they fix a non-obvious bug.
- Platform-specific behaviour needs a `#[cfg]` branch, not a Unix-only
  assumption: tagged releases build for Linux, Windows and macOS.
- Agent-facing repo rules are in [AGENTS.md](AGENTS.md); they apply to human
  contributors just the same.

## Reporting bugs

Open an issue with the command you ran, what you expected, and what happened.
For TUI problems, the last lines of
`~/.local/share/whycode/logs/unified.jsonl` are the fastest diagnostic — see
[docs/KNOWHOW.md](docs/KNOWHOW.md) for what the lifecycle events mean.

Security issues: please use the process in [SECURITY.md](SECURITY.md) instead
of a public issue.
