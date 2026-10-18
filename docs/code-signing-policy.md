# Code signing policy

Free code signing provided by [SignPath.io](https://about.signpath.io), certificate by [SignPath Foundation](https://signpath.org).

Windows release binaries of whycodes (`whycodes.exe` in the `x86_64-pc-windows-msvc` release archive) are signed. Every signed file is built by GitHub Actions from the public source at [github.com/whycorporation/whycodes](https://github.com/whycorporation/whycodes), using the release workflow in that repository; nothing built elsewhere is signed. Each signing request is approved by hand before the signature is applied. Checksums in `SHA256SUMS` are computed after signing.

## Team roles

| Role | Members |
|---|---|
| Committers and reviewers | [mrnasil](https://github.com/mrnasil), [ugorur](https://github.com/ugorur) |
| Approvers | [mrnasil](https://github.com/mrnasil) |

- **Committers** may change the source code directly.
- **Reviewers** review every change from contributors outside the team before it is merged.
- **Approvers** approve each signing request.

All team members use multi-factor authentication for GitHub and SignPath.

## Privacy policy

whycodes does not collect telemetry and does not transfer information to other networked systems unless the user asks for it:

- **Model providers.** Prompts, the files and command output the agent works with, and tool results are sent to the model provider the user configures (for example a hosted API or a local model server), under that provider's own privacy policy.
- **Features the user turns on or invokes:** sign-in to a provider, web search and web fetch, GitHub issue and pull request tools, MCP servers, notifications (Discord, Telegram, webhooks). Each sends data only to the service the user configured.
- **Update check.** When an interactive session starts, whycodes asks the GitHub releases API whether a newer version exists and asks before installing it. Turn the check off with `--no-auto-update`, `WHYCODES_NO_AUTO_UPDATE=1`, or `auto_update = false` under `[general]` in the config file.

Nothing is sent to whycorporation.
