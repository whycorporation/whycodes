//! `/keys`: the active key bindings and the ones that cannot work here.
//!
//! A binding can be dead for reasons the keymap cannot see: two config keys
//! that spell the same chord, a chord the terminal only sends with the kitty
//! keyboard protocol, a chord the OS / terminal / tmux keeps for itself, or
//! a session key the run loop handles before the keymap.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use crossterm::event::{KeyCode, KeyModifiers};

use crate::app::FocusPane;
use crate::keymap::{Action, Chord, Keymap, KeymapContext, action_from_name, action_name};
use crate::terminal_setup::TerminalEnv;

/// One binding that will not behave as configured.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct KeyIssue {
    pub key: String,
    pub problem: String,
}

impl KeyIssue {
    fn new(key: impl Into<String>, problem: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            problem: problem.into(),
        }
    }
}

/// Chords the run loop handles before the keymap (`idle_loop_key_action`),
/// so `[tui.key_bindings]` cannot take them over.
const SESSION_KEYS: &[(KeyCode, &str)] = &[
    (KeyCode::Char('t'), "cycle primary agent (when idle)"),
    (KeyCode::Char('n'), "new parallel session"),
    (KeyCode::Char('o'), "session dashboard"),
    (KeyCode::Tab, "most recent session"),
    (KeyCode::PageUp, "previous session"),
    (KeyCode::PageDown, "next session"),
];

const NEEDS_PROTOCOL: &str =
    "needs the kitty keyboard protocol; this terminal sends it as another key";

fn session_key(chord: Chord) -> Option<&'static str> {
    if !chord.mods().contains(KeyModifiers::CONTROL) {
        return None;
    }
    SESSION_KEYS
        .iter()
        .find(|(code, _)| *code == chord.code())
        .map(|(_, what)| *what)
}

/// Whether a terminal without the kitty protocol sends `chord` as itself.
fn legacy_deliverable(chord: Chord) -> bool {
    let mods = chord.mods();
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let shift = mods.contains(KeyModifiers::SHIFT);
    match chord.code() {
        KeyCode::Tab => !ctrl,
        KeyCode::Enter | KeyCode::Esc | KeyCode::Backspace => !ctrl && !shift,
        KeyCode::Char(' ') => true,
        // Ctrl+I is Tab and Ctrl+M is Enter on the wire; Ctrl+Shift+X is Ctrl+X.
        KeyCode::Char(c) if ctrl && c.is_ascii_alphabetic() => !shift && c != 'i' && c != 'm',
        KeyCode::Char(_) => !ctrl,
        _ => true,
    }
}

fn captured_by(chord: Chord, env: &TerminalEnv) -> Option<&'static str> {
    let mods = chord.mods();
    let ctrl_shift = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
    match chord.code() {
        KeyCode::Char('t' | 'w' | 'n' | 'p') if env.windows_terminal && mods == ctrl_shift => {
            Some("Windows Terminal keeps it (tabs, panes, command palette)")
        }
        KeyCode::Char('b') if env.tmux && mods == KeyModifiers::CONTROL => {
            Some("tmux prefix; tmux keeps it")
        }
        _ => None,
    }
}

/// Why `chord` will not reach its action in this terminal.
pub fn chord_problems(chord: Chord, env: &TerminalEnv) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(what) = session_key(chord) {
        out.push(format!("taken by the session keys ({what})"));
    }
    if !env.modifiers_reported() && !legacy_deliverable(chord) {
        out.push(NEEDS_PROTOCOL.to_string());
    }
    if let Some(by) = captured_by(chord, env) {
        out.push(by.to_string());
    }
    out
}

/// A Cmd / Super / Win modifier: the OS or terminal keeps those.
fn os_modifier(spec: &str) -> bool {
    spec.split(['+', '-']).any(|m| {
        matches!(
            m.trim().to_ascii_lowercase().as_str(),
            "cmd" | "command" | "super" | "win" | "windows"
        )
    })
}

fn unbinds(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "none" | "unbind" | ""
    )
}

