//! A recursive-descent parser for the shell an agent writes, lowered to the
//! tokenizer's [`Segment`] shape.
//!
//! # Why a parser, and why this one
//!
//! [`crate::tokenize`] splits on separators and strips quotes. That is enough
//! to find `rm -rf ~`, but it cannot tell a here-document body from a command,
//! `2>&1` from a file, or `$'\x2f'` from a literal, and it does not know that
//! `DIR=/` gives `$DIR` a value. This module parses the POSIX/bash subset that
//! coding agents emit and lowers it to the same `Segment`s, so `assess` and the
//! parser-independent catastrophic tier in [`crate::paths`] stay unchanged.
//!
//! It is written here rather than taken from a crate because the crate is a
//! foundation linked into the agent: the only maintained bash parser on
//! crates.io brings seventy-odd transitive crates (a PEG generator, a cache, a
//! snapshot-testing library) into the release binary for a job that needs a
//! few hundred lines.
//!
//! # Scope
//!
//! Lists (`;` `&` `&&` `||` newline), pipelines (`|` `|&` `!`), subshells,
//! brace groups, `if`/`while`/`until`/`for`/`select`/`case`, function
//! definitions, `[[ … ]]`, `(( … ))`, single, double, ANSI-C (`$'…'`) and
//! locale (`$"…"`) quoting, `$(…)`, backticks, `$((…))`, `${…}`, process
//! substitution, typed redirections (`>` `>>` `>|` `&>` `&>>` `>&` `<` `<>`
//! `<&` `<<<`, with fd numbers), here-documents, and assignments (scalar and
//! array, as a prefix or through `export`/`local`/`declare`/…).
//!
//! # Failure is a fallback, not a verdict
//!
//! Anything outside that subset — unbalanced quotes, a stray `)`, PowerShell,
//! a here-document without its terminator, nesting deeper than [`MAX_DEPTH`] —
//! is a [`ParseError`], and the caller goes back to the tokenizer. The parser
//! therefore never has to guess: when it is unsure it gives up, and the old,
//! deliberately pessimistic path decides.

use std::collections::HashMap;

use crate::tokenize::{self, Segment, Word};

/// Nesting beyond this is refused, so adversarial input cannot exhaust the
/// stack. The tokenizer, which is iterative, takes over.
const MAX_DEPTH: usize = 64;

/// How many readings of a command with multiply-assigned variables are tried.
const MAX_VARIANTS: usize = 16;

/// Commands whose assignment-shaped arguments set variables.
const BINDERS: &[&str] = &["export", "local", "declare", "typeset", "readonly", "env"];

/// Commands that run what they read on stdin as a script.
const SHELLS: &[&str] = &["sh", "bash", "zsh", "fish", "dash", "ksh"];

/// The input is outside the subset this parser understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ParseError;

type Parsed<T> = Result<T, ParseError>;

