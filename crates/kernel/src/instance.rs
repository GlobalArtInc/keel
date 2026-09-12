use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use keel_types::Id;
use keel_types::id::InvalidId;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, TransactionTrait};

use crate::kv;
use crate::secret::{Key, KeyError, Sealed};
use crate::storage::lock;

pub const SECRET_KEY_FILE: &str = "secret.key";
pub const INSTANCE_ID_FILE: &str = "instance.id";
pub const KV_INSTANCE_ID: &str = "instance_id";
pub const KV_SECRET_KEY: &str = "secret_key";

const SECRET_KEY_MODE: u32 = 0o600;

#[derive(Debug)]
pub struct Instance {
    id: Id,
    key: Key,
}

#[derive(Debug, thiserror::Error)]
pub enum InstanceError {
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
    #[error("{path} is not a secret key: {source}")]
    SecretKey {
        path: PathBuf,
        #[source]
        source: KeyError,
    },
    #[error("{path} has mode {mode:04o}; it must be readable by the product user alone (0600)")]
    SecretKeyMode { path: PathBuf, mode: u32 },
    #[error(
        "{path} does not match the wrapped key in keel_kv; restore the secret.key of this database or start against a fresh one"
    )]
    SecretKeyMismatch { path: PathBuf },
    #[error("{path} is not an instance id: {reason}")]
    InstanceId { path: PathBuf, reason: String },
    #[error(
        "{path} holds {file} but keel_kv holds {database}; the data directory and the database belong to different installations"
    )]
    InstanceIdMismatch {
        path: PathBuf,
        file: Id,
        database: Id,
    },
    #[error("keel_kv holds an unreadable {key} value: {reason}")]
    Stored { key: &'static str, reason: String },
    #[error("could not generate key material: {0}")]
    Entropy(#[source] KeyError),
    #[error("could not seal the data key under {path}: {source}")]
    Seal {
        path: PathBuf,
        #[source]
        source: KeyError,
    },
    #[error("the database refused a query: {0}")]
    Database(#[from] DbErr),
}

impl Instance {
    pub async fn establish(
        data_dir: &Path,
        db: &DatabaseConnection,
    ) -> Result<Self, InstanceError> {
        fs::create_dir_all(data_dir).map_err(|source| InstanceError::Create {
            path: data_dir.to_owned(),
            source,
        })?;
        let secret_path = data_dir.join(SECRET_KEY_FILE);
        let file_key = load_or_create_secret_key(&secret_path)?;
        let id_path = data_dir.join(INSTANCE_ID_FILE);
        let file_id = read_instance_id(&id_path)?;

        let txn = db.begin().await?;
        txn.execute_unprepared(&format!("SELECT pg_advisory_xact_lock({})", lock::INSTANCE))
            .await?;
        let id = reconcile_id(&txn, &id_path, file_id).await?;
        let key = reconcile_key(&txn, &secret_path, &file_key).await?;
        txn.commit().await?;
        tracing::info!(instance = %id, data_dir = %data_dir.display(), "instance established");
        Ok(Self { id, key })
    }

    pub fn id(&self) -> Id {
        self.id
    }

    pub fn key(&self) -> &Key {
        &self.key
    }
}

fn load_or_create_secret_key(path: &Path) -> Result<Key, InstanceError> {
    match fs::read_to_string(path) {
        Ok(text) => {
            let mode = fs::metadata(path)
                .map_err(|source| InstanceError::Read {
                    path: path.to_owned(),
                    source,
                })?
                .permissions()
                .mode()
                & 0o777;
            if mode & 0o077 != 0 {
                return Err(InstanceError::SecretKeyMode {
                    path: path.to_owned(),
                    mode,
                });
            }
            Key::parse_hex(&text).map_err(|source| InstanceError::SecretKey {
                path: path.to_owned(),
                source,
            })
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let key = Key::generate().map_err(InstanceError::Entropy)?;
            write_file(
                path,
                format!("{}\n", key.to_hex()).as_bytes(),
                Some(SECRET_KEY_MODE),
            )?;
            Ok(key)
        }
        Err(source) => Err(InstanceError::Read {
            path: path.to_owned(),
            source,
        }),
    }
}

fn read_instance_id(path: &Path) -> Result<Option<Id>, InstanceError> {
    match fs::read_to_string(path) {
        Ok(text) => {
            text.trim()
                .parse()
                .map(Some)
                .map_err(|error: InvalidId| InstanceError::InstanceId {
                    path: path.to_owned(),
                    reason: error.to_string(),
                })
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(InstanceError::Read {
            path: path.to_owned(),
            source,
        }),
    }
}

fn write_file(path: &Path, content: &[u8], mode: Option<u32>) -> Result<(), InstanceError> {
    let failed = |source| InstanceError::Create {
        path: path.to_owned(),
        source,
    };
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    if let Some(mode) = mode {
        options.mode(mode);
    }
    let mut file = options.open(path).map_err(failed)?;
    file.write_all(content).map_err(failed)?;
    file.sync_all().map_err(failed)
}

async fn reconcile_id<C: ConnectionTrait>(
    db: &C,
    path: &Path,
    file_id: Option<Id>,
) -> Result<Id, InstanceError> {
    let stored_id = stored_instance_id(db).await?;
    let id = match (file_id, stored_id) {
        (Some(file), Some(database)) if file != database => {
            return Err(InstanceError::InstanceIdMismatch {
                path: path.to_owned(),
                file,
                database,
            });
        }
        (Some(file), _) => file,
        (None, Some(database)) => database,
        (None, None) => Id::generate(),
    };
    if file_id.is_none() {
        write_file(path, format!("{id}\n").as_bytes(), None)?;
    }
    if stored_id.is_none() {
        kv::insert(
            db,
            KV_INSTANCE_ID,
            &serde_json::Value::String(id.to_string()),
        )
        .await?;
    }
    Ok(id)
}

async fn reconcile_key<C: ConnectionTrait>(
    db: &C,
    path: &Path,
    file_key: &Key,
) -> Result<Key, InstanceError> {
    let unreadable = |error: &dyn std::fmt::Display| InstanceError::Stored {
        key: KV_SECRET_KEY,
        reason: error.to_string(),
    };
    match kv::get(db, KV_SECRET_KEY).await? {
        Some(value) => {
            let sealed: Sealed =
                serde_json::from_value(value).map_err(|error| unreadable(&error))?;
            let bytes = file_key
                .open(&sealed)
                .map_err(|_| InstanceError::SecretKeyMismatch {
                    path: path.to_owned(),
                })?;
            Key::from_slice(&bytes).map_err(|error| unreadable(&error))
        }
        None => {
            let key = Key::generate().map_err(InstanceError::Entropy)?;
            let sealed = file_key
                .seal(key.as_bytes())
                .map_err(|source| InstanceError::Seal {
                    path: path.to_owned(),
                    source,
                })?;
            let value = serde_json::to_value(&sealed).map_err(|error| unreadable(&error))?;
            kv::insert(db, KV_SECRET_KEY, &value).await?;
            Ok(key)
        }
    }
}

async fn stored_instance_id<C: ConnectionTrait>(db: &C) -> Result<Option<Id>, InstanceError> {
    let Some(value) = kv::get(db, KV_INSTANCE_ID).await? else {
        return Ok(None);
    };
    let text = value.as_str().ok_or_else(|| InstanceError::Stored {
        key: KV_INSTANCE_ID,
        reason: "not a string".to_owned(),
    })?;
    text.parse()
        .map(Some)
        .map_err(|error: InvalidId| InstanceError::Stored {
            key: KV_INSTANCE_ID,
            reason: error.to_string(),
        })
}
