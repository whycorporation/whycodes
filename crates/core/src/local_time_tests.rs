use super::*;
use chrono::TimeZone;

/// The TZ test changes the process zone; tests that read it take this lock.
static TZ_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn local_time_keeps_the_instant_and_has_a_sane_offset() {
    let _tz = TZ_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let at = Utc
        .with_ymd_and_hms(2026, 8, 15, 12, 0, 0)
        .single()
        .expect("valid");
    let off = offset_at(at);
    // Real zones are within UTC-12 .. UTC+14.
    assert!((-12 * 3600..=14 * 3600).contains(&off.local_minus_utc()));
    let local = to_local(at);
    assert_eq!(local.with_timezone(&Utc), at, "same instant");
    assert_eq!(*local.offset(), off);
    let now = now();
    assert!((now.with_timezone(&Utc) - Utc::now()).num_seconds().abs() < 5);
}

// POSIX; the `libc` crate does not export it on every Unix (macOS).
#[cfg(unix)]
unsafe extern "C" {
    fn tzset();
}

#[cfg(unix)]
#[test]
fn unix_offset_follows_tz() {
    let _tz = TZ_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: test-only; every test that reads the zone holds TZ_LOCK.
    unsafe {
        std::env::set_var("TZ", "UTC");
        tzset();
    }
    let at = Utc
        .with_ymd_and_hms(2026, 1, 15, 12, 0, 0)
        .single()
        .expect("valid");
    assert_eq!(offset_at(at).local_minus_utc(), 0);
    // SAFETY: as above.
    unsafe {
        std::env::set_var("TZ", "Asia/Tokyo");
        tzset();
    }
    assert_eq!(offset_at(at).local_minus_utc(), 9 * 3600);
    // SAFETY: as above.
    unsafe {
        std::env::remove_var("TZ");
        tzset();
    }
}
