# Whycode

A coding agent for the terminal, written in Rust. It reads, writes and edits
files, runs commands, searches codebases and drives an LLM through multi-turn
tool use — either in a full-screen TUI or as a one-shot CLI invocation.

- Ships as a single binary with no runtime dependencies. Search runs in-process,
  so `ripgrep` and `grep` are not required on `PATH`.
- Built and tested on Linux, macOS and Windows in CI.
- Works with Anthropic, OpenAI, Google, Groq, xAI, DeepSeek, Ollama, OpenRouter,
  Mistral, Together, and any OpenAI-compatible endpoint.
- Reads the project's `AGENTS.md`, connects to MCP servers, and drives language
  servers over LSP.
- Latency-focused agent loop: core tool profile, Anthropic prompt cache,
  parallel safe tools, trivial-chat fast model route, doom-loop guard — see
  [docs/FEATURES.md](docs/FEATURES.md) §10 and
  [docs/plan-latency-competitors.md](docs/plan-latency-competitors.md).

## Installation

macOS and Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/whycorporation/whycode/main/scripts/install.sh | sh
```

Windows:

```powershell
irm https://raw.githubusercontent.com/whycorporation/whycode/main/scripts/install.ps1 | iex
```

Both verify the download against the release's `SHA256SUMS` and refuse to
install anything that does not match. Neither modifies `PATH`; they print the
install directory if it is not already on it. `WHYCODE_INSTALL_DIR` overrides
the location, and `scripts/uninstall.sh` / `uninstall.ps1` remove it again —
add `--purge` / `-Purge` to also delete config and session data.

The binaries are unsigned, so macOS Gatekeeper and Windows SmartScreen will
warn on first run.

`whycode upgrade` replaces the running binary with the newest release, checksum
verified, and leaves the existing one in place if anything fails.

### Homebrew (partial)

macOS and Linux with [Homebrew](https://brew.sh). The formula lives in this
repo (`Formula/whycode.rb`); a dedicated tap repo is not required yet.

```bash
brew tap whycorporation/whycode https://github.com/whycorporation/whycode
brew install --HEAD whycode
```

`--HEAD` builds from `main` (Homebrew installs the Rust toolchain as a build
dependency). After the first tagged release, `scripts/update_homebrew_formula.sh`
switches the formula to prebuilt binaries; then `brew install whycode` works
without compiling. Details: [`packaging/README.md`](packaging/README.md).

From source, which needs a Rust toolchain:

```bash
git clone https://github.com/whycorporation/whycode.git
cd whycode
cargo build --release -p whycode-cli
# Optional extras (Unicode mermaid + bat/two-face languages, ~+1.7 MB):
# cargo build --release -p whycode-cli --features full
```

The binary is written to `target/release/whycode`.

## Quick start

```bash
export ANTHROPIC_API_KEY="sk-ant-..."

# Interactive session in a project
whycode -d ./my-project

# One-shot, non-interactive
whycode generate "Explain the error handling in main.rs" -d ./my-project

# CI / scripts: final JSON envelope (result, usage, session_id)
whycode generate "Summarize the last commit" --format json | jq -r '.result'

# CI / scripts: live NDJSON event stream
whycode generate "Refactor utils" --format stream-json | jq -r '.type'

# Interactive, seeded with an opening prompt
whycode run "Where is the retry logic?" -d ./my-project

# Resume the last saved session (same project dir recommended)
whycode --continue
# or a specific session id / unique prefix
whycode --resume a1b2c3d4

# A different provider and model
whycode -P openai -m gpt-4o generate "Refactor this module"
```

## Command line

```
Usage: whycode [OPTIONS] [COMMAND]

