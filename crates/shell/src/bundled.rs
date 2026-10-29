//! Bundled utilities: registry, shell shims and the child-process entry.
//!
//! The shell registers one builtin per bundled name. Running it re-executes
//! the host binary as `whycodes __shell/<name> <args...>` through brush's
//! external-command path, so redirections and pipes are real OS handles and
//! the utility runs (and may panic) in its own process. Builtins win over
//! `PATH`, which keeps `find` and `sort` away from the DOS programs.

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

use brush_core::builtins::{BoxFuture, ContentOptions, ContentType, Registration};
use brush_core::commands::{self, CommandArg, ExecutionContext};
use brush_core::extensions::ShellExtensions;

/// Entry point of a bundled utility: `argv[0]` is its name.
pub(crate) type BundledFn = fn(Vec<OsString>) -> i32;

/// Wrap a uutils `uumain` the way `uucore::bin!` does, minus `exit`.
macro_rules! uutil {
    ($name:literal, $krate:ident) => {
        ($name, {
            fn run(args: Vec<OsString>) -> i32 {
                prepare_uutil(stringify!($krate));
                let code = $krate::uumain(args.into_iter());
                flush_stdout();
                code
            }
            run as BundledFn
        })
    };
}

/// Every bundled utility, by the name the shell resolves.
const BUNDLED: &[(&str, BundledFn)] = &[
    uutil!("cat", uu_cat),
    uutil!("cp", uu_cp),
    uutil!("cut", uu_cut),
    ("diff", crate::diff::main),
    uutil!("env", uu_env),
    ("find", find),
    ("grep", crate::grep::main),
    uutil!("head", uu_head),
    uutil!("ls", uu_ls),
    uutil!("mkdir", uu_mkdir),
    uutil!("mv", uu_mv),
    uutil!("rm", uu_rm),
    ("sed", sed),
    uutil!("sort", uu_sort),
    uutil!("tail", uu_tail),
    uutil!("tee", uu_tee),
    uutil!("touch", uu_touch),
    uutil!("tr", uu_tr),
    uutil!("uniq", uu_uniq),
    uutil!("wc", uu_wc),
    ("which", crate::which::main),
    ("xargs", xargs),
];

/// True when `name` is a bundled utility (it resolves before `PATH`).
pub fn is_bundled(name: &str) -> bool {
    lookup(name).is_some()
}

fn lookup(name: &str) -> Option<BundledFn> {
    BUNDLED
        .iter()
        .find(|(bundled, _)| *bundled == name)
        .map(|(_, run)| *run)
}

/// Child-process entry: run bundled `name` with `args` (no `argv[0]`).
pub(crate) fn invoke(name: &str, args: &[OsString]) -> i32 {
    let Some(run) = lookup(name) else {
        eprintln!("whycodes shell: unknown bundled command: {name}");
        return 127;
    };
    let mut argv = Vec::with_capacity(args.len() + 1);
    argv.push(OsString::from(name));
    argv.extend(crate::msys::translate_args(name, args));
    run(argv)
}

/// First half of `uucore::bin!`: utility name from `argv[1]`, quiet broken
/// pipes, and the Fluent bundle (without it errors print raw message keys).
fn prepare_uutil(util_crate: &str) {
    uucore::set_utility_is_second_arg();
    uucore::panic::mute_sigpipe_panic();
    let name = uucore::get_canonical_util_name(util_crate);
    if let Err(err) = uucore::locale::setup_localization(name) {
        eprintln!("{name}: cannot load messages: {err}");
    }
}

fn flush_stdout() {
    if let Err(err) = std::io::stdout().flush() {
        eprintln!("whycodes shell: flushing stdout: {err}");
    }
}

fn sed(args: Vec<OsString>) -> i32 {
    // sed names itself from argv[1] too; it ships no Fluent bundle.
    uucore::set_utility_is_second_arg();
    uucore::panic::mute_sigpipe_panic();
    let code = sed::sed::uumain(gnu_in_place(args).into_iter());
    flush_stdout();
    code
}

/// GNU `sed -i` takes no separate value (`-i.bak` attaches it); uutils sed
/// would read `sed -i 's/a/b/' f` as suffix `s/a/b/` and script `f`.
pub(crate) fn gnu_in_place(args: Vec<OsString>) -> Vec<OsString> {
    args.into_iter()
        .map(|arg| {
            if arg == "-i" || arg == "--in-place" {
                OsString::from("--in-place=")
            } else {
                arg
            }
        })
        .collect()
}

fn find(args: Vec<OsString>) -> i32 {
    let args = lossy(rewrite_find_exec(args));
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    findutils::find::find_main(&refs, &FindDeps::new())
}

/// findutils joins paths with backslashes on Windows, and `xargs` then
/// reads them as escapes. Print forward slashes, as Git Bash's find does.
struct FindDeps {
    inner: findutils::find::StandardDependencies,
    output: std::cell::RefCell<SlashWriter<std::io::Stdout>>,
}

impl FindDeps {
    fn new() -> Self {
        Self {
            inner: findutils::find::StandardDependencies::new(),
            output: std::cell::RefCell::new(SlashWriter {
                inner: std::io::stdout(),
                convert: cfg!(windows),
            }),
        }
    }
}

impl findutils::find::Dependencies for FindDeps {
    fn get_output(&self) -> &std::cell::RefCell<dyn Write> {
        &self.output
    }

