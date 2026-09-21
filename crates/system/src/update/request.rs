use std::path::{Path, PathBuf};

use keel_sdk::serde::{Deserialize, Serialize};
use keel_sdk::serde_json;
use keel_sdk::{KeelError, Result};
use semver::Version;

use super::channel::Channel;

pub const FILE: &str = "update.request";
const ERROR_REQUEST: &str = "update.error.request";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "keel_sdk::serde")]
pub(super) struct Request {
    pub(super) version: Version,
    pub(super) channel: Channel,
    pub(super) requested_by: String,
}

fn path_of(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

pub(super) async fn write(data_dir: &Path, request: &Request) -> Result<PathBuf> {
    let path = path_of(data_dir);
    let staging = data_dir.join(format!("{FILE}.tmp"));
    let failed = |path: &Path| {
        let path = path.display().to_string();
        move |error: std::io::Error| {
            KeelError::internal(ERROR_REQUEST)
                .param("path", path)
                .param("reason", error.to_string())
                .source(error)
        }
    };
    let mut text = serde_json::to_string(request)?;
    text.push('\n');
    tokio::fs::write(&staging, text)
        .await
        .map_err(failed(&staging))?;
    tokio::fs::rename(&staging, &path)
        .await
        .map_err(failed(&path))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use keel_sdk::serde_json::json;

    use super::*;

    #[test]
    fn the_request_is_the_three_fields_the_root_side_reads() {
        let request = Request {
            version: Version::new(1, 3, 0),
            channel: Channel::Stable,
            requested_by: "admin".into(),
        };
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({ "version": "1.3.0", "channel": "stable", "requested_by": "admin" })
        );
        assert_eq!(
            path_of(Path::new("/var/lib/keel")),
            PathBuf::from("/var/lib/keel/update.request")
        );
    }

    #[tokio::test]
    async fn the_request_lands_atomically_and_replaces_an_unconsumed_one() {
        let dir = tempfile::tempdir().unwrap();
        let first = Request {
            version: Version::new(0, 2, 0),
            channel: Channel::Beta,
            requested_by: "root".into(),
        };
        let path = write(dir.path(), &first).await.unwrap();
        assert_eq!(path, dir.path().join(FILE));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.ends_with('\n'));
        assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), first);
        let second = Request {
            version: Version::new(0, 3, 0),
            ..first
        };
        write(dir.path(), &second).await.unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), second);
        assert!(!dir.path().join(format!("{FILE}.tmp")).exists());
        let missing = write(&dir.path().join("nowhere"), &second)
            .await
            .unwrap_err();
        assert_eq!(missing.key(), ERROR_REQUEST);
    }
}
