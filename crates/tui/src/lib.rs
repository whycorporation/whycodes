// ── lib.rs: TUI crate root ────────────────────────────────────────────
// Re-exports all public modules in the whycode-tui crate.

pub mod app;
pub mod config;
pub mod input;
pub mod keymap;
pub mod theme;
pub mod ui;
pub mod widgets;

pub use app::TuiApp;