Commands:
  run       Start an interactive session (default)
  generate  Generate code from a prompt (non-interactive)
  acp       Agent Client Protocol (stub; after product launch)
  pr        Create a pull request from current changes
  github    GitHub operations
  serve     Start API server
  web       Open web UI (not yet implemented; use serve + browser)
  mcp       MCP server management
  provider  Provider management (add, list, remove, default)
  model     Model management
  agent     Agent configuration
  config    Configuration management
  session   Session management (list, view, delete, rename, share)
  stats     Show usage statistics
  debug     Show debug information
  upgrade   Self-update

Options (global):
  -P, --provider <PROVIDER>  Provider to use
  -m, --model <MODEL>        Model to use
  -a, --agent <AGENT>        Agent name to use
  -d, --dir <DIR>            Project directory (defaults to the current directory)
  -c, --continue             Resume the most recently updated saved session
  -r, --resume <SESSION_ID>  Resume a saved session (full id or unique prefix)
      --plain                Use plain stdin REPL instead of the full-screen TUI
      --debug                Write debug logs under the data dir
```

The prompt is a positional argument rather than a flag — `whycode generate
"<prompt>"`, not `whycode -p "<prompt>"`. `run` and `generate` also accept
`-t, --max-turns <N>` (default 25).

### Output formats (headless / CI)

`generate` and `run <prompt>` accept `--format` (alias `--output-format`):

| Format | Flag | stdout shape | Use case |
|---|---|---|---|
| **text** | `--format text` (default) | Final assistant text | Humans, simple pipes |
| **json** | `--format json` | One JSON object after the turn | Scripts, jq, cost gates |
| **stream-json** | `--format stream-json` | NDJSON events (one object per line) | Live progress, long tasks |

```bash
# Final envelope only
whycode generate "List open TODOs" --format json | jq '{result, usage, session_id}'

# Live event types
whycode run "Migrate the auth module" --format stream-json -t 20 \
  | jq -r 'select(.type=="result") | .result'

# Parallel fan-out: N prompts, each with its own session, capped at -j workers
whycode generate "Summarize src/" "Summarize tests/" -j 2 --format json \
  | jq -r '.session_id'
```

With multiple prompts, `generate` runs each in its own agent + session
concurrently (semaphore at `-j` workers). Every prompt always produces a
final envelope — per-prompt failures never abort siblings, and the process
exits non-zero if any prompt failed. In `stream-json`, parallel events are
wrapped as `{"type":"session","session_id":…,"event":{…}}` so interleaved
output stays attributable.

`stream-json` events use a `type` field: `init`, `text_delta`, `thinking_delta`,
`tool_start`, `tool_end`, `usage`, `status`, `result`, `error`, `cancelled`.
The last event is always `result` (with `is_error` and optional `error`).
Parallel runs add a `session` wrapper (see above).

Structured formats auto-approve tool permission prompts so pipelines do not
hang on stdin (catastrophic shell risk is still hard-blocked). Prefer explicit
permission allow rules in config when you want tighter control.

## Interactive session

The default interface is a full-screen TUI with live streaming of text, tool
calls and thinking. `--plain` switches to a line-based stdin REPL.

| Key | Action |
|---|---|
| `Tab` | Focus prompt ↔ scrollback |
| `Ctrl+T` | Cycle primary agents (`build` ↔ `plan`), when idle |
| `Ctrl+O` | Live session dashboard (needs-input → working → idle) |
| `Ctrl+N` | New parallel session (up to 8 live) |
| `Ctrl+Tab` | Switch to most recent session |
| `Ctrl+PgUp/PgDn` | Cycle sessions in order |
| `Esc` | Cancel the in-flight turn (keeps partial output); dismiss a dialog; double-Esc clears the draft |
| `?` | Toggle the help / keybinding cheatsheet |
| `:` | Enter command mode (`:theme`, `:q`, …) |
| `Ctrl+P` / `Ctrl+M` | Provider setup / model selection |
| `Ctrl+B` | Toggle sidebar |
| `Ctrl+C` / `Ctrl+Q` | Clear draft or quit |

Two input prefixes are available in both interfaces:

- `!ls -la` runs a shell command and attaches its output to the conversation.
- `@src/main.rs` inlines a file's contents into the prompt.

In the TUI you can also **drag-drop or paste image file paths** onto the prompt
(png, jpeg, gif, webp, …). Staged images show as chips above the input; Enter
sends them as multimodal content with your text. Backspace on an empty draft
removes the last attachment.

### Slash commands

Available in both the TUI and the `--plain` REPL:

| Command | Description |
|---|---|
| `/help` `/h` | Show help |
| `/exit` `/quit` `/q` | Exit |
| `/new` `/clear` | Start a new session |
| `/rename [name]` | Set the session title (locks auto-title) |
| `/init` | Create or update `AGENTS.md` |
| `/undo` | Undo the last turn and restore files via git |
| `/redo` | Redo the last undone turn |
| `/share` `/export` | Export the session and print its local share URL |
| `/compact` `/summarize` | Compact the conversation context |
| `/sessions` | Open the session picker (Enter to resume) |
| `/resume [id]` | Resume by id/prefix, or open the picker |
| `/continue` | Resume the most recently updated session |
| `/models [provider/id]` | Show or switch the model |
| `/agent [name]` | Show or switch the agent |
| `/connect` | Provider and API key setup help |
| `/tools` | List the tools available to the current agent |
| `/info` `/details` | Session details |

TUI only:

| Command | Description |
|---|---|
| `/unshare` | Delete the local share files for this session |

`--plain` REPL only:

| Command | Description |
|---|---|
| `/thinking` | Toggle display of thinking output |
| `/themes` | List the available TUI themes |

Commands defined under `.whycode/commands/` are available alongside these; see
[Custom commands](#custom-commands).

### Sharing a session

`/share` exports the session and prints a URL of the form
`http://127.0.0.1:3030/s/<id>`. Serve it with `whycode serve`; the port can be
overridden with `WHYCODE_SHARE_PORT`. Nothing leaves the machine — `/unshare`
removes the exported files.

