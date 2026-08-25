#!/usr/bin/env node
/** Attach to a running `whycodes serve` and list sessions. */
import { WhyCodesClient } from "../dist/index.js";

const addr = process.argv[2] ?? "127.0.0.1:3030";
const client = await WhyCodesClient.connect(addr);
const health = await client.health();
console.log(`protocol=${health.protocol} version=${health.version} project=${health.project}`);
for (const s of await client.listSessions()) {
  console.log(`  ${s.id}  ${s.title}`);
}
