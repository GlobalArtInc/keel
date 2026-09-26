use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use nix::fcntl::OFlag;
use nix::unistd::geteuid;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::protocol::Failure;

const SUFFIX: &str = ".sha256";
const MODE: u32 = 0o600;
const WRITABLE_BY_OTHERS: u32 = 0o022;
const DIGEST_LEN: u64 = 64;
const BUFFER: usize = 64 * 1024;

pub(super) fn record(target: &Path, file: &mut File) -> Result<(), Failure> {
    let digest = digest(file).map_err(|error| Failure::io(target.display(), &error))?;
    let path = of(target);
    discard(target)?;
    let mut seal = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(MODE)
        .custom_flags(OFlag::O_NOFOLLOW.bits())
        .open(&path)
        .map_err(|error| Failure::io(path.display(), &error))?;
    seal.write_all(digest.as_bytes())
        .and_then(|()| seal.sync_all())
        .map_err(|error| Failure::io(path.display(), &error))
}

pub(super) fn verify(target: &Path, file: &mut File) -> Result<(), Failure> {
    let meta = file
        .metadata()
        .map_err(|error| Failure::io(target.display(), &error))?;
    if !agents(&meta) {
        return Err(Failure::policy(format!(
            "{} is not a file of the agent: it belongs to another user or others may write it",
            target.display()
        )));
    }
    let expected = expected(target)?;
    let actual = digest(file).map_err(|error| Failure::io(target.display(), &error))?;
    if !bool::from(expected.as_bytes().ct_eq(actual.as_bytes())) {
        return Err(Failure::policy(format!(
            "{} differs from what the agent wrote",
            target.display()
        )));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|error| Failure::io(target.display(), &error))?;
    Ok(())
}

pub(super) fn discard(target: &Path) -> Result<(), Failure> {
    let path = of(target);
    match fs::remove_file(&path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            Err(Failure::io(path.display(), &error))
        }
        _ => Ok(()),
    }
}

fn expected(target: &Path) -> Result<String, Failure> {
    let path = of(target);
    let seal = OpenOptions::new()
        .read(true)
        .custom_flags(OFlag::O_NOFOLLOW.bits())
        .open(&path)
        .map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                Failure::policy(format!(
                    "the agent recorded no digest of {}",
                    target.display()
                ))
            } else {
                Failure::io(path.display(), &error)
            }
        })?;
    let meta = seal
        .metadata()
        .map_err(|error| Failure::io(path.display(), &error))?;
    if !agents(&meta) {
        return Err(Failure::policy(format!(
            "the digest {} is not a file of the agent",
            path.display()
        )));
    }
    let mut text = String::new();
    seal.take(DIGEST_LEN + 1)
        .read_to_string(&mut text)
        .map_err(|error| Failure::io(path.display(), &error))?;
    Ok(text)
}

fn agents(meta: &fs::Metadata) -> bool {
    meta.is_file() && meta.uid() == geteuid().as_raw() && meta.mode() & WRITABLE_BY_OTHERS == 0
}

fn digest(file: &mut File) -> io::Result<String> {
    file.seek(SeekFrom::Start(0))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; BUFFER];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => hasher.update(&buffer[..read]),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    let bytes: [u8; 32] = hasher.finalize().into();
    let mut hex = String::with_capacity(64);
    for byte in bytes {
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(hex)
}

fn of(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    target.with_file_name(format!(".{name}{SUFFIX}"))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn opened(path: &Path) -> File {
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap()
    }

    #[test]
    fn a_recorded_file_verifies_until_one_byte_changes() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("keel-panel-a.sql");
        fs::write(&target, "CREATE TABLE t (a INT);\n").unwrap();
        record(&target, &mut opened(&target)).unwrap();
        let seal = dir.path().join(".keel-panel-a.sql.sha256");
        assert_eq!(fs::read_to_string(&seal).unwrap().len(), 64);
        assert_eq!(fs::metadata(&seal).unwrap().mode() & 0o777, MODE);
        verify(&target, &mut opened(&target)).unwrap();

        fs::write(&target, "CREATE TABLE t (a INT);\r").unwrap();
        let changed = verify(&target, &mut opened(&target)).unwrap_err();
        assert_eq!(changed.key, "host.error.policy");

        fs::write(&target, "CREATE TABLE t (a INT);\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o620)).unwrap();
        let shared = verify(&target, &mut opened(&target)).unwrap_err();
        assert_eq!(shared.key, "host.error.policy");

        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        discard(&target).unwrap();
        assert!(!seal.exists());
        let unsealed = verify(&target, &mut opened(&target)).unwrap_err();
        assert_eq!(unsealed.key, "host.error.policy");
        discard(&target).unwrap();
    }
}
