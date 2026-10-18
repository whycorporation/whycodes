# Repair

How the `whycodes` agent changes configuration.

1. Get the facts: `whycodes config path`, `whycodes config get <dotted>
   --json` (typed value and winning layer), `whycodes config keys <prefix>`,
   `whycodes debug --json`.
2. Pick the layer. User-wide intent: the global file (default). "For this
   project only": `--project` (`.whycodes/config.toml`). When an env
   variable or the project file wins, a global change will not take effect:
   say so. Known quirk: any project overlay resets `default_agent` to
   `build`, so a global `default_agent` is shadowed whenever an overlay
   exists. Check `config get default_agent` (it names the winning layer)
   and use `--project` in that case.
3. Say which file you will write, then write it with
   `whycodes config set <dotted> <value> [--project]`. The value is a TOML
   literal (bool, number, array, inline table), else a string. It writes
   only that key. A global `set` is validated against defaults + global;
   `--project` against defaults + global + overlay. It refuses only
   validation errors the edit introduces (errors already in the file do not
   block unrelated keys), and refuses edits that "would have no effect"
   (unknown key, or a value the merge discards, such as a built-in agent's
   description or prompt). A refused file stays unchanged. Report a refusal
   as printed.
4. `<file>.bak` is written once: it holds the file as it was before the
   first edit, not before the latest one.
5. Verify with `whycodes config get <dotted>`.

Example keys: `default_agent`, `session.tool_profile`,
`session.intent_guidance`, `memory.auto_inject`, `general.approval_mode`,
`permission.bash`, `security.sandbox`, `mcp_servers.<name>.command`,
`lsp.servers.<name>.disabled`, `tui.theme`, `slop.delta_loc`, `notify.on`,
`agents.<name>.model`, `agents.<name>.temperature`.

Secrets (`api_key`, `token`, `secret`, `password`, `webhook*`,
`authorization`, header values): never read them back or print them. `set`
accepts them write-only; prefer an env variable or a `${VAR}` reference
(`get`, `keys` and `show` print `${VAR}` placeholders as names).
`notify.telegram_bot_token` and `notify.telegram_chat_id` must be set
together: set the whole `notify` inline table in one `set`, or use the
`WHYCODES_NOTIFY_TELEGRAM_*` env variables.

Raw edits only when `config set` cannot express the change (a new
`[[hooks]]` entry, a new `[[agents]]` table): copy the file to
`<file>.bak`, `edit` the TOML (the global file by the absolute path from
`config path`; the project overlay as `.whycodes/config.toml`), then run
`whycodes config show` to confirm it still loads before claiming success.

Never touch `auth.json`, `whycodes.db`, `mcp/auth.json`, the project's
source files, or keys the user did not ask about.
