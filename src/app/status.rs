//! How long a status message lasts.

use super::App;
use crate::model::{Millis, StatusExpiry};
use jiff::{Span, Timestamp, civil::Weekday, tz::TimeZone};

impl App {
    /// When the status message being set clears, from the chosen expiry.
    pub(super) fn status_expiry_millis(&self) -> Option<Millis> {
        expiry_millis(self.status_expiry, crate::time::now(), &TimeZone::system())
    }
}

/// The time (UTC milliseconds) `expiry` ends, counted from `now` in `tz`;
/// `None` for never.
pub fn expiry_millis(expiry: StatusExpiry, now: Millis, tz: &TimeZone) -> Option<Millis> {
    const HOUR: Millis = 3_600_000;
    match expiry {
        StatusExpiry::Never => None,
        StatusExpiry::OneHour => now.checked_add(HOUR),
        StatusExpiry::FourHours => now.checked_add(4 * HOUR),
        StatusExpiry::Today => next_midnight(now, tz, 1),
        StatusExpiry::ThisWeek => {
            let zoned = Timestamp::from_millisecond(now).ok()?.to_zoned(tz.clone());
            let since_monday = i32::from(zoned.weekday().since(Weekday::Monday));
            next_midnight(now, tz, 7 - since_monday)
        }
    }
}

/// Local midnight `days` calendar days after the day containing `now`.
fn next_midnight(now: Millis, tz: &TimeZone, days: i32) -> Option<Millis> {
    let zoned = Timestamp::from_millisecond(now).ok()?.to_zoned(tz.clone());
    let target = zoned.date().checked_add(Span::new().days(days)).ok()?;
    let start = target.to_zoned(tz.clone()).ok()?;
    Some(start.timestamp().as_millisecond())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(iso: &str) -> Millis {
        iso.parse::<Timestamp>().unwrap().as_millisecond()
    }

    #[test]
    fn never_is_none() {
        assert_eq!(expiry_millis(StatusExpiry::Never, 5, &TimeZone::UTC), None);
    }

    #[test]
    fn one_hour() {
        assert_eq!(
            expiry_millis(StatusExpiry::OneHour, 0, &TimeZone::UTC),
            Some(3_600_000)
        );
    }

    #[test]
    fn four_hours() {
        assert_eq!(
            expiry_millis(StatusExpiry::FourHours, 1000, &TimeZone::UTC),
            Some(1000 + 4 * 3_600_000)
        );
    }

    #[test]
    fn today_is_next_midnight() {
        let now = ms("2026-11-22T10:00:00Z");
        assert_eq!(
            expiry_millis(StatusExpiry::Today, now, &TimeZone::UTC),
            Some(ms("2026-11-23T00:00:00Z"))
        );
    }

    #[test]
    fn today_at_exact_midnight_is_a_day_later() {
        let now = ms("2026-11-22T00:00:00Z");
        assert_eq!(
            expiry_millis(StatusExpiry::Today, now, &TimeZone::UTC),
            Some(ms("2026-11-23T00:00:00Z"))
        );
    }

    #[test]
    fn today_across_dst() {
        let berlin = TimeZone::get("Europe/Berlin").unwrap();
        // Clocks go back on 2026-10-25: that local day has 25 hours.
        let now = ms("2026-10-25T09:00:00Z"); // 10:00 CET after the change
        let start = ms("2026-10-24T22:00:00Z"); // local midnight, CEST
        let end = expiry_millis(StatusExpiry::Today, now, &berlin).unwrap();
        assert_eq!(end, ms("2026-10-25T23:00:00Z"));
        assert_eq!(end - start, 25 * 3_600_000);
    }

    #[test]
    fn this_week_from_sunday_is_next_day() {
        let now = ms("2026-11-22T10:00:00Z"); // Sunday
        assert_eq!(
            expiry_millis(StatusExpiry::ThisWeek, now, &TimeZone::UTC),
            Some(ms("2026-11-23T00:00:00Z"))
        );
    }

    #[test]
    fn this_week_on_monday_is_next_monday() {
        let now = ms("2026-11-23T10:00:00Z"); // Monday
        assert_eq!(
            expiry_millis(StatusExpiry::ThisWeek, now, &TimeZone::UTC),
            Some(ms("2026-11-30T00:00:00Z"))
        );
    }

    #[test]
    fn this_week_midweek() {
        let now = ms("2026-11-25T10:00:00Z"); // Wednesday
        assert_eq!(
            expiry_millis(StatusExpiry::ThisWeek, now, &TimeZone::UTC),
            Some(ms("2026-11-30T00:00:00Z"))
        );
    }

    #[test]
    fn this_week_uses_local_day() {
        // Sunday 23:30 UTC is already Monday 00:30 in Berlin (CET).
        let berlin = TimeZone::get("Europe/Berlin").unwrap();
        let now = ms("2026-11-22T23:30:00Z");
        assert_eq!(
            expiry_millis(StatusExpiry::ThisWeek, now, &berlin),
            Some(ms("2026-11-29T23:00:00Z"))
        );
    }
}
