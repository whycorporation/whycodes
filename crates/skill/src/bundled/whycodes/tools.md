# Tools

`session.tool_profile`: `core` (default: `read`, `write`, `edit`, `grep`,
`glob`, `list`, `repomap`, `bash`, `bg`, `question`, `task`, `todowrite` /
`todo`, `todoread`, `tool_search`) or `full` (every built-in). Under `core`
the rest (`apply_patch`, `memory`, `swarm`, `webfetch`, `websearch`, `lsp`,
`browser`, `panel`, `schedule`, `checkpoint`, git and GitHub tools, ...) is
loaded on demand with `tool_search`.

"Why is X not in this session?": check the profile, the agent's allowed and
denied tools (`whycodes agent <name>`), and `[permission]` deny rules;
`/tools` lists the current agent's tools.

`read` also serves internal URLs: `skill://<name>[/<topic>]`, `agent://<id>`,
`agent://tools/<name>` (long tool guides), `proc://<job-id>`,
`rule://<name>`, `pr://<n>`, `issue://<n>`, `mcp://<server>/<uri>`.

After `edit` / `write`: the project's own formatter runs when the project
shows it uses one (`[tools.format] enabled`), then LSP diagnostics are
appended (`[lsp] edit_diagnostics_ms`, `0` = off).

`bash` auto-backgrounds after 20 s (`[tools.bash] auto_background`,
`auto_background_after_secs`); output goes to `.whycodes/scratch/<id>.log`.
MCP tools bind as `{server}_{tool}`. Shell plugins come from `plugins.toml`
(`whycodes plugins`).

Note: `tools.enable_*`, `tools.disabled_tools` and `tools.custom_tools` are
parsed but not read at runtime in this version; remove a tool with a
`[permission]` deny instead.

Details: docs/guide.md "Tools"; docs/tools/README.md (one page per tool);
docs/config.md "[tools]".
