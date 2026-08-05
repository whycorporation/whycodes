# Özellikler ve karşılaştırmalı tablo

Terminal coding agent’ların özellik matrisi: **whycode**, **Grok Build**, **OpenCode**, **jcode**, **Claude Code**.

Son güncelleme: **2026-08-05**. Kaynaklar: whycode README + codebase, [opencode.ai](https://opencode.ai/), [jcode README](https://github.com/1jehuang/jcode), [docs.x.ai/build](https://docs.x.ai/build/overview), [code.claude.com](https://code.claude.com/docs/en/overview). Rakip sütunları her ürünün her minor sürümünü bire bir doğrulamaz; “var / kısmi / yok” seviyesinde konumlandırma içindir.

## Ürün özeti

| | **whycode** | **Grok Build** | **OpenCode** | **jcode** | **Claude Code** |
|---|---|---|---|---|
| **Ne** | Rust terminal coding agent | SpaceXAI / xAI coding agent + TUI | Açık kaynak agent (TUI + desktop + IDE) | RAM-efficient Rust harness | Anthropic resmi coding agent |
| **Dil** | Rust | Rust | TypeScript / Effect (TUI + desktop + server) | Rust | Native binary (eski: Node) |
| **Lisans** | MIT | Apache-2.0 (açık kaynak) | Açık kaynak (MIT benzeri) | MIT | Proprietary (kapalı kaynak) |
| **Varsayılan model** | Herhangi (API key) | Grok 4.5 + özel modeller | 75+ provider | Herhangi + OAuth | Claude (Opus/Sonnet) + 3. parti |
| **Konum** | Hafif, shell-safe, latency stack, Windows CI | Zengin TUI, skills/plugins/hooks, ACP | Geniş ekosistem, multi-surface | Performans + swarm + bellek | Ürün yüzeyi (web/desktop/IDE/CI) |

### Sembol açıklaması

| Sembol | Anlam |
|:---:|---|
| ✅ | Var / production |
| ⚠️ | Kısmi, iskelet, stub veya sınırlı |
| ❌ | Yok / planlanmamış veya roadmap’te |
| ★ | Bu alanda belirgin güçlü yön |

† whycode ACP: bilinçli **ürün sonrası** — karar `docs/status.md` decision log (2026-08-04).

---

## 1. Platform, dağıtım, runtime

| Özellik | whycode | Grok Build | OpenCode | jcode | Claude Code |
|---|:---:|:---:|:---:|:---:|:---:|
| Tek binary / native CLI | ✅ | ✅ | ✅ | ✅ | ✅ |
| Install script (curl/irm) | ✅ | ✅ | ✅ | ✅ | ✅ |
| Self-update (`upgrade`) | ✅ | ✅ | ✅ | ✅ | ✅ |
| Homebrew / paket yöneticisi | ⚠️ HEAD formula | ⚠️ | ✅ | ✅ | ✅ |
| Linux | ✅ | ✅ | ✅ | ✅ | ✅ |
| macOS | ✅ | ✅ | ✅ | ✅ | ✅ |
| Windows native | ✅★ | ⚠️ | ⚠️ (WSL önerilir) | ✅ | ✅ |
| Cross-platform CI (tam suite) | ✅★ | ⚠️ | ⚠️ | ✅ | ✅ |
| Runtime bağımlılık (Node vs) | ❌ yok | ❌ yok | ⚠️ npm yolu var | ❌ yok | ❌ (native) |
| In-process arama (rg gerekmez) | ✅ | ⚠️ | ⚠️ | ✅ | ⚠️ |
| Açık kaynak | ✅ | ✅ | ✅ | ✅ | ❌ |

---

## 2. Arayüzler (surfaces)

| Özellik | whycode | Grok Build | OpenCode | jcode | Claude Code |
|---|:---:|:---:|:---:|:---:|:---:|
| Fullscreen TUI | ✅ | ✅★ | ✅★ | ✅★ | ✅ |
| Plain / headless REPL | ✅ `--plain` | ✅ `-p` | ✅ | ✅ `run` | ✅ `-p` |
| Mouse-interactive TUI | ✅★ HitArea, stop, scrollbar, slash | ✅★ | ⚠️ | ⚠️ | ⚠️ |
| Desktop app | ❌ | ❌ | ✅ | ❌ | ✅ |
| IDE extension (VS Code vb.) | ❌ | ⚠️ ACP | ✅ | ❌ | ✅ VS Code + JetBrains |
| Web UI | ⚠️ stub | ❌ | ⚠️ | ❌ | ✅ claude.ai/code |
| iOS / mobil | ❌ | ❌ | ❌ | ⚠️ planlı | ✅ app + remote |
| ACP (Agent Client Protocol) | ⚠️ stub† | ✅ | ⚠️ | ⚠️ | ⚠️ |
| HTTP API / `serve` | ✅ (local share) | ⚠️ | ✅ | ✅ `serve`/`connect` | ⚠️ |
| Streaming JSON / CI mode | ✅ | ✅ | ✅ | ✅ | ✅ |

---

## 3. LLM provider ve kimlik doğrulama

| Özellik | whycode | Grok Build | OpenCode | jcode | Claude Code |
|---|:---:|:---:|:---:|:---:|:---:|
| Çoklu provider (API key) | ✅ | ✅ custom | ✅★ 75+ | ✅★ | ⚠️ 3. parti |
| Anthropic | ✅ | ⚠️ | ✅ | ✅ OAuth | ✅ native |
| OpenAI | ✅ | ⚠️ | ✅ | ✅ OAuth | ⚠️ |
| Google / Gemini | ✅ | ⚠️ | ✅ | ✅ OAuth | ⚠️ |
| xAI / Grok | ✅ | ✅★ | ✅ | ✅ | ❌ |
| Groq, DeepSeek, Mistral, Together | ✅ | ⚠️ | ✅ | ✅ | ❌ |
| OpenRouter | ✅ | ⚠️ | ✅ | ✅ | ❌ |
| Ollama / local | ✅ | ⚠️ | ✅ | ✅ | ❌ |
| OpenAI-compatible custom | ✅ | ✅ | ✅ | ✅ | ⚠️ |
| OAuth (Claude / ChatGPT / Copilot) | ❌ | ✅ (xAI login) | ✅ | ✅★ | ✅ |
| Multi-account switch | ❌ | ⚠️ | ⚠️ | ✅ | ⚠️ |
| Credential import (diğer CLI’lerden) | ❌ | ⚠️ Claude import | ⚠️ | ✅★ | n/a |

---

## 4. Agent sistemi

| Özellik | whycode | Grok Build | OpenCode | jcode | Claude Code |
|---|:---:|:---:|:---:|:---:|:---:|
| Build / full-access agent | ✅ | ✅ | ✅ | ✅ | ✅ |
| Plan / read-only mode | ✅ Ctrl+T | ✅ Shift+Tab | ✅ Tab | ✅ | ✅ |
| Subagent (`task` vb.) | ✅ | ✅ | ✅ | ✅ | ✅ |
| Built-in: general / explore / scout | ✅ | ⚠️ | ✅ | ⚠️ | ⚠️ |
| Özel agent tanımları | ✅ config | ✅ | ✅ | ✅ | ✅ |
| Parallel multi-session (aynı repo) | ❌ | ⚠️ fork | ✅ | ✅★ swarm | ✅ background |
| Swarm + conflict notify | ❌ | ❌ | ❌ | ✅★ | ⚠️ teams |
| Agent-to-agent messaging | ❌ | ⚠️ | ❌ | ✅★ | ⚠️ |
| Coordinator + worker spawn | ⚠️ tek `task` | ✅ | ⚠️ | ✅★ | ✅ |
| Plan file gate (sadece plan edit) | ⚠️ | ✅ | ⚠️ | ⚠️ | ✅ |
| Max turns / loop koruması | ✅★ max turns + doom-loop | ✅ | ✅ doom_loop | ⚠️ | ✅ |

---

## 5. Araçlar (tools)

| Özellik | whycode | Grok Build | OpenCode | jcode | Claude Code |
|---|:---:|:---:|:---:|:---:|:---:|
| read / write / edit | ✅ | ✅ | ✅ | ✅ | ✅ |
| apply_patch / multi-hunk | ✅ | ✅ | ✅ | ✅ | ✅ |
| grep / glob / list | ✅ | ✅ | ✅ | ✅★ agent-grep | ✅ |
| bash / shell | ✅ | ✅ | ✅ | ✅ | ✅ |
| websearch / webfetch | ✅ | ✅ | ⚠️ skill/MCP | ⚠️ | ✅ |
| git status/diff/log/blame/commit | ✅ | ✅ | ⚠️ | ⚠️ | ✅ |
| GitHub issue / PR | ✅ | ⚠️ | ⚠️ | ⚠️ | ✅ Actions/review |
| todo write/read | ✅ | ✅ | ✅ | ⚠️ | ✅ |
| question (kullanıcıya sor) | ✅ | ✅ | ✅ | ⚠️ | ✅ |
| plan tool | ✅ | ✅ | ✅ | ⚠️ | ✅ |
| LSP tool | ✅★ crate | ⚠️ | ✅ auto-load | ❌ ayrı crate yok | ⚠️ |
| code_mode / external_directory | ✅ | ⚠️ | ✅ | ⚠️ | ⚠️ |
| Core tool profile (küçük schema) | ✅★ default `core` | ⚠️ | ⚠️ | ⚠️ curated OAuth | ✅ defer |
| Parallel tool execution | ✅★ safe fan-out | ✅ | ⚠️ | ⚠️ | ✅ |
| Browser automation | ❌ | ⚠️ | ⚠️ | ✅★ Firefox bridge | ✅ Chrome |
| Image gen (`/imagine`) | ❌ | ✅ | ❌ | ❌ | ⚠️ |
| Video gen | ❌ | ✅ | ❌ | ❌ | ❌ |
| Dictation / STT | ❌ | ❌ | ❌ | ✅ | ⚠️ |

---

## 6. Güvenlik ve izinler

| Özellik | whycode | Grok Build | OpenCode | jcode | Claude Code |
|---|:---:|:---:|:---:|:---:|:---:|
| Tool permission: allow / ask / deny | ✅ | ✅ | ✅ | ✅ | ✅ |
| Permission globs (`mymcp_*`) | ✅ | ✅ | ✅ | ⚠️ | ✅ |
| Multi-ask permission queue | ✅★ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| Auto-approve / auto-deny env | ✅ | ✅ always-approve | ⚠️ | ⚠️ | ✅ |
| Shell **risk sınıflandırması** | ✅★ | ⚠️ | ⚠️ | ✅ safety system | ⚠️ |
| Catastrophic komut hard-block | ✅★ (`rm -rf ~` asla onaylanmaz) | ⚠️ | ❌ | ⚠️ | ⚠️ |
| Sandbox (OS / network) | ✅★ (Linux bwrap workspace; net opt-out) | ✅ | ❌ | ⚠️ | ✅ |
| Network allowlist | ✅ (HTTP tools; shell stays binary) | ⚠️ | ❌ | ⚠️ | ✅ |
| Hooks (pre/post tool) | ✅ config shell hooks | ✅★ | ✅★ | ⚠️ | ✅★ |

---

## 7. Uzantılar: MCP, skills, plugins, commands

| Özellik | whycode | Grok Build | OpenCode | jcode | Claude Code |
|---|:---:|:---:|:---:|:---:|:---:|
| MCP client (stdio) | ✅ | ✅ | ✅ | ✅ | ✅ |
| MCP HTTP/SSE | ✅ | ⚠️ | ✅ | ❌ (skip) | ✅ |
| MCP yönetim CLI | ✅ | ✅ `/mcps` | ✅ | ✅ mcp.json | ✅ |
| Skills (SKILL.md) | ✅ | ✅★ + marketplace | ✅ | ✅ semantic inject | ✅★ |
| Plugins / extensions | ⚠️ iskelet | ✅★ marketplace | ✅★ JS/TS | Self-dev★ | ⚠️ |
| Hooks marketplace | ❌ | ✅ | ✅ | ❌ | ⚠️ |
| Markdown custom slash commands | ✅ | ✅ skills as cmds | ✅ | ✅ | ✅ |
| `.opencode/commands` okuma | ✅ | ⚠️ | n/a | ⚠️ | ❌ |
| AGENTS.md / CLAUDE.md | ✅ AGENTS.md | ✅ | ✅ AGENTS.md | ✅ | ✅ CLAUDE.md |
| Self-dev (kendi kaynak kodunu edit+reload) | ❌ | ❌ | ❌ | ✅★ | ❌ |

---

## 8. Oturum, bellek, context

| Özellik | whycode | Grok Build | OpenCode | jcode | Claude Code |
|---|:---:|:---:|:---:|:---:|:---:|
| SQLite / kalıcı session | ✅ | ✅ | ✅ | ✅ | ✅ |
| Session list / rename / delete | ✅ TUI + plain | ✅ | ✅ | ✅ | ✅ |
| Resume session | ✅ TUI `/sessions` `/resume` + CLI | ✅ | ✅ | ✅ | ✅ |
| Auto-title (heuristic + LLM refine) | ✅ async small-model | ✅ | ✅ | ⚠️ | ✅ |
| Cross-harness resume (Claude/Codex/…) | ❌ | ⚠️ import Claude | ⚠️ | ✅★ | n/a |
| Undo / redo (git restore) | ✅ | ✅ rewind | ✅ | ⚠️ | ⚠️ |
| Context compact / summarize | ✅ drop + tool prune | ✅ | ✅★ LLM compact | ✅ | ✅ |
| Semantic memory (embedding) | ❌ roadmap | ✅ `/memory` `/dream` | ❌ | ✅★ | ✅ auto memory |
| Session search (geçmiş RAG) | ❌ | ⚠️ | ❌ | ✅ | ⚠️ |
| Share link | ✅ local only | ✅ | ✅ cloud | ⚠️ | ⚠️ |
| Export transcript | ✅ | ✅ | ✅ | ⚠️ | ✅ |
| Token / cost / usage stats | ✅ `stats` + session usage + cache tokens | ✅ `/usage` | ⚠️ | ⚠️ cache warn | ✅ `/cost` |

---

## 9. TUI deneyimi

| Özellik | whycode | Grok Build | OpenCode | jcode | Claude Code |
|---|:---:|:---:|:---:|:---:|:---:|
| Live streaming text + tools | ✅ | ✅ | ✅ | ✅ | ✅ |
| Thinking / reasoning gösterimi | ✅ | ✅ | ✅ | ✅ | ✅ |
| Markdown render | ✅ | ✅ | ✅ | ✅ | ✅ |
| Syntax highlighting | ✅ | ✅ | ✅ | ✅ | ✅ |
| Inline diff viewer | ✅ | ✅★ | ✅ | ✅ side panel | ✅ |
| Mermaid diagrams | ✅ Unicode (mermaid-text) | ⚠️ vendored | ❌ | ✅★ native rust | ❌ |
| Theme sistemi | ✅ 29 + JSON | ✅ | ✅★ | ✅ | ⚠️ |
| OpenCode theme JSON uyumu | ✅★ | ⚠️ | n/a | ❌ | ❌ |
| Slash suggest popup | ✅ + mouse hover/click | ✅ | ✅ | ✅ | ✅ |
| Turn strip + **Worked for Xs** | ✅★ Grok-style | ✅ | ⚠️ | ⚠️ | ⚠️ |
| Context meter (tokens ↔ % hover) | ✅★ sticky HitArea | ✅ | ⚠️ | ⚠️ | ⚠️ |
| Mouse stop / scrollbar / path hover | ✅★ | ✅ | ⚠️ | ⚠️ | ⚠️ |
| Which-key / keybind hints | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ |
| Vim mode scrollback | ❌ | ✅ | ⚠️ | ⚠️ | ❌ |
| Side panel / info widgets | ⚠️ sidebar | ⚠️ | ⚠️ | ✅★ | ⚠️ |
| Toast notifications | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ |
| `@file` mention (capped inline) | ✅ 24k cap | ✅ | ✅ | ✅ | ✅ |
| `!shell` prefix | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| Image drag-drop prompt | ✅ path paste/drop | ⚠️ | ✅ | ⚠️ | ✅ |
| 1000+ fps claim / flicker-free | ❌ | ⚠️ | ❌ | ✅★ | ✅ `/tui` flicker-free |

---

## 10. Latency / agent-loop hızı (whycode odağı)

Bu satırlar **agent TTFT / multi-step wall-clock** için; process RSS ile karıştırılmamalı. Detay: [plan-latency-competitors.md](plan-latency-competitors.md).

| Özellik | whycode | Grok Build | OpenCode | jcode | Claude Code |
|---|:---:|:---:|:---:|:---:|:---:|
| Shared HTTP keep-alive | ✅★ | ✅ | ✅ | ✅ server | ✅ |
| Anthropic prompt cache (system+tools+latest user) | ✅★ OpenCode-parity | ⚠️ | ✅★ auto | ⚠️ | ✅ |
| Parallel safe tools + perm queue | ✅★ | ✅ | ⚠️ | ⚠️ | ✅ |
| Core tool profile (default) | ✅★ | ⚠️ | ⚠️ | ⚠️ | ✅ |
| Trivial chat: no tools + fast model route | ✅★ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| Doom-loop refuse (3× same tool) | ✅★ | ⚠️ | ✅ | ⚠️ | ⚠️ |
| Per-step tool prune (old dumps → 2k) | ✅★ | ⚠️ | ✅ prune | ⚠️ | ⚠️ |
| Async title (UI’yi bloklamaz) | ✅★ | ✅ | ✅ | ⚠️ | ✅ |
| JSONL `ttft_ms` / `tool_batch_ms` / cache | ✅★ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |

---

## 11. Çoklu ajan, arka plan, otomasyon

| Özellik | whycode | Grok Build | OpenCode | jcode | Claude Code |
|---|:---:|:---:|:---:|:---:|:---:|
| Background tasks UI | ❌ | ✅ `/tasks` | ⚠️ | ✅ | ✅ |
| Scheduled / loop prompts | ❌ | ✅ `/loop` | ❌ | ⚠️ ambient | ✅ `/loop` + routines |
| Cloud / remote session | ❌ | ❌ | ❌ | ⚠️ iOS plan | ✅★ teleport/remote |
| Slack / chat channels | ❌ | ❌ | ❌ | ❌ | ✅ |
| GitHub Actions / CI agent | ❌ | ⚠️ headless | ⚠️ | ⚠️ | ✅★ |
| Multi-agent same-dir (worktree’siz) | ❌ | ❌ | ⚠️ | ✅★ | ⚠️ worktree |

---

## 12. Performans (yayınlanan / ölçülen)

Rakamlar **farklı makinelerde, farklı tarihlerde** alınmıştır; doğrudan “daha hızlı” iddiası için değil, mertebe göstermek için.

| Metrik | whycode | Grok Build | OpenCode | jcode | Claude Code |
|---|---|---|---|---|---|
| `--version` / boot (ms) | ~21 ms (Win, 2026-07-31) | — | ~1036 ms TTFF (jcode bench) | **~14 ms** TTFF | ~3437 ms TTFF |
| Peak RSS (1 session) | ~10 MB (`--version`) | — | ~372 MB PSS | **~28 MB** PSS (embed off) | ~387 MB PSS |
| 1 session PSS (idle TUI) | **~4.1 MB** | — | — | **~28 MB** (embed off) | — |
| 10 session PSS | **~16.8 MB** (~1.4 MB/extra) | — | ~3.2 GB | **~117 MB** (embed off) | ~2.3 GB |
| Kaynak | [benchmarks.md](benchmarks.md) | — | jcode bench tablosu | jcode README | jcode bench tablosu |

whycode: process-level startup + hot-path criterion bench’ler mevcut. Agent TTFT artık JSONL `turn.step` ile ölçülebilir (`ttft_ms`, `cache_read_tokens`).

---

## 13. whycode’a özel güçlü yanlar

| Alan | Detay |
|---|---|
| **Shell risk gate** | `safe` → `caution` → `destructive` → `catastrophic`. Catastrophic **asla onaylanmaz**. |
| **Latency stack** | Core tools, OpenCode-parity cache, parallel reads, doom-loop, fast-route trivial chat, async title. |
| **Mouse TUI chrome** | Sticky HitArea: context %, stop, scrollbar, slash hover, path underline. |
| **LSP crate** | Ayrı `whycode-lsp` crate. |
| **Windows CI** | Linux + macOS + Windows full test. |
| **Küçük kod tabanı** | ~50k LOC Rust crates — okunabilir, fork edilebilir. |
| **OpenCode interop** | Theme JSON, AGENTS.md, `.opencode/commands`, permission modeli, agent isimleri. |
| **In-process grep** | `ripgrep`/`grep` PATH’te olmasa da çalışır. |
| **Yerel share** | `/share` → `http://127.0.0.1:3030/s/...` — veri makineden çıkmaz. |

---

## 14. whycode’da olmayan / zayıf kalan (rakiplere göre)

| Boşluk | Kimde var | Not |
|---|---|---|
| OAuth + subscription login | jcode, OpenCode, Claude, Grok | [plan-oauth](plan-oauth.md) — blocked |
| Semantic / cross-session memory | jcode★, Grok, Claude | [plan-memory](plan-memory.md) — not started |
| Swarm / multi-agent conflict | jcode★ | dropped ([archive](archive/phase-7-multi-agent.md)) |
| Browser automation | jcode, Claude | Yok |
| Desktop / IDE / web surface | OpenCode, Claude | `web` + `acp` stub; **ACP ürün sonrası** |
| Hooks + plugin marketplace | Grok, OpenCode, Claude | Config shell hooks ✅; marketplace iskelet |
| OS sandbox (macOS/Windows backend) | Grok, Claude | Linux bwrap ✅; diğer platformlar fallback |
| Cloud share / remote control | OpenCode, Claude | Sadece local share |
| Side panel UI | jcode | Basit sidebar; Mermaid Unicode ✅ |
| Image/video generation | Grok | Yok |
| Self-dev hot reload | jcode | Yok |
| LLM-summary compact (OpenCode agent) | OpenCode★ | Drop+prune var; LLM summary P2 |
| Package manager taps (brew bottles…) | hepsi | ⚠️ HEAD formula; bottles later |

---

## 15. Hızlı “kim ne için?”

| İhtiyaç | Önerilen yön |
|---|---|
| En düşük RAM / en çok paralel session | **jcode** |
| Claude aboneliği + web/desktop/IDE/CI ekosistemi | **Claude Code** |
| Model özgürlüğü + masaüstü + IDE + büyük community | **OpenCode** |
| xAI Grok, zengin TUI, skills/plugins/hooks, ACP | **Grok Build** |
| Hafif Rust, shell safety, mouse TUI, latency stack, Windows CI | **whycode** |

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

`/help`, `/exit`, `/new`, `/init`, `/undo`, `/redo`, `/share`, `/unshare`, `/compact`, `/sessions`, `/resume`, `/continue`, `/rename`, `/models`, `/agent`, `/connect`, `/tools`, `/info` — custom: `.whycode/commands/*.md`. Plain ek: `/thinking`, `/themes` (REPL).

### Built-in tools

**Core profile (default, LLM’e giden):** `read`, `write`, `edit`, `apply_patch`, `grep`, `glob`, `list`, `bash`/`shell`, `todo_*`, `task`.

**Full profile:** + `git_*`, `github_*`, `webfetch`, `websearch`, `plan`, `question`, `skill`, `lsp`, `code_mode`, `external_directory`, `truncate`, MCP `{server}_{tool}`.

### Agents

| Agent | Mod |
|---|---|
| `build` | primary, full access |
| `plan` | primary, read-only |
| `general` | subagent |
| `explore` | subagent, read-only search |
| `scout` | subagent, docs/deps research |

### Config (latency-relevant)

```toml
[session]
tool_profile = "core"     # or "full"
prompt_cache = "auto"     # or "none"
model_fast = "anthropic/claude-haiku-4-5-20251001"  # optional
compaction_threshold = 150000
auto_title = true

[permission]
bash = "ask"
edit = "allow"

[security]
bash_risk_threshold = "destructive"
sandbox = "workspace"
sandbox_network = true
```

### Güvenlik (kısa)

```toml
[permission]
bash = "ask"
edit = "allow"

[security]
bash_risk_threshold = "destructive"  # caution | destructive | off
sandbox = "workspace"                # off | workspace
sandbox_network = true
sandbox_fallback = "allow"           # allow | deny
```

---

## Kaynaklar

- whycode: [README.md](../README.md), [comparison.md](comparison.md), [status.md](status.md), [benchmarks.md](benchmarks.md), [plan-latency-competitors.md](plan-latency-competitors.md)
- OpenCode: <https://opencode.ai/> · <https://opencode.ai/docs/>
- jcode: <https://github.com/1jehuang/jcode> · <https://jcode.sh>
- Grok Build: <https://docs.x.ai/build/overview> · <https://github.com/xai-org/grok-build>
- Claude Code: <https://code.claude.com/docs/en/overview>

Bu tablo living snapshot’tır. Büyük özellik veya rakip konum değişince aynı PR’da güncelle.
