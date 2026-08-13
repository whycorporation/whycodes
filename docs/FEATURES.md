# Feature matrix

Feature comparison of terminal coding agents.

**Terminal / harness:** whycode · Grok Build · OpenCode · jcode · Codex CLI · Gemini CLI · Pi  

**Product surface:** Claude Code · Cursor

Last updated: **2026-08-13** (latency P2: first-token race + text-only semantic cache).  
Sources are listed at the end of the file. Cells are at “yes / partial / no” granularity; not every minor release is re-verified.

### Legend

| | |
|:---:|---|
| ✅ | Yes / production |
| ⚠️ | Partial, skeleton, or limited |
| ❌ | No / roadmap |
| ★ | Notable strength in this area |

† whycode ACP: deliberately **post-product** (`docs/status.md`, 2026-08-04).  
‡ Gemini CLI: **Antigravity CLI** migration announced for free / Google One users (2026-06-18); the matrix still follows the Gemini CLI documentation.  
§ whycode OAuth: login/store/refresh for `anthropic`, `openai`, `github-copilot`, `google` (`whycode auth login` or in-TUI `/connect`); API-call routing live for all four (openai → Codex backend, google → Code Assist). Credential import: `whycode auth import` — consent-based, per-path persisted, symlink-refusing, read-only ([auth.md](auth.md), [plan-oauth](plan-oauth.md)).

---

## Product summary

| | whycode | Grok Build | OpenCode | jcode | Claude Code | Codex CLI | Gemini CLI | Pi | Cursor |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| **Type** | Terminal agent | Terminal agent | Multi-surface | Agent harness | First-party product agent | Terminal agent | Terminal agent | Minimal harness | AI IDE + agent |
| **Surface** | TUI, CLI | TUI, CLI, ACP | TUI, desktop, IDE | TUI, serve | TUI, web, desktop, IDE, CI | TUI, CLI, IDE, app, cloud | TUI, CLI, IDE companion | TUI, CLI, SDK/RPC | IDE, CLI, cloud agents |
| **Language** | Rust | Rust | TypeScript | Rust | Closed (native binary) | Rust | TypeScript | TypeScript | Closed (VS Code based) |
| **License** | MIT | Apache-2.0 | MIT | MIT | Proprietary | Apache-2.0 | Apache-2.0 | MIT | Proprietary |
| **Auth** | API key + OAuth | xAI login + API | API + OAuth | API + OAuth | Claude sub / API | ChatGPT plan / API | Google OAuth / API / Vertex | BYOK API + `/login` | Cursor account |
| **Model** | Multi-provider | Grok + custom | 75+ provider | Multi + OAuth | Claude (+ limited) | OpenAI / Codex models | Gemini 3 (1M ctx) | Multi BYOK | Claude / GPT / Gemini / Composer |
| **Open source** | Yes | Yes | Yes | Yes | No | Yes | Yes | Yes | No |

| Product | Position |
|---|---|
| **whycode** | Shell safety, latency stack, mouse TUI, Windows CI |
| **Grok Build** | Rich TUI; skills / plugins / hooks; ACP |
| **OpenCode** | Broad OSS ecosystem; TUI + desktop + IDE |
| **jcode** | Low RAM / boot; swarm; semantic memory |
| **Claude Code** | Anthropic product surface (web, desktop, IDE, CI) |
| **Codex CLI** | OpenAI terminal agent; OS sandbox; AGENTS.md; cloud Codex |
| **Gemini CLI** | Google terminal agent; free tier; Search grounding; plan mode |
| **Pi** | Minimal harness; extensions/skills; multi-provider; container sandbox |
| **Cursor** | IDE-first Agent + Tab; cloud agents; multi-model |

---

## 1. Platform, distribution, runtime

Short names: **why** · **Grok** · **OC** OpenCode · **jc** jcode · **CC** Claude · **Codex** · **Gem** Gemini · **Pi** · **Cur** Cursor