## Agents

Primary agents run the main conversation and are switched with `Ctrl+T` or
`/agent`. Subagents are spawned by the `task` tool for scoped work (or `swarm`
for parallel workers in git worktrees with merge + conflict notify) and report
back to the primary agent.

| Agent | Mode | Role |
|---|---|---|
| `build` | primary | Full-access coding (default); soft intent posture for Q&A vs edit |
| `plan` | primary | Read-only planning (structured plan, no edits) |
| `ask` | primary | Read-only Q&A / explain (Cursor Ask-style) |
| `general` | subagent | Multi-step tasks |
| `explore` | subagent | Fast read-only codebase search |
| `scout` | subagent | External documentation and dependency research |

Cycle primary agents with `Ctrl+T` or `/agent`. In **build**, high-confidence
questions get an ephemeral intent hint (not stored in history) so the model
answers instead of over-eager edits. Set `session.intent_guidance = "off"` to
disable.

## Tools

| Category | Tools |
|---|---|
| Files | `read`, `write`, `edit`, `apply_patch` |
| Search | `grep`, `glob`, `list` |
| Execution | `bash` (alias `shell`) |
| Git | `git_status`, `git_diff`, `git_log`, `git_blame`, `git_commit` |
| GitHub | `github_issue`, `github_pr` |
| Web | `webfetch`, `websearch` |
| Workflow | `task`, `plan`, `todowrite` (alias `todo`), `todoread`, `question` |
| Extensions | `skill`, `lsp`, `code_mode`, `external_directory`, `truncate` |

`grep` is implemented in-process with the `regex` crate. It skips dot
directories, common build directories and binary files, and needs no external
search binary.

MCP server tools are bound automatically under `{server}_{tool}`.

## Configuration

Configuration is a `config.toml` in the platform config directory. Run `whycode
debug` to print the exact path for your machine.

