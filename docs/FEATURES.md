# Özellikler ve karşılaştırmalı tablo

Terminal coding agent’ların özellik matrisi.

**Terminal / harness:** whycode · Grok Build · OpenCode · jcode · Codex CLI · Gemini CLI · Pi  

**Ürün yüzeyi:** Claude Code · Cursor

Son güncelleme: **2026-08-05** (rakip hücreleri dokümantasyondan dolduruldu).  
Kaynaklar dosya sonundadır. Hücreler “var / kısmi / yok” seviyesindedir; her minor sürüm bire bir doğrulanmaz.

### Sembol

| | |
|:---:|---|
| ✅ | Var / production |
| ⚠️ | Kısmi, iskelet veya sınırlı |
| ❌ | Yok / roadmap |
| ★ | Bu alanda belirgin güçlü yön |

† whycode ACP: bilinçli **ürün sonrası** (`docs/status.md`, 2026-08-04).  
‡ Gemini CLI: ücretsiz/Google One kullanıcıları için **Antigravity CLI** geçişi duyuruldu (2026-06-18); matris hâlâ Gemini CLI dokümantasyonuna göre.

---

## Ürün özeti

| | whycode | Grok Build | OpenCode | jcode | Claude Code | Codex CLI | Gemini CLI | Pi | Cursor |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| **Tür** | Terminal agent | Terminal agent | Multi-surface | Agent harness | Resmi ürün ajanı | Terminal agent | Terminal agent | Minimal harness | AI IDE + agent |
| **Yüzey** | TUI, CLI | TUI, CLI, ACP | TUI, desktop, IDE | TUI, serve | TUI, web, desktop, IDE, CI | TUI, CLI, IDE, app, cloud | TUI, CLI, IDE companion | TUI, CLI, SDK/RPC | IDE, CLI, cloud agents |
| **Dil** | Rust | Rust | TypeScript | Rust | Kapalı (native binary) | Rust | TypeScript | TypeScript | Kapalı (VS Code tabanlı) |
| **Lisans** | MIT | Apache-2.0 | MIT | MIT | Proprietary | Apache-2.0 | Apache-2.0 | MIT | Proprietary |
| **Kimlik** | API key | xAI login + API | API + OAuth | API + OAuth | Claude sub / API | ChatGPT plan / API | Google OAuth / API / Vertex | BYOK API + `/login` | Cursor hesap |
| **Model** | Çoklu provider | Grok + custom | 75+ provider | Çoklu + OAuth | Claude (+ sınırlı) | OpenAI / Codex modelleri | Gemini 3 (1M ctx) | Çoklu BYOK | Claude / GPT / Gemini / Composer |
| **Açık kaynak** | Evet | Evet | Evet | Evet | Hayır | Evet | Evet | Evet | Hayır |

| Ürün | Konum |
|---|---|
| **whycode** | Shell safety, latency stack, mouse TUI, Windows CI |
| **Grok Build** | Zengin TUI; skills / plugins / hooks; ACP |
| **OpenCode** | Geniş OSS ekosistem; TUI + desktop + IDE |
| **jcode** | Düşük RAM / boot; swarm; semantic memory |
| **Claude Code** | Anthropic ürün yüzeyi (web, desktop, IDE, CI) |
| **Codex CLI** | OpenAI terminal agent; OS sandbox; AGENTS.md; cloud Codex |
| **Gemini CLI** | Google terminal agent; ücretsiz kota; Search grounding; plan mode |
| **Pi** | Minimal harness; extensions/skills; multi-provider; container sandbox |
| **Cursor** | IDE-first Agent + Tab; cloud agents; multi-model |

---

## 1. Platform, dağıtım, runtime

Kısa başlıklar: **why** · **Grok** · **OC** OpenCode · **jc** jcode · **CC** Claude · **Codex** · **Gem** Gemini · **Pi** · **Cur** Cursor