/// One piece of a word.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    /// Text after quote removal.
    Lit(String),
    /// `$NAME` or `${NAME}`. `raw` is kept when the value is unknown, so
    /// [`crate::paths::has_unresolved_variable`] still sees a variable.
    Var {
        name: String,
        raw: String,
        quoted: bool,
    },
    /// Only known by running something: `$(…)`, backticks, `$((…))`, process
    /// substitution. Carries the placeholder the tokenizer would have kept.
    Dynamic(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WordAst {
    parts: Vec<Part>,
    /// `NAME=value` (one value) or `NAME=(a b …)` (one per element).
    assign: Option<(String, Vec<Vec<Part>>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Redirect {
    /// Opens a file for writing. `op` is spelled as the tokenizer spells it.
    Write {
        op: &'static str,
        target: WordAst,
    },
    /// A here-document: index into [`Parser::heredocs`].
    HereDoc(usize),
    HereString(WordAst),
    /// Reads, fd duplication and closing: no file is written.
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    Simple {
        words: Vec<WordAst>,
        redirects: Vec<Redirect>,
    },
    /// Subshell, group, `if`, loop, `case`, function body: everything inside
    /// runs (or may run), so it is flattened.
    Compound {
        body: Vec<Pipeline>,
        redirects: Vec<Redirect>,
    },
    For {
        var: String,
        items: Vec<WordAst>,
        body: Vec<Pipeline>,
        redirects: Vec<Redirect>,
    },
}

type Pipeline = Vec<Command>;

struct PendingHeredoc {
    delim: String,
    strip_tabs: bool,
    slot: usize,
}

struct Parser {
    s: Vec<char>,
    pos: usize,
    depth: usize,
    /// Nesting of `$(…)` / backticks / process substitution. Bodies are only
    /// recorded at level 0: nested ones are found again when the outer body
    /// is assessed, and recording them twice would make the work exponential.
    subst_depth: usize,
    bodies: Vec<String>,
    heredocs: Vec<String>,
    pending: Vec<PendingHeredoc>,
    /// Comment text, `#` included. Only bash and PowerShell read `#` as a
    /// comment; `cmd.exe` runs `echo # & rm -rf ~`. So comments are handed to
    /// the tokenizer, which splits them exactly as it did before the parser.
    comments: Vec<String>,
}

/// The lowered form of a command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Lowered {
    /// The segments, once with every variable left as written and once per
    /// reading of the variables assigned earlier in the line. The worst
    /// verdict over all of them wins, so knowing a value can only add risk.
    pub variants: Vec<Vec<Segment>>,
    /// Bodies of `$(…)`, backticks and process substitutions, outermost only.
    pub bodies: Vec<String>,
    /// Here-document and here-string text fed to a shell, which runs it.
    pub scripts: Vec<String>,
    /// A comment did not tokenize cleanly.
    pub malformed: bool,
}

/// Parse `input` and lower it.
pub(crate) fn parse(input: &str) -> Parsed<Lowered> {
    let mut p = Parser {
        s: input.chars().collect(),
        pos: 0,
        depth: 0,
        subst_depth: 0,
        bodies: Vec::new(),
        heredocs: Vec::new(),
        pending: Vec::new(),
        comments: Vec::new(),
    };
    let mut program = Vec::new();
    p.list(&mut program)?;
    if p.pos < p.s.len() || !p.pending.is_empty() {
        return Err(ParseError);
    }

    let mut raw = Lower::new(&p.heredocs, None);
    raw.pipelines(&program);
    let mut variants = vec![raw.segments];
    for pick in 0..raw.max_values.min(MAX_VARIANTS) {
        let mut l = Lower::new(&p.heredocs, Some(pick));
        l.pipelines(&program);
        variants.push(l.segments);
    }
    let mut malformed = false;
    for comment in &p.comments {
        let tokens = tokenize::tokenize(comment);
        malformed |= tokens.malformed;
        for segments in &mut variants {
            segments.extend(tokens.segments.iter().cloned());
        }
    }
    Ok(Lowered {
        variants,
        bodies: p.bodies,
        scripts: raw.scripts,
        malformed,
    })
}

/// `\r` is a blank too, as in the tokenizer, so CRLF input parses.
fn is_blank(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r')
}

fn is_meta(c: char) -> bool {
    is_blank(c) || matches!(c, '\n' | ';' | '&' | '|' | '<' | '>' | '(' | ')')
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(is_name_start) && chars.all(is_name_char)
}

impl Parser {
    fn at(&self, i: usize) -> Option<char> {
        self.s.get(i).copied()
    }

    fn peek(&self) -> Option<char> {
        self.at(self.pos)
    }

    fn next(&mut self) -> Parsed<char> {
        let c = self.peek().ok_or(ParseError)?;
        self.pos += 1;
        Ok(c)
    }

    fn looking_at(&self, text: &str) -> bool {
        (self.pos..)
            .zip(text.chars())
            .all(|(i, c)| self.at(i) == Some(c))
    }

    fn eat(&mut self, text: &str) -> bool {
        let found = self.looking_at(text);
        if found {
            self.pos += text.chars().count();
        }
        found
    }

    fn enter(&mut self) -> Parsed<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(ParseError);
        }
        Ok(())
    }

    fn slice(&self, from: usize, to: usize) -> String {
        self.s[from..to].iter().collect()
    }

    /// Spaces, tabs and line continuations.
    fn blanks(&mut self) {
        loop {
            if self.peek().is_some_and(is_blank) {
                self.pos += 1;
            } else if !self.eat("\\\n") {
                return;
            }
        }
    }

    /// Blanks, then a comment up to (not including) the newline.
    fn blanks_and_comment(&mut self) {
        self.blanks();
        if self.peek() == Some('#') {
            let start = self.pos;
            while self.peek().is_some_and(|c| c != '\n') {
                self.pos += 1;
            }
            self.comments.push(self.slice(start, self.pos));
        }
    }

    /// Blanks, comments and newlines. Each newline first reads the bodies of
    /// the here-documents opened on the line it ends.
    fn linebreaks(&mut self) -> Parsed<()> {
        loop {
            self.blanks_and_comment();
            if !self.eat("\n") {
                return Ok(());
            }
            self.heredoc_bodies()?;
        }
    }

    fn heredoc_bodies(&mut self) -> Parsed<()> {
        for doc in std::mem::take(&mut self.pending) {
            let mut body = String::new();
            loop {
                if self.pos >= self.s.len() {
                    return Err(ParseError);
                }
                let start = self.pos;
                while self.peek().is_some_and(|c| c != '\n') {
                    self.pos += 1;
                }
                let line = self.slice(start, self.pos);
                self.eat("\n");
                let line = if doc.strip_tabs {
                    line.trim_start_matches('\t')
                } else {
                    line.as_str()
                };
                if line.strip_suffix('\r').unwrap_or(line) == doc.delim {
                    break;
                }
                body.push_str(line);
                body.push('\n');
            }
            self.heredocs[doc.slot] = body;
        }
        Ok(())
    }

    /// A reserved word at the current position, when followed by a delimiter.
    fn reserved(&self) -> Option<&'static str> {
        const RESERVED: &[&str] = &[
            "if", "then", "elif", "else", "fi", "do", "done", "while", "until", "for", "select",
            "case", "esac", "in", "function", "{", "}", "!", "[[",
        ];
        RESERVED
            .iter()
            .copied()
            .find(|r| self.looking_at(r) && self.at(self.pos + r.len()).is_none_or(is_meta))
    }

    fn expect_word(&mut self, word: &str) -> Parsed<()> {
        self.linebreaks()?;
        if self.reserved() == Some(word) {
            self.pos += word.len();
            Ok(())
        } else {
            Err(ParseError)
        }
    }

    fn at_list_end(&self) -> bool {
        self.peek().is_none_or(|c| c == ')')
            || self.looking_at(";;")
            || self.looking_at(";&")
            || matches!(
                self.reserved(),
                Some("then" | "elif" | "else" | "fi" | "do" | "done" | "esac" | "}")
            )
    }

    /// A list of and-or lists, up to whatever ends it; the caller checks the
    /// terminator.
    fn list(&mut self, out: &mut Vec<Pipeline>) -> Parsed<()> {
        self.enter()?;
        loop {
            self.linebreaks()?;
            if self.at_list_end() {
                break;
            }
            self.and_or(out)?;
            self.blanks_and_comment();
            if self.looking_at(";;") || self.looking_at(";&") {
                break;
            }
            if !self.eat(";") && !self.eat("&") && self.peek() != Some('\n') {
                break;
            }
        }
        self.depth -= 1;
        Ok(())
    }

    fn and_or(&mut self, out: &mut Vec<Pipeline>) -> Parsed<()> {
        loop {
            out.push(self.pipeline()?);
            self.blanks();
            if !self.eat("&&") && !self.eat("||") {
                return Ok(());
            }
            self.linebreaks()?;
        }
    }

    fn pipeline(&mut self) -> Parsed<Pipeline> {
        self.blanks();
        if self.reserved() == Some("!") {
            self.pos += 1;
        }
        let mut commands = vec![self.command()?];
        loop {
            self.blanks();
            if self.looking_at("||") || !(self.eat("|&") || self.eat("|")) {
                return Ok(commands);
            }
            self.linebreaks()?;
            commands.push(self.command()?);
        }
    }

    fn command(&mut self) -> Parsed<Command> {
        self.blanks();
        let body = match self.reserved() {
            Some("if") => self.if_clause()?,
            Some(kw @ ("while" | "until")) => {
                self.pos += kw.len();
                let mut body = Vec::new();
                self.list(&mut body)?;
                self.expect_word("do")?;
                self.list(&mut body)?;
                self.expect_word("done")?;
                body
            }
            Some(kw @ ("for" | "select")) => {
                self.pos += kw.len();
                return self.for_clause();
            }
            Some("case") => self.case_clause()?,
            Some("{") => {
                self.pos += 1;
                let mut body = Vec::new();
                self.list(&mut body)?;
                self.expect_word("}")?;
                body
            }
            Some("function") => {
                self.pos += "function".len();
                self.blanks();
                self.word()?.ok_or(ParseError)?;
                self.blanks();
                self.eat("()");
                return self.function_body();
            }
            Some("[[") => return self.test_clause(),
            Some("then" | "elif" | "else" | "fi" | "do" | "done" | "esac" | "}" | "!") => {
                return Err(ParseError);
            }
            _ if self.eat("((") => {
                self.arithmetic()?;
                Vec::new()
            }
            _ if self.eat("(") => {
                let mut body = Vec::new();
                self.list(&mut body)?;
                if !self.eat(")") {
                    return Err(ParseError);
                }
                body
            }
            _ => return self.simple(),
        };
        let redirects = self.redirects()?;
        Ok(Command::Compound { body, redirects })
    }

    fn if_clause(&mut self) -> Parsed<Vec<Pipeline>> {
        self.pos += "if".len();
        let mut body = Vec::new();
        self.list(&mut body)?;
        self.expect_word("then")?;
        self.list(&mut body)?;
        loop {
            self.linebreaks()?;
            match self.reserved() {
                Some("elif") => {
                    self.pos += "elif".len();
                    self.list(&mut body)?;
                    self.expect_word("then")?;
                    self.list(&mut body)?;
                }
                Some("else") => {
                    self.pos += "else".len();
                    self.list(&mut body)?;
                }
                _ => break,
            }
        }
        self.expect_word("fi")?;
        Ok(body)
    }

    fn for_clause(&mut self) -> Parsed<Command> {
        self.blanks();
        let mut body = Vec::new();
        if self.eat("((") {
            self.arithmetic()?;
            self.blanks();
            self.eat(";");
            self.expect_word("do")?;
            self.list(&mut body)?;
            self.expect_word("done")?;
            let redirects = self.redirects()?;
            return Ok(Command::Compound { body, redirects });
        }
        let start = self.pos;
        while self.peek().is_some_and(is_name_char) {
            self.pos += 1;
        }
        let var = self.slice(start, self.pos);
        if !is_name(&var) {
            return Err(ParseError);
        }
        let mut items = Vec::new();
        self.linebreaks()?;
        if self.reserved() == Some("in") {
            self.pos += "in".len();
            loop {
                self.blanks_and_comment();
                match self.word()? {
                    Some(w) => items.push(w),
                    None => break,
                }
            }
        }
        self.blanks_and_comment();
        self.eat(";");
        self.expect_word("do")?;
        self.list(&mut body)?;
        self.expect_word("done")?;
        let redirects = self.redirects()?;
        Ok(Command::For {
            var,
            items,
            body,
            redirects,
        })
    }

    fn case_clause(&mut self) -> Parsed<Vec<Pipeline>> {
        self.pos += "case".len();
        self.blanks();
        self.word()?.ok_or(ParseError)?;
        self.expect_word("in")?;
        let mut body = Vec::new();
        loop {
            self.linebreaks()?;
            if self.reserved() == Some("esac") {
                self.pos += "esac".len();
                return Ok(body);
            }
            self.eat("(");
            loop {
                self.blanks();
                self.word()?.ok_or(ParseError)?;
                self.blanks();
                if !self.eat("|") {
                    break;
                }
            }
            if !self.eat(")") {
                return Err(ParseError);
            }
            self.list(&mut body)?;
            let _terminated = self.eat(";;&") || self.eat(";;") || self.eat(";&");
        }
    }

    /// `[[ … ]]`: operators inside are words, not separators.
    fn test_clause(&mut self) -> Parsed<Command> {
        self.pos += "[[".len();
        let mut words = vec![lit_word("[[")];
        loop {
            self.blanks();
            if self.eat("]]") {
                words.push(lit_word("]]"));
                break;
            }
            let op = ["&&", "||", "(", ")", "<", ">", "!"]
                .into_iter()
                .find(|op| self.looking_at(op));
            if let Some(op) = op {
                self.pos += op.len();
                words.push(lit_word(op));
            } else {
                words.push(self.word()?.ok_or(ParseError)?);
            }
        }
        let redirects = self.redirects()?;
        Ok(Command::Simple { words, redirects })
    }

    /// The text of `(( … ))` after the opening `((`. Opaque: arithmetic runs
    /// nothing, and a substitution inside it is found by the raw scan.
    fn arithmetic(&mut self) -> Parsed<()> {
        let mut depth = 0usize;
        loop {
            match self.next()? {
                '(' => depth += 1,
                ')' if depth > 0 => depth -= 1,
                ')' if self.eat(")") => return Ok(()),
                ')' => return Err(ParseError),
                _ => {}
            }
        }
    }

    /// A function body is assessed where it is defined: calling it is assumed.
    fn function_body(&mut self) -> Parsed<Command> {
        self.linebreaks()?;
        self.command()
    }

    fn redirects(&mut self) -> Parsed<Vec<Redirect>> {
        let mut out = Vec::new();
        loop {
            self.blanks();
            match self.redirect()? {
                Some(r) => out.push(r),
                None => return Ok(out),
            }
        }
    }

    /// A redirection at the current position, or `None` (position unchanged).
    fn redirect(&mut self) -> Parsed<Option<Redirect>> {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        // `<(…)` and `>(…)` are words (process substitution), not redirects.
        if self.looking_at("<(") || self.looking_at(">(") {
            self.pos = start;
            return Ok(None);
        }
        const OPS: &[&str] = &[
            "<<<", "<<-", "<<", "<>", "<&", "<", ">>", ">|", ">&", ">", "&>>", "&>",
        ];
        let Some(op) = OPS.iter().copied().find(|op| self.looking_at(op)) else {
            self.pos = start;
            return Ok(None);
        };
        if op.starts_with('&') && self.pos != start {
            self.pos = start;
            return Ok(None);
        }
        self.pos += op.len();
        self.blanks();
        let target = self.word()?.ok_or(ParseError)?;
        Ok(Some(match op {
            "<<" | "<<-" => {
                let delim = plain_text(&target.parts);
                let slot = self.heredocs.len();
                self.heredocs.push(String::new());
                self.pending.push(PendingHeredoc {
                    delim,
                    strip_tabs: op == "<<-",
                    slot,
                });
                Redirect::HereDoc(slot)
            }
            "<<<" => Redirect::HereString(target),
            ">&" if !is_fd_target(&target) => Redirect::Write { op: "&>", target },
            ">" | ">>" | ">|" | "&>" | "&>>" => Redirect::Write { op, target },
            _ => Redirect::Other,
        }))
    }

    fn simple(&mut self) -> Parsed<Command> {
        let mut words = Vec::new();
        let mut redirects = Vec::new();
        loop {
            self.blanks_and_comment();
            if let Some(r) = self.redirect()? {
                redirects.push(r);
                continue;
            }
            let Some(word) = self.word()? else { break };
            words.push(word);
            // `name() compound`: a function definition.
            if words.len() == 1 && redirects.is_empty() {
                let save = self.pos;
                self.blanks();
                if self.eat("(") {
                    self.blanks();
                    if !self.eat(")") {
                        return Err(ParseError);
                    }
                    return self.function_body();
                }
                self.pos = save;
            }
        }
        if words.is_empty() && redirects.is_empty() {
            return Err(ParseError);
        }
        Ok(Command::Simple { words, redirects })
    }

    /// One word at the current position, or `None` at a delimiter.
    fn word(&mut self) -> Parsed<Option<WordAst>> {
        let mut parts = Vec::new();
        let mut assign = None;

        // `NAME=` or `NAME+=` starts an assignment; `NAME=(` an array.
        let start = self.pos;
        while self.peek().is_some_and(is_name_char) {
            self.pos += 1;
        }
        let name = self.slice(start, self.pos);
        let eq = if self.looking_at("+=") { "+=" } else { "=" };
        if is_name(&name) && self.eat(eq) {
            parts.push(Part::Lit(format!("{name}{eq}")));
            if self.eat("(") {
                let elements = self.array(&mut parts)?;
                return Ok(Some(WordAst {
                    parts,
                    assign: Some((name, elements)),
                }));
            }
            // The value gets its own part, so it can be read back alone.
            parts.push(Part::Lit(String::new()));
            assign = Some(name);
        } else {
            self.pos = start;
        }
        let value_from = parts.len().saturating_sub(1);

        // Process substitution, including Zsh's `=(…)`, at word start.
        for (open, placeholder) in [("<(", "<()"), (">(", ">()"), ("=(", "=()")] {
            if parts.is_empty() && self.eat(open) {
                self.substitution()?;
                parts.push(Part::Dynamic(placeholder));
            }
        }

        while let Some(c) = self.peek() {
            if is_meta(c) {
                break;
            }
            self.pos += 1;
            match c {
                '\\' if self.eat("\n") => {}
                '\\' => match self.peek() {
                    Some(next) => {
                        self.pos += 1;
                        push_lit(&mut parts, next);
                    }
                    None => push_lit(&mut parts, '\\'),
                },
                '\'' => {
                    parts.push(Part::Lit(String::new()));
                    loop {
                        match self.next()? {
                            '\'' => break,
                            ch => push_lit(&mut parts, ch),
                        }
                    }
                }
                '"' => self.double_quoted(&mut parts)?,
                '$' => self.dollar(&mut parts, false)?,
                '`' => self.backtick(&mut parts)?,
                other => push_lit(&mut parts, other),
            }
        }

        if parts.is_empty() {
            return Ok(None);
        }
        let assign = assign.map(|name| (name, vec![parts[value_from..].to_vec()]));
        Ok(Some(WordAst { parts, assign }))
    }

    /// Elements of `NAME=( … )`, after the `(`. Each element is also appended
    /// to `parts` so the word reads `NAME=(a b)`.
    fn array(&mut self, parts: &mut Vec<Part>) -> Parsed<Vec<Vec<Part>>> {
        let mut elements = Vec::new();
        parts.push(Part::Lit("(".into()));
        loop {
            self.linebreaks()?;
            if self.eat(")") {
                parts.push(Part::Lit(")".into()));
                return Ok(elements);
            }
            let element = self.word()?.ok_or(ParseError)?;
            if !elements.is_empty() {
                parts.push(Part::Lit(" ".into()));
            }
            parts.extend(element.parts.iter().cloned());
            elements.push(element.parts);
        }
    }

    fn double_quoted(&mut self, parts: &mut Vec<Part>) -> Parsed<()> {
        parts.push(Part::Lit(String::new()));
        loop {
            match self.next()? {
                '"' => return Ok(()),
                '\\' => match self.next()? {
                    '\n' => {}
                    c @ ('$' | '`' | '"' | '\\') => push_lit(parts, c),
                    c => {
                        push_lit(parts, '\\');
                        push_lit(parts, c);
                    }
                },
                '$' => self.dollar(parts, true)?,
                '`' => self.backtick(parts)?,
                c => push_lit(parts, c),
            }
        }
    }

    /// After a `$`.
    fn dollar(&mut self, parts: &mut Vec<Part>, quoted: bool) -> Parsed<()> {
        if !quoted && self.eat("'") {
            return self.ansi_c(parts);
        }
        if !quoted && self.eat("\"") {
            return self.double_quoted(parts);
        }
        if self.eat("((") {
            self.arithmetic()?;
            parts.push(Part::Dynamic(""));
            return Ok(());
        }
        if self.eat("(") {
            self.substitution()?;
            parts.push(Part::Dynamic(""));
            return Ok(());
        }
        if self.eat("{") {
            return self.braced(parts, quoted);
        }
        let start = self.pos;
        while self.peek().is_some_and(is_name_char) {
            self.pos += 1;
        }
        let name = self.slice(start, self.pos);
        if is_name(&name) {
            parts.push(Part::Var {
                raw: format!("${name}"),
                name,
                quoted,
            });
            return Ok(());
        }
        // `$1`, `$@`, `$?`, … : one character, kept as written.
        self.pos = start;
        match self.peek() {
            Some(c) if c.is_ascii_digit() || "@*#?$!-".contains(c) => {
                self.pos += 1;
                parts.push(Part::Lit(format!("${c}")));
            }
            _ => push_lit(parts, '$'),
        }
        Ok(())
    }

    /// `${ … }` after the `{`.
    fn braced(&mut self, parts: &mut Vec<Part>, quoted: bool) -> Parsed<()> {
        let start = self.pos;
        let mut depth = 0usize;
        loop {
            match self.next()? {
                '\\' => {
                    self.next()?;
                }
                '{' => depth += 1,
                '}' if depth == 0 => break,
                '}' => depth -= 1,
                _ => {}
            }
        }
        let inner = self.slice(start, self.pos - 1);
        let raw = format!("${{{inner}}}");
        if is_name(&inner) {
            parts.push(Part::Var {
                name: inner,
                raw,
                quoted,
            });
        } else {
            let dynamic = inner.contains("$(") || inner.contains('`');
            parts.push(Part::Lit(raw));
            if dynamic {
                parts.push(Part::Dynamic(""));
            }
        }
        Ok(())
    }

    /// `$'…'` after the quote: C escapes decoded.
    fn ansi_c(&mut self, parts: &mut Vec<Part>) -> Parsed<()> {
        parts.push(Part::Lit(String::new()));
        loop {
            let c = match self.next()? {
                '\'' => return Ok(()),
                '\\' => match self.next()? {
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    'a' => '\x07',
                    'b' => '\x08',
                    'e' | 'E' => '\x1b',
                    'f' => '\x0c',
                    'v' => '\x0b',
                    'x' => self.code_point(16, 2, 'x'),
                    'u' => self.code_point(16, 4, 'u'),
                    'U' => self.code_point(16, 8, 'U'),
                    d @ '0'..='7' => {
                        self.pos -= 1;
                        self.code_point(8, 3, d)
                    }
                    c @ ('\\' | '\'' | '"' | '?') => c,
                    other => {
                        push_lit(parts, '\\');
                        other
                    }
                },
                c => c,
            };
            push_lit(parts, c);
        }
    }

    /// Up to `max` digits in `radix`; with none, the escape stays literal
    /// (`\x` reads as `x`, as bash prints it).
    fn code_point(&mut self, radix: u32, max: usize, letter: char) -> char {
        let start = self.pos;
        while self.pos - start < max && self.peek().is_some_and(|c| c.is_digit(radix)) {
            self.pos += 1;
        }
        let digits = self.slice(start, self.pos);
        match u32::from_str_radix(&digits, radix) {
            Ok(n) => char::from_u32(n).unwrap_or(char::REPLACEMENT_CHARACTER),
            Err(_empty) => letter,
        }
    }

    /// The body of `$(…)` / `<(…)` after the `(`, through the `)`.
    fn substitution(&mut self) -> Parsed<()> {
        let start = self.pos;
        self.subst_depth += 1;
        let mut ignored = Vec::new();
        self.list(&mut ignored)?;
        if !self.eat(")") {
            return Err(ParseError);
        }
        self.subst_depth -= 1;
        if self.subst_depth == 0 {
            self.bodies.push(self.slice(start, self.pos - 1));
        }
        Ok(())
    }

    fn backtick(&mut self, parts: &mut Vec<Part>) -> Parsed<()> {
        let mut body = String::new();
        loop {
            match self.next()? {
                '`' => break,
                '\\' => match self.next()? {
                    c @ ('`' | '\\' | '$') => body.push(c),
                    c => {
                        body.push('\\');
                        body.push(c);
                    }
                },
                c => body.push(c),
            }
        }
        if self.subst_depth == 0 {
            self.bodies.push(body);
        }
        parts.push(Part::Dynamic(""));
        Ok(())
    }
}

