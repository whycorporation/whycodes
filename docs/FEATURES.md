# Özellikler ve karşılaştırmalı tablo

Terminal coding agent’ların özellik matrisi: **whycode**, **Grok Build**, **OpenCode**, **jcode**, **Claude Code**.

Son güncelleme: **2026-08-04**. Kaynaklar: whycode README + codebase, [opencode.ai](https://opencode.ai/), [jcode README](https://github.com/1jehuang/jcode), [docs.x.ai/build](https://docs.x.ai/build/overview), [code.claude.com](https://code.claude.com/docs/en/overview). Rakip sütunları her ürünün her minor sürümünü bire bir doğrulamaz; “var / kısmi / yok” seviyesinde konumlandırma içindir.

## Ürün özeti

| | **whycode** | **Grok Build** | **OpenCode** | **jcode** | **Claude Code** |
|---|---|---|---|---|
| **Ne** | Rust terminal coding agent | SpaceXAI / xAI coding agent + TUI | Açık kaynak agent (TUI + desktop + IDE) | RAM-efficient Rust harness | Anthropic resmi coding agent |
| **Dil** | Rust | Rust | Go / TS ekosistemi (TUI + desktop) | Rust | Native binary (eski: Node) |
| **Lisans** | MIT | Apache-2.0 (açık kaynak) | Açık kaynak (MIT benzeri) | MIT | Proprietary (kapalı kaynak) |
| **Varsayılan model** | Herhangi (API key) | Grok 4.5 + özel modeller | 75+ provider | Herhangi + OAuth | Claude (Opus/Sonnet) + 3. parti |
| **Konum** | Hafif, okunabilir, Windows-first CI | Zengin TUI, skills/plugins/hooks, ACP | Geniş ekosistem, multi-surface | Performans + swarm + bellek | Ürün yüzeyi (web/desktop/IDE/CI) |

### Sembol açıklaması

| Sembol | Anlam |
|:---:|---|
| ✅ | Var / production |
| ⚠️ | Kısmi, iskelet, stub veya sınırlı |
| ❌ | Yok / planlanmamış veya roadmap’te |
| ★ | Bu alanda belirgin güçlü yön |

---

## 1. Platform, dağıtım, runtime

| Özellik | whycode | Grok Build | OpenCode | jcode | Claude Code |
|---|:---:|:---:|:---:|:---:|:---:|
| Tek binary / native CLI | ✅ | ✅ | ✅ | ✅ | ✅ |
| Install script (curl/irm) | ✅ | ✅ | ✅ | ✅ | ✅ |
| Self-update (`upgrade`) | ✅ | ✅ | ✅ | ✅ | ✅ |
| Homebrew / paket yöneticisi | ❌ | ⚠️ | ✅ | ✅ | ✅ |
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
| Mouse-interactive TUI | ❌ | ✅★ | ⚠️ | ⚠️ | ⚠️ |
| Desktop app | ❌ | ❌ | ✅ | ❌ | ✅ |
| IDE extension (VS Code vb.) | ❌ | ⚠️ ACP | ✅ | ❌ | ✅ VS Code + JetBrains |
| Web UI | ⚠️ stub | ❌ | ⚠️ | ❌ | ✅ claude.ai/code |
| iOS / mobil | ❌ | ❌ | ❌ | ⚠️ planlı | ✅ app + remote |
| ACP (Agent Client Protocol) | ⚠️ stub | ✅ | ⚠️ | ⚠️ | ⚠️ |
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
| Plan / read-only mode | ✅ Tab | ✅ Shift+Tab | ✅ Tab | ✅ | ✅ |
| Subagent (`task` vb.) | ✅ | ✅ | ✅ | ✅ | ✅ |
| Built-in: general / explore / scout | ✅ | ⚠️ | ✅ | ⚠️ | ⚠️ |
| Özel agent tanımları | ✅ config | ✅ | ✅ | ✅ | ✅ |
| Parallel multi-session (aynı repo) | ❌ | ⚠️ fork | ✅ | ✅★ swarm | ✅ background |
| Swarm + conflict notify | ❌ | ❌ | ❌ | ✅★ | ⚠️ teams |
| Agent-to-agent messaging | ❌ | ⚠️ | ❌ | ✅★ | ⚠️ |
| Coordinator + worker spawn | ⚠️ tek `task` | ✅ | ⚠️ | ✅★ | ✅ |
| Plan file gate (sadece plan edit) | ⚠️ | ✅ | ⚠️ | ⚠️ | ✅ |
| Max turns / loop koruması | ✅ | ✅ | ⚠️ doom_loop | ⚠️ | ✅ |

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
| Auto-approve / auto-deny env | ✅ | ✅ always-approve | ⚠️ | ⚠️ | ✅ |
| Shell **risk sınıflandırması** | ✅★ | ⚠️ | ⚠️ | ✅ safety system | ⚠️ |
| Catastrophic komut hard-block | ✅★ (`rm -rf ~` asla onaylanmaz) | ⚠️ | ❌ | ⚠️ | ⚠️ |
| Sandbox (OS / network) | ❌ | ✅ | ❌ | ⚠️ | ✅ |
| Network allowlist | ❌ | ⚠️ | ❌ | ⚠️ | ✅ |
| Hooks (pre/post tool) | ⚠️ plugin iskelet | ✅★ | ✅★ | ⚠️ | ✅★ |

---

## 7. Uzantılar: MCP, skills, plugins, commands

| Özellik | whycode | Grok Build | OpenCode | jcode | Claude Code |
|---|:---:|:---:|:---:|:---:|:---:|
| MCP client (stdio) | ✅ | ✅ | ✅ | ✅ | ✅ |
| MCP HTTP/SSE | ⚠️ | ⚠️ | ✅ | ❌ (skip) | ✅ |
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
| Session list / rename / delete | ✅ | ✅ | ✅ | ✅ | ✅ |
| Resume session | ✅ plain | ✅ | ✅ | ✅ | ✅ |
| Cross-harness resume (Claude/Codex/…) | ❌ | ⚠️ import Claude | ⚠️ | ✅★ | n/a |
| Undo / redo (git restore) | ✅ | ✅ rewind | ✅ | ⚠️ | ⚠️ |
| Context compact / summarize | ✅ | ✅ | ✅ | ✅ | ✅ |
| Semantic memory (embedding) | ❌ roadmap | ✅ `/memory` `/dream` | ❌ | ✅★ | ✅ auto memory |
| Session search (geçmiş RAG) | ❌ | ⚠️ | ❌ | ✅ | ⚠️ |
| Share link | ✅ local only | ✅ | ✅ cloud | ⚠️ | ⚠️ |
| Export transcript | ✅ | ✅ | ✅ | ⚠️ | ✅ |
| Token / cost / usage stats | ⚠️ `stats` | ✅ `/usage` | ⚠️ | ⚠️ cache warn | ✅ `/cost` |

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
| Slash suggest popup | ✅ | ✅ | ✅ | ✅ | ✅ |
| Which-key / keybind hints | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ |
| Vim mode scrollback | ❌ | ✅ | ⚠️ | ⚠️ | ❌ |
| Side panel / info widgets | ⚠️ sidebar | ⚠️ | ⚠️ | ✅★ | ⚠️ |
| Toast notifications | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ |
| `@file` mention | ✅ | ✅ | ✅ | ✅ | ✅ |
| `!shell` prefix | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| Image drag-drop prompt | ✅ path paste/drop | ⚠️ | ✅ | ⚠️ | ✅ |
| 1000+ fps claim / flicker-free | ❌ | ⚠️ | ❌ | ✅★ | ✅ `/tui` flicker-free |

---

## 10. Çoklu ajan, arka plan, otomasyon

| Özellik | whycode | Grok Build | OpenCode | jcode | Claude Code |
|---|:---:|:---:|:---:|:---:|:---:|
| Background tasks UI | ❌ | ✅ `/tasks` | ⚠️ | ✅ | ✅ |
| Scheduled / loop prompts | ❌ | ✅ `/loop` | ❌ | ⚠️ ambient | ✅ `/loop` + routines |
| Cloud / remote session | ❌ | ❌ | ❌ | ⚠️ iOS plan | ✅★ teleport/remote |
| Slack / chat channels | ❌ | ❌ | ❌ | ❌ | ✅ |
| GitHub Actions / CI agent | ❌ | ⚠️ headless | ⚠️ | ⚠️ | ✅★ |
| Multi-agent same-dir (worktree’siz) | ❌ | ❌ | ⚠️ | ✅★ | ⚠️ worktree |

---

## 11. Performans (yayınlanan / ölçülen)

Rakamlar **farklı makinelerde, farklı tarihlerde** alınmıştır; doğrudan “daha hızlı” iddiası için değil, mertebe göstermek için.

| Metrik | whycode | Grok Build | OpenCode | jcode | Claude Code |
|---|---|---|---|---|---|
| `--version` / boot (ms) | ~21 ms (Win, 2026-07-31) | — | ~1036 ms TTFF (jcode bench) | **~14 ms** TTFF | ~3437 ms TTFF |
| Peak RSS (1 session) | ~10 MB (`--version`) | — | ~372 MB PSS | **~28 MB** PSS (embed off) | ~387 MB PSS |
| 1 session PSS (idle TUI) | **~4.1 MB** | — | — | **~28 MB** (embed off) | — |
| 10 session PSS | **~16.8 MB** (~1.4 MB/extra) | — | ~3.2 GB | **~117 MB** (embed off) | ~2.3 GB |
| Kaynak | [benchmarks.md](benchmarks.md) | — | jcode bench tablosu | jcode README | jcode bench tablosu |

whycode: TTFF (ilk TUI frame) henüz jcode ile aynı metodla yayınlanmadı; process-level startup ve hot-path criterion bench’ler mevcut.

---

## 12. whycode’a özel güçlü yanlar

| Alan | Detay |
|---|---|
| **Shell risk gate** | `safe` → `caution` → `destructive` → `catastrophic`. Catastrophic **asla onaylanmaz** (`bash=allow` ve `threshold=off` bile yetmez). |
| **LSP crate** | Ayrı `whycode-lsp` crate; jcode workspace’inde karşılığı yok. |
| **Windows CI** | Linux + macOS + Windows’ta full test; platforma özgü bug’lar CI’da yakalanıyor. |
| **Küçük kod tabanı** | ~24k LOC Rust — okunabilir, fork edilebilir, agent’ların self-edit etmesi kolay. |
| **OpenCode interop** | Theme JSON, AGENTS.md, `.opencode/commands`, permission modeli, agent isimleri. |
| **In-process grep** | `ripgrep`/`grep` PATH’te olmasa da çalışır. |
| **Yerel share** | `/share` → `http://127.0.0.1:3030/s/...` — veri makineden çıkmaz. |

---

## 13. whycode’da olmayan / zayıf kalan (rakiplere göre)

| Boşluk | Kimde var | Not |
|---|---|---|
| OAuth + subscription login | jcode, OpenCode, Claude, Grok | Phase 3 — blocked (owner kararı) |
| Semantic / cross-session memory | jcode★, Grok, Claude | Phase 6 — not started |
| Swarm / multi-agent conflict | jcode★ | Phase 7 — dropped (bilinçli) |
| Browser automation | jcode, Claude | Yok |
| Desktop / IDE / web surface | OpenCode, Claude | `web` + `acp` stub |
| Hooks + plugin marketplace | Grok, OpenCode, Claude | Plugin crate iskelet |
| OS sandbox | Grok, Claude | Defence-in-depth risk parse var, sandbox yok |
| Cloud share / remote control | OpenCode, Claude | Sadece local share |
| Side panel UI | jcode | Basit sidebar; Mermaid Unicode fenced blocks ✅ |
| Image/video generation | Grok | Yok |
| Self-dev hot reload | jcode | Yok |
| Package manager taps (brew…) | hepsi | Install script + self-update var |

---

## 14. Hızlı “kim ne için?”

| İhtiyaç | Önerilen yön |
|---|---|
| En düşük RAM / en çok paralel session | **jcode** |
| Claude aboneliği + web/desktop/IDE/CI ekosistemi | **Claude Code** |
| Model özgürlüğü + masaüstü + IDE + büyük community | **OpenCode** |
| xAI Grok, zengin TUI, skills/plugins/hooks, ACP | **Grok Build** |
| Hafif Rust, Windows CI, shell safety, okunabilir kaynak, OpenCode uyumu | **whycode** |

---

## 15. whycode özellik envanteri (tek başına)

### CLI

| Komut | Durum |
|---|---|
| `run` / varsayılan TUI | ✅ |
| `generate` (one-shot) | ✅ |
| `--format text\|json\|stream-json` (CI / NDJSON) | ✅ |
| `acp` | ⚠️ stub |
| `pr` / `github` | ✅ |
| `serve` (API + local share) | ✅ |
| `web` | ⚠️ stub |
| `mcp` / `provider` / `model` / `agent` / `config` / `session` | ✅ |
| `stats` / `debug` / `upgrade` | ✅ |

### Slash commands

`/help`, `/exit`, `/new`, `/init`, `/undo`, `/redo`, `/share`, `/unshare`, `/compact`, `/models`, `/agent`, `/connect`, `/tools`, `/info` — TUI + plain. Plain: `/sessions`, `/thinking`, `/themes`. Custom: `.whycode/commands/*.md`.

### Built-in tools

`read`, `write`, `edit`, `apply_patch`, `grep`, `glob`, `list`, `bash`/`shell`, `git_*`, `github_*`, `webfetch`, `websearch`, `task`, `plan`, `todowrite`/`todoread`, `question`, `skill`, `lsp`, `code_mode`, `external_directory`, `truncate` + MCP `{server}_{tool}`.

### Agents

| Agent | Mod |
|---|---|
| `build` | primary, full access |
| `plan` | primary, read-only |
| `general` | subagent |
| `explore` | subagent, read-only search |
| `scout` | subagent, docs/deps research |

### Config katmanları

1. Built-in defaults  
2. Global `config.toml`  
3. Project `.whycode/config.toml`  
4. `WHYCODE_*` env  

### Güvenlik

```toml
[permission]
bash = "ask"
edit = "allow"

[security]
bash_risk_threshold = "destructive"  # caution | destructive | off
```

---

## Kaynaklar

- whycode: [README.md](../README.md), [comparison.md](comparison.md), [status.md](status.md), [benchmarks.md](benchmarks.md)
- OpenCode: <https://opencode.ai/> · <https://opencode.ai/docs/>
- jcode: <https://github.com/1jehuang/jcode> · <https://jcode.sh>
- Grok Build: <https://docs.x.ai/build/overview> · <https://github.com/xai-org/grok-build>
- Claude Code: <https://code.claude.com/docs/en/overview>

Bu tablo statik bir snapshot’tır. whycode tarafında büyük bir özellik eklendiğinde veya rakip konumları değiştiğinde bu dosyayı aynı PR’da güncelle.
