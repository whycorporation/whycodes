//! `/terminal-setup`: what the terminal can deliver, and the tmux fix.
//!
//! Everything here is pure (env lookups and file text go in as arguments)
//! except [`apply_tmux_setup`], which the run loop calls only after the user
//! said yes in the confirm dialog.

use std::path::{Path, PathBuf};

/// The parts of the host terminal that decide which chords reach the TUI.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TerminalEnv {
    /// The startup probe found the kitty keyboard protocol and enabled it.
    pub keyboard_enhanced: bool,
    /// `$TMUX` is set.
    pub tmux: bool,
    /// `$TERM_PROGRAM` (`WezTerm`, `tmux`, `iTerm.app`, …), empty when unset.
    pub term_program: String,
    /// `$TERM`, empty when unset.
    pub term: String,
    /// `$WT_SESSION` is set (Windows Terminal).
    pub windows_terminal: bool,
    /// Built for Windows: the console reports every modifier without the
    /// kitty protocol.
    pub windows: bool,
}

impl TerminalEnv {
    /// Read the process environment.
    pub fn detect(keyboard_enhanced: bool) -> Self {
        Self::from_lookup(keyboard_enhanced, cfg!(windows), |k| std::env::var(k).ok())
    }

    pub fn from_lookup(
        keyboard_enhanced: bool,
        windows: bool,
        var: impl Fn(&str) -> Option<String>,
    ) -> Self {
        let set = |k: &str| var(k).is_some_and(|v| !v.is_empty());
        Self {
            keyboard_enhanced,
            tmux: set("TMUX"),
            term_program: var("TERM_PROGRAM").unwrap_or_default(),
            term: var("TERM").unwrap_or_default(),
            windows_terminal: set("WT_SESSION"),
            windows,
        }
    }

    /// Whether Ctrl/Shift on Enter, Tab, punctuation, … reach the TUI.
    pub fn modifiers_reported(&self) -> bool {
        self.keyboard_enhanced || self.windows
    }

    pub fn wezterm(&self) -> bool {
        self.term_program.eq_ignore_ascii_case("WezTerm")
    }
}

/// One line `/terminal-setup` adds to `~/.tmux.conf`.
pub struct TmuxSetting {
    pub line: &'static str,
    /// Option name; a non-comment line naming it counts as configured, so a
    /// deliberate `focus-events off` is left alone.
    option: &'static str,
    /// Extra word the line must contain (`extkeys` for `terminal-features`).
    needs: Option<&'static str>,
    pub why: &'static str,
}

pub const TMUX_SETTINGS: &[TmuxSetting] = &[
    TmuxSetting {
        line: "set -s extended-keys on",
        option: "extended-keys",
        needs: None,
        why: "pass Shift+Enter, Ctrl+Enter and other modified keys through",
    },
    TmuxSetting {
        line: "set -as terminal-features 'xterm*:extkeys'",
        option: "terminal-features",
        needs: Some("extkeys"),
        why: "tell tmux the outer terminal understands extended keys",
    },
    TmuxSetting {
        line: "set -g allow-passthrough on",
        option: "allow-passthrough",
        needs: None,
        why: "let clipboard and image escapes reach the outer terminal",
    },
    TmuxSetting {
        line: "set -g focus-events on",
        option: "focus-events",
        needs: None,
        why: "report focus in/out to the TUI",
    },
];

const TMUX_HEADER: &str = "# Added by whycodes /terminal-setup";

/// WezTerm keeps its config in Lua; print this instead of editing it.
pub const WEZTERM_LINE: &str = "config.enable_kitty_keyboard = true";

fn configured(text: &str, setting: &TmuxSetting) -> bool {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| l.starts_with("set"))
        .any(|l| {
            l.split_whitespace().any(|w| w == setting.option)
                && setting.needs.is_none_or(|n| l.contains(n))
        })
}

/// Settings from [`TMUX_SETTINGS`] that `text` does not configure yet.
pub fn tmux_missing(text: &str) -> Vec<&'static TmuxSetting> {
    TMUX_SETTINGS
        .iter()
        .filter(|s| !configured(text, s))
        .collect()
}

/// `current` with the missing settings appended, or `None` when every
/// setting is already there (a second run changes nothing).
pub fn tmux_conf_with_settings(current: &str) -> Option<String> {
    let missing = tmux_missing(current);
    if missing.is_empty() {
        return None;
    }
    let mut out = current.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(TMUX_HEADER);
    out.push('\n');
    for s in missing {
        out.push_str(s.line);
        out.push('\n');
    }
    Some(out)
}

