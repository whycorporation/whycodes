# Configuration reference

> Status: Implemented · Updated: 2026-10-16

Every key WhyCodes reads from `config.toml`, in one place, with the default
the code uses and the environment variable that overrides it. The source of
truth is the serde `Config` tree in `crates/config/src/types.rs` (leaf types
in `crates/core/src/types.rs`); the loader and environment overlay are in
`crates/config/src/load.rs`, the layer merge in `merge.rs`, and validation in
`validate.rs`. The narrative guide is [guide.md](guide.md#configuration).

**Where the file lives.** The global file is `~/.whycodes/config.toml` on
Linux and macOS and `%USERPROFILE%\.whycodes\config.toml` on Windows (the
loader reads `$HOME`, then `%USERPROFILE%`). Sessions, `auth.json`,
`whycodes.db`, memory banks, `plugins.toml`, `commands/`, `prompts/` and
`skills/` sit in the same directory. `WHYCODES_HOME`, when set and
non-empty, replaces that directory as the instance root (config and data
both move). The project file is `<project>/.whycodes/config.toml`. First
launch writes the global file with `[providers.openrouter]` and
`default_model = openrouter / anthropic/claude-sonnet-4.5`; `save()` then
serializes the whole tree, so a saved file also contains the built-in
agents and every default value.

**Winning rule.** Later layers win. A scalar in a higher layer replaces the
lower one when it differs from the built-in default; maps (`providers`,
`models`, `mcp_servers`, `lsp.servers`, `permission`, `commands`,
`tui.agent_colors`) merge per key; lists replace when the higher layer's
list is non-empty (`hooks`, `security.network_*`, `session.stream_rules`,
`tools.disabled_tools`, `notify.on`); `agents` append by unique name.
Booleans that default to `true` (`memory.enabled`, `session.auto_title`,
`session.preconnect`, `general.auto_update`, `swarm.enabled`, …) can only be
turned off by a higher layer, and `tui.show_sidebar` can only be turned on.
Environment variables are applied after both files.

**Printing the resolved path.** `whycodes config path` prints the global
file; `whycodes debug` (or `whycodes debug --json`) prints the config path,
whether it exists, the data dir, and the `WHYCODES_*` variables it saw.
`whycodes config show` / `get <key>` / `set <key> <value>` read and write
the global file only (no project or environment layer).

## Layers

Lowest to highest precedence, from `Config::load_layered`:

1. Built-in defaults (`Config::default()`: six built-in agents, all
   `default_*` functions below).
2. Global `config.toml` (`~/.whycodes/config.toml`, or
   `$WHYCODES_HOME/config.toml`). Created on first run if missing. A file
   that fails to parse is an error, not a silent fallback.
3. Project `.whycodes/config.toml` next to the project directory. A file
   that fails to parse or read is logged as a warning and skipped.
4. `WHYCODES_*` environment variables (`apply_env_overrides`; the exact
   names appear in the tables). `${VAR}` / `$VAR` inside
   `notify.discord_webhook`, `notify.telegram_bot_token` and
   `notify.telegram_chat_id` are expanded from the environment after this
   step; unknown variables stay literal, and a value that expands to blank
   becomes unset.
5. Project `.whycodes/slop.toml` (`[slop]` keys only, written at top level
   of that file). Non-default values overlay `[slop]` from the layers above.

Command-line flags sit above all five layers for the keys they touch:
`-P/--provider` and `-m/--model` replace `default_model.provider_id` and
`default_model.model_id` for the run, `-a/--agent` replaces
`default_agent`, `-d/--dir` sets the project directory (and
`general.project_path` for the session), `--no-memory` forces
`memory.enabled = false`, `--no-auto-update` forces
`general.auto_update = false`, and `--plain` (or `WHYCODES_PLAIN` set to
anything) skips the full-screen TUI. `--approve-tools` forces
`session.headless_ask = "allow"` for a structured run.

## Top-level keys

| Key | Default | Env override | Meaning |
|---|---|---|---|
| `default_agent` | `"build"` | — | Name of the agent used when no `-a/--agent` flag is given. Must match an entry in `[[agents]]`; validation reports a name that resolves to nothing. |
| `default_model` | — | `WHYCODES_PROVIDER`, `WHYCODES_MODEL` | Table with the same fields as `[models.<key>]`. Unset means the first-run seed (`openrouter` / `anthropic/claude-sonnet-4.5`) or, with `-P`/`-m`, the flags. `WHYCODES_MODEL` alone replaces `model_id`; `WHYCODES_PROVIDER` creates the provider entry if missing (reading `<PROVIDER>_API_KEY`, then `OPENAI_API_KEY`, then `ANTHROPIC_API_KEY`) and, together with `WHYCODES_MODEL`, fills `default_model` when it was unset. |
| `schema_version` | `1` | — | On-disk schema version. A missing or lower value is rewritten to the current version on load (best-effort save). |

## `[providers.<name>]`

A map keyed by provider id. When a table omits `name`, the key is used.

| Key | Default | Env override | Meaning |
|---|---|---|---|
| `name` | table key | — | Provider id (`openai`, `anthropic`, `openrouter`, `xai`, `ollama`, …). Picks the built-in base URL when `base_url` / `api_base` are unset. |
| `api_key` | — | — | API key. Unset means the key comes from `<NAME>_API_KEY` (then `WHYCODES_<NAME>_API_KEY`, `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`) or an OAuth login; validation warns when none of these exist. |
| `api_base` | — | — | Base URL for the API. Unset means the provider's well-known endpoint. `whycodes provider discover --save` and the TUI provider dialog write it for a discovered local server (`lmstudio`, `vllm`, `llamacpp`), with no `api_key`; an existing block is never overwritten (only a missing `api_base` is filled; a different one is skipped). |
| `base_url` | — | — | Base URL that overrides `api_base` when both are set. A `localhost` / `127.0.0.1` value only logs a warning. |
| `headers` | — | — | Map of extra HTTP headers sent with every request to this provider. |
| `models` | `[]` | — | Known model ids for this provider; optional, used by listings and completion. |
| `tool_arguments` | — | — | Wire shape of `function.arguments` on assistant tool calls: `json_string` (OpenAI style, the behaviour when unset) or `object` (bare JSON object, for gateways that require it). |
| `extra_body` (alias `extra`) | `{}` | — | Extra top-level fields merged into every request body (for example `service_tier = "flex"`). `model`, `messages`, `stream`, `tools` and `system` are never overridden. |

## `[providers.<name>.credentials]`

| Key | Default | Env override | Meaning |
|---|---|---|---|
| `order` | `[]` | — | Credential names tried in order (`interactive`, `ci`, …). Names from `env` that are not listed follow; with neither, the single name `default` maps to `<NAME>_API_KEY`. |
| `reserve` | `0.10` | — | Share of a credential the other lane should not take first; clamped to 0–1 at use. With a positive value, interactive sessions try `interactive`/`live`/`local`/`dev` names before `ci`/`batch`/`github`/`*-ci`, and a CI process (`CI`, `GITHUB_ACTIONS`, or `WHYCODES_CREDENTIAL_LANE=ci`) does the reverse. |
| `env` | `{}` | — | Credential name → environment variable that holds the secret (never the secret itself). A missing or empty entry falls back to `<NAME>_<CREDENTIAL>_API_KEY`. |

## `[models.<key>]` and `[default_model]`

`models` is a map of named model configurations; `default_model` is one
such table at the top level. `model_id` and `provider_id` are required.

| Key | Default | Env override | Meaning |
|---|---|---|---|
| `model_id` | required | `WHYCODES_MODEL` (default_model only) | Model id as the provider names it. |
| `provider_id` | required | `WHYCODES_PROVIDER` (default_model only) | Provider id the model belongs to. Validation reports an empty id on `default_model` when providers are configured. |
| `max_tokens` | — | — | Maximum completion tokens per response (sent as `max_tokens`). Unset leaves the provider default. |
| `context_window` | — | — | Full prompt + completion budget in tokens, used for the context meter and compaction. Unset falls back to the built-in catalog, then `session.max_context_tokens`. |
| `temperature` | — | — | Sampling temperature. Unset sends none. |
| `top_p` | — | — | Nucleus sampling. Unset sends none. |
| `thinking` | — | — | Whether the model is used with extended thinking. Unset means auto by model family. |
| `supports_tools` | — | — | Overrides tool-call support detection. Unset means detect. |
| `supports_images` | — | — | Overrides image-input support detection. Unset means detect. |

## `[session]`

| Key | Default | Env override | Meaning |
|---|---|---|---|
| `max_context_tokens` | `200000` | `WHYCODES_MAX_TURNS` | Context budget in tokens when the model has no `context_window`. `0` is reported by validation (it disables context). The environment variable name is historical; it writes this field when it parses as an integer. |
| `compaction_threshold` | `150000` | — | Token count at which the transcript is compacted automatically. |
| `store_path` | — | — | Intended session-store location. No runtime code reads it in this version; the CLI always opens `<data dir>/whycodes.db`. |
| `auto_title` | `true` | — | Auto-name sessions: project placeholder → first-message heuristic → small-model refine after the first turn. |
| `title_model` | — | — | Model used to refine titles, as `provider/model` or a bare id on the session provider. Unset picks a known small sibling (haiku / mini / flash). |
| `tool_profile` | `"core"` | — | Tools advertised to the model: `core` (about 12 tools, faster first token) or `full` (every built-in including github, web, lsp, browser, panel, checkpoint). |
| `prompt_cache` | `"auto"` | — | Prompt-cache policy for providers that support cached prefixes: `auto` or `none`. |
| `cache_warm_minutes` | `0` | — | Keep the prompt cache warm for this many idle minutes after a turn (TUI and daemon). Each keep-alive is a paid request; `0` turns it off. |
| `model_fast` | — | — | Fast model for trivial chat and `/goal` checks (`provider/model` or bare id). Unset auto-picks the small sibling of the session model. |
| `model_smol` | — | — | Cheap model for `task` / `swarm` workers. Unset auto-picks the small sibling. |
| `model_plan` | — | — | Model used while the `plan` agent is active. Unset keeps the session model. |
| `model_race` | `"off"` | — | First-token race partner: `off`, `auto` (small sibling) or `provider/model`. When set, a slow primary first token starts the partner. |
| `race_after_ms` | `800` | — | How long to wait for the primary first token before opening `model_race`. Validation reports values above 30000. |
| `fallback_models` | `[]` | — | `provider/model` refs tried in order when the active model cannot open a stream (429 after named credentials, 5xx, network, timeout). Entries without a usable key are skipped. Not carried by the layer merge in this version (see last section). |
| `response_cache` | `"auto"` | — | Process-local text-only response cache: `auto` or `off` (`on`/`true`/`1`/`false`/`0`/`none` are accepted). Any other value is reported by validation. |
| `preconnect` | `true` | — | Open the provider connection while the prompt is typed so the first request skips DNS/TCP/TLS. Sends no credentials or prompt. |
| `intent_guidance` | `"auto"` | — | Heuristic intent posture for build turns: `auto`, `off` or `always`. When active, high-confidence question/plan signals add an ephemeral guidance block to the request. |
| `compaction_llm` | `"auto"` | — | Full-replace compaction (manual `/compact` and auto): `auto` asks the session model for a structured summary; `off` uses the local stub only. |
| `reasoning_effort` | — | — | `low`, `medium`, `high` or `xhigh` for OpenAI-compatible and xAI thinking models. Unset means the family default (`medium` when supported); `xhigh` clamps to `high` on models that lack it. |
| `headless_ask` | — | `WHYCODES_HEADLESS_ASK` | How `--format json` / `stream-json` treat permission `ask`: `deny` (unset behaviour; tool result stamped `denied:headless`) or `allow` (auto-approve, same as `--approve-tools`). The TUI and `--plain` ignore it. The environment variable also accepts `fail`, `fail-closed`, `auto`, `approve`. |

## `[session.magic_keywords]`

| Key | Default | Env override | Meaning |
|---|---|---|---|
| `enabled` | `true` | — | Master switch for standalone prose keywords that add a hidden per-turn instruction. |
| `ultrathink` | `true` | — | `ultrathink` in a prompt asks for careful multi-step reasoning and the highest thinking effort the model supports. |
| `orchestrate` | `true` | — | `orchestrate` in a prompt fans work out through `task` / `swarm` and verifies each phase. |

## `[[session.stream_rules]]`

Regexes matched against streamed assistant text. A hit aborts the current
request and injects `hint` before the next step. A non-empty list in a
higher layer replaces the whole list.

| Key | Default | Env override | Meaning |
|---|---|---|---|
| `name` | `""` | — | Label shown when the rule fires. |
| `pattern` | `""` | — | Regular expression matched against the draft. |
| `hint` | `""` | — | Text injected after the abort. |

## `[memory]`

| Key | Default | Env override | Meaning |
|---|---|---|---|
| `enabled` | `true` | `WHYCODES_MEMORY` (`0/1`), `WHYCODES_NO_MEMORY=1` | Master switch for cross-session memory (facts, MEMORY.md, code and session recall). `--no-memory` forces it off for one run. |
| `auto_inject` | `true` | — | Inject the top semantic hits for the current user message into the system prompt. |
| `auto_retain` | `true` | — | Retain durable facts after each turn (heuristic and/or model). |
| `retain_llm` | `true` | — | Also call a small model to extract facts when the heuristic finds none. |
| `retain_llm_always` | `false` | — | Run the model retain pass even when the heuristic already found facts. |
| `retain_every_n` | `1` | — | Run retain every N user turns (`1` = every turn). |
| `retain_max_facts` | `3` | — | Maximum facts retained per turn. |
| `max_index_lines` | `200` | — | Maximum lines of `MEMORY.md` loaded into every session. |
| `max_index_bytes` | `25600` | — | Maximum bytes of `MEMORY.md` loaded into every session. |
| `recall_top_k` | `5` | — | Maximum recalled facts injected per turn. |
| `recall_min_score` | `0.28` | — | Minimum cosine similarity for a fact to be recalled. |
| `recall_token_budget` | `800` | — | Token budget for recalled facts (characters ≈ tokens × 4). |
| `embed_dim` | `256` | — | Hashing embedder dimension (stored vector width). |
| `scope` | `"user"` | — | `user` keeps banks in the data dir; `project` keeps them in `.whycodes/memory` (shareable in git). |
| `embed_backend` | `"hash"` | — | `hash` (local hashing embedder) or `onnx` (MiniLM; needs a build with `--features onnx`). |
| `code_inject` | `true` | — | Inject code-index hits when an index exists. |
| `code_top_k` | `4` | — | Maximum code chunks injected per turn. |
| `code_min_score` | `0.22` | — | Minimum similarity for a code chunk. |
| `subagent_banks` | `true` | — | Give subagents their own memory bank (`project::agent_name`). |
| `auto_index` | `true` | — | On session start, build a code index when none exists. |
| `auto_index_max_files` | `1500` | — | Maximum source files walked when auto-indexing. |
| `auto_index_max_chunks` | `4000` | — | Maximum chunks produced when auto-indexing. |
| `session_inject` | `true` | — | Inject related turns from past sessions (`# Past sessions`). |
| `session_top_k` | `3` | — | Maximum past-session excerpts per turn. |
| `session_min_score` | `0.22` | — | Minimum similarity for a past-session excerpt. |
| `consolidate` | `true` | — | Cap the fact bank after retain by dropping the least-recalled entries. |
| `consolidate_max` | `80` | — | Fact-bank size kept by consolidation. |

## `[permission]`

A map from tool name or pattern to an action. Merged into every agent's
`permission.rules`; an agent's own rule for the same key wins. Exact names
win over patterns; patterns are `*` or `prefix*` (for example `mymcp_*`).

| Key | Default | Env override | Meaning |
|---|---|---|---|
| `<tool or pattern>` | — | — | `allow` runs without a prompt, `ask` prompts (TUI dialog, stdin in `--plain`, denied in structured output unless `session.headless_ask = "allow"`), `deny` blocks. Only those three spellings are valid in TOML. Unset tools follow the agent's own lists and category flags. |

## `[security]`

| Key | Default | Env override | Meaning |
|---|---|---|---|
| `bash_risk_threshold` | `"destructive"` | — | Lowest shell risk level that prompts: `caution`, `destructive` or `off`. `catastrophic` commands are always refused and cannot be approved. |
| `sandbox` | `"workspace"` | `WHYCODES_SANDBOX` | OS sandbox for `bash` / `shell`: `workspace` (project read-write, host read-only, private `/tmp`; bubblewrap on Linux) or `off`. |
| `sandbox_network` | `true` | `WHYCODES_SANDBOX_NETWORK` (`1/true/yes/on` = on, anything else = off) | Whether the sandboxed shell may use the network. Does not affect `webfetch` / `websearch`. |
| `sandbox_fallback` | `"allow"` | `WHYCODES_SANDBOX_FALLBACK` | When the requested sandbox cannot be applied (no `bwrap`, non-Linux): `allow` warns and runs on the host, `deny` fails the tool call. |
| `network_allowlist` | `[]` | `WHYCODES_NETWORK_ALLOWLIST` (comma/space separated) | Host patterns allowed for HTTP tools (`webfetch`, `websearch`, GitHub API). Empty means unrestricted; `example.com` matches the apex and subdomains, `*.example.com` subdomains only, `*` any host. |
| `redact_secrets` | `true` | `WHYCODES_REDACT_SECRETS` | Replace credential-looking values with placeholders in every provider request and restore them inside tool arguments before execution ([guide](guide.md#secret-redaction)). An explicit `false` in any layer wins. |
| `network_denylist` | `[]` | `WHYCODES_NETWORK_DENYLIST` (comma/space separated) | Host patterns always blocked for HTTP tools; wins over the allowlist. |

## `[tools]`, `[tools.question]`, `[tools.bash]`, `[tools.format]`, `[tools.custom_tools.<name>]`

| Key | Default | Env override | Meaning |
|---|---|---|---|
| `tools.enable_read` | `true` | — | Intended switch for the `read` tool. Defined and merged, but no runtime code reads it in this version; use `[permission]` or an agent's `denied_tools` to remove a tool. |
| `tools.enable_write` | `true` | — | Same as `enable_read`, for `write`. |
| `tools.enable_edit` | `true` | — | Same as `enable_read`, for `edit`. |
| `tools.enable_glob` | `true` | — | Same as `enable_read`, for `glob`. |
| `tools.enable_grep` | `true` | — | Same as `enable_read`, for `grep`. |
| `tools.enable_shell` | `true` | — | Same as `enable_read`, for `bash` / `shell`. |
| `tools.enable_webfetch` | `true` | — | Same as `enable_read`, for `webfetch`. |
| `tools.enable_websearch` | `true` | — | Same as `enable_read`, for `websearch`. |
| `tools.disabled_tools` | `[]` | — | Tool names to drop. Defined and merged (a non-empty higher layer replaces the list), but no runtime code reads it in this version. |
| `tools.question.timeout_enabled` | `true` | — | Whether an unanswered `question` questionnaire fails after `timeout_secs`. |
| `tools.question.timeout_secs` | `1800` | — | Seconds to wait for questionnaire answers when the timeout is enabled. |
| `tools.bash.auto_background` | `true` | — | Detach a still-running, non-catastrophic `bash` command after `auto_background_after_secs` and return a `job_id` plus the last lines; output continues to `.whycodes/scratch/<job-id>.log`. |
| `tools.bash.auto_background_after_secs` | `20` | — | Seconds before a foreground `bash` is detached. |
| `tools.format.enabled` | `true` | — | Run the project's own formatter (rustfmt, prettier, biome, deno, ruff, black, gofmt) on a file after `edit` / `write` when the project shows it uses one. |
| `tools.custom_tools.<name>.command` | required | — | Shell command for a custom tool named `<name>`. Lower-layer entries are kept; a higher layer adds only names that do not exist yet. No runtime code registers these in this version; `[[plugins]]` in `plugins.toml` is the working way to add a shell tool. |
| `tools.custom_tools.<name>.description` | required | — | Description for the custom tool (see `command`). |
| `tools.custom_tools.<name>.parameters` | — | — | JSON Schema for the tool's arguments (see `command`). |

## `[lsp]`, `[lsp.servers.<name>]`

An empty `servers` map keeps built-in auto-detect. A named entry overlays
the matching built-in server or registers a new one; nested tables replace
rather than merge. camelCase aliases (`idleTimeoutMs`, `fileTypes`,
`languageId`, `rootMarkers`, `initOptions`, `editDiagnosticsMs`) are
accepted.

| Key | Default | Env override | Meaning |
|---|---|---|---|
| `lsp.idle_timeout_ms` | — | — | Milliseconds an idle language server stays alive before it is stopped. Unset uses the language-server manager's runtime default. |
| `lsp.edit_diagnostics_ms` | — | — | Milliseconds to wait for diagnostics after `edit` / `write`; `0` turns the wait off. Unset uses the runtime default of 1500. Not carried by the layer merge in this version (see last section). |
| `lsp.servers.<name>.command` | — | — | Executable to spawn. Unset keeps the built-in command for a known server. |
| `lsp.servers.<name>.args` | `[]` | — | Arguments for the executable. |
| `lsp.servers.<name>.file_types` | `[]` | — | File extensions the server handles. |
| `lsp.servers.<name>.language_id` | — | — | LSP `languageId` sent for documents. |
| `lsp.servers.<name>.root_markers` | `[]` | — | Files or directories that identify the workspace root. |
| `lsp.servers.<name>.init_options` | — | — | `initializationOptions` table passed at startup. |
| `lsp.servers.<name>.settings` | — | — | `workspace/configuration` settings table. |
| `lsp.servers.<name>.disabled` | — | — | `true` disables the server (built-in or custom). Unset means enabled. |
| `lsp.servers.<name>.is_linter` | — | — | Mark the server as a linter rather than a full language server. Unset keeps the built-in classification. |

## `[mcp_servers.<name>]`

Tools from a server bind as `<name>_<tool>`. A higher layer replaces the
whole entry with the same name. `whycodes mcp add|remove|login|logout`
edit the global file.

| Key | Default | Env override | Meaning |
|---|---|---|---|
| `type` | — | — | Transport: `stdio`, `http` (Streamable HTTP), `sse` (legacy HTTP+SSE) or `auto` (try Streamable HTTP, fall back to SSE). Unset infers `auto` when `url` is set, else `stdio` when `command` is set; neither is an error. |
| `command` | — | — | Local executable to spawn for `stdio`. Required when `type = "stdio"`. |
| `args` | `[]` | — | Arguments for `command`. |
| `env` | — | — | Environment variables for the spawned process. |
| `cwd` | — | — | Working directory for the spawned process. Unset leaves the child in the WhyCodes process directory. |
| `url` | — | — | Remote endpoint for `http` / `sse` / `auto`. Required for those transports. |
| `headers` | — | — | Extra HTTP headers for a remote server. An `Authorization` header here wins over a token stored by `whycodes mcp login`. |

## `[[hooks]]`

Shell hooks around tool calls and session lifecycle. A non-empty list in a
higher layer replaces the whole list. The command receives
`WHYCODES_HOOK_EVENT`, `WHYCODES_TOOL_NAME`, `WHYCODES_TOOL_ID`,
`WHYCODES_TOOL_INPUT`, `WHYCODES_SESSION_ID`, `WHYCODES_WORKING_DIR`; post-tool
adds `WHYCODES_TOOL_IS_ERROR` and `WHYCODES_TOOL_OUTPUT`; lifecycle events
add `WHYCODES_PROMPT`, `WHYCODES_RESPONSE`, `WHYCODES_SUBAGENT_*`.

| Key | Default | Env override | Meaning |
|---|---|---|---|
| `event` | `"pre_tool"` | — | `pre_tool`, `pre_tool_transform` (stdout JSON object replaces the tool input, before every gate), `post_tool`, `session_start`, `user_prompt_submit`, `stop`, `pre_compact` or `subagent_stop`. |
| `match` | `"*"` | — | Tool name pattern: exact, `*`, `prefix*` or `*suffix`. For `subagent_stop` it filters the agent type; ignored for the other lifecycle events. |
| `command` | required | — | Shell command (`sh -c` on Unix, `cmd /C` on Windows). |
| `block_on_failure` | `false` | — | On `pre_tool` / `user_prompt_submit`, a non-zero exit refuses the tool call or the turn. Other events only log failures. |
| `feedback` | `false` | — | Show a successful hook's stdout to the model: appended to the tool result (`post_tool`) or to the user turn (`session_start`, `user_prompt_submit`). Ignored elsewhere. |
| `timeout_secs` | `30` | — | Kill the hook after this many seconds. |

## `[notify]`

Off until `on` is non-empty **and** a channel is configured (Discord, a
complete Telegram pair, or `desktop`). `${VAR}` / `$VAR` in the three
secret fields are expanded from the environment after load.

| Key | Default | Env override | Meaning |
|---|---|---|---|
| `on` | `[]` | `WHYCODES_NOTIFY_ON` (comma separated) | Events that fire: `turn_done` (parent turn finished) and/or `need_input` (permission dialog, `question`, SDK ask). `done`, `input` and dashed spellings are accepted; anything else is reported by validation. |
| `discord_webhook` | — | `WHYCODES_NOTIFY_DISCORD_WEBHOOK` | Discord Incoming Webhook URL (`https://discord.com/api/webhooks/…` or `discordapp.com`). Unset or blank skips Discord; any other host is reported by validation. |
| `telegram_bot_token` | — | `WHYCODES_NOTIFY_TELEGRAM_BOT_TOKEN` | Telegram bot token. Unset skips Telegram; validation requires the chat id with it. |
| `telegram_chat_id` | — | `WHYCODES_NOTIFY_TELEGRAM_CHAT_ID` | Telegram chat, group or channel id. Unset skips Telegram; validation requires the token with it. |
| `timeout_secs` | `8` | `WHYCODES_NOTIFY_TIMEOUT_SECS` (clamped 1–60) | HTTP timeout per channel. Validation reports `0` or values above 60 from the files. |
| `desktop` | — | `WHYCODES_NOTIFY_DESKTOP` (`1/0`) | `true` raises a terminal desktop notification (OSC 9 / OSC 777) from the TUI for the `on` events while the window is not focused. Unset means off. |

## `[tui]`, `[tui.key_bindings]`, `[tui.agent_colors]`

| Key | Default | Env override | Meaning |
|---|---|---|---|
| `tui.theme` | — | — | Theme name (built-in or a JSON file next to `config.toml`). Unset uses the default dark theme; `/theme` changes it at runtime. |
| `tui.key_bindings` | — | — | Map of `"key" = "action"` rebinds; `"none"` unbinds a default. Key and action names are listed in [guide.md](guide.md#interactive-session). A higher layer replaces the whole map. |
| `tui.prompt_suggestions` | `"off"` | — | Idle follow-up suggestion in the prompt: `off` or `idle` (Tab accepts when the input is empty). Not carried by the layer merge in this version (see last section). |
| `tui.show_sidebar` | `false` | — | Open the sidebar when the TUI starts. A higher layer can only turn it on. |
| `tui.agent_colors.<agent or "model">` | `{}` | — | Prompt-chrome color per agent name, plus `model`: `#rgb` / `#rrggbb`, a theme role (`accent`, `success`, `info`, `warning`, `error`, `primary`, `secondary`, `thinking`, `dim`) or an ANSI name. Keys merge across layers. |
| `tui.skip_openrouter_key_prompt` | `false` | — | Set by the first-launch key modal when you skip it; stops the TUI from asking again. A higher layer can only turn it on. |

## `[[agents]]`

The six built-in agents (`build`, `plan`, `ask`, `explore`, `general`,
`scout`) always exist. A file entry is appended only when its `name` is
new; an existing name is not overridden. `[agents.permission]` is a
required sub-table (no serde default on the field), and its three
`allow_*` booleans are required inside it.

| Key | Default | Env override | Meaning |
|---|---|---|---|
| `name` | required | — | Agent name used by `-a`, `/agent`, `default_agent` and `task`. |
| `description` | required | — | One-line description shown in pickers and to the parent agent for subagents. |
| `mode` | required | — | `primary` (selectable in the TUI), `subagent` (only via `task` / `swarm`) or `all`. |
| `permission.allowed_tools` | — | — | Explicit allow list; when set, only these tools are candidates. Unset means every registered tool. |
| `permission.denied_tools` | — | — | Explicit deny list. |
| `permission.allow_file_writes` | required | — | Category flag for write-class tools. |
| `permission.allow_network` | required | — | Category flag for network tools. |
| `permission.allow_shell` | required | — | Category flag for shell tools. |
| `permission.allowed_paths` | — | — | Paths the agent may touch. In this version it only contributes to the tool-registry cache key; it is not enforced as a path gate. |
| `permission.rules` | `{}` | — | Per-agent `tool or pattern = allow|ask|deny`; wins over the global `[permission]` map for the same key. |
| `model` | — | — | Table with the `[models.<key>]` fields. In this version only its `thinking` flag is consulted (to enable or disable extended thinking on the request); the route stays the session provider/model. Unset means detect by capability. |
| `system_prompt` | — | — | Inline system prompt. Unset loads the built-in prompt for the six default agents, or none. |
| `temperature` | — | — | Sampling temperature sent with this agent's requests. Unset sends none. |
| `top_p` | — | — | Nucleus sampling. Defined, but no runtime code reads it in this version. |

## `[general]`

| Key | Default | Env override | Meaning |
|---|---|---|---|
| `project_path` | — | `WHYCODES_PROJECT_DIR` | Project directory recorded in the config. The CLI sets it to the resolved `-d/--dir` (or the current directory) for every session, so the file value is informational. |
| `log_level` | — | `WHYCODES_LOG_LEVEL` | Log filter (`debug`, `info,whycodes_agent=debug`, …). `RUST_LOG` wins over both; unset means `info`. |
| `default_gcp_project` | — | — | Default Google Cloud project id. Defined and merged, but no runtime code reads it in this version. |
| `auto_update` | `true` | `WHYCODES_AUTO_UPDATE` (`0/1`), `WHYCODES_NO_AUTO_UPDATE=1` | Check for a newer release when an interactive TUI session starts (home-screen confirm, never a silent replace). `--no-auto-update`, `CI` and benchmark runs also skip it. |
| `approval_mode` | — | `WHYCODES_APPROVAL_MODE` | When to interrupt: `auto` (auto-allow permission `ask`, auto-pick routine questions while todos or jobs are open), `important` (prompt on questions and high-risk tools), `manual` (prompt on every question and ask). Unset means `auto`. Never overrides `deny`, catastrophic shell risk, or the sandbox. |

## `[swarm]`

| Key | Default | Env override | Meaning |
|---|---|---|---|
| `enabled` | `true` | `WHYCODES_SWARM` (`0/1`) | Advertise and run the `swarm` tool. A higher layer can only turn it off. |
| `max_agents` | `4` | `WHYCODES_SWARM_MAX_AGENTS` (clamped 1–8) | Maximum concurrent workers; the agent hard-caps at 8. |
| `worktrees` | `true` | `WHYCODES_SWARM_WORKTREES` (`0/1`) | Isolate writers in detached git worktrees under `.whycodes/swarm/` and merge back with three-way conflict detection. Falls back to same-checkout plus file claims when the project is not a git repo. |
| `isolation` | — | — | `worktree` or `checkout`; when set, overrides `worktrees`. Unset defers to `worktrees`. |

## `[automation]`

| Key | Default | Env override | Meaning |
|---|---|---|---|
| `max_background_jobs` | `8` | — | Maximum concurrent background shell jobs (auto-detached `bash`, `/bg`). |

## `[slop]`

Tripwires for `whycodes slop` / `/slop`. The default verdict at the first
tier is `review`, not `block`. `.whycodes/slop.toml` holds the same keys at
its top level and overlays this table last.

| Key | Default | Env override | Meaning |
|---|---|---|---|
| `verbosity` | `0.25` | — | Review when the verbosity score is at least this. Validation requires 0–1. |
| `erosion` | `0.50` | — | Review when the erosion score is at least this. Validation requires 0–1. |
| `delta_loc` | `800` | — | Review when the absolute line delta of the diff is at least this. |
| `block_verbosity` | `0.40` | — | Block when verbosity is at least this. |
| `block_erosion` | `0.75` | — | Block when erosion is at least this. |
| `block_delta_loc` | `5000` | — | Block when the line delta is at least this. |
| `hotspots` | `8` | — | Number of hotspot functions listed. Validation requires at least 1. |

## `[pricing."provider/model"]`

USD per million tokens. Config prices win over prices learned from the
provider's model catalog. Not carried by the layer merge in this version
(see last section).

| Key | Default | Env override | Meaning |
|---|---|---|---|
| `input` | required | — | Price per million input tokens. |
| `output` | required | — | Price per million output tokens. |
| `cache_read` | — | — | Price per million cache-read tokens. Unset uses the input rate. |
| `cache_write` | — | — | Price per million cache-write tokens. Unset uses the input rate. |

## `[commands.<name>]`

Custom slash commands. Markdown files in `commands/` next to the global
file, `.whycodes/commands/` and `.opencode/commands/` (frontmatter keys
`description`, `agent`, `model`, `subtask`) are loaded into the same map
and win over TOML entries of the same name; the built-in `review`,
`security-review` and `commit` prompts are added only for names that are
still free.

| Key | Default | Env override | Meaning |
|---|---|---|---|
| `template` | required | — | Prompt body. `$ARGUMENTS`, `$1`, `$2`, … are expanded; `` !`cmd` `` runs a shell command and inlines its output. The rendered text is sent as the user turn. |
| `description` | — | — | Human-readable description. Stored; the slash runner does not display it in this version. |
| `agent` | — | — | Intended agent for the command. Stored; the slash runner in this version renders `template` on the current agent and model. |
| `model` | — | — | Intended model for the command. Stored; see `agent`. |
| `subtask` | — | — | Intended "run as subagent" flag. Stored; see `agent`. |

## `[command_configs.<name>]`

Per-command overrides keyed by command name. Defined and merged per
command, but `Config::get_command_config` has no caller in this version,
so these keys have no effect yet.

| Key | Default | Env override | Meaning |
|---|---|---|---|
| `model` | — | — | Table with the `[models.<key>]` fields intended for this command. |
| `agent` | — | — | Agent name intended for this command. |
| `max_turns` | — | — | Maximum agentic turns intended for this command. |

## `[[plugins]]` (plugins.toml)

Read from `plugins.toml` next to the global `config.toml` and from
`.whycodes/plugins.toml`; a project entry replaces a global entry with the
same `name`. Each entry registers a tool named `plugin_<name>`. Arguments
reach the command as `PLUGIN_ARG_<KEY>` environment variables and
`PLUGIN_WORKSPACE` is the session directory. Directory plugins
(`plugins/<id>/plugin.json`) use a different manifest schema; see
[guide.md](guide.md#shell-plugins).

| Key | Default | Env override | Meaning |
|---|---|---|---|
| `name` | required | — | Unique plugin name; the tool is `plugin_<name>`. |
| `command` | required | — | Shell command to run. |
| `description` | required | — | Description shown to the model. |
| `parameters` | — | — | JSON Schema for the arguments. Unset means a free-form input. |
| `working_dir` | — | — | Directory the child process starts in. Unset inherits the session directory (directory plugins set it to the plugin directory). |

## Documented but not found in config types

No key in the guide is missing from the types. Three related gaps are
worth knowing:

- The guide's `[[agents]]` example (`name`, `description`, `mode` only)
  omits `[agents.permission]`, which the `AgentInfo` type requires
  (`allow_file_writes`, `allow_network`, `allow_shell` have no serde
  default). As written, that entry does not deserialize.
- Keys that parse and merge but have no runtime consumer in this version
  (documented above with that note): `tools.enable_*` (all eight),
  `tools.disabled_tools`, `tools.custom_tools`, `session.store_path`,
  `general.default_gcp_project`, `command_configs.*`,
  `commands.<name>.description` / `agent` / `model` / `subtask`,
  `agents[].top_p`, and `agents[].permission.allowed_paths` (cache key
  only).
- Environment-only switches mentioned in the guide that are not config
  keys: `WHYCODES_HOME` (instance root), `WHYCODES_PLAIN` (skip the TUI),
  `WHYCODES_AUTO_APPROVE` / `WHYCODES_AUTO_DENY` (resolve permission
  `ask` and `question` prompts), `WHYCODES_CREDENTIAL_LANE`
  (`ci` / `interactive`), `WHYCODES_SHELL` (Windows host interpreter),
  `WHYCODES_BROWSER` / `WHYCODES_BROWSER_PORT` (Chromium for the `browser`
  tool), `WHYCODES_SKIP_IMPORT` (skip the first-run import offer),
  `RUST_LOG` (log filter that wins over `general.log_level`).

## Related

- [guide.md — Configuration](guide.md#configuration): narrative,
  examples, themes, hooks and notification setup.
- [auth.md](auth.md): credential precedence and subscription login.
- [architecture.md](architecture.md): where the `config` crate sits in the
  crate graph.
- Source: `crates/config/src/types.rs`, `load.rs`, `merge.rs`,
  `validate.rs`; leaf types in `crates/core/src/types.rs`; `plugins.toml`
  schema in `crates/skill/src/plugin.rs` and `registry.rs`.
