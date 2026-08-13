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

macOS and Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/whycorporation/whycode/main/scripts/install.sh | sh
```

Windows:

```powershell
irm https://raw.githubusercontent.com/whycorporation/whycode/main/scripts/install.ps1 | iex
```

Both verify the download against the release `SHA256SUMS`. They do not modify
`PATH`; they print the install directory if it is not already on it.
`WHYCODE_INSTALL_DIR` overrides the location.
`scripts/uninstall.sh` / `uninstall.ps1` remove it — add `--purge` / `-Purge`
to delete config and session data as well.

The binaries are unsigned, so macOS Gatekeeper and Windows SmartScreen will
warn on first run. `whycode upgrade` replaces the running binary with the
newest release (checksum verified) and leaves the existing one in place if
anything fails.

### Homebrew

```bash
brew tap whycorporation/whycode https://github.com/whycorporation/whycode
brew install --HEAD whycode
```

`--HEAD` builds from `main`. After a tagged release the formula can switch to
prebuilt binaries — see [packaging/README.md](packaging/README.md).

### From source

```bash
git clone https://github.com/whycorporation/whycode.git
cd whycode
cargo build --release -p whycode-cli
```

The binary is written to `target/release/whycode`. Optional extras (Unicode
mermaid + extra highlight languages, ~+1.7 MB):

```bash
cargo build --release -p whycode-cli --features full
```

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

## Documentation

| Doc | What it is |
|---|---|
| [docs/guide.md](docs/guide.md) | Usage: CLI, TUI, agents, tools, config |
| [docs/auth.md](docs/auth.md) | API keys, OAuth, credential import |
| [docs/architecture.md](docs/architecture.md) | Crate map and layering |
| [docs/roadmap.md](docs/roadmap.md) | Current focus and deferred work |
| [docs/KNOWHOW.md](docs/KNOWHOW.md) | Hard-won bugs (TUI, tty, silent exits) |
| [docs/benchmarks.md](docs/benchmarks.md) | How to measure startup, RSS, idle draws |
| [docs/budgets.md](docs/budgets.md) | CI quality budgets |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Dev setup and required checks |
| [SECURITY.md](SECURITY.md) | Vulnerability reporting |
| [AGENTS.md](AGENTS.md) | Rules for coding agents in this repo |
| [docs/archive/](docs/archive/README.md) | Completed plans (not open work) |

## License

MIT — [whycorporation/whycode](https://github.com/whycorporation/whycode)
