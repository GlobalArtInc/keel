use std::fs::{self, OpenOptions};
use std::io;
use std::os::unix::fs::{OpenOptionsExt, fchown};
use std::path::Path;

use nix::fcntl::OFlag;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Call, Dest};
use crate::process;
use crate::protocol::Failure;

const CLIENT: &str = "mariadb";
const DUMP: &str = "mariadb-dump";
const HOST: &str = "localhost";
const NAME_LIMIT: usize = 64;
const PASSWORD_LIMIT: usize = 256;
const DUMP_MODE: u32 = 0o600;
const REDACTED: &str = "***";
const SESSION: &str =
    "SET SESSION sql_mode = (SELECT REPLACE(@@SESSION.sql_mode, 'NO_BACKSLASH_ESCAPES', ''));";

#[derive(Deserialize)]
#[serde(tag = "statement", deny_unknown_fields)]
pub(super) enum Statement {
    #[serde(rename = "database.create")]
    DatabaseCreate { database: String },
    #[serde(rename = "database.drop")]
    DatabaseDrop { database: String },
    #[serde(rename = "user.create")]
    UserCreate { user: String, password: String },
    #[serde(rename = "user.password")]
    UserPassword { user: String, password: String },
    #[serde(rename = "user.drop")]
    UserDrop { user: String },
    #[serde(rename = "user.lock")]
    UserLock { user: String },
    #[serde(rename = "user.unlock")]
    UserUnlock { user: String },
    #[serde(rename = "grant")]
    Grant { user: String, database: String },
    #[serde(rename = "revoke")]
    Revoke { user: String, database: String },
    #[serde(rename = "dump")]
    Dump { database: String, dest: Dest },
}

pub(super) fn exec(call: &Call<'_>, statement: Statement) -> Result<Value, Failure> {
    let prefix = call.user_prefix()?;
    match statement {
        Statement::DatabaseCreate { database } => run(
            call,
            &format!(
                "CREATE DATABASE {} CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;",
                database_of(&database, prefix)?
            ),
            None,
        ),
        Statement::DatabaseDrop { database } => run(
            call,
            &format!(
                "DROP DATABASE IF EXISTS {};",
                database_of(&database, prefix)?
            ),
            None,
        ),
        Statement::UserCreate { user, password } => run(
            call,
            &format!(
                "CREATE USER {} IDENTIFIED BY {};",
                account_of(&user, prefix)?,
                password_of(&password)?
            ),
            Some(&password),
        ),
        Statement::UserPassword { user, password } => run(
            call,
            &format!(
                "ALTER USER {} IDENTIFIED BY {};",
                account_of(&user, prefix)?,
                password_of(&password)?
            ),
            Some(&password),
        ),
        Statement::UserDrop { user } => run(
            call,
            &format!("DROP USER IF EXISTS {};", account_of(&user, prefix)?),
            None,
        ),
        Statement::UserLock { user } => run(
            call,
            &format!("ALTER USER {} ACCOUNT LOCK;", account_of(&user, prefix)?),
            None,
        ),
        Statement::UserUnlock { user } => run(
            call,
            &format!("ALTER USER {} ACCOUNT UNLOCK;", account_of(&user, prefix)?),
            None,
        ),
        Statement::Grant { user, database } => run(
            call,
            &format!(
                "GRANT ALL PRIVILEGES ON {}.* TO {};",
                grant_scope(&database, prefix)?,
                account_of(&user, prefix)?
            ),
            None,
        ),
        Statement::Revoke { user, database } => {
            let database = grant_scope(&database, prefix)?;
            let account = account_of(&user, prefix)?;
            run(
                call,
                &format!(
                    "GRANT SELECT ON {database}.* TO {account};\nREVOKE ALL PRIVILEGES ON {database}.* FROM {account};"
                ),
                None,
            )
        }
        Statement::Dump { database, dest } => dump(call, name_of(&database, prefix)?, &dest),
    }
}

fn run(call: &Call<'_>, statement: &str, secret: Option<&str>) -> Result<Value, Failure> {
    let (program, args) = entry(call, CLIENT)?;
    let script = format!("{SESSION}\n{statement}\n");
    process::feed(program, args, script.as_bytes()).map_err(|failure| redact(failure, secret))?;
    Ok(json!({}))
}

fn dump(call: &Call<'_>, database: &str, dest: &Dest) -> Result<Value, Failure> {
    let (program, base) = entry(call, DUMP)?;
    let target = call.resolve(dest)?;
    let temporary = target.with_file_name(format!(".{}.tmp", dest.name));
    if let Err(error) = fs::remove_file(&temporary)
        && error.kind() != io::ErrorKind::NotFound
    {
        return Err(Failure::io(temporary.display(), &error));
    }
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(DUMP_MODE)
        .custom_flags(OFlag::O_NOFOLLOW.bits())
        .open(&temporary)
        .map_err(|error| Failure::io(temporary.display(), &error))?;
    let mut args = base.to_vec();
    args.push(database.to_owned());
    let written = fchown(&file, Some(call.admitted), None)
        .map_err(|error| Failure::io(temporary.display(), &error))
        .and_then(|()| process::into_file(program, &args, file))
        .and_then(|_| {
            fs::rename(&temporary, &target).map_err(|error| Failure::io(target.display(), &error))
        });
    if let Err(failure) = written {
        let _ = fs::remove_file(&temporary);
        return Err(failure);
    }
    let bytes = fs::symlink_metadata(&target)
        .map_err(|error| Failure::io(target.display(), &error))?
        .len();
    Ok(json!({ "bytes": bytes }))
}

