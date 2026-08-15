#!/usr/bin/env node
/** Attach to a running `whycode serve` and list sessions. */
import { WhycodeClient } from "../dist/index.js";

const addr = process.argv[2] ?? "127.0.0.1:3030";
const client = await WhycodeClient.connect(addr);
const health = await client.health();
console.log(`protocol=${health.protocol} version=${health.version} project=${health.project}`);
for (const s of await client.listSessions()) {
  console.log(`  ${s.id}  ${s.title}`);
}
