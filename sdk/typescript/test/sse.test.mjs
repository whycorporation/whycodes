import assert from "node:assert/strict";
import { test } from "node:test";
import { decodeSsePayload, popSseData } from "../dist/sse.js";
import { parseSdkEvent } from "../dist/types.js";

test("popSseData skips keepalive and yields data frames", () => {
  const buf = { text: ': ping\n\ndata: {"ev":"cancelled"}\n\n' };
  assert.deepEqual(popSseData(buf), ['{"ev":"cancelled"}']);
});

test("unknown ev is forward compatible", () => {
  const ev = parseSdkEvent({ ev: "future_thing", x: 1 });
  assert.equal(ev.ev, "unknown");
});

test("text_delta parses", () => {
  const ev = decodeSsePayload('{"ev":"text_delta","text":"hi"}');
  assert.deepEqual(ev, { ev: "text_delta", text: "hi" });
});

test("bad json becomes unknown", () => {
  const ev = decodeSsePayload("not-json");
  assert.equal(ev.ev, "unknown");
});
