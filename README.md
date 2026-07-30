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

## Installation

```bash
git clone https://github.com/whycorporation/whycode.git
cd whycode
cargo build --release
```

The binary is written to `target/release/whycode`.

## Quick start

```bash
export ANTHROPIC_API_KEY="sk-ant-..."

# Interactive session in a project
whycode -d ./my-project

# One-shot, non-interactive
whycode generate "Explain the error handling in main.rs" -d ./my-project

# Interactive, seeded with an opening prompt
whycode run "Where is the retry logic?" -d ./my-project

# A different provider and model
whycode -P openai -m gpt-4o generate "Refactor this module"
```

## Command line

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

The prompt is a positional argument rather than a flag — `whycode generate
"<prompt>"`, not `whycode -p "<prompt>"`. `run` and `generate` also accept
`-t, --max-turns <N>` (default 25).

## Interactive session

The default interface is a full-screen TUI with live streaming of text, tool
calls and thinking. `--plain` switches to a line-based stdin REPL.

| Key | Action |
|---|---|
| `Tab` | Cycle primary agents (`build` ↔ `plan`), when idle |
| `Esc` | Cancel the in-flight turn, keeping partial output; dismiss a dialog |
| `Ctrl+C` / `Ctrl+Q` | Cancel the turn and quit |

Two input prefixes are available in both interfaces:

- `!ls -la` runs a shell command and attaches its output to the conversation.
- `@src/main.rs` inlines a file's contents into the prompt.

### Slash commands

Available in both the TUI and the `--plain` REPL:

| Command | Description |
|---|---|
| `/help` `/h` | Show help |
| `/exit` `/quit` `/q` | Exit |
| `/new` `/clear` | Start a new session |
| `/init` | Create or update `AGENTS.md` |
| `/undo` | Undo the last turn and restore files via git |
| `/redo` | Redo the last undone turn |
| `/share` `/export` | Export the session and print its local share URL |
| `/compact` `/summarize` | Compact the conversation context |
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
| `/sessions` `/resume` `/continue` | List stored sessions |
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

Primary agents run the main conversation and are switched with `Tab` or
`/agent`. Subagents are spawned by the `task` tool for scoped work and report
back to the primary agent.

| Agent | Mode | Role |
|---|---|---|
| `build` | primary | Full-access coding (default) |
| `plan` | primary | Read-only planning |
| `general` | subagent | Multi-step tasks |
| `explore` | subagent | Fast read-only codebase search |
| `scout` | subagent | External documentation and dependency research |

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

[mcp_servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]

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

29 TUI themes are available, including Catppuccin, Tokyo Night, Rose Pine,
Gruvbox, Nord, Dracula, Solarized and Ayu variants. Set one with `[tui] theme`;
`/themes` in the `--plain` REPL prints the full list of valid names.

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
├── core/      — Shared types, config, error handling
├── llm/       — LLM providers
├── tools/     — Tool system and built-ins
├── session/   — Conversation, compaction, undo/redo
├── agent/     — Agent loop, subagents, AGENTS.md
├── skill/     — Skill registry and plugins
├── mcp/       — MCP client
├── lsp/       — LSP tool
├── storage/   — SQLite sessions
├── server/    — HTTP API
├── tui/       — Terminal UI (ratatui)
└── cli/       — Command line entry point (clap)
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

## License

MIT — [whycorporation/whycode](https://github.com/whycorporation/whycode)
