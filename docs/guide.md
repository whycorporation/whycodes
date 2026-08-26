# User guide

How to run whycodes, drive the TUI, and configure it. Authentication details
live in [auth.md](auth.md). Crate layout is in [architecture.md](architecture.md).

## Command line

```
Usage: whycodes [OPTIONS] [COMMAND]

Commands:
  run       Start an interactive session (default)
  generate  One-shot, non-interactive turn
  acp       Agent Client Protocol (not yet implemented)
  pr        Create a pull request from current changes
  github    GitHub operations
  serve     Start the local API / share server
  connect   Attach a TUI to a running serve daemon
  web       Open web UI (not yet implemented; use serve + browser)
  mcp       MCP server management
  provider  Provider management (add, list, remove, default)
  model     Model management
  agent     Agent configuration
  plugins   List shell plugins (plugins.toml + plugin.json)
  config    Configuration management
  session   Session management (list, view, delete, rename, share)
  memory    Cross-session memory (list, search, add, delete, clear, path, …)
  auth      Subscription login via OAuth
  stats     Show usage statistics
  debug     Show debug information
  upgrade   Self-update
  completions  Shell completion scripts (bash, zsh, fish, powershell, elvish)

Options (global):
  -P, --provider <PROVIDER>  Provider to use
  -m, --model <MODEL>        Model to use
  -a, --agent <AGENT>        Agent name to use
  -d, --dir <DIR>            Project directory (defaults to cwd)
  -c, --continue             Resume the most recently updated session
  -r, --resume <SESSION_ID>  Resume a saved session (full id or unique prefix)
      --plain                Line-based stdin REPL instead of the TUI
      --debug                Write debug logs under the data dir
      --no-memory            Disable cross-session memory for this process
```

The prompt is positional: `whycodes generate "<prompt>"`, not `whycodes -p`.
`generate` (and `run --format json|stream-json`) accept `-t, --max-turns <N>` as
an optional cap with **no default**. Interactive TUI / `whycodes run` ignores
`--max-turns` (Grok parity): the agent runs until it finishes, you cancel, or
the doom-loop guard trips.

```bash
whycodes -d ./my-project
whycodes generate "Explain the error handling in main.rs" -d ./my-project
whycodes run "Where is the retry logic?" -d ./my-project
whycodes --continue
whycodes --resume a1b2c3d4
whycodes session import ./transcript.jsonl --from auto
whycodes -P openai -m gpt-4o generate "Refactor this module"
```

`session import` accepts `--from auto|whycodes|claude|codex|opencode|pi`. Tools do
not replay; resume with `whycodes --resume <id>`.

### Output formats (headless / CI)

`generate` and `run <prompt>` accept `--format` (alias `--output-format`):

| Format | stdout | Use |
|---|---|---|
| `text` (default) | Final assistant text | Humans, simple pipes |
| `json` | One JSON object after the turn | Scripts, jq, cost gates |
| `stream-json` | NDJSON events | Live progress, long tasks |

```bash
whycodes generate "List open TODOs" --format json | jq '{result, usage, session_id}'

whycodes run "Migrate the auth module" --format stream-json -t 20 \
  | jq -r 'select(.type=="result") | .result'

# N prompts, each in its own session, capped at -j workers
whycodes generate "Summarize src/" "Summarize tests/" -j 2 --format json
```

With multiple prompts, `generate` runs each concurrently (semaphore at `-j`).
Every prompt produces a final envelope; a failure never aborts siblings, and
the process exits non-zero if any prompt failed. In `stream-json`, parallel
events are wrapped as
`{"type":"session","session_id":…,"event":{…}}`.

`stream-json` event types: `init`, `text_delta`, `thinking_delta`,
`tool_start`, `tool_end`, `usage`, `status`, `result`, `error`, `cancelled`.
The last event is always `result` (with `is_error` and optional `error`).

