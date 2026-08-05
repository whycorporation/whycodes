# Whycode — agent rules

## Build after every change (required)

Whenever you edit Rust source, `Cargo.toml`, or anything that affects compilation:

1. **Rebuild before finishing the turn.** Do not leave the workspace uncompiled.
2. Prefer a targeted check first, then widen if needed:

   ```bash
   # Touched a single crate
   cargo check -p whycode-<crate>

   # Multiple crates or workspace-wide impact
   cargo check --workspace

   # CLI / binary path changed
   cargo build -p whycode-cli
   ```

3. If the change is non-trivial (logic, API, providers, agent loop, TUI), also run the relevant tests:

   ```bash
   cargo test -p whycode-<crate>
   # or
   cargo test -p whycode-<crate> --lib
   ```

4. **Fix compile errors in the same turn** before reporting done. A “done” response with a red `cargo check` is incomplete.
5. Docs-only, comment-only, or pure markdown/config prose that cannot affect the build may skip compile — when unsure, run `cargo check -p …` anyway.

### Why

Agents and the developer rely on a green tree. Unverified edits accumulate; the next session pays for them. Auto-build keeps feedback local and cheap.

## Commit and push after every change (required)

When a turn produces real project changes (source, config, docs that belong in the repo):

1. **Commit** on the current branch after the build (and relevant tests) are green. Do not leave a pile of uncommitted work for the user to remember.
2. **Push** to `origin` on the same branch (`git push -u origin HEAD` if needed). No force-push unless the user explicitly asks.
3. Use a clear commit message (what / why). Prefer one logical commit per turn; split only when the user prefers.
4. **Do not** commit secrets (API keys, `.env`, credentials), local junk (`.omo/`, scratch logs), or huge generated artifacts unless the project already tracks them.
5. If push fails (auth, non-fast-forward), report the error and stop — do not rewrite remote history.

The user should **not** have to say “commit and push” every time. This is the default for this repo.

Exceptions (skip commit/push unless asked): pure Q&A with no file edits; the user forbids commit for that turn; only secret or out-of-repo paths were touched.

## Workspace map (short)

| Crate | Path | Notes |
|-------|------|--------|
| `whycode-cli` | `crates/cli` | Binary entrypoint (`whycode`) |
| `whycode-tui` | `crates/tui` | Terminal UI |
| `whycode-agent` | `crates/agent` | Agent loop / tools orchestration |
| `whycode-llm` | `crates/llm` | Providers (OpenAI-compat, Anthropic, …) |
| `whycode-core` | `crates/core` | Leaf types, `Tool` trait, sandbox settings, errors, network, logging |
| `whycode-config` | `crates/config` | Config load/merge/validate (depends on core only) |
| `whycode-session` | `crates/session` | Conversation session |
| `whycode-tools` | `crates/tools` | Built-in tools (`file/`, `git/`, `github/`, `web/`, `agent_tools/`) |
| `whycode-memory` | `crates/memory` | Cross-session semantic / auto memory (MEMORY.md + hash embed) |
| `whycode-storage` | `crates/storage` | SQLite sessions + memories |

Package names use the `whycode-` prefix even when the directory is shorter (e.g. `crates/llm` → `-p whycode-llm`).

Dependency rule of thumb: **leaf types and traits stay in `core`**; I/O and policy trees that load user config live in `config`. Do not re-export `config` from `core` (cycle).

## Hard-won pitfalls (read when touching TUI / terminal)

See **[`docs/KNOWHOW.md`](docs/KNOWHOW.md)** — living log of silent exits, mouse/event-loop return values, `/dev/tty`, SIGPIPE, context-window vs rate limits, etc.

When you fix a non-obvious bug of that kind: **append a short entry** to that file (template at the bottom) so the next session does not repeat it.
