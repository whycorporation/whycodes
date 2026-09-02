# test(server/sdk/lsp/mcp): whycodes-server, whycodes-sdk, whycodes-lsp, whycodes-mcp to 100%

Parent: #57. Grouped HTTP/protocol crates. One PR, **separate commits per crate**.

## Problem
Approximate lines: server 79.1%, sdk 80.4%, lsp 64.1%, mcp 80.8%. File counts: server 5, sdk 3, lsp 5, mcp 7. None have a `FLOORS` entry. `lsp` is among the lowest in the workspace.

## Surfaces
- `crates/server/src/*` (`/api/*`, `/v1/*`)
- `crates/sdk/src/*` (protocol v1 client)
- `crates/lsp/src/{client,tool,types,error}.rs`
- `crates/mcp/src/*`

## Proposal
Hyper/axum test server or existing SDK integration tests. LSP: fake JSON-RPC. MCP: in-process transport. Each crate commit adds `FLOORS` 100%.

## Acceptance
- [ ] All four crates in `FLOORS` at 100%.
- [ ] File-test ratio >= 80% each.
- [ ] `python scripts/check_sdk_protocol.py` still green if protocol types are touched.
- [ ] Coverage OK 100% each.

## Validation
```bash
cargo test -p whycodes-server -p whycodes-sdk -p whycodes-lsp -p whycodes-mcp
python scripts/check_sdk_protocol.py
scripts/coverage.sh
```
