# Where whycode stands

Background snapshot for the roadmap ([status.md](status.md)). Measured
2026-07-31 against [jcode](https://github.com/1jehuang/jcode) at a shallow
clone of `master`. Numbers may be stale; re-measure via [benchmarks.md](benchmarks.md).

## Numbers

| | whycode | jcode |
|---|---|---|
| First commit | 2026-07-09 | 2026-01-05 |
| Commits | 40 | 6,408 |
| Rust LOC | 23,955 | 598,753 |
| Crates | 18 | 79 |
| Stars / forks | — | 14.2k / 1.56k |
| Open issues | — | 176 |
| Distribution | source build only | install script, Homebrew tap, PowerShell installer |
| CI workflows | 1 | 7 |

Two caveats on those numbers, so they are read correctly:

- jcode's 79 crates overstate its modularity. Three crates hold 400k of the
  598k lines — `jcode-tui` (182k), `jcode-app-core` (119k), `jcode-base`
  (93k). Much of the remaining crate count is small `*-types` splits.
- jcode averages ~30 commits per day since January and ships `.claude/` and
  `AGENTS.md` in-repo, so it is heavily agent-authored. Commit count is not a
  proxy for human effort on either side.

## Claims we have not verified

jcode's README advertises 27.8 MB RSS per session against Claude Code's
386.6 MB, and 14.0 ms time-to-first-frame against 3436.9 ms. We have not
reproduced these. Note that the memory figure is stated with local embeddings
disabled, and that jcode's embedding crate loads an ONNX `all-MiniLM-L6-v2`
model in-process, so the enabled figure will be materially higher.

whycode measurements (release, see [benchmarks.md](benchmarks.md)): **~1.3 ms**
`--version` on Linux after the 2026-08-05 boot-path cut (Windows baseline was
~21 ms on a larger binary), ~9.6 MB peak RSS for CLI floor, ~4.7 ms in-process
first TUI frame, and multi-session idle PSS of **~4.1 MB** (1 session) /
**~16.8 MB** (10 sessions) on Linux. jcode's published 27.8 MB / 117 MB figures
remain its own claims.

## What whycode has that is worth keeping

- A dedicated `lsp` crate. jcode has no LSP crate in its workspace.
- Verified cross-platform behaviour. Linux, macOS and Windows all run the full
  test suite in CI, and two genuine Windows defects were fixed on 2026-07-30
  (the `grep` tool shelled out to `which`/`grep`, and plugin execution
  hardcoded `sh -c`).
- A small, readable codebase. 24k lines is an asset while the design is still
  moving.

## Gaps then vs now

Snapshot from 2026-07-31, with current status (2026-08-04):

| Gap (2026-07-31) | Then | Now |
|---|---|---|
| No shell risk classification | open | **done** — [archive/phase-1](archive/phase-1-command-risk.md) + OS sandbox [phase-9](archive/phase-9-sandbox.md) |
| No binaries / self-update | open | **implemented** — [plan-distribution](plan-distribution.md); first `v*` tag still needed |
| API keys only | open | **blocked** — [plan-oauth](plan-oauth.md) (owner terms decision) |
| No quality budgets | open | **done** — [budgets.md](budgets.md), [archive/phase-4](archive/phase-4-ci-budgets.md) |
| No benchmarks | open | **mostly done** — [benchmarks.md](benchmarks.md), residual [plan-performance](plan-performance.md) |
| No memory across sessions | open | **not started** — [plan-memory](plan-memory.md) |
| No multi-agent coordination | open | **dropped** — [archive/phase-7](archive/phase-7-multi-agent.md) |

Living feature matrix vs other products: [FEATURES.md](FEATURES.md).

## Positioning

"OpenCode parity in Rust" is a weak goal (recorded in early history): a user
comparing whycode to OpenCode has no reason to pick the copy, and jcode already
occupies the "measurably more efficient alternative" slot with numbers.

The roadmap does not pick a new slogan; it closes ship-blockers first. Candidate
axes still on the table once measurements are trusted:

- **First-class Windows support** — full suite in CI; contested by jcode smoke.
- **LSP depth** — whycode has an LSP crate; jcode's workspace did not at measure time.

Neither is a product decision yet. See [status.md](status.md).
