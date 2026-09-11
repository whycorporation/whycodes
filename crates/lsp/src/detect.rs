//! Binary lookup, cwd-only root markers, and `file://` URIs.

use std::path::{Path, PathBuf};

/// Strip a leading dot and lowercase an extension (`".Rs"` → `"rs"`).
pub fn normalize_ext(ext: &str) -> String {
    ext.trim_start_matches('.').to_ascii_lowercase()
}

/// LSP `file://` URI for a filesystem path (Windows drive letters included).
pub fn file_uri(path: impl AsRef<Path>) -> String {
    file_uri_from_lossy(&path.as_ref().to_string_lossy())
}

pub(crate) fn file_uri_from_lossy(raw: &str) -> String {
    let unix = raw.replace('\\', "/");
    if unix.starts_with("//") {
        // UNC: //server/share → file://server/share
        return format!("file:{unix}");
    }
    // Drive letter: C:/foo → file:///C:/foo
    let bytes = unix.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return format!("file:///{unix}");
    }
    if unix.starts_with('/') {
        format!("file://{unix}")
    } else {
        format!("file:///{unix}")
    }
}

/// Reverse of [`file_uri`] for `didOpen` fallbacks.
pub fn path_from_file_uri(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    #[cfg(windows)]
    {
        let trimmed = rest.trim_start_matches('/');
        if trimmed.len() >= 2 {
            let b = trimmed.as_bytes();
            if b[0].is_ascii_alphabetic() && b[1] == b':' {
                return Some(PathBuf::from(trimmed.replace('/', "\\")));
            }
        }
        if rest.starts_with("//") {
            return Some(PathBuf::from(rest.replace('/', "\\")));
        }
        Some(PathBuf::from(rest.replace('/', "\\")))
    }
    #[cfg(not(windows))]
    {
        if rest.starts_with('/') {
            Some(PathBuf::from(rest))
        } else {
            Some(PathBuf::from(format!("/{rest}")))
        }
    }
}

/// True when `cwd` itself contains at least one marker (no parent walk).
///
/// Empty `markers` always matches. `*.cabal`-style globs match names in `cwd`.
pub fn root_markers_match(cwd: &Path, markers: &[String]) -> bool {
    if markers.is_empty() {
        return true;
    }
    markers.iter().any(|m| marker_in_cwd(cwd, m))
}

fn marker_in_cwd(cwd: &Path, marker: &str) -> bool {
    if marker.contains('*') {
        let Ok(entries) = std::fs::read_dir(cwd) else {
            return false;
        };
        entries
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .any(|name| wildcard_match(&name, marker))
    } else {
        cwd.join(marker).exists()
    }
}

fn wildcard_match(name: &str, pattern: &str) -> bool {
    if let Some((pre, suf)) = pattern.split_once('*')
        && !pre.contains('*')
        && !suf.contains('*')
    {
        return name.starts_with(pre) && name.ends_with(suf) && name.len() >= pre.len() + suf.len();
    }
    name == pattern
}

/// Resolve `command` to an executable: absolute path, project-local bins, then PATH.
pub fn resolve_command(cwd: &Path, command: &str) -> Option<PathBuf> {
    let given = Path::new(command);
    if given.is_absolute() {
        return is_executable(given).then(|| given.to_path_buf());
    }
    let joined = cwd.join(given);
    if given.components().count() > 1 {
        return is_executable(&joined).then_some(joined);
    }
    if is_executable(&joined) {
        return Some(joined);
    }
    for dir in local_bin_dirs(cwd) {
        if let Some(found) = lookup_in_dir(&dir, command) {
            return Some(found);
        }
    }
    which_command(command)
}

fn local_bin_dirs(cwd: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![cwd.join("node_modules").join(".bin"), cwd.join("bin")];
    #[cfg(windows)]
    {
        dirs.push(cwd.join(".venv").join("Scripts"));
        dirs.push(cwd.join("venv").join("Scripts"));
    }
    #[cfg(not(windows))]
    {
        dirs.push(cwd.join(".venv").join("bin"));
        dirs.push(cwd.join("venv").join("bin"));
    }
    dirs
}

fn lookup_in_dir(dir: &Path, command: &str) -> Option<PathBuf> {
    lookup_in_dir_with_exts(dir, command, &pathext())
}

fn lookup_in_dir_with_exts(dir: &Path, command: &str, exts: &[String]) -> Option<PathBuf> {
    let direct = dir.join(command);
    if is_executable(&direct) {
        return Some(direct);
    }
    for ext in exts {
        let candidate = dir.join(format!("{command}{ext}"));
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// `command` on `PATH` (does not shell out to `which`).
pub fn which_command(command: &str) -> Option<PathBuf> {
    which_command_in(std::env::var_os("PATH"), command)
}

fn which_command_in(path_os: Option<std::ffi::OsString>, command: &str) -> Option<PathBuf> {
    let path_os = path_os?;
    for dir in std::env::split_paths(&path_os) {
        if let Some(found) = lookup_in_dir(&dir, command) {
            return Some(found);
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn pathext() -> Vec<String> {
    #[cfg(windows)]
    {
        parse_pathext(std::env::var("PATHEXT").ok())
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

#[cfg(any(windows, test))]
fn parse_pathext(value: Option<String>) -> Vec<String> {
    value
        .unwrap_or_else(|| ".EXE;.CMD;.BAT;.COM".into())
        .split(';')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
#[path = "detect_tests.rs"]
mod tests;