| Feature | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Single binary / native CLI | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ Rust | ⚠️ npm/Node | ⚠️ npm; bin build exists | ❌ IDE; CLI separate |
| Install (curl / npm / brew) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ curl+npm+brew | ✅ npm+brew | ✅ npm+curl | ✅ app + `@cursor/cli` |
| Self-update | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ npm channel | ✅ `pi update` | ✅ app update |
| Homebrew / package | ⚠️ HEAD | ⚠️ | ✅ | ✅ | ✅ | ✅ cask | ✅ | ⚠️ npm | ✅ |
| Linux / macOS | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Windows native | ✅★ | ⚠️ | ⚠️ | ✅ | ✅ | ✅ | ⚠️ | ✅ docs | ✅ |
| Cross-platform CI (full) | ✅★ | ⚠️ | ⚠️ | ✅ | ✅ | ⚠️ | ✅ CI badges | ✅ | n/a product |
| No runtime dependency | ✅ | ✅ | ⚠️ | ✅ | ✅ | ✅ | ❌ Node | ❌ Node | n/a |
| In-process search (no rg) | ✅ +live index | ⚠️ | ⚠️ | ✅ | ⚠️ | ⚠️ | ⚠️ ships rg helper | ⚠️ | n/a IDE search |
| Open source | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ❌ |

---

## 2. Surfaces

| Feature | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Fullscreen TUI | ✅ | ✅★ | ✅★ | ✅★ | ✅ | ✅ | ✅ | ✅ | ❌ (IDE UI) |
| Headless / plain | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ `-p` + json | ✅ print/RPC/json | ✅ Cursor CLI |
| Mouse-interactive TUI | ✅★ | ✅★ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | n/a |
| Desktop app | ❌ | ❌ | ✅ | ❌ | ✅ | ✅ `codex app` | ❌ | ❌ | ✅★ |
| IDE extension | ❌ | ⚠️ ACP | ✅ | ❌ | ✅ | ✅ VS Code/Cursor/Windsurf | ✅ companion | ❌ | n/a (IDE) |
| Web UI | ⚠️ stub | ❌ | ⚠️ | ❌ | ✅ | ✅ chatgpt.com/codex | ❌ | ❌ | ✅ agents web |
| ACP | ⚠️ stub† | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| HTTP / serve / attach | ✅ local share | ⚠️ | ✅ | ✅ serve | ⚠️ | ⚠️ mcp-server | ⚠️ | ✅ RPC/SDK | ⚠️ cloud |
| Streaming JSON / CI | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ stream-json | ✅ json events | ✅ headless |

---

## 3. LLM providers & auth

| Feature | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Multi-provider (API) | ✅ | ✅ custom | ✅★ 75+ | ✅★ | ⚠️ | ❌ OpenAI family | ❌ Gemini family | ✅★ unified API | ✅ multi |
| Anthropic | ✅ | ⚠️ | ✅ | ✅ OAuth | ✅ | ❌ | ❌ | ✅ | ✅ |
| OpenAI | ✅ | ⚠️ | ✅ | ✅ OAuth | ⚠️ | ✅★ | ❌ | ✅ | ✅ |
| Google / Gemini | ✅ | ⚠️ | ✅ | ✅ OAuth | ⚠️ | ❌ | ✅★ | ✅ | ✅ |
| xAI / Grok | ✅ | ✅★ | ✅ | ✅ | ❌ | ❌ | ❌ | ⚠️ | ✅ |
| OpenRouter / Ollama / local | ✅ | ⚠️ | ✅ | ✅ | ❌ | ⚠️ | ⚠️ | ✅ + llama.cpp | ⚠️ |
| OpenAI-compatible custom | ✅ | ✅ | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ | ✅ | ⚠️ |
| OAuth / subscription login | ✅ 4 providers § | ✅ xAI | ✅ | ✅★ | ✅ | ✅ ChatGPT | ✅ Google | ✅ `/login` | ✅ Cursor |
| Credential import | ✅ 4 CLIs, consent § | ⚠️ | ⚠️ | ✅★ | n/a | ⚠️ | ⚠️ | ⚠️ | n/a |

---

## 4. Agent system

