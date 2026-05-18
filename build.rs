//! Tiny build script that injects two environment variables:
//!
//! * `WIRI_GIT_HASH` — short git commit hash for the version string in
//!   `wiri-ctl --version`.  Empty / unset if git is unavailable.
//! * `WIRI_BUILD_DATE` — ISO-ish build date (YYYY-MM-DD).
//!
//! It also re-runs on every build (cheap) so the version reflects the latest
//! commit without forcing a clean rebuild.

use std::process::Command;

fn main() {
    // Re-run if .git/HEAD changes (cheap heuristic — covers commit/checkout).
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=build.rs");

    // ---- WIRI_GIT_HASH --------------------------------------------------
    let hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| if o.status.success() { Some(o.stdout) } else { None })
        .map(|b| String::from_utf8_lossy(&b).trim().to_string())
        .unwrap_or_default();
    println!("cargo:rustc-env=WIRI_GIT_HASH={}", hash);

    // ---- WIRI_BUILD_DATE ------------------------------------------------
    // Use SystemTime; format YYYY-MM-DD without any extra crates.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d) = days_to_ymd((secs / 86400) as i64);
    println!("cargo:rustc-env=WIRI_BUILD_DATE={:04}-{:02}-{:02}", y, m, d);
}

/// Convert a Unix day count (days since 1970-01-01) to (year, month, day).
/// Uses Howard Hinnant's "civil_from_days" algorithm — correct for any era.
fn days_to_ymd(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}
