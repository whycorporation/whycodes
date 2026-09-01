use std::sync::OnceLock;

use regex::Regex;

use crate::highlight::highlight_code;
use crate::mermaid::{is_mermaid_language, render_mermaid};

/// Compile a static inline-markdown regex once. Invalid patterns return `None`
/// (no panic) so the line is left unstyled rather than aborting the TUI.
fn re_code() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"`([^`]+)`").ok()).as_ref()
}

fn re_bold() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\*\*(.+?)\*\*").ok())
        .as_ref()
}

fn re_italic() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\*(.+?)\*").ok()).as_ref()
}

fn re_link() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").ok())
        .as_ref()
}

// ── Structured markdown ────────────────────────────────────────────────
//
// `render_markdown` below emits ANSI, which suits a plain terminal but not
// ratatui, which needs `Style` values rather than escape sequences. Parsing to
// this structure first lets each frontend render it its own way. Fenced code
// blocks use the shared syntect/two-face highlighter (Tokyo Night).

/// A run of text within a line, carrying its emphasis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    Bold(String),
    Italic(String),
    Code(String),
    Link { text: String, url: String },
}

impl Inline {
    /// The characters this run contributes, ignoring emphasis.
    pub fn text(&self) -> &str {
        match self {
            Self::Text(s) | Self::Bold(s) | Self::Italic(s) | Self::Code(s) => s,
            Self::Link { text, .. } => text,
        }
    }
}

/// Column alignment from a GFM separator row (`---`, `:---`, `---:`, `:---:`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TableAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// A block-level element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading {
        level: u8,
        spans: Vec<Inline>,
    },
    Paragraph(Vec<Inline>),
    ListItem {
        indent: usize,
        /// `None` = bullet; `Some(n)` = ordered list starting at n.
        number: Option<u32>,
        spans: Vec<Inline>,
    },
    /// A fenced block. `closed` is false while the closing fence has not
    /// arrived yet, which happens constantly during streaming.
    Code {
        language: Option<String>,
        lines: Vec<String>,
        closed: bool,
    },
    /// GFM pipe table: header + separator + zero or more body rows.
    ///
    /// Cells are plain text (inline markup stripped to display text). The TUI
    /// paints this as an aligned box; without this variant pipe lines soft-wrap
    /// as paragraphs and look broken.
    Table {
        headers: Vec<String>,
        aligns: Vec<TableAlign>,
        rows: Vec<Vec<String>>,
    },
    Blank,
}

/// One highlighted run of code: 24-bit colour and the text it applies to.
pub use crate::highlight::CodeSpan;

/// Syntax-highlight code into coloured runs (re-export for TUI consumers).
pub use crate::highlight::highlight_code_spans;

/// Parse markdown into blocks.
///
/// Deliberately line-oriented and total: any input produces blocks, and an
/// unterminated fence yields `Code { closed: false }` rather than an error, so
/// a partially streamed response still renders.
///
/// GFM pipe tables are detected when a row is immediately followed by a
/// separator (`|---|---|`); body rows are consumed until a blank / non-row.
pub fn parse_markdown(text: &str) -> Vec<Block> {
    let lines: Vec<&str> = text.lines().collect();
    let mut blocks = Vec::new();
    let mut fence: Option<(Option<String>, Vec<String>)> = None;
    let mut i = 0usize;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();

        if trimmed.starts_with("```") {
            match fence.take() {
                Some((language, code_lines)) => blocks.push(Block::Code {
                    language,
                    lines: code_lines,
                    closed: true,
                }),
                None => {
                    let lang = trimmed.trim_start_matches('`').trim();
                    fence = Some(((!lang.is_empty()).then(|| lang.to_string()), Vec::new()));
                }
            }
            i += 1;
            continue;
        }

        if let Some((_, code_lines)) = fence.as_mut() {
            code_lines.push(line.to_string());
            i += 1;
            continue;
        }

        // GFM table: header + separator (+ optional body). Needs a lookahead.
        if let Some((table, consumed)) = try_parse_table(&lines[i..]) {
            blocks.push(table);
            i += consumed;
            continue;
        }

        if let Some((level, rest)) = heading(trimmed) {
            blocks.push(Block::Heading {
                level,
                spans: parse_inline(rest),
            });
        } else if let Some((number, rest)) = list_item(trimmed) {
            blocks.push(Block::ListItem {
                indent: line.len() - trimmed.len(),
                number,
                spans: parse_inline(rest),
            });
        } else if trimmed.is_empty() {
            blocks.push(Block::Blank);
        } else {
            blocks.push(Block::Paragraph(parse_inline(line)));
        }
        i += 1;
    }

    if let Some((language, code_lines)) = fence {
        blocks.push(Block::Code {
            language,
            lines: code_lines,
            closed: false,
        });
    }

    blocks
}

