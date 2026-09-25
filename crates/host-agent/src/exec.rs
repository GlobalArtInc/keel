use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
#[cfg(any(target_os = "linux", test))]
use std::process::Command;

use crate::accounts::{Accounts, User};
use crate::policy::Policy;
#[cfg(any(target_os = "linux", test))]
use crate::process;

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
    #[error("the lookup of the uid {uid} answered the account {name} with the uid {found}")]
    Mismatch { uid: u32, name: String, found: u32 },
    #[error(
        "the account of the uid {uid} has the group {gid}, outside the uid_range {from}..{to} of this installation"
    )]
    GroupOutsideRange {
        uid: u32,
        gid: u32,
        from: u32,
        to: u32,
    },
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
    match account(policy, target) {
        Ok(account) => become_and_exec(target, &account),
        Err(error) => error,
    }
}

fn account(policy: &Policy, target: &Target) -> Result<User, ExecError> {
    let range = policy.uid_range().ok_or(ExecError::NoRange)?;
    if !range.contains(target.uid) {
        return Err(ExecError::OutsideRange {
            uid: target.uid,
            from: range.from,
            to: range.to,
        });
    }
    let account = Accounts::new(&policy.tools().getent)
        .user(&target.uid.to_string())
        .map_err(|failure| ExecError::Lookup {
            uid: target.uid,
            detail: failure.detail,
        })?
        .ok_or(ExecError::Unknown(target.uid))?;
    if account.uid != target.uid {
        return Err(ExecError::Mismatch {
            uid: target.uid,
            name: account.name,
            found: account.uid,
        });
    }
    if !range.contains(account.gid) {
        return Err(ExecError::GroupOutsideRange {
            uid: target.uid,
            gid: account.gid,
            from: range.from,
            to: range.to,
        });
    }
    Ok(account)
}

#[cfg(any(target_os = "linux", test))]
fn command(target: &Target, account: &User) -> Command {
    let mut command = Command::new(&target.program);
    command
        .args(&target.args)
        .env_clear()
        .env("PATH", process::PATH)
        .env("LC_ALL", "C")
        .env("HOME", &account.home)
        .env("USER", &account.name)
        .env("LOGNAME", &account.name)
        .current_dir("/");
    command
}

#[cfg(target_os = "linux")]
fn become_and_exec(target: &Target, account: &User) -> ExecError {
    use std::os::unix::process::CommandExt;

    use nix::unistd::{Gid, Uid, setgroups, setresgid, setresuid};

    let uid = Uid::from_raw(account.uid);
    let gid = Gid::from_raw(account.gid);
    let mut command = command(target, account);
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
fn become_and_exec(_target: &Target, _account: &User) -> ExecError {
    ExecError::Unsupported
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use super::*;

    fn stub(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn policy_answering(dir: &Path, passwd: &str) -> Policy {
        let getent = stub(dir, "getent", &format!("printf '%s\\n' '{passwd}'\n"));
        Policy::parse(&format!(
            "policy:\n  uid_range: {{ from: 100000, to: 100009 }}\n  tools: {{ getent: {} }}\n",
            getent.display()
        ))
        .unwrap()
    }

    fn target(uid: u32) -> Target {
        Target {
            uid,
            program: PathBuf::from("/bin/true"),
            args: Vec::new(),
        }
    }

    #[test]
    fn a_uid_whose_account_carries_a_group_outside_the_range_is_refused_before_the_exec() {
        let dir = tempfile::tempdir().unwrap();
        let policy = policy_answering(dir.path(), "hp_a:x:100001:0::/home/hp_a:/bin/sh");
        let error = run(&policy, &target(100_001));
        assert!(
            matches!(
                error,
                ExecError::GroupOutsideRange {
                    uid: 100_001,
                    gid: 0,
                    from: 100_000,
                    to: 100_009
                }
            ),
            "{error}"
        );
        let policy = policy_answering(dir.path(), "hp_a:x:100001:100001::/home/hp_a:/bin/sh");
        let account = account(&policy, &target(100_001)).unwrap();
        assert_eq!((account.uid, account.gid), (100_001, 100_001));
    }

    #[test]
    fn a_lookup_answering_another_account_than_the_uid_asked_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let policy = policy_answering(dir.path(), "root:x:0:0:root:/root:/bin/sh");
        let error = run(&policy, &target(100_002));
        assert!(
            matches!(&error, ExecError::Mismatch { uid: 100_002, name, found: 0 } if name == "root"),
            "{error}"
        );
        let error = run(&policy, &target(99_999));
        assert!(
            matches!(error, ExecError::OutsideRange { uid: 99_999, .. }),
            "{error}"
        );
    }

    #[test]
    fn the_program_run_under_another_uid_gets_a_clean_environment_and_the_root_directory() {
        let dir = tempfile::tempdir().unwrap();
        let program = stub(
            dir.path(),
            "show",
            "printf '%s|%s|%s|%s|%s|%s|%s' \"$PATH\" \"$LC_ALL\" \"$HOME\" \"$USER\" \"$LOGNAME\" \"${CARGO_MANIFEST_DIR:-unset}\" \"$(pwd)\"\n",
        );
        let account = User {
            name: "hp_a".to_owned(),
            uid: 100_001,
            gid: 100_001,
            home: "/var/www/keel-panel-a".to_owned(),
            shell: "/bin/sh".to_owned(),
        };
        let target = Target {
            uid: account.uid,
            program,
            args: Vec::new(),
        };
        let output = command(&target, &account).output().unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!(
                "{}|C|/var/www/keel-panel-a|hp_a|hp_a|unset|/",
                process::PATH
            )
        );
    }
}
