# Unofficial auth plugins

These directories are **not** part of the WhyCodes product. They are not
installed by default, not listed in `whycodes plugins`, and not published
by Why Corporation.

Each folder is a `kind: "auth"` plugin that registers a third-party OAuth
client (Claude Code, Codex CLI, Gemini CLI, VS Code Copilot, Grok Build,
Antigravity hub). Using them may violate that provider's terms.

Copy a folder into `~/.config/com.whycorporation.whycodes/plugins/` or
`<project>/.whycodes/plugins/` only if you choose to. WhyCodes will then
load it as a local plugin.

Do not treat this tree as an official marketplace.
