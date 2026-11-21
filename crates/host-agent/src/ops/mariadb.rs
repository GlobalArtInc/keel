use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use nix::fcntl::OFlag;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Call, Dest, seal};
use crate::process;
use crate::protocol::Failure;

const CLIENT: &str = "mariadb";
const DUMP: &str = "mariadb-dump";
const DUMPS_ROOT: &str = "dumps";
const SANDBOX: &str = "--sandbox";
const CHARSET: &str = "--default-character-set=utf8mb4";
const SKIP_COMMENTS: &str = "--skip-comments";
const HOST: &str = "localhost";
const NAME_LIMIT: usize = 64;
const PASSWORD_LIMIT: usize = 256;
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
    #[serde(rename = "import")]
    Import { database: String, source: Dest },
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
        Statement::Import { database, source } => {
            import(call, name_of(&database, prefix)?, &source)
        }
    }
}

fn run(call: &Call<'_>, statement: &str, secret: Option<&str>) -> Result<Value, Failure> {
    let (program, args) = client(call)?;
    let script = format!("{SESSION}\n{statement}\n");
    process::feed(
        program,
        &args,
        script.as_bytes(),
        call.policy.timeout(CLIENT),
    )
    .map_err(|failure| redact(failure, secret))?;
    Ok(json!({}))
}

fn dump(call: &Call<'_>, database: &str, dest: &Dest) -> Result<Value, Failure> {
    let (program, base) = entry(call, DUMP)?;
    let mut args = pinned(base, &[SKIP_COMMENTS, CHARSET]);
    args.push(database.to_owned());
    let bytes = super::exec::capture(call, dest, program, &args, true, call.policy.timeout(DUMP))?;
    Ok(json!({ "bytes": bytes }))
}

fn import(call: &Call<'_>, database: &str, source: &Dest) -> Result<Value, Failure> {
    if source.root != DUMPS_ROOT {
        return Err(Failure::policy(format!(
            "an import reads a dump of the root {DUMPS_ROOT}, not of {}",
            source.root
        )));
    }
    let (program, mut args) = client(call)?;
    let path = call.resolve(source)?;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(OFlag::O_NOFOLLOW.bits())
        .open(&path)
        .map_err(|error| Failure::io(path.display(), &error))?;
    seal::verify(&path, &mut file)?;
    args.push(database.to_owned());
    process::from_file(program, &args, file, call.policy.timeout(CLIENT))?;
    Ok(json!({}))
}

fn client<'a>(call: &'a Call<'_>) -> Result<(&'a Path, Vec<String>), Failure> {
    let (program, base) = entry(call, CLIENT)?;
    Ok((program, pinned(base, &[SANDBOX, CHARSET])))
}

fn pinned(base: &[String], flags: &[&str]) -> Vec<String> {
    let mut args = base.to_vec();
    for flag in flags {
        if !args.iter().any(|arg| arg == flag) {
            args.push((*flag).to_owned());
        }
    }
    args
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
mod fuzz;

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
    fn the_client_reads_every_statement_as_utf8mb4_and_a_dump_carries_no_comment() {
        let policy = |args: &[&str]| args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            pinned(&policy(&["--batch"]), &[SANDBOX, CHARSET]),
            ["--batch", SANDBOX, CHARSET]
        );
        assert_eq!(
            pinned(
                &policy(&["--default-character-set=gbk", SANDBOX]),
                &[SANDBOX, CHARSET]
            ),
            ["--default-character-set=gbk", SANDBOX, CHARSET]
        );
        assert_eq!(
            pinned(
                &policy(&["--single-transaction", CHARSET]),
                &[SKIP_COMMENTS, CHARSET]
            ),
            ["--single-transaction", CHARSET, SKIP_COMMENTS]
        );
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
