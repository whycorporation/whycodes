//! Machine-readable line counts at the end of a file-mutating tool result.
//!
//! `write`, `edit` and `apply_patch` end a successful result with a line of
//! the form `[+12 -3]`; the session sums these into its edit statistics.
//! The counts are what the tool replaced (an `edit` counts the whole
//! replaced block), not a minimal diff, so consumers label them approximate.

/// `[+added -removed]`, placed on its own line at the end of a result.
pub fn edit_trailer(added: usize, removed: usize) -> String {
    format!("[+{added} -{removed}]")
}

/// Sum every `[+N -M]` line in `content` (a multi-file patch emits one per
/// file). `None` when there is no trailer at all.
pub fn parse_edit_trailers(content: &str) -> Option<(u64, u64)> {
    let mut found = false;
    let mut added = 0u64;
    let mut removed = 0u64;
    for line in content.lines() {
        let Some((plus, minus)) = parse_one(line.trim_end()) else {
            continue;
        };
        found = true;
        added += plus;
        removed += minus;
    }
    found.then_some((added, removed))
}

fn parse_one(line: &str) -> Option<(u64, u64)> {
    let inner = line.strip_prefix("[+")?.strip_suffix(']')?;
    let (plus, minus) = inner.split_once(" -")?;
    Some((plus.parse().ok()?, minus.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailer_round_trips_and_sums() {
        assert_eq!(edit_trailer(12, 3), "[+12 -3]");
        assert_eq!(parse_edit_trailers("Wrote a.rs\n\n[+12 -3]"), Some((12, 3)));
        assert_eq!(
            parse_edit_trailers(
                "Patch applied to `a` (1 hunk).\n[+1 -1]\nPatch applied to `b` (2 hunks).\n[+4 -0]\n"
            ),
            Some((5, 1))
        );
        assert_eq!(parse_edit_trailers("Edited a.rs"), None);
        assert_eq!(parse_edit_trailers("[+x -1]"), None);
        assert_eq!(parse_edit_trailers("[+1 -]"), None);
        assert_eq!(parse_edit_trailers("[+1]"), None);
        assert_eq!(parse_edit_trailers("+1 -2]"), None);
    }
}
