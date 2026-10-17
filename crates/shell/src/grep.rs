//! `grep`: the common GNU flags over ripgrep's searcher and printer.
//!
//! There is no uutils grep. Patterns are basic regular expressions unless
//! `-E`, `-F` or `-P` says otherwise; `\(`, `\|`, `\{` and `\<`/`\>` are
//! rewritten into the Rust regex dialect. `-r` walks every file (no ignore
//! files, like GNU grep) and skips binary files.

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clap::Parser;
use globset::{Glob, GlobSet, GlobSetBuilder};
use grep_printer::{StandardBuilder, SummaryBuilder, SummaryKind};
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder};

#[derive(Parser, Debug)]
#[command(name = "grep", disable_help_flag = true, disable_version_flag = true)]
struct Opts {
    #[arg(short = 'e', long = "regexp")]
    regexps: Vec<String>,
    #[arg(short = 'E', long = "extended-regexp")]
    extended: bool,
    #[arg(short = 'F', long = "fixed-strings")]
    fixed: bool,
    #[arg(short = 'G', long = "basic-regexp")]
    basic: bool,
    #[arg(short = 'P', long = "perl-regexp")]
    perl: bool,
    #[arg(short = 'i', long = "ignore-case")]
    ignore_case: bool,
    #[arg(short = 'w', long = "word-regexp")]
    word: bool,
    #[arg(short = 'x', long = "line-regexp")]
    line: bool,
    #[arg(short = 'v', long = "invert-match")]
    invert: bool,
    #[arg(short = 'c', long = "count")]
    count: bool,
    #[arg(short = 'l', long = "files-with-matches")]
    files_with_matches: bool,
    #[arg(short = 'L', long = "files-without-match")]
    files_without_match: bool,
    #[arg(short = 'n', long = "line-number")]
    line_number: bool,
    #[arg(short = 'H', long = "with-filename")]
    with_filename: bool,
    #[arg(short = 'h', long = "no-filename")]
    no_filename: bool,
    #[arg(short = 'o', long = "only-matching")]
    only_matching: bool,
    #[arg(short = 'q', long = "quiet", visible_alias = "silent")]
    quiet: bool,
    #[arg(short = 's', long = "no-messages")]
    no_messages: bool,
    #[arg(short = 'r', long = "recursive")]
    recursive: bool,
    #[arg(short = 'R', long = "dereference-recursive")]
    dereference_recursive: bool,
    #[arg(short = 'A', long = "after-context")]
    after: Option<usize>,
    #[arg(short = 'B', long = "before-context")]
    before: Option<usize>,
    #[arg(short = 'C', long = "context")]
    context: Option<usize>,
    #[arg(short = 'm', long = "max-count")]
    max_count: Option<u64>,
    #[arg(short = 'a', long = "text")]
    text: bool,
    #[arg(short = 'I')]
    skip_binary: bool,
    #[arg(long)]
    include: Vec<String>,
    #[arg(long)]
    exclude: Vec<String>,
    #[arg(long = "exclude-dir")]
    exclude_dir: Vec<String>,
    #[arg(long, alias = "colour", num_args = 0..=1, default_missing_value = "auto")]
    color: Option<String>,
    operands: Vec<OsString>,
}

/// Exit status: 0 a line was selected, 1 none was, 2 an error occurred.
pub(crate) fn main(args: Vec<OsString>) -> i32 {
    let opts = match Opts::try_parse_from(args) {
        Ok(opts) => opts,
        Err(err) => {
            eprintln!("grep: {err}");
            return 2;
        }
    };
    match Grep::new(opts) {
        Ok(mut grep) => grep.run(),
        Err(err) => {
            eprintln!("grep: {err}");
            2
        }
    }
}

struct Grep {
    opts: Opts,
    matcher: RegexMatcher,
    searcher: Searcher,
    targets: Vec<Target>,
    with_filename: bool,
    include: GlobSet,
    exclude: GlobSet,
    exclude_dir: GlobSet,
    matched: bool,
    failed: bool,
}

enum Target {
    Stdin,
    Path(PathBuf),
    /// `grep -r pat` with no operand: walk `.`, print paths without `./`.
    Cwd,
}

