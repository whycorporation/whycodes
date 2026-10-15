//! Keep resident memory close to what the process actually uses.
//!
//! glibc keeps freed heap pages in its arenas, so after a burst (a long
//! history loaded, a big request built, a turn finished) the resident set
//! stays high even though the program no longer needs the memory. This module
//! limits that and gives the pages back:
//!
//! - [`configure_system_allocator`] caps the number of malloc arenas and pins
//!   the mmap threshold, so large transient buffers are mmapped and returned
//!   to the OS as soon as they are freed (glibc otherwise raises the threshold
//!   dynamically and keeps them in the heap).
//! - [`release_retained_heap`] runs `malloc_trim(0)`.
//! - [`RetentionWatch`] trims when memory that is resident but not allocated
//!   grows past the lowest level seen since the last trim.
//!
//! Everything is a no-op outside Linux/glibc.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Default cap on glibc malloc arenas (glibc's own default is 8 × cores).
pub const DEFAULT_ARENA_MAX: i32 = 4;
/// Default mmap threshold: allocations at least this big bypass the arenas.
pub const DEFAULT_MMAP_THRESHOLD: i32 = 256 * 1024;

static LAST_TRIM: Mutex<Option<Instant>> = Mutex::new(None);

/// Apply the allocator limits once at startup. `WHYCODES_GLIBC_ARENA_MAX` and
/// `WHYCODES_GLIBC_MMAP_THRESHOLD` override the defaults; `0` keeps glibc's
/// own behaviour for that setting.
pub fn configure_system_allocator() {
    let arena_max = env_int("WHYCODES_GLIBC_ARENA_MAX", DEFAULT_ARENA_MAX);
    let mmap_threshold = env_int("WHYCODES_GLIBC_MMAP_THRESHOLD", DEFAULT_MMAP_THRESHOLD);
    glibc::set_limits(arena_max, mmap_threshold);
}

fn env_int(key: &str, default: i32) -> i32 {
    parse_int(std::env::var(key).as_deref().unwrap_or(""), default)
}

fn parse_int(raw: &str, default: i32) -> i32 {
    raw.trim().parse().unwrap_or(default)
}

/// Return freed-but-retained heap pages to the OS.
pub fn release_retained_heap(reason: &'static str) {
    let trimmed = glibc::trim();
    tracing::debug!(reason, trimmed, "released retained heap");
    *crate::file_claims::recover_lock(LAST_TRIM.lock()) = Some(Instant::now());
}

/// [`release_retained_heap`] unless one already ran within `min_interval`.
/// Returns whether it ran.
pub fn release_retained_heap_debounced(reason: &'static str, min_interval: Duration) -> bool {
    let last = *crate::file_claims::recover_lock(LAST_TRIM.lock());
    let recent = last.is_some_and(|t| t.elapsed() < min_interval);
    if recent {
        return false;
    }
    release_retained_heap(reason);
    true
}

/// Resident anonymous memory not held by live allocations: what a trim could
/// give back. `None` where it cannot be measured.
pub fn retained_bytes() -> Option<u64> {
    let rss_anon = rss_anon_bytes()?;
    let live = glibc::live_bytes()?;
    Some(rss_anon.saturating_sub(live))
}

/// `RssAnon` of this process in bytes (Linux).
#[cfg(target_os = "linux")]
pub fn rss_anon_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status");
    status.ok().and_then(|s| parse_rss_anon(&s))
}

/// `RssAnon` of this process in bytes (Linux).
#[cfg(not(target_os = "linux"))]
pub fn rss_anon_bytes() -> Option<u64> {
    None
}