| Feature | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Full-access agent | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ Agent |
| Plan / read-only mode | ✅ Ctrl+T | ✅ | ✅ Tab | ✅ | ✅ | ⚠️ approval modes | ✅ plan mode | ⚠️ | ✅ plan |
| Subagent | ✅ `task` | ✅ | ✅ | ✅ | ✅ | ✅ headless spawn | ✅ complete_task | ⚠️ ext | ✅ subagents |
| Built-in explore / scout | ✅ | ⚠️ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ❌ minimal | ⚠️ |
| Custom agent definitions | ✅ config | ✅ | ✅ | ✅ | ✅ | ⚠️ AGENTS.md | ⚠️ GEMINI.md | ✅ extensions | ✅ rules/agents |
| Parallel multi-session | ✅ Ctrl+O/N/Tab | ⚠️ | ✅ | ✅★ | ✅ | ⚠️ | ⚠️ | ⚠️ | ✅★ cloud fan-out |
| Swarm + conflict notify | ✅ worktrees + claims | ❌ | ❌ | ✅★ | ⚠️ teams | ❌ | ❌ | ❌ | ⚠️ |
| Max turns / loop guard | ✅★ doom-loop | ✅ | ✅ | ⚠️ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |

---

## 5. Tools

| Feature | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| read / write / edit | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| apply_patch / multi-hunk | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ replace | ⚠️ | ✅ |
| grep / glob / list | ✅ | ✅ | ✅ | ✅★ | ✅ | ✅ | ✅ | ⚠️ few built-in | ✅ |
| bash / shell | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ sandboxed | ✅ | ✅ | ✅ |
| websearch / fetch | ✅ | ✅ | ⚠️ | ⚠️ | ✅ | ⚠️ search flag | ✅★ Google Search + fetch | ⚠️ ext/MCP | ✅ |
| git tools | ✅ built-in | ✅ | ⚠️ | ⚠️ | ✅ | ⚠️ shell | ⚠️ shell | ⚠️ shell | ✅ |
| GitHub issue / PR | ✅ | ⚠️ | ⚠️ | ⚠️ | ✅ | ⚠️ | ✅ GH Action | ❌ | ✅ Bugbot/PR |
| todo / plan / question | ✅ | ✅ | ✅ | ⚠️ | ✅ | ⚠️ | ✅ todos+ask_user+plan | ⚠️ | ⚠️ |
| LSP | ✅★ crate | ⚠️ | ✅ | ❌ | ⚠️ | ⚠️ | ⚠️ | ❌ | ✅ IDE |
| Core / minimal tool set | ✅★ profile | ⚠️ | ⚠️ | ⚠️ | ✅ | ⚠️ | ⚠️ | ✅★ few tools | ⚠️ |
| Deferred tool load / ToolSearch | ✅ `tool_search` | ⚠️ | ⚠️ | ⚠️ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| Parallel tools | ✅★ | ✅ | ⚠️ | ⚠️ | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ |
| Browser automation | ❌ | ⚠️ | ⚠️ | ✅★ | ✅ | ⚠️ | ⚠️ | ❌ | ✅★ cloud VM |
| Image / multimodal | ✅ read+@attach | ✅ gen | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ✅ PDF/image | ⚠️ | ✅ |
| Video gen | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ⚠️ MCP (Veo) | ❌ | ❌ |

---

## 6. Security & permissions

| Feature | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| allow / ask / deny | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ approval policy | ✅ confirm mutators | ❌ no built-in | ✅ |
| Permission globs / policy | ✅ tool + bash + path | ✅ | ✅ | ⚠️ | ✅ | ✅ sandbox_mode | ✅ policy engine | ⚠️ container | ⚠️ |
| Multi-ask queue | ✅★ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ❌ | ⚠️ |
| Auto-approve / full-auto | ✅ env | ✅ | ⚠️ | ⚠️ | ✅ | ✅ full access mode | ⚠️ trusted folders | ⚠️ | ✅ |
| Shell risk classification | ✅★ 4-tier | ⚠️ | ⚠️ | ✅ | ⚠️ | ⚠️ sandbox tiers | ⚠️ | ❌ | ⚠️ |
| Catastrophic hard-block | ✅★ | ⚠️ | ❌ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ❌ | ⚠️ |
| OS / container sandbox | ✅★ bwrap | ✅ | ❌ | ⚠️ | ✅ | ✅★ Landlock/seccomp/AppContainer | ✅ sandbox docs | ⚠️ Docker/Gondolin | ⚠️ cloud VM |
| Network allowlist / net-off | ✅ HTTP tools | ⚠️ | ❌ | ⚠️ | ✅ | ✅ net-off default legacy | ⚠️ | ⚠️ container | ⚠️ |
| Hooks pre/post tool | ✅ | ✅★ | ✅★ | ⚠️ | ✅★ | ⚠️ | ⚠️ | ✅ extensions | ✅ hooks |

