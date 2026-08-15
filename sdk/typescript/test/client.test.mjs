import assert from "node:assert/strict";
import { test } from "node:test";
import {
  ERROR_CODES,
  KNOWN_EVS,
  PROTOCOL_MAJOR,
  extractJson,
  normalizeBase,
  parseHandshake,
  validateInstance,
} from "../dist/index.js";

test("normalizeBase adds scheme and strips slash", () => {
  assert.equal(normalizeBase("127.0.0.1:3030"), "http://127.0.0.1:3030");
  assert.equal(normalizeBase("http://localhost:3030/"), "http://localhost:3030");
});

test("protocol major is 1", () => {
  assert.equal(PROTOCOL_MAJOR, 1);
  const hs = parseHandshake({
    protocol: 1,
    version: "0.1.0",
    healthy: true,
    project: "/tmp",
    uptime_secs: 1,
    sessions_in_memory: 0,
  });
  assert.equal(hs?.protocol, 1);
});

test("error codes and known evs are non-empty", () => {
  assert.ok(ERROR_CODES.includes("unknown_session"));
  assert.ok(ERROR_CODES.includes("structured_output_invalid"));
  assert.ok(KNOWN_EVS.includes("text_delta"));
  assert.ok(KNOWN_EVS.includes("permission_request"));
  assert.ok(KNOWN_EVS.includes("question_request"));
});

test("extractJson and validateInstance", () => {
  assert.deepEqual(extractJson("```json\n{\"a\":1}\n```"), { a: 1 });
  const schema = {
    type: "object",
    required: ["name"],
    properties: { name: { type: "string" } },
  };
  assert.deepEqual(validateInstance(schema, { name: "x" }), []);
  assert.ok(validateInstance(schema, {}).some((e) => e.includes("missing required name")));
});
