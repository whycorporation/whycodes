# Özellikler ve karşılaştırmalı tablo

Terminal coding agent’ların özellik matrisi.

**Terminal / harness:** whycode · Grok Build · OpenCode · jcode · Codex CLI · Gemini CLI · Pi  

**Ürün yüzeyi (karşılaştırma için):** Claude Code · Cursor

Son güncelleme: **2026-08-05**. Kaynaklar bölüm sonundadır. Rakip sütunları “var / kısmi / yok” seviyesinde konumlandırma içindir; her minor sürüm bire bir doğrulanmaz.

### Sembol

| | |
|:---:|---|
| ✅ | Var / production |
| ⚠️ | Kısmi, iskelet veya sınırlı |
| ❌ | Yok / roadmap |
| ★ | Bu alanda belirgin güçlü yön |

† whycode ACP: bilinçli **ürün sonrası** (`docs/status.md`, 2026-08-04).

---

## Ürün özeti

| | whycode | Grok Build | OpenCode | jcode | Claude Code | Codex CLI | Gemini CLI | Pi | Cursor |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| **Tür** | Terminal agent | Terminal agent | Multi-surface | Agent harness | Resmi ürün ajanı | Terminal agent | Terminal agent | Minimal harness | AI IDE |
| **Yüzey** | TUI, CLI | TUI, CLI, ACP | TUI, desktop, IDE | TUI, serve | TUI, web, desktop, IDE, CI | TUI, CLI | TUI, CLI | TUI, CLI | IDE, agent, cloud |
| **Dil** | Rust | Rust | TypeScript | Rust | Kapalı | Rust | TypeScript | TypeScript | Kapalı (VS Code tabanlı) |
| **Lisans** | MIT | Apache-2.0 | MIT | MIT | Proprietary | Apache-2.0 | Apache-2.0 | MIT | Proprietary |
| **Kimlik** | API key | xAI login + API | API + OAuth | API + OAuth | Claude sub / API | ChatGPT / API | Google / API | API key (BYOK) | Cursor hesap + modeller |
| **Model** | Çoklu provider | Grok + custom | 75+ provider | Çoklu + OAuth | Claude (+ sınırlı) | OpenAI ailesi | Gemini | Çoklu BYOK | Claude / GPT / Gemini / Composer |
| **Açık kaynak** | Evet | Evet | Evet | Evet | Hayır | Evet | Evet | Evet | Hayır |

| Ürün | Konum |
|---|---|
| **whycode** | Shell safety, latency stack, mouse TUI, Windows CI |
| **Grok Build** | Zengin TUI; skills / plugins / hooks; ACP |
| **OpenCode** | Geniş OSS ekosistem; TUI + desktop + IDE |
| **jcode** | Düşük RAM / boot; swarm; semantic memory |
| **Claude Code** | Anthropic ürün yüzeyi (web, desktop, IDE, CI) |
| **Codex CLI** | OpenAI terminal agent; sandbox / headless; Terminal-Bench |
| **Gemini CLI** | Google terminal agent; ücretsiz kota; geniş context |
| **Pi** | Minimal, okunabilir harness; az araç; self-extend |
| **Cursor** | IDE-first agent; tab + Agent mode; multi-model |

---

## 1. Platform, dağıtım, runtime

Kısa başlıklar: **why** whycode · **Grok** · **OC** OpenCode · **jc** jcode · **CC** Claude Code · **Codex** · **Gem** Gemini CLI · **Pi** · **Cur** Cursor

| Özellik | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Tek binary / native CLI | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ | ⚠️ | ❌ IDE |
| Install (curl / npm / brew) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ app |
| Self-update | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ | ⚠️ | ✅ app |
| Homebrew / paket | ⚠️ HEAD | ⚠️ | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ | ✅ |
| Linux / macOS | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Windows native | ✅★ | ⚠️ | ⚠️ | ✅ | ✅ | ✅ | ⚠️ | ⚠️ | ✅ |
| Cross-platform CI (full) | ✅★ | ⚠️ | ⚠️ | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ | n/a |
| Runtime bağımlılık yok | ✅ | ✅ | ⚠️ | ✅ | ✅ | ✅ | ⚠️ Node | ⚠️ Node | n/a |
| In-process arama (rg yok) | ✅ | ⚠️ | ⚠️ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | n/a IDE |
| Açık kaynak | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ❌ |

---

## 2. Arayüzler (surfaces)

