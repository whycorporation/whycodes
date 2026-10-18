# Permissions and hooks

`[permission]` maps a tool name or pattern to `allow` / `ask` / `deny`;
it is merged into every agent's rules (an agent's own rule for the same key
wins). Forms: exact (`bash`), prefix (`mymcp_*`), path rules
(`edit(src/**)`, `write(docs/**)`; longest glob wins) and shell rules
(`bash(git *)`; longest pattern wins; broad interpreter allows become ask).

Approval mode (`[general] approval_mode`, `--approval-mode`, `/mode`):
`auto` (default; `ask` runs without a prompt), `important` (prompts on
questions and high-risk calls), `manual` (prompts on every `ask`).
Structured output (`--format json|stream-json`) denies `ask` unless
`session.headless_ask = "allow"` or `--approve-tools`.

Shell risk: `[security] bash_risk_threshold` (`caution`, `destructive`,
`off`); catastrophic commands are always refused. OS sandbox for shell:
`security.sandbox = "workspace" | "off"`, `sandbox_network`,
`sandbox_fallback`. HTTP tools: `network_allowlist`, `network_denylist`.
`redact_secrets` masks credentials in provider requests.

`[[hooks]]`: `event` (`pre_tool`, `pre_tool_transform`, `post_tool`,
`session_start`, `user_prompt_submit`, `stop`, `pre_compact`,
`subagent_stop`), `match`, `command`, `block_on_failure`, `feedback`,
`timeout_secs`. A non-empty list in a higher layer replaces the whole list,
so a project hook hides the global ones. Hooks are a list of tables: dotted
`config set` cannot append one (topic `repair`).

Details: docs/guide.md "Permissions", "Shell command risk", "Shell OS
sandbox", "Hooks"; docs/internals/permission-gate.md; docs/config.md
"[permission]", "[security]", "[[hooks]]".
