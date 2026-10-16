# Roadmap

Current focus and standing decisions.

Last updated: **2026-10-16**.

## Now

Nothing open. Later stays later.

SDK protocol v1 (`whycodes serve` `/v1/*`, Rust `whycodes-sdk`,
`sdk/typescript`) shipped. Not on the public npm registry yet.

ACP (`whycodes acp`, editor ↔ agent over stdio) shipped.

## Later

| Item | Notes |
|---|---|
| `web` surface | Use `whycodes serve` + a browser for local share. |

## Out

- 1000 fps render loop — idle dirty-draw stays the policy.
- Self-dev binary reload — point the agent at this repo if you want that.
- Memory graph / ambient daemon.
- Plugin marketplace.

## Standing decisions

These are still in force.

- The goal is not feature parity with any other tool.
- Safety before features: shell risk classification and the OS sandbox stay
  on by default.
- Default `bash_risk_threshold` is `destructive`, not `caution`.
- Unrecognised commands are `safe`; unresolvable targets escalate to
  `destructive`, never `catastrophic`.
- Catastrophic commands cannot be approved.
- Sandbox defaults to `workspace`, network on, fallback allow (so
  non-Linux keeps working).
- No CLI telemetry or phone-home. Interactive sessions may check GitHub
  Releases for a newer binary (opt-out). Install volume is GitHub
  release `download_count` (`python scripts/release_downloads.py`) and,
  later, Homebrew-core analytics — not unique people and not daily use.
- Do not add a WhyCodes GitHub login for usage stats. Copilot OAuth is a
  model credential, not a WhyCodes account. Optional GitHub identity is
  only for a user-facing feature (never `repo` scope, never required).
