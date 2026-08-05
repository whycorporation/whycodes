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
pub fn parse_markdown(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut fence: Option<(Option<String>, Vec<String>)> = None;

    for line in text.lines() {
        let trimmed = line.trim_start();

        if trimmed.starts_with("```") {
            match fence.take() {
                Some((language, lines)) => blocks.push(Block::Code {
                    language,
                    lines,
                    closed: true,
                }),
                None => {
                    let lang = trimmed.trim_start_matches('`').trim();
                    fence = Some(((!lang.is_empty()).then(|| lang.to_string()), Vec::new()));
                }
            }
            continue;
        }

        if let Some((_, lines)) = fence.as_mut() {
            lines.push(line.to_string());
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
    }

    if let Some((language, lines)) = fence {
        blocks.push(Block::Code {
            language,
            lines,
            closed: false,
        });
    }

    blocks
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
    let chars: Vec<char> = line.chars().collect();
    let mut spans = Vec::new();
    let mut plain = String::new();
    let mut i = 0;

    macro_rules! flush {
        () => {
            if !plain.is_empty() {
                spans.push(Inline::Text(std::mem::take(&mut plain)));
            }
        };
    }

    while i < chars.len() {
        // Inline code first: nothing inside a backtick pair is markup.
        if chars[i] == '`'
            && let Some(end) = find(&chars, i + 1, "`")
        {
            flush!();
            spans.push(Inline::Code(chars[i + 1..end].iter().collect()));
            i = end + 1;
            continue;
        }
        if chars[i] == '*'
            && i + 1 < chars.len()
            && chars[i + 1] == '*'
            && let Some(end) = find(&chars, i + 2, "**")
        {
            flush!();
            spans.push(Inline::Bold(chars[i + 2..end].iter().collect()));
            i = end + 2;
            continue;
        }
        if chars[i] == '*'
            && let Some(end) = find(&chars, i + 1, "*")
        {
            flush!();
            spans.push(Inline::Italic(chars[i + 1..end].iter().collect()));
            i = end + 1;
            continue;
        }
        if chars[i] == '['
            && let Some(close) = find(&chars, i + 1, "]")
            && chars.get(close + 1) == Some(&'(')
            && let Some(paren) = find(&chars, close + 2, ")")
        {
            flush!();
            spans.push(Inline::Link {
                text: chars[i + 1..close].iter().collect(),
                url: chars[close + 2..paren].iter().collect(),
            });
            i = paren + 1;
            continue;
        }
        plain.push(chars[i]);
        i += 1;
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
fn find(chars: &[char], from: usize, needle: &str) -> Option<usize> {
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
pub fn render_markdown(text: &str) -> String {
    let mut output = String::with_capacity(text.len() * 2);
    let lines: Vec<&str> = text.lines().collect();
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_buffer = String::new();

    for line in &lines {
        // Handle fenced code blocks
        if line.trim_start().starts_with("```") {
            if in_code_block {
                // End of code block
                output.push_str(&render_fenced_block(&code_buffer, &code_lang));
                code_buffer.clear();
                code_lang.clear();
                in_code_block = false;
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
                continue;
            }
        }

        if in_code_block {
            if !code_buffer.is_empty() {
                code_buffer.push('\n');
            }
            code_buffer.push_str(line);
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
mod tests {
    use super::*;

    #[test]
    fn test_bold() {
        let result = render_markdown("hello **world**");
        assert!(result.contains("\x1b[1mworld\x1b[0m"));
    }

    #[test]
    fn test_italic() {
        let result = render_markdown("hello *world*");
        assert!(result.contains("\x1b[3mworld\x1b[0m"));
    }

    #[test]
    fn test_inline_code() {
        let result = render_markdown("use `foo()` here");
        assert!(result.contains("\x1b[7mfoo()\x1b[0m"));
    }

    #[test]
    fn test_header() {
        let result = render_markdown("# Title");
        assert!(result.contains("\x1b[1m\x1b[4mTitle\x1b[0m"));
    }

    #[test]
    fn test_link() {
        let result = render_markdown("see [docs](https://example.com)");
        assert!(result.contains("\x1b[36m\x1b[4mdocs\x1b[0m"));
    }

    #[test]
    fn test_code_block() {
        let result = render_markdown("```rust\nlet x = 1;\n```");
        // Should contain syntax-highlighted content, not raw backticks
        assert!(!result.contains("```"));
    }

    #[test]
    fn mermaid_fence_is_labelled() {
        let result = render_markdown("```mermaid\ngraph LR; A[Build] --> B[Deploy]\n```");
        assert!(result.contains("Build"), "{result}");
        assert!(result.contains("Deploy"), "{result}");
        assert!(result.contains("mermaid"), "{result}");
        #[cfg(feature = "mermaid")]
        // Full renderer: box-drawing, not raw mermaid source.
        assert!(!result.contains("graph LR"), "{result}");
        #[cfg(not(feature = "mermaid"))]
        // Slim binary: source kept so the diagram is still readable.
        assert!(result.contains("graph LR"), "{result}");
    }

    // ── Structured parsing ──────────────────────────────────────────────

    #[test]
    fn parses_headings_by_level() {
        let blocks = parse_markdown("# One\n### Three");
        assert_eq!(
            blocks[0],
            Block::Heading {
                level: 1,
                spans: vec![Inline::Text("One".into())]
            }
        );
        assert!(matches!(blocks[1], Block::Heading { level: 3, .. }));
    }

    #[test]
    fn a_hash_without_a_space_is_not_a_heading() {
        assert!(matches!(parse_markdown("#hashtag")[0], Block::Paragraph(_)));
        assert!(matches!(
            parse_markdown("####### seven")[0],
            Block::Paragraph(_)
        ));
    }

    #[test]
    fn parses_list_items_with_any_marker() {
        for input in ["- a", "* a", "+ a"] {
            assert!(
                matches!(parse_markdown(input)[0], Block::ListItem { .. }),
                "{input}"
            );
        }
    }

    #[test]
    fn records_list_indentation() {
        let blocks = parse_markdown("    - nested");
        assert_eq!(
            blocks[0],
            Block::ListItem {
                indent: 4,
                number: None,
                spans: vec![Inline::Text("nested".into())]
            }
        );
    }

    #[test]
    fn parses_ordered_list_items() {
        let blocks = parse_markdown("1. first\n2. second");
        assert_eq!(
            blocks[0],
            Block::ListItem {
                indent: 0,
                number: Some(1),
                spans: vec![Inline::Text("first".into())]
            }
        );
        assert!(matches!(
            blocks[1],
            Block::ListItem {
                number: Some(2),
                ..
            }
        ));
    }

    #[test]
    fn parses_fenced_code_with_language() {
        let blocks = parse_markdown("```rust\nlet x = 1;\n```");
        assert_eq!(
            blocks[0],
            Block::Code {
                language: Some("rust".into()),
                lines: vec!["let x = 1;".into()],
                closed: true
            }
        );
    }

    #[test]
    fn an_unterminated_fence_still_parses() {
        // This is the streaming case: the closing fence has not arrived yet.
        let blocks = parse_markdown("```rust\nlet x = 1;");
        assert_eq!(
            blocks[0],
            Block::Code {
                language: Some("rust".into()),
                lines: vec!["let x = 1;".into()],
                closed: false
            }
        );
    }

    #[test]
    fn markup_inside_a_fence_stays_literal() {
        let blocks = parse_markdown("```\n**not bold**\n# not a heading\n```");
        match &blocks[0] {
            Block::Code { lines, .. } => {
                assert_eq!(lines, &["**not bold**", "# not a heading"]);
            }
            other => panic!("expected code, got {other:?}"),
        }
    }

    #[test]
    fn parses_inline_emphasis() {
        assert_eq!(
            parse_inline("a **b** c"),
            vec![
                Inline::Text("a ".into()),
                Inline::Bold("b".into()),
                Inline::Text(" c".into())
            ]
        );
        assert_eq!(parse_inline("*i*"), vec![Inline::Italic("i".into())]);
        assert_eq!(parse_inline("`c`"), vec![Inline::Code("c".into())]);
    }

    #[test]
    fn bold_is_not_re_read_as_two_italics() {
        assert_eq!(parse_inline("**b**"), vec![Inline::Bold("b".into())]);
    }

    #[test]
    fn markup_inside_inline_code_stays_literal() {
        assert_eq!(
            parse_inline("`**not bold**`"),
            vec![Inline::Code("**not bold**".into())]
        );
    }

    #[test]
    fn parses_links() {
        assert_eq!(
            parse_inline("[docs](https://example.com)"),
            vec![Inline::Link {
                text: "docs".into(),
                url: "https://example.com".into()
            }]
        );
    }

    #[test]
    fn an_unclosed_marker_stays_literal() {
        assert_eq!(parse_inline("a * b"), vec![Inline::Text("a * b".into())]);
        assert_eq!(
            parse_inline("`unclosed"),
            vec![Inline::Text("`unclosed".into())]
        );
    }

    #[test]
    fn blank_lines_are_preserved_as_blocks() {
        let blocks = parse_markdown("a\n\nb");
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[1], Block::Blank);
    }

    #[test]
    fn empty_input_parses_to_nothing() {
        assert!(parse_markdown("").is_empty());
    }

    // Code-span highlighting tests live in `highlight` (shared module).
}
