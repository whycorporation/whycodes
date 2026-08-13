# Architecture

The workspace is 23 crates with one-way layering:
**foundations → services → orchestration → applications**.

Allowed internal edges are allowlisted in
[`scripts/dependency_boundaries.json`](../scripts/dependency_boundaries.json)
and verified in CI.

Two rules keep the graph acyclic:

- `core` holds leaf types, the `Tool` trait, errors and logging, and depends
  on **no** other workspace crate.
- `config` (user-config loading and policy) depends only on `core`. `core`
  never re-exports `config`.

| Layer | Crate | Responsibility |
|---|---|---|
| Foundations | `core` | Leaf types, `Tool` trait, sandbox settings, errors, logging |
| | `command-risk` | Shell command risk classification (pure, no I/O) |
| | `auth` | OAuth: PKCE / device code, token store |
| | `config` | Config load / merge / validate |
| | `storage` | SQLite for sessions and memories |
| | `format` | Markdown, highlighting, diffs, tables |
| | `protocol` | CI / stream-json event envelopes |
| | `schema` | Schema validation |
| | `sandbox` | OS sandbox for shell commands (bubblewrap on Linux) |
| | `lsp` | Language-server client |
| | `skill` | Skill registry |
| | `function` | Function-tool helpers |
| Services | `llm` | LLM providers |
| | `session` | Conversation state, compaction, undo/redo |
| | `memory` | `MEMORY.md`, semantic recall, code index |
| | `plugin` | Shell plugin loader |
| | `tools` | Tool system and built-ins |
| | `mcp` | MCP client; tools bind as `{server}_{tool}` |
| Orchestration | `agent` | Agent loop, subagents, swarm, `AGENTS.md` |
| Applications | `cli` | The only binary (`whycode`) |
| | `tui` | Full-screen terminal UI (ratatui) |
| | `server` | Local HTTP API for session sharing |
| | `sdk` | Library client for embedding |

Package names use the `whycode-` prefix even when the directory is shorter
(`crates/llm` → `-p whycode-llm`).

Before changing the TUI event loop, mouse handling or terminal setup, read
[KNOWHOW.md](KNOWHOW.md).
