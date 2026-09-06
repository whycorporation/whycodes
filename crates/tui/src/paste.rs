//! Collapsed paste blocks for the prompt.
//!
//! Large pastes are stored separately and shown in the input as a short
//! token like `[pasted #1 ~ 42 lines]`. On submit the tokens expand back
//! to the full text so the agent still receives everything.
//!
//! Thresholds: multi-line (2+) **or** longer than a short sentence so the
//! prompt box never reflows into a tall wall of wrapped text on paste.

use std::sync::atomic::{AtomicU32, Ordering};

/// Collapse when the paste has at least this many logical lines.
pub const COLLAPSE_MIN_LINES: usize = 2;
/// Collapse when the paste has at least this many characters (even 1 line).
///
/// Kept well below a full prompt wrap so a long paragraph becomes a chip
/// instead of overflowing the boxed input.
pub const COLLAPSE_MIN_CHARS: usize = 160;

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
/// Placeholder format: `[pasted #N ~ L lines]` (singular `line` when L=1).
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
#[path = "paste_tests.rs"]
mod tests;
