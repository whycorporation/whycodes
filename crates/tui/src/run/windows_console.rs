//! Windows console UTF-8 + VT so box-drawing is lines, not OEM mojibake.
//!
//! WhyCodes draws on `CONOUT$`, a **new** handle. Crossterm enables VT on
//! stdout; console mode is per-handle, so `CONOUT$` can stay on the OEM
//! code page (CP857 on Turkish Windows). UTF-8 `─│╭╮` then shows as `Ööö`.
//!
//! Non-Windows builds compile the same API as no-ops so Linux CI covers it.

use super::TuiWriter;

/// UTF-8 (Windows code page 65001).
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(super) const CP_UTF8: u32 = 65001;
/// `ENABLE_PROCESSED_OUTPUT`.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(super) const ENABLE_PROCESSED_OUTPUT: u32 = 0x0001;
/// `ENABLE_VIRTUAL_TERMINAL_PROCESSING`.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(super) const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

/// OR the VT + processed-output bits onto an existing console mode.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(super) const fn vt_processing_mode(old_mode: u32) -> u32 {
    old_mode | ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING
}

/// Set CP 65001 and VT on a live console writer. No-op for Buf/Fail and Unix.
pub(super) fn prepare_windows_console(out: &TuiWriter) {
    #[cfg(windows)]
    imp::prepare(out);
    #[cfg(not(windows))]
    {
        let _ = out;
    }
}

/// Restore the code page saved by [`prepare_windows_console`]. No-op otherwise.
pub(super) fn restore_windows_console() {
    #[cfg(windows)]
    imp::restore();
}

#[cfg(windows)]
mod imp {
    use std::sync::Mutex;

    use super::{CP_UTF8, TuiWriter, vt_processing_mode};

    const STD_OUTPUT_HANDLE: i32 = -11;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetConsoleOutputCP() -> u32;
        fn SetConsoleOutputCP(w_code_page_id: u32) -> i32;
        fn GetConsoleMode(handle: *mut core::ffi::c_void, mode: *mut u32) -> i32;
        fn SetConsoleMode(handle: *mut core::ffi::c_void, mode: u32) -> i32;
        fn GetStdHandle(n_std_handle: i32) -> *mut core::ffi::c_void;
    }

    static SAVED_OUTPUT_CP: Mutex<Option<u32>> = Mutex::new(None);

    fn saved_lock() -> std::sync::MutexGuard<'static, Option<u32>> {
        SAVED_OUTPUT_CP.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(super) fn prepare(out: &TuiWriter) {
        let Some(handle) = console_handle(out) else {
            return;
        };
        save_and_set_utf8();
        let vt_ok = enable_vt(handle);
        let stdout_vt = std_output_handle().map(enable_vt).unwrap_or(false);
        whycodes_core::logging::emit(
            "whycodes_tui",
            "info",
            "tui.windows_console",
            Some(serde_json::json!({
                "output_cp": CP_UTF8,
                "vt": vt_ok,
                "stdout_vt": stdout_vt,
            })),
        );
    }

    pub(super) fn restore() {
        let prev = saved_lock().take();
        let Some(cp) = prev else {
            return;
        };
        let ok = unsafe { SetConsoleOutputCP(cp) };
        if ok == 0 {
            tracing::debug!(cp, "restore console output code page failed");
        }
    }

    fn save_and_set_utf8() {
        let current = unsafe { GetConsoleOutputCP() };
        if current != 0 {
            let mut saved = saved_lock();
            if saved.is_none() {
                *saved = Some(current);
            }
        }
        let ok = unsafe { SetConsoleOutputCP(CP_UTF8) };
        if ok == 0 {
            tracing::debug!("SetConsoleOutputCP(65001) failed");
        }
    }

    fn enable_vt(handle: *mut core::ffi::c_void) -> bool {
        if handle.is_null() || handle == (-1isize as *mut core::ffi::c_void) {
            return false;
        }
        let mut mode = 0u32;
        if unsafe { GetConsoleMode(handle, &mut mode) } == 0 {
            return false;
        }
        let new_mode = vt_processing_mode(mode);
        if new_mode == mode {
            return true;
        }
        let ok = unsafe { SetConsoleMode(handle, new_mode) };
        ok != 0
    }

    fn console_handle(out: &TuiWriter) -> Option<*mut core::ffi::c_void> {
        use std::os::windows::io::AsRawHandle;
        match out {
            TuiWriter::Console(f) => Some(f.as_raw_handle()),
            TuiWriter::Stdout(s) => Some(s.as_raw_handle()),
            TuiWriter::Buf(_) | TuiWriter::Fail => None,
        }
    }

    fn std_output_handle() -> Option<*mut core::ffi::c_void> {
        let handle = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
        if handle.is_null() || handle == (-1isize as *mut core::ffi::c_void) {
            None
        } else {
            Some(handle)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_code_page_is_65001() {
        assert_eq!(CP_UTF8, 65001);
    }

    #[test]
    fn vt_mode_sets_processing_without_clearing_other_bits() {
        let old = 0x0002 | 0x0010;
        let mode = vt_processing_mode(old);
        assert_eq!(mode & old, old);
        assert_ne!(mode & ENABLE_VIRTUAL_TERMINAL_PROCESSING, 0);
        assert_ne!(mode & ENABLE_PROCESSED_OUTPUT, 0);
    }

    #[test]
    fn vt_mode_is_idempotent() {
        let once = vt_processing_mode(0);
        assert_eq!(vt_processing_mode(once), once);
    }

    #[test]
    fn restore_without_prepare_is_noop() {
        // Buf/Fail prepare never records a code page, so restore is a no-op.
        restore_windows_console();
    }

    #[test]
    fn prepare_on_buf_writer_is_noop() {
        let buf = TuiWriter::Buf(Vec::new());
        prepare_windows_console(&buf);
        let fail = TuiWriter::Fail;
        prepare_windows_console(&fail);
    }

    /// Live CONOUT$: OEM (e.g. CP857) → 65001 → previous. Skips when there is
    /// no console (piped CI).
    #[cfg(windows)]
    #[test]
    fn prepare_live_console_sets_utf8_then_restores() {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetConsoleOutputCP() -> u32;
        }
        let prev = unsafe { GetConsoleOutputCP() };
        if prev == 0 {
            return;
        }
        let Ok(file) = super::super::try_open_windows_console() else {
            return;
        };
        let out = TuiWriter::Console(file);
        prepare_windows_console(&out);
        let mid = unsafe { GetConsoleOutputCP() };
        restore_windows_console();
        let after = unsafe { GetConsoleOutputCP() };
        assert_eq!(mid, CP_UTF8, "TUI must switch the console to UTF-8");
        assert_eq!(
            after, prev,
            "TUI must restore the previous output code page"
        );
    }
}
