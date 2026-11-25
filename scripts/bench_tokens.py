#!/usr/bin/env python3
"""Measure what whycodes sends to the model over a scripted session.

A local OpenAI-compatible mock plays the model: it records every request and
answers with a fixed script of tool calls. `--workload basic` (the default,
two turns): read a large and a small file, grep, write a 60-line file, run a
failing test that prints 4,000 lines; then read and grep again. `--workload
dev` (three turns) is an edit loop: write a file, read it, edit it twice, run a
build that prints rustc-style warning blocks and an error, grep and glob
widely, then fix and re-run. Edits only touch scratch files. whycodes runs that session in `--plain` mode
against the repository, under a throwaway `WHYCODES_HOME`, so the real account
is never touched and nothing is billed.

For each model request it reports the input size and the part that repeats the
previous request byte for byte from the start: what a provider prompt cache can
serve. `uncached` is what the provider has to process again, the figure that
drives cost and latency.

Token counts use tiktoken's o200k encoding when tiktoken is installed and fall
back to chars/4 otherwise (the output says which).

Usage:
    python scripts/bench_tokens.py [--binary path] [--compare other] [--workload basic|dev]
                                   [--json out.json]

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

DEV_PATH = ".whycodes/scratch/bench_tokens_dev.rs"
DEV_BODY = "".join(
    f"pub fn step_{i}(input: &[u32]) -> u32 {{\n"
    "    let mut total = 0;\n"
    "    for value in input {\n"
    f"        total += value * {i};\n"
    "    }\n"
    "    total\n"
    "}\n\n"
    for i in range(40)
)
# A build as rustc prints it: 120 multi-line warning blocks, each about a
# different name in a different file (rustc adds the lint note to the first
# only), then one error. The output is a file the command cats, so no shell
# quoting stands in the way.
BUILD_LOG = ".whycodes/scratch/bench_tokens_build.log"
_WORDS = ["cache", "depth", "entry", "frame", "guard", "limit", "match", "owner",
          "probe", "queue", "range", "score", "token", "value", "width"]
_NAMES = [f"{a}_{b}" for a in _WORDS for b in _WORDS if a != b][:120]
BUILD_TEXT = "".join(
    f"warning: unused variable: `{name}`\n"
    f"  --> crates/{name.split('_')[0]}/src/{name.split('_')[1]}.rs:{17 + i * 3}:13\n"
    "   |\n"
    f"{17 + i * 3} |         let {name} = self.{name.split('_')[1]}();\n"
    f"   |             {'^' * len(name)} help: if this is intentional, prefix it with an "
    f"underscore: `_{name}`\n"
    + ("   |\n   = note: `#[warn(unused_variables)]` on by default\n" if i == 0 else "")
    + "\n"
    for i, name in enumerate(_NAMES)
) + (
    "error[E0308]: mismatched types\n"
    "  --> src/step_7.rs:4:18\n"
    "   |\n"
    "4 |         total += value * 7;\n"
    "   |                  ^^^^^ expected `u32`, found `&u32`\n\n"
    "error: could not compile `bench` (lib) due to 1 previous error; 120 warnings emitted\n"
)
DEV_SCRIPT = {
    "TURN1": [
        ("write", {"path": DEV_PATH, "content": DEV_BODY}),
        ("read", {"path": DEV_PATH}),
        ("edit", {"path": DEV_PATH, "old_string": "        total += value * 7;",
                  "new_string": "        total += *value * 7;"}),
        ("edit", {"path": DEV_PATH, "old_string": "pub fn step_12(input: &[u32]) -> u32 {",
                  "new_string": "pub fn step_12(input: &[u32]) -> u32 {\n    // twelve"}),
        ("bash", {"command": f"cat {BUILD_LOG}; exit 101"}),
        None,
    ],
    "TURN2": [
        ("grep", {"pattern": "fn ", "path": "crates/tools/src/file"}),
        ("glob", {"pattern": "crates/**/*.rs"}),
        ("read", {"path": "crates/tools/src/file/edit.rs", "offset": 1, "limit": 120}),
        None,
    ],
    "TURN3": [
        ("edit", {"path": DEV_PATH, "old_string": "        total += value * 9;",
                  "new_string": "        total += *value * 9;"}),
        ("bash", {"command": "echo '    Finished dev profile [unoptimized + debuginfo] in 0.42s'"}),
        None,
    ],
}
DEV_PROMPTS = [
    "TURN1 add the bench steps, fix the type errors and build",
    "TURN2 find where edit matching lives and how files are listed",
    "TURN3 apply the same fix to step 9 and rebuild",
]
# name -> (script, prompts, scratch files written up front, scratch files to remove)
WORKLOADS = {
    "basic": (SCRIPT, PROMPTS, {}, [WRITE_PATH]),
    "dev": (DEV_SCRIPT, DEV_PROMPTS, {BUILD_LOG: BUILD_TEXT}, [DEV_PATH, BUILD_LOG]),
}

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


def make_handler(captured: list, script: dict):
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
                    for key in script:
                        if text.startswith(key):
                            turn, start = key, idx
            step = sum(1 for m in msgs[start:] if m.get("role") == "assistant" and m.get("tool_calls"))
            steps = script.get(turn, [None])
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


def run_session(binary: Path, workload: str = "basic") -> list:
    script, prompts, setup, scratch = WORKLOADS[workload]
    for path, text in setup.items():
        (ROOT / path).parent.mkdir(parents=True, exist_ok=True)
        (ROOT / path).write_text(text, encoding="utf-8", newline="\n")
    captured: list = []
    server = ThreadingHTTPServer(("127.0.0.1", 0), make_handler(captured, script))
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
            stdin = "\n".join(prompts + ["/exit"]) + "\n"
            proc = subprocess.run([str(binary), "--plain"], input=stdin, text=True, encoding="utf-8",
                                  env=env, cwd=ROOT, capture_output=True, timeout=600)
            if proc.returncode != 0:
                sys.exit(f"{binary} exited {proc.returncode}:\n{proc.stderr[-2000:]}")
    finally:
        server.shutdown()
        for path in scratch:
            (ROOT / path).unlink(missing_ok=True)
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
    ap.add_argument("--workload", choices=sorted(WORKLOADS), default="basic")
    ap.add_argument("--json", type=Path, default=None)
    args = ap.parse_args()

    binaries = [args.binary or default_binary()] + ([args.compare] if args.compare else [])
    results = {}
    for binary in binaries:
        results[str(binary)] = measure(run_session(binary, args.workload))
        report(str(binary), results[str(binary)])
    if args.compare:
        new, old = results[str(binaries[0])], results[str(binaries[1])]
        for key in ("total_input", "uncached", "last_request"):
            change = (new[key] - old[key]) / old[key] if old[key] else 0.0
            print(f"{key:>12}: {old[key]} -> {new[key]} ({change:+.0%})")
    if args.json:
        args.json.write_text(json.dumps({"tokenizer": TOKENIZER, "workload": args.workload,
                                         "results": results}, indent=2),
                             encoding="utf-8")
        print(f"wrote {args.json}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
