use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};

use super::Call;
use super::user::inside;
use crate::process;
use crate::protocol::Failure;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Set {
    user: String,
    root: String,
    blocks: Limits,
    inodes: Limits,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Limits {
    soft: u64,
    hard: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct List {
    root: String,
}

pub(super) fn set(call: &Call<'_>, args: Set) -> Result<Value, Failure> {
    call.account_name(&args.user)?;
    let range = call.uid_range()?;
    let user = call
        .accounts()
        .user(&args.user)?
        .filter(|user| user.name == args.user)
        .ok_or_else(|| Failure::policy(format!("{} is not a user of this host", args.user)))?;
    inside(range, user.uid, &args.user)?;
    for limits in [&args.blocks, &args.inodes] {
        if limits.hard != 0 && limits.soft > limits.hard {
            return Err(Failure::args("a soft limit is above its hard limit"));
        }
    }
    let filesystem = filesystem(call, &args.root)?;
    let setquota = &call.policy.tools().setquota;
    let argv = [
        "-u".to_owned(),
        args.user.clone(),
        args.blocks.soft.to_string(),
        args.blocks.hard.to_string(),
        args.inodes.soft.to_string(),
        args.inodes.hard.to_string(),
        filesystem.display().to_string(),
    ];
    process::checked(setquota, process::tool(setquota, argv, &[])?)?;
    Ok(json!({ "user": args.user, "filesystem": filesystem.display().to_string() }))
}

pub(super) fn list(call: &Call<'_>, args: List) -> Result<Value, Failure> {
    let prefix = call.user_prefix()?;
    let filesystem = filesystem(call, &args.root)?;
    let repquota = &call.policy.tools().repquota;
    let output = process::checked(
        repquota,
        process::tool(
            repquota,
            [OsStr::new("-u"), OsStr::new("-p"), filesystem.as_os_str()],
            &[],
        )?,
    )?;
    let quotas: Vec<Value> = process::stdout(&output)
        .lines()
        .filter_map(row)
        .filter(|quota| {
            quota["user"]
                .as_str()
                .is_some_and(|user| user.starts_with(prefix))
        })
        .collect();
    Ok(json!({ "filesystem": filesystem.display().to_string(), "quotas": quotas }))
}

fn filesystem(call: &Call<'_>, root: &str) -> Result<PathBuf, Failure> {
    let dir = call.root_dir(root)?;
    mount_point(&dir).map_err(|error| Failure::io(dir.display(), &error))
}

fn mount_point(dir: &Path) -> std::io::Result<PathBuf> {
    let device = fs::metadata(dir)?.dev();
    let mut current = dir.to_path_buf();
    while let Some(parent) = current.parent() {
        if fs::metadata(parent)?.dev() != device {
            break;
        }
        current = parent.to_path_buf();
    }
    Ok(current)
}

fn row(line: &str) -> Option<Value> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    let [user, flags, numbers @ ..] = fields.as_slice() else {
        return None;
    };
    if numbers.len() != 8 || flags.len() != 2 || !flags.chars().all(|c| c == '-' || c == '+') {
        return None;
    }
    let numbers: Vec<u64> = numbers
        .iter()
        .map(|number| number.parse().ok())
        .collect::<Option<_>>()?;
    let usage = |at: usize| {
        json!({
            "used": numbers[at],
            "soft": numbers[at + 1],
            "hard": numbers[at + 2],
            "grace": numbers[at + 3],
        })
    };
    Some(json!({ "user": user, "blocks": usage(0), "inodes": usage(4) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPORT: &str = "*** Report for user quotas on device /dev/vda1\nBlock grace time: 7days; Inode grace time: 7days\n                        Block limits                File limits\nUser            used    soft    hard  grace    used  soft  hard  grace\n----------------------------------------------------------------------\nroot      --  1843212       0       0      0   61234     0     0      0\nhp_site   +-     2100    1000    4000 604800      12     0     0      0\n";

    #[test]
    fn a_report_row_carries_the_usage_limits_and_grace_of_a_user() {
        let rows: Vec<Value> = REPORT.lines().filter_map(row).collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[1],
            json!({
                "user": "hp_site",
                "blocks": { "used": 2100, "soft": 1000, "hard": 4000, "grace": 604800 },
                "inodes": { "used": 12, "soft": 0, "hard": 0, "grace": 0 },
            })
        );
    }

    #[test]
    fn the_mount_point_is_the_topmost_directory_on_the_same_device() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a/b");
        fs::create_dir_all(&nested).unwrap();
        let nested = nested.canonicalize().unwrap();
        let found = mount_point(&nested).unwrap();
        assert!(nested.starts_with(&found), "{}", found.display());
        let device = fs::metadata(&nested).unwrap().dev();
        assert_eq!(fs::metadata(&found).unwrap().dev(), device);
        if let Some(parent) = found.parent() {
            assert_ne!(fs::metadata(parent).unwrap().dev(), device);
        }
    }
}