| Özellik | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Tek binary / native CLI | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ Rust | ⚠️ npm/Node | ⚠️ npm; bin build var | ❌ IDE; CLI ayrı |
| Install (curl / npm / brew) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ curl+npm+brew | ✅ npm+brew | ✅ npm+curl | ✅ app + `@cursor/cli` |
| Self-update | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ npm channel | ✅ `pi update` | ✅ app update |
| Homebrew / paket | ⚠️ HEAD | ⚠️ | ✅ | ✅ | ✅ | ✅ cask | ✅ | ⚠️ npm | ✅ |
| Linux / macOS | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Windows native | ✅★ | ⚠️ | ⚠️ | ✅ | ✅ | ✅ | ⚠️ | ✅ docs | ✅ |
| Cross-platform CI (full) | ✅★ | ⚠️ | ⚠️ | ✅ | ✅ | ⚠️ | ✅ CI badges | ✅ | n/a ürün |
| Runtime bağımlılık yok | ✅ | ✅ | ⚠️ | ✅ | ✅ | ✅ | ❌ Node | ❌ Node | n/a |
| In-process arama (rg yok) | ✅ | ⚠️ | ⚠️ | ✅ | ⚠️ | ⚠️ | ⚠️ ships rg helper | ⚠️ | n/a IDE search |
| Açık kaynak | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ❌ |

---

## 2. Arayüzler (surfaces)

| Özellik | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
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

## 3. LLM provider ve kimlik

| Özellik | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Çoklu provider (API) | ✅ | ✅ custom | ✅★ 75+ | ✅★ | ⚠️ | ❌ OpenAI ailesi | ❌ Gemini ailesi | ✅★ unified API | ✅ multi |
| Anthropic | ✅ | ⚠️ | ✅ | ✅ OAuth | ✅ | ❌ | ❌ | ✅ | ✅ |
| OpenAI | ✅ | ⚠️ | ✅ | ✅ OAuth | ⚠️ | ✅★ | ❌ | ✅ | ✅ |
| Google / Gemini | ✅ | ⚠️ | ✅ | ✅ OAuth | ⚠️ | ❌ | ✅★ | ✅ | ✅ |
| xAI / Grok | ✅ | ✅★ | ✅ | ✅ | ❌ | ❌ | ❌ | ⚠️ | ✅ |
| OpenRouter / Ollama / local | ✅ | ⚠️ | ✅ | ✅ | ❌ | ⚠️ | ⚠️ | ✅ + llama.cpp | ⚠️ |
| OpenAI-compatible custom | ✅ | ✅ | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ | ✅ | ⚠️ |
| OAuth / subscription login | ❌ | ✅ xAI | ✅ | ✅★ | ✅ | ✅ ChatGPT | ✅ Google | ✅ `/login` | ✅ Cursor |
| Credential import | ❌ | ⚠️ | ⚠️ | ✅★ | n/a | ⚠️ | ⚠️ | ⚠️ | n/a |

---

## 4. Agent sistemi

| Özellik | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Full-access agent | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ Agent |
| Plan / read-only mode | ✅ Ctrl+T | ✅ | ✅ Tab | ✅ | ✅ | ⚠️ approval modes | ✅ plan mode | ⚠️ | ✅ plan |
| Subagent | ✅ `task` | ✅ | ✅ | ✅ | ✅ | ✅ headless spawn | ✅ complete_task | ⚠️ ext | ✅ subagents |
| Built-in explore / scout | ✅ | ⚠️ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ❌ minimal | ⚠️ |
| Özel agent tanımları | ✅ config | ✅ | ✅ | ✅ | ✅ | ⚠️ AGENTS.md | ⚠️ GEMINI.md | ✅ extensions | ✅ rules/agents |
| Parallel multi-session | ❌ | ⚠️ | ✅ | ✅★ | ✅ | ⚠️ | ⚠️ | ⚠️ | ✅★ cloud fan-out |
| Swarm + conflict notify | ❌ | ❌ | ❌ | ✅★ | ⚠️ teams | ❌ | ❌ | ❌ | ⚠️ |
| Max turns / loop guard | ✅★ doom-loop | ✅ | ✅ | ⚠️ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |

---

## 5. Araçlar (tools)