    fn now(&self) -> std::time::SystemTime {
        self.inner.now()
    }

    fn confirm(&self, prompt: &str) -> bool {
        self.inner.confirm(prompt)
    }
}

/// Writes bytes with each backslash turned into `/` when `convert` is set
/// (Windows, where a file name cannot contain a backslash).
pub(crate) struct SlashWriter<W> {
    pub(crate) inner: W,
    pub(crate) convert: bool,
}

impl<W: Write> Write for SlashWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if !self.convert {
            return self.inner.write(buf);
        }
        let converted: Vec<u8> = buf
            .iter()
            .map(|&b| if b == b'\\' { b'/' } else { b })
            .collect();
        self.inner.write_all(&converted)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn xargs(args: Vec<OsString>) -> i32 {
    let args = lossy(rewrite_xargs_command(args));
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    findutils::xargs::xargs_main(&refs)
}

fn lossy(args: Vec<OsString>) -> Vec<String> {
    args.into_iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

/// `find ... -exec grep ...`: point a bundled command word at this binary,
/// since `find` spawns it from `PATH` and Windows has no `grep` there.
pub(crate) fn rewrite_find_exec(args: Vec<OsString>) -> Vec<OsString> {
    let mut out = Vec::with_capacity(args.len());
    let mut after_exec = false;
    for arg in args {
        if after_exec {
            after_exec = false;
            out.extend(rewrite_one(arg));
            continue;
        }
        after_exec = matches!(arg.to_str(), Some("-exec" | "-execdir" | "-ok" | "-okdir"));
        out.push(arg);
    }
    out
}

/// xargs options that take a separate value (`-n 1`, `-I {}`, ...).
const XARGS_VALUE_FLAGS: &[&str] = &[
    "-a",
    "-d",
    "-E",
    "-I",
    "-L",
    "-n",
    "-P",
    "-s",
    "--arg-file",
    "--delimiter",
    "--eof",
    "--max-args",
    "--max-lines",
    "--max-procs",
    "--max-chars",
    "--replace",
];

/// `xargs [options] grep ...`: same rewrite as [`rewrite_find_exec`] for the
/// first word that is not an option.
pub(crate) fn rewrite_xargs_command(args: Vec<OsString>) -> Vec<OsString> {
    let mut out = Vec::with_capacity(args.len() + 1);
    let mut iter = args.into_iter();
    out.extend(iter.next());
    while let Some(arg) = iter.next() {
        let text = arg.to_string_lossy().into_owned();
        if text == "--" {
            out.push(arg);
            out.extend(iter.next().into_iter().flat_map(rewrite_one));
            break;
        }
        if !text.starts_with('-') {
            out.extend(rewrite_one(arg));
            break;
        }
        out.push(arg);
        if XARGS_VALUE_FLAGS.contains(&text.as_str()) {
            out.extend(iter.next());
        }
    }
    out.extend(iter);
    out
}

fn rewrite_one(arg: OsString) -> Vec<OsString> {
    match arg.to_str().filter(|name| is_bundled(name)) {
        Some(name) => self_invocation(name),
        None => vec![arg],
    }
}

/// `[<this binary>, "__shell/<name>"]`: argv prefix that runs bundled `name`.
fn self_invocation(name: &str) -> Vec<OsString> {
    vec![
        self_exe().clone().into_os_string(),
        OsString::from(format!("{}{name}", crate::INVOKE_PREFIX)),
    ]
}

/// This binary. `current_exe` only fails when the OS cannot report the
/// image path; the spawn of an empty path then fails with a shell error.
fn self_exe() -> &'static PathBuf {
    static EXE: OnceLock<PathBuf> = OnceLock::new();
    EXE.get_or_init(|| std::env::current_exe().unwrap_or_default())
}

/// Register a shim builtin for every bundled name.
pub(crate) fn register<SE: ShellExtensions>(shell: &mut brush_core::Shell<SE>) {
    for (name, _) in BUNDLED {
        shell.register_builtin(*name, shim_registration::<SE>());
    }
}

fn shim_registration<SE: ShellExtensions>() -> Registration<SE> {
    Registration {
        execute_func: shim_execute::<SE>,
        content_func: shim_content,
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
    }
}

fn shim_content(
    name: &str,
    _content_type: ContentType,
    _options: &ContentOptions,
) -> Result<String, brush_core::Error> {
    Ok(format!(
        "{name} - bundled with the whycodes embedded shell\n"
    ))
}

/// Re-execute this binary as `__shell/<name>` with the shell's redirections.
fn shim_execute<SE: ShellExtensions>(
    context: ExecutionContext<'_, SE>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<brush_core::ExecutionResult, brush_core::Error>> {
    Box::pin(async move {
        let name = context.command_name.clone();
        let mut child_args = Vec::with_capacity(args.len() + 1);
        // args[0] is dropped by the external path; argv[1] carries the name.
        child_args.push(CommandArg::String(String::new()));
        child_args.push(CommandArg::String(format!(
            "{}{name}",
            crate::INVOKE_PREFIX
        )));
        child_args.extend(args.into_iter().skip(1));
        let mut cmd = commands::SimpleCommand::new(
            commands::ShellForCommand::ParentShell(context.shell),
            context.params,
            self_exe().to_string_lossy().into_owned(),
            child_args,
        );
        cmd.use_functions = false;
        let spawned = cmd.execute().await?;
        Ok(spawned.wait().await?.into())
    })
}