Structured formats auto-approve tool permission prompts so pipelines do not
hang on stdin. Catastrophic shell risk is still hard-blocked. Prefer explicit
permission allow rules in config when you want tighter control.

## Interactive session

The default interface is a full-screen TUI. `--plain` is a line-based REPL.

Contributors checking a TUI change on Alacritty / Kitty / WezTerm / VTE: see
[tui-term-matrix.md](tui-term-matrix.md).

| Key | Action |
|---|---|
| `Tab` | Focus prompt ↔ scrollback |
| `Ctrl+T` | Cycle primary agents (`build` ↔ `plan`), when idle |
| `Ctrl+O` | Live session dashboard |
| `Ctrl+N` | New parallel session (up to 8 live) |
| `Ctrl+Tab` | Switch to most recent session |
| `Ctrl+PgUp/PgDn` | Cycle sessions |
| `Esc` | Cancel the in-flight turn; dismiss a dialog; double-Esc clears the draft |
| `?` | Help / keybinding cheatsheet |
| `:` | Command mode (`:theme`, `:q`, …) |
| `Ctrl+P` / `Ctrl+M` | Provider setup / model selection |
| `Ctrl+B` | Toggle sidebar (Files / Diag / MCP / Todos / View) |
| click / `t` | Fold the sticky todo panel (header chevron; finished items stay visible when expanded) |
| `[` / `]` | Cycle sidebar tabs (scrollback focus) |
| `Ctrl+C` / `Ctrl+Q` | Clear draft or quit |

Input prefixes (TUI and `--plain`):

- `!ls -la` runs a shell command and attaches its output to the conversation.
- `@src/main.rs` inlines a file into the prompt.

In the TUI you can drag-drop or paste image file paths (png, jpeg, gif, webp).
Staged images show as chips; Enter sends them with the text. Backspace on an
empty draft removes the last attachment.

### Slash commands

Both TUI and `--plain`:

| Command | Description |
|---|---|
| `/help` `/h` | Show help |
| `/exit` `/quit` `/q` | Exit |
| `/new` `/clear` | Start a new session |
| `/rename [name]` | Set the session title (locks auto-title) |
| `/init` | Create or update `AGENTS.md` |
| `/review` | AI review of git changes (read-only) |
| `/security-review` | Security-focused review of changes |
| `/commit` | Draft (and optionally create) a git commit |
| `/diff` | Git status + `diff --stat` |
| `/undo` | Undo the last turn and restore files via git |
| `/redo` | Redo the last undone turn |
| `/share` `/export` | Export the session and print its local share URL |
| `/compact [context]` `/summarize` | Compact the conversation (LLM full-replace; optional note of what to keep) |
| `/fresh` | Skip the provider prompt cache on the next turn (stale cache / wedged stream) |
| `/context` | Context window breakdown |
| `/cost` `/usage` | Session + last-turn token usage |
| `/sessions` | Session picker (Enter to resume) |
| `/resume [id]` | Resume by id/prefix, or open the picker |
| `/continue` | Resume the most recently updated session |
| `/models [provider/id]` | Show or switch the model |
| `/effort [low\|medium\|high\|xhigh]` | Reasoning effort (TUI: click the chip next to the model) |
| `/agent [name]` | Show or switch the agent |
| `/connect` | Provider and API key setup (starts OAuth when missing) |
| `/login [provider]` | Subscription sign-in |
| `/remember [text]` | Save a durable project memory |
| `/memory` | Show memory path and recent entries |
| `/themes` | Theme picker (TUI) / list names (plain) |
| `/tools` | List tools for the current agent |
| `/info` `/details` | Session details |
| `/doctor` | Environment / config diagnostics |

TUI only: `/theme [name]`, `/unshare`, `/bg` (list or `kill <id>`),
`/loop` (`/loop 3 <prompt>`; `/loop stop`).

`--plain` only: `/thinking` toggles thinking output.

