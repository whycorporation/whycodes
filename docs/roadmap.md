# Roadmap

Current focus and standing decisions. Completed phase plans live under
[archive/](archive/README.md) — do not treat them as open work.

Last updated: **2026-08-14**.

## Now

Nothing open. Later stays later.

## Later

| Item | Notes |
|---|---|
| Public release | **Last.** Repo public, Windows install smoke. Homebrew binary formula auto-bumps from `release.yml`. Assets already cut as `v0.1.0`. |
| ACP | Editor ↔ agent JSON-RPC. `whycode acp` is a stub. |
| `web` surface | Same band as ACP. Use `whycode serve` + a browser for local share. |

## Out

- 1000 fps render loop — idle dirty-draw stays the policy.
- Self-dev binary reload — point the agent at this repo if you want that.
- jcode memory graph / ambient daemon.
- Plugin marketplace.

## Standing decisions

These are still in force. The full historical log is
[archive/status.md](archive/status.md).

- The goal is not “OpenCode parity” (nor jcode clone).
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
