# Config

Files:
- Global: `~/.whycodes/config.toml` (`%USERPROFILE%\.whycodes\config.toml`
  on Windows). `WHYCODES_HOME` moves the whole instance root (config,
  `auth.json`, `whycodes.db`, memory, `skills/`, `prompts/`).
- Project overlay: `<project>/.whycodes/config.toml`.
- `.whycodes/slop.toml` (slop thresholds), `.whycodes/rules/*.md`
  (stream rules and project rules).

Layers, lowest to highest: built-in defaults, global file, project file,
`WHYCODES_*` environment variables, `slop.toml`. Command-line flags
(`-P`, `-m`, `-a`, `--no-memory`, `--approval-mode`, ...) win for that run
only. Maps (`providers`, `mcp_servers`, `permission`, ...) merge per key;
most lists are replaced by a non-empty higher layer; `agents` append by new
name only (built-in agents cannot be redefined). Booleans that default to
`true` can only be turned off by a higher layer. A global file that fails to
parse is an error; a broken project file is skipped with a warning.

Inspect, never guess:
- `whycodes config path`: the global file.
- `whycodes config get <dotted> [--json]`: one value and the layer that set
  it (`overriddenBy`: default / global / project / env).
- `whycodes config keys [prefix] [--json]`: leaf keys, effective values,
  winning layer.
- `whycodes config show`: the TOML; secrets print as `<redacted>`.
- `whycodes debug --json`: paths, whether the config exists, data dir, which
  `WHYCODES_*` and key variables are set (names only, never values).
- `/doctor` in the TUI: the same facts for the running session. You cannot
  run it; ask the user to.

`schema_version` is bumped in place on load. Validation reports, among
others, a `default_agent` that names no agent.

Details: docs/config.md (every key with default and env override, "Layers");
docs/guide.md "Configuration".