---

## 7. Extensions: MCP, skills, plugins

| Feature | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| MCP client | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ packages/ext | ✅ |
| MCP HTTP/SSE | ✅ | ⚠️ | ✅ | ❌ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| MCP as server (export) | ✅ `mcp serve` | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ✅ `codex mcp-server` | ❌ | ⚠️ | ⚠️ |
| Skills (SKILL.md) | ✅ | ✅★ | ✅ | ✅ | ✅★ | ✅ `.codex/skills` | ✅ `.gemini/skills` | ✅ | ✅ |
| Plugins / extensions | ✅ plugins.toml | ✅★ | ✅★ | Self-dev★ | ⚠️ | ⚠️ | ✅ extensions | ✅★ TS extensions | ✅ |
| Custom slash / commands | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ custom cmds | ✅ prompt templates | ✅ |
| AGENTS.md / CLAUDE.md / GEMINI.md | ✅ AGENTS | ✅ | ✅ AGENTS | ✅ | ✅ CLAUDE | ✅ AGENTS | ✅ GEMINI.md | ✅ AGENTS | ✅ rules / AGENT.md |
| Self-dev / self-extend | ❌ | ❌ | ❌ | ✅★ | ❌ | ❌ | ❌ | ✅★ self-extensible | ❌ |

---

## 8. Session, memory, context

| Feature | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Persistent session | ✅ SQLite | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ checkpoint | ✅ JSONL sessions | ✅ |
| Resume / list | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ checkpointing | ✅ tree/branch | ✅ |
| Cross-harness resume | ❌ | ⚠️ | ⚠️ | ✅★ | n/a | ⚠️ | ⚠️ | ⚠️ | ❌ |
| Undo / redo | ✅ git | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ branch | ✅ IDE |
| Context compact | ✅ prune+breaker | ✅ | ✅★ | ✅ | ✅ | ⚠️ | ✅ token caching | ✅ compaction | ✅ |
| Semantic / auto memory | ✅ | ✅ | ❌ | ✅★ | ✅ | ✅ memories md | ⚠️ | ❌ | ⚠️ |
| Share / export | ✅ local | ✅ | ✅ | ⚠️ | ✅ | ⚠️ | ⚠️ | ✅ HF share tooling | ⚠️ |
| Usage / cost stats | ✅ `/cost` | ✅ | ⚠️ | ⚠️ | ✅ | ⚠️ | ⚠️ telemetry | ⚠️ | ✅ `/usage` |

---

## 9. TUI / UX

| Feature | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Live stream text + tools | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Thinking / reasoning | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ | ⚠️ | ✅ |
| Markdown + highlight | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ | ✅★ |
| Inline diff | ✅ | ✅★ | ✅ | ✅ | ✅ | ⚠️ | ✅ confirm diff | ⚠️ | ✅★ |
| Mermaid | ✅ | ⚠️ | ❌ | ✅★ | ❌ | ❌ | ❌ | ❌ | ⚠️ |
| Theme system | ✅ 29+JSON | ✅ | ✅★ | ✅ | ⚠️ | ⚠️ | ⚠️ | ✅ themes | ✅ IDE |
| Mouse chrome (stop/scroll) | ✅★ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | n/a |
| `@file` / `!shell` / image | ✅ +fuzzy picker | ✅ | ✅ | ✅ | ✅ | ⚠️ | ✅ @ + ! + image | ⚠️ | ✅ |

