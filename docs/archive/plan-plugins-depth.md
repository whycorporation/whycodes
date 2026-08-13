# Plan — Plugins depth

**Status:** done · **Shipped:** 2026-08-14 · **Living tracker:** [../roadmap.md](../roadmap.md)

Wire `PluginManager` / `plugin.json` discovery into the agent. Marketplace
stays out. `plugins.toml` → `plugin_*` and config hooks already shipped.

## Tasks

| # | Slice | Status |
|---|---|---|
| 1 | Discover `$CONFIG/plugins/*/plugin.json` and `.whycode/plugins/*/plugin.json` | [x] |
| 2 | Register as `plugin_*` (relative command + cwd; infer `run.sh`) | [x] |
| 3 | Last-wins merge: toml then json; project over global | [x] |
| 4 | `whycode plugins` lists both sources | [x] |

## Non-goals

- Plugin marketplace
- Loading `hooks` from `plugin.json` (config `[hooks]` remains the hook path)