impl Grep {
    fn new(mut opts: Opts) -> Result<Self, String> {
        let mut operands = std::mem::take(&mut opts.operands).into_iter();
        if opts.regexps.is_empty() {
            let pattern = operands.next().ok_or("no pattern given")?;
            opts.regexps.push(pattern.to_string_lossy().into_owned());
        }
        let recursive = opts.recursive || opts.dereference_recursive;
        let mut targets: Vec<Target> = operands.map(target).collect();
        let with_filename =
            !opts.no_filename && (opts.with_filename || recursive || targets.len() > 1);
        if targets.is_empty() {
            targets.push(if recursive {
                Target::Cwd
            } else {
                Target::Stdin
            });
        }
        Ok(Self {
            matcher: matcher(&opts)?,
            searcher: searcher(&opts),
            include: globs(&opts.include)?,
            exclude: globs(&opts.exclude)?,
            exclude_dir: globs(&opts.exclude_dir)?,
            targets,
            with_filename,
            opts,
            matched: false,
            failed: false,
        })
    }

    fn run(&mut self) -> i32 {
        for target in std::mem::take(&mut self.targets) {
            match target {
                Target::Stdin => self.search(None),
                Target::Path(path) => self.search_operand(&path, false),
                Target::Cwd => self.search_operand(Path::new("."), true),
            }
            if self.matched && self.opts.quiet {
                return 0;
            }
        }
        if self.failed && !(self.opts.quiet && self.matched) {
            2
        } else if self.matched {
            0
        } else {
            1
        }
    }

    fn search_operand(&mut self, path: &Path, strip_dot: bool) {
        if !path.is_dir() {
            return self.search(Some(path));
        }
        if !(self.opts.recursive || self.opts.dereference_recursive) {
            return self.error(&format!("{}: Is a directory", display(path)));
        }
        let exclude_dir = self.exclude_dir.clone();
        let walk = walkdir::WalkDir::new(path)
            .follow_links(self.opts.dereference_recursive)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(move |e| e.depth() == 0 || !excluded_dir(&exclude_dir, e));
        for entry in walk {
            match entry {
                Ok(entry) if entry.file_type().is_file() && self.included(entry.path()) => {
                    let found = entry.path();
                    let shown = if strip_dot {
                        found.strip_prefix(".").unwrap_or(found)
                    } else {
                        found
                    };
                    self.search(Some(shown));
                }
                Ok(_) => {}
                Err(err) => self.error(&err.to_string()),
            }
        }
    }

    fn included(&self, path: &Path) -> bool {
        let name = path.file_name().unwrap_or(path.as_os_str());
        (self.include.is_empty() || self.include.is_match(name)) && !self.exclude.is_match(name)
    }

    fn search(&mut self, path: Option<&Path>) {
        let shown = path.map_or_else(|| "(standard input)".to_string(), display);
        let stdout = io::stdout();
        let out = stdout.lock();
        let result = match self.summary_kind() {
            Some(kind) => self.search_summary(kind, path, &shown, out),
            None => self.search_lines(path, &shown, out),
        };
        match result {
            Ok(found) => self.matched |= found,
            Err(err) if err.kind() == io::ErrorKind::BrokenPipe => self.matched = true,
            Err(err) => self.error(&format!("{shown}: {err}")),
        }
    }

    fn summary_kind(&self) -> Option<SummaryKind> {
        if self.opts.quiet {
            Some(SummaryKind::QuietWithMatch)
        } else if self.opts.files_with_matches {
            Some(SummaryKind::PathWithMatch)
        } else if self.opts.files_without_match {
            Some(SummaryKind::PathWithoutMatch)
        } else if self.opts.count {
            Some(SummaryKind::Count)
        } else {
            None
        }
    }

    fn search_summary(
        &mut self,
        kind: SummaryKind,
        path: Option<&Path>,
        shown: &str,
        out: impl Write,
    ) -> io::Result<bool> {
        let mut printer = SummaryBuilder::new()
            .kind(kind)
            .path(self.with_filename || path_kind(kind))
            .exclude_zero(false)
            .separator_field(b":".to_vec())
            .build_no_color(out);
        let mut sink = printer.sink_with_path(&self.matcher, shown);
        run_search(&mut self.searcher, &self.matcher, path, &mut sink)?;
        // -L succeeds when some file had no match, like GNU grep 3.5+.
        Ok(sink.has_match() != (kind == SummaryKind::PathWithoutMatch))
    }

