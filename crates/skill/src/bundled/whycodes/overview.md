# Overview

One binary, `whycodes`. Bare `whycodes` (or `whycodes -d <dir>`) opens the
full-screen TUI; `--plain` is a line REPL; `whycodes generate "<prompt>"` and
`whycodes run "<prompt>"` are one-shot turns (`--format text|json|stream-json`).
`whycodes serve` runs a local daemon (`/api/*` for an attached TUI via
`whycodes connect`, `/v1/*` for SDK clients); `whycodes acp` speaks the
editor protocol on stdio.

Startup: parse flags, load layered config (topic `config`), resolve
provider/model/credentials, open `<data dir>/whycodes.db`, build the agent and
session. A turn: build the system prompt and visible tools, stream the model,
run tool calls through the risk / intent / path / permission / sandbox / hook
gates, append results, repeat until no tool calls remain. Read-only calls may
run in parallel; mutating and shell calls run one at a time.

- **Session**: one conversation, saved in SQLite; resume with `-c`,
  `-r <id>`, `/resume`; `--fork` / `/fork` copies it.
- **Named (primary) agent**: who runs the conversation (`build`, `plan`,
  `ask`, `whycodes`); chosen with `-a`, `/agent`, `Ctrl+T`.
- **Subagents**: workers the model spawns with `task` (one goal) or `swarm`
  (parallel, git worktrees by default). They report back to the parent.

Project instructions: `AGENTS.md` at the repo root (`/init` writes one),
plus `.whycodes/AGENTS.md`, appended to the system prompt.

Details: docs/guide.md "Command line", "Interactive session";
docs/architecture.md; docs/internals/turn-loop.md.
