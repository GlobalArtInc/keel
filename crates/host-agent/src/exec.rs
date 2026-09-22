use std::ffi::OsString;
use std::io;
use std::path::PathBuf;

use crate::accounts::Accounts;
use crate::policy::Policy;

#[derive(Debug)]
pub struct Target {
    pub uid: u32,
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    #[error("the policy allots no uid_range to this installation")]
    NoRange,
    #[error("the uid {uid} is outside the uid_range {from}..{to} of this installation")]
    OutsideRange { uid: u32, from: u32, to: u32 },
    #[error("could not look up the uid {uid}: {detail}")]
    Lookup { uid: u32, detail: String },
    #[error("the uid {0} is not an account of this host")]
    Unknown(u32),
    #[error("could not execute {program}: {source}")]
    Exec {
        program: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("running a program under another uid is a Linux operation; this build refuses it")]
    Unsupported,
}

pub fn run(policy: &Policy, target: &Target) -> ExecError {
    let range = match policy.uid_range() {
        Some(range) => range,
        None => return ExecError::NoRange,
    };
    if !range.contains(target.uid) {
        return ExecError::OutsideRange {
            uid: target.uid,
            from: range.from,
            to: range.to,
        };
    }
    let found = match Accounts::new(&policy.tools().getent).user(&target.uid.to_string()) {
        Ok(found) => found,
        Err(failure) => {
            return ExecError::Lookup {
                uid: target.uid,
                detail: failure.detail,
            };
        }
    };
    let account = match found {
        Some(account) => account,
        None => return ExecError::Unknown(target.uid),
    };
    become_and_exec(target, account.gid)
}

#[cfg(target_os = "linux")]
fn become_and_exec(target: &Target, gid: u32) -> ExecError {
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    use nix::unistd::{Gid, Uid, setgroups, setresgid, setresuid};

    let uid = Uid::from_raw(target.uid);
    let gid = Gid::from_raw(gid);
    let mut command = Command::new(&target.program);
    command.args(&target.args);
    let source = unsafe {
        command.pre_exec(move || {
            setgroups(&[gid]).map_err(|errno| io::Error::other(format!("setgroups: {errno}")))?;
            setresgid(gid, gid, gid)
                .map_err(|errno| io::Error::other(format!("setresgid: {errno}")))?;
            setresuid(uid, uid, uid)
                .map_err(|errno| io::Error::other(format!("setresuid: {errno}")))?;
            Ok(())
        })
    }
    .exec();
    ExecError::Exec {
        program: target.program.clone(),
        source,
    }
}

#[cfg(not(target_os = "linux"))]
fn become_and_exec(_target: &Target, _gid: u32) -> ExecError {
    ExecError::Unsupported
}