/// `~/.tmux.conf`, the file `/terminal-setup` edits.
pub fn tmux_conf_path(home: &Path) -> PathBuf {
    home.join(".tmux.conf")
}

/// Text of every tmux config file tmux reads from `home`, so settings kept
/// in `~/.config/tmux/tmux.conf` count as configured too.
pub fn read_tmux_configs(home: &Path) -> String {
    [
        tmux_conf_path(home),
        home.join(".config").join("tmux").join("tmux.conf"),
    ]
    .iter()
    .filter_map(|p| std::fs::read_to_string(p).ok())
    .collect::<Vec<_>>()
    .join("\n")
}

/// The `/terminal-setup` report. `tmux_text` is [`read_tmux_configs`] when
/// inside tmux.
pub fn report(env: &TerminalEnv, tmux_text: Option<&str>, conf: Option<&Path>) -> String {
    let mut lines = vec!["Terminal setup".to_string()];
    let program = if env.term_program.is_empty() {
        "unknown"
    } else {
        &env.term_program
    };
    let term = if env.term.is_empty() {
        "unset"
    } else {
        &env.term
    };
    lines.push(format!("  terminal: {program} (TERM={term})"));
    lines.push(format!(
        "  kitty keyboard protocol: {}",
        if env.keyboard_enhanced {
            "on — Shift+Enter, Ctrl+Enter and Ctrl+punctuation reach whycodes"
        } else if env.windows {
            "off — the Windows console reports modifiers anyway"
        } else {
            "off — Shift+Enter, Ctrl+Enter, Ctrl+Tab and Ctrl+punctuation may not arrive (Alt+Enter still inserts a newline)"
        }
    ));
    lines.push(format!(
        "  inside tmux: {}",
        if env.tmux { "yes" } else { "no" }
    ));
    if env.tmux {
        let text = tmux_text.unwrap_or("");
        let missing = tmux_missing(text);
        let at = conf.map_or_else(|| "~/.tmux.conf".into(), |p| p.display().to_string());
        if missing.is_empty() {
            lines.push(format!("  tmux config ({at}): already configured"));
        } else {
            lines.push(format!(
                "  tmux config ({at}): {} of {} settings missing",
                missing.len(),
                TMUX_SETTINGS.len()
            ));
            for s in missing {
                lines.push(format!("    {}  — {}", s.line, s.why));
            }
        }
    }
    if env.wezterm() && !env.keyboard_enhanced {
        lines.push("  WezTerm: add this line to ~/.wezterm.lua and restart WezTerm:".into());
        lines.push(format!("    {WEZTERM_LINE}"));
    }
    lines.join("\n")
}

/// Outcome of [`apply_tmux_setup`].
#[derive(Debug, PartialEq, Eq)]
pub enum TmuxApply {
    AlreadyConfigured,
    /// Settings appended; `backup` is the `.bak` copy when the file existed.
    Written {
        added: usize,
        backup: Option<PathBuf>,
    },
}

/// Append the missing settings to `conf`, copying it to `<conf>.bak` first.
/// Call only after the user confirmed.
pub fn apply_tmux_setup(conf: &Path) -> std::io::Result<TmuxApply> {
    let current = match std::fs::read_to_string(conf) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    let text = current.as_deref().unwrap_or("");
    let added = tmux_missing(text).len();
    let Some(next) = tmux_conf_with_settings(text) else {
        return Ok(TmuxApply::AlreadyConfigured);
    };
    let backup = match current {
        Some(_) => {
            let mut name = conf.as_os_str().to_owned();
            name.push(".bak");
            let bak = PathBuf::from(name);
            std::fs::copy(conf, &bak)?;
            Some(bak)
        }
        None => None,
    };
    std::fs::write(conf, next)?;
    Ok(TmuxApply::Written { added, backup })
}

/// Status text for an [`apply_tmux_setup`] result.
pub fn apply_message(conf: &Path, result: &std::io::Result<TmuxApply>) -> String {
    match result {
        Ok(TmuxApply::AlreadyConfigured) => format!("{} is already configured", conf.display()),
        Ok(TmuxApply::Written { added, backup }) => {
            let bak = backup
                .as_ref()
                .map(|b| format!(" (backup: {})", b.display()))
                .unwrap_or_default();
            format!(
                "Added {added} setting(s) to {}{bak}. Reload with: tmux source-file {}",
                conf.display(),
                conf.display()
            )
        }
        Err(e) => format!("Could not update {}: {e}", conf.display()),
    }
}

#[cfg(test)]
#[path = "terminal_setup_tests.rs"]
mod tests;
