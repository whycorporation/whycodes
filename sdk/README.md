# SDKs

Language clients for [`whycode serve`](../docs/guide.md). They speak
protocol v1 (`SdkEvent`); they do not embed the agent loop.

| Path | Language | Notes |
|---|---|---|
| [`typescript/`](typescript/) | TypeScript | npm package `@whycorporation/whycode-sdk` |
| [`../crates/sdk`](../crates/sdk) | Rust | Workspace crate `whycode-sdk` |

Keep the event tags in lockstep: `crates/protocol/src/sdk.rs` and
`sdk/typescript/src/types.ts` are checked by
`python scripts/check_sdk_protocol.py`.
