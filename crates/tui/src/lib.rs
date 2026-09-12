// ── lib.rs: TUI crate root ────────────────────────────────────────────
// Re-exports all public modules in the whycodes-tui crate.
// `TuiApp` stays in `app` (not re-exported); callers use `run()`.

pub mod app;
pub mod bench;
pub mod cell_grid;
pub mod clipboard;
pub mod clipboard_image;
pub mod color;
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
    LoopInject, RESUME_LATEST, TuiExit, TuiRunOptions, paint_first_frame_sync,
    resolve_and_load_session, run, run_sync, tui_available,
};
pub use theme::file as theme_file;
pub use theme::tokens;

/// Serializes tests that mutate `WHYCODES_HOME` (session DB path is process-global).
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Holds `ENV_LOCK` and restores `WHYCODES_HOME` on drop (including panic).
#[cfg(test)]
pub(crate) struct IsolatedHome {
    _lock: std::sync::MutexGuard<'static, ()>,
    dir: tempfile::TempDir,
    prev: Option<std::ffi::OsString>,
}

#[cfg(test)]
impl IsolatedHome {
    pub(crate) fn new() -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let prev = std::env::var_os("WHYCODES_HOME");
        unsafe { std::env::set_var("WHYCODES_HOME", dir.path()) };
        Self {
            _lock: lock,
            dir,
            prev,
        }
    }

    pub(crate) fn path(&self) -> &std::path::Path {
        self.dir.path()
    }

    /// Re-pin after another crate in `cargo test --workspace` may have
    /// overwritten the process env (TUI `ENV_LOCK` is not process-wide).
    pub(crate) fn pin(&self) {
        unsafe { std::env::set_var("WHYCODES_HOME", self.dir.path()) };
    }
}

#[cfg(test)]
impl Drop for IsolatedHome {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => unsafe { std::env::set_var("WHYCODES_HOME", v) },
            None => unsafe { std::env::remove_var("WHYCODES_HOME") },
        }
    }
}

#[cfg(test)]
mod tests;
