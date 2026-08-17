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
    let mut c = crate::highlight::recover_lock(cache().lock());
    if c.len() >= CACHE_ENTRIES
        && let Some(old) = c.keys().next().copied()
    {
        c.remove(&old);
    }
    c.insert(key, result.clone());
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
#[path = "mermaid_tests.rs"]
mod tests;
