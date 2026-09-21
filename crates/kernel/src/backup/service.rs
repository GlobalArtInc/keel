use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use chrono::{DateTime, Utc};
use keel_sdk::async_trait::async_trait;
use keel_sdk::runtime::Contributions;
use keel_sdk::sea_orm::DatabaseConnection;
use keel_sdk::system::{BackupCreated, BackupManifest, BackupModule, BackupProduct, Backups};
use keel_sdk::{KeelError, Result as SdkResult};
use keel_types::Id;

use super::restore::Request;
use super::seal::{Passphrase, SealedKey};
use super::{BackupError, ERROR_NOT_STARTED, archive, dump, files, manifest, seal, sources};
use crate::clock::Clock;
use crate::instance::{INSTANCE_ID_FILE, SECRET_KEY_FILE};
use crate::loader::Graph;
use crate::migrations::LEDGER;
use crate::registry::Registry;
use crate::secret::Key;

pub const DIRECTORY: &str = "backups";
const TLS_DIR: &str = "tls";
const STAGING_PREFIX: &str = ".staging-";
const SEALED_MODE: u32 = 0o600;

fn archive_name(version: &str, at: DateTime<Utc>) -> String {
    format!(
        "keel-{version}-{}.{}",
        at.format("%Y%m%dT%H%M%SZ"),
        archive::EXTENSION
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Created {
    pub archive: PathBuf,
    pub bytes: u64,
    pub manifest: BackupManifest,
}

pub struct Site {
    version: String,
    carried: Vec<String>,
    products: Vec<BackupProduct>,
    modules: Vec<(String, String, Option<String>)>,
    data_dir: PathBuf,
    config_file: PathBuf,
    clock: Arc<dyn Clock>,
    contributions: Arc<Contributions>,
}

impl Site {
    pub fn new(graph: &Graph, registry: &Registry) -> Self {
        let mut modules: Vec<(String, String, Option<String>)> = graph
            .modules()
            .iter()
            .map(|def| {
                let ledger = if def.id == crate::keel::MODULE.id {
                    Some(LEDGER.to_owned())
                } else {
                    def.migrations
                        .map(|catalogue| catalogue.ledger().to_owned())
                };
                (def.id.to_owned(), def.version.to_owned(), ledger)
            })
            .collect();
        modules.sort();
        Self {
            version: graph.version().to_owned(),
            modules,
            carried: graph
                .manifests()
                .iter()
                .map(|bundle| bundle.id.to_owned())
                .collect(),
            products: graph
                .products()
                .iter()
                .map(|bundle| BackupProduct {
                    id: bundle.id.to_owned(),
                    version: bundle.version.to_owned(),
                    edition: bundle
                        .default_edition()
                        .map(|edition| edition.id.to_owned()),
                })
                .collect(),
            data_dir: graph.data_dir().to_owned(),
            config_file: graph.sections().file().to_owned(),
            clock: graph.clock().clone(),
            contributions: registry.contributions().clone(),
        }
    }

    pub fn directory(&self) -> PathBuf {
        self.data_dir.join(DIRECTORY)
    }

    fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn contributions(&self) -> &Arc<Contributions> {
        &self.contributions
    }

    pub fn now(&self) -> DateTime<Utc> {
        self.clock.now()
    }

    pub fn default_archive(&self) -> PathBuf {
        self.directory()
            .join(archive_name(&self.version, self.clock.now()))
    }

    pub fn admits(&self, manifest: &BackupManifest) -> Result<(), BackupError> {
        manifest::compatible(&manifest.keel, &self.version)?;
        for product in &manifest.products {
            if !self.carried.contains(&product.id) {
                return Err(BackupError::Bundle {
                    id: product.id.clone(),
                    known: self.carried.clone(),
                });
            }
        }
        Ok(())
    }

    pub async fn create(
        &self,
        db: &DatabaseConnection,
        out: &Path,
        passphrase: Option<&Passphrase>,
    ) -> Result<Created, BackupError> {
        let parent = match out.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_owned(),
            _ => PathBuf::from("."),
        };
        fs::create_dir_all(&parent).map_err(|source| BackupError::Create {
            path: parent.clone(),
            source,
        })?;
        let staging = parent.join(format!("{STAGING_PREFIX}{}", Id::generate()));
        fs::create_dir_all(&staging).map_err(|source| BackupError::Create {
            path: staging.clone(),
            source,
        })?;
        let outcome = self.stage(db, &staging, out, passphrase).await;
        let _ = fs::remove_dir_all(&staging);
        outcome
    }

    async fn stage(
        &self,
        db: &DatabaseConnection,
        staging: &Path,
        out: &Path,
        passphrase: Option<&Passphrase>,
    ) -> Result<Created, BackupError> {
        let at = self.clock.now();
        let dumped = dump::write(db, &staging.join(archive::DB_DIR)).await?;
        let config = match self.config_file.parent() {
            Some(config_dir) if !config_dir.as_os_str().is_empty() => files::snapshot(
                config_dir,
                &staging.join(archive::ETC_DIR),
                &[self.data_dir.clone(), staging.to_owned(), out.to_owned()],
            )?,
            _ => Vec::new(),
        };
        let data_staging = staging.join(archive::DATA_DIR);
        let mut data = files::snapshot(
            &self.data_dir.join(TLS_DIR),
            &data_staging.join(TLS_DIR),
            &[],
        )?
        .into_iter()
        .map(|name| format!("{TLS_DIR}/{name}"))
        .collect::<Vec<_>>();
        let instance = self.copy_instance(&data_staging, &mut data)?;
        let secret_key = match passphrase {
            Some(passphrase) => {
                self.seal_secret_key(&data_staging, passphrase)?;
                data.push(seal::FILE.to_owned());
                true
            }
            None => false,
        };
        data.sort();
        let sources = sources::write_all(
            &self.contributions,
            db,
            &staging.join(archive::SOURCES_DIR),
            &self.data_dir,
        )
        .await?;
        let manifest = BackupManifest {
            keel: self.version.clone(),
            at,
            instance,
            schema: dumped.schema,
            products: self.products.clone(),
            modules: self
                .modules
                .iter()
                .map(|(id, version, ledger)| {
                    let stats = ledger.as_ref().and_then(|table| dumped.ledgers.get(table));
                    BackupModule {
                        id: id.clone(),
                        version: version.clone(),
                        ledger: ledger.clone(),
                        applied: stats.map_or(0, |ledger| ledger.applied),
                        head: stats.and_then(|ledger| ledger.head.clone()),
                    }
                })
                .collect(),
            tables: dumped.tables,
            sources,
            config,
            data,
            secret_key,
        };
        let text = serde_json::to_vec_pretty(&manifest).map_err(|source| BackupError::Encode {
            what: "the backup manifest",
            source,
        })?;
        let manifest_path = staging.join(archive::MANIFEST);
        fs::write(&manifest_path, text).map_err(|source| BackupError::Write {
            path: manifest_path,
            source,
        })?;
        let bytes = archive::pack(staging, out)?;
        Ok(Created {
            archive: out.to_owned(),
            bytes,
            manifest,
        })
    }

    fn copy_instance(
        &self,
        data_staging: &Path,
        data: &mut Vec<String>,
    ) -> Result<Option<String>, BackupError> {
        let path = self.data_dir.join(INSTANCE_ID_FILE);
        match fs::read_to_string(&path) {
            Ok(text) => {
                files::write_whole(&data_staging.join(INSTANCE_ID_FILE), text.as_bytes(), 0o644)?;
                data.push(INSTANCE_ID_FILE.to_owned());
                Ok(Some(text.trim().to_owned()))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(BackupError::Read { path, source }),
        }
    }

    fn seal_secret_key(
        &self,
        data_staging: &Path,
        passphrase: &Passphrase,
    ) -> Result<(), BackupError> {
        let path = self.data_dir.join(SECRET_KEY_FILE);
        let text = fs::read_to_string(&path).map_err(|source| BackupError::Read {
            path: path.clone(),
            source,
        })?;
        let key = Key::parse_hex(&text).map_err(|source| BackupError::SecretKey {
            path: path.clone(),
            source,
        })?;
        let sealed = SealedKey::seal(&key, passphrase)?;
        let text = serde_json::to_vec_pretty(&sealed).map_err(|source| BackupError::Encode {
            what: "the sealed secret key",
            source,
        })?;
        files::write_whole(&data_staging.join(seal::FILE), &text, SEALED_MODE)
    }
}

pub struct Handle {
    attached: OnceLock<(Arc<Site>, DatabaseConnection)>,
}

impl Handle {
    pub fn new() -> Self {
        Self {
            attached: OnceLock::new(),
        }
    }

    pub fn attach(&self, site: Arc<Site>, db: DatabaseConnection) -> bool {
        self.attached.set((site, db)).is_ok()
    }

    fn attached(&self) -> SdkResult<&(Arc<Site>, DatabaseConnection)> {
        self.attached
            .get()
            .ok_or_else(|| KeelError::unavailable(ERROR_NOT_STARTED))
    }
}

impl Default for Handle {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Handle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handle")
            .field("attached", &self.attached.get().is_some())
            .finish()
    }
}

#[async_trait]
impl Backups for Handle {
    fn directory(&self) -> PathBuf {
        self.attached()
            .map(|(site, _)| site.directory())
            .unwrap_or_else(|_| PathBuf::from(DIRECTORY))
    }

    async fn create(&self, archive: &Path) -> SdkResult<BackupCreated> {
        let (site, db) = self.attached()?;
        let created = site.create(db, archive, None).await?;
        Ok(BackupCreated {
            archive: created.archive,
            bytes: created.bytes,
            manifest: created.manifest,
        })
    }

    async fn inspect(&self, archive: &Path) -> SdkResult<BackupManifest> {
        let path = archive.to_owned();
        let manifest = tokio::task::spawn_blocking(move || archive::read_manifest(&path))
            .await
            .map_err(|error| KeelError::internal(super::ERROR_FAILED).source(error))??;
        Ok(manifest)
    }

    fn admits(&self, manifest: &BackupManifest) -> SdkResult<()> {
        let (site, _) = self.attached()?;
        Ok(site.admits(manifest)?)
    }

    async fn request_restore(&self, archive: &Path, requested_by: &str) -> SdkResult<PathBuf> {
        let (site, _) = self.attached()?;
        let request = Request {
            archive: archive.to_owned(),
            requested_by: requested_by.to_owned(),
        };
        Ok(request.write(site.data_dir())?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_archive_is_named_by_the_version_and_the_instant() {
        let at: DateTime<Utc> = "2026-09-21T10:15:30Z".parse().unwrap();
        assert_eq!(
            archive_name("0.1.0", at),
            "keel-0.1.0-20260921T101530Z.tar.zst"
        );
    }

    #[tokio::test]
    async fn a_handle_before_start_answers_unavailable() {
        let handle = Handle::new();
        assert_eq!(format!("{handle:?}"), "Handle { attached: false }");
        assert_eq!(handle.directory(), PathBuf::from(DIRECTORY));
        let error = handle
            .create(Path::new("/tmp/x.tar.zst"))
            .await
            .unwrap_err();
        assert_eq!(error.key(), ERROR_NOT_STARTED);
        let error = handle
            .request_restore(Path::new("/tmp/x.tar.zst"), "alice")
            .await
            .unwrap_err();
        assert_eq!(error.key(), ERROR_NOT_STARTED);
        let manifest = BackupManifest {
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
        };
        assert_eq!(
            handle.admits(&manifest).unwrap_err().key(),
            ERROR_NOT_STARTED
        );
        let error = handle
            .inspect(Path::new("/nowhere/x.tar.zst"))
            .await
            .unwrap_err();
        assert_eq!(error.key(), super::super::ERROR_ARCHIVE);
    }
}