```toml
[providers.anthropic]
name = "anthropic"
api_key = "sk-ant-..."

[providers.openai]
name = "openai"
api_key = "sk-..."

[tui]
theme = "default_dark"

# Session titles: project-ab → first-message heuristic → small-model refine
[session]
auto_title = true
# title_model = "anthropic/claude-haiku-4-5-20251001"  # optional override

[mcp_servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]

# Remote MCP (Streamable HTTP; falls back to legacy SSE when needed)
# [mcp_servers.remote]
# url = "https://mcp.example.com/mcp"
# type = "http"   # or "sse" / "auto"
# headers = { Authorization = "Bearer …" }

[[agents]]
name = "build"
description = "Default coding agent"
mode = "primary"
```

Settings are layered, with each level overriding the one above it:

1. Built-in defaults
2. Global `config.toml`
3. Project `.whycode/config.toml`
4. `WHYCODE_*` environment variables

Project instructions belong in an `AGENTS.md` at the repository root; `/init`
generates one. It is injected into the system prompt automatically.

### Themes

29 themes are built in, including Catppuccin, Tokyo Night, Rose Pine, Gruvbox,
Nord, Dracula, Solarized and Ayu variants. Set one with `[tui] theme`;
`/themes` in the `--plain` REPL prints the full list of valid names.