    fn search_lines(
        &mut self,
        path: Option<&Path>,
        shown: &str,
        out: impl Write,
    ) -> io::Result<bool> {
        let mut printer = StandardBuilder::new()
            .path(self.with_filename)
            .only_matching(self.opts.only_matching)
            .separator_context(Some(b"--".to_vec()))
            .build_no_color(out);
        let mut sink = printer.sink_with_path(&self.matcher, shown);
        run_search(&mut self.searcher, &self.matcher, path, &mut sink)?;
        Ok(sink.has_match())
    }

    fn error(&mut self, message: &str) {
        self.failed = true;
        if !self.opts.no_messages {
            eprintln!("grep: {message}");
        }
    }
}

fn excluded_dir(globs: &GlobSet, entry: &walkdir::DirEntry) -> bool {
    entry.file_type().is_dir() && globs.is_match(entry.file_name())
}

fn path_kind(kind: SummaryKind) -> bool {
    matches!(
        kind,
        SummaryKind::PathWithMatch | SummaryKind::PathWithoutMatch
    )
}

fn run_search<S: grep_searcher::Sink<Error = io::Error>>(
    searcher: &mut Searcher,
    matcher: &RegexMatcher,
    path: Option<&Path>,
    sink: S,
) -> io::Result<()> {
    match path {
        Some(path) => searcher.search_path(matcher, path, sink),
        None => searcher.search_reader(matcher, io::stdin().lock(), sink),
    }
}

fn target(operand: OsString) -> Target {
    if operand == "-" {
        return Target::Stdin;
    }
    Target::Path(PathBuf::from(crate::msys::translate_path(&operand)))
}

/// Forward slashes, as Git Bash prints them, on every platform.
fn display(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn matcher(opts: &Opts) -> Result<RegexMatcher, String> {
    let basic = !(opts.extended || opts.fixed || opts.perl) || opts.basic;
    let patterns: Vec<String> = opts
        .regexps
        .iter()
        .flat_map(|p| p.split('\n'))
        .map(|p| {
            if opts.fixed {
                p.to_string()
            } else {
                to_rust_regex(p, basic)
            }
        })
        .collect();
    RegexMatcherBuilder::new()
        .case_insensitive(opts.ignore_case)
        .word(opts.word)
        .whole_line(opts.line)
        .fixed_strings(opts.fixed)
        .line_terminator(Some(b'\n'))
        .build_many(&patterns)
        .map_err(|err| err.to_string())
}

fn searcher(opts: &Opts) -> Searcher {
    let context = opts.context.unwrap_or(0);
    let binary = if opts.text {
        BinaryDetection::none()
    } else {
        BinaryDetection::quit(0)
    };
    SearcherBuilder::new()
        .line_number(opts.line_number)
        .invert_match(opts.invert)
        .after_context(opts.after.unwrap_or(context))
        .before_context(opts.before.unwrap_or(context))
        .max_matches(opts.max_count)
        .binary_detection(binary)
        .build()
}

fn globs(patterns: &[String]) -> Result<GlobSet, String> {
    let mut set = GlobSetBuilder::new();
    for pattern in patterns {
        set.add(Glob::new(pattern).map_err(|err| err.to_string())?);
    }
    set.build().map_err(|err| err.to_string())
}

/// Rewrite a POSIX basic (`basic`) or extended regular expression into the
/// Rust regex dialect: in a BRE `\(`, `\)`, `\|`, `\{`, `\}`, `\+`, `\?` are
/// operators and the bare characters are literals; `\<` and `\>` are word
/// boundaries in both.
pub(crate) fn to_rust_regex(pattern: &str, basic: bool) -> String {
    let mut out = String::with_capacity(pattern.len() + 8);
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        match (c, basic) {
            ('\\', _) => match chars.next() {
                Some('<' | '>') => out.push_str("\\b"),
                Some(n @ ('(' | ')' | '|' | '{' | '}' | '+' | '?')) if basic => out.push(n),
                Some(n) => {
                    out.push('\\');
                    out.push(n);
                }
                None => out.push_str("\\\\"),
            },
            ('(' | ')' | '|' | '{' | '}' | '+' | '?', true) => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}
