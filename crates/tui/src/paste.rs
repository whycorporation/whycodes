//! Collapsed paste blocks for the prompt (OpenCode / Claude Code style).
//!
//! Large pastes are stored separately and shown in the input as a short
//! token like `[pasted #1 ~ 42 lines]`. On submit the tokens expand back
//! to the full text so the agent still receives everything.
//!
//! Thresholds follow Claude Code defaults (more than 2 lines **or** more
//! than 800 characters) so small multi-line snippets stay editable inline.

use std::sync::atomic::{AtomicU32, Ordering};

/// Collapse when the paste has more than this many lines.
pub const COLLAPSE_MIN_LINES: usize = 3;
/// Collapse when the paste has at least this many characters (even 1 line).
pub const COLLAPSE_MIN_CHARS: usize = 800;

static NEXT_PASTE_ID: AtomicU32 = AtomicU32::new(1);

/// Full text of one collapsed paste, keyed by a stable id.
#[derive(Debug, Clone)]
pub struct PastedBlock {
    pub id: u32,
    pub content: String,
}

impl PastedBlock {
    pub fn line_count(&self) -> usize {
        line_count(&self.content)
    }
}

/// Allocate a new paste id (monotonic for the process).
pub fn next_id() -> u32 {
    NEXT_PASTE_ID.fetch_add(1, Ordering::Relaxed)
}

/// True when a paste should be collapsed into a placeholder token.
pub fn should_collapse(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    let lines = line_count(text);
    let chars = text.chars().count();
    lines >= COLLAPSE_MIN_LINES || chars >= COLLAPSE_MIN_CHARS
}

/// Build the visible placeholder inserted into `input_buffer`.
///
/// Format matches OpenCode: `[pasted #N ~ L lines]` (singular `line` when L=1).
pub fn placeholder(id: u32, line_count: usize) -> String {
    let n = line_count.max(1);
    let unit = if n == 1 { "line" } else { "lines" };
    format!("[pasted #{id} ~ {n} {unit}]")
}

/// Number of logical lines (at least 1 for non-empty text).
pub fn line_count(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    // `lines()` drops a trailing empty line after a final `\n`; count newlines
    // so "a\nb\n" is 3 rows (cursor can sit on the blank).
    let newlines = text.bytes().filter(|&b| b == b'\n').count();
    newlines + 1
}

/// One occurrence of a paste placeholder in a buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaceholderSpan {
    /// Byte start (inclusive).
    pub start: usize,
    /// Byte end (exclusive).
    pub end: usize,
    pub id: u32,
}

/// Find the placeholder that contains `byte_pos` (start..end exclusive), if any.
/// A cursor sitting exactly at `end` is **not** inside (so typing after works).
pub fn placeholder_at(buf: &str, byte_pos: usize) -> Option<PlaceholderSpan> {
    find_placeholders(buf)
        .into_iter()
        .find(|p| byte_pos >= p.start && byte_pos < p.end)
}

/// Placeholder immediately before `byte_pos` (cursor sitting on its end boundary).
pub fn placeholder_ending_at(buf: &str, byte_pos: usize) -> Option<PlaceholderSpan> {
    find_placeholders(buf)
        .into_iter()
        .find(|p| p.end == byte_pos)
}

/// Placeholder immediately at or after `byte_pos` (for Delete key).
pub fn placeholder_starting_at(buf: &str, byte_pos: usize) -> Option<PlaceholderSpan> {
    find_placeholders(buf)
        .into_iter()
        .find(|p| p.start == byte_pos)
}

/// Scan `buf` for `[pasted #ID ~ N line(s)]` tokens.
pub fn find_placeholders(buf: &str) -> Vec<PlaceholderSpan> {
    let mut out = Vec::new();
    let bytes = buf.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        // Look for `[pasted #`
        if bytes[i] == b'['
            && let Some(rest) = buf.get(i..)
            && let Some(span) = parse_placeholder_at(rest)
        {
            out.push(PlaceholderSpan {
                start: i,
                end: i + span.len,
                id: span.id,
            });
            i += span.len;
            continue;
        }
        i += 1;
    }
    out
}

struct ParsedPlaceholder {
    id: u32,
    len: usize,
}

/// Parse a placeholder starting at the beginning of `s`. Returns `None` if not a match.
fn parse_placeholder_at(s: &str) -> Option<ParsedPlaceholder> {
    const PREFIX: &str = "[pasted #";
    if !s.starts_with(PREFIX) {
        return None;
    }
    let after_prefix = &s[PREFIX.len()..];
    let id_end = after_prefix.find(|c: char| !c.is_ascii_digit())?;
    if id_end == 0 {
        return None;
    }
    let id: u32 = after_prefix[..id_end].parse().ok()?;
    let after_id = &after_prefix[id_end..];
    // ` ~ N line(s)]`
    if !after_id.starts_with(" ~ ") {
        return None;
    }
    let after_tilde = &after_id[3..];
    let n_end = after_tilde.find(|c: char| !c.is_ascii_digit())?;
    if n_end == 0 {
        return None;
    }
    let after_n = &after_tilde[n_end..];
    let unit_len = if after_n.starts_with(" lines]") {
        " lines]".len()
    } else if after_n.starts_with(" line]") {
        " line]".len()
    } else {
        return None;
    };
    let len = PREFIX.len() + id_end + 3 + n_end + unit_len;
    Some(ParsedPlaceholder { id, len })
}

