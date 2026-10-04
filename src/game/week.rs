//! VS weeks. Each week runs Monday to Sunday; days reset at 02:00 UTC (00:00 server time).

use std::time::Duration;

/// VS days reset at 02:00 UTC.
const VS_RESET_SECS: u64 = 2 * 3600;

/// The VS week number and day (1 = Monday … 7 = Sunday) at a time since the Unix epoch.
pub fn vs_day(time: Duration) -> (u64, i64) {
    let days = time.as_secs().saturating_sub(VS_RESET_SECS) / 86_400;
    // 1970-01-01 was a Thursday; adding 3 makes each week start on a Monday.
    ((days + 3) / 7, ((days + 3) % 7 + 1) as i64)
}

/// Whether Desert Storm sign-ups are open at `time`: Monday to Friday of the VS week. The
/// week's battles end on Friday; from Saturday the participants panel lists the alliance with
/// empty time slots and no other sign-up fields, until the Monday 02:00 UTC reset.
pub fn ds_signups_open(time: Duration) -> bool {
    vs_day(time).1 <= 5
}

/// Whether the game still shows VS day `day`'s ranking on `today` (both 1 = Monday … 7 =
/// Sunday): on Sunday only Saturday's is shown, Monday to Friday's are gone.
pub fn vs_ranking_shown(day: i64, today: i64) -> bool {
    today < 7 || day == 6
}

/// Days since 1970-01-01 of the Monday that starts VS week `week`.
pub fn monday_days(week: u64) -> u64 {
    (week * 7).saturating_sub(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Friday 2026-10-02 20:00 UTC.
    const FRIDAY: u64 = 1_790_971_200;
    const DAY: u64 = 86_400;

    #[test]
    fn vs_days_reset_at_two_utc() {
        assert_eq!(vs_day(Duration::from_secs(FRIDAY)).1, 5);
        // Friday 01:59 UTC is still Thursday; 02:00 is Friday.
        let friday_midnight = FRIDAY - 20 * 3600;
        assert_eq!(vs_day(Duration::from_secs(friday_midnight + 7199)).1, 4);
        assert_eq!(vs_day(Duration::from_secs(friday_midnight + 7200)).1, 5);
        // Monday 02:00 starts a new week.
        let (week, _) = vs_day(Duration::from_secs(FRIDAY));
        let monday = friday_midnight + 3 * DAY + 7200;
        assert_eq!(vs_day(Duration::from_secs(monday)), (week + 1, 1));
        assert_eq!(vs_day(Duration::from_secs(monday - 1)), (week, 7));
        // That week's Monday is 2026-09-28: day 20724 since 1970-01-01.
        assert_eq!(monday_days(week), 20_724);
    }

    #[test]
    fn ds_signups_close_from_saturday_to_the_monday_reset() {
        let saturday = FRIDAY - 20 * 3600 + DAY + 7200;
        let monday = saturday + 2 * DAY;
        let open = |t: u64| ds_signups_open(Duration::from_secs(t));
        assert!(open(FRIDAY));
        // Friday's 01:00 UTC battle slot falls on Saturday's date, but is still Friday.
        assert!(open(saturday - 1));
        assert!(!open(saturday));
        assert!(!open(monday - 1));
        assert!(open(monday));
    }
}
