use std::collections::BTreeSet;

use serde::Deserialize;
use serde_json::{Value, json};

use super::Call;
use super::user::{free_id, inside};
use crate::accounts::Group;
use crate::policy::tool;
use crate::process;
use crate::protocol::{ERROR_EXEC_FAILED, ERROR_NOT_FOUND, Failure};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Named {
    name: String,
}

pub(super) fn create(call: &Call<'_>, args: Named) -> Result<Value, Failure> {
    call.account_name(&args.name)?;
    let range = call.uid_range()?;
    let accounts = call.accounts();
    if let Some(group) = accounts.group(&args.name)? {
        inside(range, group.gid, &args.name)?;
        return Ok(describe(&group, false));
    }
    let taken: BTreeSet<u32> = accounts
        .groups()?
        .iter()
        .map(|group| group.gid)
        .chain(accounts.users()?.iter().map(|user| user.uid))
        .collect();
    let id = free_id(range, &taken).ok_or_else(|| {
        Failure::policy(format!(
            "no free id is left in the uid_range {}..{}",
            range.from, range.to
        ))
    })?;
    let tools = call.policy.tools();
    process::succeed(
        &tools.groupadd,
        ["--gid", &id.to_string(), &args.name],
        call.policy.timeout(tool::GROUPADD),
    )?;
    match accounts.group(&args.name)? {
        Some(group) => Ok(describe(&group, true)),
        None => Err(Failure::new(
            ERROR_EXEC_FAILED,
            format!(
                "{} exited 0 but {} is not a group afterwards",
                tools.groupadd.display(),
                args.name
            ),
        )),
    }
}

pub(super) fn delete(call: &Call<'_>, args: Named) -> Result<Value, Failure> {
    call.account_name(&args.name)?;
    let range = call.uid_range()?;
    let Some(group) = call.accounts().group(&args.name)? else {
        return Ok(json!({ "removed": false }));
    };
    inside(range, group.gid, &args.name)?;
    process::succeed(
        &call.policy.tools().groupdel,
        [args.name.as_str()],
        call.policy.timeout(tool::GROUPDEL),
    )?;
    Ok(json!({ "removed": true }))
}

pub(super) fn get(call: &Call<'_>, args: Named) -> Result<Value, Failure> {
    call.account_name(&args.name)?;
    let range = call.uid_range()?;
    let group = call.accounts().group(&args.name)?.ok_or_else(|| {
        Failure::new(
            ERROR_NOT_FOUND,
            format!("{} is not a group of this host", args.name),
        )
    })?;
    inside(range, group.gid, &args.name)?;
    Ok(describe_fields(&group))
}

pub(super) fn list(call: &Call<'_>, (): ()) -> Result<Value, Failure> {
    let prefix = call.user_prefix()?;
    let range = call.uid_range()?;
    let groups: Vec<Value> = call
        .accounts()
        .groups()?
        .iter()
        .filter(|group| group.name.starts_with(prefix) && range.contains(group.gid))
        .map(describe_fields)
        .collect();
    Ok(json!({ "groups": groups }))
}

fn describe(group: &Group, created: bool) -> Value {
    let mut value = describe_fields(group);
    value["created"] = json!(created);
    value
}

fn describe_fields(group: &Group) -> Value {
    json!({
        "name": group.name,
        "gid": group.gid,
        "members": group.members,
    })
}
