//! Unit tests for the platform-independent pieces. The shell itself is
//! exercised end to end in `tests/e2e.rs`.

use std::ffi::OsString;
use std::io::Write;

use crate::bundled::{SlashWriter, gnu_in_place, rewrite_find_exec, rewrite_xargs_command};
use crate::grep::to_rust_regex;
use crate::msys::{to_windows, translate_args};
use crate::pathext::{candidate_words, resolve_script};
use crate::which::{executable_extensions, resolve};

fn os(words: &[&str]) -> Vec<OsString> {
    words.iter().map(OsString::from).collect()
}

fn strings(words: &[OsString]) -> Vec<String> {
    words
        .iter()
        .map(|w| w.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn basic_regex_operators_become_rust_regex() {
    assert_eq!(to_rust_regex(r"foo\|bar", true), "foo|bar");
    assert_eq!(to_rust_regex(r"\(a\)\{2\}\+\?", true), "(a){2}+?");
    assert_eq!(to_rust_regex("f(x)|y{1}+?", true), r"f\(x\)\|y\{1\}\+\?");
    assert_eq!(to_rust_regex(r"\<w\>", true), r"\bw\b");
    assert_eq!(to_rust_regex(r"\<w\>", false), r"\bw\b");
    assert_eq!(to_rust_regex(r"a\.b", true), r"a\.b");
    assert_eq!(to_rust_regex(r"(a|b)+", false), "(a|b)+");
    assert_eq!(to_rust_regex(r"\(", false), r"\(");
    assert_eq!(to_rust_regex("tail\\", true), r"tail\\");
}

#[test]
fn msys_drive_paths_need_an_existing_drive() {
    fn yes(_: char) -> bool {
        true
    }
    fn no(_: char) -> bool {
        false
    }
    assert_eq!(
        to_windows("/c/Users/me", yes).as_deref(),
        Some("C:/Users/me")
    );
    assert_eq!(to_windows("/d", yes).as_deref(), Some("D:/"));
    assert_eq!(to_windows("/c/Users", no), None);
    assert_eq!(to_windows("/cc/x", yes), None);
    assert_eq!(to_windows("/1/x", yes), None);
    assert_eq!(to_windows("c/x", yes), None);
    assert_eq!(to_windows("/", yes), None);
}

#[test]
fn only_path_utilities_get_translated_arguments() {
    let args = os(&["/no-such-drive-path", "x"]);
    assert_eq!(translate_args("ls", &args), args);
    assert_eq!(translate_args("sed", &args), args);
    assert_eq!(
        crate::msys::translate_path(std::ffi::OsStr::new("plain")),
        "plain"
    );
}

#[cfg(not(windows))]
#[test]
fn drive_paths_stay_unix_paths_off_windows() {
    let path = std::ffi::OsStr::new("/c/Users");
    assert_eq!(crate::msys::translate_path(path), path);
}

#[test]
fn find_exec_and_xargs_point_bundled_commands_at_this_binary() {
    let exe = std::env::current_exe()
        .expect("current_exe")
        .to_string_lossy()
        .into_owned();
    let find = strings(&rewrite_find_exec(os(&[
        "find", ".", "-exec", "grep", "-l", "x", "{}", "+", "-exec", "python", "{}", ";",
    ])));
    assert_eq!(
        find,
        [
            "find",
            ".",
            "-exec",
            &exe,
            "__shell/grep",
            "-l",
            "x",
            "{}",
            "+",
            "-exec",
            "python",
            "{}",
            ";"
        ]
    );

    let xargs = strings(&rewrite_xargs_command(os(&[
        "xargs", "-n", "1", "-0", "grep", "x",
    ])));
    assert_eq!(xargs, ["xargs", "-n", "1", "-0", &exe, "__shell/grep", "x"]);
    let dashdash = strings(&rewrite_xargs_command(os(&["xargs", "--", "cat"])));
    assert_eq!(dashdash, ["xargs", "--", &exe, "__shell/cat"]);
    let external = strings(&rewrite_xargs_command(os(&["xargs", "python", "-c", "1"])));
    assert_eq!(external, ["xargs", "python", "-c", "1"]);
    let bare = strings(&rewrite_xargs_command(os(&["xargs"])));
    assert_eq!(bare, ["xargs"]);
}

#[test]
fn sed_in_place_never_swallows_the_script() {
    let args = strings(&gnu_in_place(os(&[
        "sed",
        "-i",
        "s/a/b/",
        "f",
        "--in-place",
        "-i.bak",
    ])));
    assert_eq!(
        args,
        ["sed", "--in-place=", "s/a/b/", "f", "--in-place=", "-i.bak"]
    );
}

#[test]
fn slash_writer_converts_only_when_asked() {
    let mut on = SlashWriter {
        inner: Vec::new(),
        convert: true,
    };
    assert_eq!(on.write(br".\a\b.rs").expect("write"), 8);
    on.flush().expect("flush");
    assert_eq!(on.inner, b"./a/b.rs");
    let mut off = SlashWriter {
        inner: Vec::new(),
        convert: false,
    };
    off.write_all(br"a\b").expect("write");
    assert_eq!(off.inner, br"a\b");
}

#[test]
fn which_lists_bundled_then_path_hits() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("tool.cmd"), "").expect("write");
    std::fs::write(dir.path().join("plain"), "").expect("write");
    let path = std::env::join_paths([dir.path()]).expect("join");
    let exts = executable_extensions(Some(std::ffi::OsStr::new(".EXE;;.CMD")));
    assert_eq!(exts, [".exe", ".cmd"]);
    assert_eq!(
        executable_extensions(None),
        [".com", ".exe", ".bat", ".cmd"]
    );

    let tool = resolve("tool".as_ref(), &path, &exts, false);
    assert_eq!(tool.len(), 1);
    assert!(tool[0].ends_with("/tool.cmd"), "{tool:?}");
    let plain = resolve("plain".as_ref(), &path, &[], true);
    assert!(plain[0].ends_with("/plain"), "{plain:?}");
    let ls = resolve("ls".as_ref(), &path, &exts, true);
    assert_eq!(ls, ["ls: bundled with the whycodes embedded shell"]);
    assert!(resolve("absent".as_ref(), &path, &exts, true).is_empty());
}

