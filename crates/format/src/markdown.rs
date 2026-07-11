use regex::Regex;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::{as_24_bit_terminal_escaped, LinesWithEndings};

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
                code_lang = line.trim_start().strip_prefix("```").unwrap_or("").trim().to_string();
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
    result = code_re
        .replace_all(&result, "\x1b[7m$1\x1b[0m")
        .to_string();

    // Bold: **text**
    let bold_re = Regex::new(r"\*\*(.+?)\*\*").unwrap();
    result = bold_re
        .replace_all(&result, "\x1b[1m$1\x1b[0m")
        .to_string();

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

/// Syntax-highlight a code block using syntect.
fn highlight_code_block(code: &str, language: &str) -> String {
    let ps = SyntaxSet::load_defaults_newlines();
    let ts = ThemeSet::load_defaults();

    let syntax = if language.is_empty() {
        None
    } else {
        ps.find_syntax_by_token(language)
            .or_else(|| ps.find_syntax_by_extension(language))
    };

    let syntax = match syntax {
        Some(s) => s,
        None => return format!("```\n{code}\n```\n"),
    };

    let theme = &ts.themes["base16-ocean.dark"];
    let mut highlighter = HighlightLines::new(syntax, theme);

    let mut output = String::with_capacity(code.len() * 2);
    for line in LinesWithEndings::from(code) {
        let ranges = highlighter.highlight_line(line, &ps).unwrap_or_default();
        let escaped = as_24_bit_terminal_escaped(&ranges[..], false);
        output.push_str(&escaped);
    }

    output
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
}
