//! Local wall-clock time without a platform time-zone framework.
//!
//! chrono's `Local` finds the zone through `iana-time-zone`, which on macOS
//! links CoreFoundation. dyld loads it at every process start whether or not
//! a clock is ever read: ~0.9 ms of a ~3.7 ms `whycodes --version` on an M2.
//! On Unix the C library already knows the offset (`localtime_r` reads `TZ`
//! and `/etc/localtime`); Windows keeps chrono's `Local`, which costs nothing
//! extra there.

use chrono::{DateTime, FixedOffset, Offset, Utc};

/// UTC offset of the local time zone at `at` (DST included).
pub fn offset_at(at: DateTime<Utc>) -> FixedOffset {
    FixedOffset::east_opt(offset_seconds(at)).unwrap_or(Utc.fix())
}

/// `at` in local time.
pub fn to_local(at: DateTime<Utc>) -> DateTime<FixedOffset> {
    at.with_timezone(&offset_at(at))
}

/// The current local time.
pub fn now() -> DateTime<FixedOffset> {
    to_local(Utc::now())
}

#[cfg(unix)]
fn offset_seconds(at: DateTime<Utc>) -> i32 {
    let secs = at.timestamp() as libc::time_t;
    // SAFETY: `tm` is a plain C struct for which all-zero bytes are valid.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: `localtime_r` reads `secs` and writes `tm`, both live for the
    // call; it keeps no pointer to either.
    let filled = unsafe { !libc::localtime_r(&secs, &mut tm).is_null() };
    if filled { tm.tm_gmtoff as i32 } else { 0 }
}

#[cfg(windows)]
fn offset_seconds(at: DateTime<Utc>) -> i32 {
    use chrono::TimeZone;
    chrono::Local
        .offset_from_utc_datetime(&at.naive_utc())
        .fix()
        .local_minus_utc()
}

#[cfg(not(any(unix, windows)))]
fn offset_seconds(_at: DateTime<Utc>) -> i32 {
    0
}

#[cfg(test)]
#[path = "local_time_tests.rs"]
mod tests;
