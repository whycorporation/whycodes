# Architecture

The workspace is 24 crates with one-way layering:
**foundations → services → orchestration → applications**.

Allowed internal edges are allowlisted in
[`scripts/dependency_boundaries.json`](../scripts/dependency_boundaries.json)
and verified in CI.

Two rules keep the graph acyclic:

- `core` holds leaf types, the `Tool` trait, errors, logging and
  `paths` (`WHYCODES_HOME`). It depends on **`index` only** among workspace
  crates (file-index types). It never depends on `config`.
- `config` (user-config loading and policy) depends only on `core`. `core`
  never re-exports `config`.

| Layer | Crate | Responsibility |
|---|---|---|
| Foundations | `core` | Leaf types, `Tool` trait, sandbox settings, errors, logging, `paths` (`WHYCODES_HOME`) |
| | `command-risk` | Shell command risk classification (pure, no I/O) |
| | `auth` | OAuth: PKCE / device code, token store |
| | `config` | Config load / merge / validate |
| | `storage` | SQLite for sessions and memories |
| | `format` | Markdown, highlighting, diffs, tables |
| | `protocol` | CI / stream-json envelopes **and** daemon protocol v1 (`SdkEvent`) |
| | `schema` | Schema validation |
| | `sandbox` | OS sandbox for shell commands (bubblewrap on Linux) |
| | `lsp` | Language-server client |
| | `skill` | Skill registry |
| | `function` | Function-tool helpers |
| | `index` | Workspace file index (`ignore` walk, fuzzy, `notify`) |
| Services | `llm` | LLM providers |
| | `session` | Conversation state, compaction, undo/redo |
| | `memory` | `MEMORY.md`, semantic recall, code index |
| | `plugin` | Shell plugin loader |
| | `tools` | Tool system and built-ins |
| | `mcp` | MCP client; tools bind as `{server}_{tool}` |
| Orchestration | `agent` | Agent loop, subagents, swarm, `AGENTS.md` |
| Applications | `cli` | The only binary (`whycodes`) |
| | `tui` | Full-screen terminal UI (ratatui) |
| | `server` | Local daemon (`whycodes serve`): `/api/*` for TUI attach, `/v1/*` for the SDK |
| | `sdk` | Thin HTTP client for protocol v1 (`connect` / `launch`). TypeScript twin: `sdk/typescript`. |

Package names use the `whycodes-` prefix even when the directory is shorter
(`crates/llm` → `-p whycodes-llm`).

## Repository layout

| Path | What |
|---|---|
| `crates/` | Rust workspace (24 crates above) |
| `sdk/typescript/` | TypeScript SDK — protocol twin of `crates/sdk` |
| `docs/` | User and contributor docs (this file, guide, know-how) |
| `scripts/` | Installers, CI budget checks, benches |
| `Formula/` | Homebrew formula — this repo *is* the tap, so the file must sit here |
| `.github/workflows/` | CI and tagged-release packaging |

Distribution notes: [packaging.md](packaging.md).

Before changing the TUI event loop, mouse handling or terminal setup, read
[knowhow.md](knowhow.md).
