// ── lib.rs: TUI crate root ────────────────────────────────────────────
// Re-exports all public modules in the whycode-tui crate.

pub mod app;
pub mod bench;
pub mod clipboard;
pub mod config;
pub mod frecency;
pub mod hit;
pub mod images;
pub mod input;
pub mod keymap;
pub mod paste;
pub mod run;
pub mod session_runtime;
pub mod theme;
pub mod theme_file;
pub mod toast;
pub mod tokens;
pub mod ui;
pub mod widgets;

pub use app::TuiApp;
pub use run::{RESUME_LATEST, TuiRunOptions, resolve_and_load_session, run, tui_available};

#[cfg(test)]
mod tests;
