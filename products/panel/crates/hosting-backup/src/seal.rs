use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use rustix::process::geteuid;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::tree;

const SUFFIX: &str = ".sha256";
const WRITABLE_BY_OTHERS: u32 = 0o022;
const DIGEST_LEN: u64 = 64;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct Refused(String);

pub(crate) struct Hashing<T> {
    inner: T,
    hasher: Sha256,
}

pub(crate) struct Seal(String);

impl<T> Hashing<T> {
    pub(crate) fn new(inner: T) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
        }
    }

    pub(crate) fn finish(self) -> (T, String) {
        let bytes: [u8; 32] = self.hasher.finalize().into();
        let mut hex = String::with_capacity(64);
        for byte in bytes {
            let _ = write!(hex, "{byte:02x}");
        }
        (self.inner, hex)
    }
}

impl<R: Read> Read for Hashing<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.hasher.update(&buf[..read]);
        Ok(read)
    }
}

impl<W: Write> Write for Hashing<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.hasher.update(&buf[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl Seal {
    pub(crate) fn confirm(&self, target: &Path, actual: &str) -> io::Result<()> {
        if bool::from(self.0.as_bytes().ct_eq(actual.as_bytes())) {
            Ok(())
        } else {
            Err(refusal(format!(
                "{} differs from what the agent wrote",
                target.display()
            )))
        }
    }
}

pub(crate) fn expected(target: &Path) -> io::Result<Seal> {
    let path = of(target);
    let file = tree::open_file(&path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            refusal(format!(
                "the agent recorded no digest of {}",
                target.display()
            ))
        } else {
            error
        }
    })?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.uid() != geteuid().as_raw() || meta.mode() & WRITABLE_BY_OTHERS != 0
    {
        return Err(refusal(format!(
            "the digest {} is not a file of the agent",
            path.display()
        )));
    }
    let mut text = String::new();
    file.take(DIGEST_LEN + 1).read_to_string(&mut text)?;
    Ok(Seal(text))
}

pub(crate) fn record(target: &Path, digest: &str) -> io::Result<()> {
    remove(target)?;
    let mut file = tree::create_private(&of(target))?;
    file.write_all(digest.as_bytes())?;
    file.sync_all()
}

pub(crate) fn remove(target: &Path) -> io::Result<()> {
    match std::fs::remove_file(of(target)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

pub(crate) fn refused(error: &io::Error) -> bool {
    error.get_ref().is_some_and(|inner| inner.is::<Refused>())
}

fn refusal(detail: String) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, Refused(detail))
}

fn of(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    target.with_file_name(format!(".{name}{SUFFIX}"))
}
