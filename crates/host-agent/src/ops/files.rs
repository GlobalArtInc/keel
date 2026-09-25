use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt, fchown};
use std::path::Path;

use nix::errno::Errno;
use nix::fcntl::OFlag;

use super::{Call, DIR_MODE_LIMIT, FILE_MODE_LIMIT, parse_mode};
use crate::protocol::Failure;

const READ_LIMIT: u64 = 8 * 1024 * 1024;
const OTHER_WRITE: u32 = 0o002;
const STICKY: u32 = 0o1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    File,
    Dir,
}

impl Kind {
    fn default_mode(self) -> u32 {
        match self {
            Self::File => 0o644,
            Self::Dir => 0o755,
        }
    }

    fn mode_limit(self) -> u32 {
        match self {
            Self::File => FILE_MODE_LIMIT,
            Self::Dir => DIR_MODE_LIMIT,
        }
    }

    fn admits(self, mode: u32) -> bool {
        match self {
            Self::File => mode & OTHER_WRITE == 0,
            Self::Dir => mode & OTHER_WRITE == 0 || mode & STICKY != 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Ownership {
    pub(super) uid: u32,
    pub(super) gid: u32,
}

#[derive(Debug)]
pub(super) struct Previous {
    pub(super) content: Vec<u8>,
    pub(super) mode: u32,
    pub(super) owner: Ownership,
}

#[derive(Debug)]
pub(super) struct Placement {
    pub(super) mode: u32,
    pub(super) owner: Option<Ownership>,
}

impl Placement {
    pub(super) fn resolve(
        call: &Call<'_>,
        mode: Option<&str>,
        owner: Option<&str>,
        group: Option<&str>,
        kind: Kind,
    ) -> Result<Self, Failure> {
        let mode = match mode {
            Some(text) => parse_mode(text, kind.mode_limit()).ok_or_else(|| {
                Failure::args(format!(
                    "the mode {text} is not octal within {:04o}",
                    kind.mode_limit()
                ))
            })?,
            None => kind.default_mode(),
        };
        if !kind.admits(mode) {
            return Err(Failure::policy(format!(
                "the mode {mode:04o} lets every account of the host write what the agent places"
            )));
        }
        let owner = ownership(call, owner, group)?;
        Ok(Self { mode, owner })
    }

    pub(super) fn matches(&self, previous: &Previous) -> bool {
        previous.mode == self.mode && self.owner.is_none_or(|owner| owner == previous.owner)
    }
}

fn ownership(
    call: &Call<'_>,
    owner: Option<&str>,
    group: Option<&str>,
) -> Result<Option<Ownership>, Failure> {
    let accounts = call.accounts();
    let user = match owner {
        Some(name) => Some(accounts.user(name)?.ok_or_else(|| {
            Failure::policy(format!("the owner {name} is not a user of this host"))
        })?),
        None => None,
    };
    let gid = match group {
        Some(name) => Some(
            accounts
                .group(name)?
                .ok_or_else(|| {
                    Failure::policy(format!("the group {name} is not a group of this host"))
                })?
                .gid,
        ),
        None => None,
    };
    Ok(match (user, gid) {
        (Some(user), gid) => Some(Ownership {
            uid: user.uid,
            gid: gid.unwrap_or(user.gid),
        }),
        (None, Some(_)) => {
            return Err(Failure::args(
                "a group without an owner is not a placement the agent applies",
            ));
        }
        (None, None) => None,
    })
}

impl Previous {
    pub(super) fn read(target: &Path) -> Result<Option<Self>, Failure> {
        let not_a_file = || Failure::policy(format!("{} is not a regular file", target.display()));
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags((OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK).bits())
            .open(target)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) if error.raw_os_error() == Some(Errno::ELOOP as i32) => {
                return Err(not_a_file());
            }
            Err(error) => return Err(Failure::io(target.display(), &error)),
        };
        let meta = file
            .metadata()
            .map_err(|error| Failure::io(target.display(), &error))?;
        if !meta.is_file() {
            return Err(not_a_file());
        }
        let too_large = || {
            Failure::policy(format!(
                "{} is over {READ_LIMIT} bytes, more than the agent reads",
                target.display()
            ))
        };
        if meta.len() > READ_LIMIT {
            return Err(too_large());
        }
        let mut content = Vec::new();
        file.take(READ_LIMIT + 1)
            .read_to_end(&mut content)
            .map_err(|error| Failure::io(target.display(), &error))?;
        if content.len() as u64 > READ_LIMIT {
            return Err(too_large());
        }
        Ok(Some(Self {
            content,
            mode: meta.mode() & 0o7777,
            owner: Ownership {
                uid: meta.uid(),
                gid: meta.gid(),
            },
        }))
    }
}

pub(super) fn place(
    target: &Path,
    content: &[u8],
    mode: u32,
    owner: Option<Ownership>,
) -> Result<(), Failure> {
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = target.with_file_name(format!(".{name}.tmp"));
    let placed = write_temporary(&temporary, content, mode, owner)
        .and_then(|()| fs::rename(&temporary, target));
    placed.map_err(|error| {
        let _ = fs::remove_file(&temporary);
        Failure::io(target.display(), &error)
    })
}

fn write_temporary(
    temporary: &Path,
    content: &[u8],
    mode: u32,
    owner: Option<Ownership>,
) -> io::Result<()> {
    if let Err(error) = fs::remove_file(temporary)
        && error.kind() != io::ErrorKind::NotFound
    {
        return Err(error);
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(OFlag::O_NOFOLLOW.bits())
        .open(temporary)?;
    file.write_all(content)?;
    settle(&file, mode, owner)?;
    file.sync_all()
}

pub(super) fn settle(file: &File, mode: u32, owner: Option<Ownership>) -> io::Result<()> {
    if let Some(owner) = owner {
        fchown(file, Some(owner.uid), Some(owner.gid))?;
    }
    file.set_permissions(fs::Permissions::from_mode(mode))
}

pub(super) fn rollback(target: &Path, previous: Option<&Previous>) -> Result<(), Failure> {
    match previous {
        Some(previous) => place(
            target,
            &previous.content,
            previous.mode,
            Some(previous.owner),
        ),
        None => fs::remove_file(target).map_err(|error| Failure::io(target.display(), &error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ERROR_POLICY;

    #[test]
    fn a_placement_never_lets_every_account_write_a_file_or_an_unsticky_directory() {
        for mode in [0o644, 0o640, 0o664, 0o600, 0o755, 0o1644] {
            assert!(Kind::File.admits(mode), "{mode:o}");
        }
        for mode in [0o666, 0o777, 0o646, 0o1777] {
            assert!(!Kind::File.admits(mode), "{mode:o}");
        }
        for mode in [0o755, 0o775, 0o2775, 0o1777, 0o3777] {
            assert!(Kind::Dir.admits(mode), "{mode:o}");
        }
        for mode in [0o777, 0o757, 0o2777] {
            assert!(!Kind::Dir.admits(mode), "{mode:o}");
        }
    }

    #[test]
    fn a_file_over_the_read_limit_is_refused_without_being_read() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("keel-dns-big.zone");
        File::create(&target)
            .unwrap()
            .set_len(READ_LIMIT + 1)
            .unwrap();
        let failure = Previous::read(&target).unwrap_err();
        assert_eq!(failure.key, ERROR_POLICY);
        assert!(
            failure.detail.contains("more than the agent reads"),
            "{}",
            failure.detail
        );
        File::create(&target).unwrap().set_len(READ_LIMIT).unwrap();
        let previous = Previous::read(&target).unwrap().unwrap();
        assert_eq!(previous.content.len() as u64, READ_LIMIT);
    }

    #[test]
    fn a_file_is_placed_through_a_temporary_beside_it_and_rolled_back_to_what_was_there() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("keel-dns-a.org.zone");
        assert!(Previous::read(&target).unwrap().is_none());
        place(&target, b"one\n", 0o640, None).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"one\n");
        assert_eq!(fs::metadata(&target).unwrap().mode() & 0o7777, 0o640);
        let previous = Previous::read(&target).unwrap().unwrap();
        assert_eq!(previous.content, b"one\n");
        assert_eq!(previous.mode, 0o640);
        place(&target, b"two\n", 0o644, Some(previous.owner)).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"two\n");
        rollback(&target, Some(&previous)).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"one\n");
        assert_eq!(fs::metadata(&target).unwrap().mode() & 0o7777, 0o640);
        rollback(&target, None).unwrap();
        assert!(!target.exists());
        assert!(fs::read_dir(dir.path()).unwrap().next().is_none());
        fs::create_dir(&target).unwrap();
        let failure = Previous::read(&target).unwrap_err();
        assert_eq!(failure.key, ERROR_POLICY);
    }
}