fn lit_word(text: &str) -> WordAst {
    WordAst {
        parts: vec![Part::Lit(text.to_string())],
        assign: None,
    }
}

fn push_lit(parts: &mut Vec<Part>, c: char) {
    if let Some(Part::Lit(s)) = parts.last_mut() {
        s.push(c);
    } else {
        parts.push(Part::Lit(c.to_string()));
    }
}

/// The literal text of a word, ignoring anything that would need expanding.
fn plain_text(parts: &[Part]) -> String {
    parts
        .iter()
        .filter_map(|p| match p {
            Part::Lit(s) => Some(s.as_str()),
            _ => None,
        })
        .collect()
}

/// `>&2`, `>&-`, `>&2-`: duplicating or closing a descriptor writes no file.
/// A target that needs expanding might name a file, so it is not one.
fn is_fd_target(target: &WordAst) -> bool {
    let literal = target.parts.iter().all(|p| matches!(p, Part::Lit(_)));
    let text = plain_text(&target.parts);
    let digits = text.strip_suffix('-').unwrap_or(&text);
    literal && digits.chars().all(|c| c.is_ascii_digit())
}

/// Turns the AST into segments for one reading of the variables.
struct Lower<'p> {
    heredocs: &'p [String],
    /// Every value each variable was given so far, in any branch. Values are
    /// only ever added: an assignment in an `if` arm does not undo one before
    /// it, because either may be the one that ran.
    env: HashMap<String, Vec<Word>>,
    /// `None` keeps variables as written; `Some(k)` uses each variable's
    /// `k`-th value (its last, when it has fewer).
    pick: Option<usize>,
    max_values: usize,
    segments: Vec<Segment>,
    scripts: Vec<String>,
}

