mod archive;
mod dump;
mod files;
mod manifest;
mod restore;
mod seal;
mod service;
mod sources;

use std::io;
use std::path::PathBuf;

use keel_sdk::KeelError;
use keel_sdk::sea_orm::sqlx;
use keel_types::ErrorCode;

use crate::secret::KeyError;

pub use archive::read_manifest;
pub use dump::DataKey;
pub use files::KEPT_SUFFIX;
pub use restore::{Note, Request, Restore};
pub use seal::{PASSPHRASE_VARIABLE, Passphrase};
pub use service::{Created, Handle, Site};

const ERROR_ARCHIVE: &str = "keel.error.backup_archive";
const ERROR_INCOMPATIBLE: &str = "keel.error.backup_incompatible";
const ERROR_BUNDLE: &str = "keel.error.backup_bundle";
const ERROR_FAILED: &str = "keel.error.backup_failed";
const ERROR_EXISTS: &str = "keel.error.backup_exists";
const ERROR_NOT_STARTED: &str = "keel.error.backup_not_started";

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("could not create {path}: {source}")]
    Create {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{path} is not a keel backup archive: {reason}")]
    Archive { path: PathBuf, reason: String },
    #[error("{path} exists already; a backup never overwrites an archive")]
    Exists { path: PathBuf },
    #[error("the archive entry {entry:?} is refused: {reason}")]
    Entry { entry: String, reason: &'static str },
    #[error("the database refused {step}: {source}")]
    Database {
        step: String,
        #[source]
        source: sqlx::Error,
    },
    #[error("the backup source {name} failed: {source}")]
    Source {
        name: String,
        #[source]
        source: KeelError,
    },
    #[error("the archive was taken by keel {archive} and this binary is keel {running}: {reason}")]
    Incompatible {
        archive: String,
        running: String,
        reason: String,
    },
    #[error("the archive names the product {id}, which this binary does not carry; it carries {}", carried(.known))]
    Bundle { id: String, known: Vec<String> },
    #[error("the passphrase does not open the sealed secret key of the archive")]
    Passphrase,
    #[error("the archive carries no secret key; a backup taken with --with-secret-key does")]
    NoSecretKey,
    #[error("the secret key could not be sealed under the passphrase: {0}")]
    Seal(#[source] KeyError),
    #[error("no key material could be generated: {0}")]
    Entropy(#[source] KeyError),
    #[error("the secret key of {path} is unusable: {source}")]
    SecretKey {
        path: PathBuf,
        #[source]
        source: KeyError,
    },
    #[error("{what} could not be encoded: {source}")]
    Encode {
        what: &'static str,
        #[source]
        source: serde_json::Error,
    },
}

fn carried(known: &[String]) -> String {
    if known.is_empty() {
        "no product bundle".to_owned()
    } else {
        known.join(", ")
    }
}

impl From<BackupError> for KeelError {
    fn from(error: BackupError) -> Self {
        let text = error.to_string();
        match &error {
            BackupError::Archive { path, .. } | BackupError::Read { path, .. } => {
                KeelError::with_key(ErrorCode::Validation, ERROR_ARCHIVE)
                    .param("path", path.display().to_string())
                    .param("reason", text)
                    .source(error)
            }
            BackupError::Entry { .. } | BackupError::NoSecretKey | BackupError::Passphrase => {
                KeelError::with_key(ErrorCode::Validation, ERROR_ARCHIVE)
                    .param("reason", text)
                    .source(error)
            }
            BackupError::Exists { path } => KeelError::conflict(ERROR_EXISTS)
                .param("path", path.display().to_string())
                .source(error),
            BackupError::Incompatible {
                archive, running, ..
            } => KeelError::conflict(ERROR_INCOMPATIBLE)
                .param("archive", archive.clone())
                .param("running", running.clone())
                .param("reason", text)
                .source(error),
            BackupError::Bundle { id, known } => KeelError::conflict(ERROR_BUNDLE)
                .param("id", id.clone())
                .param("known", known.join(", "))
                .source(error),
            _ => KeelError::internal(ERROR_FAILED)
                .param("reason", text)
                .source(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_backup_error_maps_to_a_call_error_by_its_kind() {
        let archive: KeelError = BackupError::Archive {
            path: PathBuf::from("/tmp/x.tar.zst"),
            reason: "no manifest".to_owned(),
        }
        .into();
        assert_eq!(archive.key(), ERROR_ARCHIVE);
        assert_eq!(archive.code(), ErrorCode::Validation);
        assert_eq!(archive.params()["path"], "/tmp/x.tar.zst");
        let bundle: KeelError = BackupError::Bundle {
            id: "dns".into(),
            known: vec!["demo".into()],
        }
        .into();
        assert_eq!(bundle.key(), ERROR_BUNDLE);
        assert_eq!(bundle.params()["known"], "demo");
        assert_eq!(
            BackupError::Bundle {
                id: "dns".into(),
                known: Vec::new()
            }
            .to_string(),
            "the archive names the product dns, which this binary does not carry; it carries no product bundle"
        );
        let incompatible: KeelError = BackupError::Incompatible {
            archive: "1.0.0".into(),
            running: "0.1.0".into(),
            reason: "major 1 is not restored by major 0".into(),
        }
        .into();
        assert_eq!(incompatible.key(), ERROR_INCOMPATIBLE);
        assert_eq!(incompatible.code(), ErrorCode::Conflict);
        let failed: KeelError = BackupError::NoSecretKey.into();
        assert_eq!(failed.key(), ERROR_ARCHIVE);
        let exists: KeelError = BackupError::Exists {
            path: PathBuf::from("/tmp/x.tar.zst"),
        }
        .into();
        assert_eq!(exists.key(), ERROR_EXISTS);
        assert_eq!(exists.code(), ErrorCode::Conflict);
        let internal: KeelError = BackupError::Write {
            path: PathBuf::from("/x"),
            source: io::Error::other("full"),
        }
        .into();
        assert_eq!(internal.key(), ERROR_FAILED);
        assert_eq!(internal.code(), ErrorCode::Internal);
    }
}
