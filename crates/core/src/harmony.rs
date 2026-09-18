//! GPT-5 Harmony protocol leak detection and replay escaping.
//!
//! Harmony-dialect models wrap tool calls in routing envelopes:
//! `<|start|>assistant<|channel|>commentary to=functions.NAME<|message|>…<|call|>`
//!
//! Constrained decoding typically masks those control-token IDs inside tool
//! arguments, so probability mass shifts onto un-bracketed shadows
//! (`analysis to=functions.edit code …`). JSON parsers accept the junk and
//! write tools (`edit` / `apply_patch`) would commit it. Detection requires
//! the `to=functions.` marker **and** a co-signal; a bare mention is not
//! enough. Fenced / quoted examples are ignored.

use serde_json::Value;

/// `to=functions.` — the routing marker Harmony models emit as a shadow.
pub const MARKER: &str = "to=functions.";

const CHANNEL_WORDS: [&str; 4] = ["analysis", "commentary", "assistant", "user"];

/// High-precision glitch-token surface strings (not a closed model-id list).
const GLITCH_SURFACES: [&str; 6] = [
    "SolidGoldMagikarp",
    "davidjl",
    "TheNitromeFan",
    "Smartling",
    "rawdownloadcloneembedreportprint",
    "\u{e000}",
];

const FAKE_RESULT: [&str; 4] = ["code_output", "Cell 0:", "Cell 1:", "Cell 2:"];

/// Reserved Harmony special-token spellings to neutralize on replay.
const SPECIAL_TOKENS: [&str; 4] = ["<|start|>", "<|channel|>", "<|message|>", "<|call|>"];

/// Escape reserved `<|…|>` spellings so untrusted tool results cannot inject
/// routing headers on the next Harmony-dialect request.
pub fn escape_replay(text: &str) -> String {
    let mut out = text.to_string();
    for tok in SPECIAL_TOKENS {
        if out.contains(tok) {
            let inner = &tok[2..tok.len() - 2];
            out = out.replace(tok, &format!("< {inner} >"));
        }
    }
    out
}

/// Walk JSON string leaves for a leak.
pub fn scan_json(value: &Value) -> Option<Hit> {
    match value {
        Value::String(s) => scan_text(s),
        Value::Array(items) => items.iter().find_map(scan_json),
        Value::Object(map) => map.values().find_map(scan_json),
        _ => None,
    }
}

/// Scan a finalized visible / thinking / argument string.
pub fn scan_text(text: &str) -> Option<Hit> {
    scan_text_with_boundary(text, None)
}

/// `trusted_end` is a byte offset where a structurally valid parse ended
/// (completed JSON value / last valid patch hunk). When omitted, that
/// co-signal stays inert so legitimate diffs that discuss the protocol
/// do not false-abort.
pub fn scan_text_with_boundary(text: &str, trusted_end: Option<usize>) -> Option<Hit> {
    let mut search_from = 0;
    let mut marker_count = 0usize;
    while let Some(rel) = text[search_from..].find(MARKER) {
        let at = search_from + rel;
        search_from = at + MARKER.len();
        if in_fence_or_quote(text, at) {
            continue;
        }
        marker_count += 1;
        if let Some(hit) = classify_marker(text, at, marker_count, trusted_end) {
            return Some(hit);
        }
    }
    None
}

/// A confirmed Harmony-shadow leak.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub at: usize,
    pub co_signal: CoSignal,
}