impl<'p> Lower<'p> {
    fn new(heredocs: &'p [String], pick: Option<usize>) -> Self {
        Self {
            heredocs,
            env: HashMap::new(),
            pick,
            max_values: 0,
            segments: Vec::new(),
            scripts: Vec::new(),
        }
    }

    fn pipelines(&mut self, pipelines: &[Pipeline]) {
        for pipeline in pipelines {
            let start = self.segments.len();
            let mut data = Vec::new();
            for command in pipeline {
                self.command(command, &mut data);
            }
            let feeds_shell = self.segments[start..]
                .iter()
                .any(|s| s.command().is_some_and(|c| SHELLS.contains(&c)));
            if feeds_shell {
                self.scripts.extend(data);
            }
        }
    }

    fn command(&mut self, command: &Command, data: &mut Vec<String>) {
        match command {
            Command::Simple { words, redirects } => {
                let mut out = self.simple_words(words);
                self.redirect_words(redirects, data, &mut out);
                self.push(out);
            }
            Command::Compound { body, redirects } => {
                self.pipelines(body);
                let mut out = Vec::new();
                self.redirect_words(redirects, data, &mut out);
                self.push(out);
            }
            Command::For {
                var,
                items,
                body,
                redirects,
            } => {
                let values: Vec<Word> = items.iter().flat_map(|w| self.word(w)).collect();
                self.env.entry(var.clone()).or_default().extend(values);
                self.pipelines(body);
                let mut out = Vec::new();
                self.redirect_words(redirects, data, &mut out);
                self.push(out);
            }
        }
    }

