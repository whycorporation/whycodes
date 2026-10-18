# Agents

Primary agents run the conversation:

| Agent | Can do |
|---|---|
| `build` (default) | Everything: write, edit, shell, network |
| `plan` | Read-only; proposes changes |
| `ask` | Read-only Q&A about the project |
| `whycodes` | WhyCodes itself: explains and repairs config; writes only config |

Subagents, spawned by the model through `task` / `swarm`: `general`
(multi-step, full tools except todos), `explore` (fast read-only search),
`scout` (external docs and dependencies).

Switching: `whycodes -a <name>`, `/agent [name]` (no name opens a picker),
`Ctrl+T` cycles primary agents when idle, `whycodes agent [name]` lists or
describes them. `default_agent` sets the startup agent and must name an
existing agent. Agents use the session model; `session.model_plan` is used
while `plan` is active, `session.model_smol` for workers.

Custom subagents: Markdown with frontmatter (`name`, `description`, `tools`,
`model`, `output`) in `~/.whycodes/agents/` or `<project>/.whycodes/agents/`;
the body is the system prompt. Extra `[[agents]]` entries in config add new
names (`mode = "primary" | "subagent" | "all"`, a `[agents.permission]`
table with `allow_file_writes`, `allow_network`, `allow_shell`, optional
`allowed_tools`, `denied_tools`, `rules`).

Finished workers write `.whycodes/agents/<id>.md`, readable as
`read agent://<id>`; `Alt+A` or `/agents` opens the agent hub.

Details: docs/guide.md "Agents", "Custom subagents"; docs/config.md
"[[agents]]".
