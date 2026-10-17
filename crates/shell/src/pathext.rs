//! Windows: run `npm`, `npx` and other `PATHEXT` scripts by bare name.
//!
//! brush 0.5 finds `npm.cmd` for `npm` but then spawns the extension-less
//! path; Windows only appends `.exe` itself, so every `.cmd`/`.bat` on `PATH`
//! failed as "command not found" (upstream #1317/#1452, fixed after 0.5.0).
//! Before the command runs, each bare word in it that resolves to a script
//! (and not to an `.exe`) is put in the shell's command hash with its full
//! name. A word that is not a command only costs a few `stat` calls.

use std::ffi::OsStr;
use std::path::PathBuf;

/// Extensions Windows does not add by itself when spawning.
const SCRIPT_EXTENSIONS: [&str; 3] = [".cmd", ".bat", ".com"];

/// Bare command-like words of `command` (no path separators, assignments or
/// options), the ones brush would look up on `PATH`.
pub(crate) fn candidate_words(command: &str) -> Vec<&str> {
    let mut words: Vec<&str> = command
        .split(|c: char| c.is_whitespace() || "|&;()<>`$\"'{}".contains(c))
        .filter(|word| {
            !word.is_empty()
                && word
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.+".contains(c))
                && !word.starts_with('-')
                && !crate::is_bundled(word)
        })
        .collect();
    words.sort_unstable();
    words.dedup();
    words
}

/// Full path of the script `name` resolves to on `path`, when the first hit
/// in `PATH` order is a script rather than an `.exe`.
pub(crate) fn resolve_script(name: &str, path: &OsStr) -> Option<PathBuf> {
    for dir in std::env::split_paths(path) {
        if dir.join(format!("{name}.exe")).is_file() {
            return None;
        }
        if let Some(script) = SCRIPT_EXTENSIONS
            .iter()
            .map(|ext| dir.join(format!("{name}{ext}")))
            .find(|candidate| candidate.is_file())
        {
            return Some(script);
        }
    }
    None
}

/// Seed the shell's command hash for the scripts `command` names.
pub(crate) fn seed(shell: &mut brush_core::Shell, command: &str, path: &OsStr) {
    for word in candidate_words(command) {
        if let Some(script) = resolve_script(word, path) {
            shell.program_location_cache_mut().set(word, script);
        }
    }
}
