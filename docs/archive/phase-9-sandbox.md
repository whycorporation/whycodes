# Phase 9 — Shell OS sandbox (OS / network)

**Status:** done (2026-08-04) · **Depends on:** phase 1 (command-risk) · **Blocks:** nothing

## Problem

`command-risk` classifies shell *strings*. A model that writes a script then
runs it, or that obfuscates a delete, can still escape that gate. Shell ran as
host `bash -c` with full filesystem and network access.

## Goal

Confine shell tool execution so blast radius is limited even when the string
gate misses — without breaking ordinary agent work (`cargo build`, `npm i`,
`git`).

## Design

- New crate `whycodes-sandbox`.
- Config under `[security]`:
  - `sandbox = "workspace" | "off"` (default `workspace`)
  - `sandbox_network = true | false` (default `true`)
  - `sandbox_fallback = "allow" | "deny"` (default `allow`)
- Linux backend: bubblewrap with RO host root, RW project, private `/tmp`,
  limited RW toolchain caches, optional `--unshare-net`.
- Non-Linux / missing bwrap: honour `sandbox_fallback`.
- Stacks with permission map and risk classifier; does not replace them.

## Out of scope

- macOS seatbelt / Windows AppContainer backends (fallback only).
- Domain-level network allowlists.
- Sandboxing non-shell tools (`webfetch` stays a normal HTTP client).
- Multi-tenant hard security claims.

## Acceptance criteria

- [x] `sandbox=workspace` + bwrap: project writes succeed; `$HOME` writes fail
- [x] `sandbox_network=false`: outbound TCP fails inside shell
- [x] `sandbox=off`: host bash (previous behaviour)
- [x] Missing bwrap + fallback allow: host run with warning
- [x] Documented honestly in README
