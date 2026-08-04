//! Mermaid fenced blocks → Unicode box-drawing for the terminal.
//!
//! Uses [`mermaid_text`] so diagrams render without a browser, image protocol,
//! or external CLI. Output is plain multi-line text (box-drawing glyphs), which
//! both the ANSI CLI path and the ratatui TUI can display as-is.
//!
//! Hot path: the TUI calls this from the render loop for every visible message,
//! so closed diagrams are memoised by `(source, max_width)`.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};

/// Fence language tags that mean Mermaid source.
pub fn is_mermaid_language(language: Option<&str>) -> bool {
    language
        .map(|s| s.trim().eq_ignore_ascii_case("mermaid") || s.trim().eq_ignore_ascii_case("mmd"))
        .unwrap_or(false)
}

/// Render Mermaid source to Unicode box-drawing lines.
///
/// `max_width` is an optional column budget (terminal cells). When set, the
/// renderer tries to compact the diagram to fit.
///
/// On parse/layout failure returns `Err` with a short human-readable reason so
/// callers can fall back to the raw fence body.
pub fn render_mermaid(source: &str, max_width: Option<usize>) -> Result<Vec<String>, String> {
    let key = cache_key(source, max_width);
    if let Some(hit) = cache()
        .lock()
        .ok()
        .and_then(|c| c.get(&key).cloned())
    {
        return hit;
    }

    let result = render_uncached(source, max_width);
    if let Ok(mut c) = cache().lock() {
        if c.len() >= CACHE_ENTRIES {
            c.clear();
        }
        c.insert(key, result.clone());
    }
    result
}

/// Most diagram renders held at once (same budget idea as the highlight memo).
const CACHE_ENTRIES: usize = 32;

type MermaidCache = Mutex<HashMap<u64, Result<Vec<String>, String>>>;

fn cache() -> &'static MermaidCache {
    static CACHE: OnceLock<MermaidCache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn cache_key(source: &str, max_width: Option<usize>) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    max_width.hash(&mut hasher);
    "mermaid-text-v1".hash(&mut hasher);
    hasher.finish()
}

fn render_uncached(source: &str, max_width: Option<usize>) -> Result<Vec<String>, String> {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        return Err("empty mermaid diagram".into());
    }

    let text = mermaid_text::render_with_width(trimmed, max_width).map_err(|e| e.to_string())?;

    // Drop a single trailing empty line the renderer sometimes leaves.
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    if lines.is_empty() {
        return Err("mermaid rendered empty".into());
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_mermaid_fence_tags() {
        assert!(is_mermaid_language(Some("mermaid")));
        assert!(is_mermaid_language(Some("Mermaid")));
        assert!(is_mermaid_language(Some("mmd")));
        assert!(!is_mermaid_language(Some("rust")));
        assert!(!is_mermaid_language(None));
    }

    #[test]
    fn renders_a_simple_flowchart() {
        let lines = render_mermaid("graph LR; A[Build] --> B[Deploy]", None).unwrap();
        let joined = lines.join("\n");
        assert!(joined.contains("Build"), "{joined}");
        assert!(joined.contains("Deploy"), "{joined}");
        // Box-drawing, not raw mermaid source.
        assert!(!joined.contains("graph LR"), "{joined}");
    }

    #[test]
    fn renders_sequence_diagrams() {
        let src = "sequenceDiagram\n    Alice->>Bob: Hello\n    Bob-->>Alice: Hi";
        let lines = render_mermaid(src, None).unwrap();
        let joined = lines.join("\n");
        assert!(joined.contains("Alice"), "{joined}");
        assert!(joined.contains("Bob"), "{joined}");
    }

    #[test]
    fn empty_source_is_an_error() {
        assert!(render_mermaid("   \n  ", None).is_err());
    }

    #[test]
    fn unsupported_or_garbage_falls_to_err() {
        // Completely blank after comments, or unknown type — either way Err.
        let result = render_mermaid("this is not mermaid at all", None);
        assert!(result.is_err(), "{result:?}");
    }

    #[test]
    fn closed_cache_returns_identical_lines() {
        let src = "graph TD; A --> B";
        let a = render_mermaid(src, Some(80)).unwrap();
        let b = render_mermaid(src, Some(80)).unwrap();
        assert_eq!(a, b);
    }
}
