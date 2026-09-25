use keel_sdk::chrono::{DateTime, Days, Utc};
use keel_sdk::prelude::*;

const ERROR_INSTANT: &str = "stats.error.instant";

pub(super) fn floor(at: DateTime<Utc>, seconds: i64) -> Result<DateTime<Utc>> {
    let timestamp = at.timestamp();
    DateTime::from_timestamp(timestamp - timestamp.rem_euclid(seconds), 0)
        .ok_or_else(|| instant(at))
}

pub(super) fn before(now: DateTime<Utc>, days: u32) -> Result<DateTime<Utc>> {
    now.checked_sub_days(Days::new(u64::from(days)))
        .ok_or_else(|| instant(now))
}

fn instant(at: DateTime<Utc>) -> KeelError {
    KeelError::internal(ERROR_INSTANT).param("at", at.to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        text.parse().unwrap()
    }

    #[test]
    fn an_instant_is_floored_to_the_minute_the_hour_and_the_day_in_utc() {
        let instant = at("2026-09-25T13:47:31.250Z");
        assert_eq!(floor(instant, 60).unwrap(), at("2026-09-25T13:47:00Z"));
        assert_eq!(floor(instant, 3_600).unwrap(), at("2026-09-25T13:00:00Z"));
        assert_eq!(floor(instant, 86_400).unwrap(), at("2026-09-25T00:00:00Z"));
        assert_eq!(
            floor(at("1969-12-31T23:59:30Z"), 60).unwrap(),
            at("1969-12-31T23:59:00Z")
        );
    }
}
