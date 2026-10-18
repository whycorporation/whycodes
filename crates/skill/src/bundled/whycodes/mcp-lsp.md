# MCP and LSP

`[mcp_servers.<name>]`: `command` + `args` + `env` + `cwd` for stdio, or
`url` (+ `type = "http" | "sse" | "auto"`, `headers`) for remote. Tools
bind as `<name>_<tool>` (stable names; `[permission]` can target
`"<name>_*"`). Resources add `mcp_list_resources` / `mcp_read_resource` and
`read mcp://<server>/<uri>`; prompts become `/mcp__<server>__<prompt>` in the
TUI. A higher layer replaces a whole server entry of the same name.

Commands: `whycodes mcp list|add|remove|login|logout`. `mcp login` stores an
OAuth token in `<data dir>/mcp/auth.json`; an `Authorization` header in
config wins over it. Header values are secrets: never print them.
`/mcp` shows connected servers.

`[lsp]`: `idle_timeout_ms`, `edit_diagnostics_ms` (default 1500, `0` off).
`[lsp.servers.<name>]`: `command`, `args`, `disabled = true`. Servers are
auto-detected from project markers (`Cargo.toml`, `package.json`,
`go.mod`, ...) and must be on PATH or in project-local bins.

Details: docs/guide.md "Tools" (MCP, `lsp`); docs/config.md
"[mcp_servers.<name>]", "[lsp]"; docs/internals/mcp-lifecycle.md.
