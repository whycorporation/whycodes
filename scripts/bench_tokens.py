#!/usr/bin/env python3
"""Measure what whycodes sends to the model over a scripted two-turn session.

A local OpenAI-compatible mock plays the model: it records every request and
answers with a fixed script of tool calls (read a large and a small file, grep,
write a 60-line file, run a failing test that prints 4,000 lines; then, in
turn 2, read and grep again). whycodes runs that session in `--plain` mode
against the repository, under a throwaway `WHYCODES_HOME`, so the real account
is never touched and nothing is billed.

For each model request it reports the input size and the part that repeats the
previous request byte for byte from the start: what a provider prompt cache can
serve. `uncached` is what the provider has to process again, the figure that
drives cost and latency.

Token counts use tiktoken's o200k encoding when tiktoken is installed and fall
back to chars/4 otherwise (the output says which).

Usage:
    python scripts/bench_tokens.py [--binary path] [--compare other] [--json out.json]

Use a release build: on Windows a debug build overflows the main-thread stack
when `--plain` runs a turn.
"""

from __future__ import annotations

import argparse
import itertools
import json
import os
import platform
import subprocess
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WRITE_PATH = ".whycodes/scratch/bench_tokens_write.rs"
WRITE_BODY = "".join(
    f"pub fn bench_fn_{i}(x: u32) -> u32 {{ x.wrapping_mul({i}) }}\n" for i in range(60)
)
FAIL_CMD = (
    'for i in $(seq 1 4000); do echo "test case_$i ... ok"; done; '
    'echo "failures: case_77 panicked"; exit 101'
)
# The step inside a turn is the number of assistant tool calls after that
# turn's prompt; `None` ends the turn with a text answer.
SCRIPT = {
    "TURN1": [
        ("read", {"path": "crates/session/src/session.rs"}),
        ("read", {"path": "crates/agent/src/notify.rs"}),
        ("grep", {"pattern": "pin_prompt_suffix|spill_old_tool_results", "path": "crates"}),
        ("write", {"path": WRITE_PATH, "content": WRITE_BODY}),
        ("bash", {"command": FAIL_CMD}),
        None,
    ],
    "TURN2": [
        ("read", {"path": "crates/agent/src/agent/cache_warm.rs"}),
        ("grep", {"pattern": "fn build_request", "path": "crates/session"}),
        None,
    ],
}
PROMPTS = [
    "TURN1 review the prompt and spill code in these crates, write a bench file and run the tests",
    "TURN2 explain how cache_warm relates to build_request",
]

try:
    import tiktoken

    _ENC = tiktoken.get_encoding("o200k_base")
    TOKENIZER = "o200k"

    def count(text: str) -> int:
        return len(_ENC.encode(text))

except ImportError:  # pragma: no cover - depends on the host
    TOKENIZER = "chars/4"

    def count(text: str) -> int:
        return len(text) // 4


def default_binary() -> Path:
    name = "whycodes.exe" if platform.system() == "Windows" else "whycodes"
    for profile in ("release", "debug"):
        candidate = ROOT / "target" / profile / name
        if candidate.exists():
            return candidate
    sys.exit("no whycodes binary found — build one first:\n  cargo build --release -p whycodes-cli")


