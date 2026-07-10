/// Shorten a path by collapsing middle components into `...`.
///
/// If the path is longer than `max_len`, middle directory components
/// are replaced with `...` while preserving the start and end.
///
/// # Examples
///
/// ```
/// use whycode_format::paths::truncate_path;
/// assert_eq!(
///     truncate_path("/very/long/path/with/many/components", 25),
///     "/very/.../many/components"
/// );
/// ```
pub fn truncate_path(path: &str, max_len: usize) -> String {
    if path.len() <= max_len {
        return path.to_string();
    }

    // Split into components
    let components: Vec<&str> = path.split('/').collect();
    if components.len() <= 3 {
        // Not enough components to truncate meaningfully
        if path.len() <= max_len {
            return path.to_string();
        }
        // Just cut from the middle
        let third = max_len.saturating_sub(3) / 2;
        return format!("{}...{}", &path[..third], &path[path.len() - third..]);
    }

    let start_count = 2; // keep first 2 components
    let end_count = 2; // keep last 2 components

    let start = components[..start_count].join("/");
    let end = components[components.len() - end_count..].join("/");

    // Try adding one component at a time from each side until we hit max_len
    let mut left = start_count;
    let mut right = components.len() - end_count;

    loop {
        let current = format!(
            "{}/.../{}",
            components[..left].join("/"),
            components[right..].join("/")
        );
        if current.len() <= max_len {
            return current;
        }

        // Expand from the left if possible
        if left + 1 < right {
            left += 1;
            continue;
        }
        // Expand from the right if possible
        if right > left {
            right -= 1;
            continue;
        }
        // Can't expand further — just use the minimal form
        return current;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_short_path_unchanged() {
        let result = truncate_path("/home/user", 80);
        assert_eq!(result, "/home/user");
    }

    #[test]
    fn test_long_path_truncated() {
        let result = truncate_path("/very/long/path/with/many/components/here", 30);
        // Should contain ... in the middle
        assert!(result.contains("..."));
        assert!(result.starts_with("/very"));
        assert!(result.ends_with("here"));
    }

    #[test]
    fn test_very_short_max_len() {
        let result = truncate_path("/a/b/c/d/e/f/g/h", 12);
        assert!(result.len() <= 12 || result.contains("..."));
    }
}