    fn push(&mut self, words: Vec<Word>) {
        if !words.is_empty() {
            self.segments.push(Segment { words });
        }
    }

    fn simple_words(&mut self, words: &[WordAst]) -> Vec<Word> {
        // Prefix assignments first. Bash expands the command's own words
        // before applying them, so `X=~ rm -rf $X` sees the old `X`; it is
        // read here as the new one anyway, because that is what the command
        // was written to mean and it is the reading that can only add risk.
        let prefix = words.iter().take_while(|w| w.assign.is_some()).count();
        for w in &words[..prefix] {
            self.bind(w);
        }
        let name = words.get(prefix).map(|w| plain_text(&w.parts));
        if name.is_some_and(|n| BINDERS.contains(&n.as_str())) {
            for w in &words[prefix..] {
                self.bind(w);
            }
        }
        words.iter().flat_map(|w| self.word(w)).collect()
    }

    fn bind(&mut self, word: &WordAst) {
        if let Some((name, values)) = &word.assign {
            let values: Vec<Word> = values.iter().map(|v| self.text(v)).collect();
            self.env.entry(name.clone()).or_default().extend(values);
        }
    }

    fn redirect_words(
        &mut self,
        redirects: &[Redirect],
        data: &mut Vec<String>,
        out: &mut Vec<Word>,
    ) {
        for r in redirects {
            match r {
                Redirect::Write { op, target } => {
                    out.push(Word::new((*op).to_string(), false));
                    out.extend(self.word(target));
                }
                Redirect::HereDoc(slot) => data.push(self.heredocs[*slot].clone()),
                Redirect::HereString(w) => data.push(self.text(&w.parts).text),
                Redirect::Other => {}
            }
        }
    }

