# Whycode

A coding agent for the terminal, written in Rust. It reads, writes and edits
files, runs commands, searches the workspace and drives an LLM through
multi-turn tool use — in a full-screen TUI or as a one-shot CLI.

- Single binary, no runtime dependencies. Search is in-process (`ripgrep` is
  not required).
- Linux, macOS and Windows, all covered in CI.
- Anthropic, OpenAI, Google, GitHub Copilot, Groq, xAI, DeepSeek, Ollama,
  OpenRouter, Mistral, Together, and any OpenAI-compatible endpoint. API keys
  or subscription login (`whycode auth login`).
- Project `AGENTS.md`, MCP servers, and language servers over LSP.
- Cross-session memory (`MEMORY.md`, semantic recall, optional code index).

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/whycorporation/whycode/main/scripts/install.sh | sh
```

```powershell
irm https://raw.githubusercontent.com/whycorporation/whycode/main/scripts/install.ps1 | iex
```

```bash
# Homebrew
brew tap whycorporation/whycode https://github.com/whycorporation/whycode
brew install whycode

# From source
cargo build --release -p whycode-cli
```

Update with `whycode upgrade`. Notes and uninstall: [packaging/README.md](packaging/README.md).

## Quick start

```bash
export ANTHROPIC_API_KEY="sk-ant-..."

whycode -d ./my-project                          # interactive TUI
whycode generate "Explain main.rs" -d ./my-project
whycode generate "Summarize the last commit" --format json
whycode --continue                               # resume last session
whycode -P openai -m gpt-4o generate "Refactor this module"
```

Or sign in with a subscription instead of an API key:

```bash
whycode auth login anthropic        # Claude Pro/Max
whycode auth login openai           # ChatGPT Plus/Pro
whycode auth login github-copilot
whycode auth login google           # Gemini
```

Full CLI, TUI keys, slash commands, agents, tools and configuration:
**[docs/guide.md](docs/guide.md)**.

## Performance

Idle TUI, Linux x86_64, 2026-08-13 (see [docs/benchmarks.md](docs/benchmarks.md)
for method and machine):

| | whycode |
|---|---|
| 1 session PSS | **6.7 MB** |
| 10 sessions PSS | **26.5 MB** (~2.2 MB each extra) |
| `--version` | **1.2 ms** |

The TUI paints only when something changed. Idle target is **~0 redraws/s**,
not a frames-per-second race.

## Documentation

| Doc | What it is |
|---|---|
| [docs/guide.md](docs/guide.md) | Usage: CLI, TUI, agents, tools, config |
| [docs/auth.md](docs/auth.md) | API keys, OAuth, credential import |
| [docs/architecture.md](docs/architecture.md) | Crate map and layering |
| [docs/roadmap.md](docs/roadmap.md) | Current focus and deferred work |
| [docs/knowhow.md](docs/knowhow.md) | Hard-won bugs (TUI, tty, silent exits) |
| [docs/benchmarks.md](docs/benchmarks.md) | How to measure startup, RSS, idle draws |
| [docs/budgets.md](docs/budgets.md) | CI quality budgets |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Dev setup and required checks |
| [SECURITY.md](SECURITY.md) | Vulnerability reporting |
| [AGENTS.md](AGENTS.md) | Rules for coding agents in this repo |
| [docs/archive/](docs/archive/README.md) | Completed plans (not open work) |

## License

MIT — [whycorporation/whycode](https://github.com/whycorporation/whycode)