| Özellik | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| read / write / edit | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| apply_patch / multi-hunk | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ replace | ⚠️ | ✅ |
| grep / glob / list | ✅ | ✅ | ✅ | ✅★ | ✅ | ✅ | ✅ | ⚠️ az built-in | ✅ |
| bash / shell | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ sandboxed | ✅ | ✅ | ✅ |
| websearch / fetch | ✅ | ✅ | ⚠️ | ⚠️ | ✅ | ⚠️ search flag | ✅★ Google Search + fetch | ⚠️ ext/MCP | ✅ |
| git tools | ✅ built-in | ✅ | ⚠️ | ⚠️ | ✅ | ⚠️ shell | ⚠️ shell | ⚠️ shell | ✅ |
| GitHub issue / PR | ✅ | ⚠️ | ⚠️ | ⚠️ | ✅ | ⚠️ | ✅ GH Action | ❌ | ✅ Bugbot/PR |
| todo / plan / question | ✅ | ✅ | ✅ | ⚠️ | ✅ | ⚠️ | ✅ todos+ask_user+plan | ⚠️ | ⚠️ |
| LSP | ✅★ crate | ⚠️ | ✅ | ❌ | ⚠️ | ⚠️ | ⚠️ | ❌ | ✅ IDE |
| Core / minimal tool set | ✅★ profile | ⚠️ | ⚠️ | ⚠️ | ✅ | ⚠️ | ⚠️ | ✅★ az araç | ⚠️ |
| Parallel tools | ✅★ | ✅ | ⚠️ | ⚠️ | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ |
| Browser automation | ❌ | ⚠️ | ⚠️ | ✅★ | ✅ | ⚠️ | ⚠️ | ❌ | ✅★ cloud VM |
| Image / multimodal | ⚠️ path attach | ✅ gen | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ✅ PDF/image | ⚠️ | ✅ |
| Video gen | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ⚠️ MCP (Veo) | ❌ | ❌ |

---

## 6. Güvenlik ve izinler

| Özellik | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| allow / ask / deny | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ approval policy | ✅ confirm mutators | ❌ built-in yok | ✅ |
| Permission globs / policy | ✅ | ✅ | ✅ | ⚠️ | ✅ | ✅ sandbox_mode | ✅ policy engine | ⚠️ container | ⚠️ |
| Multi-ask queue | ✅★ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ❌ | ⚠️ |
| Auto-approve / full-auto | ✅ env | ✅ | ⚠️ | ⚠️ | ✅ | ✅ full access mode | ⚠️ trusted folders | ⚠️ | ✅ |
| Shell risk sınıflandırması | ✅★ 4-tier | ⚠️ | ⚠️ | ✅ | ⚠️ | ⚠️ sandbox tiers | ⚠️ | ❌ | ⚠️ |
| Catastrophic hard-block | ✅★ | ⚠️ | ❌ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ❌ | ⚠️ |
| OS / container sandbox | ✅★ bwrap | ✅ | ❌ | ⚠️ | ✅ | ✅★ Landlock/seccomp/AppContainer | ✅ sandbox docs | ⚠️ Docker/Gondolin | ⚠️ cloud VM |
| Network allowlist / net-off | ✅ HTTP tools | ⚠️ | ❌ | ⚠️ | ✅ | ✅ net-off default legacy | ⚠️ | ⚠️ container | ⚠️ |
| Hooks pre/post tool | ✅ | ✅★ | ✅★ | ⚠️ | ✅★ | ⚠️ | ⚠️ | ✅ extensions | ✅ hooks |

---

## 7. Uzantılar: MCP, skills, plugins

| Özellik | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| MCP client | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ packages/ext | ✅ |
| MCP HTTP/SSE | ✅ | ⚠️ | ✅ | ❌ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| MCP as server (export) | ❌ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ✅ `codex mcp-server` | ❌ | ⚠️ | ⚠️ |
| Skills (SKILL.md) | ✅ | ✅★ | ✅ | ✅ | ✅★ | ✅ `.codex/skills` | ✅ `.gemini/skills` | ✅ | ✅ |
| Plugins / extensions | ⚠️ iskelet | ✅★ | ✅★ | Self-dev★ | ⚠️ | ⚠️ | ✅ extensions | ✅★ TS extensions | ✅ |
| Custom slash / commands | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ custom cmds | ✅ prompt templates | ✅ |
| AGENTS.md / CLAUDE.md / GEMINI.md | ✅ AGENTS | ✅ | ✅ AGENTS | ✅ | ✅ CLAUDE | ✅ AGENTS | ✅ GEMINI.md | ✅ AGENTS | ✅ rules / AGENT.md |
| Self-dev / self-extend | ❌ | ❌ | ❌ | ✅★ | ❌ | ❌ | ❌ | ✅★ self-extensible | ❌ |

---

## 8. Oturum, bellek, context

