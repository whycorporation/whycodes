//! Agent-writable side-panel updates.
//!
//! Tools push a [`PanelUpdate`] through [`PanelSink`]. The TUI maps that onto
//! the sidebar Preview tab. No I/O here — just the payload.

use std::sync::Arc;

/// What the sidebar Preview tab should show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelUpdate {
    /// Pin a file's contents.
    File { path: String, text: String },
    /// Pin a unified diff.
    Diff { path: String, unified: String },
    /// Pin mermaid source (TUI renders when the mermaid feature is on).
    Mermaid { source: String },
    /// Clear the preview.
    Clear,
}

/// Callback the host (agent loop) installs so `panel` can reach the TUI.
pub type PanelSink = Arc<dyn Fn(PanelUpdate) + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_update_variants_debug_eq() {
        let a = PanelUpdate::File {
            path: "a.rs".into(),
            text: "x".into(),
        };
        let b = PanelUpdate::Diff {
            path: "a.rs".into(),
            unified: "d".into(),
        };
        let c = PanelUpdate::Mermaid {
            source: "graph TD".into(),
        };
        let d = PanelUpdate::Clear;
        assert_ne!(a, b);
        assert_eq!(d, PanelUpdate::Clear);
        let _ = format!("{a:?}{b:?}{c:?}{d:?}");
        let sink: PanelSink = Arc::new(|_| {});
        sink(PanelUpdate::Clear);
    }
}