    /// The value of `$name` in this reading, if it has one.
    fn value(&mut self, name: &str) -> Option<Word> {
        let values = self.env.get(name)?;
        self.max_values = self.max_values.max(values.len());
        let pick = self.pick?;
        values
            .get(pick.min(values.len().saturating_sub(1)))
            .cloned()
    }

    /// A word as the segments see it. An unquoted variable that is the whole
    /// word is split on whitespace, as the shell would split it.
    fn word(&mut self, word: &WordAst) -> Vec<Word> {
        if let [
            Part::Var {
                name,
                quoted: false,
                ..
            },
        ] = word.parts.as_slice()
            && let Some(value) = self.value(name)
        {
            let fields: Vec<Word> = value
                .text
                .split_whitespace()
                .map(|f| Word::new(f.to_string(), value.dynamic))
                .collect();
            return if fields.is_empty() && value.dynamic {
                vec![Word::new(String::new(), true)]
            } else {
                fields
            };
        }
        vec![self.text(&word.parts)]
    }

    fn text(&mut self, parts: &[Part]) -> Word {
        let mut text = String::new();
        let mut dynamic = false;
        for part in parts {
            match part {
                Part::Lit(s) => text.push_str(s),
                Part::Var { name, raw, .. } => match self.value(name) {
                    Some(v) => {
                        text.push_str(&v.text);
                        dynamic |= v.dynamic;
                    }
                    None => text.push_str(raw),
                },
                Part::Dynamic(placeholder) => {
                    text.push_str(placeholder);
                    dynamic = true;
                }
            }
        }
        Word::new(text, dynamic)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Words of each segment in the as-written reading.
    fn segs(input: &str) -> Vec<Vec<String>> {
        let lowered = parse(input).unwrap();
        lowered.variants[0]
            .iter()
            .map(|s| s.words.iter().map(|w| w.text.clone()).collect())
            .collect()
    }

    /// Words of each segment in every reading.
    fn readings(input: &str) -> Vec<Vec<Vec<String>>> {
        parse(input)
            .unwrap()
            .variants
            .iter()
            .map(|v| {
                v.iter()
                    .map(|s| s.words.iter().map(|w| w.text.clone()).collect())
                    .collect()
            })
            .collect()
    }

    fn word(input: &str) -> Word {
        parse(input).unwrap().variants[0][0].words[1].clone()
    }

    fn fails(input: &str) -> bool {
        parse(input) == Err(ParseError)
    }

    #[test]
    fn lists_and_pipelines_split_into_segments() {
        assert_eq!(
            segs("a && b || c; d & e | f |& g\nh"),
            [["a"], ["b"], ["c"], ["d"], ["e"], ["f"], ["g"], ["h"]]
        );
        assert_eq!(segs("! a | b"), [["a"], ["b"]]);
        assert_eq!(segs("a &&\n b"), [["a"], ["b"]]);
        assert_eq!(segs("ls \\\n -la"), [["ls", "-la"]]);
        // Comments are re-read by the tokenizer and appended (`cmd.exe` has
        // no `#` comments).
        assert_eq!(
            segs("ls # rm -rf ~ & b\n# c\npwd"),
            vec![
                vec!["ls"],
                vec!["pwd"],
                vec!["#", "rm", "-rf", "~"],
                vec!["b"],
                vec!["#", "c"]
            ]
        );
        assert!(parse("ls # it's").unwrap().malformed);
        assert!(!parse("ls # fine").unwrap().malformed);
        assert!(segs("").is_empty());
        assert!(segs(" \n ").is_empty());
    }

    #[test]
    fn syntax_errors_fall_back() {
        for input in [
            ";",
            "a; ;",
            "| a",
            "a )",
            "then ls",
            "fi",
            "! ! ls",
            "(ls",
            "echo >",
            "echo (",
            "f (x",
            "[[ -f x",
            "if a; then b",
            "case x in a) ls",
            "case x in a ls",
            "case",
            "for 1 in a; do b; done",
            "function",
            "((a)b))",
            "((a",
        ] {
            assert!(fails(input), "{input:?}");
        }
        assert!(fails(&"( ".repeat(MAX_DEPTH + 2)));
    }

    #[test]
    fn compound_commands_are_walked() {
        assert_eq!(
            segs("if a; then b; elif c; then d; else e; fi"),
            [["a"], ["b"], ["c"], ["d"], ["e"]]
        );
        assert_eq!(segs("while a; do b; done"), [["a"], ["b"]]);
        assert_eq!(segs("until a\ndo\n b\ndone"), [["a"], ["b"]]);
        assert_eq!(segs("{ a; b; }"), [["a"], ["b"]]);
        assert_eq!(segs("(a; (b))"), [["a"], ["b"]]);
        assert_eq!(segs("((i++)); (( (1+2) * 3 ))"), Vec::<Vec<String>>::new());
        assert_eq!(segs("for ((i=0; i<3; i++)); do a; done"), [["a"]]);
        assert_eq!(segs("for x; do a; done"), [["a"]]);
        assert_eq!(segs("select x in a b\ndo c; done"), [["c"]]);
        assert_eq!(
            segs("case $x in (a|b) c;; *) d;& e) f;;& esac"),
            [["c"], ["d"], ["f"]]
        );
        assert_eq!(segs("case x in\n a) ;;\nesac"), Vec::<Vec<String>>::new());
        assert_eq!(segs("f() { a; }"), [["a"]]);
        assert_eq!(segs("f ( ) (b)"), [["b"]]);
        assert_eq!(segs("function g { a; }"), [["a"]]);
        assert_eq!(segs("function g() { a; }"), [["a"]]);
        assert_eq!(
            segs("[[ -f a && ( ! -d b || c < d ) ]] > out"),
            [[
                "[[", "-f", "a", "&&", "(", "!", "-d", "b", "||", "c", "<", "d", ")", "]]", ">",
                "out"
            ]]
        );
        // Redirections on a compound command become their own segment.
        assert_eq!(
            segs("{ a; } > /etc/x"),
            vec![vec!["a"], vec![">", "/etc/x"]]
        );
        assert_eq!(
            segs("for x in y; do a; done >> log"),
            vec![vec!["a"], vec![">>", "log"]]
        );
        assert_eq!(
            segs("for ((;;)); do a; done > log"),
            vec![vec!["a"], vec![">", "log"]]
        );
        // Reserved words are only reserved in the command slot.
        assert_eq!(
            segs("echo if then } { !x"),
            [["echo", "if", "then", "}", "{", "!x"]]
        );
    }

    #[test]
    fn redirections_are_typed() {
        assert_eq!(segs("> out ls"), [["ls", ">", "out"]]);
        assert_eq!(
            segs("ls 2>err 1>>log >|clob"),
            [["ls", ">", "err", ">>", "log", ">|", "clob"]]
        );
        assert_eq!(
            segs("ls &>all &>>more"),
            [["ls", "&>", "all", "&>>", "more"]]
        );
        assert_eq!(segs("ls >&file"), [["ls", "&>", "file"]]);
        assert_eq!(segs("ls >& $f"), [["ls", "&>", "$f"]]);
        // Duplicating, closing and reading write no file.
        assert_eq!(segs("ls 2>&1 >&- 3>&2- <&0 <in <>rw"), [["ls"]]);
        assert_eq!(segs("ls 2 a2>x"), [["ls", "2", "a2", ">", "x"]]);
        assert_eq!(segs("ls 2&>x"), [["ls", "2", "&>", "x"]]);
    }

    #[test]
    fn here_documents_are_data() {
        let l = parse("cat <<EOF > out\nrm -rf ~\nEOF\nls").unwrap();
        assert_eq!(
            segs("cat <<EOF > out\nrm -rf ~\nEOF\nls"),
            vec![vec!["cat", ">", "out"], vec!["ls"]]
        );
        assert!(l.scripts.is_empty());
        // `<<-` strips leading tabs; quoted delimiters; CRLF line ends.
        assert_eq!(segs("cat <<-'E'\n\tx\n\tE\nls"), [["cat"], ["ls"]]);
        assert_eq!(segs("cat <<\"E\"\r\nx\r\nE\r\n"), [["cat"]]);
        assert_eq!(segs("a <<A; b <<B\n1\nA\n2\nB\nc"), [["a"], ["b"], ["c"]]);
        // A here-document inside a substitution.
        assert_eq!(segs("echo $(cat <<E\nx)\nE\n)"), [["echo", ""]]);
        // No body, or no terminator.
        assert!(fails("cat <<EOF"));
        assert!(fails("cat <<EOF\nbody"));
    }

    #[test]
    fn stdin_fed_to_a_shell_is_a_script() {
        let l = parse("bash <<EOF\nrm -rf ~\nEOF").unwrap();
        assert_eq!(l.scripts, ["rm -rf ~\n"]);
        let l = parse("cat <<EOF | sudo sh\nx\nEOF").unwrap();
        assert_eq!(l.scripts, ["x\n"]);
        let l = parse("bash <<< \"$cmd\"").unwrap();
        assert_eq!(l.scripts, ["$cmd"]);
        assert!(parse("cat <<< hi").unwrap().scripts.is_empty());
    }

    #[test]
    fn quoting_is_resolved_exactly() {
        assert_eq!(
            segs(r#"a 'x y' "p q" r\ s ''"#),
            [["a", "x y", "p q", "r s", ""]]
        );
        assert_eq!(segs("a b\\\nc"), [["a", "bc"]]);
        assert_eq!(segs("a \\"), [["a", "\\"]]);
        assert_eq!(
            segs(
                r#"a "\$ \` \" \ \x" "l\
m""#
            ),
            [["a", r#"$ ` " \ \x"#, "lm"]]
        );
        assert_eq!(segs(r#"a "$'x'" "$""#), [["a", "$'x'", "$"]]);
        assert_eq!(
            segs(r#"a $"loc" $'\x2f\101\u00e9\U0001F600\ud800\x\q\\\'\"\?'"#),
            [["a", "loc", "/A\u{e9}\u{1F600}\u{FFFD}x\\q\\'\"?"]]
        );
        assert_eq!(
            segs(r"a $'\n\t\r\a\b\e\E\f\v'"),
            [["a", "\n\t\r\x07\x08\x1b\x1b\x0c\x0b"]]
        );
        for input in [
            "a 'x", "a \"x", "a $'x", "a $'\\", "a `x", "a `\\", "a \"\\",
        ] {
            assert!(fails(input), "{input:?}");
        }
    }

    #[test]
    fn expansions_keep_their_spelling() {
        assert_eq!(
            segs(r"a $HOME ${HOME} ${x:-y} $1 $@ $? $ $% ${a{b}c} ${x\{\}}"),
            [[
                "a",
                "$HOME",
                "${HOME}",
                "${x:-y}",
                "$1",
                "$@",
                "$?",
                "$",
                "$%",
                "${a{b}c}",
                r"${x\{\}}"
            ]]
        );
        assert!(fails("a ${x"));
        assert!(fails("a ${x\\"));
        // Substitutions are dynamic and leave no text, as in the tokenizer.
        let w = word("a x$(ls)y");
        assert_eq!((w.text.as_str(), w.dynamic), ("xy", true));
        for input in [
            "a `ls`",
            "a $((1+2))",
            "a ${x:-$(ls)}",
            "a ${x:-`ls`}",
            "a \"$(ls)\"",
        ] {
            assert!(word(input).dynamic, "{input:?}");
        }
        assert!(!word("a ${x:-y}").dynamic);
        // Process substitution keeps the tokenizer's placeholder.
        assert_eq!(segs("a <(b) >(c) =(d)"), [["a", "<()", ">()", "=()"]]);
    }

    #[test]
    fn substitution_bodies_are_outermost_only() {
        let l = parse(r#"a $(b $(c) `d`) `e \` \\ \$ \x` <(f) "$(g ")")""#).unwrap();
        assert_eq!(l.bodies, ["b $(c) `d`", r"e ` \ $ \x", "f", r#"g ")""#]);
        assert!(fails("a $(b"));
        assert!(fails("a $(b ]"));
    }

    #[test]
    fn assignments_are_recorded() {
        assert_eq!(segs("A=1 B+=2 C= cmd"), [["A=1", "B+=2", "C=", "cmd"]]);
        assert_eq!(segs("A=(x 'y z'\n w) cmd"), [["A=(x y z w)", "cmd"]]);
        assert!(fails("A=(x"));
        assert!(fails("A=(x ;"));
        // Readings: as written, then each value in turn.
        assert_eq!(
            readings("D=/; rm $D"),
            [
                vec![vec!["D=/"], vec!["rm", "$D"]],
                vec![vec!["D=/"], vec!["rm", "/"]]
            ]
        );
        assert_eq!(readings("A=(x y); rm $A")[2][1], ["rm", "y"]);
        assert_eq!(readings("export E=~ && rm \"$E\"")[1][1], ["rm", "~"]);
        assert_eq!(
            readings("make CC=/ && rm $CC").len(),
            1,
            "an argument is not an assignment"
        );
        // Values only accumulate; later readings take each one.
        let r = readings("D=a; D=b; rm $D");
        assert_eq!(
            (r.len(), &r[1][2], &r[2][2]),
            (
                3,
                &vec!["rm".to_string(), "a".into()],
                &vec!["rm".to_string(), "b".into()]
            )
        );
        // Values chain, and unquoted ones are split.
        assert_eq!(readings("A=/; B=\"$A x\"; rm $B")[1][2], ["rm", "/", "x"]);
        assert_eq!(readings("A='a b'; rm \"$A\"")[1][1], ["rm", "a b"]);
        // A loop variable takes each item.
        assert_eq!(readings("for d in a ~; do rm $d; done")[2][0], ["rm", "~"]);
        assert_eq!(readings("for d; do rm $d; done")[0][0], ["rm", "$d"]);
        // A value from a substitution stays dynamic.
        let v = parse("X=$(ls); rm $X").unwrap().variants;
        assert_eq!(v[1][1].words[1], Word::new(String::new(), true));
        let v = parse("X=\"$(ls) /\"; rm $X").unwrap().variants;
        assert_eq!(v[1][1].words[1], Word::new("/".into(), true));
        let v = parse("X=$(ls)/; rm \"$X\"").unwrap().variants;
        assert_eq!(v[1][1].words[1], Word::new("/".into(), true));
        let v = parse("X=; rm $X").unwrap().variants;
        assert_eq!(v[1][1].words.len(), 1);
        // Readings are capped.
        let many: String = (0..40).map(|i| format!("D={i}; ")).collect::<String>() + "rm $D";
        assert_eq!(parse(&many).unwrap().variants.len(), MAX_VARIANTS + 1);
    }
}
