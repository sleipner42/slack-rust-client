//! When "Remind me about this" reminds you, as in Slack's menu.

use chrono::{DateTime, Datelike, Days, Duration, NaiveTime, TimeZone};

/// Slack's choices, as labels and Unix times, counted from `now`.
pub fn reminder_choices<Tz: TimeZone>(now: &DateTime<Tz>) -> Vec<(&'static str, i64)> {
    let morning = |days: u64| {
        now.date_naive()
            .checked_add_days(Days::new(days))
            .map(|date| date.and_time(NaiveTime::from_hms_opt(9, 0, 0).unwrap_or_default()))
            .and_then(|time| time.and_local_timezone(now.timezone()).earliest())
            .map(|time| time.timestamp())
    };
    let next_monday = 7 - u64::from(now.weekday().num_days_from_monday());
    let mut choices = vec![
        (
            "In 20 minutes",
            (now.clone() + Duration::minutes(20)).timestamp(),
        ),
        ("In 1 hour", (now.clone() + Duration::hours(1)).timestamp()),
        ("In 3 hours", (now.clone() + Duration::hours(3)).timestamp()),
    ];
    choices.extend(morning(1).map(|time| ("Tomorrow at 9:00", time)));
    choices.extend(morning(next_monday).map(|time| ("Next Monday at 9:00", time)));
    choices
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    #[test]
    fn choices_count_from_now_and_mornings_are_local() {
        let zone = FixedOffset::east_opt(2 * 3600).unwrap();
        // Wednesday 21 October 2026, 15:30 at UTC+2.
        let now = zone.with_ymd_and_hms(2026, 10, 21, 15, 30, 0).unwrap();
        let at = |y, m, d, h| zone.with_ymd_and_hms(y, m, d, h, 0, 0).unwrap().timestamp();
        let choices = reminder_choices(&now);
        assert_eq!(choices[0].1, now.timestamp() + 20 * 60);
        assert_eq!(choices[2].1, now.timestamp() + 3 * 3600);
        assert_eq!(choices[3], ("Tomorrow at 9:00", at(2026, 10, 22, 9)));
        assert_eq!(choices[4], ("Next Monday at 9:00", at(2026, 10, 26, 9)));
        let monday = zone.with_ymd_and_hms(2026, 10, 26, 8, 0, 0).unwrap();
        assert_eq!(
            reminder_choices(&monday)[4].1,
            at(2026, 11, 2, 9),
            "a week on"
        );
    }
}
