# User guide

How to run whycode, drive the TUI, and configure it. Authentication details
live in [auth.md](auth.md). Crate layout is in [architecture.md](architecture.md).

## Command line

```
Usage: whycode [OPTIONS] [COMMAND]

Commands:
  run       Start an interactive session (default)
  generate  One-shot, non-interactive turn
  acp       Agent Client Protocol (not yet implemented)
  pr        Create a pull request from current changes
  github    GitHub operations
  serve     Start the local API / share server
  web       Open web UI (not yet implemented; use serve + browser)
  mcp       MCP server management
  provider  Provider management (add, list, remove, default)
  model     Model management
  agent     Agent configuration
  plugins   List shell plugins from plugins.toml (global + project)
  config    Configuration management
  session   Session management (list, view, delete, rename, share)
  memory    Cross-session memory (list, search, add, delete, clear, path, …)
  auth      Subscription login via OAuth
  stats     Show usage statistics
  debug     Show debug information
  upgrade   Self-update

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

The prompt is positional: `whycode generate "<prompt>"`, not `whycode -p`.
`run` and `generate` also accept `-t, --max-turns <N>` (default 25).

```bash
whycode -d ./my-project
whycode generate "Explain the error handling in main.rs" -d ./my-project
whycode run "Where is the retry logic?" -d ./my-project
whycode --continue
whycode --resume a1b2c3d4
whycode -P openai -m gpt-4o generate "Refactor this module"
```

### Output formats (headless / CI)

`generate` and `run <prompt>` accept `--format` (alias `--output-format`):

| Format | stdout | Use |
|---|---|---|
| `text` (default) | Final assistant text | Humans, simple pipes |
| `json` | One JSON object after the turn | Scripts, jq, cost gates |
| `stream-json` | NDJSON events | Live progress, long tasks |

```bash
whycode generate "List open TODOs" --format json | jq '{result, usage, session_id}'

whycode run "Migrate the auth module" --format stream-json -t 20 \
  | jq -r 'select(.type=="result") | .result'

# N prompts, each in its own session, capped at -j workers
whycode generate "Summarize src/" "Summarize tests/" -j 2 --format json
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
| `Ctrl+B` | Toggle sidebar |
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
| `/compact` `/summarize` | Compact the conversation context |
| `/context` | Context window breakdown |
| `/cost` `/usage` | Session + last-turn token usage |
| `/sessions` | Session picker (Enter to resume) |
| `/resume [id]` | Resume by id/prefix, or open the picker |
| `/continue` | Resume the most recently updated session |
| `/models [provider/id]` | Show or switch the model |
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

Project commands under `.whycode/commands/` are available alongside these;
see [Custom commands](#custom-commands).

### Sharing a session

`/share` exports the session and prints `http://127.0.0.1:3030/s/<id>`.
Serve it with `whycode serve` (`WHYCODE_SHARE_PORT` overrides the port).
Nothing leaves the machine. `/unshare` removes the exported files.

## Agents

Primary agents run the main conversation (`Ctrl+T` or `/agent`). Subagents
are spawned by the `task` tool (or `swarm` for parallel workers in git
worktrees) and report back.

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
| Files | `read`, `write`, `edit`, `apply_patch` |
| Search | `grep`, `glob`, `list` |
| Execution | `bash` (alias `shell`) |
| Git | `git_status`, `git_diff`, `git_log`, `git_blame`, `git_commit`, `worktree` |
| GitHub | `github_issue`, `github_pr` |
| Web | `webfetch`, `websearch` |
| Workflow | `task`, `swarm`, `plan`, `todowrite` (`todo`), `todoread`, `question`, `bg`, `schedule` |
| Memory | `memory` |
| Extensions | `skill`, `lsp`, `code_mode`, `external_directory`, `truncate`, `tool_search` |

`grep` is in-process (`regex` crate). It skips dot directories, common build
directories and binary files. MCP server tools bind as `{server}_{tool}`.

## Memory

On by default, per project:

- **Auto memory** — a human-editable `MEMORY.md`.
- **Semantic facts** — durable facts in SQLite with embeddings. Top hits are
  injected into the system prompt; new facts are retained after each turn.
- **Code RAG** — chunk index via `whycode memory index`, searched with
  `whycode memory code-search`.

Manage it with `whycode memory …`, `/remember` / `/memory`, or the agent's
`memory` tool. Project scope: `.whycode/memory`. User scope: the data dir.

```toml
[memory]
enabled = true
auto_inject = true
auto_retain = true
```

`whycode --no-memory` disables the subsystem for one run. The default
embedder is a local hashing model; `--features onnx` adds MiniLM
(`whycode memory onnx-smoke` verifies the download).

## Configuration

`config.toml` in the platform config directory. `whycode debug` prints the
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
2. Global `config.toml`
3. Project `.whycode/config.toml`
4. `WHYCODE_*` environment variables

Project instructions belong in `AGENTS.md` at the repository root (`/init`
generates one). It is injected into the system prompt automatically.

### Subscription login

`whycode auth login` signs in with an existing subscription. Resolution
order is env var → config `api_key` → OAuth store, so an explicit key
always wins. Details: [auth.md](auth.md).

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
| `WHYCODE_AUTO_APPROVE=1` | auto-allow |
| `WHYCODE_AUTO_DENY=1` | auto-deny |
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
| `WHYCODE_SANDBOX` | `off` or `workspace` |
| `WHYCODE_SANDBOX_NETWORK` | `0`/`1` |
| `WHYCODE_SANDBOX_FALLBACK` | `allow` or `deny` |
| `WHYCODE_NETWORK_ALLOWLIST` | comma/space-separated host patterns |
| `WHYCODE_NETWORK_DENYLIST` | comma/space-separated host patterns |

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
command = "echo checking $WHYCODE_TOOL_NAME"
block_on_failure = true     # non-zero exit refuses the tool (pre only)
timeout_secs = 30

[[hooks]]
event = "post_tool"
match = "*"
command = "logger -t whycode \"done $WHYCODE_TOOL_NAME err=$WHYCODE_TOOL_IS_ERROR\""
```

Environment: `WHYCODE_HOOK_EVENT`, `WHYCODE_TOOL_NAME`, `WHYCODE_TOOL_ID`,
`WHYCODE_TOOL_INPUT` (JSON), `WHYCODE_SESSION_ID`, `WHYCODE_WORKING_DIR`.
Post-tool also sets `WHYCODE_TOOL_IS_ERROR` (`0`/`1`) and
`WHYCODE_TOOL_OUTPUT` (truncated). Hooks run after the risk gate and
permission prompt, before execution. Subagent loops do not invoke hooks yet.

### Custom commands

Markdown files become slash commands named after the file, from
`.whycode/commands/` and from a `commands/` directory next to the global
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

### Interoperability

| Convention | Where |
|---|---|
| `AGENTS.md` project instructions | Repository root |
| Markdown slash commands | `.whycode/commands/`, also `.opencode/commands/` |
| MCP servers | `[mcp_servers]` in `config.toml`, tools as `{server}_{tool}` |
| `allow` / `ask` / `deny` | `[permission]` in `config.toml` |
