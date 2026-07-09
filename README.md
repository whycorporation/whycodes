# Whycode

An AI-powered coding agent built in Rust — a high-performance, native implementation inspired by OpenCode.

## Features

- 🦀 **Pure Rust** — single 7.4MB binary, zero runtime dependencies
- 🤖 **AI Coding Agent** — reads, writes, edits, searches, and executes code
- 🔌 **Multi-Provider** — Anthropic (Claude), OpenAI, Google (Gemini), and OpenAI-compatible APIs
- 🔧 **8 Built-in Tools** — read, write, edit, grep, glob, shell, webfetch, websearch
- 🏗️ **GitHub Integration** — create issues, list PRs, merge branches (via tools)
- 📡 **Streaming** — real-time text, tool use, and thinking events
- 💬 **Interactive CLI** — full conversation loop with session management

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
  run     Start interactive mode
  config  Show configuration
  models  List available models
  tools   List available tools
  help    Print this message

Options:
  -p, --prompt <PROMPT>        Single prompt (non-interactive)
  -d, --dir <DIR>              Project directory [default: .]
  -P, --provider <PROVIDER>    LLM provider [default: anthropic]
  -m, --model <MODEL>          Model name
  -t, --max-turns <TURNS>      Max conversation turns [default: 25]
```

## Architecture

```
crates/
├── core/      — Shared types, config, error handling
├── llm/       — LLM providers (Anthropic, OpenAI, Google)
├── tools/     — Tool system (read, write, edit, grep, glob, shell, web, github)
├── session/   — Conversation management, compaction
├── agent/     — Agent loop, tool orchestration, streaming
└── cli/       — Interactive CLI (clap)
```

## Configuration

Configuration is stored at `$XDG_CONFIG_HOME/whycode/config.toml`:

```toml
[providers.anthropic]
name = "anthropic"
api_key = "sk-ant-..."

[providers.openai]
name = "openai"
api_key = "sk-..."

[[agents]]
name = "build"
description = "Default coding agent"
mode = "primary"

[agents.permission]
allow_file_writes = true
allow_network = true
allow_shell = true
```

## Tools

| Tool | Description |
|---|---|
| `read` | Read files with line numbers and pagination |
| `write` | Create/overwrite files |
| `edit` | Find-and-replace in files |
| `grep` | Regex search (ripgrep-backed) |
| `glob` | Find files by pattern |
| `shell` | Execute shell commands (with timeout) |
| `webfetch` | Fetch and parse web pages |
| `websearch` | Web search (SerpAPI or DuckDuckGo) |
| `github_issue` | Create, list, view issues |
| `github_pr` | Create, list, view, merge PRs |

## License

MIT — [whycorporation/whycode](https://github.com/whycorporation/whycode)
