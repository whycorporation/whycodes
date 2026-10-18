# Skills

A skill is Markdown with frontmatter (`name`, `description`, optional
`tools_allowed`, `file_patterns`). The system prompt lists project skills by
**name and description only**; the body is loaded on demand with
`read skill://<name>` or the `skill` tool (`action=load`); `action=list`
lists every loadable skill.

Where they are read from:
- Project: `.skills/*.skill.md`, `skills/*.skill.md`,
  `.whycodes/skills/*.skill.md`, and `<dir>/<name>/SKILL.md` trees under
  those directories.
- User: `~/.whycodes/skills/` (under `WHYCODES_HOME` when set). Loadable,
  but not listed in the system prompt.
- Bundled: `whycodes` (this manual). Its body is a topic index; a topic is
  `read skill://whycodes/<topic>`. A project or user skill with the same
  name wins for `skill://whycodes`; topics always come from the binary.

Duplicate names: the first one found wins. A file without frontmatter is
skipped with a warning.

Related: project rules in `.whycodes/rules/*.md` are appended to the system
prompt (`read rule://<name>`); custom subagents live in `.whycodes/agents/`.

Details: docs/guide.md "Configuration" (skills paragraph), "Tools"
(internal URLs).
