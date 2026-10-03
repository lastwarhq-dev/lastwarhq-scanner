//! UTC dates and times without a date library. Times are `Duration`s since the Unix epoch.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The current time since the Unix epoch.
pub fn now() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}

/// (year, month 1–12, day) for days since 1970-01-01 (Howard Hinnant's algorithm).
pub fn civil(days: u64) -> (i64, u32, u32) {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// `YYYY-MM-DDTHH:MM:SSZ`.
pub fn utc_iso(time: Duration) -> String {
    let secs = time.as_secs();
    let (year, month, day) = civil(secs / 86_400);
    let rem = secs % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_dates() {
        assert_eq!(utc_iso(Duration::ZERO), "1970-01-01T00:00:00Z");
        assert_eq!(
            utc_iso(Duration::from_secs(951_782_400)),
            "2000-02-29T00:00:00Z"
        );
        assert_eq!(
            utc_iso(Duration::from_millis(1_790_965_360_911)),
            "2026-10-02T18:22:40Z"
        );
    }
}
