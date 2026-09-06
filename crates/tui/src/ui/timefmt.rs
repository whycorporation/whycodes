//! Grok Build clock labels for chat bubbles and the session picker.
//!
//! Chat clocks (always on): `2:32 PM` — Grok pager `%-I:%M %p`.
//! History / welcome: `just now`, `5m ago`, `3h ago`, then the long absolute.

use chrono::{Datelike, Timelike};

/// Short bubble clock — Grok overlay `%-I:%M %p` (`2:32 PM`).
pub fn format_clock(ts: chrono::DateTime<chrono::Utc>) -> String {
    let local = ts.with_timezone(&chrono::Local);
    let (pm, hour) = local.hour12();
    let ampm = if pm { "PM" } else { "AM" };
    format!("{hour}:{:02} {ampm}", local.minute())
}

/// Long absolute — session picker / welcome after a day (`August 15, 14:32`).
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
#[path = "timefmt_tests.rs"]
mod tests;
