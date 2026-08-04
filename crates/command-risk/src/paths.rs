//! Path classification: what a command argument would touch.
//!
//! The catastrophic tier here is deliberately a small, absolute set of
//! path checks. It does not depend on understanding the command, because a
//! command that defeats the parser must still not be able to delete a home
//! directory.

use std::path::{Component, Path, PathBuf};

/// What a path argument means for the blast radius of a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathScope {
    /// Inside the project directory.
    InProject,
    /// A real path outside the project directory.
    Outside,
    /// A protected system or home location.
    Catastrophic,
}

/// The user's home directory, from the environment.
///
/// Read from the environment rather than a crate so this stays dependency
/// free, and so tests can override it.
pub fn home_dir() -> Option<PathBuf> {
    for var in ["HOME", "USERPROFILE"] {
        if let Ok(v) = std::env::var(var)
            && !v.is_empty()
        {
            return Some(PathBuf::from(v));
        }
    }
    // Windows fallback: HOMEDRIVE + HOMEPATH
    match (std::env::var("HOMEDRIVE"), std::env::var("HOMEPATH")) {
        (Ok(d), Ok(p)) if !d.is_empty() && !p.is_empty() => Some(PathBuf::from(format!("{d}{p}"))),
        _ => None,
    }
}

/// True for the null device: writing to it destroys nothing, whatever the
/// redirect operator. `NUL` is the Windows spelling.
///
/// Regression: jcode#738/#709 — `echo hi 2>/dev/null` was gated because the
/// redirect target lives under the protected `/dev` tree. Gating the most
/// common stderr idiom in shell trains users to disable the gate.
pub fn is_null_device(raw: &str) -> bool {
    let s = raw.trim();
    s == "/dev/null" || s.eq_ignore_ascii_case("nul")
}

/// Absolute paths that must never be a destructive target, whatever the
/// command. Matching is on the path itself, not on the command name.
const PROTECTED_PREFIXES: &[&str] = &[
    "/",
    "/bin",
    "/boot",
    "/dev",
    "/etc",
    "/lib",
    "/opt",
    "/proc",
    "/root",
    "/sbin",
    "/srv",
    "/sys",
    "/usr",
    "/var",
    "/Applications",
    "/Library",
    "/System",
    "/Users",
    "/Volumes",
];

/// Windows equivalents. Written with forward slashes because comparison is
/// done on the normalised form, which converts separators.
const PROTECTED_PREFIXES_WINDOWS: &[&str] = &[
    "c:/windows",
    "c:/program files",
    "c:/program files (x86)",
    "c:/programdata",
    "c:/users",
];

/// Expand the parts of a path we can resolve without running a shell.
///
/// `~` and `$HOME`/`%USERPROFILE%` become the home directory. Every other
/// variable is left alone — an unexpanded `$VAR` is reported by
/// [`has_unresolved_variable`] so the caller can escalate rather than assume.
pub fn expand(raw: &str, home: Option<&Path>) -> String {
    let mut s = raw.to_string();

    if let Some(home) = home {
        let home_str = home.to_string_lossy().to_string();
        if s == "~" {
            return home_str;
        }
        for prefix in ["~/", "~\\"] {
            if let Some(rest) = s.strip_prefix(prefix) {
                return format!("{}/{}", home_str.trim_end_matches(['/', '\\']), rest);
            }
        }
        for var in ["$HOME", "${HOME}", "%USERPROFILE%", "$env:USERPROFILE"] {
            if s.contains(var) {
                s = s.replace(var, &home_str);
            }
        }
    }
    s
}

/// True when a path still contains a shell variable after expansion, so its
/// real target is unknown.
pub fn has_unresolved_variable(path: &str) -> bool {
    let bytes: Vec<char> = path.chars().collect();
    for (i, c) in bytes.iter().enumerate() {
        if *c == '$' && i + 1 < bytes.len() {
            let next = bytes[i + 1];
            if next.is_ascii_alphanumeric() || next == '_' || next == '{' {
                return true;
            }
        }
        if *c == '%' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_alphabetic() {
            return true;
        }
    }
    false
}