Additional themes can be dropped into a `themes/` directory beside
`config.toml` as JSON, using
[opencode's theme schema](https://opencode.ai/theme.json) — opencode's own
theme files work unmodified:

```json
{
  "defs":  { "darkRed": "#e06c75" },
  "theme": { "error": { "dark": "darkRed", "light": "#d1383d" } }
}
```

Each file carries both a dark and a light variant, so `themes/mine.json`
provides two selectable names: `mine` and `mine-light`. Only `background`,
`text`, `border` and `accent` are required; anything else falls back to the
built-in theme, so a partial file is still usable. A file theme wins over a
built-in of the same name. A malformed file is reported with the offending role
and skipped, without taking the other themes or the TUI down with it.

### Permissions

```toml
[permission]
bash = "ask"
edit = "allow"
webfetch = "allow"
"mymcp_*" = "deny"
```

`allow` runs the tool immediately and `deny` blocks it. `ask` prompts — a y/n
dialog in the TUI, a stdin prompt in `--plain` — unless overridden:

| Condition | Result for `ask` |
|---|---|
| `WHYCODE_AUTO_APPROVE=1` | auto-allow |
| `WHYCODE_AUTO_DENY=1` | auto-deny |
| stdin is not a terminal (piped, CI) | auto-deny |

### Shell command risk

The table above decides per tool *name*, before the command is known, so on its
own `bash = "allow"` would run anything a model emits. Shell commands are
therefore also classified by what they would destroy:

| Level | Meaning | Example |
|---|---|---|
| `safe` | Read-only, or confined to the project | `cargo build`, `rm build.log` |
| `caution` | Writes or deletes inside the project | `rm -rf target`, `git reset --hard` |
| `destructive` | Reaches outside the project, or cannot be undone | `rm -rf /tmp/x`, `git push --force`, `curl … \| sh` |
| `catastrophic` | Home directory, system location, or a whole disk | `rm -rf ~`, `mkfs`, `dd of=/dev/sda` |

```toml
[security]
bash_risk_threshold = "destructive"   # caution | destructive | off
```

The threshold is the lowest level that prompts. `destructive` is the default:
`caution` would prompt on ordinary cleanup like `rm -rf target`, which trains
people to switch the feature off.

`catastrophic` is refused outright and **cannot be approved** — not by a prompt,
not by `bash = "allow"`, not by `bash_risk_threshold = "off"`. Run such a
command yourself if you mean it.

Two limits worth stating plainly. An unrecognised command is treated as `safe`,
so a shell script that deletes your home directory is invisible to this; the
alternative is prompting on every build. And a sufficiently obfuscated command
can defeat any static parser, which is why the catastrophic tier checks paths
rather than trusting the parse. That layer is defence in depth, not a sandbox —
the OS sandbox below is the second lock.

### Shell OS sandbox

Shell commands also run under an OS sandbox by default (`security.sandbox =
"workspace"`). On Linux this uses [bubblewrap](https://github.com/containers/bubblewrap)
(`bwrap`):

| Mode | Behaviour |
|---|---|
| `workspace` (default) | Project directory is read-write; the rest of the host is read-only; `/tmp` is a private tmpfs. Common toolchain caches (`~/.cargo`, `~/.npm`, …) stay writable so builds work. |
| `off` | Host `bash -c` with no namespace isolation (previous behaviour). |

Network is allowed inside the sandbox by default so `cargo` / `npm` / `git`
keep working. Set `sandbox_network = false` to cut TCP/UDP for the shell
process (`--unshare-net`). Dedicated tools (`webfetch`, `websearch`) are
unchanged by this flag.

```toml
[security]
bash_risk_threshold = "destructive"   # caution | destructive | off
sandbox = "workspace"                 # off | workspace
sandbox_network = true                # false → no network in sandboxed shell
sandbox_fallback = "allow"            # allow | deny (when bwrap is missing)
# network_allowlist = ["github.com", "crates.io", "*.npmjs.org"]
# network_denylist = ["tracking.example.com"]
```

| Env | Effect |
|---|---|
| `WHYCODE_SANDBOX` | `off` or `workspace` |
| `WHYCODE_SANDBOX_NETWORK` | `0`/`1` (or true/false) |
| `WHYCODE_SANDBOX_FALLBACK` | `allow` or `deny` |
| `WHYCODE_NETWORK_ALLOWLIST` | comma/space-separated host patterns |
| `WHYCODE_NETWORK_DENYLIST` | comma/space-separated host patterns |

If `bwrap` is not installed (or you are on macOS/Windows), `sandbox_fallback =
"allow"` warns and runs on the host; `"deny"` fails the tool call instead.
This is still not a multi-tenant security boundary — it reduces blast radius
for agent shell mistakes and obfuscated commands that slip past the risk
classifier.

### Network allowlist (HTTP tools)

Domain policy for tools that open remote URLs: `webfetch`, `websearch` /
`mcp_websearch`, and GitHub API tools (`github_issue`, `github_pr`). Empty
allowlist (default) means unrestricted; a non-empty allowlist requires a host
match; denylist always wins.

| Pattern | Matches |
|---|---|
| `example.com` | apex and any subdomain (`api.example.com`) |
| `*.example.com` | subdomains only (not the apex) |
| `*` | any host |

Shell network stays binary (`sandbox_network`). Domain filtering does not apply
inside the sandboxed shell — only to the dedicated HTTP tools above. When you
set an allowlist and still want search, include the provider hosts
(`serpapi.com` and/or `html.duckduckgo.com`).

### Tool hooks

Run shell commands before or after each tool call (primary agent path):

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
Post-tool also sets `WHYCODE_TOOL_IS_ERROR` (`0`/`1`) and `WHYCODE_TOOL_OUTPUT`
(truncated). Hooks run after the risk gate and permission prompt, before
execution. Subagent tool loops do not invoke hooks yet.

### Custom commands

Markdown files become slash commands named after the file. They are read from
`.whycode/commands/` in the project and from a `commands/` directory next to the
global `config.toml`. `.whycode/commands/test.md`:

```markdown
---
description: Run tests
agent: build
---
Run the full test suite and summarize failures.
Focus on $ARGUMENTS.
```

`/test unit` sends the body with `$ARGUMENTS` replaced by `unit`. Positional
placeholders `$1`, `$2`, … are also expanded.

### Interoperability

Whycode uses the file formats and naming that terminal coding agents have
converged on, so a repository set up for another agent needs no changes:

| Convention | Where |
|---|---|
| `AGENTS.md` project instructions | Repository root |
| Markdown slash commands | `.whycode/commands/`, also read from `.opencode/commands/` |
| MCP servers | `[mcp_servers]` in `config.toml`, tools bound as `{server}_{tool}` |
| `allow` / `ask` / `deny` tool permissions | `[permission]` in `config.toml` |

## Architecture

```
crates/
├── core/         — Shared types, config, error handling
├── command-risk/ — Shell command risk classification
├── sandbox/      — OS sandbox for shell (bubblewrap on Linux)
├── format/       — Markdown, syntax highlight, diffs, tables
├── protocol/     — CI / stream-json event envelopes
├── schema/       — Schema validation
├── llm/          — LLM providers
├── tools/        — Tool system and built-ins
├── session/      — Conversation, compaction, undo/redo
├── agent/        — Agent loop, subagents, AGENTS.md
├── skill/        — Skill registry
├── plugin/       — Plugin loader
├── function/     — Function-tool helpers
├── mcp/          — MCP client
├── lsp/          — LSP tool
├── storage/      — SQLite sessions
├── server/       — HTTP API (local share)
├── sdk/          — Library client API
├── tui/          — Terminal UI (ratatui)
└── cli/          — Command line entry point (clap)
```

## Development

CI runs these three checks on every push and pull request. Run them before
opening a pull request:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Note that clippy is run with `--all-targets`, so test code is linted under
`-D warnings` too. Tests and release builds run on `ubuntu-latest`,
`windows-latest` and `macos-latest`, so platform-specific behaviour needs a
`#[cfg]` branch rather than a Unix-only assumption.

The tree was reformatted with `rustfmt` in a single commit. Exclude it from
blame output:

```bash
git config blame.ignoreRevsFile .git-blame-ignore-revs
```

## Documentation

| Doc | What it is |
|---|---|
| [docs/status.md](docs/status.md) | Living roadmap: open work, deferred items, decision log |
| [docs/FEATURES.md](docs/FEATURES.md) | Feature matrix vs Grok Build, OpenCode, jcode, Claude Code, Codex, Gemini CLI, Pi, Cursor |
| [docs/KNOWHOW.md](docs/KNOWHOW.md) | Hard-won bugs (TUI, tty, silent exits) — read before event-loop changes |
| [docs/benchmarks.md](docs/benchmarks.md) | How to measure startup, RSS, first frame, idle draws |
| [docs/budgets.md](docs/budgets.md) | CI quality budgets (panic, swallow, dependency edges) |
| [docs/comparison.md](docs/comparison.md) | Early jcode snapshot + gap status |
| [packaging/README.md](packaging/README.md) | Homebrew formula and packaging notes |
| [AGENTS.md](AGENTS.md) | Rules for coding agents working in this repo |

### Open plans

Still-open work with acceptance criteria:

| Plan | Status |
|---|---|
| [docs/plan-distribution.md](docs/plan-distribution.md) | Implemented; first `v*` release exercises remaining criteria |
| [docs/plan-oauth.md](docs/plan-oauth.md) | Partially shipped — [docs/auth.md](docs/auth.md) |
| [docs/plan-performance.md](docs/plan-performance.md) | Mostly done; residual stats / CI ceilings |
| [docs/plan-latency-competitors.md](docs/plan-latency-competitors.md) | P0+P1 done |
| [docs/plan-features-improvements.md](docs/plan-features-improvements.md) | FEATURES matrix + TUI discoverability |
| [docs/plan-memory.md](docs/plan-memory.md) | Shipped v1+v2 (retain, sync, code RAG, ONNX opt) |

### Archived phases

Completed or dropped numbered phases (1, 4, 7, 8, 9) live under
[docs/archive/](docs/archive/README.md). Do not treat them as open work.

Post-launch deferred: **ACP** (Agent Client Protocol) and `web` surface — see
decision log in [docs/status.md](docs/status.md).

## License

MIT — [whycorporation/whycode](https://github.com/whycorporation/whycode)