/// Last byte offset of a *stable* prefix while `text` is still growing.
///
/// Grok Build checkpoint: content before this offset cannot change if more
/// bytes are appended, so a streaming renderer freezes those output lines
/// and only re-parses the tail (O(N) instead of O(N²) over a turn).
///
/// Conservative on purpose — a later chunk must not retroactively merge
/// frozen text into a new block (open fence, GFM table waiting on `---`).
/// Newline scan is byte-wise (`b'\n'`) so we never allocate `.lines()`.
pub fn last_checkpoint(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut in_fence = false;
    let mut last = 0usize;
    let mut line_start = 0usize;
    let mut i = 0usize;
    while i <= bytes.len() {
        if i == bytes.len() || bytes[i] == b'\n' {
            let line = &text[line_start..i];
            let is_fence = line.trim_start().starts_with("```");
            if is_fence {
                in_fence = !in_fence;
                if !in_fence && i < bytes.len() {
                    last = i + 1;
                }
            } else if !in_fence && i < bytes.len() {
                let t = line.trim();
                if t.is_empty() || (!looks_like_table_row(line) && !is_table_separator(line)) {
                    last = i + 1;
                }
            }
            line_start = i.saturating_add(1);
        }
        if i == bytes.len() {
            break;
        }
        i += 1;
    }
    last.min(text.len())
}

/// Try to parse a GFM pipe table starting at `lines[0]`.
///
/// Returns `(Block::Table, lines_consumed)` or `None` if this is not a table
/// (e.g. a lone `| a | b |` without a separator — still a paragraph while
/// streaming, until the `---` row arrives).
fn try_parse_table(lines: &[&str]) -> Option<(Block, usize)> {
    if lines.len() < 2 {
        return None;
    }
    let header_line = lines[0];
    let sep_line = lines[1];
    if !looks_like_table_row(header_line) || !is_table_separator(sep_line) {
        return None;
    }

    let headers = split_table_cells(header_line);
    if headers.is_empty() || headers.iter().all(|h| h.is_empty()) {
        return None;
    }
    let col_count = headers.len();
    let aligns = parse_table_alignments(sep_line, col_count);

    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut n = 2usize;
    while n < lines.len() {
        let l = lines[n];
        if l.trim().is_empty() {
            break;
        }
        // Don't swallow the next table's separator or a non-row paragraph.
        if is_table_separator(l) || !looks_like_table_row(l) {
            break;
        }
        let mut cells = split_table_cells(l);
        if cells.len() > col_count {
            cells.truncate(col_count);
        }
        cells.resize(col_count, String::new());
        rows.push(cells);
        n += 1;
    }

    Some((
        Block::Table {
            headers,
            aligns,
            rows,
        },
        n,
    ))
}

/// A table data/header row has at least two pipe-separated cells.
fn looks_like_table_row(line: &str) -> bool {
    let t = line.trim();
    if !t.contains('|') {
        return false;
    }
    // Reject pure separator rows here — they are handled by `is_table_separator`.
    if is_table_separator(t) {
        return false;
    }
    split_table_cells(t).len() >= 2
}

/// GFM separator: each cell is dashes with optional leading/trailing colons.
pub(crate) fn is_table_separator(line: &str) -> bool {
    let cells = split_table_cells(line);
    if cells.is_empty() {
        return false;
    }
    cells.iter().all(|c| {
        let c = c.trim();
        if c.is_empty() {
            return false;
        }
        let mut saw_dash = false;
        for ch in c.chars() {
            match ch {
                '-' => saw_dash = true,
                ':' | ' ' => {}
                _ => return false,
            }
        }
        saw_dash
    })
}

