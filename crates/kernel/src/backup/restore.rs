use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use keel_sdk::runtime::Contributions;
use keel_sdk::sea_orm::DatabaseConnection;
use keel_sdk::system::BackupManifest;
use keel_types::Id;
use serde::{Deserialize, Serialize};

use super::sources::SourcesRestored;
use super::{BackupError, archive, dump, files, seal, service, sources};
use crate::instance::SECRET_KEY_FILE;
use crate::secret::Key;

pub const REQUEST_FILE: &str = "restore.request";

fn is_permission_denied(error: &BackupError) -> bool {
    match error {
        BackupError::Write { source, .. } | BackupError::Create { source, .. } => {
            source.kind() == io::ErrorKind::PermissionDenied
        }
        _ => false,
    }
}
pub const NOTE_FILE: &str = "restore.note";
const STAGING_PREFIX: &str = ".restore-";
const SECRET_KEY_MODE: u32 = 0o600;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub archive: PathBuf,
    pub requested_by: String,
}

impl Request {
    pub fn write(&self, data_dir: &Path) -> Result<PathBuf, BackupError> {
        let path = data_dir.join(REQUEST_FILE);
        let text = serde_json::to_vec_pretty(self).map_err(|source| BackupError::Encode {
            what: "the restore request",
            source,
        })?;
        files::write_whole(&path, &text, SECRET_KEY_MODE)?;
        Ok(path)
    }

    pub fn consume(data_dir: &Path) -> Result<Option<Self>, BackupError> {
        let path = data_dir.join(REQUEST_FILE);
        let text = match fs::read(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(BackupError::Read { path, source }),
        };
        fs::remove_file(&path).map_err(|source| BackupError::Write {
            path: path.clone(),
            source,
        })?;
        let request = serde_json::from_slice(&text).map_err(|error| BackupError::Archive {
            path: path.clone(),
            reason: format!("the restore request does not parse: {error}"),
        })?;
        Ok(Some(request))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    pub archive: PathBuf,
    pub at: DateTime<Utc>,
    pub keel: String,
    pub requested_by: String,
    pub products_before: Vec<String>,
    pub products_after: Vec<String>,
    pub data_key: String,
    pub tables: usize,
    pub sources: Vec<String>,
}

impl Note {
    pub fn write(&self, data_dir: &Path) -> Result<PathBuf, BackupError> {
        let path = data_dir.join(NOTE_FILE);
        let text = serde_json::to_vec_pretty(self).map_err(|source| BackupError::Encode {
            what: "the restore note",
            source,
        })?;
        files::write_whole(&path, &text, SECRET_KEY_MODE)?;
        Ok(path)
    }

    pub fn consume(data_dir: &Path) -> Result<Option<Self>, BackupError> {
        let path = data_dir.join(NOTE_FILE);
        let text = match fs::read(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(BackupError::Read { path, source }),
        };
        fs::remove_file(&path).map_err(|source| BackupError::Write {
            path: path.clone(),
            source,
        })?;
        let note = serde_json::from_slice(&text).map_err(|error| BackupError::Archive {
            path: path.clone(),
            reason: format!("the restore note does not parse: {error}"),
        })?;
        Ok(Some(note))
    }

