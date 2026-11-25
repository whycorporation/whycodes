#!/usr/bin/env python3
"""How often code recall injects chunks that are on topic, for this repository.

Each case is a question a user might ask and the path prefixes that answer it.
For every case it runs `whycodes memory code-search` (the ranking turn recall
uses) and keeps hits at or above the recall floor, then reports:

- on-topic: injected chunks whose path answers the question, over all injected;
- answered: questions with at least one on-topic chunk.

The `bm25` row ranks the same FTS index by BM25 alone, straight from SQLite,
as a reference. Index first, under a throwaway home so the real account is not
touched:

    WHYCODES_HOME=/tmp/wc-eval whycodes memory index
    python scripts/eval_code_recall.py target/release/whycodes /tmp/wc-eval [top_k] [min_score] [-v]

The cases were written knowing the repository, so treat the numbers as a
comparison between rankings on one fixed set, not as an absolute measure.
"""
import os
import re
import sqlite3
import subprocess
import sys

# (user message, path prefixes that answer it)
CASES = [
    ("where does edit matching live", ["crates/tools/src/file/edit", "crates/tools/src/file/tolerant"]),
    ("how are files listed by the glob tool", ["crates/tools/src/file/glob", "crates/tools/src/file/list"]),
    ("explain how cache_warm relates to build_request", ["crates/agent/src/agent/cache_warm", "crates/session/src/session"]),
    ("how are old tool results spilled to disk", ["crates/session/src/session", "crates/agent/src/agent/spill"]),
    ("why does a background job lose the start of its output", ["crates/agent/src/background"]),
    ("strip ansi escape codes from command output", ["crates/sandbox/src/clean"]),
    ("how does the read tool build an outline of a large file", ["crates/tools/src/file/read", "crates/tools/src/file/repomap"]),
    ("how is recalled context added to the user message", ["crates/memory/src/service", "crates/cli/src/cmd/helpers", "crates/tui/src/run/persist"]),
    ("pkce code verifier for oauth login", ["crates/auth/src/pkce", "crates/auth/src/flow"]),
    ("question tool multiple choice options", ["crates/tools/src/agent_tools/question"]),
    ("send a discord or telegram notification when a turn finishes", ["crates/agent/src/notify", "crates/plugin/src/notify"]),
    ("assess the risk of a shell command before running it", ["crates/command-risk/src"]),
    ("fuzzy file picker for @ mentions", ["crates/index/src", "crates/tui/src/picker", "crates/tui/src/mention"]),
    ("how are mcp tools named when registered", ["crates/mcp/src"]),
    ("syntax highlighting of markdown code blocks", ["crates/format/src"]),
    ("sqlite schema migrations", ["crates/storage/src/migrations", "crates/storage/src/db"]),
    ("image file sent to the model as base64", ["crates/tools/src/file/read", "crates/session/src/session", "crates/llm/src"]),
    ("retry with backoff when the provider returns 429", ["crates/llm/src"]),
    ("compact the conversation when the context window fills up", ["crates/agent/src/agent/compact", "crates/session/src/session"]),
    ("permission prompt before a tool runs", ["crates/agent/src/agent/gate", "crates/agent/src/permission", "crates/tui/src/permission"]),
    ("load AGENTS.md and CLAUDE.md into the system prompt", ["crates/agent/src/context_files"]),
    ("subagent task tool spawning a worker", ["crates/agent/src/subagent", "crates/tools/src/agent_tools/task", "crates/agent/src/agent/task"]),
]

HIT = re.compile(r"^\[\d+\] (\S+?):\d+-\d+ · rel ([0-9.]+)", re.M)


def current(exe, home, q, k):
    env = {**os.environ, "WHYCODES_HOME": home}
    out = subprocess.run([exe, "memory", "code-search", q, "--limit", str(k)], env=env,
                         capture_output=True, text=True, encoding="utf-8").stdout
    return [(p, float(s)) for p, s in HIT.findall(out)]


def fts_words(q):
    words = []
    for w in re.split(r"[^0-9A-Za-z]+", q):
        w = w.lower()
        if len(w) >= 2 and w not in words:
            words.append(w)
    return " OR ".join(f'"{w}"' for w in words)


def bm25(db, q, k):
    rows = db.execute(
        "SELECT t.path, bm25(code_chunks_fts) FROM code_chunks t JOIN code_chunks_fts "
        "ON code_chunks_fts.rowid = t.rowid WHERE code_chunks_fts MATCH ? "
        "ORDER BY bm25(code_chunks_fts) LIMIT ?", (fts_words(q), k)).fetchall()
    return [(p, -s) for p, s in rows]


def score(cases_hits, k):
    shown = good = answered = 0
    for (q, want), hits in cases_hits:
        hits = hits[:k]
        ok = [any(p.startswith(w) for w in want) for p, _ in hits]
        shown += len(hits); good += sum(ok); answered += any(ok)
    return good, shown, answered


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    exe, home = sys.argv[1], sys.argv[2]
    k = int(sys.argv[3]) if len(sys.argv) > 3 else 4
    floor = float(sys.argv[4]) if len(sys.argv) > 4 else 0.22
    db = sqlite3.connect(os.path.join(home, "whycodes.db"))
    variants = {"whycodes code-search": [], "bm25": []}
    for q, want in CASES:
        cur = [h for h in current(exe, home, q, k) if h[1] >= floor]
        variants["whycodes code-search"].append(((q, want), cur))
        variants["bm25"].append(((q, want), bm25(db, q, k)))
    for name, ch in variants.items():
        good, shown, answered = score(ch, k)
        print(f"{name:24} on-topic {good}/{shown} ({good / max(shown, 1):.0%})  "
              f"answered {answered}/{len(CASES)}")
    if "-v" in sys.argv:
        for (q, want), hits in variants["whycodes code-search"]:
            print("\n#", q)
            for p, s in hits:
                print(f"   {'+' if any(p.startswith(w) for w in want) else '-'} {s:.2f} {p}")


if __name__ == "__main__":
    main()
