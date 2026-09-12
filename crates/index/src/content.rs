//! In-memory content trigrams for grep candidate pruning.
//!
//! Overlapping 3-byte trigrams, the same filter Microsoft tgrep uses, but
//! resident in the workspace index (no disk index, no TCP server). The grep
//! tool still confirms with `grep-searcher`; this layer only drops files that
//! cannot contain a required literal. Files not yet indexed are never dropped.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use rustc_hash::{FxHashMap, FxHashSet};

/// Bytes of an overlapping trigram.
pub const TRIGRAM_LEN: usize = 3;
/// Files larger than this are skipped (grep also refuses to search them).
pub const MAX_INDEX_BYTES: u64 = 1024 * 1024;
/// NUL sniff window; matches the file-tools binary check.
const BINARY_SNIFF: usize = 8192;

/// Regex metacharacters that mean the pattern is not a single literal.
const REGEX_META: &[char] = &[
    '.', '^', '$', '*', '+', '?', '(', ')', '[', ']', '{', '}', '|',
];

/// Per-root overlay: relative path → unique sorted trigrams.
#[derive(Debug, Default)]
pub struct ContentIndex {
    files: FxHashMap<Box<str>, Vec<u32>>,
    skipped: FxHashSet<Box<str>>,
    complete: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Load {
    Bytes(Vec<u8>),
    /// NUL / binary — grep will skip these too.
    Binary,
    /// Missing, symlink, directory, too large, or I/O error. Never prune.
    Unknown,
}

impl ContentIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.files.clear();
        self.skipped.clear();
        self.complete = false;
    }

    pub fn is_complete(&self) -> bool {
        self.complete
    }

    pub fn mark_complete(&mut self) {
        self.complete = true;
    }

    pub fn indexed_len(&self) -> usize {
        self.files.len()
    }

    pub fn skipped_len(&self) -> usize {
        self.skipped.len()
    }

    /// Re-read `rel` under `root` and replace its posting.
    pub fn upsert(&mut self, root: &Path, rel: &str, size: u64) {
        self.remove(rel);
        match load_file(&root.join(rel), size) {
            Load::Binary => {
                self.skipped.insert(rel.into());
            }
            Load::Bytes(bytes) => {
                self.files
                    .insert(rel.into(), extract_indexed_trigrams(&bytes));
            }
            Load::Unknown => {}
        }
    }

    pub fn remove(&mut self, rel: &str) {
        self.files.remove(rel);
        self.skipped.remove(rel);
    }

    /// Drop `rel` and every descendant (`rel/…`).
    pub fn remove_tree(&mut self, rel: &str) {
        self.remove(rel);
        let prefix = tree_prefix(rel);
        self.files.retain(|k, _| !k.starts_with(&prefix));
        self.skipped.retain(|k| !k.starts_with(&prefix));
    }

    /// `true` when `rel` might contain every `required` trigram.
    ///
    /// Unknown paths (not yet filled, read error, symlink, too large) return
    /// `true` so a partial overlay never hides a match. Binary paths return
    /// `false` — the grep engine would skip them too.
    pub fn may_match(&self, rel: &str, required: &[u32]) -> bool {
        if required.is_empty() {
            return true;
        }
        if self.skipped.contains(rel) {
            return false;
        }
        match self.files.get(rel) {
            None => true,
            Some(tris) => contains_all(tris, required),
        }
    }
}

/// Pack three bytes into a `u32` key.
pub fn pack_trigram(a: u8, b: u8, c: u8) -> u32 {
    (u32::from(a) << 16) | (u32::from(b) << 8) | u32::from(c)
}