---

## 10. Latency / agent loop (whycode focus)

| Feature | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Shared HTTP keep-alive | ✅★ | ✅ | ✅ | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ | n/a |
| Prompt / token cache | ✅★ | ⚠️ | ✅★ | ⚠️ | ✅ | ⚠️ OpenAI cache | ✅ token caching | ⚠️ | ⚠️ |
| Parallel safe tools | ✅★ | ✅ | ⚠️ | ⚠️ | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ |
| Core / minimal tools | ✅★ | ⚠️ | ⚠️ | ⚠️ | ✅ | ⚠️ | ⚠️ | ✅★ | ⚠️ |
| Doom-loop guard | ✅★ | ⚠️ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| Tool result prune | ✅★ | ⚠️ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| Autocompact circuit breaker | ✅★ 3× | ⚠️ | ⚠️ | ⚠️ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| TTFT metric (JSONL) | ✅★ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| First-token race failover | ✅ `model_race` | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| Semantic response cache | ✅ text-only | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |

Details: [plan-latency-competitors.md](plan-latency-competitors.md).

---

## 11. Multi-agent, background, automation

| Feature | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Background tasks UI | ✅ `bg` + toast | ✅ | ⚠️ | ✅ | ✅ | ⚠️ | ⚠️ shell bg | ❌ | ✅★ |
| Scheduled / loop | ✅ `schedule` + `/loop` | ✅ | ❌ | ⚠️ ambient | ✅ | ⚠️ | ⚠️ | ❌ | ⚠️ |
| Cloud / remote session | ❌ | ❌ | ❌ | ⚠️ iOS plan | ✅★ | ✅★ Codex Web | ⚠️ | ❌ | ✅★ cloud agents |
| CI / headless agent | ✅ format | ⚠️ | ⚠️ | ⚠️ | ✅★ | ✅ | ✅ GH Action | ✅ json/RPC | ✅ Cursor CLI |
| Same-dir multi-agent | ✅ `swarm`+worktrees | ❌ | ⚠️ | ✅★ | ⚠️ | ⚠️ | ⚠️ | ❌ | ⚠️ parallel cloud |

---

## 12. Performance (published / measured)

Different machines / dates; orders of magnitude only. jcode README bench table (Linux, 10× PTY) + whycode [benchmarks.md](benchmarks.md).

| Metric | whycode | Grok Build | OpenCode | jcode | Claude Code | Codex CLI | Gemini CLI | Pi | Cursor |
|---|---|---|---|---|---|---|---|---|---|
| Boot / TTFF | **~1.0 ms** `--version` (Linux; Win baseline was ~21 ms) | — | ~1036 ms | **~14 ms** | ~3437 ms | ~883 ms | — | ~591 ms | IDE (order of seconds) |
| 1 session PSS | **~4.1 MB** idle TUI | — | ~372 MB | **~28 MB** (embed off) | ~387 MB | ~140 MB | — | ~144 MB | IDE (hundreds of MB+) |
| 10 session PSS | **~16.8 MB** | — | ~3.2 GB | **~117 MB** | ~2.3 GB | ~335 MB | — | ~833 MB | — |
| Source | whycode benches | — | jcode bench | jcode README | jcode bench | jcode bench | public free-tier docs | jcode bench | product class |

whycode: process startup + criterion hot-path; agent TTFT via JSONL `ttft_ms`. No comparable PSS table published for Grok Build / Gemini CLI (—).

---

## 13. whycode-specific strengths

| Area | Detail |
|---|---|
| **Shell risk gate** | `safe` → `caution` → `destructive` → `catastrophic` (last is never approvable) |
| **Latency stack** | Core tools, prompt cache, parallel reads, doom-loop, fast-route, race, text cache |
| **Mouse TUI** | HitArea: context %, stop, scrollbar, slash hover |
| **LSP crate** | Separate `whycode-lsp` |
| **Windows CI** | Linux + macOS + Windows full suite |
| **Small codebase** | ~50k LOC Rust |
| **OpenCode interop** | Theme JSON, AGENTS.md, `.opencode/commands` |
| **In-process grep** | Works without `rg` on PATH |
| **Local share** | `127.0.0.1` only |

