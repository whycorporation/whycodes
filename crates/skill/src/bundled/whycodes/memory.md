# Memory

On by default, per project:
- `MEMORY.md`: human-editable notes loaded into every session
  (`memory.max_index_lines`, `max_index_bytes`). `whycodes memory path`
  prints where it is.
- Semantic facts in SQLite. Top hits are appended to the user message (not
  the system prompt) when `memory.auto_inject = true`; new facts are
  retained after each turn (`auto_retain`, `retain_llm`).
- Code index (`whycodes memory index`, `code-search`; `auto_index`) and past
  sessions (`session-search`, `session_inject`).

`memory.scope`: `user` (banks in the data dir) or `project`
(`.whycodes/memory`, shareable). Turn it all off with
`memory.enabled = false`, `--no-memory` (one run), or
`WHYCODES_NO_MEMORY=1`.

Manage: `whycodes memory list|search|add|delete|clear|path|export|import`,
`/remember`, `/memory`, and the `memory` tool.

Details: docs/guide.md "Memory"; docs/config.md "[memory]".
