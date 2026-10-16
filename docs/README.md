# Documentation

Start here, then pick the page you need.

## By audience

| Doc | Audience | What it is |
|---|---|---|
| [guide.md](guide.md) | Users | CLI, TUI, agents, tools, memory, configuration, SDK |
| [tools/](tools/README.md) | Users | One page per built-in tool (generated from the live catalog) |
| [config.md](config.md) | Users | Every config key: default, env override, meaning |
| [providers.md](providers.md) | Users, contributors | Provider catalogue: special cases, stream behaviour, auth, model handling |
| [auth.md](auth.md) | Users | API keys, OAuth login, credential import |
| [examples/](examples/README.md) | Users | Copy-ready plugin, slash command and hooks; loaded by a test |
| [internals/](internals/) | Contributors | How the turn loop, permission gate, compaction, prompt cache, retry and MCP lifecycle work |
| [architecture.md](architecture.md) | Contributors | Crate map and one-way layering |
| [rust-best-practices.md](rust-best-practices.md) | Contributors | Rust idiom audit (errors, modules, async, panics) |
| [roadmap.md](roadmap.md) | Contributors | Current focus and deferred work |
| [knowhow.md](knowhow.md) | Contributors | Bugs that are easy to reintroduce |
| [errata.md](errata.md) | Contributors | Claims that were retracted, with the evidence |
| [message-voice.md](message-voice.md) | Contributors | How user-facing strings are written |
| [acceptance/](acceptance/TEMPLATE.md) | Contributors | Requirement-to-observation ledgers for features that need real-world checks |
| [proposals/](proposals/embedded-shell-spike.md) | Contributors | Design spikes that are not decided yet (status header says so) |
| [tui-term-matrix.md](tui-term-matrix.md) | Contributors | Manual TUI pass on Alacritty / Kitty / VTE |
| [benchmarks.md](benchmarks.md) | Contributors | How to measure startup, RSS, idle draws |
| [coverage.md](coverage.md) | Contributors | How to measure line coverage |
| [budgets.md](budgets.md) | Contributors | CI quality budgets (panic, swallow, edges) |
| [../CONTRIBUTING.md](../CONTRIBUTING.md) | Contributors | Setup and required checks |
| [../CODE_OF_CONDUCT.md](../CODE_OF_CONDUCT.md) | Everyone | Community standards |
| [../SUPPORT.md](../SUPPORT.md) | Everyone | Where to ask for help; GitHub Sponsors |
| [../SECURITY.md](../SECURITY.md) | Everyone | Vulnerability reporting |
| [../AGENTS.md](../AGENTS.md) | Agents | Build, commit, and crate rules |
| [packaging.md](packaging.md) | Maintainers | Homebrew, installers, release download counts |

The product landing page is the [root README](../README.md).

## Internals pages

| Page | Covers |
|---|---|
| [internals/turn-loop.md](internals/turn-loop.md) | One turn: hooks, steering, context assembly, streaming, tool scheduling, termination, persistence |
| [internals/permission-gate.md](internals/permission-gate.md) | The ordered gates a tool call passes: rules, shell risk, paths, hooks, sandbox, headless policy |
| [internals/compaction.md](internals/compaction.md) | Triggers, the full-replace summary, what survives, the circuit breaker |
| [internals/prompt-cache.md](internals/prompt-cache.md) | Request block order, what changes between turns, miss detection, keep-alive, preconnect |
| [internals/retry.md](internals/retry.md) | Error classes, backoff, credential rotation, fallback models, replay after side effects |
| [internals/mcp-lifecycle.md](internals/mcp-lifecycle.md) | Connect, bind, resources and prompts, execution, failure and shutdown |

## Conventions

- **Status header.** Design and internals pages start with
  `> Status: Design | Implemented | Partial (…) | Verified | Living · Updated: YYYY-MM-DD`.
  Change the date when the content changes, not when a link does.
- **Implementation files and Known limitations.** Every internals page ends
  with the crate paths it describes and the gaps it found. A page without a
  limitations section has not been checked against the code.
- **Generated pages are not edited.** `tools/*.md` come from
  `tools/catalog.json` via `scripts/gen_tool_docs.py`; CI fails when they
  drift (`--check`) and a CLI test pins the catalog to the binary.
- **Acceptance ledgers** live in `acceptance/<feature>.md` from the
  [template](acceptance/TEMPLATE.md) when a feature touches a real provider,
  terminal, sandbox, network or process.
- **Errata, not silent edits.** A retracted claim gets an entry in
  [errata.md](errata.md) with the original wording.
- **Describe behaviour, do not cite other tools.** Interoperability paths
  (file names other agents read) are fine; comparisons are not.
