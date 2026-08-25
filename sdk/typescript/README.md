# @whycorporation/whycodes-sdk

Thin TypeScript client for [`whycodes serve`](../../docs/guide.md). Same protocol v1 as the Rust `whycodes-sdk` crate. Does **not** embed the agent loop.

Requires Node 18+. Zero runtime dependencies.

Not on the public npm registry yet. From a clone:

```bash
cd sdk/typescript && npm ci && npm run build
# or npm pack / npm publish when the @whycorporation scope is ready
# daemon
whycodes serve
```

```ts
import { WhyCodesClient } from "@whycorporation/whycodes-sdk";

const client = await WhyCodesClient.connect("127.0.0.1:3030");
const session = await client.createSession();
const turn = await client.run(session.id, "summarize this repo");
console.log(turn.text);
await client.close();
```

`WhyCodesClient.launch()` spawns a private `whycodes serve`. Pass
`inheritLogins: false` for a private `WHYCODES_HOME` (no user API keys).
`getHistory` / `peek`, `listModels` / `setModel`, `renameSession` /
`rewind` / `compact` are first-class. Branch on `SdkError.code`.
Unknown `ev` values become `{ ev: "unknown" }`.

`run()` auto-approves tool `Ask`s. `runEvents()` emits `permission_request`; answer with `respondToPermission(sessionId, requestId, "allow" | "allow_always" | "deny")`.

`runStructured(sessionId, prompt, schema)` retries until the reply matches a JSON Schema subset (`type`, `required`, `properties`).

Events: `text_delta`, `tool_start`, `permission_request`, `turn_done`, … — see `KNOWN_EVS`.
