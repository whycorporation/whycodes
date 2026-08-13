# Roadmap

Current focus and standing decisions. Completed phase plans live under
[archive/](archive/README.md) — do not treat them as open work.

Last updated: **2026-08-13**.

## Now

1. **Public release** — repo public, Homebrew binary formula, Windows
   install smoke. Release assets already cut as `v0.1.0`.
2. **Plugins depth** — `plugins.toml` → `plugin_*` tools;
   `whycode plugins list`; project + global load. Marketplace is out of
   scope.
3. **Performance residual (optional)** — live provider token reconcile
   against a real API session.

## Deferred

| Item | Notes |
|---|---|
| ACP (Agent Client Protocol) | Editor ↔ agent JSON-RPC. `whycode acp` is a stub. After product launch. |
| `web` surface | Same band as ACP. Use `whycode serve` + a browser for local share. |

## Standing decisions

These are still in force. The full historical log is
[archive/status.md](archive/status.md).

- The goal is not “OpenCode parity”.
- Safety before features: shell risk classification and the OS sandbox stay
  on by default.
- Default `bash_risk_threshold` is `destructive`, not `caution`.
- Unrecognised commands are `safe`; unresolvable targets escalate to
  `destructive`, never `catastrophic`.
- Catastrophic commands cannot be approved.
- Sandbox defaults to `workspace`, network on, fallback allow (so
  non-Linux keeps working).
- Public release is last: coding, perf and plugins rank ahead of opening
  the repo.