/// Normalise a path for comparison: absolute, forward slashes, no trailing
/// separator, `.` and `..` resolved lexically.
fn normalize(path: &Path, base: &Path) -> PathBuf {
    let joined = if path.is_absolute() || is_windows_absolute(path) {
        path.to_path_buf()
    } else {
        base.join(path)
    };

    let mut out = PathBuf::new();
    for comp in joined.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `C:\foo` is absolute on Windows but not recognised as such when the test
/// runs on Unix, and vice versa. Detect the drive-letter form explicitly so
/// classification does not depend on the host.
fn is_windows_absolute(path: &Path) -> bool {
    let s = path.to_string_lossy();
    let b: Vec<char> = s.chars().collect();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == ':' && (b[2] == '\\' || b[2] == '/')
}

fn as_comparable(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    let trimmed = s.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

/// True when `path` is exactly `prefix` or the whole of it.
fn is_at_or_above(path: &str, prefix: &str) -> bool {
    path.eq_ignore_ascii_case(prefix)
}

/// Classify what a path argument would affect.
///
/// `project` is the working directory of the tool call; `home` is the user's
/// home directory, passed in so tests do not depend on the real environment.
pub fn classify(raw: &str, project: &Path, home: Option<&Path>) -> PathScope {
    let expanded = expand(raw, home);

    // A variable we cannot expand could be anything, so it is certainly not
    // known to be inside the project. It is not `Catastrophic` either: that
    // tier refuses without a prompt, and refusing a legitimate
    // `rm -rf $BUILD_DIR` outright is worse than asking.
    if has_unresolved_variable(&expanded) {
        return PathScope::Outside;
    }

    // A glob stands for everything under its parent, so `/*` is `/`.
    let expanded = strip_glob_tail(&expanded);

    let path = PathBuf::from(&expanded);
    let normalized = normalize(&path, project);
    let cmp = as_comparable(&normalized);

    // The home directory itself, but not a path inside it.
    if let Some(home) = home {
        let home_cmp = as_comparable(home);
        if is_at_or_above(&cmp, &home_cmp) {
            return PathScope::Catastrophic;
        }
    }

    for prefix in PROTECTED_PREFIXES {
        if is_at_or_above(&cmp, prefix) {
            return PathScope::Catastrophic;
        }
    }
    for prefix in PROTECTED_PREFIXES_WINDOWS {
        if is_at_or_above(&cmp.to_ascii_lowercase(), prefix) {
            return PathScope::Catastrophic;
        }
    }

    // A bare drive root, e.g. `C:\` or `C:`.
    let lower = cmp.to_ascii_lowercase();
    let lb: Vec<char> = lower.chars().collect();
    if lb.len() <= 3 && lb.len() >= 2 && lb[0].is_ascii_alphabetic() && lb[1] == ':' {
        return PathScope::Catastrophic;
    }

    let project_cmp = as_comparable(project);
    if cmp.eq_ignore_ascii_case(&project_cmp)
        || cmp
            .to_ascii_lowercase()
            .starts_with(&format!("{}/", project_cmp.to_ascii_lowercase()))
    {
        PathScope::InProject
    } else {
        PathScope::Outside
    }
}

/// Drop a trailing glob component, so a path is judged by the directory the
/// glob expands within: `/*` is `/`, `~/*` is `~`, `target/*` is `target`.
fn strip_glob_tail(path: &str) -> String {
    let last = path.rsplit(['/', '\\']).next().unwrap_or("");
    if !last.is_empty() && last.chars().all(|c| matches!(c, '*' | '?' | '.')) {
        let cut = path.len() - last.len();
        let head = path[..cut].trim_end_matches(['/', '\\']);
        return if head.is_empty() {
            path[..cut].to_string()
        } else {
            head.to_string()
        };
    }
    path.to_string()
}

/// True when the argument looks like a path rather than a flag.
pub fn looks_like_path(word: &str) -> bool {
    !word.is_empty() && !word.starts_with('-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> PathBuf {
        PathBuf::from("/work/proj")
    }
    fn home() -> PathBuf {
        PathBuf::from("/home/user")
    }
    fn c(raw: &str) -> PathScope {
        classify(raw, &project(), Some(&home()))
    }

    #[test]
    fn project_paths_are_in_project() {
        assert_eq!(c("build"), PathScope::InProject);
        assert_eq!(c("./target"), PathScope::InProject);
        assert_eq!(c("src/main.rs"), PathScope::InProject);
        assert_eq!(c("/work/proj/target"), PathScope::InProject);
        assert_eq!(c("/work/proj"), PathScope::InProject);
    }

    #[test]
    fn escaping_the_project_is_outside() {
        assert_eq!(c("../sibling"), PathScope::Outside);
        assert_eq!(c("/work/other"), PathScope::Outside);
        assert_eq!(c("/tmp/scratch"), PathScope::Outside);
    }

    #[test]
    fn home_itself_is_catastrophic() {
        assert_eq!(c("~"), PathScope::Catastrophic);
        assert_eq!(c("$HOME"), PathScope::Catastrophic);
        assert_eq!(c("${HOME}"), PathScope::Catastrophic);
        assert_eq!(c("/home/user"), PathScope::Catastrophic);
        assert_eq!(c("/home/user/"), PathScope::Catastrophic);
    }

    #[test]
    fn paths_inside_home_are_merely_outside() {
        assert_eq!(c("~/Documents"), PathScope::Outside);
        assert_eq!(c("/home/user/Documents"), PathScope::Outside);
    }

    #[test]
    fn root_and_system_paths_are_catastrophic() {
        for p in ["/", "/etc", "/usr", "/System", "/dev", "/var", "/Users"] {
            assert_eq!(c(p), PathScope::Catastrophic, "{p}");
        }
    }

    #[test]
    fn windows_system_paths_are_catastrophic() {
        for p in [
            r"C:\Windows",
            r"c:\windows",
            r"C:\Program Files",
            r"C:\Users",
        ] {
            assert_eq!(c(p), PathScope::Catastrophic, "{p}");
        }
    }

    #[test]
    fn drive_roots_are_catastrophic() {
        assert_eq!(c(r"C:\"), PathScope::Catastrophic);
        assert_eq!(c("D:/"), PathScope::Catastrophic);
    }

    #[test]
    fn traversal_back_to_root_is_catastrophic() {
        assert_eq!(c("../../.."), PathScope::Catastrophic);
        assert_eq!(c("/work/proj/../../.."), PathScope::Catastrophic);
    }

    #[test]
    fn unresolved_variables_are_outside_not_catastrophic() {
        // Not knowable, so certainly not in-project — but promptable, because
        // `rm -rf $BUILD_DIR` is a legitimate thing to want to approve.
        assert_eq!(c("$TARGET"), PathScope::Outside);
        assert_eq!(c("$PREFIX/lib"), PathScope::Outside);
        assert_eq!(c("%APPDATA%"), PathScope::Outside);
    }

    #[test]
    fn a_glob_is_judged_by_the_directory_it_expands_in() {
        assert_eq!(c("/*"), PathScope::Catastrophic);
        assert_eq!(c("~/*"), PathScope::Catastrophic);
        assert_eq!(c("/etc/*"), PathScope::Catastrophic);
        assert_eq!(c("target/*"), PathScope::InProject);
        assert_eq!(c("/tmp/*"), PathScope::Outside);
        // A glob in the middle is left alone.
        assert_eq!(c("src/*/mod.rs"), PathScope::InProject);
    }

    #[test]
    fn a_project_subpath_named_like_a_system_path_stays_in_project() {
        assert_eq!(c("/work/proj/etc"), PathScope::InProject);
        assert_eq!(c("etc"), PathScope::InProject);
    }

    #[test]
    fn system_prefix_children_are_not_catastrophic_by_prefix_alone() {
        // Deleting /etc is catastrophic; deleting one file under it is merely
        // outside the project, and the command tier decides what that means.
        assert_eq!(c("/etc/hosts"), PathScope::Outside);
        assert_eq!(c("/usr/local/bin/tool"), PathScope::Outside);
    }

    #[test]
    fn expand_leaves_unknown_variables_alone() {
        assert_eq!(expand("$FOO/bar", Some(&home())), "$FOO/bar");
        assert_eq!(expand("~/x", Some(&home())), "/home/user/x");
        assert_eq!(expand("~", Some(&home())), "/home/user");
    }

    #[test]
    fn detects_unresolved_variables() {
        assert!(has_unresolved_variable("$FOO"));
        assert!(has_unresolved_variable("${FOO}"));
        assert!(has_unresolved_variable("%APPDATA%"));
        assert!(!has_unresolved_variable("/plain/path"));
        assert!(!has_unresolved_variable("100%"));
    }

    #[test]
    fn null_device_is_recognised() {
        assert!(is_null_device("/dev/null"));
        assert!(is_null_device("NUL"));
        assert!(is_null_device("nul"));
        assert!(!is_null_device("/dev/sda"));
        assert!(!is_null_device("out.txt"));
    }

    #[test]
    fn flags_are_not_paths() {
        assert!(!looks_like_path("-rf"));
        assert!(!looks_like_path("--force"));
        assert!(looks_like_path("build"));
        assert!(looks_like_path("/tmp"));
    }
}