| Özellik | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Kalıcı session | ✅ SQLite | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ checkpoint | ✅ JSONL sessions | ✅ |
| Resume / list | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ checkpointing | ✅ tree/branch | ✅ |
| Cross-harness resume | ❌ | ⚠️ | ⚠️ | ✅★ | n/a | ⚠️ | ⚠️ | ⚠️ | ❌ |
| Undo / redo | ✅ git | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ branch | ✅ IDE |
| Context compact | ✅ drop+prune | ✅ | ✅★ | ✅ | ✅ | ⚠️ | ✅ token caching | ✅ compaction | ✅ |
| Semantic / auto memory | ❌ | ✅ | ❌ | ✅★ | ✅ | ✅ memories md | ⚠️ | ❌ | ⚠️ |
| Share / export | ✅ local | ✅ | ✅ | ⚠️ | ✅ | ⚠️ | ⚠️ | ✅ HF share tooling | ⚠️ |
| Usage / cost stats | ✅ | ✅ | ⚠️ | ⚠️ | ✅ | ⚠️ | ⚠️ telemetry | ⚠️ | ✅ `/usage` |

---

## 9. TUI / UX

| Özellik | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Live stream text + tools | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Thinking / reasoning | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ | ⚠️ | ✅ |
| Markdown + highlight | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ | ✅★ |
| Inline diff | ✅ | ✅★ | ✅ | ✅ | ✅ | ⚠️ | ✅ confirm diff | ⚠️ | ✅★ |
| Mermaid | ✅ | ⚠️ | ❌ | ✅★ | ❌ | ❌ | ❌ | ❌ | ⚠️ |
| Theme sistemi | ✅ 29+JSON | ✅ | ✅★ | ✅ | ⚠️ | ⚠️ | ⚠️ | ✅ themes | ✅ IDE |
| Mouse chrome (stop/scroll) | ✅★ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | n/a |
| `@file` / `!shell` / image | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ | ✅ @ + ! + image | ⚠️ | ✅ |

---

## 10. Latency / agent-loop (whycode odağı)

| Özellik | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Shared HTTP keep-alive | ✅★ | ✅ | ✅ | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ | n/a |
| Prompt / token cache | ✅★ | ⚠️ | ✅★ | ⚠️ | ✅ | ⚠️ OpenAI cache | ✅ token caching | ⚠️ | ⚠️ |
| Parallel safe tools | ✅★ | ✅ | ⚠️ | ⚠️ | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ |
| Core / minimal tools | ✅★ | ⚠️ | ⚠️ | ⚠️ | ✅ | ⚠️ | ⚠️ | ✅★ | ⚠️ |
| Doom-loop guard | ✅★ | ⚠️ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| Tool result prune | ✅★ | ⚠️ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| TTFT metrik (JSONL) | ✅★ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |

Detay: [plan-latency-competitors.md](plan-latency-competitors.md).

---

## 11. Çoklu ajan, arka plan, otomasyon

| Özellik | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Background tasks UI | ❌ | ✅ | ⚠️ | ✅ | ✅ | ⚠️ | ⚠️ shell bg | ❌ | ✅★ |
| Scheduled / loop | ❌ | ✅ | ❌ | ⚠️ ambient | ✅ | ⚠️ | ⚠️ | ❌ | ⚠️ |
| Cloud / remote session | ❌ | ❌ | ❌ | ⚠️ iOS plan | ✅★ | ✅★ Codex Web | ⚠️ | ❌ | ✅★ cloud agents |
| CI / headless agent | ✅ format | ⚠️ | ⚠️ | ⚠️ | ✅★ | ✅ | ✅ GH Action | ✅ json/RPC | ✅ Cursor CLI |
| Same-dir multi-agent | ❌ | ❌ | ⚠️ | ✅★ | ⚠️ | ⚠️ | ⚠️ | ❌ | ⚠️ parallel cloud |

---

## 12. Performans (yayınlanan / ölçülen)

Farklı makineler / tarihler; mertebe içindir. jcode README bench tablosu (Linux, 10× PTY) + whycode [benchmarks.md](benchmarks.md).

