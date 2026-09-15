use std::fmt;

use croner::Cron;
use croner::parser::{CronParser, Seconds, Year};
use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::{KeelError, Result};

pub const ERROR_SCHEDULE: &str = "cron.error.schedule";

#[derive(Clone, Debug)]
pub struct Schedule {
    text: String,
    cron: Cron,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduleError {
    pub text: String,
    pub reason: String,
}

impl fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the schedule {:?} does not parse: {}",
            self.text, self.reason
        )
    }
}

impl std::error::Error for ScheduleError {}

impl Schedule {
    pub fn parse(text: &str) -> std::result::Result<Self, ScheduleError> {
        let trimmed = text.trim();
        let fields = trimmed.split_whitespace().count();
        if fields != 5 {
            return Err(ScheduleError {
                text: text.to_owned(),
                reason: format!("expected five fields, found {fields}"),
            });
        }
        let cron = CronParser::builder()
            .seconds(Seconds::Disallowed)
            .year(Year::Disallowed)
            .build()
            .parse(trimmed)
            .map_err(|error| ScheduleError {
                text: text.to_owned(),
                reason: error.to_string(),
            })?;
        Ok(Self {
            text: trimmed.to_owned(),
            cron,
        })
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn next_after(&self, at: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.cron.find_next_occurrence(&at, false).ok()
    }
}

impl PartialEq for Schedule {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}

impl fmt::Display for Schedule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

pub fn contributed(job: &str, text: &str) -> Result<Schedule> {
    Schedule::parse(text).map_err(|error| {
        KeelError::internal(ERROR_SCHEDULE)
            .param("job", job)
            .param("schedule", text)
            .param("reason", error.reason.clone())
            .source(error)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        text.parse().expect("an instant")
    }

    #[test]
    fn a_five_field_expression_parses_and_answers_the_next_occurrence_after_an_instant() {
        let every_five = Schedule::parse(" */5 * * * * ").expect("parses");
        assert_eq!(every_five.text(), "*/5 * * * *");
        assert_eq!(every_five.to_string(), "*/5 * * * *");
        assert_eq!(
            every_five.next_after(at("2026-09-15T10:00:00Z")),
            Some(at("2026-09-15T10:05:00Z"))
        );
        assert_eq!(
            every_five.next_after(at("2026-09-15T10:04:59.500Z")),
            Some(at("2026-09-15T10:05:00Z"))
        );
        let six_hourly = Schedule::parse("0 */6 * * *").expect("parses");
        assert_eq!(
            six_hourly.next_after(at("2026-09-15T10:00:00Z")),
            Some(at("2026-09-15T12:00:00Z"))
        );
        assert_eq!(
            six_hourly.next_after(at("2026-09-15T12:00:00Z")),
            Some(at("2026-09-15T18:00:00Z"))
        );
        assert_eq!(Schedule::parse("*/5 * * * *").expect("parses"), every_five);
        assert_ne!(Schedule::parse("0 */6 * * *").expect("parses"), every_five);
    }

    #[test]
    fn an_expression_without_five_fields_or_with_a_bad_field_does_not_parse() {
        let words = Schedule::parse("every day").expect_err("words are not a schedule");
        assert_eq!(words.text, "every day");
        assert_eq!(words.reason, "expected five fields, found 2");
        assert_eq!(
            words.to_string(),
            "the schedule \"every day\" does not parse: expected five fields, found 2"
        );
        let seconds = Schedule::parse("0 0 */6 * * *").expect_err("six fields");
        assert_eq!(seconds.reason, "expected five fields, found 6");
        let range = Schedule::parse("0 99 * * *").expect_err("99 is not an hour");
        assert_eq!(range.text, "0 99 * * *");
        assert!(!range.reason.is_empty());
        assert!(Schedule::parse("").is_err());
    }

    #[test]
    fn a_contributed_schedule_that_does_not_parse_is_an_internal_error_naming_the_job() {
        let error = contributed("dns.serial_check", "every day").expect_err("does not parse");
        assert_eq!(error.key(), ERROR_SCHEDULE);
        assert_eq!(error.params()["job"], "dns.serial_check");
        assert_eq!(error.params()["schedule"], "every day");
        assert_eq!(error.params()["reason"], "expected five fields, found 2");
        assert!(error.to_string().contains("dns.serial_check"));
        let ok = contributed("dns.serial_check", "0 */6 * * *").expect("parses");
        assert_eq!(ok.text(), "0 */6 * * *");
    }
}
