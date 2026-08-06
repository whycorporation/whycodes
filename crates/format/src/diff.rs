/// Max body lines (each of remove/add) shown in a compact edit preview.
const PREVIEW_BODY_LINES: usize = 40;

/// Plain unified-style preview of a string replacement (no ANSI).
///
/// TUI paints `+`/`-`/`@@` with theme colours; this only shapes the text so
/// both CLI and TUI can share the same shape.
///
/// ```text
/// Edited path/to/file.rs
///
/// - old line
/// + new line
/// ```
pub fn format_edit_preview(path: &str, old: &str, new: &str, replace_count: usize) -> String {
    let mut out = String::new();
    if replace_count > 1 {
        out.push_str(&format!(
            "Edited {path}  ·  {replace_count} replacements\n\n"
        ));
    } else {
        out.push_str(&format!("Edited {path}\n\n"));
    }

    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    // Single-line swap: one-liners read better without a full LCS dump.
    if old_lines.len() <= 1 && new_lines.len() <= 1 {
        if old.is_empty() && !new.is_empty() {
            for line in new_lines.iter().take(PREVIEW_BODY_LINES) {
                out.push_str(&format!("+{line}\n"));
            }
        } else if new.is_empty() && !old.is_empty() {
            for line in old_lines.iter().take(PREVIEW_BODY_LINES) {
                out.push_str(&format!("-{line}\n"));
            }
        } else {
            out.push_str(&format!("-{}\n", old_lines.first().copied().unwrap_or("")));
            out.push_str(&format!("+{}\n", new_lines.first().copied().unwrap_or("")));
        }
        return out;
    }

    // Multi-line: emit removals then additions (compact, Grok-like).
    let old_trunc = old_lines.len() > PREVIEW_BODY_LINES;
    let new_trunc = new_lines.len() > PREVIEW_BODY_LINES;
    for line in old_lines.iter().take(PREVIEW_BODY_LINES) {
        out.push_str(&format!("-{line}\n"));
    }
    if old_trunc {
        out.push_str(&format!(
            "… {} more removed lines\n",
            old_lines.len() - PREVIEW_BODY_LINES
        ));
    }
    for line in new_lines.iter().take(PREVIEW_BODY_LINES) {
        out.push_str(&format!("+{line}\n"));
    }
    if new_trunc {
        out.push_str(&format!(
            "… {} more added lines\n",
            new_lines.len() - PREVIEW_BODY_LINES
        ));
    }
    out
}

/// Compact write preview (all additions). TUI colours `+` lines green.
///
/// ```text
/// Wrote path/to/file.rs  ·  12 lines
///
/// +first line
/// +second line
/// ```
pub fn format_write_preview(path: &str, content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    let mut out = String::new();
    out.push_str(&format!("Wrote {path}  ·  {total} lines\n\n"));
    let trunc = total > PREVIEW_BODY_LINES;
    for line in lines.iter().take(PREVIEW_BODY_LINES) {
        out.push_str(&format!("+{line}\n"));
    }
    if trunc {
        out.push_str(&format!(
            "… {} more lines\n",
            total - PREVIEW_BODY_LINES
        ));
    }
    // Empty file: still show the header so the tool result is not blank.
    if total == 0 {
        out.push_str("(empty file)\n");
    }
    out
}

/// Path from an edit/write preview header, if present.
///
/// `"Edited src/a.rs"` / `"Wrote src/a.rs  ·  3 lines"` → `Some("src/a.rs")`.
pub fn preview_file_path(text: &str) -> Option<&str> {
    let first = text.lines().next()?.trim();
    let rest = first
        .strip_prefix("Edited ")
        .or_else(|| first.strip_prefix("Wrote "))?;
    // Drop trailing " ·  N replacements/lines".
    let path = rest.split("  ·  ").next().unwrap_or(rest).trim();
    if path.is_empty() {
        None
    } else {
        Some(path)
    }
}

/// True when `text` looks like a unified / edit preview diff the TUI should
/// colour as add/remove rather than plain dim text.
pub fn looks_like_diff(text: &str) -> bool {
    let mut plus = 0usize;
    let mut minus = 0usize;
    let mut edit_header = false;
    for line in text.lines().take(60) {
        if line.starts_with("diff --git")
            || line.starts_with("@@ ")
            || line.starts_with("+++ ")
            || line.starts_with("--- ")
        {
            return true;
        }
        if line.starts_with("Edited ") || line.starts_with("Wrote ") {
            edit_header = true;
            continue;
        }
        // Edit preview: bare `+` / `-` prefixes (not list bullets like `- item`
        // without a following token that looks like code — still allow both).
        if line.starts_with('+') && !line.starts_with("+++") {
            plus += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            minus += 1;
        }
    }
    // Explicit tool previews may be one-sided (pure write / pure delete).
    if edit_header && (plus > 0 || minus > 0) {
        return true;
    }
    // Need both sides so a plain markdown list doesn't light up green/red.
    plus > 0 && minus > 0
}

