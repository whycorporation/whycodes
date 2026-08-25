//! Embed short git hash + build date into the binary for `whycodes --version`.
//!
//! Release artifacts should report more than the crate version so installers
//! and bug reports can pin an exact build. Missing `.git` (source tarball,
//! some CI checkouts) falls back to `unknown` rather than failing the build.

use std::process::Command;

fn main() {
    let hash = git_short_hash().unwrap_or_else(|| "unknown".into());
    let date = build_date_utc();

    println!("cargo:rustc-env=WHYCODES_GIT_HASH={hash}");
    println!("cargo:rustc-env=WHYCODES_BUILD_DATE={date}");

    // Rebuild when HEAD moves (best-effort; ignored if .git is absent).
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/heads/main");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
}

fn git_short_hash() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8(output.stdout).ok()?;
    let s = s.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

fn build_date_utc() -> String {
    if let Ok(epoch) = std::env::var("SOURCE_DATE_EPOCH")
        && let Ok(secs) = epoch.parse::<i64>()
    {
        // Minimal UTC YYYY-MM-DD from unix seconds without pulling chrono into build.rs.
        // Algorithm: civil_from_days (Howard Hinnant), days since 1970-01-01.
        return civil_date_from_unix(secs);
    }

    // Prefer `date -u` for a human-readable UTC stamp on developer machines.
    if let Ok(output) = Command::new("date").args(["-u", "+%Y-%m-%d"]).output()
        && output.status.success()
        && let Ok(s) = String::from_utf8(output.stdout)
    {
        let s = s.trim();
        if !s.is_empty() {
            return s.to_string();
        }
    }

    "unknown".into()
}

/// Convert Unix timestamp (seconds) to `YYYY-MM-DD` in UTC.
fn civil_date_from_unix(secs: i64) -> String {
    // Floor-divide for negative epochs.
    let days = if secs >= 0 {
        secs / 86_400
    } else {
        (secs - 86_399) / 86_400
    };
    // civil_from_days: z = days + 719468 (shift to 0000-03-01 era)
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}
