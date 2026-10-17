//! `which [-a] NAME...`: bundled utilities first, then `PATH` (with
//! `PATHEXT` on Windows), the order the embedded shell resolves them in.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// Exit status 0 when every name was found, 1 otherwise.
pub(crate) fn main(args: Vec<OsString>) -> i32 {
    let mut all = false;
    let mut names = Vec::new();
    for arg in args.into_iter().skip(1) {
        if arg == "-a" {
            all = true;
        } else {
            names.push(arg);
        }
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    let exts = path_extensions();
    let mut missing = names.is_empty();
    for name in names {
        let found = resolve(&name, &path, &exts, all);
        missing |= found.is_empty();
        for line in found {
            println!("{line}");
        }
    }
    i32::from(missing)
}

/// Every place `name` resolves to, in shell order; only the first unless `all`.
pub(crate) fn resolve(name: &OsStr, path: &OsStr, exts: &[String], all: bool) -> Vec<String> {
    let mut found = Vec::new();
    if name.to_str().is_some_and(crate::is_bundled) {
        found.push(format!(
            "{}: bundled with the whycodes embedded shell",
            name.to_string_lossy()
        ));
    }
    for dir in std::env::split_paths(path) {
        for candidate in candidates(&dir, name, exts) {
            if candidate.is_file() {
                found.push(candidate.to_string_lossy().replace('\\', "/"));
                break;
            }
        }
    }
    if !all {
        found.truncate(1);
    }
    found
}

fn candidates(dir: &Path, name: &OsStr, exts: &[String]) -> Vec<PathBuf> {
    // `PATHEXT` first: `npm` resolves to `npm.cmd`, not the sh script beside it.
    let base = dir.join(name);
    let mut out: Vec<PathBuf> = exts
        .iter()
        .map(|ext| {
            let mut with_ext = base.clone().into_os_string();
            with_ext.push(ext);
            PathBuf::from(with_ext)
        })
        .collect();
    out.push(base);
    out
}

#[cfg(windows)]
fn path_extensions() -> Vec<String> {
    executable_extensions(std::env::var_os("PATHEXT").as_deref())
}

#[cfg(not(windows))]
fn path_extensions() -> Vec<String> {
    Vec::new()
}

/// `PATHEXT` entries (`.EXE;.CMD`), lowercased.
#[cfg(any(windows, test))]
pub(crate) fn executable_extensions(pathext: Option<&OsStr>) -> Vec<String> {
    pathext
        .map(|v| v.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_string())
        .split(';')
        .filter(|ext| !ext.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}
