//! Grok Build clock labels for chat bubbles and the session picker.
//!
//! Chat (`/timestamps`): `August 15, 14:32`
//! History / welcome: `just now`, `5m ago`, `3h ago`, then the same absolute.

use chrono::{Datelike, Timelike};

/// Absolute local wall-clock — Grok pager `%B %-d, %H:%M`.
pub fn format_absolute(ts: chrono::DateTime<chrono::Utc>) -> String {
    let local = ts.with_timezone(&chrono::Local);
    format!(
        "{} {}, {:02}:{:02}",
        local.format("%B"),
        local.day(),
        local.hour(),
        local.minute()
    )
}

/// Relative, then absolute — Grok session picker / welcome list.
pub fn format_relative(ts: chrono::DateTime<chrono::Utc>) -> String {
    format_relative_at(ts, chrono::Local::now())
}

pub fn format_relative_at(
    ts: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Local>,
) -> String {
    let local = ts.with_timezone(&chrono::Local);
    let delta = now.signed_duration_since(local);
    let secs = delta.num_seconds();
    if secs < 60 {
        return "just now".into();
    }
    if secs < 3600 {
        return format!("{}m ago", secs / 60);
    }
    if secs < 86_400 {
        return format!("{}h ago", secs / 3600);
    }
    if local.date_naive() == now.date_naive().pred_opt().unwrap_or(now.date_naive()) {
        return format!("Yesterday, {:02}:{:02}", local.hour(), local.minute());
    }
    format_absolute(ts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn local(y: i32, m: u32, d: u32, h: u32, min: u32) -> chrono::DateTime<chrono::Local> {
        chrono::Local
            .with_ymd_and_hms(y, m, d, h, min, 0)
            .single()
            .expect("valid local datetime")
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
}