| Özellik | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Fullscreen TUI | ✅ | ✅★ | ✅★ | ✅★ | ✅ | ✅ | ✅ | ✅ | ❌ (IDE) |
| Headless / plain | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ agent CLI |
| Mouse-interactive TUI | ✅★ | ✅★ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | n/a |
| Desktop app | ❌ | ❌ | ✅ | ❌ | ✅ | ❌ | ❌ | ❌ | ✅★ IDE |
| IDE extension | ❌ | ⚠️ ACP | ✅ | ❌ | ✅ | ⚠️ | ⚠️ | ❌ | n/a (IDE) |
| Web UI | ⚠️ stub | ❌ | ⚠️ | ❌ | ✅ | ⚠️ cloud | ⚠️ | ❌ | ⚠️ |
| ACP | ⚠️ stub† | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| HTTP / serve | ✅ local | ⚠️ | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ cloud |
| Streaming JSON / CI | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ | ⚠️ |

---

## 3. LLM provider ve kimlik

| Özellik | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Çoklu provider (API) | ✅ | ✅ custom | ✅★ | ✅★ | ⚠️ | ⚠️ OpenAI | ⚠️ Gemini | ✅★ BYOK | ✅ multi |
| Anthropic | ✅ | ⚠️ | ✅ | ✅ OAuth | ✅ | ❌ | ❌ | ✅ | ✅ |
| OpenAI | ✅ | ⚠️ | ✅ | ✅ OAuth | ⚠️ | ✅★ | ❌ | ✅ | ✅ |
| Google / Gemini | ✅ | ⚠️ | ✅ | ✅ OAuth | ⚠️ | ❌ | ✅★ | ✅ | ✅ |
| xAI / Grok | ✅ | ✅★ | ✅ | ✅ | ❌ | ❌ | ❌ | ⚠️ | ✅ |
| OpenRouter / Ollama | ✅ | ⚠️ | ✅ | ✅ | ❌ | ⚠️ | ⚠️ | ✅ | ⚠️ |
| OpenAI-compatible | ✅ | ✅ | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ | ✅ | ⚠️ |
| OAuth / subscription | ❌ | ✅ xAI | ✅ | ✅★ | ✅ | ✅ ChatGPT | ✅ Google | ❌ | ✅ Cursor |
| Credential import | ❌ | ⚠️ | ⚠️ | ✅★ | n/a | ⚠️ | ⚠️ | ⚠️ | n/a |

---

## 4. Agent sistemi

| Özellik | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Full-access agent | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ Agent |
| Plan / read-only | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ | ✅ plan |
| Subagent | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ | ✅ |
| Built-in explore / scout | ✅ | ⚠️ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ❌ minimal | ⚠️ |
| Özel agent tanımları | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| Parallel multi-session | ❌ | ⚠️ | ✅ | ✅★ | ✅ | ⚠️ | ⚠️ | ⚠️ | ✅ |
| Swarm + conflict notify | ❌ | ❌ | ❌ | ✅★ | ⚠️ | ❌ | ❌ | ❌ | ⚠️ |
| Max turns / loop guard | ✅★ | ✅ | ✅ | ⚠️ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |

---

## 5. Araçlar (tools)

| Özellik | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| read / write / edit | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| apply_patch / multi-hunk | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ | ⚠️ | ✅ |
| grep / glob / list | ✅ | ✅ | ✅ | ✅★ | ✅ | ✅ | ✅ | ⚠️ az araç | ✅ |
| bash / shell | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| websearch / fetch | ✅ | ✅ | ⚠️ | ⚠️ | ✅ | ⚠️ | ✅ grounding | ⚠️ | ✅ |
| git tools | ✅ | ✅ | ⚠️ | ⚠️ | ✅ | ⚠️ | ⚠️ | ⚠️ | ✅ |
| GitHub issue / PR | ✅ | ⚠️ | ⚠️ | ⚠️ | ✅ | ⚠️ | ⚠️ | ❌ | ⚠️ |
| todo / plan / question | ✅ | ✅ | ✅ | ⚠️ | ✅ | ⚠️ | ⚠️ | ❌ minimal | ⚠️ |
| LSP | ✅★ | ⚠️ | ✅ | ❌ | ⚠️ | ⚠️ | ⚠️ | ❌ | ✅ IDE |
| Core tool profile | ✅★ | ⚠️ | ⚠️ | ⚠️ | ✅ | ⚠️ | ⚠️ | ✅★ az araç | ⚠️ |
| Parallel tools | ✅★ | ✅ | ⚠️ | ⚠️ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| Browser automation | ❌ | ⚠️ | ⚠️ | ✅★ | ✅ | ⚠️ | ⚠️ | ❌ | ✅ |
| Image / video gen | ❌ | ✅★ | ❌ | ❌ | ⚠️ | ⚠️ | ⚠️ | ❌ | ⚠️ |

