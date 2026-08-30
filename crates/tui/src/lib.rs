// ── lib.rs: TUI crate root ────────────────────────────────────────────
// Re-exports all public modules in the whycodes-tui crate.
// `TuiApp` stays in `app` (not re-exported); callers use `run()`.

pub mod app;
pub mod bench;
pub mod cell_grid;
pub mod clipboard;
pub mod config;
pub mod frecency;
pub mod heap;
pub mod hit_area;
pub mod images;
pub mod input;
pub mod keymap;
pub mod md_stream;
pub mod paste;
pub mod redraw_schedule;
pub mod remote;
pub mod run;
pub mod session_runtime;
pub mod theme;
pub mod toast;
pub mod ui;
pub mod widgets;

pub use app::UpdateOffer;
pub use remote::RemoteAttach;
pub use run::{
    RESUME_LATEST, TuiExit, TuiRunOptions, resolve_and_load_session, run, tui_available,
};
pub use theme::file as theme_file;
pub use theme::tokens;

#[cfg(test)]
mod tests;
