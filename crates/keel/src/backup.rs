use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

use keel_kernel::backup::{
    BackupError, Created, DataKey, Note, PASSPHRASE_VARIABLE, Passphrase, Request, Restore, Site,
};
use keel_kernel::instance::SECRET_KEY_FILE;
use keel_kernel::loader::{BootError, Sections};
use keel_kernel::secret::Key;
use keel_kernel::{Config, Storage};
use keel_sdk::system::BackupManifest;

use crate::error::Error;
use crate::products::refuse_while_running;
use crate::runtime::Runtime;

pub(crate) const RESTORE_CLI: &str = "keel restore";
const RESTORE_BOOT: &str = "keel serve";

#[derive(Debug, thiserror::Error)]
pub(crate) enum BackupCliError {
    #[error(transparent)]
    Runtime(Box<Error>),
    #[error(transparent)]
    Backup(#[from] BackupError),
    #[error(
        "a passphrase is needed to {action} the secret key: set {PASSPHRASE_VARIABLE} or answer the prompt on a terminal"
    )]
    NoPassphrase { action: &'static str },
    #[error("the passphrase could not be read from the terminal: {0}")]
    Terminal(#[source] io::Error),
}

impl From<Error> for BackupCliError {
    fn from(error: Error) -> Self {
        Self::Runtime(Box::new(error))
    }
}

#[derive(Debug)]
pub(crate) struct Restored {
    pub(crate) note: Note,
    pub(crate) skipped_tables: Vec<String>,
    pub(crate) config_kept: Vec<String>,
    pub(crate) config_fallback: Option<PathBuf>,
    pub(crate) files_written: usize,
    pub(crate) sources_skipped: Vec<String>,
    pub(crate) secret_key: Option<PathBuf>,
}

impl Runtime {
    pub(crate) async fn backup(
        &self,
        config: &Config,
        out: Option<&Path>,
        passphrase: Option<&Passphrase>,
    ) -> Result<Created, BackupCliError> {
        let storage = Storage::connect(config).await.map_err(Error::from)?;
        let outcome = async {
            let enabled = self.enabled(&storage).await?.ids;
            let frozen = self
                .composition(&enabled)?
                .key(throwaway_key()?)
                .sections(Sections::new(&config.file, config.sections.clone()))
                .data_dir(config.data_dir.clone())
                .resolve()
                .map_err(Error::from)?
                .init()
                .map_err(BootError::from)
                .map_err(Error::from)?
                .freeze()
                .map_err(BootError::from)
                .map_err(Error::from)?;
            let site = Site::new(frozen.graph(), frozen.registry());
            let archive = out.map_or_else(|| site.default_archive(), Path::to_path_buf);
            Ok(site.create(storage.db(), &archive, passphrase).await?)
        }
        .await;
        storage.close().await.map_err(Error::from)?;
        outcome
    }

    pub(crate) async fn restore(
        &self,
        config: &Config,
        archive: &Path,
        passphrase: Option<&Passphrase>,
        requested_by: &str,
    ) -> Result<Restored, BackupCliError> {
        refuse_while_running(config).await?;
        let restore = Restore::open(archive, &config.data_dir)?;
        let outcome = self
            .restore_opened(config, &restore, passphrase, requested_by)
            .await;
        restore.close()?;
        outcome
    }

    async fn restore_opened(
        &self,
        config: &Config,
        restore: &Restore,
        passphrase: Option<&Passphrase>,
        requested_by: &str,
    ) -> Result<Restored, BackupCliError> {
        let manifest = restore.manifest();
        self.check_bundles(manifest)?;
        let enabled: Vec<String> = manifest
            .products
            .iter()
            .map(|product| product.id.clone())
            .collect();
        let storage = Storage::connect(config).await.map_err(Error::from)?;
        let outcome = async {
            let graph = self
                .composition(&enabled)?
                .key(throwaway_key()?)
                .sections(Sections::new(&config.file, config.sections.clone()))
                .data_dir(config.data_dir.clone())
                .resolve()
                .map_err(Error::from)?;
            let frozen = graph
                .init()
                .map_err(BootError::from)
                .map_err(Error::from)?
                .freeze()
                .map_err(BootError::from)
                .map_err(Error::from)?;
            let site = Site::new(frozen.graph(), frozen.registry());
            site.admits(manifest)?;
            graph.migrate(&storage).await.map_err(Error::from)?;
            let (file_key, secret_key) = match passphrase {
                Some(passphrase) => {
                    let key = restore.secret_key(passphrase)?;
                    let path = restore.write_secret_key(&config.data_dir, &key)?;
                    (Some(key), Some(path))
                }
                None => (host_key(&config.data_dir)?, None),
            };
            let loaded = restore.database(storage.db(), file_key.as_ref()).await?;
            let files = restore.files(&config.data_dir, &config.file)?;
            let sources = restore
                .sources(site.contributions(), storage.db(), &config.data_dir)
                .await?;
            let note = Note {
                archive: restore.archive().to_owned(),
                at: site.now(),
                keel: manifest.keel.clone(),
                requested_by: requested_by.to_owned(),
                products_before: loaded.products_before,
                products_after: loaded.products_after,
                data_key: match loaded.data_key {
                    DataKey::Kept => "kept",
                    DataKey::Dropped => "dropped",
                    DataKey::Absent => "absent",
                }
                .to_owned(),
                tables: loaded.loaded.len(),
                sources: sources.restored,
            };
            note.write(&config.data_dir)?;
            Ok(Restored {
                note,
                skipped_tables: loaded.skipped,
                config_kept: files.etc.kept,
                config_fallback: files.etc_fallback,
                files_written: files.etc.written.len() + files.data.written.len(),
                sources_skipped: sources.skipped,
                secret_key,
            })
        }
        .await;
        storage.close().await.map_err(Error::from)?;
        outcome
    }

    fn check_bundles(&self, manifest: &BackupManifest) -> Result<(), BackupError> {
        for product in &manifest.products {
            if self.bundle(&product.id).is_none() {
                return Err(BackupError::Bundle {
                    id: product.id.clone(),
                    known: self.known_ids(),
                });
            }
        }
        Ok(())
    }

    pub(crate) async fn restore_pending(&self, config: &Config) -> Result<(), Error> {
        if let Some(request) = Request::consume(&config.data_dir).map_err(BootError::from)? {
            tracing::warn!(
                archive = %request.archive.display(),
                requested_by = %request.requested_by,
                "a restore request is pending; restoring before the start"
            );
            let restored = self
                .restore(config, &request.archive, None, &request.requested_by)
                .await
                .map_err(|error| match error {
                    BackupCliError::Runtime(error) => *error,
                    BackupCliError::Backup(error) => Error::Boot(BootError::Restore(error)),
                    other => Error::Boot(BootError::Restore(BackupError::Archive {
                        path: request.archive.clone(),
                        reason: other.to_string(),
                    })),
                })?;
            tracing::info!(
                archive = %request.archive.display(),
                tables = restored.note.tables,
                "{RESTORE_BOOT} restored the archive the request named"
            );
        }
        if let Some(note) = Note::consume(&config.data_dir).map_err(BootError::from)? {
            tracing::info!(
                archive = %note.archive.display(),
                keel = %note.keel,
                requested_by = %note.requested_by,
                tables = note.tables,
                data_key = %note.data_key,
                enabled = %note.changes(),
                "the installation was restored from a backup; the enabled set changed: {}",
                note.changes()
            );
        }
        Ok(())
    }
}

pub(crate) fn throwaway_key() -> Result<Key, BackupError> {
    Key::generate().map_err(BackupError::Entropy)
}

fn host_key(data_dir: &Path) -> Result<Option<Key>, BackupError> {
    let path = data_dir.join(SECRET_KEY_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => Key::parse_hex(&text)
            .map(Some)
            .map_err(|source| BackupError::SecretKey { path, source }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(BackupError::Read { path, source }),
    }
}

pub(crate) fn passphrase(
    action: &'static str,
    variable: Option<String>,
) -> Result<Passphrase, BackupCliError> {
    if let Some(text) = variable
        && !text.is_empty()
    {
        return Ok(Passphrase::new(text));
    }
    let stdin = io::stdin();
    if !stdin.is_terminal() {
        return Err(BackupCliError::NoPassphrase { action });
    }
    eprint!("passphrase to {action} the secret key: ");
    io::stderr().flush().map_err(BackupCliError::Terminal)?;
    let text = terminal::read_hidden(&stdin).map_err(BackupCliError::Terminal)?;
    eprintln!();
    if text.is_empty() {
        return Err(BackupCliError::NoPassphrase { action });
    }
    Ok(Passphrase::new(text))
}

mod terminal {
    use std::io::{self, BufRead, Stdin};
    use std::os::fd::AsFd;

    use nix::sys::termios::{LocalFlags, SetArg, tcgetattr, tcsetattr};

    pub(super) fn read_hidden(stdin: &Stdin) -> io::Result<String> {
        let original = tcgetattr(stdin.as_fd())?;
        let mut hidden = original.clone();
        hidden.local_flags.remove(LocalFlags::ECHO);
        tcsetattr(stdin.as_fd(), SetArg::TCSANOW, &hidden)?;
        let mut line = String::new();
        let read = stdin.lock().read_line(&mut line);
        tcsetattr(stdin.as_fd(), SetArg::TCSANOW, &original)?;
        read?;
        Ok(line.trim_end_matches(['\r', '\n']).to_owned())
    }
}

pub(crate) fn describe_manifest(manifest: &BackupManifest) -> String {
    let products: Vec<String> = manifest
        .products
        .iter()
        .map(|product| match &product.edition {
            Some(edition) => format!("{} {} ({edition})", product.id, product.version),
            None => format!("{} {}", product.id, product.version),
        })
        .collect();
    format!(
        "keel {} taken {}; products: {}; {} table(s), {} source(s), {} configuration file(s); secret key {}",
        manifest.keel,
        manifest.at.to_rfc3339(),
        if products.is_empty() {
            "none".to_owned()
        } else {
            products.join(", ")
        },
        manifest.tables.len(),
        manifest.sources.len(),
        manifest.config.len(),
        if manifest.secret_key {
            "included"
        } else {
            "not included"
        }
    )
}

#[cfg(test)]
mod tests {
    use keel_sdk::chrono::Utc;
    use keel_sdk::system::BackupProduct;

    use super::*;

    #[test]
    fn a_passphrase_comes_from_the_variable_before_the_terminal() {
        let given = passphrase("seal", Some("secret words".into())).unwrap();
        assert_eq!(given, Passphrase::new("secret words"));
        let missing = passphrase("open", Some(String::new()));
        let terminal = io::stdin().is_terminal();
        match missing {
            Err(BackupCliError::NoPassphrase { action }) => {
                assert_eq!(action, "open");
                assert!(!terminal);
            }
            other => assert!(terminal, "{other:?}"),
        }
        assert_eq!(
            BackupCliError::NoPassphrase { action: "open" }.to_string(),
            "a passphrase is needed to open the secret key: set KEEL_BACKUP_PASSPHRASE or answer the prompt on a terminal"
        );
    }

    #[test]
    fn a_manifest_is_described_in_one_line() {
        let manifest = BackupManifest {
            keel: "0.1.0".into(),
            at: "2026-09-21T10:15:30Z".parse().unwrap(),
            instance: None,
            schema: "keel".into(),
            products: vec![
                BackupProduct {
                    id: "demo".into(),
                    version: "0.1.0".into(),
                    edition: None,
                },
                BackupProduct {
                    id: "dns".into(),
                    version: "0.2.0".into(),
                    edition: Some("pro".into()),
                },
            ],
            modules: Vec::new(),
            tables: Vec::new(),
            sources: vec!["dns.zonefiles".into()],
            config: vec!["config.yaml".into()],
            data: Vec::new(),
            secret_key: true,
        };
        assert_eq!(
            describe_manifest(&manifest),
            "keel 0.1.0 taken 2026-09-21T10:15:30+00:00; products: demo 0.1.0, dns 0.2.0 (pro); 0 table(s), 1 source(s), 1 configuration file(s); secret key included"
        );
        let bare = BackupManifest {
            products: Vec::new(),
            secret_key: false,
            at: Utc::now(),
            ..manifest
        };
        assert!(describe_manifest(&bare).contains("products: none"));
        assert!(describe_manifest(&bare).ends_with("secret key not included"));
    }
}