/// Render a simple line-by-line diff with ANSI colors:
/// - `+` lines in green
/// - `-` lines in red
/// - Context lines normal
pub fn render_diff(old: &str, new: &str) -> String {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    // Simple LCS-based diff approach (line-by-line comparison)
    let lcs = lcs(&old_lines, &new_lines);
    let mut output = String::new();
    let mut old_idx = 0;
    let mut new_idx = 0;

    for (old_pos, new_pos) in lcs {
        // Lines in old but not in new (removals) — show in red
        while old_idx < old_pos {
            output.push_str(&format!("\x1b[31m- {}\x1b[0m\n", old_lines[old_idx]));
            old_idx += 1;
        }
        // Lines in new but not in old (additions) — show in green
        while new_idx < new_pos {
            output.push_str(&format!("\x1b[32m+ {}\x1b[0m\n", new_lines[new_idx]));
            new_idx += 1;
        }
        // Common line
        output.push_str(&format!("  {}\n", new_lines[new_pos]));
        old_idx += 1;
        new_idx += 1;
    }

    // Remaining old lines (removals)
    while old_idx < old_lines.len() {
        output.push_str(&format!("\x1b[31m- {}\x1b[0m\n", old_lines[old_idx]));
        old_idx += 1;
    }

    // Remaining new lines (additions)
    while new_idx < new_lines.len() {
        output.push_str(&format!("\x1b[32m+ {}\x1b[0m\n", new_lines[new_idx]));
        new_idx += 1;
    }

    output
}

/// Colorize an existing unified diff output.
/// Lines starting with `+` become green, `-` become red, `@@` become cyan (hunk headers).
pub fn render_diff_unified(diff_output: &str) -> String {
    diff_output
        .lines()
        .map(|line| {
            if line.starts_with("+++") || line.starts_with("---") {
                format!("\x1b[1m{}\x1b[0m", line)
            } else if line.starts_with("@@") {
                format!("\x1b[36m{}\x1b[0m", line)
            } else if line.starts_with('+') {
                format!("\x1b[32m{}\x1b[0m", line)
            } else if line.starts_with('-') {
                format!("\x1b[31m{}\x1b[0m", line)
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Compute the longest common subsequence of two slices.
/// Returns a vector of (index_in_a, index_in_b) pairs.
fn lcs<T: PartialEq>(a: &[T], b: &[T]) -> Vec<(usize, usize)> {
    let m = a.len();
    let n = b.len();
    let mut dp = vec![vec![0usize; n + 1]; m + 1];

    for i in 1..=m {
        for j in 1..=n {
            if a[i - 1] == b[j - 1] {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }

    // Backtrack to find the LCS
    let mut result = Vec::new();
    let mut i = m;
    let mut j = n;
    while i > 0 && j > 0 {
        if a[i - 1] == b[j - 1] {
            result.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if dp[i - 1][j] > dp[i][j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    result.reverse();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_diff() {
        let old = "line1\nline2\nline3\n";
        let new = "line1\nline2_modified\nline3\nline4\n";
        let result = render_diff(old, new);
        assert!(result.contains("\x1b[31m- line2\x1b[0m"));
        assert!(result.contains("\x1b[32m+ line2_modified\x1b[0m"));
        assert!(result.contains("\x1b[32m+ line4\x1b[0m"));
    }

    #[test]
    fn test_diff_unified() {
        let diff = "@@ -1,3 +1,4 @@\n context\n-old\n+new\n more";
        let result = render_diff_unified(diff);
        assert!(result.contains("\x1b[36m@@ -1,3 +1,4 @@\x1b[0m"));
        assert!(result.contains("\x1b[31m-old\x1b[0m"));
        assert!(result.contains("\x1b[32m+new\x1b[0m"));
    }

    #[test]
    fn edit_preview_shapes_single_line_swap() {
        let p = format_edit_preview("src/a.rs", "old", "new", 1);
        assert!(p.contains("Edited src/a.rs"));
        assert!(p.contains("-old"));
        assert!(p.contains("+new"));
        assert!(looks_like_diff(&p));
        assert_eq!(preview_file_path(&p), Some("src/a.rs"));
    }

    #[test]
    fn write_preview_is_one_sided_diff() {
        let p = format_write_preview("src/a.rs", "fn main() {}\n");
        assert!(p.contains("Wrote src/a.rs"));
        assert!(p.contains("+fn main() {}"));
        assert!(looks_like_diff(&p));
        assert_eq!(preview_file_path(&p), Some("src/a.rs"));
    }

    #[test]
    fn looks_like_diff_rejects_plain_lists() {
        assert!(!looks_like_diff("- only removals\n- more"));
        assert!(!looks_like_diff("hello\nworld"));
        assert!(looks_like_diff("diff --git a/x b/x\n"));
    }
}