/// Problems with `[tui.key_bindings]` entries alone (what the startup
/// notice reports).
pub fn configured_issues(bindings: &HashMap<String, String>, env: &TerminalEnv) -> Vec<KeyIssue> {
    let mut issues = BTreeSet::new();
    let mut by_chord: BTreeMap<String, (Chord, Vec<(String, String)>)> = BTreeMap::new();
    let sorted: BTreeMap<&String, &String> = bindings.iter().collect();
    for (spec, name) in sorted {
        let Some(chord) = Chord::parse(spec) else {
            let problem = if os_modifier(spec) {
                "Cmd / Super / Win never reach a terminal app"
            } else {
                "unknown key; ignored"
            };
            issues.insert(KeyIssue::new(spec.as_str(), problem));
            continue;
        };
        if !unbinds(name) && action_from_name(name).is_none() {
            issues.insert(KeyIssue::new(
                spec.as_str(),
                format!("unknown action `{name}`; ignored"),
            ));
            continue;
        }
        by_chord
            .entry(chord.label())
            .or_insert_with(|| (chord, Vec::new()))
            .1
            .push((spec.clone(), name.trim().to_ascii_lowercase()));
    }
    for (label, (chord, entries)) in by_chord {
        let actions: BTreeSet<&str> = entries.iter().map(|(_, a)| a.as_str()).collect();
        if actions.len() > 1 {
            let listed: Vec<String> = entries.iter().map(|(s, a)| format!("{s} = {a}")).collect();
            issues.insert(KeyIssue::new(
                label.as_str(),
                format!("bound twice ({}); only one applies", listed.join(", ")),
            ));
        }
        if entries.iter().all(|(_, a)| unbinds(a)) {
            continue;
        }
        for problem in chord_problems(chord, env) {
            issues.insert(KeyIssue::new(label.as_str(), problem));
        }
    }
    issues.into_iter().collect()
}

fn candidate_chords() -> Vec<Chord> {
    let mut codes: Vec<KeyCode> = ('a'..='z')
        .chain('0'..='9')
        .chain(" .,[]/;'-=`\\?".chars())
        .map(KeyCode::Char)
        .collect();
    codes.extend([
        KeyCode::Enter,
        KeyCode::Tab,
        KeyCode::Esc,
        KeyCode::Backspace,
        KeyCode::Delete,
        KeyCode::Insert,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::PageUp,
        KeyCode::PageDown,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Left,
        KeyCode::Right,
    ]);
    codes.extend((1..=12).map(KeyCode::F));
    let mods = [
        KeyModifiers::NONE,
        KeyModifiers::CONTROL,
        KeyModifiers::ALT,
        KeyModifiers::SHIFT,
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    ];
    codes
        .iter()
        .flat_map(|code| {
            mods.iter().filter_map(move |m| {
                let spec = format!("{}{}", mod_prefix(*m), code_spec(*code));
                Chord::parse(&spec)
            })
        })
        .collect()
}

fn mod_prefix(mods: KeyModifiers) -> String {
    let mut out = String::new();
    if mods.contains(KeyModifiers::CONTROL) {
        out.push_str("ctrl+");
    }
    if mods.contains(KeyModifiers::SHIFT) {
        out.push_str("shift+");
    }
    if mods.contains(KeyModifiers::ALT) {
        out.push_str("alt+");
    }
    out
}