---

## 6. Güvenlik ve izinler

| Özellik | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| allow / ask / deny | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ | ✅ |
| Permission globs | ✅ | ✅ | ✅ | ⚠️ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| Multi-ask queue | ✅★ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| Auto-approve env | ✅ | ✅ | ⚠️ | ⚠️ | ✅ | ✅ full-auto | ⚠️ | ⚠️ | ✅ |
| Shell risk sınıflandırması | ✅★ | ⚠️ | ⚠️ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| Catastrophic hard-block | ✅★ | ⚠️ | ❌ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| OS sandbox | ✅★ bwrap | ✅ | ❌ | ⚠️ | ✅ | ✅★ | ⚠️ | ⚠️ | ⚠️ |
| Network allowlist | ✅ HTTP tools | ⚠️ | ❌ | ⚠️ | ✅ | ✅ net-off mode | ⚠️ | ⚠️ | ⚠️ |
| Hooks pre/post tool | ✅ | ✅★ | ✅★ | ⚠️ | ✅★ | ⚠️ | ⚠️ | ⚠️ | ✅ |

---

## 7. Uzantılar: MCP, skills, plugins

| Özellik | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| MCP client | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ | ✅ |
| MCP HTTP/SSE | ✅ | ⚠️ | ✅ | ❌ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| Skills (SKILL.md) | ✅ | ✅★ | ✅ | ✅ | ✅★ | ⚠️ | ⚠️ | ⚠️ | ✅ |
| Plugins / marketplace | ⚠️ | ✅★ | ✅★ | Self-dev★ | ⚠️ | ⚠️ | ⚠️ extensions | ⚠️ | ✅ |
| Custom slash / commands | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| AGENTS.md / CLAUDE.md | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ rules |
| Self-dev hot reload | ❌ | ❌ | ❌ | ✅★ | ❌ | ❌ | ❌ | ⚠️ self-extend | ❌ |

---

## 8. Oturum, bellek, context

| Özellik | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Kalıcı session | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ | ✅ |
| Resume | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ | ✅ |
| Cross-harness resume | ❌ | ⚠️ | ⚠️ | ✅★ | n/a | ⚠️ | ⚠️ | ⚠️ | ❌ |
| Undo / redo | ✅ | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ✅ IDE |
| Context compact | ✅ | ✅ | ✅★ | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ | ✅ |
| Semantic memory | ❌ | ✅ | ❌ | ✅★ | ✅ | ❌ | ⚠️ | ❌ | ⚠️ |
| Share / export | ✅ local | ✅ | ✅ | ⚠️ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| Usage / cost stats | ✅ | ✅ | ⚠️ | ⚠️ | ✅ | ⚠️ | ⚠️ | ⚠️ | ✅ |

---

## 9. TUI / UX

| Özellik | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Live stream text + tools | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Thinking gösterimi | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ | ⚠️ | ✅ |
| Markdown + highlight | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ | ✅★ |
| Inline diff | ✅ | ✅★ | ✅ | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ | ✅★ |
| Mermaid | ✅ | ⚠️ | ❌ | ✅★ | ❌ | ❌ | ❌ | ❌ | ⚠️ |
| Theme sistemi | ✅ 29+JSON | ✅ | ✅★ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ✅ IDE |
| Mouse chrome (stop / scroll) | ✅★ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | n/a |
| `@file` / image attach | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ | ✅ |

---

## 10. Latency / agent-loop (whycode odağı)

| Özellik | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Shared HTTP keep-alive | ✅★ | ✅ | ✅ | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ | n/a |
| Prompt cache | ✅★ | ⚠️ | ✅★ | ⚠️ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| Parallel safe tools | ✅★ | ✅ | ⚠️ | ⚠️ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| Core / minimal tools | ✅★ | ⚠️ | ⚠️ | ⚠️ | ✅ | ⚠️ | ⚠️ | ✅★ | ⚠️ |
| Doom-loop guard | ✅★ | ⚠️ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| Tool result prune | ✅★ | ⚠️ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| TTFT metrik JSONL | ✅★ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |

