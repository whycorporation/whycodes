# Security Policy

## Reporting a vulnerability

Please do **not** open a public issue for security problems.

Use GitHub's private reporting channel:
[github.com/whycorporation/whycode/security/advisories/new](https://github.com/whycorporation/whycode/security/advisories/new)

Include enough detail to reproduce: whycode version (`whycode --version`), OS,
configuration (redact credentials), and the input or sequence that triggers
the problem. You can expect an acknowledgement within a few days; fixes ship
in the next release after the report is confirmed.

## Supported versions

Only the latest tagged release receives security fixes. `whycode upgrade`
moves you to it.

## Scope

whycode is an agent: it runs shell commands and edits files **by design**, on
behalf of a model whose output is not trusted input. The following are treated
as security vulnerabilities:

- Bypassing the shell command risk classification
  (`safe`/`caution`/`destructive`/`catastrophic`) so that a command executes
  in a lower tier than intended, or a `catastrophic` command executes at all
- Escaping the OS sandbox (`crates/sandbox`) from a sandboxed shell invocation
- Circumventing `allow`/`ask`/`deny` tool permissions or the HTTP domain
  allowlist from model-controlled input
- Reading, exfiltrating or weakening the storage of credentials under the
  whycode data directory (API keys, OAuth tokens — stored `0600`, symlink
  refused)
- The self-update path installing a binary whose checksum does not match the
  release's `SHA256SUMS`

Out of scope: a model producing unwanted but correctly-gated actions (that is
a model behaviour, not a whycode vulnerability — tune `bash_risk_threshold`
and permissions), and issues in third-party LLM providers themselves.