fn parse_table_alignments(sep_line: &str, col_count: usize) -> Vec<TableAlign> {
    let mut aligns: Vec<TableAlign> = split_table_cells(sep_line)
        .iter()
        .map(|c| {
            let c = c.trim();
            let left = c.starts_with(':');
            let right = c.ends_with(':');
            match (left, right) {
                (true, true) => TableAlign::Center,
                (false, true) => TableAlign::Right,
                _ => TableAlign::Left,
            }
        })
        .collect();
    aligns.resize(col_count, TableAlign::Left);
    aligns.truncate(col_count);
    aligns
}

/// Split a pipe row into cell strings; strips surrounding pipes.
///
/// Inline markdown in a cell is flattened to plain display text so column
/// widths stay stable (bold markers would skew alignment).
pub(crate) fn split_table_cells(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    // Trailing empty after final `|` already stripped; still split on remaining.
    if t.is_empty() {
        return Vec::new();
    }
    t.split('|')
        .map(|cell| flatten_inline(cell.trim()))
        .collect()
}

/// `**bold**` / `` `code` `` → plain text for table cell measurement.
fn flatten_inline(s: &str) -> String {
    parse_inline(s)
        .into_iter()
        .map(|span| span.text().to_string())
        .collect()
}

fn heading(trimmed: &str) -> Option<(u8, &str)> {
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) {
        trimmed[hashes..]
            .strip_prefix(' ')
            .map(|rest| (hashes as u8, rest))
    } else {
        None
    }
}

/// Bullet or ordered list marker. Returns `(number, rest)` where `number` is
/// `None` for bullets and `Some(n)` for `n. ` markers.
fn list_item(trimmed: &str) -> Option<(Option<u32>, &str)> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return Some((None, rest));
        }
    }
    // Ordered: `1. `, `12. `, …
    let digits = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0
        && trimmed[digits..].starts_with(". ")
        && let Ok(n) = trimmed[..digits].parse::<u32>()
    {
        return Some((Some(n), &trimmed[digits + 2..]));
    }
    None
}

/// Split a line into emphasis runs.
///
/// Scans once rather than applying regexes in sequence, so `**bold**` cannot be
/// re-matched as two italics and a marker inside inline code stays literal.
pub fn parse_inline(line: &str) -> Vec<Inline> {
    let mut spans = Vec::new();
    let mut plain = String::new();
    let mut pos = 0usize;
    let len = line.len();

    macro_rules! flush {
        () => {
            if !plain.is_empty() {
                spans.push(Inline::Text(std::mem::take(&mut plain)));
            }
        };
    }

    while pos < len {
        let remaining = &line[pos..];
        let Some(c) = remaining.chars().next() else {
            break;
        };
        let c_len = c.len_utf8();

        // Inline code first: nothing inside a backtick pair is markup.
        if c == '`'
            && let Some(rel) = line[pos + c_len..].find('`')
        {
            let end = pos + c_len + rel;
            flush!();
            spans.push(Inline::Code(line[pos + c_len..end].to_string()));
            pos = end + 1;
            continue;
        }
        if c == '*'
            && remaining.starts_with("**")
            && let Some(rel) = line[pos + 2..].find("**")
        {
            let end = pos + 2 + rel;
            flush!();
            spans.push(Inline::Bold(line[pos + 2..end].to_string()));
            pos = end + 2;
            continue;
        }
        if c == '*'
            && let Some(rel) = line[pos + c_len..].find('*')
        {
            let end = pos + c_len + rel;
            flush!();
            spans.push(Inline::Italic(line[pos + c_len..end].to_string()));
            pos = end + 1;
            continue;
        }
        if c == '['
            && let Some(rel_close) = line[pos + c_len..].find(']')
        {
            let close = pos + c_len + rel_close;
            if close + 1 < len
                && line[close + 1..].starts_with('(')
                && let Some(rel_paren) = line[close + 2..].find(')')
            {
                let paren = close + 2 + rel_paren;
                flush!();
                spans.push(Inline::Link {
                    text: line[pos + c_len..close].to_string(),
                    url: line[close + 2..paren].to_string(),
                });
                pos = paren + 1;
                continue;
            }
        }
        plain.push(c);
        pos += c_len;
    }

    flush!();
    spans
}