Detay: [plan-latency-competitors.md](plan-latency-competitors.md).

---

## 11. Çoklu ajan, arka plan, otomasyon

| Özellik | why | Grok | OC | jc | CC | Codex | Gem | Pi | Cur |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Background tasks UI | ❌ | ✅ | ⚠️ | ✅ | ✅ | ⚠️ | ⚠️ | ❌ | ✅★ |
| Scheduled / loop | ❌ | ✅ | ❌ | ⚠️ | ✅ | ⚠️ | ⚠️ | ❌ | ⚠️ |
| Cloud / remote session | ❌ | ❌ | ❌ | ⚠️ | ✅★ | ✅ cloud | ⚠️ | ❌ | ✅ cloud agents |
| CI / headless agent | ✅ format | ⚠️ | ⚠️ | ⚠️ | ✅★ | ✅ | ✅ | ⚠️ | ⚠️ |
| Same-dir multi-agent | ❌ | ❌ | ⚠️ | ✅★ | ⚠️ | ⚠️ | ⚠️ | ❌ | ⚠️ |

---

## 12. Performans (yayınlanan / ölçülen)

Farklı makineler / tarihler; mertebe içindir.

| Metrik | whycode | OpenCode | jcode | Claude Code | Codex / Gemini / Pi / Cursor |
|---|---|---|---|---|---|
| Boot / TTFF | ~21 ms `--version` | ~1.0 s TTFF (jcode bench) | **~14 ms** TTFF | ~3.4 s TTFF | jcode bench: Codex ~0.9 s; Gem/Pi ölçümler değişir; Cursor IDE |
| 1 session PSS | **~4.1 MB** | ~372 MB | **~28 MB** (embed off) | ~387 MB | — |
| 10 session PSS | **~16.8 MB** | ~3.2 GB | **~117 MB** | ~2.3 GB | — |
| Kaynak | [benchmarks.md](benchmarks.md) | jcode bench | jcode README | jcode bench | jcode / public benches |

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
| OAuth / subscription login | jcode, OC, CC, Grok, Codex, Gemini, Cursor | [plan-oauth](plan-oauth.md) |
| Semantic memory | jcode★, Grok, Claude | [plan-memory](plan-memory.md) |
| Swarm | jcode★ | dropped |
| Browser automation | jcode, Claude, Cursor | Yok |
| Desktop / IDE | OpenCode, Claude, **Cursor★** | `web` + `acp` stub |
| Plugin marketplace | Grok, OpenCode, Cursor | Config hooks ✅ |
| OS sandbox (non-Linux) | Grok, Claude, **Codex★** | Linux bwrap ✅ |
| Cloud agents | Claude, Codex, Cursor | Yok |
| Free Gemini kota | **Gemini CLI★** | Yok |
| Minimal harness (az araç) | **Pi★** | Core profile var; Pi kadar minimal değil |
| Self-dev | jcode | Yok |

---

## 15. Hızlı “kim ne için?”

| İhtiyaç | Yön |
|---|---|
| En düşük RAM, swarm, memory | **jcode** |
| Claude abonelik + web/desktop/IDE/CI | **Claude Code** |
| Model özgürlüğü + OSS multi-surface | **OpenCode** |
| Grok + plugins + ACP | **Grok Build** |
| OpenAI + sandbox / Terminal-Bench | **Codex CLI** |
| Ücretsiz kota + Gemini context | **Gemini CLI** |
| Minimal, hackable harness | **Pi** |
| IDE-first agent + tab complete | **Cursor** |
| Shell safety + latency + mouse TUI + Windows CI | **whycode** |

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
- OpenCode: <https://opencode.ai/>
- jcode: <https://github.com/1jehuang/jcode> · <https://jcode.sh>
- Grok Build: <https://docs.x.ai/build/overview> · <https://github.com/xai-org/grok-build>
- Claude Code: <https://code.claude.com/docs/en/overview>
- Codex CLI: <https://github.com/openai/codex>
- Gemini CLI: <https://github.com/google-gemini/gemini-cli> · <https://geminicli.com/>
- Pi: <https://pi.dev/> · <https://github.com/badlogic/pi-mono>
- Cursor: <https://cursor.com/>

Bu tablo living snapshot’tır. Rakip hücreleri ürün dokümantasyonu ve halka açık konumlandırmaya dayanır; her minor sürüm bire bir doğrulanmaz. Büyük özellik veya rakip konum değişince aynı PR’da güncelle.