def make_handler(captured: list):
    counter = itertools.count(1)

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def _json(self, payload: dict) -> None:
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps(payload).encode())

        def do_GET(self):  # model listing
            self._json({"object": "list", "data": [{"id": "mock", "object": "model"}]})

        def do_POST(self):
            raw = self.rfile.read(int(self.headers.get("Content-Length", 0)))
            body = json.loads(raw)
            msgs = body.get("messages", [])
            usage = {"prompt_tokens": len(raw) // 4, "completion_tokens": 10,
                     "total_tokens": len(raw) // 4 + 10}
            if not body.get("stream"):
                # Side requests (title, memory extraction): plain completion.
                text = "NONE" if "durable" in json.dumps(msgs) else "Bench session"
                self._json({"id": "x", "object": "chat.completion", "model": "mock",
                            "choices": [{"index": 0, "finish_reason": "stop",
                                         "message": {"role": "assistant", "content": text}}],
                            "usage": usage})
                return
            if body.get("tools"):
                captured.append(body)
            turn, start = None, 0
            for idx, m in enumerate(msgs):
                content = m.get("content")
                text = content if isinstance(content, str) else json.dumps(content)
                if m.get("role") == "user":
                    for key in SCRIPT:
                        if text.startswith(key):
                            turn, start = key, idx
            step = sum(1 for m in msgs[start:] if m.get("role") == "assistant" and m.get("tool_calls"))
            steps = SCRIPT.get(turn, [None])
            action = steps[min(step, len(steps) - 1)]
            if action is None:
                deltas = [{"role": "assistant", "content": f"{turn} done."}]
                finish = "stop"
            else:
                name, args = action
                deltas = [{"role": "assistant", "tool_calls": [{
                    "index": 0, "id": f"call_{next(counter)}", "type": "function",
                    "function": {"name": name, "arguments": json.dumps(args)}}]}]
                finish = "tool_calls"
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            chunks = [{"choices": [{"index": 0, "delta": d, "finish_reason": None}]} for d in deltas]
            chunks.append({"choices": [{"index": 0, "delta": {}, "finish_reason": finish}], "usage": usage})
            for c in chunks:
                c.update({"id": "x", "object": "chat.completion.chunk", "model": "mock"})
                self.wfile.write(b"data: " + json.dumps(c).encode() + b"\n\n")
            self.wfile.write(b"data: [DONE]\n\n")

    return Handler


def run_session(binary: Path) -> list:
    captured: list = []
    server = ThreadingHTTPServer(("127.0.0.1", 0), make_handler(captured))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    port = server.server_address[1]
    try:
        with tempfile.TemporaryDirectory(prefix="whycodes-bench-tokens-") as home:
            Path(home, "config.toml").write_text(
                "[default_model]\nprovider_id = \"mock\"\nmodel_id = \"mock\"\ncontext_window = 200000\n\n"
                f"[providers.mock]\nname = \"openai\"\nbase_url = \"http://127.0.0.1:{port}/v1\"\n"
                "api_key = \"sk-mock\"\nmodels = [\"mock\"]\n",
                encoding="utf-8",
            )
            env = {**os.environ, "WHYCODES_HOME": home, "WHYCODES_PLAIN": "1",
                   "WHYCODES_APPROVAL_MODE": "auto"}
            stdin = "\n".join(PROMPTS + ["/exit"]) + "\n"
            proc = subprocess.run([str(binary), "--plain"], input=stdin, text=True, encoding="utf-8",
                                  env=env, cwd=ROOT, capture_output=True, timeout=600)
            if proc.returncode != 0:
                sys.exit(f"{binary} exited {proc.returncode}:\n{proc.stderr[-2000:]}")
    finally:
        server.shutdown()
        (ROOT / WRITE_PATH).unlink(missing_ok=True)
    return captured


def tok(obj) -> int:
    return count(json.dumps(obj, ensure_ascii=False, sort_keys=True))


def measure(requests: list) -> dict:
    rows, prev = [], None
    for req in requests:
        head = tok(req["tools"])  # tools go first on the wire
        total = head + sum(tok(m) for m in req["messages"])
        cached = 0
        if prev is not None and prev["tools"] == req["tools"]:
            cached = head
            for a, b in zip(prev["messages"], req["messages"]):
                if a != b:
                    break
                cached += tok(b)
        rows.append({"messages": len(req["messages"]), "total": total, "cached_prefix": cached,
                     "uncached": total - cached})
        prev = req
    return {"requests": rows,
            "total_input": sum(r["total"] for r in rows),
            "uncached": sum(r["uncached"] for r in rows),
            "last_request": rows[-1]["total"] if rows else 0}


def report(label: str, result: dict) -> None:
    print(f"{label}  (tokenizer: {TOKENIZER})")
    print(f"  {'req':>3} {'msgs':>4} {'total':>8} {'cached':>8} {'uncached':>8}")
    for i, r in enumerate(result["requests"], 1):
        print(f"  {i:>3} {r['messages']:>4} {r['total']:>8} {r['cached_prefix']:>8} {r['uncached']:>8}")
    print(f"  total input {result['total_input']}, uncached {result['uncached']}, "
          f"last request {result['last_request']}\n")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--binary", type=Path, default=None)
    ap.add_argument("--compare", type=Path, default=None, help="a second binary, e.g. the previous release")
    ap.add_argument("--json", type=Path, default=None)
    args = ap.parse_args()

    binaries = [args.binary or default_binary()] + ([args.compare] if args.compare else [])
    results = {}
    for binary in binaries:
        results[str(binary)] = measure(run_session(binary))
        report(str(binary), results[str(binary)])
    if args.compare:
        new, old = results[str(binaries[0])], results[str(binaries[1])]
        for key in ("total_input", "uncached", "last_request"):
            change = (new[key] - old[key]) / old[key] if old[key] else 0.0
            print(f"{key:>12}: {old[key]} -> {new[key]} ({change:+.0%})")
    if args.json:
        args.json.write_text(json.dumps({"tokenizer": TOKENIZER, "results": results}, indent=2),
                             encoding="utf-8")
        print(f"wrote {args.json}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
