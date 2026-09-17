# WhyCodes — agent rules

## Build after every change (required)

Whenever you edit Rust source, `Cargo.toml`, or anything that affects compilation:

1. **Rebuild before finishing the turn.** Do not leave the workspace uncompiled.
2. Prefer a targeted check first, then widen if needed:

   ```bash
   # Touched a single crate
   cargo check -p whycodes-<crate>

   # Multiple crates or workspace-wide impact
   cargo check --workspace

   # CLI / binary path changed
   cargo build -p whycodes-cli
   ```

3. If the change is non-trivial (logic, API, providers, agent loop, TUI), also run the relevant tests:

   ```bash
   cargo test -p whycodes-<crate>
   # or
   cargo test -p whycodes-<crate> --lib
   ```

4. **Fix compile errors in the same turn** before reporting done. A “done” response with a red `cargo check` is incomplete.
5. Docs-only, comment-only, or pure markdown/config prose that cannot affect the build may skip compile — when unsure, run `cargo check -p …` anyway.
6. **CI Budgets + Clippy are not covered by `cargo check`.** After `.rs` or `Cargo.toml` edits, run these before finishing (seconds, same as the `Budgets` / `Check & Lint` jobs):

   ```bash
   python scripts/check_panic_budget.py
   python scripts/check_swallowed_error_budget.py
   python scripts/check_dependency_boundaries.py
   python scripts/check_tracked_secrets.py
   cargo fmt --all --check
   cargo clippy -p whycodes-<crate> --all-targets -- -D warnings
   ```

   Formatter/clippy policy lives in-tree: `rustfmt.toml` +
   `[workspace.lints.clippy]` (`correctness` / `suspicious` deny). CI
   `clippy -- -D warnings` is unchanged. License/source gate is
   `cargo deny check licenses sources` (`deny.toml`); advisories stay on
   `cargo audit`.

   A `let _ = send(...)`, `Err(_) =>`, `return x.ok();`, or a new `whycodes-*` Cargo.toml line will fail CI even when the crate compiles. Handle the error (name it / log it) or register the edge in `scripts/dependency_boundaries.json` in the **same** commit. Details: [`docs/knowhow.md`](docs/knowhow.md) rule 8.

### Why

Agents and the developer rely on a green tree. Unverified edits accumulate; the next session pays for them. Auto-build keeps feedback local and cheap.

## Commit and push after every change (required)

When a turn produces real project changes (source, config, docs that belong in the repo):

1. **Commit** on the current branch after the build (and relevant tests) are green. Do not leave a pile of uncommitted work for the user to remember.
2. **Push** to `origin` on the same branch (`git push -u origin HEAD` if needed). No force-push unless the user explicitly asks.
3. Use a clear commit message (what / why). Prefer one logical commit per turn; split only when the user prefers.
4. **Do not** commit secrets (API keys, `.env`, credentials), local junk (`.omo/`, `.whycode/`, scratch logs), or huge generated artifacts unless the project already tracks them. CI runs `python scripts/check_tracked_secrets.py`. Do not `git push --mirror` (local Cline checkpoint refs stay on the machine).
5. If push fails (auth, non-fast-forward), report the error and stop — do not rewrite remote history.

The user should **not** have to say “commit and push” every time. This is the default for this repo.

Exceptions (skip commit/push unless asked): pure Q&A with no file edits; the user forbids commit for that turn; only secret or out-of-repo paths were touched.

## Releases (required when the user asks to ship)

`main` is ruleset-protected: **no direct pushes**, including `gh pr merge --admin`.
Everything lands through a PR whose required checks actually run.

1. **Never open a docs-only PR** (`**.md`, `docs/**`, `landing/**`).
   `ci.yml` `pull_request.paths-ignore` skips those paths, so the four required
   checks stay `expected` forever and the PR cannot merge. Bundle a `.rs` /
   `Cargo.toml` / `Formula/` / workflow change, or the PR is stuck.
2. **Patch / minor:** bump `[workspace.package] version` in the root
   `Cargo.toml` and `cd sdk/typescript && npm version X.Y.Z --no-git-tag-version`.
   `cargo check -p whycodes-cli` refreshes `Cargo.lock`. Open `release/vX.Y.Z`,
   wait for Lint / Test (linux) / Coverage / Build (linux), merge.