| Metrik | whycode | Grok Build | OpenCode | jcode | Claude Code | Codex CLI | Gemini CLI | Pi | Cursor |
|---|---|---|---|---|---|---|---|---|---|
| Boot / TTFF | **~1.3 ms** `--version` (Linux; Win baseline was ~21 ms) | — | ~1036 ms | **~14 ms** | ~3437 ms | ~883 ms | — | ~591 ms | IDE (saniye mertebesi) |
| 1 session PSS | **~4.1 MB** idle TUI | — | ~372 MB | **~28 MB** (embed off) | ~387 MB | ~140 MB | — | ~144 MB | IDE (yüzlerce MB+) |
| 10 session PSS | **~16.8 MB** | — | ~3.2 GB | **~117 MB** | ~2.3 GB | ~335 MB | — | ~833 MB | — |
| Kaynak | whycode benches | — | jcode bench | jcode README | jcode bench | jcode bench | public free-tier docs | jcode bench | ürün class |

whycode: process startup + criterion hot-path; agent TTFT için JSONL `ttft_ms`. Grok Build / Gemini CLI için karşılaştırılabilir PSS tablosu yayınlanmadı (—).

---

## 13. whycode’a özel güçlü yanlar

| Alan | Detay |
|---|---|
| **Shell risk gate** | `safe` → `caution` → `destructive` → `catastrophic` (sonuncusu asla onaylanmaz) |
| **Latency stack** | Core tools, prompt cache, parallel reads, doom-loop, fast-route, async title |
| **Mouse TUI** | HitArea: context %, stop, scrollbar, slash hover |
| **LSP crate** | Ayrı `whycode-lsp` |
| **Windows CI** | Linux + macOS + Windows full suite |
| **Küçük kod tabanı** | ~50k LOC Rust |
| **OpenCode interop** | Theme JSON, AGENTS.md, `.opencode/commands` |
| **In-process grep** | `rg` PATH’te olmasa da çalışır |
| **Yerel share** | `127.0.0.1` only |

---

## 14. whycode’da olmayan / zayıf (rakiplere göre)

| Boşluk | Kimde var | Not |
|---|---|---|
| OAuth / subscription login | jcode, OC, CC, Grok, Codex, Gemini, Pi `/login`, Cursor | [plan-oauth](plan-oauth.md) |
| Semantic memory | jcode★, Grok, Claude, Codex memories | [plan-memory](plan-memory.md) |
| Swarm | jcode★ | dropped |
| Browser automation | jcode, Claude, Cursor★ | Yok |
| Desktop / IDE | OpenCode, Claude, Codex app/IDE, **Cursor★** | `web` + `acp` stub |
| Plugin marketplace | Grok, OpenCode, Cursor, Gemini ext, Pi packages | Config hooks ✅ |
| OS sandbox (non-Linux) | Grok, Claude, **Codex★**, Gemini sandbox | Linux bwrap ✅ |
| Cloud agents | Claude, **Codex Web**, **Cursor★** | Yok |
| Free model kota | **Gemini CLI★** (1k req/day) | Yok |
| Minimal harness | **Pi★** | Core profile var |
| Self-dev / self-extend | jcode, **Pi★** | Yok |
| MCP-as-server export | **Codex★** | Yok |

---

## 15. Hızlı “kim ne için?”

| İhtiyaç | Yön |
|---|---|
| En düşük RAM, swarm, memory | **jcode** |
| Claude abonelik + web/desktop/IDE/CI | **Claude Code** |
| Model özgürlüğü + OSS multi-surface | **OpenCode** |
| Grok + plugins + ACP | **Grok Build** |
| OpenAI + OS sandbox + cloud Codex | **Codex CLI** |
| Ücretsiz kota + Search + plan mode | **Gemini CLI** |
| Minimal, extension-first harness | **Pi** |
| IDE-first + Tab + cloud agents | **Cursor** |
| Shell safety + latency + mouse TUI + Windows CI | **whycode** |

### Rakip notları (kısa)

| Ürün | Not |
|---|---|
| **Codex CLI** | Apache-2.0 Rust; ChatGPT login veya API; sandbox (Landlock/seccomp/AppContainer); AGENTS.md; MCP client + `mcp-server`; IDE eklentisi + Codex Web/App. |
| **Gemini CLI** | Apache-2.0 TypeScript; Google OAuth ücretsiz kota; built-in Search/fetch/plan/todos; sandbox + policy; GEMINI.md; stream-json. Antigravity CLI geçişi duyurusu var (‡). |
| **Pi** | MIT TypeScript monorepo (Earendil); multi-provider; az built-in tool + TS extensions/skills/themes; session tree + compaction; **built-in FS/net permission yok** — container önerilir. |
| **Cursor** | Proprietary IDE; Agent + Tab + multi-model; Cloud Agents (browser/desktop VM); MCP/skills/hooks; ayrı headless CLI. |

