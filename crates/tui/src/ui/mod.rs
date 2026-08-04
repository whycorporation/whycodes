// ── ui/mod.rs: UI module root ──────────────────────────────────────────
// Declares sub-modules and re-exports the render entry point.

pub mod chat;
pub mod dialogs;
pub mod header;
pub mod layout;
pub mod markdown;
pub mod progress_bar;
pub mod prompt;
pub mod render;
pub mod scrollbar;
pub mod sidebar;
pub mod slash_suggest;
pub mod status;
pub mod status_bar;
pub mod toast;

pub use render::render;