3. **Tag the merge commit**, not the bump commit:
   `git tag -a vX.Y.Z <merge-sha> && git push origin vX.Y.Z`.
   That starts `.github/workflows/release.yml` (4 targets + publish + landing).
   Do **not** re-tag if a job fails.
4. **Homebrew job always fails** (it pushes `Formula/whycodes.rb` straight to
   `main`; the ruleset rejects it). After the GitHub release exists, run
   `scripts/update_homebrew_formula.sh vX.Y.Z` on a `chore/homebrew-vX.Y.Z`
   branch and open a PR. `Formula/**` is ignored on **push** to main, **not**
   on pull requests — a formula-only PR must still run CI (v0.6.3 deadlock).
5. Coverage flakes (`poll_matches_adopts_fuzzy_hits…` and similar) on a
   formula-only PR are not a product regression. `gh run rerun <id> --failed`;
   do not bump versions or re-tag to “fix” them.

Playbook detail: [`docs/packaging.md`](docs/packaging.md). Recurring traps:
[`docs/knowhow.md`](docs/knowhow.md) (2026-09-17 update-modal / fast-path entries).

## Interactive TUI fast path (do not strip)

Bare `whycodes` / `whycodes -d <dir>` / `whycodes run -d <dir>` skip clap and
`async_main` (`early_tui_run_dir_from` → `cmd_run_fast_tui`). That **is** the
interactive default. Extra flags (`--plain`, `--no-auto-update`, a prompt)
still take `cmd_run`.

`cmd_run_fast_tui` must keep all of:

- `ignore_sigpipe()` + `logging::init` (env-only level, `with_stderr: false`)
- `spawn_update_check_if(should_auto_update_fast_path())` **inside** the Tokio
  runtime (`tokio::spawn` panics without one)
- `after_tui_exit` on `TuiExit::Upgrade`

Do **not** pass `update_rx: None` on this path. `WHYCODES_BENCH` may still
early-return before the runtime. Remote attach (`cmd_connect`) is allowed to
skip the GitHub check. `crates/cli/src/cmd/run_tests.rs` ratchets the source
so a TTFF cleanup cannot drop the spawn again.

## Measure coverage locally before pushing (required)

Do **not** push a PR (or a coverage-related commit) until the same floors CI
checks have passed **on this machine**. Guessing from CI logs and iterating
on `origin` is not allowed.

Run this when any of the following changed: `scripts/coverage.sh`,
`scripts/check_coverage_floors.py`, `scripts/llvm_cov_skip_expansions.sh`,
`.github/workflows/ci.yml` Coverage job, a crate in
`scripts/check_coverage_floors.py` `FULL_COVER_CRATES`, or production lines
that those 100% floors count.

```bash
# Same wrapper as CI `Coverage (line floor)`. Needs cargo-llvm-cov +
# llvm-tools-preview. Windows: use Git Bash; rustup llvm-cov is under
# lib/rustlib/<host>/bin (the wrapper prepends it).
scripts/coverage.sh
# or, if system sqlite is missing:
COVERAGE_FEATURES=whycodes-storage/bundled scripts/coverage.sh
```

Pass means the script exits 0 and prints `OK` for the workspace 82% floor
and every crate floor. A red CI Coverage job is not a substitute for this
run. How to measure, flags, and floors: [`docs/coverage.md`](docs/coverage.md).

## Workspace map

26 crates, one-way layering. Full map and allowed edges:
[`docs/architecture.md`](docs/architecture.md).

Package names use the `whycodes-` prefix even when the directory is shorter
(e.g. `crates/llm` → `-p whycodes-llm`). The TypeScript SDK lives at
`sdk/typescript` (twin of `crates/sdk`), not under `crates/`.

Dependency rule of thumb: **leaf types and traits stay in `core`**; I/O and policy trees that load user config live in `config`. Do not re-export `config` from `core` (cycle).

## Hard-won pitfalls (read when touching TUI / terminal)

See **[`docs/knowhow.md`](docs/knowhow.md)** — living log of silent exits, mouse/event-loop return values, `/dev/tty`, SIGPIPE, context-window vs rate limits, etc.

When you fix a non-obvious bug of that kind: **append a short entry** to that file (template at the bottom) so the next session does not repeat it. Shipping a tag: follow **Releases** above. Touching `cmd_run_fast_tui`: follow **Interactive TUI fast path**.
