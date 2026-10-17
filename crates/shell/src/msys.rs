//! MSYS-style drive paths (`/c/Users/...`) on Windows.
//!
//! Git Bash rewrites `/c/...` to `C:/...` for every program it starts. The
//! embedded shell only does it where a word is certainly a path: the
//! operands of the bundled file utilities, `grep`'s file operands, and `cd`.
//! A word is rewritten only when the drive exists, so `/a` on a machine
//! without `A:` stays a path on the current drive. Script arguments such as
//! `sed '/c/d'` and words passed to external programs are never touched.

use std::ffi::{OsStr, OsString};

/// Bundled utilities whose operands are paths.
const PATH_UTILITIES: &[&str] = &[
    "cat", "cp", "diff", "find", "head", "ls", "mkdir", "mv", "rm", "sort", "tail", "tee", "touch",
    "uniq", "wc",
];

/// Arguments for bundled `name`, with drive paths rewritten on Windows.
pub(crate) fn translate_args(name: &str, args: &[OsString]) -> Vec<OsString> {
    if PATH_UTILITIES.contains(&name) {
        args.iter().map(|arg| translate_path(arg)).collect()
    } else {
        args.to_vec()
    }
}

/// One word, rewritten when it is an MSYS drive path for an existing drive.
pub(crate) fn translate_path(arg: &OsStr) -> OsString {
    arg.to_str()
        .and_then(|text| to_windows(text, drive_exists))
        .map_or_else(|| arg.to_os_string(), OsString::from)
}

/// `/c` and `/c/rest` to `C:/` and `C:/rest` when `exists('c')`.
pub(crate) fn to_windows(arg: &str, exists: fn(char) -> bool) -> Option<String> {
    let rest = arg.strip_prefix('/')?;
    let mut chars = rest.chars();
    let drive = chars.next().filter(char::is_ascii_alphabetic)?;
    let tail = chars.as_str();
    if !(tail.is_empty() || tail.starts_with('/')) || !exists(drive) {
        return None;
    }
    let tail = if tail.is_empty() { "/" } else { tail };
    Some(format!("{}:{tail}", drive.to_ascii_uppercase()))
}

#[cfg(windows)]
fn drive_exists(drive: char) -> bool {
    std::path::Path::new(&format!("{drive}:\\")).is_dir()
}

/// Off Windows `/c/...` is an ordinary absolute path.
#[cfg(not(windows))]
fn drive_exists(_drive: char) -> bool {
    false
}
