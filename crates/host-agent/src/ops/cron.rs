use std::fs;
use std::io;
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{Value, json};

use super::files::{Previous, place};
use super::user::inside;
use super::{Call, plain_target, valid_file_name};
use crate::protocol::Failure;

const CRON_MODE: u32 = 0o644;
const JOB_LIMIT: usize = 64;
const COMMAND_LIMIT: usize = 4096;
const NICKNAMES: [&str; 8] = [
    "@reboot",
    "@yearly",
    "@annually",
    "@monthly",
    "@weekly",
    "@daily",
    "@midnight",
    "@hourly",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Write {
    name: String,
    jobs: Vec<Job>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Job {
    schedule: String,
    user: String,
    exec: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Remove {
    name: String,
}

pub(super) fn write(call: &Call<'_>, args: Write) -> Result<Value, Failure> {
    let target = target(call, &args.name)?;
    if args.jobs.is_empty() || args.jobs.len() > JOB_LIMIT {
        return Err(Failure::args(format!(
            "a drop-in carries between 1 and {JOB_LIMIT} jobs"
        )));
    }
    let mut content = String::new();
    for job in &args.jobs {
        let schedule = schedule(&job.schedule)?;
        runs_as(call, &job.user)?;
        let argv = call.policy.exec(&job.exec).ok_or_else(|| {
            Failure::policy(format!("{} is not an entry of exec_allow", job.exec))
        })?;
        let command = command(argv)?;
        content.push_str(&format!("{schedule} {} {command}\n", job.user));
    }
    let previous = Previous::read(&target)?;
    if previous.as_ref().is_some_and(|previous| {
        previous.content == content.as_bytes() && previous.mode == CRON_MODE
    }) {
        return Ok(json!({ "changed": false }));
    }
    place(&target, content.as_bytes(), CRON_MODE, None)?;
    Ok(json!({ "changed": true }))
}

pub(super) fn remove(call: &Call<'_>, args: Remove) -> Result<Value, Failure> {
    let target = target(call, &args.name)?;
    match fs::remove_file(&target) {
        Ok(()) => Ok(json!({ "removed": true })),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(json!({ "removed": false })),
        Err(error) => Err(Failure::io(target.display(), &error)),
    }
}

pub(super) fn list(call: &Call<'_>, (): ()) -> Result<Value, Failure> {
    let prefix = call.drop_in_prefix()?;
    let dir = directory(call)?;
    let mut names = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|error| Failure::io(dir.display(), &error))? {
        let entry = entry.map_err(|error| Failure::io(dir.display(), &error))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if valid_file_name(&name, prefix) && drop_in_characters(&name) {
            names.push(name);
        }
    }
    names.sort();
    Ok(json!({ "entries": names }))
}

fn target(call: &Call<'_>, name: &str) -> Result<PathBuf, Failure> {
    let prefix = call.drop_in_prefix()?;
    if !valid_file_name(name, prefix) || !drop_in_characters(name) {
        return Err(Failure::policy(format!(
            "the name {name} is not a cron drop-in name under the prefix {prefix}"
        )));
    }
    plain_target(&directory(call)?, name)
}

fn directory(call: &Call<'_>) -> Result<PathBuf, Failure> {
    let dir = &call.policy.tools().cron_dir;
    dir.canonicalize().map_err(|error| {
        Failure::policy(format!(
            "the cron directory {} is not reachable: {error}",
            dir.display()
        ))
    })
}

fn drop_in_characters(name: &str) -> bool {
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn runs_as(call: &Call<'_>, name: &str) -> Result<(), Failure> {
    let user = call
        .accounts()
        .user(name)?
        .filter(|user| user.name == name)
        .ok_or_else(|| Failure::policy(format!("{name} is not a user of this host")))?;
    if user.uid == call.admitted {
        return Ok(());
    }
    call.account_name(name)?;
    inside(call.uid_range()?, user.uid, name)
}

fn schedule(text: &str) -> Result<String, Failure> {
    let trimmed = text.trim();
    if NICKNAMES.contains(&trimmed) {
        return Ok(trimmed.to_owned());
    }
    let fields: Vec<&str> = trimmed.split_whitespace().collect();
    let valid = fields.len() == 5
        && fields.iter().all(|field| {
            field
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '*' | '/' | ',' | '-'))
        });
    if valid {
        Ok(fields.join(" "))
    } else {
        Err(Failure::args(format!("{text:?} is not a cron schedule")))
    }
}

fn command(argv: &[String]) -> Result<String, Failure> {
    let mut line = String::new();
    for argument in argv {
        if argument.contains(['\n', '\r', '\0']) || argument.contains("\\%") {
            return Err(Failure::policy(format!(
                "the argument {argument:?} of an exec_allow entry cannot stand in a cron line"
            )));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push('\'');
        line.push_str(&argument.replace('\'', "'\\''").replace('%', "\\%"));
        line.push('\'');
    }
    if line.is_empty() || line.len() > COMMAND_LIMIT {
        return Err(Failure::policy(format!(
            "a job's command line is between 1 and {COMMAND_LIMIT} bytes"
        )));
    }
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ERROR_ARGS, ERROR_POLICY};

    #[test]
    fn a_schedule_is_five_fields_or_a_nickname_and_nothing_that_ends_the_line() {
        assert_eq!(schedule("17 3 * * *").unwrap(), "17 3 * * *");
        assert_eq!(
            schedule(" */5  1-5 * jan,feb mon ").unwrap(),
            "*/5 1-5 * jan,feb mon"
        );
        assert_eq!(schedule("@daily").unwrap(), "@daily");
        for bad in [
            "",
            "* * * *",
            "* * * * * *",
            "@often",
            "* * * * ;",
            "1 2 3 4 5 root",
        ] {
            assert_eq!(schedule(bad).unwrap_err().key, ERROR_ARGS, "{bad}");
        }
    }

    #[test]
    fn a_command_quotes_each_argument_and_keeps_its_percent_signs_literal() {
        let argv = |parts: &[&str]| {
            parts
                .iter()
                .map(|part| (*part).to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            command(&argv(&["/opt/keel/bin/current", "ctl", "note.recompute"])).unwrap(),
            "'/opt/keel/bin/current' 'ctl' 'note.recompute'"
        );
        assert_eq!(
            command(&argv(&["logger", "it's 100%", "$(id)"])).unwrap(),
            "'logger' 'it'\\''s 100\\%' '$(id)'"
        );
        for bad in [
            argv(&[]),
            argv(&["a\nb"]),
            argv(&["a\rb"]),
            argv(&["a\0b"]),
            argv(&["a\\%b"]),
            argv(&["x".repeat(COMMAND_LIMIT).as_str()]),
        ] {
            assert_eq!(command(&bad).unwrap_err().key, ERROR_POLICY, "{bad:?}");
        }
    }

    #[test]
    fn a_drop_in_name_is_what_cron_reads() {
        assert!(drop_in_characters("keel-dns-renew_1"));
        assert!(!drop_in_characters("keel-dns-renew.cron"));
        assert!(!drop_in_characters("keel-dns-renew~"));
    }
}
