# Whycode

An AI-powered coding agent built in Rust — a high-performance, native implementation targeting **OpenCode parity**.

## Features

- 🦀 **Pure Rust** — single binary, no runtime dependencies (search is in-process; no `ripgrep`/`grep` on `PATH` required)
- 🖥 **Cross-platform** — Linux, macOS and Windows, all built and tested in CI
- 🤖 **AI Coding Agent** — reads, writes, edits, searches, and executes code
- 🔌 **Multi-Provider** — Anthropic, OpenAI, Google, Groq, xAI, DeepSeek, Ollama, OpenRouter, Mistral, Together, custom OpenAI-compatible
- 🔧 **OpenCode-aligned tools** — `bash`, `read`, `write`, `edit`, `apply_patch`, `grep`, `glob`, `list`, `task`, `todowrite`/`todoread`, `webfetch`, `websearch`, `question`, `skill`, `lsp`, plus git/github helpers
- 🧭 **Agents** — primary `build` / `plan` (switch with `/agent`), subagents `general` / `explore` / `scout` via `task`
- 📡 **Streaming** — real-time text, tool use, and thinking events
- 💻 **Full-screen TUI** — default interactive UI with live streaming (use `--plain` for readline REPL)
- 💬 **Slash commands** — OpenCode-style `/init` `/undo` `/agent` `/models` … + custom commands
- ↹ **Tab agent switch** — cycle primary agents (`build` ↔ `plan`)
- ⏹ **Esc cancel** — abort in-flight generation (partial output kept)
- 🔗 **Local share links** — `/share` → `http://localhost:3030/s/<id>` via `whycode serve`
- 🔐 **Permissions** — OpenCode `allow` / `ask` / `deny` with TUI y/n dialog
- 📜 **AGENTS.md** — `/init` and automatic project instructions injection
- ↩ **Undo/Redo** — conversation + git-backed file restore
- 🔌 **MCP** — config persistence + auto-bind tools as `{server}_{tool}`
- 📝 **Custom commands** — `.whycode/commands/*.md` (also `.opencode/commands/`)
- 🌐 **HTTP API** — `whycode serve`

## Quick Start

```bash
# Build from source
git clone https://github.com/whycorporation/whycode.git
cd whycode
cargo build --release

# Run with Anthropic
export ANTHROPIC_API_KEY="sk-ant-..."
./target/release/whycode -d ./my-project

# Single prompt (non-interactive)
./target/release/whycode generate "Explain the error handling in main.rs" -d ./my-project

# Interactive session seeded with an opening prompt
./target/release/whycode run "Where is the retry logic?" -d ./my-project

# Use OpenAI
./target/release/whycode -P openai -m gpt-4o generate "Refactor this module"
```

## Usage

```
Usage: whycode [OPTIONS] [COMMAND]

Commands:
  run       Start an interactive session (default)
  generate  Generate code from a prompt (non-interactive)
  acp       Agent Control Protocol (automated mode)
  pr        Create a pull request from current changes
  github    GitHub operations
  serve     Start API server
  web       Open web UI
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
      --plain                Use plain stdin REPL instead of the full-screen TUI
```

The prompt is a positional argument, not a flag: `whycode generate "<prompt>"`
and `whycode run "<prompt>"`. `run` and `generate` additionally accept
`-t, --max-turns <N>` (default 25).

## Interactive slash commands (OpenCode-compatible)

Available in both the TUI and the `--plain` REPL:

| Command | Description |
|---|---|
| `/help` `/h` | Show help |
| `/exit` `/quit` `/q` | Exit |
| `/new` `/clear` | New session |
| `/init` | Create/update `AGENTS.md` |
| `/undo` | Undo last turn + restore files (git) |
| `/redo` | Redo |
| `/share` `/export` | Export + local share URL (`/s/:id`) |
| `/compact` `/summarize` | Compact context |
| `/models [provider/id]` | List or switch model |
| `/agent [name]` | List or switch agent (`build`/`plan`) |
| `/connect` | Provider setup help |
| `/tools` | List tools for current agent |
| `/info` `/details` | Session details |

TUI only:

| Command | Description |
|---|---|
| `/unshare` | Delete local share files |