#[test]
fn pathext_seeding_finds_scripts_but_not_exes() {
    let words = candidate_words("npm run build && FOO=1 npx -y x | grep y; ./local.cmd \"npm\"");
    assert_eq!(words, ["build", "npm", "npx", "run", "x", "y"]);

    let first = tempfile::tempdir().expect("tempdir");
    let second = tempfile::tempdir().expect("tempdir");
    std::fs::write(first.path().join("npm.cmd"), "").expect("write");
    std::fs::write(first.path().join("git.exe"), "").expect("write");
    std::fs::write(second.path().join("git.cmd"), "").expect("write");
    let path = std::env::join_paths([first.path(), second.path()]).expect("join");
    let npm = resolve_script("npm", &path).expect("npm.cmd");
    assert!(npm.ends_with("npm.cmd"), "{npm:?}");
    assert_eq!(resolve_script("git", &path), None);
    assert_eq!(resolve_script("absent", &path), None);
}

#[test]
fn pathext_seed_fills_the_command_hash() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("tool.cmd"), "").expect("write");
    let path = std::env::join_paths([dir.path()]).expect("join");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");
    let mut shell = runtime
        .block_on(brush_core::Shell::builder().build())
        .expect("shell");
    crate::pathext::seed(&mut shell, "tool && other", &path);
    let hashed = shell.program_location_cache().get("tool").expect("hashed");
    assert!(hashed.ends_with("tool.cmd"), "{hashed:?}");
    assert_eq!(shell.program_location_cache().get("other"), None);
}

#[test]
fn dispatch_recognises_only_shell_invocations() {
    assert_eq!(crate::dispatch(&os(&["run"])), None);
    assert_eq!(crate::dispatch(&[]), None);
    assert_eq!(crate::dispatch(&os(&["__shell"])), Some(2));
    assert_eq!(crate::dispatch(&os(&["__shell/not-bundled"])), Some(127));
    assert!(crate::is_bundled("grep"));
    assert!(!crate::is_bundled("bash"));
}

#[test]
fn shell_errors_are_reported_with_status_one() {
    assert_eq!(crate::run::report(Ok(4)), 4);
    assert_eq!(crate::run::report(Err("boom".into())), 1);
    assert_eq!(crate::run::run_dash_c("exit 5", &[]), Ok(5));
}
