/// Truncate text to fit within line and character limits.
/// Preserves complete lines and appends a truncation message.
///
/// `max_lines` and `max_chars` are both soft limits — the function
/// keeps complete lines and truncates at the first boundary that would
/// exceed either limit.
pub fn truncate(text: &str, max_lines: usize, max_chars: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut char_count: usize = 0;
    let mut kept_lines: usize = 0;
    let mut truncated = false;

    for (i, line) in lines.iter().enumerate() {
        // Check if adding this line would exceed max_lines
        if i >= max_lines {
            truncated = true;
            break;
        }

        // Check if adding this line would exceed max_chars
        // +1 for newline
        let line_len = line.len() + 1;
        if char_count + line_len > max_chars {
            if i == 0 {
                // First line itself is too long — truncate entirely
                kept_lines = 0;
                truncated = true;
                break;
            }
            truncated = true;
            break;
        }

        char_count += line_len;
        kept_lines = i + 1;
    }

    if !truncated {
        return text.to_string();
    }

    let total_lines = lines.len();
    let skipped = total_lines - kept_lines;

    let mut result = lines[..kept_lines].join("\n");
    result.push('\n');
    result.push_str(&format!(
        "[... {} line{} truncated]",
        skipped,
        if skipped == 1 { "" } else { "s" }
    ));

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_truncation() {
        let text = "line1\nline2\nline3";
        let result = truncate(text, 10, 1000);
        assert_eq!(result, text);
    }

    #[test]
    fn test_line_truncation() {
        let text = "a\nb\nc\nd\ne";
        let result = truncate(text, 3, 1000);
        assert!(result.starts_with("a\nb\nc\n"));
        assert!(result.contains("[... 2 lines truncated]"));
    }

    #[test]
    fn test_char_truncation() {
        let text = "aaaaa\nbbbbb\nccccc";
        let result = truncate(text, 100, 12);
        // 12 chars: "aaaaa\n" = 6, "bbbbb\n" = 6 → total 12, fits exactly
        // "ccccc\n" = 6 → would be 18, exceeds
        assert!(result.starts_with("aaaaa\nbbbbb\n"));
        assert!(result.contains("truncated"));
    }

    #[test]
    fn test_single_line_text() {
        let text = "just one line no newlines";
        let result = truncate(text, 100, 5);
        // Single line, 0 lines kept, truncated
        assert!(result.contains("truncated"));
    }
}