/// Unique sorted overlapping 3-byte trigrams. Empty when `bytes` is shorter
/// than [`TRIGRAM_LEN`].
pub fn extract_trigrams(bytes: &[u8]) -> Vec<u32> {
    if bytes.len() < TRIGRAM_LEN {
        return Vec::new();
    }
    let mut v: Vec<u32> = bytes
        .windows(TRIGRAM_LEN)
        .map(|w| pack_trigram(w[0], w[1], w[2]))
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Required trigrams for a grep pattern, or `None` when the overlay must not
/// prune (regex, too short, or Unicode case-fold).
pub fn required_trigrams(pattern: &str, case_insensitive: bool) -> Option<Vec<u32>> {
    if skip_unicode_casefold(pattern, case_insensitive) {
        return None;
    }
    let literal = as_literal(pattern)?;
    let bytes = if case_insensitive {
        ascii_lower(literal.as_bytes())
    } else {
        literal.into_bytes()
    };
    if bytes.len() < TRIGRAM_LEN {
        return None;
    }
    Some(extract_trigrams(&bytes))
}

/// Original-case trigrams plus ASCII-lowercased ones so a case-insensitive
/// query can prune without hiding a case-sensitive match (false positives
/// are confirmed by the grep engine; false negatives are not).
fn extract_indexed_trigrams(bytes: &[u8]) -> Vec<u32> {
    let mut v = extract_trigrams(bytes);
    let lower = ascii_lower(bytes);
    if lower.as_slice() != bytes {
        v.extend(extract_trigrams(&lower));
        v.sort_unstable();
        v.dedup();
    }
    v
}

fn skip_unicode_casefold(pattern: &str, case_insensitive: bool) -> bool {
    case_insensitive && pattern.bytes().any(|b| !b.is_ascii())
}

fn ascii_lower(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().map(|b| b.to_ascii_lowercase()).collect()
}

fn tree_prefix(rel: &str) -> String {
    format!("{rel}/")
}

fn contains_all(hay: &[u32], needles: &[u32]) -> bool {
    needles.iter().all(|n| hay.binary_search(n).is_ok())
}

/// Unescaped regex → `None`. Escaped metas become their literal character.
fn as_literal(pattern: &str) -> Option<String> {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            let n = chars.next()?;
            if is_regex_meta(n) || n == '\\' {
                out.push(n);
            } else {
                return None;
            }
        } else if is_regex_meta(c) {
            return None;
        } else {
            out.push(c);
        }
    }
    Some(out)
}

fn is_regex_meta(c: char) -> bool {
    REGEX_META.contains(&c)
}

fn load_file(path: &Path, size_hint: u64) -> Load {
    if size_hint > MAX_INDEX_BYTES {
        return too_large();
    }
    classify_metadata(path)
}

fn classify_metadata(path: &Path) -> Load {
    let Ok(md) = std::fs::symlink_metadata(path) else {
        return missing_path();
    };
    if md.file_type().is_symlink() || md.is_dir() {
        return skip_non_file();
    }
    if md.len() > MAX_INDEX_BYTES {
        return too_large();
    }
    read_text_or_binary(path)
}

fn read_text_or_binary(path: &Path) -> Load {
    let Ok(mut f) = File::open(path) else {
        return open_failed();
    };
    let mut sniff = [0u8; BINARY_SNIFF];
    classify_sniff(f.read(&mut sniff), &sniff, &mut f)
}

fn classify_sniff(result: std::io::Result<usize>, sniff: &[u8], f: &mut File) -> Load {
    match result {
        Ok(n) => {
            let n = n.min(sniff.len());
            if sniff[..n].contains(&0) {
                return Load::Binary;
            }
            let mut buf = Vec::with_capacity(n);
            buf.extend_from_slice(&sniff[..n]);
            if n == BINARY_SNIFF {
                append_rest(f, &mut buf)
            } else {
                Load::Bytes(buf)
            }
        }
        Err(err) => {
            tracing::debug!(error = %err, "content index sniff read failed");
            read_failed()
        }
    }
}

fn append_rest(f: &mut File, buf: &mut Vec<u8>) -> Load {
    let cap = MAX_INDEX_BYTES.saturating_sub(BINARY_SNIFF as u64);
    let mut rest = Vec::new();
    classify_append(f.take(cap).read_to_end(&mut rest), buf, rest)
}

fn classify_append(result: std::io::Result<usize>, buf: &mut Vec<u8>, rest: Vec<u8>) -> Load {
    match result {
        Ok(_) => {
            buf.extend(rest);
            Load::Bytes(std::mem::take(buf))
        }
        Err(err) => {
            tracing::debug!(error = %err, "content index append read failed");
            append_failed()
        }
    }
}

fn missing_path() -> Load {
    Load::Unknown
}

fn skip_non_file() -> Load {
    Load::Unknown
}

fn open_failed() -> Load {
    Load::Unknown
}

fn read_failed() -> Load {
    Load::Unknown
}

fn append_failed() -> Load {
    Load::Unknown
}

fn too_large() -> Load {
    Load::Unknown
}

#[cfg(test)]
#[path = "content_tests.rs"]
mod tests;