/// Replace every known placeholder with its full content.
/// Unknown ids (user edited the token) are left as-is.
pub fn expand(buf: &str, blocks: &[PastedBlock]) -> String {
    let spans = find_placeholders(buf);
    if spans.is_empty() {
        return buf.to_string();
    }
    let mut out = String::with_capacity(buf.len());
    let mut cursor = 0usize;
    for span in spans {
        out.push_str(&buf[cursor..span.start]);
        if let Some(block) = blocks.iter().find(|b| b.id == span.id) {
            out.push_str(&block.content);
        } else {
            // Keep the token if we lost the body (should be rare).
            out.push_str(&buf[span.start..span.end]);
        }
        cursor = span.end;
    }
    out.push_str(&buf[cursor..]);
    out
}

/// Drop paste blocks that no longer appear as placeholders in `buf`.
pub fn prune_unused(blocks: &mut Vec<PastedBlock>, buf: &str) {
    let live: std::collections::HashSet<u32> =
        find_placeholders(buf).into_iter().map(|p| p.id).collect();
    blocks.retain(|b| live.contains(&b.id));
}

/// True when `byte_range` overlaps a placeholder token (for styled rendering).
pub fn style_ranges(buf: &str) -> Vec<(usize, usize)> {
    find_placeholders(buf)
        .into_iter()
        .map(|p| (p.start, p.end))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_single_line_stays_inline() {
        assert!(!should_collapse("hello world"));
        assert!(!should_collapse("a\nb")); // 2 lines
    }

    #[test]
    fn three_lines_collapse() {
        assert!(should_collapse("a\nb\nc"));
    }

    #[test]
    fn long_single_line_collapses() {
        let s = "x".repeat(COLLAPSE_MIN_CHARS);
        assert!(should_collapse(&s));
        assert!(!should_collapse(&"x".repeat(COLLAPSE_MIN_CHARS - 1)));
    }

    #[test]
    fn placeholder_roundtrip_parse() {
        let p = placeholder(7, 42);
        assert_eq!(p, "[pasted #7 ~ 42 lines]");
        let spans = find_placeholders(&format!("fix {p} please"));
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].id, 7);
        assert_eq!(&format!("fix {p} please")[spans[0].start..spans[0].end], p);
    }

    #[test]
    fn singular_line_unit() {
        assert_eq!(placeholder(1, 1), "[pasted #1 ~ 1 line]");
        let spans = find_placeholders("[pasted #1 ~ 1 line]");
        assert_eq!(spans.len(), 1);
    }

    #[test]
    fn expand_replaces_known_blocks() {
        let blocks = vec![PastedBlock {
            id: 3,
            content: "one\ntwo\nthree".into(),
        }];
        let buf = format!("see {} end", placeholder(3, 3));
        assert_eq!(expand(&buf, &blocks), "see one\ntwo\nthree end");
    }

    #[test]
    fn expand_keeps_unknown_token() {
        let buf = placeholder(99, 5);
        assert_eq!(expand(&buf, &[]), buf);
    }

    #[test]
    fn prune_drops_orphans() {
        let mut blocks = vec![
            PastedBlock {
                id: 1,
                content: "a".into(),
            },
            PastedBlock {
                id: 2,
                content: "b".into(),
            },
        ];
        let buf = placeholder(2, 1);
        prune_unused(&mut blocks, &buf);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].id, 2);
    }

    #[test]
    fn placeholder_at_cursor_inside() {
        let p = placeholder(1, 10);
        let buf = format!("x{p}y");
        let start = 1;
        let end = 1 + p.len();
        assert!(placeholder_at(&buf, start).is_some());
        assert!(placeholder_at(&buf, start + 3).is_some());
        assert!(placeholder_at(&buf, end).is_none()); // on boundary → outside
        assert_eq!(placeholder_ending_at(&buf, end).map(|s| s.id), Some(1));
        assert_eq!(placeholder_starting_at(&buf, start).map(|s| s.id), Some(1));
    }

    #[test]
    fn line_count_counts_trailing_newline() {
        assert_eq!(line_count("a\nb"), 2);
        assert_eq!(line_count("a\nb\n"), 3);
        assert_eq!(line_count("solo"), 1);
    }
}
