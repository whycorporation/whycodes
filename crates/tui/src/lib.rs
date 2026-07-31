// ── lib.rs: TUI crate root ────────────────────────────────────────────
// Re-exports all public modules in the whycode-tui crate.

pub mod app;
pub mod config;
pub mod input;
pub mod keymap;
pub mod opencode_tokens;
pub mod run;
pub mod theme;
pub mod theme_file;
pub mod ui;
pub mod widgets;

pub use app::TuiApp;
pub use run::{TuiRunOptions, run};

#[cfg(test)]
mod tests;
