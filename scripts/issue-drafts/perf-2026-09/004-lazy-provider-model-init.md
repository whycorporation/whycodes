# Lazy provider and model catalog initialization for fast cold start

## Problem
Every CLI invocation — including `--version`, `--help`, `config show`, and `session list` — pays for provider registry construction, model catalog loading, and config merges before it knows it will never make an LLM call. This dominates the 1-2ms `--version` floor and inflates peak RSS for short-lived commands.

## Proposal
Defer provider/model initialization until the first LLM-bound turn.

**How it works:**
- In `crates/cli/src/main.rs`, parse args and match light commands (`--version`, `--help`, `completions`, `config show`, `session list`, `mcp`) before constructing `ProviderRegistry` or touching the model catalog. These paths use a `current_thread` runtime or no runtime at all (already partially done — extend the allowlist).
- In `crates/llm/src/provider.rs`, make `ProviderRegistry::get` / `list_models` lazily instantiate the concrete provider on first use via `OnceLock`, rather than eagerly building all providers from `config.toml` at startup.
- Gate the `/v1/models` context-window fetch (`WHYCODES_NO_MODEL_CATALOG` path) behind the first turn that actually needs a context window; light commands never hit the network.

**Expected benefit:** `--version` stays at ~1ms p50 even as provider count grows; `config show` / `session list` RSS drops by ~3-4 MB; fewer cold-start syscalls on every invocation.