---

## 14. Missing / weak in whycode (vs competitors)

| Gap | Who has it | Note |
|---|---|---|
| Semantic memory | whycode ✅ v2 (retain+RAG+sync+ONNX opt), jcode★, Grok, Claude | [archive/plan-memory](archive/plan-memory.md) |
| Swarm | jcode★ / whycode `swarm` | git worktrees + 3-way merge + claims toast |
| Browser automation | jcode, Claude, Cursor★ | None |
| Desktop / IDE | OpenCode, Claude, Codex app/IDE, **Cursor★** | `web` + `acp` stub |
| Plugin marketplace | Grok, OpenCode, Cursor, Gemini ext, Pi packages | Config hooks ✅ |
| OS sandbox (non-Linux) | Grok, Claude, **Codex★**, Gemini sandbox | Linux bwrap ✅ |
| Cloud agents | Claude, **Codex Web**, **Cursor★** | None |
| Free model tier | **Gemini CLI★** (1k req/day) | None |
| Minimal harness | **Pi★** | Core profile exists |
| Self-dev / self-extend | jcode, **Pi★** | None |
| MCP-as-server export | **Codex★** | None |

---

## 15. Quick “who is what for?”

| Need | Pick |
|---|---|
| Lowest RAM, swarm, memory | **jcode** |
| Claude subscription + web/desktop/IDE/CI | **Claude Code** |
| Model freedom + OSS multi-surface | **OpenCode** |
| Grok + plugins + ACP | **Grok Build** |
| OpenAI + OS sandbox + cloud Codex | **Codex CLI** |
| Free tier + Search + plan mode | **Gemini CLI** |
| Minimal, extension-first harness | **Pi** |
| IDE-first + Tab + cloud agents | **Cursor** |
| Shell safety + latency + mouse TUI + Windows CI | **whycode** |

### Competitor notes (short)

| Product | Note |
|---|---|
| **Codex CLI** | Apache-2.0 Rust; ChatGPT login or API; sandbox (Landlock/seccomp/AppContainer); AGENTS.md; MCP client + `mcp-server`; IDE extension + Codex Web/App. |
| **Gemini CLI** | Apache-2.0 TypeScript; Google OAuth free tier; built-in Search/fetch/plan/todos; sandbox + policy; GEMINI.md; stream-json. Antigravity CLI migration announced (‡). |
| **Pi** | MIT TypeScript monorepo (Earendil); multi-provider; few built-in tools + TS extensions/skills/themes; session tree + compaction; **no built-in FS/net permission** — container recommended. |
| **Cursor** | Proprietary IDE; Agent + Tab + multi-model; Cloud Agents (browser/desktop VM); MCP/skills/hooks; separate headless CLI. |

---

## 16. whycode feature inventory (standalone)

### CLI

| Command | Status |
|---|---|
| `run` / default TUI | ✅ |
| `generate` (one-shot) | ✅ |
| `--format text\|json\|stream-json` (CI / NDJSON) | ✅ |
| `--continue` / `--resume` | ✅ |
| `acp` | ⚠️ stub (post-product) |
| `pr` / `github` | ✅ |
| `serve` (API + local share) | ✅ |
| `web` | ⚠️ stub |
| `mcp` / `provider` / `model` / `agent` / `config` / `session` | ✅ |
| `stats` / `debug` / `upgrade` | ✅ |

### Slash commands (TUI)

`/help`, `/exit`, `/new`, `/init`, `/undo`, `/redo`, `/share`, `/unshare`, `/compact`, `/sessions`, `/resume`, `/continue`, `/rename`, `/models`, `/agent`, `/connect`/`/login`, `/tools`, `/info`, `/theme`/`/themes` (picker + apply by name) — custom: `.whycode/commands/*.md` and `.opencode/commands`. Extra plain: `/thinking` (REPL). Command mode: `:` → `:theme`, `:q`, …

