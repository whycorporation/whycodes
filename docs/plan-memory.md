# Plan — Semantic / auto memory

**Status:** shipped (v1 + v2 complete) · **Was:** phase 6

## Decision

| Layer | Implementation |
|-------|----------------|
| v1 | Hash embedder + MEMORY.md + auto-recall + CLI/tool write |
| v2 | Auto-retain, project scope sync, code RAG, ONNX feature, subagent banks |

**Default embedder remains hashing** (zero RSS). MiniLM ONNX is opt-in:

```bash
cargo build -p whycode-cli --features onnx
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
- `whycode-memory` crate
- SQLite `memories` + dual-write `MEMORY.md`
- Auto-inject (index + top-k cosine) on user turns
- `/remember`, `/memory`, `memory` tool, `whycode memory …`
- **Auto-retain** (heuristic) after successful turns — `auto_retain = true`
- Dedupe on write (cosine ≥ 0.92)

### Cross-machine sync
- `scope = "user"` (default, data_dir) or `"project"` (`.whycode/memory`, git-shareable)
- `whycode memory export|import` JSON snapshots

### Codebase RAG
- SQLite `code_chunks` table
- `whycode memory index` / `memory` tool `action=index`
- `whycode memory code-search` / tool `code_search`
- Auto-inject code hits when `code_inject = true` and index exists

### ONNX MiniLM
- Feature `onnx` on `whycode-memory` (+ optional forward from CLI)
- `embed_backend = "onnx"` with download + tract inference; falls back to hash

### Subagent banks
- `project_key::agent_name` bank isolation
- Subagent system prompt injects its own bank (`subagent_banks` default true)

## Config

```toml
[memory]
enabled = true
auto_inject = true
auto_retain = true
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
```

Flags: `--no-memory`, `WHYCODE_NO_MEMORY=1`.

## CLI

```text
whycode memory list|search|add|delete|clear|path
whycode memory export [-o file.json]
whycode memory import file.json
whycode memory index
whycode memory code-search "query"
```

## Acceptance

- [x] Offline hash embedding + recall
- [x] Auto-retain durable preferences
- [x] Export/import + project-scoped MEMORY.md
- [x] Code index + search
- [x] Subagent bank isolation
- [x] ONNX feature path (build-time optional; download on use)
- [x] Tests green; dependency boundaries ok
