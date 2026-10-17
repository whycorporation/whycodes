//! Shared engine for `ast_grep` / `ast_edit`: tree-sitter languages for
//! `ast-grep-core`, file selection and a unified diff of planned edits.
//!
//! Only the grammars below are compiled in (each one is C source built by
//! `cc`); the `ast-tools` Cargo feature gates the whole module.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use ast_grep_core::matcher::PatternBuilder;
use ast_grep_core::meta_var::{MetaVarEnv, MetaVariable};
use ast_grep_core::tree_sitter::{LanguageExt, StrDoc, TSLanguage};
use ast_grep_core::{Language, NodeMatch, Pattern, PatternError};

use super::paths::{MAX_GREP_FILE_BYTES, display_path, file_len, walk_files};

/// A language `ast_grep` / `ast_edit` can parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AstLang {
    Rust,
    Python,
    Go,
    JavaScript,
    TypeScript,
    Tsx,
}

pub const ALL_LANGS: [AstLang; 6] = [
    AstLang::Rust,
    AstLang::Python,
    AstLang::Go,
    AstLang::JavaScript,
    AstLang::TypeScript,
    AstLang::Tsx,
];

pub const LANGUAGE_NAMES: &str = "rust, python, go, javascript, typescript, tsx";

impl AstLang {
    /// `language` argument (name or common extension), case-insensitive.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "rust" | "rs" => Some(Self::Rust),
            "python" | "py" => Some(Self::Python),
            "go" | "golang" => Some(Self::Go),
            "javascript" | "js" | "jsx" | "mjs" | "cjs" => Some(Self::JavaScript),
            "typescript" | "ts" | "mts" | "cts" => Some(Self::TypeScript),
            "tsx" => Some(Self::Tsx),
            _ => None,
        }
    }

    /// Language of a file by extension.
    pub fn from_file(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "pyi" => Some(Self::Python),
            "py" | "rs" | "go" | "js" | "jsx" | "mjs" | "cjs" | "ts" | "mts" | "cts" | "tsx" => {
                Self::from_name(&ext)
            }
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Python => "python",
            Self::Go => "go",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
        }
    }

    /// Whether a file in `self` is searched when the caller asked for `filter`.
    /// `typescript` also covers `.tsx` files.
    pub fn selected_by(self, filter: Self) -> bool {
        self == filter || (filter == Self::TypeScript && self == Self::Tsx)
    }

    /// Languages whose parser rejects `$` in identifiers get a stand-in
    /// character for metavariables while the pattern is parsed.
    fn expando(self) -> char {
        match self {
            Self::Rust | Self::Python | Self::Go => 'µ',
            Self::JavaScript | Self::TypeScript | Self::Tsx => '$',
        }
    }
}

impl Language for AstLang {
    fn pre_process_pattern<'q>(&self, query: &'q str) -> Cow<'q, str> {
        let expando = self.expando();
        if expando == '$' {
            return Cow::Borrowed(query);
        }
        Cow::Owned(swap_sigils(query, expando))
    }

    fn expando_char(&self) -> char {
        self.expando()
    }

    fn kind_to_id(&self, kind: &str) -> u16 {
        self.get_ts_language().id_for_node_kind(kind, true)
    }

    fn field_to_id(&self, field: &str) -> Option<u16> {
        self.get_ts_language()
            .field_id_for_name(field)
            .map(|f| f.get())
    }

    fn build_pattern(&self, builder: &PatternBuilder) -> Result<Pattern, PatternError> {
        builder.build(|src| StrDoc::try_new(src, *self))
    }
}

impl LanguageExt for AstLang {
    fn get_ts_language(&self) -> TSLanguage {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::Go => tree_sitter_go::LANGUAGE.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        }
    }
}

