# Examples

Small, complete files to copy into a project. A CLI test loads each one with
the same code paths WhyCodes uses at runtime, so the examples cannot rot.

| Example | Copy to | Loaded by |
|---|---|---|
| [plugins/hello/](plugins/hello/) | `<project>/.whycodes/plugins/hello/` or `~/.whycodes/plugins/hello/` | Plugin discovery (`plugin.json` → tool `plugin_hello`) |
| [commands/test.md](commands/test.md) | `<project>/.whycodes/commands/test.md` | Custom slash command loader (`/test unit`) |
| [config/hooks.toml](config/hooks.toml) | the `[[hooks]]` tables into `config.toml` | Config parser (`[[hooks]]`) |

The reference for each format is in the [user guide](../guide.md):
[Shell plugins](../guide.md#shell-plugins),
[Custom commands](../guide.md#custom-commands),
[Hooks](../guide.md#hooks); every key is listed in [config.md](../config.md).

`run.sh` needs a POSIX `sh` on `PATH` (Git for Windows provides one); the
plugin mechanism itself does not.
