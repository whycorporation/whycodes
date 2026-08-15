#!/usr/bin/env python3
"""Fail when protocol v1 tags drift between Rust and the TypeScript SDK.

Extracts SdkEvent / ErrorCode variants from crates/protocol/src/sdk.rs and
compares them to KNOWN_EVS / ERROR_CODES in sdk/typescript/src/types.ts.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RUST = ROOT / "crates" / "protocol" / "src" / "sdk.rs"
TS = ROOT / "sdk" / "typescript" / "src" / "types.ts"


def camel_to_snake(name: str) -> str:
    out: list[str] = []
    for i, ch in enumerate(name):
        if ch.isupper() and i and (name[i - 1].islower() or (i + 1 < len(name) and name[i + 1].islower())):
            out.append("_")
        out.append(ch.lower())
    return "".join(out)


def rust_enum_variants(src: str, enum_name: str) -> list[str]:
    m = re.search(rf"pub enum {enum_name} \{{(.*?)\n\}}", src, re.S)
    if not m:
        raise SystemExit(f"error: enum {enum_name} not found in {RUST}")
    names = re.findall(r"^\s{4}([A-Z][A-Za-z0-9]+)\b", m.group(1), re.M)
    return names


def ts_string_array(src: str, const_name: str) -> list[str]:
    m = re.search(rf"export const {const_name} = \[(.*?)] as const", src, re.S)
    if not m:
        raise SystemExit(f"error: {const_name} not found in {TS}")
    return re.findall(r'"([a-z0-9_]+)"', m.group(1))


def main() -> int:
    rust = RUST.read_text(encoding="utf-8")
    ts = TS.read_text(encoding="utf-8")

    rust_evs = [camel_to_snake(v) for v in rust_enum_variants(rust, "SdkEvent") if v != "Unknown"]
    rust_codes = [camel_to_snake(v) for v in rust_enum_variants(rust, "ErrorCode")]
    ts_evs = ts_string_array(ts, "KNOWN_EVS")
    ts_codes = ts_string_array(ts, "ERROR_CODES")

    failed = False
    for label, left, right in (
        ("SdkEvent ev", rust_evs, ts_evs),
        ("ErrorCode", rust_codes, ts_codes),
    ):
        missing = [x for x in left if x not in right]
        extra = [x for x in right if x not in left]
        if missing or extra:
            failed = True
            print(f"\n{label} drift (Rust vs TypeScript):")
            if missing:
                print(f"  missing in TS: {', '.join(missing)}")
            if extra:
                print(f"  extra in TS: {', '.join(extra)}")

    proto = re.search(r"pub const PROTOCOL_MAJOR: u32 = (\d+)", rust)
    ts_proto = re.search(r"export const PROTOCOL_MAJOR = (\d+)", ts)
    if not proto or not ts_proto or proto.group(1) != ts_proto.group(1):
        failed = True
        print("PROTOCOL_MAJOR mismatch between Rust and TypeScript")

    if failed:
        return 1
    print("sdk protocol: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
