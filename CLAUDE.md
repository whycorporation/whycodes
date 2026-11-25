# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Repository rules

@AGENTS.md

`AGENTS.md` (above) is the shared rulebook: build after every change, the budget ratchets, commit/push, releases, coverage, and the workspace map. This file only adds what it does not cover. `CONTRIBUTING.md` is the short pre-PR checklist. For terminal event-loop, mouse, clipboard, `/dev/tty`, or signal changes, read `docs/knowhow.md` first and run the manual checks in `docs/tui-term-matrix.md`.

## Run and test

```bash
cargo run -p whycodes-cli -- -d .

# One test by name / one integration test exactly
cargo test -p whycodes-<crate> <test-name>
cargo test -p whycodes-cli --test cli_args test_cli_help -- --exact
```

Before a PR, also run workspace-wide `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, the tests, and `python scripts/check_sdk_protocol.py`. CI also checks benchmark ceilings, installer scripts, a release workspace build, and coverage (98.5% workspace floor; selected foundational crates have 100% floors): see `.github/workflows/ci.yml` and `docs/coverage.md`.

### TypeScript SDK

The separate protocol-v1 client is in `sdk/typescript` and requires Node.js 18+:

```bash
cd sdk/typescript
npm ci
npm run build
npm test

# One test file (build first because tests import dist/)
npm run build
node --test test/sse.test.mjs

# One named Node test
node --test --test-name-pattern="<pattern>" test/<file>.test.mjs
```

Changes to `crates/protocol/src/sdk.rs` or the TypeScript client must keep `sdk/typescript/src/types.ts` synchronized and pass `python scripts/check_sdk_protocol.py`.

## Architecture

The 27 Rust crates follow a checked, one-way dependency graph:

**foundations → services → orchestration → applications**

The complete crate map and rationale are in `docs/architecture.md`; allowed internal edges are machine-checked from `scripts/dependency_boundaries.json`.

- **Foundations** own narrow infrastructure and leaf concepts: `core`, `auth`, `config`, `storage`, `protocol`, `format`, `schema`, `sandbox`, `shell`, `lsp`, `skill`, `function`, `command-risk`, `slop`, `index`, and `import`.
- **Services** add domain behavior: `llm`, `session`, `memory`, `plugin`, `tools`, and `mcp`.
- **Orchestration** lives in `agent`: the model/tool loop, context compaction, permission and sandbox gates, subagents, and tool execution scheduling.
- **Applications** are `cli`, `tui`, `server`, and the Rust `sdk`. `whycodes-cli` is the composition root and produces the repository's only executable, `whycodes`; the server and self-update features are enabled by default.

### Runtime flow

1. `crates/cli/src/main.rs` parses commands, chooses a Tokio runtime, loads layered configuration, resolves provider/model/project credentials, opens storage, starts indexing, and constructs the agent/session.
2. Interactive execution enters either the ratatui TUI or plain REPL; one-shot commands use the same lower-level services.
3. `crates/agent/src/agent/` owns each turn (`turn.rs`): build context and visible tools, stream the LLM response, assemble tool calls, execute them through risk/intent/path/permission/sandbox/hook gates (`gate.rs` / `dispatch.rs`), append results, and repeat until no tool calls remain. `Agent` in `mod.rs` is the facade.
4. Read-only tool calls may run concurrently. Mutating, shell, and interactive calls are serialized.
5. Session and message persistence is best-effort around the turn loop; storage behavior and SQLite migrations remain inside `whycodes-storage`.

### Authentication, persistence, and protocols

- `whycodes-auth` owns provider definitions, OAuth/PKCE/device-code flows, credential discovery, refresh data, and the protected `auth.json` token store. The CLI coordinates provider-specific credential precedence and registers refresh behavior with `whycodes-llm`.
- `whycodes-storage` is the SQLite boundary for sessions, messages, state, memories, session chunks, and code chunks. The CLI selects `<Config::data_dir()>/whycodes.db`; other crates should use storage APIs rather than access SQLite directly.
- `whycodes-protocol` owns stream/CI envelopes and daemon protocol v1 (`SdkEvent`). `whycodes-server` exposes `/api/*` for TUI attachment and `/v1/*` for SDK clients. The Rust and TypeScript SDKs are deliberately thin clients over that protocol.
- `whycodes-mcp` binds remote tools as `{server}_{tool}`. Agent-facing tool string names are stable product API and should not be renamed merely to match Rust module names.

Tests live with their owning crates (inline unit tests and crate-local `tests/` integration targets), not in a central test package. Prefer the narrowest relevant test during iteration, then widen based on dependency impact.