    pub fn changes(&self) -> String {
        let mut changes: Vec<String> = self
            .products_after
            .iter()
            .filter(|id| !self.products_before.contains(id))
            .map(|id| format!("+{id}"))
            .collect();
        changes.extend(
            self.products_before
                .iter()
                .filter(|id| !self.products_after.contains(id))
                .map(|id| format!("-{id}")),
        );
        if changes.is_empty() {
            "unchanged".to_owned()
        } else {
            changes.join(" ")
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FilesRestored {
    pub etc: files::Restored,
    pub etc_fallback: Option<PathBuf>,
    pub data: files::Restored,
}

#[derive(Debug)]
pub struct Restore {
    archive: PathBuf,
    staging: PathBuf,
    manifest: BackupManifest,
}

impl Restore {
    pub fn open(archive: &Path, data_dir: &Path) -> Result<Self, BackupError> {
        let staging = data_dir
            .join(service::DIRECTORY)
            .join(format!("{STAGING_PREFIX}{}", Id::generate()));
        fs::create_dir_all(&staging).map_err(|source| BackupError::Create {
            path: staging.clone(),
            source,
        })?;
        let unpacked = archive::unpack(archive, &staging).and_then(|()| {
            let path = staging.join(archive::MANIFEST);
            let text = fs::read(&path).map_err(|source| BackupError::Read {
                path: path.clone(),
                source,
            })?;
            serde_json::from_slice::<BackupManifest>(&text).map_err(|error| BackupError::Archive {
                path: archive.to_owned(),
                reason: format!("its {} does not parse: {error}", archive::MANIFEST),
            })
        });
        match unpacked {
            Ok(manifest) => Ok(Self {
                archive: archive.to_owned(),
                staging,
                manifest,
            }),
            Err(error) => {
                let _ = fs::remove_dir_all(&staging);
                Err(error)
            }
        }
    }

    pub fn archive(&self) -> &Path {
        &self.archive
    }

    pub fn manifest(&self) -> &BackupManifest {
        &self.manifest
    }

    pub fn secret_key(&self, passphrase: &seal::Passphrase) -> Result<Key, BackupError> {
        let path = self.staging.join(archive::DATA_DIR).join(seal::FILE);
        let text = match fs::read(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(BackupError::NoSecretKey);
            }
            Err(source) => return Err(BackupError::Read { path, source }),
        };
        let sealed: seal::SealedKey =
            serde_json::from_slice(&text).map_err(|error| BackupError::Archive {
                path: self.archive.clone(),
                reason: format!("its sealed secret key does not parse: {error}"),
            })?;
        sealed.open(passphrase)
    }

    pub fn write_secret_key(&self, data_dir: &Path, key: &Key) -> Result<PathBuf, BackupError> {
        let path = data_dir.join(SECRET_KEY_FILE);
        files::write_whole(
            &path,
            format!("{}\n", key.to_hex()).as_bytes(),
            SECRET_KEY_MODE,
        )?;
        Ok(path)
    }

    pub async fn database(
        &self,
        db: &DatabaseConnection,
        file_key: Option<&Key>,
    ) -> Result<dump::Loaded, BackupError> {
        dump::load(
            db,
            &self.staging.join(archive::DB_DIR),
            &self.manifest.tables,
            file_key,
        )
        .await
    }

    pub fn files(&self, data_dir: &Path, config_file: &Path) -> Result<FilesRestored, BackupError> {
        let staged = self.staging.join(archive::ETC_DIR);
        let (etc, etc_fallback) = match config_file.parent() {
            Some(config_dir) if !config_dir.as_os_str().is_empty() => {
                match files::restore(&staged, config_dir, &[config_file.to_owned()], &[]) {
                    Ok(etc) => (etc, None),
                    Err(error) if is_permission_denied(&error) => {
                        let fallback = self.etc_fallback(data_dir);
                        (
                            files::restore(&staged, &fallback, &[], &[])?,
                            Some(fallback),
                        )
                    }
                    Err(error) => return Err(error),
                }
            }
            _ => (files::Restored::default(), None),
        };
        let data = files::restore(
            &self.staging.join(archive::DATA_DIR),
            data_dir,
            &[],
            &[seal::FILE, SECRET_KEY_FILE],
        )?;
        Ok(FilesRestored {
            etc,
            etc_fallback,
            data,
        })
    }

    fn etc_fallback(&self, data_dir: &Path) -> PathBuf {
        let stem = self
            .archive
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| {
                name.trim_end_matches(&format!(".{}", archive::EXTENSION))
                    .to_owned()
            })
            .unwrap_or_else(|| "archive".to_owned());
        data_dir
            .join(service::DIRECTORY)
            .join(format!("{stem}.{}", archive::ETC_DIR))
    }

    pub async fn sources(
        &self,
        contributions: &Contributions,
        db: &DatabaseConnection,
        data_dir: &Path,
    ) -> Result<SourcesRestored, BackupError> {
        sources::restore_all(
            contributions,
            db,
            &self.staging.join(archive::SOURCES_DIR),
            &self.manifest.sources,
            data_dir,
        )
        .await
    }

    pub fn close(self) -> Result<(), BackupError> {
        fs::remove_dir_all(&self.staging).map_err(|source| BackupError::Write {
            path: self.staging.clone(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_written_read_once_and_gone_afterwards() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Request::consume(dir.path()).unwrap(), None);
        let request = Request {
            archive: PathBuf::from("/var/lib/keel/backups/keel-0.1.0-20260921T101530Z.tar.zst"),
            requested_by: "alice".into(),
        };
        let path = request.write(dir.path()).unwrap();
        assert_eq!(path, dir.path().join(REQUEST_FILE));
        assert_eq!(Request::consume(dir.path()).unwrap(), Some(request));
        assert!(!path.exists());
        assert_eq!(Request::consume(dir.path()).unwrap(), None);
        fs::write(&path, "not json").unwrap();
        let error = Request::consume(dir.path()).unwrap_err();
        assert!(matches!(error, BackupError::Archive { .. }), "{error}");
        assert!(!path.exists());
    }

    #[test]
    fn a_note_names_what_the_enabled_set_changed() {
        let dir = tempfile::tempdir().unwrap();
        let note = Note {
            archive: PathBuf::from("/tmp/a.tar.zst"),
            at: "2026-09-21T10:15:30Z".parse().unwrap(),
            keel: "0.1.0".into(),
            requested_by: "keel restore".into(),
            products_before: vec!["demo".into(), "panel".into()],
            products_after: vec!["demo".into(), "dns".into()],
            data_key: "kept".into(),
            tables: 12,
            sources: Vec::new(),
        };
        assert_eq!(note.changes(), "+dns -panel");
        note.write(dir.path()).unwrap();
        assert_eq!(Note::consume(dir.path()).unwrap(), Some(note.clone()));
        assert_eq!(Note::consume(dir.path()).unwrap(), None);
        let same = Note {
            products_after: note.products_before.clone(),
            ..note
        };
        assert_eq!(same.changes(), "unchanged");
    }

    #[test]
    fn the_configuration_tree_lands_under_backups_when_its_directory_is_not_writable() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join("staging");
        fs::create_dir_all(staging.join(archive::ETC_DIR).join("templates")).unwrap();
        fs::create_dir_all(staging.join(archive::DATA_DIR)).unwrap();
        fs::write(
            staging.join(archive::ETC_DIR).join("config.yaml"),
            "from: archive\n",
        )
        .unwrap();
        fs::write(
            staging.join(archive::ETC_DIR).join("agent.yaml"),
            "policy\n",
        )
        .unwrap();
        fs::write(
            staging
                .join(archive::ETC_DIR)
                .join("templates")
                .join("zone.j2"),
            "zone\n",
        )
        .unwrap();
        fs::write(
            staging.join(archive::DATA_DIR).join("instance.id"),
            "01ID\n",
        )
        .unwrap();
        fs::write(staging.join(archive::DATA_DIR).join(seal::FILE), "sealed").unwrap();
        let restore = Restore {
            archive: PathBuf::from("/srv/site.tar.zst"),
            staging: staging.clone(),
            manifest: BackupManifest {
                keel: "0.1.0".into(),
                at: Utc::now(),
                instance: None,
                schema: "keel".into(),
                products: Vec::new(),
                modules: Vec::new(),
                tables: Vec::new(),
                sources: Vec::new(),
                config: Vec::new(),
                data: Vec::new(),
                secret_key: false,
            },
        };
        let etc = dir.path().join("etc");
        fs::create_dir_all(&etc).unwrap();
        fs::write(etc.join("config.yaml"), "from: host\n").unwrap();
        fs::set_permissions(&etc, fs::Permissions::from_mode(0o500)).unwrap();
        let data_dir = dir.path().join("lib");
        let restored = restore.files(&data_dir, &etc.join("config.yaml")).unwrap();
        fs::set_permissions(&etc, fs::Permissions::from_mode(0o700)).unwrap();
        let fallback = data_dir.join(service::DIRECTORY).join("site.etc");
        assert_eq!(restored.etc_fallback, Some(fallback.clone()));
        assert_eq!(
            restored.etc.written,
            ["agent.yaml", "config.yaml", "templates/zone.j2"]
        );
        assert!(restored.etc.kept.is_empty());
        assert_eq!(
            fs::read_to_string(fallback.join("config.yaml")).unwrap(),
            "from: archive\n"
        );
        assert_eq!(
            fs::read_to_string(etc.join("config.yaml")).unwrap(),
            "from: host\n"
        );
        assert_eq!(restored.data.written, ["instance.id"]);
        assert!(!data_dir.join(seal::FILE).exists());
        assert_eq!(
            fs::read_to_string(data_dir.join("instance.id")).unwrap(),
            "01ID\n"
        );
        let writable = restore.files(&data_dir, &etc.join("config.yaml")).unwrap();
        assert_eq!(writable.etc_fallback, None);
        assert_eq!(writable.etc.kept, ["config.yaml"]);
        assert_eq!(writable.etc.written, ["agent.yaml", "templates/zone.j2"]);
        assert_eq!(restore.manifest().keel, "0.1.0");
        assert_eq!(restore.archive(), Path::new("/srv/site.tar.zst"));
    }

    #[test]
    fn opening_something_that_is_not_an_archive_leaves_no_staging_behind() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("plain.txt");
        fs::write(&plain, "hello").unwrap();
        let error = Restore::open(&plain, dir.path()).unwrap_err();
        assert!(matches!(error, BackupError::Archive { .. }), "{error}");
        let backups = dir.path().join(service::DIRECTORY);
        let leftovers: Vec<_> = fs::read_dir(&backups)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }
}
