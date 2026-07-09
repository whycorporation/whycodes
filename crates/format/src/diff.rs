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
}