/// Index of the next occurrence of `needle` at or after `from`.
///
/// Compares char by char rather than collecting `needle`, because this is
/// called from a loop over every character of every line the TUI renders.
///
/// Hot needles from `parse_inline` are single- or double-ASCII (`\``, `*`,
/// `**`, `]`, `)`). Those take a fixed-length path so we never re-walk the
/// needle as an iterator on every character of every line.
#[allow(dead_code)]
pub(crate) fn find(chars: &[char], from: usize, needle: &str) -> Option<usize> {
    if needle.is_empty() || chars.len() < from {
        return None;
    }

    // Fast path: pure-ASCII needles used by the inline scanner.
    if needle.is_ascii() {
        let n = needle.as_bytes();
        let len = n.len();
        if chars.len() < len {
            return None;
        }
        let last = chars.len() - len;
        if from > last {
            return None;
        }
        return match len {
            1 => {
                let c0 = n[0] as char;
                (from..=last).find(|&i| chars[i] == c0)
            }
            2 => {
                let c0 = n[0] as char;
                let c1 = n[1] as char;
                (from..=last).find(|&i| chars[i] == c0 && chars[i + 1] == c1)
            }
            _ => (from..=last).find(|&i| {
                n.iter()
                    .enumerate()
                    .all(|(k, &b)| chars[i + k] == b as char)
            }),
        };
    }

    let len = needle.chars().count();
    if len == 0 || chars.len() < len {
        return None;
    }
    let needle_chars: Vec<char> = needle.chars().collect();
    (from..=chars.len() - len).find(|&i| chars[i..i + len] == needle_chars[..])
}

/// Render a markdown string to ANSI-escaped terminal output.
///
/// Supported formatting:
/// - `**bold**` → ANSI bold
/// - `*italic*` → ANSI italic
/// - `` `code` `` → inverted colors
/// - ` ```language ... ``` ` → syntax-highlighted via syntect
/// - ` ```mermaid ... ``` ` → Unicode box-drawing diagram via `mermaid-text`
/// - `# headers` → bold + underline
/// - `- lists` → bullet points
/// - `[links](url)` → cyan underlined
/// - GFM pipe tables → aligned box-drawing table
pub fn render_markdown(text: &str) -> String {
    let mut output = String::with_capacity(text.len() * 2);
    let lines: Vec<&str> = text.lines().collect();
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_buffer = String::new();
    let mut i = 0usize;

    while i < lines.len() {
        let line = lines[i];
        // Handle fenced code blocks
        if line.trim_start().starts_with("```") {
            if in_code_block {
                // End of code block
                output.push_str(&render_fenced_block(&code_buffer, &code_lang));
                code_buffer.clear();
                code_lang.clear();
                in_code_block = false;
                i += 1;
                continue;
            } else {
                // Start of code block
                in_code_block = true;
                code_lang = line
                    .trim_start()
                    .strip_prefix("```")
                    .unwrap_or("")
                    .trim()
                    .to_string();
                i += 1;
                continue;
            }
        }

        if in_code_block {
            if !code_buffer.is_empty() {
                code_buffer.push('\n');
            }
            code_buffer.push_str(line);
            i += 1;
            continue;
        }

        // GFM pipe table (header + separator + body).
        if let Some((
            Block::Table {
                headers,
                aligns,
                rows,
            },
            consumed,
        )) = try_parse_table(&lines[i..])
        {
            let hdrs: Vec<&str> = headers.iter().map(|s| s.as_str()).collect();
            let table = crate::table::format_table_aligned(&hdrs, &rows, &aligns);
            output.push_str(&table);
            output.push('\n');
            i += consumed;
            continue;
        }

        // Headers: #, ##, ###, etc.
        if let Some(rest) = line.strip_prefix("# ") {
            output.push_str(&format!("\x1b[1m\x1b[4m{rest}\x1b[0m\n"));
        } else if let Some(rest) = line.strip_prefix("## ") {
            output.push_str(&format!("\x1b[1m\x1b[4m{rest}\x1b[0m\n"));
        } else if let Some(rest) = line.strip_prefix("### ") {
            output.push_str(&format!("\x1b[1m\x1b[4m{rest}\x1b[0m\n"));
        } else if let Some(rest) = line.strip_prefix("#### ") {
            output.push_str(&format!("\x1b[1m\x1b[4m{rest}\x1b[0m\n"));
        } else if let Some(rest) = line.strip_prefix("##### ") {
            output.push_str(&format!("\x1b[1m\x1b[4m{rest}\x1b[0m\n"));
        } else if let Some(rest) = line.strip_prefix("###### ") {
            output.push_str(&format!("\x1b[1m\x1b[4m{rest}\x1b[0m\n"));
        } else if line.trim_start().starts_with("- ") || line.trim_start().starts_with("* ") {
            // Unordered list items
            let trimmed = line.trim_start();
            let bullet = &trimmed[..1];
            let rest = &trimmed[2..];
            output.push_str(&format_inline(&format!("  {bullet} {rest}")));
            output.push('\n');
        } else {
            // Regular paragraph line — apply inline formatting
            output.push_str(&format_inline(line));
            output.push('\n');
        }
        i += 1;
    }

    // Close any unclosed code block
    if in_code_block && !code_buffer.is_empty() {
        output.push_str(&render_fenced_block(&code_buffer, &code_lang));
    }

    output
}