/// `RssAnon:` from a `/proc/<pid>/status` body, in bytes.
pub fn parse_rss_anon(status: &str) -> Option<u64> {
    let line = status.lines().find(|l| l.starts_with("RssAnon:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// Trims when retained memory grows `threshold` bytes past a baseline. The
/// baseline is the level measured after the last trim and only moves down
/// between trims: glibc's free-chunk counters do not drop after a trim, so a
/// plain threshold would fire again on every check.
#[derive(Debug)]
pub struct RetentionWatch {
    threshold: u64,
    baseline: Option<u64>,
}

impl RetentionWatch {
    pub fn new(threshold_bytes: u64) -> Self {
        Self {
            threshold: threshold_bytes,
            baseline: None,
        }
    }

    /// Measure and trim if retention grew too much. Returns whether it trimmed.
    pub fn check(&mut self, reason: &'static str) -> bool {
        retained_bytes().is_some_and(|r| self.observe(r, reason, retained_bytes))
    }

    /// Decide on one measurement; `after_trim` re-measures once trimmed.
    pub fn observe(
        &mut self,
        retained: u64,
        reason: &'static str,
        after_trim: impl FnOnce() -> Option<u64>,
    ) -> bool {
        let baseline = *self.baseline.get_or_insert(retained);
        if retained < baseline {
            self.baseline = Some(retained);
        }
        if retained.saturating_sub(baseline) < self.threshold {
            return false;
        }
        release_retained_heap(reason);
        self.baseline = Some(after_trim().unwrap_or(retained));
        true
    }

    /// The level new growth is measured against.
    pub fn baseline(&self) -> Option<u64> {
        self.baseline
    }
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
mod glibc {
    use std::ffi::{c_char, c_int, c_void};

    // <malloc.h> mallopt parameters.
    const M_MMAP_THRESHOLD: c_int = -3;
    const M_ARENA_MAX: c_int = -8;

    // Field layout of `struct mallinfo2`; only two fields are read.
    #[repr(C)]
    #[allow(dead_code)]
    struct Mallinfo2 {
        arena: usize,
        ordblks: usize,
        smblks: usize,
        hblks: usize,
        hblkhd: usize,
        usmblks: usize,
        fsmblks: usize,
        uordblks: usize,
        fordblks: usize,
        keepcost: usize,
    }

    unsafe extern "C" {
        fn mallopt(param: c_int, value: c_int) -> c_int;
        fn malloc_trim(pad: usize) -> c_int;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }

    pub(super) fn set_limits(arena_max: i32, mmap_threshold: i32) {
        // SAFETY: mallopt only adjusts allocator tunables; invalid values are
        // rejected by glibc with a 0 return.
        unsafe {
            if arena_max > 0 {
                mallopt(M_ARENA_MAX, arena_max);
            }
            if mmap_threshold > 0 {
                mallopt(M_MMAP_THRESHOLD, mmap_threshold);
            }
        }
    }

    pub(super) fn trim() -> bool {
        // SAFETY: malloc_trim(0) walks the arenas and releases free pages.
        unsafe { malloc_trim(0) == 1 }
    }

    /// Bytes in live allocations (`uordblks` + mmapped `hblkhd`). Resolved at
    /// run time: `mallinfo2` only exists since glibc 2.33.
    pub(super) fn live_bytes() -> Option<u64> {
        // SAFETY: RTLD_DEFAULT (null) searches the global scope; the symbol,
        // when present, has the documented `struct mallinfo2 (void)` signature.
        let sym = unsafe { dlsym(std::ptr::null_mut(), c"mallinfo2".as_ptr()) };
        (!sym.is_null()).then(|| {
            let mallinfo2: extern "C" fn() -> Mallinfo2 = unsafe { std::mem::transmute(sym) };
            let info = mallinfo2();
            (info.uordblks + info.hblkhd) as u64
        })
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
mod glibc {
    pub(super) fn set_limits(_arena_max: i32, _mmap_threshold: i32) {}

    pub(super) fn trim() -> bool {
        false
    }

    pub(super) fn live_bytes() -> Option<u64> {
        None
    }
}

#[cfg(test)]
#[path = "process_memory_tests.rs"]
mod tests;
