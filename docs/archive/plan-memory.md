# Plan — Semantic / auto memory

**Status:** shipped (v1 + v2 + polish) · **Was:** phase 6

## Decision

| Layer | Implementation |
|-------|----------------|
| v1 | Hash embedder + MEMORY.md + auto-recall + CLI/tool write |
| v2 | Auto-retain, project scope sync, code RAG, ONNX feature, subagent banks |

**Default embedder remains hashing** (zero RSS). MiniLM ONNX is opt-in:

```bash
cargo build -p whycodes-cli --features onnx
# config:
# [memory]
# embed_backend = "onnx"
```

Model files download on first use to `{data_dir}/models/minilm/`.

## Competitive alignment

| Source | Feature |
|--------|---------|
| **Claude Code** | MEMORY.md, `/memory`, subagent banks, project vs user scope |
| **Grok / Hindsight** | Auto-recall + auto-retain after turns |
| **jcode** | Local embeddings, auto inject, code-oriented memory |

## What shipped

### Facts / auto memory
- `whycodes-memory` crate
- SQLite `memories` + dual-write `MEMORY.md`
- Auto-inject (index + top-k cosine) on user turns
- `/remember`, `/memory`, `memory` tool, `whycodes memory …`
- **Auto-retain** after successful turns inside `Agent::run_turn`:
  - heuristic always (when enabled)
  - optional **LLM extract** via small/fast sibling model when heuristic is empty
    (`retain_llm = true`, `retain_llm_always` optional)
- Dedupe on write (cosine ≥ 0.92)

### Cross-machine sync
- `scope = "user"` (default, data_dir) or `"project"` (`.whycodes/memory`, git-shareable)
- `whycodes memory export|import` JSON snapshots

### Codebase RAG
- SQLite `code_chunks` table
- `whycodes memory index` / `memory` tool `action=index`
- `whycodes memory code-search` / tool `code_search`
- Auto-inject code hits when `code_inject = true` and index exists

### ONNX MiniLM
- Feature `onnx` on `whycodes-memory` (+ `whycodes-cli --features onnx`)
- `embed_backend = "onnx"` with download + SHA-256 sidecar verify + tract inference
- `whycodes memory onnx-smoke` end-to-end probe

### Subagent banks
- `project_key::agent_name` bank isolation when `[memory] subagent_banks = true`
- Threaded from config through `Agent` → `SubagentRunner` (env `WHYCODES_SUBAGENT_BANKS=0` override)

### Session auto-index
- On TUI/plain session start: if code_chunks empty for bank, index once
  (`auto_index = true`)

## Config

```toml
[memory]
enabled = true
auto_inject = true
auto_retain = true
retain_llm = true
retain_llm_always = false
retain_every_n = 1
retain_max_facts = 3
max_index_lines = 200
max_index_bytes = 25600
recall_top_k = 5
recall_min_score = 0.28
recall_token_budget = 800
embed_dim = 256
scope = "user"          # or "project"
embed_backend = "hash"  # or "onnx"
code_inject = true
code_top_k = 4
code_min_score = 0.22
subagent_banks = true
auto_index = true
auto_index_max_files = 1500
auto_index_max_chunks = 4000
```

Flags: `--no-memory`, `WHYCODES_NO_MEMORY=1`, `WHYCODES_SUBAGENT_BANKS=0`.

## CLI

```text
whycodes memory list|search|add|delete|clear|path
whycodes memory export [-o file.json]
whycodes memory import file.json
whycodes memory index
whycodes memory code-search "query"
whycodes memory onnx-smoke   # needs --features onnx
```

## Acceptance

- [x] Offline hash embedding + recall
- [x] Auto-retain durable preferences (heuristic + optional LLM)
- [x] Export/import + project-scoped MEMORY.md
- [x] Code index + search + session auto-index
- [x] Subagent bank isolation (config-wired)
- [x] ONNX feature compiles; checksum sidecars; `onnx-smoke` CLI
- [x] Tests green; dependency boundaries ok
