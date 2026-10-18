# Session and TUI

Keys: `Ctrl+T` cycle primary agents, `Ctrl+P` providers, `Ctrl+M` models,
`Ctrl+N` new parallel session, `Ctrl+O` dashboard, `Ctrl+B` sidebar,
`Alt+A` agent hub, `Esc` cancel, `?` help. Rebind under
`[tui.key_bindings]`; `/keys` shows bindings and conflicts.

Useful slash commands: `/agent [name]`, `/models`, `/effort`, `/mode`,
`/doctor`, `/tools`, `/context`, `/cost`, `/info`, `/compact`, `/undo`,
`/redo`, `/fork`, `/branch`, `/tree`, `/sessions`, `/resume`, `/mcp`,
`/remember`, `/memory`, `/connect`, `/login`, `/themes`, `/init`,
`/queue`, `/now`, `/btw`. Prefixes: `!cmd` runs a shell command, `@path`
inlines a file.

`--plain`: line REPL, same slash commands. Headless:
`whycodes generate "<prompt>" --format json` (one object with `result`,
`usage`, `session_id`) or `stream-json` (NDJSON events ending in `result`);
`-t` caps turns; `-j` runs several prompts in parallel. Permission `ask` is
denied there (topic `permissions-hooks`).

Session settings live under `[session]`: `tool_profile`, `undo_depth`,
`auto_title`, `compaction_threshold`, `intent_guidance`,
`cache_warm_minutes`, `preconnect`, `stream_rules`, `magic_keywords`.
UI: `[tui] theme`, `show_sidebar`, `agent_colors`.

Details: docs/guide.md "Interactive session", "Slash commands", "Output
formats (headless / CI)"; docs/config.md "[session]", "[tui]".
