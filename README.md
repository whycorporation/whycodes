# Whycode

An AI-powered coding agent built in Rust — a high-performance, native implementation targeting **OpenCode parity**.

## Features

- 🦀 **Pure Rust** — single binary, zero runtime dependencies
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
./target/release/whycode -p "Explain the error handling in main.rs" -d ./my-project

# Use OpenAI
./target/release/whycode -P openai -m gpt-4o -p "Refactor this module"
```

## Usage

```
Usage: whycode [OPTIONS] [COMMAND]

Commands:
  run       Start interactive mode (default)
  generate  Non-interactive single prompt
  serve     Start HTTP API server
  mcp       MCP server management
  provider  Provider management
  model     Model management
  agent     List/show agents
  config    Configuration
  session   Session list/view/delete/share
  stats     Usage statistics
  debug     Debug info
  upgrade   Self-update
  github    GitHub helpers
  pr        Create a pull request
  ...

Options:
  -p, --prompt <PROMPT>        (via generate or run prompt)
  -d, --dir <DIR>              Project directory [default: .]
  -P, --provider <PROVIDER>    LLM provider [default: anthropic]
  -m, --model <MODEL>          Model name
  -a, --agent <AGENT>          Agent name [default: build]
  -t, --max-turns <TURNS>      Max conversation turns [default: 25]
```

## Interactive slash commands (OpenCode-compatible)

| Command | Description |
|---|---|
| `/help` | Show help |
| `/exit` `/quit` `/q` | Exit |
| `/new` `/clear` | New session |
| `/init` | Create/update `AGENTS.md` |
| `/undo` | Undo last turn + restore files (git) |
| `/redo` | Redo |
| `/share` `/export` | Export + local share URL (`/s/:id`) |
| `/unshare` | Delete local share files |
| `/compact` `/summarize` | Compact context |
| `/sessions` | List sessions |
| `/models [provider/id]` | List or switch model |
| `/agent [name]` | List or switch agent (`build`/`plan`) |
| `/connect` | Provider setup help |
| `/thinking` | Toggle thinking display |
| `/tools` | List tools for current agent |
| `/info` | Session details |

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

Configuration is stored at the platform config dir (`whycode/config.toml`):

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

`allow` runs immediately, `ask` prompts (stdin in `--plain`, auto-approve in TUI unless `WHYCODE_AUTO_DENY=1`), `deny` blocks.

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
| `grep` | `grep` | Regex search |
| `glob` | `glob` | Find files by pattern |
| `list` | `list` | List directory contents |
| `task` | `task` | Spawn subagent (`general`/`explore`/`scout`) |
| `todowrite` / `todoread` | `todowrite` | Task list management |
| `webfetch` | `webfetch` | Fetch web pages |
| `websearch` | `websearch` | Web search |
| `question` | `question` | Ask the user a question |
| `skill` | `skill` | Load a skill |
| `lsp` | `lsp` | Language server intelligence |

## License

MIT — [whycorporation/whycode](https://github.com/whycorporation/whycode)
