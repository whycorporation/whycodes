/// Max body lines (each of remove/add) shown in a compact edit preview.
const PREVIEW_BODY_LINES: usize = 40;

/// Width of the right-aligned line-number gutter in edit/write previews.
const LINE_NO_WIDTH: usize = 4;

/// Format a right-aligned line number + marker + body (Grok-style).
///
/// ```text
///   12|-old line
///   12|+new line
/// ```
fn numbered_diff_line(line_no: usize, marker: char, body: &str) -> String {
    format!("{line_no:>width$}|{marker}{body}\n", width = LINE_NO_WIDTH)
}

/// Plain unified-style preview of a string replacement (no ANSI).
///
/// TUI paints line numbers + `+`/`-` with theme colours.
///
/// `start_line` is 1-based file line of the first removed (or added) line.
/// When `None`, lines still get sequential numbers starting at 1.
///
/// ```text
/// Edited path/to/file.rs
///
///   42|-old line
///   42|+new line
/// ```
pub fn format_edit_preview(path: &str, old: &str, new: &str, replace_count: usize) -> String {
    format_edit_preview_at(path, old, new, replace_count, None)
}

/// Like [`format_edit_preview`] with an optional 1-based start line in the file.
pub fn format_edit_preview_at(
    path: &str,
    old: &str,
    new: &str,
    replace_count: usize,
    start_line: Option<usize>,
) -> String {
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
    let base = start_line.unwrap_or(1).max(1);

    // Single-line swap: one-liners read better without a full LCS dump.
    if old_lines.len() <= 1 && new_lines.len() <= 1 {
        if old.is_empty() && !new.is_empty() {
            for (i, line) in new_lines.iter().take(PREVIEW_BODY_LINES).enumerate() {
                out.push_str(&numbered_diff_line(base + i, '+', line));
            }
        } else if new.is_empty() && !old.is_empty() {
            for (i, line) in old_lines.iter().take(PREVIEW_BODY_LINES).enumerate() {
                out.push_str(&numbered_diff_line(base + i, '-', line));
            }
        } else {
            out.push_str(&numbered_diff_line(
                base,
                '-',
                old_lines.first().copied().unwrap_or(""),
            ));
            out.push_str(&numbered_diff_line(
                base,
                '+',
                new_lines.first().copied().unwrap_or(""),
            ));
        }
        return out;
    }

    // Multi-line: emit removals then additions (compact, Grok-like).
    let old_trunc = old_lines.len() > PREVIEW_BODY_LINES;
    let new_trunc = new_lines.len() > PREVIEW_BODY_LINES;
    for (i, line) in old_lines.iter().take(PREVIEW_BODY_LINES).enumerate() {
        out.push_str(&numbered_diff_line(base + i, '-', line));
    }
    if old_trunc {
        out.push_str(&format!(
            "… {} more removed lines\n",
            old_lines.len() - PREVIEW_BODY_LINES
        ));
    }
    // Additions re-start at the same base line (replacement block).
    for (i, line) in new_lines.iter().take(PREVIEW_BODY_LINES).enumerate() {
        out.push_str(&numbered_diff_line(base + i, '+', line));
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
///    1|+first line
///    2|+second line
/// ```
pub fn format_write_preview(path: &str, content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    let mut out = String::new();
    out.push_str(&format!("Wrote {path}  ·  {total} lines\n\n"));
    let trunc = total > PREVIEW_BODY_LINES;
    for (i, line) in lines.iter().take(PREVIEW_BODY_LINES).enumerate() {
        out.push_str(&numbered_diff_line(i + 1, '+', line));
    }
    if trunc {
        out.push_str(&format!("… {} more lines\n", total - PREVIEW_BODY_LINES));
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
    if path.is_empty() { None } else { Some(path) }
}

/// Parsed pieces of a numbered or bare diff line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLineParts<'a> {
    /// Right-aligned line number digits (no spaces), if present.
    pub line_no: Option<&'a str>,
    /// Leading spaces before the line number (padding).
    pub line_no_pad: &'a str,
    /// `+` / `-` when this is an add/remove row.
    pub marker: Option<char>,
    /// Rest of the line after the marker (or full line if no marker).
    pub body: &'a str,
}

/// Split `  12|-body` or bare `+body` / `-body` into paint-friendly parts.
pub fn parse_diff_line(line: &str) -> DiffLineParts<'_> {
    if line.starts_with("+++") || line.starts_with("---") || line.starts_with("@@") {
        return DiffLineParts {
            line_no: None,
            line_no_pad: "",
            marker: None,
            body: line,
        };
    }

    // Numbered: optional spaces, digits, optional `|`, then +/- marker.
    // Accepts both `  12|-body` (preferred) and bare `  12-body`.
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() && bytes[i] == b' ' {
        i += 1;
    }
    let pad_end = i;
    let digit_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i > digit_start {
        let mut j = i;
        if j < bytes.len() && bytes[j] == b'|' {
            j += 1;
        }
        if j < bytes.len()
            && (bytes[j] == b'+' || bytes[j] == b'-')
            && !(j + 2 < bytes.len()
                && bytes[j] == b'+'
                && bytes[j + 1] == b'+'
                && bytes[j + 2] == b'+')
            && !(j + 2 < bytes.len()
                && bytes[j] == b'-'
                && bytes[j + 1] == b'-'
                && bytes[j + 2] == b'-')
        {
            let marker = bytes[j] as char;
            let body = &line[j + 1..];
            return DiffLineParts {
                line_no: Some(&line[digit_start..i]),
                line_no_pad: &line[..pad_end],
                marker: Some(marker),
                body,
            };
        }
    }

    // Bare +/-
    let mut chars = line.chars();
    match chars.next() {
        Some(c @ ('+' | '-')) => DiffLineParts {
            line_no: None,
            line_no_pad: "",
            marker: Some(c),
            body: chars.as_str(),
        },
        _ => DiffLineParts {
            line_no: None,
            line_no_pad: "",
            marker: None,
            body: line,
        },
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
        let parts = parse_diff_line(line);
        match parts.marker {
            Some('+') => plus += 1,
            Some('-') => minus += 1,
            _ => {}
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

/// 1-based line number of the first occurrence of `needle` in `haystack`.
pub fn first_line_number(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(1);
    }
    let pos = haystack.find(needle)?;
    Some(haystack[..pos].bytes().filter(|&b| b == b'\n').count() + 1)
}

#[cfg(test)]
#[path = "diff_tests.rs"]
mod tests;