### Built-in tools

**Core profile (default, sent to the LLM schema):** `read`, `write`, `edit`, `apply_patch`, `grep`, `glob`, `list`, `bash` (alias `shell`, `background: true`), `bg`, `schedule`, `swarm`, `todowrite` (alias `todo`), `todoread`, `task`.

**Full profile:** + `git_status`/`git_diff`/`git_log`/`git_blame`/`git_commit`, `github_issue`/`github_pr`, `webfetch`, `websearch`, `plan`, `question`, `skill`, `lsp`, `code_mode`, `external_directory`, `truncate`, MCP `{server}_{tool}`.

### Agents

| Agent | Mode |
|---|---|
| `build` | primary, full access (default); soft intent posture |
| `plan` | primary, read-only planning (`Ctrl+T`) |
| `ask` | primary, read-only Q&A / explain (`Ctrl+T`) |
| `general` | subagent (`task`) |
| `explore` | subagent, read-only search |
| `scout` | subagent, docs/deps research |

**Intent layer:** hard mode (ask/plan tool denylist) + build prompt protocol + zero-LLM heuristic (`session.intent_guidance = auto|off|always`). TUI: `[Q]`/`chg`/`plan` badge + warning toast (mode mismatch). Tool auth: mutator in question/plan turn → Confirm; read-only shell unrestricted.

### Config (latency + security)

```toml
[session]
tool_profile = "core"     # or "full"
prompt_cache = "auto"     # or "none"
model_fast = "anthropic/claude-haiku-4-5-20251001"  # optional trivial-chat route
model_race = "off"        # off | auto | provider/model — first-token failover
# race_after_ms = 800
response_cache = "auto"   # auto | off — text-only exact + semantic
intent_guidance = "auto"  # auto | off | always — build-mode question/plan posture
compaction_threshold = 150000
auto_title = true
# title_model = "anthropic/claude-haiku-4-5-20251001"

[permission]
bash = "ask"
edit = "allow"
# "mymcp_*" = "deny"

[security]
bash_risk_threshold = "destructive"  # caution | destructive | off
sandbox = "workspace"                # off | workspace
sandbox_network = true
sandbox_fallback = "allow"           # allow | deny (when bwrap is missing)
# network_allowlist = ["github.com", "crates.io"]
# network_denylist = ["tracking.example.com"]

# [[hooks]]
# event = "pre_tool"   # or post_tool
# match = "bash"
# command = "echo $WHYCODE_TOOL_NAME"
# block_on_failure = true
```

### Security (short)

- Tool permission: `allow` / `ask` / `deny` + glob; TUI **multi-ask queue** (parallel ask backlog).
- Shell risk: `safe` → `caution` → `destructive` → `catastrophic` (last is never approved).
- OS sandbox: Linux `bwrap` workspace; other-OS fallback.
- HTTP tool domain allow/denylist; shell network is binary (`sandbox_network`).
- Config shell hooks (`pre_tool` / `post_tool`); plugin marketplace skeleton.

---

## Sources

- whycode: [README.md](../README.md), [comparison.md](comparison.md), [status.md](status.md), [benchmarks.md](benchmarks.md)
- OpenCode: <https://opencode.ai/> · <https://github.com/anomalyco/opencode>
- jcode: <https://github.com/1jehuang/jcode> · <https://jcode.sh>
- Grok Build: <https://docs.x.ai/build/overview> · <https://github.com/xai-org/grok-build>
- Claude Code: <https://code.claude.com/docs/en/overview>
- Codex CLI: <https://github.com/openai/codex> · install/docs via openai.com/chatgpt.com
- Gemini CLI: <https://github.com/google-gemini/gemini-cli> · <https://geminicli.com/docs/>
- Pi: <https://pi.dev/> · <https://github.com/earendil-works/pi>
- Cursor: <https://cursor.com/> · <https://cursor.com/docs>

Performance rows (TTFF/PSS): jcode README comparison table + whycode `docs/benchmarks.md` (different machines).

This table is a living snapshot. Update it in the same PR when a major feature or competitor position changes.
