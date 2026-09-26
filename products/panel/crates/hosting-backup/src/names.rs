use croner::parser::{CronParser, Seconds, Year};
use keel::prelude::*;
use keel::sdk::chrono::{DateTime, Utc};

pub(crate) const SCHEDULE_MAX: usize = 100;

const PREFIX: &str = "keel-panel-";
const ARCHIVE_SUFFIX: &str = ".tar.zst";
const SCHEDULE: &str = "hosting.backup.schedule.error.expression";
const FIELDS: usize = 5;

pub(crate) fn archive_of(id: &str) -> String {
    format!("{PREFIX}{id}{ARCHIVE_SUFFIX}")
}

pub(crate) fn dump_of(run: &str, index: usize) -> String {
    format!("{PREFIX}{run}-{index}.sql")
}

pub(crate) fn download_of(domain: &str, at: DateTime<Utc>) -> String {
    format!("{domain}-{}{ARCHIVE_SUFFIX}", at.format("%Y%m%dT%H%M%SZ"))
}

pub(crate) fn next_run(text: &str, after: DateTime<Utc>) -> Result<(String, DateTime<Utc>)> {
    let refused = || {
        KeelError::validation(SCHEDULE)
            .field("schedule")
            .param("schedule", text)
    };
    let trimmed = text.trim();
    if trimmed.len() > SCHEDULE_MAX || trimmed.split_whitespace().count() != FIELDS {
        return Err(refused());
    }
    let cron = CronParser::builder()
        .seconds(Seconds::Disallowed)
        .year(Year::Disallowed)
        .build()
        .parse(trimmed)
        .map_err(|_| refused())?;
    let next = cron
        .find_next_occurrence(&after, false)
        .map_err(|_| refused())?;
    Ok((
        trimmed.split_whitespace().collect::<Vec<_>>().join(" "),
        next,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        text.parse().unwrap()
    }

    #[test]
    fn a_schedule_is_five_cron_fields_and_names_its_next_run_after_the_given_instant() {
        let (text, next) = next_run(" 30  3 * * * ", at("2026-09-26T02:00:00Z")).unwrap();
        assert_eq!(text, "30 3 * * *");
        assert_eq!(next, at("2026-09-26T03:30:00Z"));
        let (_, next) = next_run("30 3 * * *", at("2026-09-26T03:30:00Z")).unwrap();
        assert_eq!(next, at("2026-09-27T03:30:00Z"));
        for refused in ["", "* * * *", "0 0 3 * * *", "61 * * * *", "daily"] {
            let error = next_run(refused, at("2026-09-26T02:00:00Z")).unwrap_err();
            assert_eq!(error.key(), SCHEDULE, "{refused}");
            assert_eq!(error.field_path(), Some("schedule"), "{refused}");
        }
    }

    #[test]
    fn archives_and_dumps_are_keyed_by_the_row_under_the_drop_in_prefix() {
        assert_eq!(archive_of("01J9"), "keel-panel-01J9.tar.zst");
        assert_eq!(dump_of("01J9", 2), "keel-panel-01J9-2.sql");
        assert_eq!(
            download_of("a.org", at("2026-09-26T03:30:05Z")),
            "a.org-20260926T033005Z.tar.zst"
        );
    }
}
