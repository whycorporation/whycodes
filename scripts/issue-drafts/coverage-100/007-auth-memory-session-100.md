# test(auth+memory+session): whycodes-auth (9/17), whycodes-memory (9/12), whycodes-session (5/6) to 100%

Parent: #57. One PR, **separate commits per crate**.

## Problem
Service crates with overlapping storage/session patterns. Current lines: auth ~99.2%, memory ~87.7%, session ~100% (no floor for auth/memory; session has no `FLOORS` entry either). File ratios: auth 9/17, memory 9/12, session 5/6.

## Surfaces
- `crates/auth/src/*` (OAuth/PKCE/device-code, token store)
- `crates/memory/src/*` (store, embeddings; skip heavy ONNX unless already gated)
- `crates/session/src/*`

## Proposal
Tempdir + sqlite fixtures. Do not store secrets in tests. Each commit adds that crate to `FLOORS` at 100%.

## Acceptance
- [ ] `FLOORS` has all three at 100%.
- [ ] File-test ratio >= 80% each.
- [ ] `cargo test -p whycodes-auth -p whycodes-memory -p whycodes-session` green; coverage OK 100% each.

## Validation
```bash
cargo test -p whycodes-auth
cargo test -p whycodes-memory
cargo test -p whycodes-session
scripts/coverage.sh
```
