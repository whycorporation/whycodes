/// Shorten a path by collapsing middle components into `...`.
///
/// If the path is longer than `max_len`, middle directory components
/// are replaced with `...` while preserving the start and end.
///
/// # Examples
///
/// ```
/// use whycodes_format::paths::truncate_path;
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
        // Path is already longer than max_len (checked above).
        let third = max_len.saturating_sub(3) / 2;
        let start = path.floor_char_boundary(third);
        let end = path.ceil_char_boundary(path.len().saturating_sub(third));
        return format!("{}...{}", &path[..start], &path[end..]);
    }

    let start_count = 2; // keep first 2 components
    let end_count = 2; // keep last 2 components

    let _start = components[..start_count].join("/");
    let _end = components[components.len() - end_count..].join("/");

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
#[path = "paths_tests.rs"]
mod tests;
