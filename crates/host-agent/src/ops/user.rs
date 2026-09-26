use std::collections::BTreeSet;
use std::fs;
use std::io;

use serde::Deserialize;
use serde_json::{Value, json};

use super::{Call, Dest};
use crate::accounts::User;
use crate::policy::UidRange;
use crate::process;
use crate::protocol::{ERROR_EXEC_FAILED, ERROR_NOT_FOUND, Failure};

const DEFAULT_SHELL: &str = "/usr/sbin/nologin";
const NO_HOME: &str = "/nonexistent";
const EXPIRED_SINCE_EPOCH_DAY: &str = "1";
const NEVER_EXPIRES: &str = "";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Create {
    name: String,
    #[serde(default)]
    shell: Option<String>,
    #[serde(default)]
    home: Option<Dest>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Delete {
    name: String,
    #[serde(default)]
    remove_home: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Modify {
    name: String,
    #[serde(default)]
    shell: Option<String>,
    #[serde(default)]
    locked: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Get {
    name: String,
}

pub(super) fn create(call: &Call<'_>, args: Create) -> Result<Value, Failure> {
    call.account_name(&args.name)?;
    let range = call.uid_range()?;
    if let Some(shell) = &args.shell {
        allowed_shell(call, shell)?;
    }
    let home = match &args.home {
        Some(dest) => Some(call.resolve(dest)?),
        None => None,
    };
    let accounts = call.accounts();
    if let Some(user) = accounts.user(&args.name)? {
        inside(range, user.uid, &args.name)?;
        return Ok(describe(&user, false));
    }
    let taken: BTreeSet<u32> = accounts
        .users()?
        .iter()
        .map(|user| user.uid)
        .chain(accounts.groups()?.iter().map(|group| group.gid))
        .collect();
    let id = free_id(range, &taken).ok_or_else(|| {
        Failure::policy(format!(
            "no free id is left in the uid_range {}..{}",
            range.from, range.to
        ))
    })?;
    let id_text = id.to_string();
    let tools = call.policy.tools();
    let (gid, group_created) = match accounts.group(&args.name)? {
        Some(group) => {
            inside(range, group.gid, &args.name)?;
            (group.gid, false)
        }
        None => {
            process::succeed(&tools.groupadd, ["--gid", &id_text, &args.name])?;
            (id, true)
        }
    };
    let gid_text = gid.to_string();
    let shell = args.shell.as_deref().unwrap_or(DEFAULT_SHELL);
    let mut argv: Vec<&str> = vec!["--uid", &id_text, "--gid", &gid_text, "--shell", shell];
    let home_text = home.as_ref().map(|path| path.display().to_string());
    match &home_text {
        Some(path) => argv.extend(["--home-dir", path, "--create-home"]),
        None => argv.extend(["--home-dir", NO_HOME, "--no-create-home"]),
    }
    argv.push(&args.name);
    if let Err(failure) = process::succeed(&tools.useradd, &argv) {
        if group_created {
            let _ = process::run(&tools.groupdel, [args.name.as_str()]);
        }
        return Err(failure);
    }
    match accounts.user(&args.name)? {
        Some(user) => Ok(describe(&user, true)),
        None => Err(Failure::new(
            ERROR_EXEC_FAILED,
            format!(
                "{} exited 0 but {} is not an account afterwards",
                tools.useradd.display(),
                args.name
            ),
        )),
    }
}

fn allowed_shell(call: &Call<'_>, shell: &str) -> Result<(), Failure> {
    if shell == DEFAULT_SHELL {
        return Ok(());
    }
    let shells = &call.policy.tools().shells;
    let refused = || {
        Failure::policy(format!(
            "the shell {shell} is neither {DEFAULT_SHELL} nor a shell listed in {}",
            shells.display()
        ))
    };
    if !shell.starts_with('/')
        || !shell
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '+' | '-'))
    {
        return Err(refused());
    }
    let listed = match fs::read_to_string(shells) {
        Ok(listed) => listed,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(Failure::io(shells.display(), &error)),
    };
    if listed.lines().any(|line| line.trim() == shell) {
        Ok(())
    } else {
        Err(refused())
    }
}

pub(super) fn delete(call: &Call<'_>, args: Delete) -> Result<Value, Failure> {
    call.account_name(&args.name)?;
    let range = call.uid_range()?;
    let accounts = call.accounts();
    let Some(user) = accounts.user(&args.name)? else {
        return Ok(json!({ "removed": false }));
    };
    inside(range, user.uid, &args.name)?;
    let tools = call.policy.tools();
    let mut argv = Vec::new();
    if args.remove_home {
        argv.push("--remove");
    }
    argv.push(&args.name);
    process::succeed(&tools.userdel, &argv)?;
    if let Some(group) = accounts.group(&args.name)?
        && range.contains(group.gid)
        && group.members.is_empty()
    {
        process::succeed(&tools.groupdel, [args.name.as_str()])?;
    }
    Ok(json!({ "removed": true }))
}

pub(super) fn modify(call: &Call<'_>, args: Modify) -> Result<Value, Failure> {
    call.account_name(&args.name)?;
    let range = call.uid_range()?;
    if args.shell.is_none() && args.locked.is_none() {
        return Err(Failure::args(format!(
            "a change of {} names a shell, a lock or both",
            args.name
        )));
    }
    if let Some(shell) = &args.shell {
        allowed_shell(call, shell)?;
    }
    let accounts = call.accounts();
    let user = accounts
        .user(&args.name)?
        .ok_or_else(|| absent(&args.name))?;
    inside(range, user.uid, &args.name)?;
    let mut argv: Vec<&str> = Vec::new();
    if let Some(shell) = &args.shell {
        argv.extend(["--shell", shell.as_str()]);
    }
    match args.locked {
        Some(true) => argv.extend(["--lock", "--expiredate", EXPIRED_SINCE_EPOCH_DAY]),
        Some(false) => argv.extend(["--unlock", "--expiredate", NEVER_EXPIRES]),
        None => {}
    }
    argv.push(&args.name);
    process::succeed(&call.policy.tools().usermod, &argv)?;
    let user = accounts
        .user(&args.name)?
        .ok_or_else(|| absent(&args.name))?;
    let mut value = describe_fields(&user);
    if let Some(locked) = args.locked {
        value["locked"] = json!(locked);
    }
    Ok(value)
}

fn absent(name: &str) -> Failure {
    Failure::new(
        ERROR_NOT_FOUND,
        format!("{name} is not an account of this host"),
    )
}

pub(super) fn get(call: &Call<'_>, args: Get) -> Result<Value, Failure> {
    call.account_name(&args.name)?;
    let range = call.uid_range()?;
    let user = call
        .accounts()
        .user(&args.name)?
        .ok_or_else(|| absent(&args.name))?;
    inside(range, user.uid, &args.name)?;
    Ok(describe_fields(&user))
}

pub(super) fn list(call: &Call<'_>, (): ()) -> Result<Value, Failure> {
    let prefix = call.user_prefix()?;
    let range = call.uid_range()?;
    let users: Vec<Value> = call
        .accounts()
        .users()?
        .iter()
        .filter(|user| user.name.starts_with(prefix) && range.contains(user.uid))
        .map(describe_fields)
        .collect();
    Ok(json!({ "users": users }))
}

fn describe(user: &User, created: bool) -> Value {
    let mut value = describe_fields(user);
    value["created"] = json!(created);
    value
}

fn describe_fields(user: &User) -> Value {
    json!({
        "name": user.name,
        "uid": user.uid,
        "gid": user.gid,
        "home": user.home,
        "shell": user.shell,
    })
}

pub(super) fn inside(range: UidRange, id: u32, name: &str) -> Result<(), Failure> {
    if range.contains(id) {
        Ok(())
    } else {
        Err(Failure::policy(format!(
            "{name} has the id {id}, outside the uid_range {}..{} of this installation",
            range.from, range.to
        )))
    }
}

pub(super) fn free_id(range: UidRange, taken: &BTreeSet<u32>) -> Option<u32> {
    (range.from..=range.to).find(|id| !taken.contains(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lowest_id_free_as_both_uid_and_gid_is_chosen_inside_the_range() {
        let range = UidRange {
            from: 100_000,
            to: 100_003,
        };
        assert_eq!(free_id(range, &BTreeSet::new()), Some(100_000));
        let taken = [100_000, 100_001, 100_003].into_iter().collect();
        assert_eq!(free_id(range, &taken), Some(100_002));
        let full = (100_000..=100_003).collect();
        assert_eq!(free_id(range, &full), None);
        assert!(inside(range, 100_003, "hp_a").is_ok());
        let failure = inside(range, 1000, "hp_a").unwrap_err();
        assert_eq!(failure.key, crate::protocol::ERROR_POLICY);
        assert!(failure.detail.contains("1000"), "{}", failure.detail);
    }
}
