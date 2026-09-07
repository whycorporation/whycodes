//! Short content tags for `read` / `grep` lines, so `edit` can name a span
//! without reciting `old_string` byte-for-byte.
//!
//! A tag is a base36 prefix of an FxHash of the line text (no trailing newline).
//! Same content always hashes the same. Different content in one window is
//! lengthened until the prefixes diverge. Lookup at edit time uses the length
//! of the caller-supplied tag against the whole file and fails closed on
//! missing or ambiguous hits.

use rustc_hash::FxHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

const ALPH: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
pub const MIN_LEN: usize = 2;
pub const MAX_LEN: usize = 6;

pub fn hash_u64(line: &str) -> u64 {
    let mut h = FxHasher::default();
    line.hash(&mut h);
    h.finish()
}

pub fn encode(hash: u64, len: usize) -> String {
    let len = len.clamp(MIN_LEN, MAX_LEN);
    // Always emit MAX_LEN digits (MSB first) so a shorter tag is a prefix of a
    // longer one for the same line.
    let mut n = hash;
    let mut digits = [b'0'; MAX_LEN];
    for slot in digits.iter_mut().rev() {
        *slot = ALPH[(n % 36) as usize];
        n /= 36;
    }
    digits[..len].iter().map(|&b| b as char).collect()
}

pub fn tag_of(line: &str, len: usize) -> String {
    encode(hash_u64(line), len)
}

/// Tags for a visible window. Lengthen until different contents get different tags.
pub fn tags_for_window(lines: &[&str]) -> Vec<String> {
    if lines.is_empty() {
        return Vec::new();
    }
    for len in MIN_LEN..=MAX_LEN {
        let tags: Vec<String> = lines.iter().map(|l| tag_of(l, len)).collect();
        if content_tags_unique(lines, &tags) {
            return tags;
        }
    }
    lines.iter().map(|l| tag_of(l, MAX_LEN)).collect()
}

fn content_tags_unique(lines: &[&str], tags: &[String]) -> bool {
    let mut first: HashMap<&str, &str> = HashMap::new();
    for (line, tag) in lines.iter().zip(tags) {
        if let Some(prev) = first.insert(tag.as_str(), line)
            && prev != *line
        {
            return false;
        }
    }
    true
}

/// `     42 a3|contents`
pub fn format_read_line(n: usize, tag: &str, line: &str) -> String {
    format!("{n:6} {tag}|{line}")
}

/// `path:12 a3:contents` or `path:12 a3-contents` for context rows.
pub fn format_grep_line(path: &str, n: u64, tag: &str, line: &str, sep: char) -> String {
    format!("{path}:{n} {tag}{sep}{line}")
}

#[derive(Debug, Clone, Copy)]
pub struct LineOffsets {
    /// Byte offset of the first content byte.
    pub start: usize,
    /// Byte offset past the last content byte (before `\n` if any).
    pub content_end: usize,
    /// Byte offset past the line terminator (`\n` / `\r\n`) or `content_end`.
    pub line_end: usize,
}

/// Split `s` into logical lines with byte offsets. A trailing newline does not
/// add an extra empty line (matches `str::lines`).
pub fn line_offsets(s: &str) -> Vec<LineOffsets> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let start = i;
        while i < bytes.len() && bytes[i] != b'\n' && bytes[i] != b'\r' {
            i += 1;
        }
        let content_end = i;
        if i < bytes.len() && bytes[i] == b'\r' {
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b'\n' {
            i += 1;
        }
        out.push(LineOffsets {
            start,
            content_end,
            line_end: i,
        });
        if start == i {
            break;
        }
    }
    out
}

pub fn line_text(s: &str, off: LineOffsets) -> &str {
    &s[off.start..off.content_end]
}

#[derive(Debug)]
pub enum TagHit {
    Unique(usize),
    Missing,
    Ambiguous(Vec<usize>),
}

pub fn find_tag(s: &str, offs: &[LineOffsets], tag: &str) -> TagHit {
    let len = tag.len();
    if !(MIN_LEN..=MAX_LEN).contains(&len) || !tag.bytes().all(is_tag_char) {
        return TagHit::Missing;
    }
    let mut hits = Vec::new();
    for (i, off) in offs.iter().enumerate() {
        if tag_of(line_text(s, *off), len) == tag {
            hits.push(i);
        }
    }
    match hits.len() {
        0 => TagHit::Missing,
        1 => TagHit::Unique(hits[0]),
        _ => TagHit::Ambiguous(hits),
    }
}

fn is_tag_char(b: u8) -> bool {
    b.is_ascii_digit() || b.is_ascii_lowercase()
}

/// Nearby tagged lines for a failed lookup (`center` is a line index, or 0).
pub fn nearby_snippet(s: &str, offs: &[LineOffsets], center: usize, radius: usize) -> String {
    if offs.is_empty() {
        return "(empty file)".into();
    }
    let len = MIN_LEN;
    let from = center.saturating_sub(radius);
    let to = (center + radius + 1).min(offs.len());
    let mut out = String::new();
    for (i, off) in offs.iter().enumerate().take(to).skip(from) {
        let text = line_text(s, *off);
        let tag = tag_of(text, len);
        out.push_str(&format_read_line(i + 1, &tag, text));
        out.push('\n');
    }
    out
}

#[cfg(test)]
#[path = "line_tag_tests.rs"]
mod tests;
