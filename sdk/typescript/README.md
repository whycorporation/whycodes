# @whycorporation/whycode-sdk

Thin TypeScript client for [`whycode serve`](../../docs/guide.md). Same protocol v1 as the Rust `whycode-sdk` crate. Does **not** embed the agent loop.

Requires Node 18+. Zero runtime dependencies.

```bash
npm install @whycorporation/whycode-sdk
# daemon
whycode serve
```

```ts
import { WhycodeClient } from "@whycorporation/whycode-sdk";

const client = await WhycodeClient.connect("127.0.0.1:3030");
const session = await client.createSession();
const turn = await client.run(session.id, "summarize this repo");
console.log(turn.text);
await client.close();
```

`WhycodeClient.launch()` spawns a private `whycode serve` (inherits env / API keys). Branch on `SdkError.code`. Unknown `ev` values become `{ ev: "unknown" }`.

Events: `text_delta`, `tool_start`, `tool_end`, `turn_done`, … — see `KNOWN_EVS`.