/// Dispatch a closed (or trailing open) fence to Mermaid or syntax highlight.
fn render_fenced_block(code: &str, language: &str) -> String {
    let lang = if language.is_empty() {
        None
    } else {
        Some(language)
    };
    if is_mermaid_language(lang) {
        match render_mermaid(code, None) {
            Ok(lines) => {
                let mut out = String::from("\x1b[2m┌ mermaid\x1b[0m\n");
                for line in lines.iter() {
                    out.push_str(&format!("\x1b[2m│\x1b[0m {line}\n"));
                }
                out.push_str("\x1b[2m└\x1b[0m\n");
                return out;
            }
            Err(err) => {
                // Fall through to source with a dim error header so the user
                // still sees the diagram text if layout fails.
                return format!(
                    "\x1b[2m┌ mermaid (render failed: {err})\x1b[0m\n{}\n",
                    highlight_code_block(code, language)
                );
            }
        }
    }
    highlight_code_block(code, language)
}

/// Apply inline markdown formatting to a single line.
fn format_inline(text: &str) -> String {
    let mut result = text.to_string();

    // Inline code: `text` (do first to avoid interference)
    if let Some(re) = re_code() {
        result = re.replace_all(&result, "\x1b[7m$1\x1b[0m").to_string();
    }

    // Bold: **text**
    if let Some(re) = re_bold() {
        result = re.replace_all(&result, "\x1b[1m$1\x1b[0m").to_string();
    }

    // Italic: *text* — simple non-greedy match between single * chars.
    // After bold replacement, remaining single * pairs are italics.
    if let Some(re) = re_italic() {
        result = re.replace_all(&result, "\x1b[3m$1\x1b[0m").to_string();
    }

    // Links: [text](url)
    if let Some(re) = re_link() {
        result = re
            .replace_all(&result, "\x1b[36m\x1b[4m$1\x1b[0m (\x1b[36m$2\x1b[0m)")
            .to_string();
    }

    result
}

/// Syntax-highlight a code block using the shared highlighter.
fn highlight_code_block(code: &str, language: &str) -> String {
    if language.is_empty() {
        return format!("```\n{code}\n```\n");
    }
    let highlighted = highlight_code(code, language);
    // `highlight_code` returns the input unchanged when the language is unknown.
    if highlighted == code {
        format!("```\n{code}\n```\n")
    } else {
        highlighted
    }
}

#[cfg(test)]
#[path = "markdown_tests.rs"]
mod tests;