impl Hit {
    pub fn summary(&self) -> String {
        format!(
            "Harmony protocol leak at byte {} ({})",
            self.at,
            self.co_signal.as_str()
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoSignal {
    ChannelAdjacency,
    GlitchToken,
    NonLatinJunk,
    Cascade,
    FakeResultFraming,
    TrustedBoundary,
}

impl CoSignal {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ChannelAdjacency => "channel-adjacency",
            Self::GlitchToken => "glitch-token",
            Self::NonLatinJunk => "non-latin-junk",
            Self::Cascade => "cascade",
            Self::FakeResultFraming => "fake-result-framing",
            Self::TrustedBoundary => "trusted-boundary",
        }
    }
}

fn classify_marker(
    text: &str,
    at: usize,
    marker_count: usize,
    trusted_end: Option<usize>,
) -> Option<Hit> {
    if marker_count >= 2 {
        return Some(Hit {
            at,
            co_signal: CoSignal::Cascade,
        });
    }
    if channel_adjacent(text, at) {
        return Some(Hit {
            at,
            co_signal: CoSignal::ChannelAdjacency,
        });
    }
    if glitch_near(text, at) {
        return Some(Hit {
            at,
            co_signal: CoSignal::GlitchToken,
        });
    }
    if fake_result_after(text, at) {
        return Some(Hit {
            at,
            co_signal: CoSignal::FakeResultFraming,
        });
    }
    if non_latin_after(text, at) {
        return Some(Hit {
            at,
            co_signal: CoSignal::NonLatinJunk,
        });
    }
    if let Some(end) = trusted_end
        && at >= end
    {
        return Some(Hit {
            at,
            co_signal: CoSignal::TrustedBoundary,
        });
    }
    None
}

fn channel_adjacent(text: &str, at: usize) -> bool {
    let prefix = text[..at].trim_end();
    CHANNEL_WORDS.iter().any(|w| {
        let Some(rest) = prefix.strip_suffix(w) else {
            return false;
        };
        rest.is_empty()
            || rest
                .chars()
                .next_back()
                .is_some_and(|c| !c.is_ascii_alphanumeric() && c != '_')
    })
}

fn glitch_near(text: &str, at: usize) -> bool {
    let start = at.saturating_sub(80);
    let window = &text[start..at];
    GLITCH_SURFACES.iter().any(|g| window.contains(g))
}

fn fake_result_after(text: &str, at: usize) -> bool {
    let end = (at + 240).min(text.len());
    let window = &text[at..end];
    FAKE_RESULT.iter().any(|f| window.contains(f))
}

fn non_latin_after(text: &str, at: usize) -> bool {
    let mut run = 0usize;
    for ch in text[at..].chars().take(160) {
        if is_non_latin_junk(ch) {
            run += 1;
            if run >= 8 {
                return true;
            }
        } else if !ch.is_ascii_whitespace() {
            run = 0;
        }
    }
    false
}

fn is_non_latin_junk(ch: char) -> bool {
    let u = ch as u32;
    // CJK, Thai, Georgian, Cyrillic — the public co-signals. ASCII / Latin
    // extended stay out so a French comment next to a marker is not a hit.
    (0x0400..=0x04FF).contains(&u)
        || (0x0E00..=0x0E7F).contains(&u)
        || (0x10A0..=0x10FF).contains(&u)
        || (0x2E80..=0x9FFF).contains(&u)
        || (0xAC00..=0xD7AF).contains(&u)
        || (0xF900..=0xFAFF).contains(&u)
}

/// True when `at` sits inside a markdown fence or a paired quote/backtick.
fn in_fence_or_quote(text: &str, at: usize) -> bool {
    let before = &text[..at];
    fenced(before) || quoted(before)
}

fn fenced(before: &str) -> bool {
    before.matches("```").count() % 2 == 1
}

fn quoted(before: &str) -> bool {
    odd_unescaped(before, '"') || odd_unescaped(before, '`')
}

fn odd_unescaped(s: &str, quote: char) -> bool {
    let mut n = 0usize;
    let mut escaped = false;
    for ch in s.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == quote {
            n += 1;
        }
    }
    n % 2 == 1
}

/// Byte offset of the last `@@` hunk header in a unified diff.
pub fn last_hunk_start(patch: &str) -> Option<usize> {
    patch
        .rmatch_indices("\n@@")
        .next()
        .map(|(i, _)| i + 1)
        .or_else(|| patch.starts_with("@@").then_some(0))
}

/// Drop the contaminated line and everything after it.
pub fn truncate_at_line(text: &str, at: usize) -> String {
    let line_start = text[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
    text[..line_start].to_string()
}

/// True when a leak at `at` sits on its own line after the last `@@` hunk
/// (optional truncate-and-apply). Mid-hunk leaks are not recoverable.
pub fn leak_is_after_last_hunk(patch: &str, at: usize) -> bool {
    let Some(hunk) = last_hunk_start(patch) else {
        return false;
    };
    if at < hunk {
        return false;
    }
    let line_start = patch[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
    !matches!(patch[line_start..at].chars().next(), Some('+' | '-' | ' '))
}

#[cfg(test)]
#[path = "harmony_tests.rs"]
mod tests;
