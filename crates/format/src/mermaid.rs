//! Mermaid fenced blocks → terminal-friendly lines.
//!
//! With the `mermaid` feature: Unicode box-drawing via [`mermaid_text`].
//! Without it (default ship binary): return the source lines so Boot/TTFF does
//! not pay ~0.8 MB of layout code for a rarely-hit fence type.

use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

use rustc_hash::FxHashMap;

/// Fence language tags that mean Mermaid source.
pub fn is_mermaid_language(language: Option<&str>) -> bool {
    language
        .map(|s| s.trim().eq_ignore_ascii_case("mermaid") || s.trim().eq_ignore_ascii_case("mmd"))
        .unwrap_or(false)
}

/// Render Mermaid source to display lines.
///
/// `max_width` is an optional column budget (terminal cells). When set and the
/// `mermaid` feature is on, the renderer tries to compact the diagram to fit.
///
/// On parse/layout failure returns `Err` with a short human-readable reason so
/// callers can fall back to the raw fence body.
/// Returns shared lines so a TUI frame that re-renders a closed diagram only
/// clones an [`Arc`], not every string.
pub fn render_mermaid(source: &str, max_width: Option<usize>) -> Result<Arc<Vec<String>>, String> {
    let key = cache_key(source, max_width);
    if let Some(hit) = cache().lock().ok().and_then(|c| c.get(&key).cloned()) {
        return hit;
    }

    let result = render_uncached(source, max_width).map(Arc::new);
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

type MermaidCache = Mutex<FxHashMap<u64, Result<Arc<Vec<String>>, String>>>;

fn cache() -> &'static MermaidCache {
    static CACHE: OnceLock<MermaidCache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn cache_key(source: &str, max_width: Option<usize>) -> u64 {
    // Same rationale as highlight::cache_key — local memo, not adversarial keys.
    let mut hasher = rustc_hash::FxHasher::default();
    source.hash(&mut hasher);
    max_width.hash(&mut hasher);
    #[cfg(feature = "mermaid")]
    "mermaid-text-v1".hash(&mut hasher);
    #[cfg(not(feature = "mermaid"))]
    "mermaid-source-v1".hash(&mut hasher);
    hasher.finish()
}

fn render_uncached(source: &str, max_width: Option<usize>) -> Result<Vec<String>, String> {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        return Err("empty mermaid diagram".into());
    }

    #[cfg(feature = "mermaid")]
    {
        let text =
            mermaid_text::render_with_width(trimmed, max_width).map_err(|e| e.to_string())?;

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

    #[cfg(not(feature = "mermaid"))]
    {
        let _ = max_width;
        // Ship binary: keep the source so the fence is still readable without
        // linking mermaid-text + ascii_dag (~0.8 MB).
        Ok(trimmed.lines().map(str::to_string).collect())
    }
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
        #[cfg(feature = "mermaid")]
        assert!(!joined.contains("graph LR"), "{joined}");
        #[cfg(not(feature = "mermaid"))]
        assert!(joined.contains("graph LR"), "{joined}");
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
    #[cfg(feature = "mermaid")]
    fn unsupported_or_garbage_falls_to_err() {
        // Completely blank after comments, or unknown type — either way Err.
        let result = render_mermaid("this is not mermaid at all", None);
        assert!(result.is_err(), "{result:?}");
    }

    #[test]
    #[cfg(not(feature = "mermaid"))]
    fn source_fallback_keeps_garbage_readable() {
        let lines = render_mermaid("this is not mermaid at all", None).unwrap();
        assert_eq!(lines.join("\n"), "this is not mermaid at all");
    }

    #[test]
    fn closed_cache_returns_identical_lines() {
        let src = "graph TD; A --> B";
        let a = render_mermaid(src, Some(80)).unwrap();
        let b = render_mermaid(src, Some(80)).unwrap();
        assert_eq!(a, b);
    }
}