Project commands under `.whycodes/commands/` are available alongside these;
see [Custom commands](#custom-commands).

### Sharing a session

`/share` exports the session and prints `http://127.0.0.1:3030/s/<id>`.
Serve it with `whycodes serve` (`WHYCODES_SHARE_PORT` overrides the port).
Nothing leaves the machine. `/unshare` removes the exported files.

Attach another TUI to that daemon (turns run on the server; tools are
auto-approved there). This is **not** `/connect` (OAuth login):

```bash
whycodes serve
whycodes connect                 # new session on 127.0.0.1:3030
whycodes connect 127.0.0.1:3030 --session <id>
```

## Embed via the SDK (protocol v1)

`whycodes-sdk` is a **thin HTTP client**. It does not link the agent loop.
The daemon is the product; the crate speaks `/v1/*`.

```rust
use whycodes_sdk::{LaunchOptions, RunOptions, WhyCodesClient};

// Attach to `whycodes serve` already running:
let client = WhyCodesClient::connect("127.0.0.1:3030").await?;

// Or spawn a private instance (inherits this process's env / API keys):
let client = WhyCodesClient::launch(LaunchOptions::default()).await?;

let session = client.create_session(None::<String>).await?;
let turn = client
    .run(&session.id, "summarize this repo", RunOptions::default())
    .await?;
println!("{}", turn.text);
client.close().await?;
```

Events on `/v1/sessions/:id/run` are tagged `ev` (`text_delta`, `tool_start`,
`permission_request`, `turn_done`, …). Unknown tags become `Unknown`.
`run()` auto-approves tool `Ask`s and `question`; `run_events()` does not —
answer with `respond_to_permission` / `respond_to_question`.
`launch({ inherit_logins: false })` uses a private `WHYCODES_HOME`
(config, sessions, auth, memory, skills, browser profile).
`get_history` / `peek`, `list_models` / `set_model`, `rename` / `rewind` /
`compact` are on `/v1`. `run_structured` retries until the reply matches a
JSON Schema subset. Branch on `SdkError.code`.

Same protocol from Node (zero runtime deps, Node 18+):

```ts
import { WhyCodesClient } from "@whycorporation/whycodes-sdk";

const client = await WhyCodesClient.connect("127.0.0.1:3030");
const session = await client.createSession();
const turn = await client.run(session.id, "summarize this repo");
console.log(turn.text);
```

Package source: `sdk/typescript`. `WhyCodesClient.launch()` spawns a private
`whycodes serve`.

`/api/*` remains for TUI attach (`whycodes connect`). New integrations should
use `/v1`.

## Agents

Primary agents run the main conversation (`Ctrl+T` or `/agent`). Subagents
are spawned by the `task` tool (or `swarm` for parallel workers in git
worktrees) and report back. Workers can `swarm_msg` each other (`to` =
`parent` / `all` / `worker-N`). In checkout mode (`[swarm] isolation =
"checkout"`), a `read` of a file another worker wrote is marked stale.
Parallel TUI sessions (`Ctrl+N`) share file claims so they cannot silently
overwrite the same path.

| Agent | Mode | Role |
|---|---|---|
| `build` | primary | Full-access coding (default) |
| `plan` | primary | Read-only planning |
| `ask` | primary | Read-only Q&A |
| `general` | subagent | Multi-step tasks |
| `explore` | subagent | Fast read-only codebase search |
| `scout` | subagent | External docs and dependency research |

In **build**, high-confidence questions get an ephemeral intent hint so the
model answers instead of over-eager edits. Set
`session.intent_guidance = "off"` to disable.

## Tools

| Category | Tools |
|---|---|
| Files | `read` (also `skill://`, `agent://`), `write`, `edit`, `apply_patch` |
| Search | `grep`, `glob`, `list` |
| Execution | `bash` (alias `shell`) |
| Git | `git_status`, `git_diff`, `git_log`, `git_blame`, `git_commit`, `worktree` |
| GitHub | `github_issue`, `github_pr` |
| Web | `webfetch`, `websearch`, `browser` |
| Workflow | `task`, `swarm`, `swarm_msg`, `plan`, `todowrite` (`todo`), `todoread`, `question`, `bg`, `schedule`, `panel`, `checkpoint`, `rewind` |
| Memory | `memory` (`write` / `list` / `search` / `delete` / `learn` / `code_search` / `index`) |
| Extensions | `skill`, `lsp`, `code_mode`, `external_directory`, `truncate`, `tool_search`, `plugin_*` |

`grep` is in-process (`regex` crate). It skips dot directories, common build
directories and binary files. MCP server tools bind as `{server}_{tool}`.

`browser` drives a local Chromium/Chrome via CDP (`status`, `open`, `snapshot`,
`click`, `type`, `wait`, `screenshot`, `close`). It is **not** in the core
profile (`tool_search` or `session.tool_profile = "full"`). Permission
defaults to `ask`. The OS sandbox and HTTP domain allowlist do **not** apply
inside the real browser. Set `WHYCODES_BROWSER` if Chrome is not on `PATH`.

`checkpoint` / `rewind` (deferred; `tool_search` or `session.tool_profile = "full"`)
mark conversation state before a speculative investigation and later collapse
that exploratory context into a short report. They do not snapshot files.

`panel` pins a file, unified diff, or mermaid diagram on the TUI sidebar
Preview tab (`action`: `show_file` / `show_diff` / `show_mermaid` / `clear`).
It is not in the core tool profile — activate it with `tool_search` or set
`session.tool_profile = "full"`. `Ctrl+B` toggles the sidebar; `[` / `]`
cycle tabs from scrollback. Set `[tui] show_sidebar = true` to open it by
default.

## Memory

On by default, per project:

- **Auto memory** — a human-editable `MEMORY.md`.
- **Semantic facts** — durable facts in SQLite with embeddings. Top hits are
  injected into the system prompt; new facts are retained after each turn.
- **Code RAG** — chunk index via `whycodes memory index`, searched with
  `whycodes memory code-search`.

Manage it with `whycodes memory …`, `/remember` / `/memory`, or the agent's
`memory` tool. Project scope: `.whycodes/memory`. User scope: the data dir.

```toml
[memory]
enabled = true
auto_inject = true
auto_retain = true
```

`whycodes --no-memory` disables the subsystem for one run. The default
embedder is a local hashing model; `--features onnx` adds MiniLM
(`whycodes memory onnx-smoke` verifies the download).

Past turns are embedded after each turn. Search them with
`whycodes memory session-search "<query>"`; matching excerpts are also
injected as `# Past sessions`. After retain, the fact bank is capped
(`memory.consolidate_max`, default 80) by dropping the least-recalled
entries.

## Configuration

`config.toml` in the platform config directory. `whycodes debug` prints the
exact path.

```toml
[providers.anthropic]
name = "anthropic"
api_key = "sk-ant-..."

[providers.openai]
name = "openai"
api_key = "sk-..."

[tui]
theme = "default_dark"

[session]
auto_title = true
# title_model = "anthropic/claude-haiku-4-5-20251001"

[mcp_servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]

# Remote MCP (Streamable HTTP; falls back to SSE)
# [mcp_servers.remote]
# url = "https://mcp.example.com/mcp"
# type = "http"   # or "sse" / "auto"
# headers = { Authorization = "Bearer …" }

[[agents]]
name = "build"
description = "Default coding agent"
mode = "primary"
```

Layers, each overriding the one above:

1. Built-in defaults
2. Global `config.toml` (platform config dir, or `$WHYCODES_HOME/config.toml`)
3. Project `.whycodes/config.toml`
4. `WHYCODES_*` environment variables

`WHYCODES_HOME`, when set, is the instance root: config, sessions, auth,
memory, skills and the browser profile all live under it. `whycodes debug`
prints the resolved paths. Isolated SDK `launch({ inherit_logins: false })`
sets this automatically.

Project instructions belong in `AGENTS.md` at the repository root (`/init`
generates one). WhyCodes also loads sibling instruction files already in the
checkout so you do not have to migrate them: `CLAUDE.md`, `GEMINI.md`,
`.github/copilot-instructions.md`, `.cursorrules`, `.cursor/rules/*.mdc`,
`.clinerules`, `.windsurfrules`, and `.whycodes/AGENTS.md`. Duplicate content
is skipped. In a git repo, parent directories up to the repository root are
included.

Skills are listed in the system prompt by **name and description only**. The
body is loaded with `read skill://<name>` or `skill` (`action=load`). Project
trees: `.skills/*.skill.md`, `.whycodes/skills/`, `.claude/skills/*/SKILL.md`.

Finished `task` / `swarm` workers also write `.whycodes/agents/<id>.md`,
readable as `read agent://<id>` (`read agent://` lists them).

Standalone lowercase words in a prompt can change that turn only (the word
stays visible; a hidden notice is added to the LLM request):

- `ultrathink` — careful multi-step reasoning, and the highest thinking
  effort the model supports
- `orchestrate` — fan work out through `task` / `swarm`, verify each phase,
  and finish the request

Disable with `[session.magic_keywords] enabled = false`, or per keyword
(`ultrathink = false`). Paths, identifiers, code spans, and fences do not
match (`orchestrate.ts`, `Ultrathink`, `` `ultrathink` ``).

Model roles (optional; empty = auto small sibling of the session model):

```toml
[session]
model_smol = "anthropic/claude-haiku-4-5-20251001"   # task + swarm workers
model_plan = "anthropic/claude-opus-4-6"            # while /agent plan
reasoning_effort = "medium"                         # low | medium | high | xhigh
```

OpenAI-compat and xAI Grok send `reasoning_effort` on thinking models. Default
is `medium`. `xhigh` (Max) is grok-4.6+ only; older Grok models clamp to
`high`. In the TUI the current level sits next to the model name on the prompt
border — click it (or `/effort`) to pick. `ultrathink` in a prompt still
raises that turn to `high`.

Context-window errors are classified separately from 429s: the same request is
not retried; whycodes compacts once and retries the step. Older tool dumps are
shaken to 512 characters when the session is still hot.

Stream rules abort a draft mid-token when the assistant text matches a regex,
then inject the hint:

```toml
[[session.stream_rules]]
name = "no-box-leak"
pattern = "Box::leak"
hint = "Don't use Box::leak on production paths; prefer Arc."
```

### Subscription login

`whycodes auth login` signs in with an existing subscription
(`anthropic`, `openai`, `github-copilot`, `google`, `google-antigravity`,
`xai`). Resolution order is env var → config `api_key` → OAuth store, so
an explicit key always wins. Details: [auth.md](auth.md).

### Themes

29 built-in themes (Catppuccin, Tokyo Night, Rose Pine, Gruvbox, Nord,
Dracula, Solarized, Ayu, …). Set `[tui] theme`, or `/themes` in the REPL.

Drop extra themes next to `config.toml` as JSON using
[opencode's theme schema](https://opencode.ai/theme.json):

```json
{
  "defs":  { "darkRed": "#e06c75" },
  "theme": { "error": { "dark": "darkRed", "light": "#d1383d" } }
}
```

Each file provides `name` and `name-light`. Only `background`, `text`,
`border` and `accent` are required; the rest falls back to the built-in.
A file theme wins over a built-in of the same name. A malformed file is
skipped without taking the TUI down.

The prompt footer (bottom-right of the input box) and the header chip
color the active agent and the provider/model. Built-ins: `build` uses
`success`, `plan` uses `accent`, `ask` uses `info`, and the model uses
`info`. Override per agent (or `model`) in config:

```toml
[tui.agent_colors]
build = "#7aa2f7"
plan = "accent"
ask = "info"
model = "secondary"
```

Values are `#rgb` / `#rrggbb`, a theme role (`accent`, `success`, `info`,
`warning`, `error`, `primary`, `secondary`, `thinking`, `dim`), or an
ANSI name (`red`, `green`, …). Theme JSON can set the same slots as
`agentBuild`, `agentPlan`, `agentAsk`, and `model`.

### Permissions

```toml
[permission]
bash = "ask"
edit = "allow"
webfetch = "allow"
"mymcp_*" = "deny"
```

`allow` runs immediately, `deny` blocks. `ask` prompts — a y/n dialog in
the TUI, a stdin prompt in `--plain` — unless overridden:

| Condition | Result for `ask` |
|---|---|
| `WHYCODES_AUTO_APPROVE=1` | auto-allow |
| `WHYCODES_AUTO_DENY=1` | auto-deny |
| stdin is not a terminal | auto-deny |

### Shell command risk

The table above decides by tool *name*, before the command is known. Shell
commands are also classified by what they would destroy:

| Level | Meaning | Example |
|---|---|---|
| `safe` | Read-only, or confined to the project | `cargo build`, `rm build.log` |
| `caution` | Writes or deletes inside the project | `rm -rf target`, `git reset --hard` |
| `destructive` | Outside the project, or hard to undo | `rm -rf /tmp/x`, `git push --force` |
| `catastrophic` | Home, system, or a whole disk | `rm -rf ~`, `mkfs`, `dd of=/dev/sda` |

```toml
[security]
bash_risk_threshold = "destructive"   # caution | destructive | off
```

The threshold is the lowest level that prompts. Default is `destructive`:
`caution` would prompt on ordinary cleanup like `rm -rf target`.

`catastrophic` is refused outright and **cannot be approved** — not by a
prompt, not by `bash = "allow"`, not by `bash_risk_threshold = "off"`.

Limits: an unrecognised command is treated as `safe` (the alternative is
prompting on every build). An obfuscated command can defeat a static
parser, which is why the catastrophic tier checks paths. This layer is
defence in depth, not a sandbox — the OS sandbox is the second lock.

### Shell OS sandbox

Default `security.sandbox = "workspace"`. On Linux this uses
[bubblewrap](https://github.com/containers/bubblewrap):

| Mode | Behaviour |
|---|---|
| `workspace` (default) | Project is read-write; the rest of the host is read-only; `/tmp` is a private tmpfs. Common toolchain caches (`~/.cargo`, `~/.npm`, …) stay writable. |
| `off` | Host `bash -c` with no namespace isolation. |

Network is allowed inside the sandbox by default. Set
`sandbox_network = false` to cut TCP/UDP (`--unshare-net`). Dedicated
tools (`webfetch`, `websearch`) are unchanged by this flag.

```toml
[security]
bash_risk_threshold = "destructive"
sandbox = "workspace"                 # off | workspace
sandbox_network = true
sandbox_fallback = "allow"            # allow | deny (when bwrap is missing)
# network_allowlist = ["github.com", "crates.io", "*.npmjs.org"]
# network_denylist = ["tracking.example.com"]
```

| Env | Effect |
|---|---|
| `WHYCODES_SANDBOX` | `off` or `workspace` |
| `WHYCODES_SANDBOX_NETWORK` | `0`/`1` |
| `WHYCODES_SANDBOX_FALLBACK` | `allow` or `deny` |
| `WHYCODES_NETWORK_ALLOWLIST` | comma/space-separated host patterns |
| `WHYCODES_NETWORK_DENYLIST` | comma/space-separated host patterns |

If `bwrap` is missing (or you are on macOS/Windows), `sandbox_fallback =
"allow"` warns and runs on the host; `"deny"` fails the tool call.
This is not a multi-tenant security boundary — it reduces blast radius.

### Network allowlist (HTTP tools)

Applies to `webfetch`, `websearch` / `mcp_websearch`, `github_issue` and
`github_pr`. Empty allowlist (default) means unrestricted; a non-empty
allowlist requires a host match; denylist always wins.

| Pattern | Matches |
|---|---|
| `example.com` | apex and any subdomain |
| `*.example.com` | subdomains only (not the apex) |
| `*` | any host |

Shell network stays binary (`sandbox_network`). Domain filtering does not
apply inside the sandboxed shell. If you set an allowlist and still want
search, include the provider hosts (`serpapi.com` and/or
`html.duckduckgo.com`).

### Tool hooks

```toml
[[hooks]]
event = "pre_tool"
match = "bash"              # tool name; `*`, `prefix*`, or `*suffix`
command = "echo checking $WHYCODES_TOOL_NAME"
block_on_failure = true     # non-zero exit refuses the tool (pre only)
timeout_secs = 30

[[hooks]]
event = "post_tool"
match = "*"
command = "logger -t whycodes \"done $WHYCODES_TOOL_NAME err=$WHYCODES_TOOL_IS_ERROR\""
```

Environment: `WHYCODES_HOOK_EVENT`, `WHYCODES_TOOL_NAME`, `WHYCODES_TOOL_ID`,
`WHYCODES_TOOL_INPUT` (JSON), `WHYCODES_SESSION_ID`, `WHYCODES_WORKING_DIR`.
Post-tool also sets `WHYCODES_TOOL_IS_ERROR` (`0`/`1`) and
`WHYCODES_TOOL_OUTPUT` (truncated). Hooks run after the risk gate and
permission prompt, before execution. Subagent loops do not invoke hooks yet.

### Custom commands

Markdown files become slash commands named after the file, from
`.whycodes/commands/` and from a `commands/` directory next to the global
`config.toml`.

```markdown
---
description: Run tests
agent: build
---
Run the full test suite and summarize failures.
Focus on $ARGUMENTS.
```

`/test unit` sends the body with `$ARGUMENTS` replaced by `unit`.
`$1`, `$2`, … are also expanded.

### Shell plugins

External commands register as `plugin_<name>` (full profile or
`tool_search`). Two layouts, merged last-wins by tool name:

**TOML** — `~/.config/com.whycorporation.whycodes/plugins.toml` or
`.whycodes/plugins.toml`:

```toml
[[plugins]]
name = "hello"
command = "echo hello $PLUGIN_ARG_INPUT"
description = "Demo plugin"
```

**Directory** — `$CONFIG/plugins/<id>/plugin.json` or
`.whycodes/plugins/<id>/plugin.json` (`manifest.json` is also accepted):

```json
{
  "name": "hello",
  "description": "Demo plugin",
  "command": "./run.sh"
}
```

Relative commands resolve against the plugin directory; the child starts
there. If `command` is omitted, `run` / `run.sh` / `plugin.sh` is used
when present. A `tools` array exposes several commands (`plugin_<name>_<tool>`
when there is more than one). `whycodes plugins` lists the merged set.

Args become `PLUGIN_ARG_<KEY>` environment variables. `PLUGIN_WORKSPACE` is
the session cwd. Config `[hooks]` stay the hook path — `hooks` in
`plugin.json` is reserved, not loaded.

A plugin with `"kind": "auth"` registers an OAuth spec instead of a shell
tool (`whycodes auth login`). Subscription-login plugins are **not** in
the default install; see [auth.md](auth.md).

### Interoperability

| Convention | Where |
|---|---|
| `AGENTS.md` project instructions | Repository root (also `CLAUDE.md`, `GEMINI.md`, Copilot, Cursor, Cline, Windsurf) |
| Markdown slash commands | `.whycodes/commands/`, also `.opencode/commands/` |
| MCP servers | `[mcp_servers]` in `config.toml`, tools as `{server}_{tool}` |
| Shell plugins | `plugins.toml` or `plugins/*/plugin.json` → `plugin_<name>` |
| Auth plugins | `plugins/*/plugin.json` with `"kind": "auth"` → OAuth login |
| `allow` / `ask` / `deny` | `[permission]` in `config.toml` |
