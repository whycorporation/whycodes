use super::*;
use chrono::TimeZone;

fn local(y: i32, m: u32, d: u32, h: u32, min: u32) -> chrono::DateTime<chrono::Local> {
    chrono::Local
        .with_ymd_and_hms(y, m, d, h, min, 0)
        .single()
        .expect("valid local datetime")
}

#[test]
fn clock_is_12_hour_ampm() {
    let ts = local(2026, 8, 15, 14, 32).with_timezone(&chrono::Utc);
    assert_eq!(format_clock(ts), "2:32 PM");
    let morning = local(2026, 8, 15, 9, 5).with_timezone(&chrono::Utc);
    assert_eq!(format_clock(morning), "9:05 AM");
    let noon = local(2026, 8, 15, 12, 0).with_timezone(&chrono::Utc);
    assert_eq!(format_clock(noon), "12:00 PM");
    let midnight = local(2026, 8, 15, 0, 7).with_timezone(&chrono::Utc);
    assert_eq!(format_clock(midnight), "12:07 AM");
}

#[test]
fn absolute_is_month_day_clock() {
    let ts = local(2026, 8, 15, 14, 32).with_timezone(&chrono::Utc);
    let s = format_absolute(ts);
    assert!(s.contains("15"), "{s}");
    assert!(s.ends_with("14:32"), "{s}");
    assert!(s.contains("August") || s.contains("Ağustos"), "{s}");
}

#[test]
fn relative_just_now_and_minutes() {
    let now = local(2026, 8, 15, 16, 0);
    let fresh = chrono::Local
        .with_ymd_and_hms(2026, 8, 15, 15, 59, 20)
        .single()
        .expect("valid local datetime")
        .with_timezone(&chrono::Utc);
    assert_eq!(format_relative_at(fresh, now), "just now");
    let mins = local(2026, 8, 15, 15, 10).with_timezone(&chrono::Utc);
    assert_eq!(format_relative_at(mins, now), "50m ago");
    let hours = local(2026, 8, 15, 13, 0).with_timezone(&chrono::Utc);
    assert_eq!(format_relative_at(hours, now), "3h ago");
}

#[test]
fn relative_yesterday() {
    let now = local(2026, 8, 15, 16, 0);
    let y = local(2026, 8, 14, 9, 5).with_timezone(&chrono::Utc);
    assert_eq!(format_relative_at(y, now), "Yesterday, 09:05");
}

#[test]
fn relative_falls_back_to_absolute_after_a_day() {
    let now = local(2026, 8, 15, 16, 0);
    let older = local(2026, 8, 1, 9, 5).with_timezone(&chrono::Utc);
    let s = format_relative_at(older, now);
    assert!(s.contains("1"), "{s}");
    assert!(s.ends_with("09:05"), "{s}");
    let _ = format_relative(older);
}