/// `$A` / `$$$ARGS` / `$$$` → `µA` / `µµµARGS` / `µµµ`; other `$` stay.
fn swap_sigils(query: &str, expando: char) -> String {
    let mut out = String::with_capacity(query.len());
    let mut dollars = 0usize;
    for c in query.chars() {
        if c == '$' {
            dollars += 1;
            continue;
        }
        let meta = c.is_ascii_uppercase() || c == '_' || dollars == 3;
        let sigil = if meta { expando } else { '$' };
        out.extend(std::iter::repeat_n(sigil, dollars));
        dollars = 0;
        out.push(c);
    }
    let sigil = if dollars == 3 { expando } else { '$' };
    out.extend(std::iter::repeat_n(sigil, dollars));
    out
}

/// Arguments shared by both tools.
pub struct Scope {
    pub pattern: String,
    pub language: Option<AstLang>,
    pub root: PathBuf,
    pub working_dir: String,
}

/// Parse the shared `pattern` / `language` / `path` arguments.
pub fn scope_from_args(args: &serde_json::Value, working_dir: &str) -> Result<Scope, String> {
    let pattern = args["pattern"].as_str().unwrap_or("").to_string();
    if pattern.trim().is_empty() {
        return Err("Missing required parameter `pattern`.".into());
    }
    let language = match args["language"].as_str().filter(|s| !s.trim().is_empty()) {
        Some(name) => Some(AstLang::from_name(name).ok_or_else(|| unknown_language(name))?),
        None => None,
    };
    let root = super::paths::resolve_path(working_dir, args["path"].as_str().unwrap_or(""));
    Ok(Scope {
        pattern,
        language,
        root,
        working_dir: working_dir.to_string(),
    })
}

fn unknown_language(name: &str) -> String {
    format!("unknown language `{name}`; supported: {LANGUAGE_NAMES}")
}

/// One file to parse: absolute path, display path, language.
pub struct Target {
    pub path: PathBuf,
    pub shown: String,
    pub lang: AstLang,
}

/// Files under `scope.root` in a supported (or the requested) language.
pub fn collect_targets(scope: &Scope) -> Result<Vec<Target>, String> {
    let root = &scope.root;
    if !root.exists() {
        return Err(format!(
            "path not found: {}",
            display_path(root, &scope.working_dir)
        ));
    }
    let shown = |p: &Path| display_path(p, &scope.working_dir).replace('\\', "/");
    if root.is_file() {
        let lang = match (scope.language, AstLang::from_file(root)) {
            (Some(lang), _) | (None, Some(lang)) => lang,
            (None, None) => {
                return Err(format!(
                    "cannot tell the language of {}; pass `language` ({LANGUAGE_NAMES})",
                    shown(root)
                ));
            }
        };
        return Ok(vec![Target {
            path: root.clone(),
            shown: shown(root),
            lang,
        }]);
    }
    let mut out = Vec::new();
    walk_files(root, &mut |file, _rel| {
        let keep = AstLang::from_file(file)
            .filter(|lang| scope.language.is_none_or(|want| lang.selected_by(want)));
        if let Some(lang) = keep {
            out.push(Target {
                path: file.to_path_buf(),
                shown: shown(file),
                lang,
            });
        }
        true
    });
    out.sort_by(|a, b| a.shown.cmp(&b.shown));
    Ok(out)
}

/// The pattern compiled for each language among `targets`.
///
/// With an explicit `language` a pattern that parses with error nodes is
/// still used (ast-grep matches around them). Without one, a language whose
/// parse of the pattern has errors is skipped: a Rust snippet is rarely
/// valid Python. Fails when no language accepts the pattern.
pub fn compile_patterns(
    scope: &Scope,
    targets: &[Target],
) -> Result<Vec<(AstLang, Pattern)>, String> {
    let mut langs: Vec<AstLang> = targets.iter().map(|t| t.lang).collect();
    langs.sort();
    langs.dedup();
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for lang in langs {
        match Pattern::try_new(&scope.pattern, lang) {
            Ok(p) if scope.language.is_some() || !p.has_error() => out.push((lang, p)),
            Ok(_) => errors.push(format!("{}: the pattern has syntax errors", lang.name())),
            Err(e) => errors.push(format!("{}: {e}", lang.name())),
        }
    }
    if out.is_empty() && !errors.is_empty() {
        return Err(format!(
            "pattern does not parse as a single node ({}). Write one complete \
             expression/statement/item, or pass `language`.",
            errors.join("; ")
        ));
    }
    Ok(out)
}