---

## 16. whycode özellik envanteri (tek başına)

### CLI

| Komut | Durum |
|---|---|
| `run` / varsayılan TUI | ✅ |
| `generate` (one-shot) | ✅ |
| `--format text\|json\|stream-json` (CI / NDJSON) | ✅ |
| `--continue` / `--resume` | ✅ |
| `acp` | ⚠️ stub (ürün sonrası) |
| `pr` / `github` | ✅ |
| `serve` (API + local share) | ✅ |
| `web` | ⚠️ stub |
| `mcp` / `provider` / `model` / `agent` / `config` / `session` | ✅ |
| `stats` / `debug` / `upgrade` | ✅ |

### Slash commands (TUI)

`/help`, `/exit`, `/new`, `/init`, `/undo`, `/redo`, `/share`, `/unshare`, `/compact`, `/sessions`, `/resume`, `/continue`, `/rename`, `/models`, `/agent`, `/connect`, `/tools`, `/info`, `/theme`/`/themes` (picker + isimle uygula) — custom: `.whycode/commands/*.md` ve `.opencode/commands`. Plain ek: `/thinking` (REPL). Command mode: `:` → `:theme`, `:q`, …

### Built-in tools

**Core profile (default, LLM şemasına giden):** `read`, `write`, `edit`, `apply_patch`, `grep`, `glob`, `list`, `bash` (alias `shell`), `todowrite` (alias `todo`), `todoread`, `task`.

**Full profile:** + `git_status`/`git_diff`/`git_log`/`git_blame`/`git_commit`, `github_issue`/`github_pr`, `webfetch`, `websearch`, `plan`, `question`, `skill`, `lsp`, `code_mode`, `external_directory`, `truncate`, MCP `{server}_{tool}`.

### Agents

| Agent | Mod |
|---|---|
| `build` | primary, full access (varsayılan) |
| `plan` | primary, read-only (`Ctrl+T` ile cycle) |
| `general` | subagent (`task`) |
| `explore` | subagent, read-only search |
| `scout` | subagent, docs/deps research |

### Config (latency + güvenlik)

```toml
[session]
tool_profile = "core"     # or "full"
prompt_cache = "auto"     # or "none"
model_fast = "anthropic/claude-haiku-4-5-20251001"  # optional trivial-chat route
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
sandbox_fallback = "allow"           # allow | deny (bwrap yoksa)
# network_allowlist = ["github.com", "crates.io"]
# network_denylist = ["tracking.example.com"]

# [[hooks]]
# event = "pre_tool"   # or post_tool
# match = "bash"
# command = "echo $WHYCODE_TOOL_NAME"
# block_on_failure = true
```

### Güvenlik (kısa)

- Tool izin: `allow` / `ask` / `deny` + glob; TUI **multi-ask queue** (paralel ask birikimi).
- Shell risk: `safe` → `caution` → `destructive` → `catastrophic` (sonuncusu asla onaylanmaz).
- OS sandbox: Linux `bwrap` workspace; diğer OS fallback.
- HTTP tool domain allow/denylist; shell ağı binary (`sandbox_network`).
- Config shell hooks (`pre_tool` / `post_tool`); plugin marketplace iskelet.

---

## Kaynaklar

- whycode: [README.md](../README.md), [comparison.md](comparison.md), [status.md](status.md), [benchmarks.md](benchmarks.md)
- OpenCode: <https://opencode.ai/> · <https://github.com/anomalyco/opencode>
- jcode: <https://github.com/1jehuang/jcode> · <https://jcode.sh>
- Grok Build: <https://docs.x.ai/build/overview> · <https://github.com/xai-org/grok-build>
- Claude Code: <https://code.claude.com/docs/en/overview>
- Codex CLI: <https://github.com/openai/codex> · install/docs via openai.com/chatgpt.com
- Gemini CLI: <https://github.com/google-gemini/gemini-cli> · <https://geminicli.com/docs/>
- Pi: <https://pi.dev/> · <https://github.com/earendil-works/pi>
- Cursor: <https://cursor.com/> · <https://cursor.com/docs>

Performans satırları (TTFF/PSS): jcode README karşılaştırma tablosu + whycode `docs/benchmarks.md` (farklı makineler).

Bu tablo living snapshot’tır. Büyük özellik veya rakip konum değişince aynı PR’da güncelle.
