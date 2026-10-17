//! End-to-end tests for the embedded shell.
//!
//! `harness = false`: this binary is both the test runner and the shell host.
//! Bundled utilities re-execute the running binary as `__shell/<name>`, so
//! `main` dispatches those (and `__shell -c`) exactly as `whycodes` does
//! before it runs the cases. Unknown arguments (libtest flags such as
//! `--skip x` from the coverage wrapper) are ignored.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Output, Stdio};

fn main() -> ExitCode {
    // Every shell and utility process here is this binary, so coverage
    // counts them once each writes its profile. The coverage runtime marks
    // itself initialised in an inherited variable, and a child that sees it
    // never writes one: drop it. Return from `main` rather than call
    // `process::exit`, which on Windows skips the hook that writes it.
    // SAFETY: first statement of `main`, before any thread exists.
    unsafe { std::env::remove_var("__LLVM_PROFILE_RT_INIT_ONCE") };
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if let Some(code) = whycodes_shell::dispatch(&args) {
        return ExitCode::from(u8::try_from(code).unwrap_or(1));
    }
    let mut failed = 0;
    for (name, case) in CASES {
        let dir = tempfile::tempdir().expect("tempdir");
        match std::panic::catch_unwind(|| case(dir.path())) {
            Ok(()) => println!("test {name} ... ok"),
            Err(_) => {
                println!("test {name} ... FAILED");
                failed += 1;
            }
        }
    }
    println!("\n{} passed; {failed} failed", CASES.len() - failed);
    ExitCode::from(u8::from(failed > 0))
}

type Case = fn(&Path);

const CASES: &[(&str, Case)] = &[
    ("dash_c_pipes_and_status", dash_c_pipes_and_status),
    ("positional_parameters", positional_parameters),
    ("exit_codes", exit_codes),
    ("usage_errors", usage_errors),
    ("file_utilities", file_utilities),
    ("text_utilities", text_utilities),
    ("grep_flags", grep_flags),
    ("find_xargs_and_exec", find_xargs_and_exec),
    ("sed_and_diff", sed_and_diff),
    ("which_and_precedence", which_and_precedence),
    (
        "utility_errors_name_the_utility",
        utility_errors_name_the_utility,
    ),
    ("one_mebibyte_through_pipes", one_mebibyte_through_pipes),
    ("relative_external_after_cd", relative_external_after_cd),
    ("model_smoke_commands", model_smoke_commands),
    #[cfg(windows)]
    ("msys_drive_paths", msys_drive_paths),
    #[cfg(windows)]
    ("pathext_commands_spawn", pathext_commands_spawn),
];

fn exe() -> PathBuf {
    std::env::current_exe().expect("current_exe")
}

fn sh(dir: &Path, command: &str) -> Output {
    sh_args(dir, &["-c", command])
}

fn sh_args(dir: &Path, args: &[&str]) -> Output {
    Command::new(exe())
        .arg("__shell")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
        .expect("spawn shell")
}

