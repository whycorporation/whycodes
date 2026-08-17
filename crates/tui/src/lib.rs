// ── lib.rs: TUI crate root ────────────────────────────────────────────
// Re-exports all public modules in the whycode-tui crate.

pub mod app;
pub mod bench;
pub mod cell_grid;
pub mod clipboard;
pub mod config;
pub mod frecency;
pub mod hit_area;
pub mod images;
pub mod input;
pub mod keymap;
pub mod md_stream;
pub mod paste;
pub mod remote;
pub mod run;
pub mod session_runtime;
pub mod theme;
pub mod toast;
pub mod ui;
pub mod widgets;

pub use app::TuiApp;
pub use remote::RemoteAttach;
pub use run::{RESUME_LATEST, TuiRunOptions, resolve_and_load_session, run, tui_available};
pub use theme::file as theme_file;
pub use theme::tokens;

#[cfg(test)]
mod tests;
