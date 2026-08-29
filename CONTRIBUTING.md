# Contributing to WhyCodes

Thank you for taking the time to contribute. This document is the short path
from clone to a merged change. Participation is covered by the
[Code of Conduct](CODE_OF_CONDUCT.md).

## Getting started

Stable Rust is the only requirement:

```bash
git clone https://github.com/whycorporation/whycodes.git
cd whycodes
cargo build -p whycodes-cli
```

Then install the repository git hooks once, so a stray `git push --mirror`
cannot publish local checkpoints or stash:

```bash
sh scripts/install_git_hooks.sh
```

The pre-push hook (`scripts/pre-push`) only allows `refs/heads/*` and
`refs/tags/*`; checkpoint refs under `refs/cline/` stay on your machine.

## Before opening a pull request

CI runs exactly these commands and fails on any of them:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
python scripts/check_panic_budget.py
python scripts/check_swallowed_error_budget.py
python scripts/check_dependency_boundaries.py
python scripts/check_sdk_protocol.py   # Rust ↔ TypeScript protocol v1 tags
python scripts/check_tracked_secrets.py
```

If you changed `crates/protocol/src/sdk.rs` or the TypeScript client, also
run the SDK tests:

```bash
cd sdk/typescript && npm ci && npm test
```

Things to know about CI:

- Clippy runs with `--all-targets`, so test code is linted under
  `-D warnings` too.
- The budget scripts are **ratchets**: counts may only go down. If your
  change legitimately needs a new panic site or swallowed error, the budget
  file change is reviewed as part of the PR.
- Coverage is enforced with `cargo llvm-cov`: the workspace fails below 82%
  line coverage, and twelve foundational crates are locked at 100%. See
  [docs/coverage.md](docs/coverage.md).
- Internal crate edges are allowlisted in
  `scripts/dependency_boundaries.json`. Adding an edge is a deliberate
  architectural decision — see [docs/architecture.md](docs/architecture.md).
- Linux CI runs on a **self-hosted** runner, so pull requests from forks are
  skipped there on purpose (untrusted workflow + checkout on that machine).
  Open the PR anyway; a maintainer will run the suite from a same-repo
  branch.

## Conventions

### Commits

Commit messages look like `area: what changed` — for example
`auth: refresh OAuth tokens on 401` or
`docs(readme): fix stale slash-command list`. One logical change per commit.

### Code placement

- Leaf types and traits live in `crates/core`; user-config loading lives in
  `crates/config`. `core` never depends on `config`.
- Platform-specific behaviour needs a `#[cfg]` branch, not a Unix-only
  assumption: tagged releases build for Linux, Windows and macOS.

### Naming

- Rust files and modules are `snake_case`; crate directories are short
  `kebab-case` (package name `whycodes-<dir>`).
- Docs outside the repo root are `kebab-case.md`; scripts are `snake_case`
  (`check_*`, `bench_*`, `update_*`).
- Do not repeat a parent directory in the file name (`git/blame.rs`, not
  `git/git_blame.rs`).
- Root meta files (`README.md`, `LICENSE`, `CONTRIBUTING.md`, `SECURITY.md`,
  `CODE_OF_CONDUCT.md`, `SUPPORT.md`, `AGENTS.md`) stay uppercase.
- Agent-facing tool *string* names (`webfetch`, `git_status`) are a stable
  product API — do not rename them to match Rust modules.

### TUI and terminal changes

Behaviour changes to the TUI event loop, mouse handling or terminal setup
should read [docs/knowhow.md](docs/knowhow.md) first, and extend it when they
fix a non-obvious bug. After such edits, run the manual host pass in
[docs/tui-term-matrix.md](docs/tui-term-matrix.md)
(`scripts/tui_term_matrix.sh`) on at least Alacritty and one VTE terminal.

### Agent rules

Agent-facing repository rules are in [AGENTS.md](AGENTS.md); they apply to
human contributors just the same.

## Secrets and local scratch

Do not commit `.env`, `auth.json`, private keys, `.omo/`, `.whycode/`,
`.whycodes/todos/`, coverage dumps, or editor scratch. CI runs
`python scripts/check_tracked_secrets.py` on the index (not rewritten
history). Author emails in `git log` are left as-is.

## Code of Conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md).
Unacceptable behaviour can be reported as described there — do not put
private details in a public issue.

## Reporting bugs

Open an issue (templates under `.github/ISSUE_TEMPLATE/`) with the command
you ran, what you expected, and what happened. More on where to ask:
[SUPPORT.md](SUPPORT.md).

For TUI problems, the last lines of
`~/.local/share/whycodes/logs/unified.jsonl` are the fastest diagnostic — see
[docs/knowhow.md](docs/knowhow.md) for what the lifecycle events mean.

Issues labelled `good first issue` are meant for a first contribution.

**Security issues:** please use the process in [SECURITY.md](SECURITY.md)
instead of opening a public issue.
