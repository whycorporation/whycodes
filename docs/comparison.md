# Where whycode stands

Background for the phased plan. Measured 2026-07-31 against
[jcode](https://github.com/1jehuang/jcode) at a shallow clone of `master`.

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

whycode measurements (release, see [benchmarks.md](benchmarks.md)): ~21 ms
`--version`, ~9.6 MB peak RSS for CLI floor, ~4.7 ms in-process first TUI
frame, and multi-session idle PSS of **~4.1 MB** (1 session) / **~16.8 MB**
(10 sessions) on Linux. jcode's published 27.8 MB / 117 MB figures remain its
own claims.

## What whycode has that is worth keeping

- A dedicated `lsp` crate. jcode has no LSP crate in its workspace.
- Verified cross-platform behaviour. Linux, macOS and Windows all run the full
  test suite in CI, and two genuine Windows defects were fixed on 2026-07-30
  (the `grep` tool shelled out to `which`/`grep`, and plugin execution
  hardcoded `sh -c`).
- A small, readable codebase. 24k lines is an asset while the design is still
  moving.

## What jcode has that whycode does not

Ordered by how much each one costs a whycode user today.

| Gap | Consequence | Phase |
|---|---|---|
| No shell risk classification | `bash = "allow"` executes anything the model emits, including `rm -rf ~` | [1](1.md) |
| No binaries, no working self-update | A user must have a Rust toolchain; `whycode upgrade` only prints instructions | [2](2.md) |
| API keys only | No OAuth, no reuse of credentials already on the machine | [3](3.md) |
| No quality budgets | Panics, swallowed errors and binary size drift silently | [4](4.md) |
| No benchmarks | No basis for any performance statement | [5](5.md) |
| No memory across sessions | Every session starts cold | [6](6.md) |
| Subagents but no coordination | `task` spawns one subagent; no parallel agents on one repo | [7](7.md) |

## Positioning

"OpenCode parity in Rust" is the goal recorded in whycode's own history
(`feat: OpenCode-parity TUI, CLI, providers — full rewrite`). It is a weak
position: a user comparing whycode to OpenCode has no reason to pick the copy,
and jcode already occupies the "measurably more efficient alternative" slot
with numbers attached.

The plan below does not pick a new slogan. It closes the gaps that make
whycode unusable by anyone who is not already building it from source, and
Phase 5 produces the measurements any future positioning claim would need.
Two candidate axes are visible from the comparison and are recorded here so
the option is not lost:

- **First-class Windows support.** Rust terminal tooling is routinely weak
  here, and whycode already runs its full suite on Windows in CI. jcode has a
  `windows-smoke.yml` workflow, so this is contested, not open.
- **LSP depth.** whycode has an LSP crate; jcode's workspace does not.

Neither is a decision yet. Making one requires Phase 5's data.
