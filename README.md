# WhyCodes

A fast, provider-independent coding agent for the terminal, written in Rust.
It reads, writes and edits files, runs commands, searches the workspace and
drives an LLM through multi-turn tool use — in a full-screen TUI or as a
machine-readable one-shot CLI.

- Ship and run one native binary with no required runtime dependencies. Search
  is in-process (`ripgrep` is not required); optional OS integrations such as
  Linux bubblewrap strengthen sandboxing when available.
- Use the same workflow on Linux, macOS and Windows. Linux runs on every CI
  change; optional Windows and macOS jobs can be enabled through the multi-OS
  CI matrix.
- Choose Anthropic, OpenAI, Google, GitHub Copilot, Groq, xAI, DeepSeek,
  Ollama, OpenRouter, Mistral, Together, or any OpenAI-compatible endpoint,
  using an API key or subscription login (`whycodes auth login`).
- Bring project instructions from `AGENTS.md` and connect existing MCP tools
  and language servers over LSP.
- Resume previous work with persisted sessions and cross-session memory
  (`MEMORY.md`, semantic recall, and an optional code index).

WhyCodes focuses on a small native footprint, an idle-efficient TUI, and a
provider-independent agent workflow.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/whycorporation/whycodes/main/scripts/install.sh | sh
```

```powershell
irm https://raw.githubusercontent.com/whycorporation/whycodes/main/scripts/install.ps1 | iex
```

```bash
# Homebrew
brew tap whycorporation/whycodes https://github.com/whycorporation/whycodes
brew install whycodes

# From source
cargo build --release -p whycodes-cli
```

Update with `whycodes upgrade`. The install scripts verify release artifacts
against the published `SHA256SUMS`. For downloadable binaries, installation
details and uninstall instructions, see
[docs/packaging.md](docs/packaging.md).

Shell completions are generated from the live CLI:

```bash
eval "$(whycodes completions zsh)"    # ~/.zshrc
eval "$(whycodes completions bash)"   # ~/.bashrc
whycodes completions fish > ~/.config/fish/completions/whycodes.fish
```

## Quick start

```bash
export ANTHROPIC_API_KEY="sk-ant-..."

whycodes -d ./my-project                          # interactive TUI
whycodes generate "Explain main.rs" -d ./my-project
whycodes generate "Summarize the last commit" --format json
whycodes --continue                               # resume last session
whycodes -P openai -m gpt-4o generate "Refactor this module"
```

Or sign in with a subscription instead of an API key:

```bash
whycodes auth login anthropic              # Claude Pro/Max
whycodes auth login openai                 # ChatGPT Plus/Pro
whycodes auth login github-copilot
whycodes auth login google                 # Gemini
whycodes auth login google-antigravity     # Antigravity (Gemini 3, Claude, GPT-OSS)
whycodes auth login xai                    # SuperGrok / X Premium
```

Full CLI, TUI keys, slash commands, agents, tools and configuration:
**[docs/guide.md](docs/guide.md)**.

## Performance

Idle TUI, Linux x86_64, 2026-08-26 (see
[docs/benchmarks.md](docs/benchmarks.md) for method and machine):

| | whycodes |
|---|---|
| 1 session PSS | **11.6 MB** |
| 10 sessions PSS | **30.7 MB** (~2.1 MB each extra) |
| `--version` | **1.5 ms** |
| First frame (harness, in-proc) | **18 ms** |
| Idle redraws (harness, 3 s) | **0.0 /s** |

The TUI paints only when something changed. Idle target is **0 redraws/s**,
not a frames-per-second race.

## Coverage

Workspace line coverage **85.58%** (Linux x86_64, 2026-08-21). CI fails
below **82%**, with twelve foundational crates held at **100%** production-code
line coverage. See [docs/coverage.md](docs/coverage.md) for the current
breakdown, measurement command and enforced floors.

## Documentation

| Doc | What it is |
|---|---|
| [docs/guide.md](docs/guide.md) | Usage: CLI, TUI, agents, tools, config, SDK |
| [docs/auth.md](docs/auth.md) | API keys, OAuth, credential import |
| [docs/architecture.md](docs/architecture.md) | Crate map and layering |
| [docs/roadmap.md](docs/roadmap.md) | Current focus and deferred work |
| [docs/knowhow.md](docs/knowhow.md) | Hard-won bugs (TUI, tty, silent exits) |
| [docs/tui-term-matrix.md](docs/tui-term-matrix.md) | Manual TUI pass on Alacritty / Kitty / VTE |
| [docs/benchmarks.md](docs/benchmarks.md) | How to measure startup, RSS, idle draws |
| [docs/coverage.md](docs/coverage.md) | How to measure line coverage |
| [docs/budgets.md](docs/budgets.md) | CI quality budgets |
| [docs/packaging.md](docs/packaging.md) | Homebrew, installers, release artifacts |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Dev setup and required checks |
| [SECURITY.md](SECURITY.md) | Vulnerability reporting |
| [AGENTS.md](AGENTS.md) | Rules for coding agents in this repo |
| [docs/archive/](docs/archive/README.md) | Completed plans (not open work) |

## License

MIT — [why.codes](https://why.codes) · [whycorporation/whycodes](https://github.com/whycorporation/whycodes)