fn ok(dir: &Path, command: &str) -> String {
    let out = sh(dir, command);
    let stdout = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
    assert!(
        out.status.success(),
        "`{command}` failed: {:?}\nstdout: {stdout}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

fn status(dir: &Path, command: &str) -> i32 {
    sh(dir, command).status.code().expect("exit code")
}

fn stderr(dir: &Path, command: &str) -> String {
    String::from_utf8_lossy(&sh(dir, command).stderr).into_owned()
}

fn write(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(path, body).expect("write");
}

fn dash_c_pipes_and_status(dir: &Path) {
    assert_eq!(ok(dir, r"printf 'a\nb\n' | wc -l && echo $?"), "2\n0\n");
    assert_eq!(ok(dir, "x=7; echo \"$x\" $((x * 6))"), "7 42\n");
    assert_eq!(
        ok(dir, "for f in a b; do echo $f; done | sort -r"),
        "b\na\n"
    );
}

fn positional_parameters(dir: &Path) {
    let out = sh_args(dir, &["-c", "echo $0 $1 $#", "name", "one"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "name one 1");
}

fn exit_codes(dir: &Path) {
    assert_eq!(status(dir, "exit 3"), 3);
    assert_eq!(status(dir, "false"), 1);
    assert_eq!(status(dir, "definitely-not-a-command-xyz"), 127);
}

fn usage_errors(dir: &Path) {
    assert_eq!(sh_args(dir, &[]).status.code(), Some(2));
    let unknown = Command::new(exe())
        .arg("__shell/no-such-utility")
        .output()
        .expect("spawn");
    assert_eq!(unknown.status.code(), Some(127));
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("unknown bundled command"));
    let not_ours = whycodes_shell::dispatch(&[OsString::from("run")]);
    assert_eq!(not_ours, None);
    assert_eq!(whycodes_shell::dispatch(&[]), None);
}

fn file_utilities(dir: &Path) {
    let out = ok(
        dir,
        "mkdir -p a/b && touch a/b/one.txt && cp a/b/one.txt two.txt && \
         mv two.txt three.txt && ls a/b && ls && rm three.txt && ls -la",
    );
    assert!(out.starts_with("one.txt\n"), "{out}");
    assert!(out.contains("three.txt"), "{out}");
    assert!(!dir.join("three.txt").exists());
    assert!(out.contains("a"), "{out}");
    assert_eq!(ok(dir, "rm -r a && ls"), "");
}

fn text_utilities(dir: &Path) {
    write(dir, "x.txt", "c\na\nb\na\n");
    assert_eq!(ok(dir, "cat x.txt | head -n 2"), "c\na\n");
    assert_eq!(ok(dir, "tail -n 1 x.txt"), "a\n");
    assert_eq!(
        ok(dir, "sort x.txt | uniq -c | tr -s ' ' | cut -d' ' -f2"),
        "2\n1\n1\n"
    );
    assert_eq!(ok(dir, "echo hello | tee copy.txt | tr a-z A-Z"), "HELLO\n");
    assert_eq!(ok(dir, "cat copy.txt"), "hello\n");
    assert_eq!(ok(dir, "wc -c < copy.txt"), "6\n");
    assert_eq!(ok(dir, "env FOO=bar env | grep ^FOO="), "FOO=bar\n");
    assert_eq!(
        ok(dir, "echo hi > out.txt; echo more >> out.txt; cat out.txt"),
        "hi\nmore\n"
    );
}

fn grep_flags(dir: &Path) {
    write(dir, "src/a.rs", "fn foo() {}\nlet bar = 1;\nfoo();\n");
    write(dir, "src/b.txt", "nothing\nFOO upper\n");
    write(dir, "bin.dat", "foo\0binary");
    assert_eq!(
        ok(dir, "grep -rn foo src"),
        "src/a.rs:1:fn foo() {}\nsrc/a.rs:3:foo();\n"
    );
    assert_eq!(
        ok(dir, "grep -rn foo --include='*.rs' ."),
        "./src/a.rs:1:fn foo() {}\n./src/a.rs:3:foo();\n"
    );
    assert_eq!(ok(dir, "cd src && grep -rli foo | sort"), "a.rs\nb.txt\n");
    assert_eq!(ok(dir, "grep -c foo src/a.rs"), "2\n");
    assert_eq!(ok(dir, "grep -v foo src/a.rs"), "let bar = 1;\n");
    assert_eq!(ok(dir, r"grep 'fn\|let' src/a.rs | wc -l"), "2\n");
    assert_eq!(ok(dir, "grep -E 'fn|let' -o src/a.rs"), "fn\nlet\n");
    assert_eq!(
        ok(dir, "grep -F '()' -h src/a.rs src/b.txt"),
        "fn foo() {}\nfoo();\n"
    );
    assert_eq!(ok(dir, "grep -w -x 'foo();' src/a.rs"), "foo();\n");
    assert_eq!(
        ok(dir, "grep -A1 -m1 fn src/a.rs"),
        "fn foo() {}\nlet bar = 1;\n"
    );
    assert_eq!(ok(dir, r"grep '\<bar\>' src/a.rs"), "let bar = 1;\n");
    assert_eq!(ok(dir, "grep -C1 bar src/a.rs | wc -l"), "3\n");
    assert_eq!(ok(dir, "grep -L foo src/*"), "src/b.txt\n");
    assert_eq!(
        ok(dir, "echo foo | grep -H foo -"),
        "(standard input):foo\n"
    );
    assert_eq!(ok(dir, "grep -q foo src/a.rs && echo yes"), "yes\n");
    // bin.dat is binary and src is excluded: nothing is selected.
    assert_eq!(status(dir, "grep -r foo --exclude-dir=src ."), 1);
    assert_eq!(ok(dir, "grep -a -c foo bin.dat"), "1\n");
    assert_eq!(status(dir, "grep -r --exclude='*.txt' -l FOO ."), 1);
    assert_eq!(status(dir, "grep zzz src/a.rs"), 1);
    assert_eq!(status(dir, "grep foo src"), 2);
    assert_eq!(status(dir, "grep -s foo missing.txt"), 2);
    assert_eq!(status(dir, "grep"), 2);
    assert_eq!(status(dir, "grep --no-such-flag x"), 2);
    // A bare paren is a literal in a basic regular expression.
    assert_eq!(ok(dir, "grep -c '(' src/a.rs"), "2\n");
    assert_eq!(status(dir, "grep -E '(' src/a.rs"), 2);
    assert_eq!(status(dir, "grep --include='[' foo src/a.rs"), 2);
    assert!(stderr(dir, "grep foo missing.txt").contains("grep: missing.txt"));
}

fn find_xargs_and_exec(dir: &Path) {
    write(dir, "a/one.rs", "needle\n");
    write(dir, "a/two.rs", "hay\n");
    write(dir, "b.md", "needle\n");
    assert_eq!(ok(dir, "find . -name '*.rs' | wc -l"), "2\n");
    assert_eq!(
        ok(
            dir,
            "find . -type f -name '*.rs' | sort | xargs grep -l needle"
        ),
        "./a/one.rs\n"
    );
    assert_eq!(
        ok(dir, "find a -name '*.rs' -exec grep -H needle {} +"),
        "a/one.rs:needle\n"
    );
    assert_eq!(ok(dir, "echo b.md | xargs -n 1 -- cat"), "needle\n");
    assert_eq!(ok(dir, "echo hi | xargs -I {} echo {}-x"), "hi-x\n");
    assert_eq!(ok(dir, "find . -name b.md -mmin -60"), "./b.md\n");
    // `-ok` asks first; stdin is closed, so the answer is no.
    assert_eq!(ok(dir, "find . -name b.md -ok cat {} ';'"), "");
}

fn sed_and_diff(dir: &Path) {
    write(dir, "f.txt", "one\ntwo\n");
    write(dir, "g.txt", "one\nthree\n");
    assert_eq!(ok(dir, "sed 's/one/1/' f.txt"), "1\ntwo\n");
    ok(dir, "sed -i 's/two/2/' f.txt");
    assert_eq!(
        std::fs::read_to_string(dir.join("f.txt")).expect("read"),
        "one\n2\n"
    );
    assert_eq!(ok(dir, "sed -n '/one/p' g.txt"), "one\n");
    assert_eq!(status(dir, "diff f.txt f.txt"), 0);
    assert_eq!(status(dir, "diff f.txt g.txt"), 1);
    let unified = sh(dir, "diff -u f.txt g.txt");
    let text = String::from_utf8_lossy(&unified.stdout);
    assert!(text.contains("-2\n+three"), "{text}");
    assert_eq!(
        ok(dir, "diff -q f.txt g.txt || true"),
        "Files f.txt and g.txt differ\n"
    );
    assert_eq!(
        ok(dir, "diff -s f.txt f.txt"),
        "Files f.txt and f.txt are identical\n"
    );
    assert_eq!(ok(dir, "diff -e f.txt g.txt || true"), "2c\nthree\n.\n");
    assert_eq!(status(dir, "diff -y f.txt g.txt >/dev/null"), 1);
    assert_eq!(status(dir, "diff -c f.txt g.txt >/dev/null"), 1);
    assert_eq!(ok(dir, "cat f.txt | diff - f.txt && echo same"), "same\n");
    assert_eq!(status(dir, "diff f.txt missing.txt"), 2);
    assert_eq!(status(dir, "diff --bogus f.txt g.txt"), 2);
}

fn which_and_precedence(dir: &Path) {
    let found = ok(dir, "which find sort");
    assert_eq!(
        found,
        "find: bundled with the whycodes embedded shell\n\
         sort: bundled with the whycodes embedded shell\n"
    );
    assert_eq!(status(dir, "which definitely-not-here-xyz"), 1);
    assert_eq!(status(dir, "which"), 1);
    assert!(ok(dir, "type grep").contains("builtin"));
    assert!(ok(dir, "help ls").contains("bundled with the whycodes embedded shell"));
    let bin = dir.join("bin");
    write(&bin, "tool.exe", "");
    write(&bin, "ls", "");
    let path = std::env::join_paths([bin.clone()]).expect("join");
    let all = whycodes_shell_which(&path, "ls");
    assert_eq!(all.len(), 2, "{all:?}");
    assert!(all[1].ends_with("bin/ls"), "{all:?}");
    // `find` resolves to the bundled one, not System32's find.exe.
    write(dir, "z.txt", "");
    assert_eq!(ok(dir, "find . -name z.txt"), "./z.txt\n");
    assert_eq!(ok(dir, "printf 'b\\na\\n' | sort"), "a\nb\n");
}

fn whycodes_shell_which(path: &std::ffi::OsStr, name: &str) -> Vec<String> {
    let out = Command::new(exe())
        .args(["__shell/which", "-a", name])
        .env("PATH", path)
        .output()
        .expect("spawn which");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

fn utility_errors_name_the_utility(dir: &Path) {
    let err = stderr(dir, "ls no-such-file");
    assert!(err.starts_with("ls: "), "{err}");
    assert_eq!(status(dir, "cat no-such-file"), 1);
}

fn one_mebibyte_through_pipes(dir: &Path) {
    let line = "x".repeat(1023);
    let body: String = (0..1024).map(|_| format!("{line}\n")).collect();
    write(dir, "big.txt", &body);
    assert_eq!(ok(dir, "cat big.txt | wc -c"), "1048576\n");
    assert_eq!(ok(dir, "cat big.txt | cat | head -n 2 | wc -l"), "2\n");
    assert_eq!(ok(dir, "cat big.txt | grep -c x"), "1024\n");
    assert_eq!(ok(dir, "cat big.txt 2>&1 | tail -n 1 | wc -c"), "1024\n");
}

fn relative_external_after_cd(dir: &Path) {
    let script = relative_script(dir);
    assert_eq!(ok(dir, &format!("cd sub && ./{script}")), "from-script\n");
    assert_eq!(
        ok(dir, "mkdir -p deep && cd deep && pwd | grep -c deep"),
        "1\n"
    );
}

#[cfg(windows)]
fn relative_script(dir: &Path) -> &'static str {
    write(dir, "sub/hello.cmd", "@echo from-script\r\n");
    "hello.cmd"
}

#[cfg(unix)]
fn relative_script(dir: &Path) -> &'static str {
    use std::os::unix::fs::PermissionsExt;
    write(dir, "sub/hello.sh", "#!/bin/sh\necho from-script\n");
    let path = dir.join("sub/hello.sh");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    "hello.sh"
}

/// Ten commands of the shape a model emits most.
fn model_smoke_commands(dir: &Path) {
    write(dir, "src/main.rs", "fn main() {\n    foo();\n}\n");
    write(dir, "src/lib.rs", "pub fn foo() {}\n");
    write(dir, "README.md", "# demo\nfoo bar\n");
    let commands = [
        "ls -la",
        "grep -rn foo .",
        "cat src/main.rs | head -n 2",
        "find . -name '*.rs' | wc -l",
        "wc -l src/*.rs",
        "grep -rl 'fn ' src | sort",
        "ls src && echo done",
        "tail -n 1 README.md",
        "sed -n '1,2p' src/main.rs",
        "mkdir -p out && echo ok > out/x && cat out/x",
    ];
    for command in commands {
        let out = ok(dir, command);
        assert!(!out.is_empty(), "`{command}` printed nothing");
    }
}

#[cfg(windows)]
fn msys_drive_paths(dir: &Path) {
    write(dir, "m.txt", "msys\n");
    let canonical = dir.canonicalize().expect("canonicalize");
    let text = canonical.to_string_lossy().replace('\\', "/");
    let text = text.trim_start_matches("//?/");
    let (drive, rest) = text.split_once(':').expect("drive letter");
    let msys = format!("/{}{rest}", drive.to_ascii_lowercase());
    assert_eq!(ok(dir, &format!("cat {msys}/m.txt")), "msys\n");
    assert_eq!(ok(dir, &format!("ls {msys}")), "m.txt\n");
    assert_eq!(ok(dir, &format!("grep -c msys {msys}/m.txt")), "1\n");
}

#[cfg(windows)]
fn pathext_commands_spawn(dir: &Path) {
    write(dir, "bin/tool.cmd", "@echo tool-ran %1\r\n");
    let bin = dir.join("bin");
    let mut path = OsString::from(&bin);
    path.push(";");
    path.push(std::env::var_os("PATH").unwrap_or_default());
    let out = Command::new(exe())
        .args(["__shell", "-c", "tool arg"])
        .env("PATH", &path)
        .current_dir(dir)
        .output()
        .expect("spawn");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "tool-ran arg",
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let which = whycodes_shell_which(&path, "tool");
    assert!(which[0].ends_with("bin/tool.cmd"), "{which:?}");
}