`--plain` REPL only:

| Command | Description |
|---|---|
| `/sessions` `/resume` `/continue` | List sessions |
| `/thinking` | Toggle thinking display |
| `/themes` | Show TUI theme names |

Also:

- `!ls` — run shell, attach output to chat
- `@src/main.rs` — include file contents in the prompt

## Architecture

```
crates/
├── core/      — Shared types, config, error handling
├── llm/       — LLM providers
├── tools/     — Tool system (OpenCode-aligned built-ins)
├── session/   — Conversation, compaction, undo/redo
├── agent/     — Agent loop, subagents, AGENTS.md
├── skill/     — Skill registry
├── mcp/       — MCP client
├── lsp/       — LSP tool
├── storage/   — SQLite sessions
├── server/    — HTTP API
├── tui/       — Terminal UI (ratatui)
└── cli/       — Interactive CLI (clap)
```

## Agents (OpenCode mapping)

| Agent | Mode | Role |
|---|---|---|
| `build` | primary | Full-access coding (default) |
| `plan` | primary | Read-only planning |
| `general` | subagent | Multi-step tasks (via `task`) |
| `explore` | subagent | Fast read-only codebase search |
| `scout` | subagent | External docs / dependency research |

## Configuration

Configuration lives in the platform config directory as `config.toml`. Run
`whycode debug` to print the exact path for your machine.

```toml
[providers.anthropic]
name = "anthropic"
api_key = "sk-ant-..."

[providers.openai]
name = "openai"
api_key = "sk-..."

[mcp_servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]

[[agents]]
name = "build"
description = "Default coding agent"
mode = "primary"
```

Project instructions: commit an `AGENTS.md` in the repo root (create with `/init`).

### Permissions (OpenCode-style)

```toml
[permission]
bash = "ask"
edit = "allow"
webfetch = "allow"
"mymcp_*" = "deny"
```

`allow` runs immediately, `deny` blocks, and `ask` prompts — a y/n dialog in the
TUI, a stdin prompt in `--plain`. The prompt can be overridden:

| Condition | Result for `ask` |
|---|---|
| `WHYCODE_AUTO_APPROVE=1` | auto-allow |
| `WHYCODE_AUTO_DENY=1` | auto-deny |
| stdin is not a terminal (piped/CI) | auto-deny |

### Custom commands

```bash
mkdir -p .whycode/commands
```

`.whycode/commands/test.md`:

```markdown
---
description: Run tests
agent: build
---
Run the full test suite and summarize failures.
Focus on $ARGUMENTS.
```

Then run `/test` in the TUI or plain REPL.

## Tools

| Tool | OpenCode name | Description |
|---|---|---|
| `bash` / `shell` | `bash` | Execute shell commands |
| `read` | `read` | Read files with line numbers |
| `write` | `write` | Create/overwrite files |
| `edit` | `edit` | Find-and-replace in files |
| `apply_patch` | `apply_patch` | Apply multi-file patches |
| `grep` | `grep` | Regex search (in-process, no external binary) |
| `glob` | `glob` | Find files by pattern |
| `list` | `list` | List directory contents |
| `task` | `task` | Spawn subagent (`general`/`explore`/`scout`) |
| `todowrite` / `todoread` | `todowrite` | Task list management |
| `webfetch` | `webfetch` | Fetch web pages |
| `websearch` | `websearch` | Web search |
| `question` | `question` | Ask the user a question |
| `skill` | `skill` | Load a skill |
| `lsp` | `lsp` | Language server intelligence |

## Development

CI runs these three checks on every push and pull request; run them locally
before opening a PR:

```bash
cargo fmt --all --check                              # formatting is enforced
cargo clippy --workspace --all-targets -- -D warnings # includes test targets
cargo test --workspace
```

Tests and release builds run on `ubuntu-latest`, `windows-latest` and
`macos-latest`, so platform-specific code needs a `#[cfg]` branch rather than a
Unix-only assumption.

The repository has been reformatted with `rustfmt` in a single commit. Skip it
in `git blame`:

```bash
git config blame.ignoreRevsFile .git-blame-ignore-revs
```

## License

MIT — [whycorporation/whycode](https://github.com/whycorporation/whycode)
