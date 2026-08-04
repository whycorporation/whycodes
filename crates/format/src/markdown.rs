use regex::Regex;

use crate::highlight::highlight_code;

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
        } else if let Some(rest) = list_item(trimmed) {
            blocks.push(Block::ListItem {
                indent: line.len() - trimmed.len(),
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

fn list_item(trimmed: &str) -> Option<&str> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return Some(rest);
        }
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
fn find(chars: &[char], from: usize, needle: &str) -> Option<usize> {
    let len = needle.chars().count();
    if len == 0 || chars.len() < len {
        return None;
    }
    (from..=chars.len() - len).find(|&i| needle.chars().enumerate().all(|(k, c)| chars[i + k] == c))
}

/// Render a markdown string to ANSI-escaped terminal output.
///
/// Supported formatting:
/// - `**bold**` → ANSI bold
/// - `*italic*` → ANSI italic
/// - `` `code` `` → inverted colors
/// - ` ```language ... ``` ` → syntax-highlighted via syntect
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
                output.push_str(&highlight_code_block(&code_buffer, &code_lang));
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
        output.push_str(&highlight_code_block(&code_buffer, &code_lang));
    }

    output
}

/// Apply inline markdown formatting to a single line.
fn format_inline(text: &str) -> String {
    let mut result = text.to_string();

    // Inline code: `text` (do first to avoid interference)
    let code_re = Regex::new(r"`([^`]+)`").unwrap();
    result = code_re.replace_all(&result, "\x1b[7m$1\x1b[0m").to_string();

    // Bold: **text**
    let bold_re = Regex::new(r"\*\*(.+?)\*\*").unwrap();
    result = bold_re.replace_all(&result, "\x1b[1m$1\x1b[0m").to_string();

    // Italic: *text* — simple non-greedy match between single * chars.
    // After bold replacement, remaining single * pairs are italics.
    let italic_re = Regex::new(r"\*(.+?)\*").unwrap();
    result = italic_re
        .replace_all(&result, "\x1b[3m$1\x1b[0m")
        .to_string();

    // Links: [text](url)
    let link_re = Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").unwrap();
    result = link_re
        .replace_all(&result, "\x1b[36m\x1b[4m$1\x1b[0m (\x1b[36m$2\x1b[0m)")
        .to_string();

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
                spans: vec![Inline::Text("nested".into())]
            }
        );
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
