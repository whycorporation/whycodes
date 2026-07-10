// ── ui/mod.rs: UI module root ──────────────────────────────────────────
// Declares sub-modules and re-exports the render entry point.

pub mod chat;
pub mod dialogs;
pub mod layout;
pub mod prompt;
pub mod render;
pub mod sidebar;
pub mod status;

pub use render::render;
