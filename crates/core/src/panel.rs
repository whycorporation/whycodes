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
    fn panel_update_file_and_diff_variants() {
        let file = PanelUpdate::File {
            path: "a.rs".into(),
            text: "fn main() {}".into(),
        };
        let diff = PanelUpdate::Diff {
            path: "a.rs".into(),
            unified: "--- a\n+++ b".into(),
        };
        assert_ne!(file, diff);
        assert_eq!(PanelUpdate::Clear, PanelUpdate::Clear);
        let mermaid = PanelUpdate::Mermaid {
            source: "graph TD; A-->B".into(),
        };
        assert_ne!(file, mermaid);
    }
}
