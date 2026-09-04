pub mod error;
pub mod history;
pub mod import;
pub mod session;
pub mod title;

pub use error::{Result, SessionError};
pub use history::SessionHistory;
pub use import::{ImportKind, import_messages};
pub use session::{
    COMPACT_CONTINUATION_PREAMBLE, CheckpointState, CompactOutcome, Session,
    compact_summary_display_text, format_compact_summary, format_compact_summary_content,
    is_compact_summary_text,
};
pub use title::{TitleSource, default_title, heuristic_title, sanitize_title};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn public_reexports_are_callable() {
        assert_eq!(sanitize_title("  Title.  "), "Title");
        assert_eq!(heuristic_title("please fix login"), "fix login");
        assert!(default_title(Path::new("/tmp/app"), "ab").contains("app"));
        assert!(is_compact_summary_text("[Compacted earlier]"));
        assert_eq!(format_compact_summary("ok"), "ok");
        assert!(!COMPACT_CONTINUATION_PREAMBLE.is_empty());
        assert!(TitleSource::Default.allows_heuristic());
        assert_eq!(ImportKind::parse("auto"), ImportKind::Auto);
        assert!(!SessionHistory::new().can_undo());
    }
}