fn entry<'a>(call: &'a Call<'_>, name: &str) -> Result<(&'a Path, &'a [String]), Failure> {
    let argv = call
        .policy
        .exec(name)
        .ok_or_else(|| Failure::policy(format!("{name} is not an entry of exec_allow")))?;
    let (program, rest) = argv
        .split_first()
        .ok_or_else(|| Failure::policy(format!("the entry {name} has no binary")))?;
    Ok((Path::new(program), rest))
}

fn name_of<'a>(value: &'a str, prefix: &str) -> Result<&'a str, Failure> {
    if valid_name(value, prefix) {
        Ok(value)
    } else {
        Err(Failure::policy(format!(
            "the name {value} is not a database or database user name under the prefix {prefix}"
        )))
    }
}

fn valid_name(value: &str, prefix: &str) -> bool {
    value.len() > prefix.len()
        && value.len() <= NAME_LIMIT
        && value.starts_with(prefix)
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

fn database_of(value: &str, prefix: &str) -> Result<String, Failure> {
    Ok(identifier(name_of(value, prefix)?))
}

fn grant_scope(value: &str, prefix: &str) -> Result<String, Failure> {
    Ok(identifier(&name_of(value, prefix)?.replace('_', "\\_")))
}

fn account_of(value: &str, prefix: &str) -> Result<String, Failure> {
    Ok(format!(
        "{}@{}",
        literal(name_of(value, prefix)?),
        literal(HOST)
    ))
}

fn password_of(value: &str) -> Result<String, Failure> {
    if value.is_empty() || value.len() > PASSWORD_LIMIT || value.chars().any(char::is_control) {
        return Err(Failure::args(format!(
            "a password is 1 to {PASSWORD_LIMIT} bytes without control characters"
        )));
    }
    Ok(literal(value))
}

fn identifier(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}

fn literal(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for c in value.chars() {
        match c {
            '\\' => quoted.push_str("\\\\"),
            '\'' => quoted.push_str("\\'"),
            '"' => quoted.push_str("\\\""),
            '\0' => quoted.push_str("\\0"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\u{1a}' => quoted.push_str("\\Z"),
            other => quoted.push(other),
        }
    }
    quoted.push('\'');
    quoted
}

fn redact(failure: Failure, secret: Option<&str>) -> Failure {
    let Some(secret) = secret else {
        return failure;
    };
    let detail = failure
        .detail
        .lines()
        .map(|line| line.split_once(" near ").map_or(line, |(head, _)| head))
        .collect::<Vec<_>>()
        .join("\n")
        .replace(&literal(secret), REDACTED)
        .replace(secret, REDACTED);
    Failure::new(&failure.key, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_carry_the_prefix_and_only_the_characters_that_need_no_quoting() {
        assert!(valid_name("hp_alice_wp", "hp_"));
        assert!(valid_name("hp_a-1_x", "hp_"));
        assert!(valid_name(&format!("hp_{}", "a".repeat(61)), "hp_"));
        assert!(!valid_name(&format!("hp_{}", "a".repeat(62)), "hp_"));
        assert!(!valid_name("hp_", "hp_"));
        assert!(!valid_name("mysql", "hp_"));
        assert!(!valid_name("hp_A", "hp_"));
        assert!(!valid_name("hp_a`b", "hp_"));
        assert!(!valid_name("hp_a'b", "hp_"));
        assert!(!valid_name("hp_a b", "hp_"));
        assert!(!valid_name("hp_a.b", "hp_"));
        assert!(!valid_name("hp_a;b", "hp_"));
    }

    #[test]
    fn identifiers_are_quoted_in_backticks_and_values_as_escaped_literals() {
        assert_eq!(identifier("hp_a"), "`hp_a`");
        assert_eq!(identifier("a`b"), "`a``b`");
        assert_eq!(literal("plain"), "'plain'");
        assert_eq!(
            literal("it's \"a\" \\ ;\n\r\0\u{1a}"),
            "'it\\'s \\\"a\\\" \\\\ ;\\n\\r\\0\\Z'"
        );
        assert_eq!(
            account_of("hp_alice_app", "hp_").unwrap(),
            "'hp_alice_app'@'localhost'"
        );
        assert_eq!(database_of("hp_alice_wp", "hp_").unwrap(), "`hp_alice_wp`");
        assert!(database_of("mysql", "hp_").is_err());
        assert_eq!(grant_scope("hp_a_b_c", "hp_").unwrap(), "`hp\\_a\\_b\\_c`");
        assert!(grant_scope("mysql", "hp_").is_err());
        assert!(account_of("root", "hp_").is_err());
    }

    #[test]
    fn a_password_has_no_control_characters_and_a_bounded_length() {
        assert_eq!(password_of("s3cr'et").unwrap(), "'s3cr\\'et'");
        assert!(password_of("").is_err());
        assert!(password_of("line\nbreak").is_err());
        assert!(password_of(&"a".repeat(PASSWORD_LIMIT + 1)).is_err());
    }

    #[test]
    fn a_failure_of_a_statement_with_a_password_never_carries_it() {
        let failure = Failure::new(
            "host.error.exec_failed",
            "ERROR 1064 (42000) at line 2: You have an error near 'IDENTIFIED BY 'it\\'s'' at line 1\nit's again",
        );
        let redacted = redact(failure, Some("it's"));
        assert_eq!(
            redacted.detail,
            "ERROR 1064 (42000) at line 2: You have an error\n*** again"
        );
        let kept = redact(Failure::new("host.error.exec_failed", "near 'x'"), None);
        assert_eq!(kept.detail, "near 'x'");
    }
}
