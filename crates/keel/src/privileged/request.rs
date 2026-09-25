use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use keel_release::{Channel, Version};
use nix::errno::Errno;
use nix::fcntl::OFlag;
use serde::Deserialize;

const MAX_BYTES: u64 = 64 * 1024;
const MAX_PRINCIPAL: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub(crate) struct Request {
    pub(crate) version: Version,
    pub(crate) channel: Channel,
    pub(crate) requested_by: String,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RequestError {
    #[error("no update request at {0}")]
    Missing(PathBuf),
    #[error("could not read the update request {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not remove the update request {path}: {source}")]
    Remove {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the update request {path} is not a regular file")]
    NotAFile { path: PathBuf },
    #[error("the update request {path} is over {MAX_BYTES} bytes")]
    TooLarge { path: PathBuf },
    #[error("the update request {path} is not a version, a channel and a principal: {source}")]
    Shape {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error(
        "the update request {path} names a principal with a control character or over {MAX_PRINCIPAL} characters"
    )]
    Principal { path: PathBuf },
}

impl Request {
    pub(crate) fn consume(path: &Path) -> Result<Self, RequestError> {
        let file = match open(path) {
            Err(RequestError::Missing(missing)) => return Err(RequestError::Missing(missing)),
            opened => {
                remove(path)?;
                opened?
            }
        };
        let bytes = read(path, file)?;
        let request: Request =
            serde_json::from_slice(&bytes).map_err(|source| RequestError::Shape {
                path: path.to_owned(),
                source,
            })?;
        let principal = &request.requested_by;
        if principal.is_empty()
            || principal.chars().count() > MAX_PRINCIPAL
            || principal.chars().any(char::is_control)
        {
            return Err(RequestError::Principal {
                path: path.to_owned(),
            });
        }
        Ok(request)
    }
}

fn open(path: &Path) -> Result<File, RequestError> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags((OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK).bits())
        .open(path)
        .map_err(|source| match source.kind() {
            io::ErrorKind::NotFound => RequestError::Missing(path.to_owned()),
            _ if source.raw_os_error() == Some(Errno::ELOOP as i32) => RequestError::NotAFile {
                path: path.to_owned(),
            },
            _ => RequestError::Read {
                path: path.to_owned(),
                source,
            },
        })?;
    let metadata = file.metadata().map_err(|source| RequestError::Read {
        path: path.to_owned(),
        source,
    })?;
    if !metadata.file_type().is_file() {
        return Err(RequestError::NotAFile {
            path: path.to_owned(),
        });
    }
    if metadata.len() > MAX_BYTES {
        return Err(RequestError::TooLarge {
            path: path.to_owned(),
        });
    }
    Ok(file)
}

fn read(path: &Path, file: File) -> Result<Vec<u8>, RequestError> {
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| RequestError::Read {
            path: path.to_owned(),
            source,
        })?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(RequestError::TooLarge {
            path: path.to_owned(),
        });
    }
    Ok(bytes)
}

fn remove(path: &Path) -> Result<(), RequestError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(RequestError::Remove {
            path: path.to_owned(),
            source,
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    const REQUEST: &str = r#"{"version":"1.3.0","channel":"stable","requested_by":"admin"}"#;

    #[test]
    fn a_request_that_grows_after_its_check_is_refused_at_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update.request");
        fs::write(&path, REQUEST).unwrap();
        let file = open(&path).unwrap();
        let mut grown = OpenOptions::new().append(true).open(&path).unwrap();
        grown.write_all(&vec![b' '; MAX_BYTES as usize]).unwrap();
        assert!(matches!(
            read(&path, file),
            Err(RequestError::TooLarge { path: refused }) if refused == path
        ));
    }

    #[test]
    fn a_request_swapped_for_a_link_after_its_check_is_read_from_the_checked_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update.request");
        fs::write(&path, REQUEST).unwrap();
        let file = open(&path).unwrap();
        let elsewhere = dir.path().join("elsewhere");
        fs::write(
            &elsewhere,
            r#"{"version":"0.0.1","channel":"beta","requested_by":"intruder"}"#,
        )
        .unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        fs::rename(&link, &path).unwrap();
        assert_eq!(read(&path, file).unwrap(), REQUEST.as_bytes());
        assert!(matches!(
            open(&path),
            Err(RequestError::NotAFile { path: refused }) if refused == path
        ));
    }

    #[test]
    fn a_fifo_at_the_request_path_is_refused_without_blocking_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update.request");
        nix::unistd::mkfifo(&path, nix::sys::stat::Mode::from_bits_truncate(0o600)).unwrap();
        assert!(matches!(
            Request::consume(&path),
            Err(RequestError::NotAFile { path: refused }) if refused == path
        ));
        assert!(fs::symlink_metadata(&path).is_err());
    }

    #[test]
    fn the_design_request_parses_and_is_removed_once_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update.request");
        fs::write(
            &path,
            r#"{"version":"1.3.0","channel":"stable","requested_by":"admin"}"#,
        )
        .unwrap();
        let request = Request::consume(&path).unwrap();
        assert_eq!(
            request,
            Request {
                version: Version::new(1, 3, 0),
                channel: Channel::Stable,
                requested_by: "admin".to_owned(),
            }
        );
        assert!(!path.exists());
        assert!(matches!(
            Request::consume(&path),
            Err(RequestError::Missing(missing)) if missing == path
        ));
    }

    #[test]
    fn extra_keys_are_ignored_and_a_broken_request_is_still_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update.request");
        fs::write(
            &path,
            r#"{"version":"0.2.0","channel":"beta","requested_by":"admin","command":"rm -rf /","url":"https://elsewhere/"}"#,
        )
        .unwrap();
        let request = Request::consume(&path).unwrap();
        assert_eq!(request.channel, Channel::Beta);
        assert_eq!(request.version, Version::new(0, 2, 0));

        fs::write(
            &path,
            r#"{"version":"0.2.0","channel":"nightly","requested_by":"admin"}"#,
        )
        .unwrap();
        assert!(matches!(
            Request::consume(&path),
            Err(RequestError::Shape { .. })
        ));
        assert!(!path.exists());

        fs::write(
            &path,
            r#"{"version":"../0.2.0","channel":"stable","requested_by":"admin"}"#,
        )
        .unwrap();
        assert!(matches!(
            Request::consume(&path),
            Err(RequestError::Shape { .. })
        ));

        fs::write(&path, "{\"version\":\"0.2.0\",\"channel\":\"stable\",\"requested_by\":\"admin\\noutcome=ok\"}").unwrap();
        assert!(matches!(
            Request::consume(&path),
            Err(RequestError::Principal { .. })
        ));
        assert!(!path.exists());

        fs::write(&path, vec![b' '; MAX_BYTES as usize + 1]).unwrap();
        assert!(matches!(
            Request::consume(&path),
            Err(RequestError::TooLarge { .. })
        ));
        assert!(!path.exists());
    }

    #[test]
    fn a_request_that_is_not_a_regular_file_is_refused_unread_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update.request");
        let target = dir.path().join("large");
        let content = vec![b'{'; MAX_BYTES as usize * 4];
        fs::write(&target, &content).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(matches!(
            Request::consume(&path),
            Err(RequestError::NotAFile { path: refused }) if refused == path
        ));
        assert!(fs::symlink_metadata(&path).is_err());
        assert_eq!(fs::read(&target).unwrap(), content);

        let status = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(matches!(
            Request::consume(&path),
            Err(RequestError::NotAFile { .. })
        ));
        assert!(fs::symlink_metadata(&path).is_err());
    }
}
