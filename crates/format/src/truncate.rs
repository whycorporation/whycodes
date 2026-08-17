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
#[path = "truncate_tests.rs"]
mod tests;
