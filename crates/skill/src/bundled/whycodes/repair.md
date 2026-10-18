# Repair

How the `whycodes` agent changes configuration.

1. Get the facts: `whycodes config path`, `whycodes config get <dotted>
   --json` (value and winning layer), `whycodes config keys <prefix>`,
   `whycodes debug --json`.
2. Pick the layer. User-wide intent: the global file (default). "For this
   project only": `--project` (`.whycodes/config.toml`). When an env
   variable or the project file wins, a global change will not take effect:
   say so.
3. Say which file you will write, then write it with
   `whycodes config set <dotted> <value> [--project]`. The value is a TOML
   literal (bool, number, array, inline table), else a string. It writes
   only that key, copies the file to `<file>.bak` before its first write,
   type-checks and validates the result, and refuses an invalid config (the
   file stays unchanged). Report a refusal as printed.
4. Verify with `whycodes config get <dotted>`.

Example keys: `default_agent`, `session.tool_profile`,
`session.intent_guidance`, `memory.auto_inject`, `general.approval_mode`,
`permission.bash`, `security.sandbox`, `mcp_servers.<name>.command`,
`lsp.servers.<name>.disabled`, `tui.theme`, `slop.delta_loc`, `notify.on`.

Secrets (`api_key`, `token`, `secret`, `password`, `webhook*`,
`authorization`, header values): never read them back or print them. `set`
accepts them write-only; prefer an env variable or a `${VAR}` reference.

Raw edits only when `config set` cannot express the change (a new
`[[hooks]]` entry, a new `[[agents]]` table): copy the file to
`<file>.bak`, `edit` the TOML (the global file by the absolute path from
`config path`; the project overlay as `.whycodes/config.toml`), then run
`whycodes config show` to confirm it still loads before claiming success.

Never touch `auth.json`, `whycodes.db`, `mcp/auth.json`, the project's
source files, or keys the user did not ask about.