pub fn pattern_for(patterns: &[(AstLang, Pattern)], lang: AstLang) -> Option<&Pattern> {
    patterns.iter().find(|(l, _)| *l == lang).map(|(_, p)| p)
}

/// Source text of a file, or `None` when too large, unreadable or not UTF-8.
pub fn read_source(path: &Path) -> Option<String> {
    if file_len(path).is_some_and(|n| n > MAX_GREP_FILE_BYTES) {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// `$A = text` lines for every captured metavariable of a match, sorted.
pub fn captures(m: &NodeMatch<'_, StrDoc<AstLang>>, src: &str) -> Vec<String> {
    let env = m.get_env();
    let mut out: Vec<String> = env
        .get_matched_variables()
        .filter_map(|var| describe_capture(var, env, src))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// `$A = text` / `$$$A = text` for one variable; `None` for the
/// non-capturing kinds (`$_`, `$$$`), which `get_matched_variables` does not
/// yield today.
fn describe_capture(
    var: MetaVariable,
    env: &MetaVarEnv<'_, StrDoc<AstLang>>,
    src: &str,
) -> Option<String> {
    match var {
        MetaVariable::Capture(name, _) => {
            let node = env.get_match(&name)?;
            Some(format!("${name} = {}", one_line(&node.text())))
        }
        MetaVariable::MultiCapture(name) => {
            let nodes = env.get_multiple_matches(&name);
            let text = match (nodes.first(), nodes.last()) {
                (Some(first), Some(last)) => {
                    &src[first.range().start..last.range().end.max(first.range().start)]
                }
                _ => "",
            };
            Some(format!("$$${name} = {}", one_line(text)))
        }
        MetaVariable::Dropped(_) | MetaVariable::Multiple => None,
    }
}

/// First line of `text`, clipped, with `…` when anything was cut.
pub fn one_line(text: &str) -> String {
    const MAX: usize = 200;
    let mut lines = text.lines();
    let first = lines.next().unwrap_or("");
    let more = lines.next().is_some();
    let clipped: String = first.chars().take(MAX).collect();
    if more || first.chars().count() > MAX {
        format!("{clipped}…")
    } else {
        clipped
    }
}

/// A byte-range replacement in one file's source.
#[derive(Debug, Clone, PartialEq)]
pub struct Replace {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// `src` with every (sorted, non-overlapping) replacement applied.
pub fn apply_replacements(src: &str, edits: &[Replace]) -> String {
    let mut out = String::with_capacity(src.len());
    let mut last = 0;
    for e in edits {
        out.push_str(&src[last..e.start]);
        out.push_str(&e.text);
        last = e.end;
    }
    out.push_str(&src[last..]);
    out
}

const CONTEXT: usize = 3;

/// Line starts of `s` (byte offsets); a final line without `\n` counts.
fn line_starts(s: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(s.match_indices('\n').map(|(i, _)| i + 1));
    if starts.last() == Some(&s.len()) && !s.is_empty() {
        starts.pop();
    }
    starts
}

fn line_of(starts: &[usize], byte: usize) -> usize {
    starts.partition_point(|&s| s <= byte).saturating_sub(1)
}

fn line_strs(s: &str) -> Vec<&str> {
    s.split_inclusive('\n')
        .map(|l| l.trim_end_matches('\n').trim_end_matches('\r'))
        .collect()
}

/// One changed run of old lines `[first, last]` and what replaces them.
struct Change {
    first: usize,
    last: usize,
    new_lines: Vec<String>,
}

fn changes(src: &str, edits: &[Replace]) -> Vec<Change> {
    let starts = line_starts(src);
    let line_end = |i: usize| starts.get(i + 1).copied().unwrap_or(src.len());
    let mut groups: Vec<(usize, usize, Vec<&Replace>)> = Vec::new();
    for e in edits {
        let first = line_of(&starts, e.start);
        let last = line_of(&starts, e.end.saturating_sub(1).max(e.start));
        match groups.last_mut() {
            Some(g) if first <= g.1 => {
                g.1 = g.1.max(last);
                g.2.push(e);
            }
            _ => groups.push((first, last, vec![e])),
        }
    }
    groups
        .into_iter()
        .map(|(first, last, group)| {
            let base = starts[first];
            let block = &src[base..line_end(last)];
            let local: Vec<Replace> = group
                .iter()
                .map(|e| Replace {
                    start: e.start - base,
                    end: e.end - base,
                    text: e.text.clone(),
                })
                .collect();
            let new_block = apply_replacements(block, &local);
            Change {
                first,
                last,
                new_lines: line_strs(&new_block)
                    .into_iter()
                    .map(String::from)
                    .collect(),
            }
        })
        .collect()
}

/// Unified diff (`---` / `+++` / `@@` hunks, 3 lines of context) of `edits`
/// applied to `src`. Returns the text and the added / removed line counts.
pub fn unified_diff(shown: &str, src: &str, edits: &[Replace]) -> (String, usize, usize) {
    let old: Vec<&str> = line_strs(src);
    let changes = changes(src, edits);
    let mut out = format!("--- a/{shown}\n+++ b/{shown}\n");
    let (mut added, mut removed) = (0usize, 0usize);
    // Signed offset between old and new line numbers before a hunk.
    let mut shift: isize = 0;
    let mut i = 0;
    while i < changes.len() {
        // Merge changes whose context windows touch into one hunk.
        let mut j = i;
        while j + 1 < changes.len() && changes[j + 1].first <= changes[j].last + 2 * CONTEXT + 1 {
            j += 1;
        }
        let start = changes[i].first.saturating_sub(CONTEXT);
        let end = (changes[j].last + CONTEXT).min(old.len().saturating_sub(1));
        let mut body = String::new();
        let mut old_count = 0usize;
        let mut new_count = 0usize;
        let mut line = start;
        let mut hunk_shift: isize = 0;
        for c in &changes[i..=j] {
            for ctx in &old[line..c.first] {
                body.push_str(&format!(" {ctx}\n"));
            }
            old_count += c.first - line;
            new_count += c.first - line;
            for l in &old[c.first..=c.last] {
                body.push_str(&format!("-{l}\n"));
            }
            for l in &c.new_lines {
                body.push_str(&format!("+{l}\n"));
            }
            let gone = c.last - c.first + 1;
            old_count += gone;
            new_count += c.new_lines.len();
            removed += gone;
            added += c.new_lines.len();
            hunk_shift += c.new_lines.len() as isize - gone as isize;
            line = c.last + 1;
        }
        let tail_end = (end + 1).max(line);
        for ctx in &old[line..tail_end] {
            body.push_str(&format!(" {ctx}\n"));
        }
        let tail = tail_end - line;
        old_count += tail;
        new_count += tail;
        let new_start = (start as isize + shift) as usize;
        out.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            hunk_start(start, old_count),
            old_count,
            hunk_start(new_start, new_count),
            new_count
        ));
        out.push_str(&body);
        shift += hunk_shift;
        i = j + 1;
    }
    (out, added, removed)
}

/// 1-based start line; an empty side names the line before it.
fn hunk_start(zero_based: usize, count: usize) -> usize {
    if count == 0 {
        zero_based
    } else {
        zero_based + 1
    }
}

#[cfg(test)]
#[path = "ast_tests.rs"]
mod tests;