fn code_spec(code: KeyCode) -> String {
    match code {
        KeyCode::Char(' ') => "space".into(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::F(n) => format!("f{n}"),
        other => format!("{other:?}").to_ascii_lowercase(),
    }
}

fn cell(action: Option<Action>) -> String {
    action.map_or_else(|| "—".to_string(), action_name)
}

/// The `/keys` listing: every active binding, then what will not work here.
pub fn keys_report(
    keymap: &Keymap,
    bindings: &HashMap<String, String>,
    env: &TerminalEnv,
) -> String {
    let custom: BTreeSet<String> = bindings
        .keys()
        .filter_map(|k| Chord::parse(k))
        .map(|c| c.label())
        .collect();
    let mut chords: BTreeMap<String, Chord> = candidate_chords()
        .into_iter()
        .chain(bindings.keys().filter_map(|k| Chord::parse(k)))
        .map(|c| (c.label(), c))
        .collect();
    chords.retain(|_, c| session_key(*c).is_none());

    let mut lines = vec![
        "Key bindings (* = [tui.key_bindings])".to_string(),
        format!("  {:<21} {:<22} {}", "key", "prompt", "scrollback"),
    ];
    let mut issues: BTreeSet<KeyIssue> = BTreeSet::new();
    let resolve = |c: Chord| {
        let ev = c.to_event();
        (
            keymap.resolve(KeymapContext::Normal, FocusPane::Prompt, &ev),
            keymap.resolve(KeymapContext::Normal, FocusPane::Scrollback, &ev),
        )
    };
    for (label, chord) in &chords {
        let (prompt, scroll) = resolve(*chord);
        if prompt.is_none() && scroll.is_none() {
            continue;
        }
        let is_custom = custom.contains(label);
        // Most default arms ignore Alt and Shift; list Alt+X / Shift+X only
        // when it does something X does not.
        let same_as_without = |m: &str| {
            let plain = label.replace(m, "");
            plain != *label && Chord::parse(&plain).is_some_and(|c| resolve(c) == (prompt, scroll))
        };
        if !is_custom && (same_as_without("alt+") || same_as_without("shift+")) {
            continue;
        }
        let mark = if is_custom { "*" } else { "" };
        lines.push(format!(
            "  {:<21} {:<22} {}",
            format!("{label}{mark}"),
            cell(prompt),
            cell(scroll)
        ));
        for problem in chord_problems(*chord, env) {
            issues.insert(KeyIssue::new(label.as_str(), problem));
        }
    }
    lines.push("  Session keys (handled before [tui.key_bindings]):".to_string());
    for (code, what) in SESSION_KEYS {
        let chord = Chord::parse(&format!("ctrl+{}", code_spec(*code)));
        if let Some(chord) = chord {
            lines.push(format!("  {:<21} {what}", chord.label()));
            for problem in chord_problems(chord, env)
                .into_iter()
                .filter(|p| !p.starts_with("taken by"))
            {
                issues.insert(KeyIssue::new(chord.label(), problem));
            }
        }
    }
    issues.extend(configured_issues(bindings, env));
    if issues.is_empty() {
        lines.push("No conflicts.".to_string());
    } else {
        lines.push("Conflicts:".to_string());
        for issue in &issues {
            lines.push(format!("  {:<21} {}", issue.key, issue.problem));
        }
        if !env.modifiers_reported() {
            lines
                .push("  /terminal-setup shows how to turn the kitty keyboard protocol on.".into());
        }
    }
    lines.join("\n")
}

/// FNV-1a over the issue list: stable across builds, unlike `DefaultHasher`.
pub fn issues_hash(issues: &[KeyIssue]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for issue in issues {
        for b in issue
            .key
            .bytes()
            .chain(*b"\t")
            .chain(issue.problem.bytes())
            .chain(*b"\n")
        {
            hash ^= u64::from(b);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}

const HASH_FILE: &str = "key-conflicts.hash";

/// Status-line notice for configured conflicts, shown once per conflict set:
/// the hash of the last set seen lives in `dir`.
pub fn startup_notice(issues: &[KeyIssue], dir: &Path) -> Option<String> {
    let path = dir.join(HASH_FILE);
    let hash = issues_hash(issues).to_string();
    let previous = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => {
            tracing::debug!(error = %e, "no key conflict hash yet");
            String::new()
        }
    };
    if previous.trim() == hash {
        return None;
    }
    if let Err(e) = std::fs::write(&path, &hash) {
        tracing::debug!(error = %e, path = %path.display(), "key conflict hash not saved");
    }
    let first = issues.first()?;
    let more = match issues.len() {
        1 => String::new(),
        n => format!(" (+{} more)", n - 1),
    };
    Some(format!(
        "Key binding {}: {}{more} — /keys for details",
        first.key, first.problem
    ))
}

#[cfg(test)]
#[path = "key_audit_tests.rs"]
mod tests;
