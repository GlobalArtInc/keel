use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use keel_release::{Channel, Version};
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
        let metadata = fs::symlink_metadata(path).map_err(|source| match source.kind() {
            io::ErrorKind::NotFound => RequestError::Missing(path.to_owned()),
            _ => RequestError::Read {
                path: path.to_owned(),
                source,
            },
        })?;
        if !metadata.file_type().is_file() {
            remove(path)?;
            return Err(RequestError::NotAFile {
                path: path.to_owned(),
            });
        }
        if metadata.len() > MAX_BYTES {
            remove(path)?;
            return Err(RequestError::TooLarge {
                path: path.to_owned(),
            });
        }
        let read = fs::read(path);
        remove(path)?;
        let bytes = read.map_err(|source| RequestError::Read {
            path: path.to_owned(),
            source,
        })?;
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
    use super::*;

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
