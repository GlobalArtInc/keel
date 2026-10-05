use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{PUBLIC_KEY_LENGTH, SECRET_KEY_LENGTH, Signer, SigningKey};
use keel_sdk::runtime::Contributions;
use keel_sdk::{InstanceInfoSource, ProductManifest};
use keel_types::id::InvalidId;
use keel_types::{Id, InstanceInfo, PROOF_SCHEME, PROTOCOL};
use rand::TryRng;
use rand::rngs::SysRng;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, TransactionTrait};

use crate::kv;
use crate::secret::{Key, KeyError, Sealed};
use crate::storage::lock;

pub const SECRET_KEY_FILE: &str = "secret.key";
pub const INSTANCE_ID_FILE: &str = "instance.id";
pub const KV_INSTANCE_ID: &str = "instance_id";
pub const KV_SECRET_KEY: &str = "secret_key";
pub const KV_INSTANCE_KEY: &str = "instance_key";

const SECRET_KEY_MODE: u32 = 0o600;
const PUBLIC_KEY_SCHEME: &str = "ed25519:";
// https://www.rfc-editor.org/rfc/rfc8410#section-4
const SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

#[derive(Debug)]
pub struct Instance {
    id: Id,
    key: Key,
    instance_key: InstanceKey,
}

#[derive(Clone)]
pub struct InstanceKey(SigningKey);

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
        let instance_key = reconcile_instance_key(&txn, &key).await?;
        txn.commit().await?;
        tracing::info!(
            instance = %id,
            data_dir = %data_dir.display(),
            public_key = %instance_key.public_key(),
            "instance established"
        );
        Ok(Self {
            id,
            key,
            instance_key,
        })
    }

    pub fn id(&self) -> Id {
        self.id
    }

    pub fn key(&self) -> &Key {
        &self.key
    }

    pub fn instance_key(&self) -> &InstanceKey {
        &self.instance_key
    }
}

impl InstanceKey {
    pub fn generate() -> Result<Self, KeyError> {
        let mut seed = [0u8; SECRET_KEY_LENGTH];
        SysRng
            .try_fill_bytes(&mut seed)
            .map_err(|error| KeyError::Entropy(error.to_string()))?;
        Ok(Self(SigningKey::from_bytes(&seed)))
    }

    pub fn public_key(&self) -> String {
        let mut der = Vec::with_capacity(SPKI_PREFIX.len() + PUBLIC_KEY_LENGTH);
        der.extend_from_slice(&SPKI_PREFIX);
        der.extend_from_slice(self.0.verifying_key().as_bytes());
        format!("{PUBLIC_KEY_SCHEME}{}", STANDARD.encode(der))
    }

    pub fn prove(&self, instance: &InstanceInfo, challenge: &str) -> String {
        let signature = self.0.sign(&instance.proof_message(challenge));
        format!("{PROOF_SCHEME}{}", STANDARD.encode(signature.to_bytes()))
    }

    fn seal(&self, data_key: &Key) -> Result<Sealed, KeyError> {
        data_key.seal(self.0.as_bytes())
    }

    fn open(data_key: &Key, sealed: &Sealed) -> Result<Self, String> {
        let bytes = data_key.open(sealed).map_err(|error| error.to_string())?;
        let seed: [u8; SECRET_KEY_LENGTH] = bytes.try_into().map_err(|bytes: Vec<u8>| {
            format!("{} bytes instead of {SECRET_KEY_LENGTH}", bytes.len())
        })?;
        Ok(Self(SigningKey::from_bytes(&seed)))
    }
}

impl fmt::Debug for InstanceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "InstanceKey({})", self.public_key())
    }
}

pub(crate) fn handshake(
    instance_id: Id,
    instance_key: &InstanceKey,
    version: &str,
    products: &[&ProductManifest],
    contributions: &Contributions,
) -> InstanceInfo {
    let mut features = Vec::new();
    let mut service_kinds = Vec::new();
    for source in contributions.of::<InstanceInfoSource>() {
        features.extend(source.features().iter().cloned());
        service_kinds.extend(source.service_kinds().iter().cloned());
    }
    let mut products: Vec<String> = products
        .iter()
        .map(|product| product.id.to_owned())
        .collect();
    for list in [&mut features, &mut service_kinds, &mut products] {
        list.sort();
        list.dedup();
    }
    InstanceInfo {
        instance_id,
        keel: version.to_owned(),
        protocol: PROTOCOL,
        products,
        public_key: instance_key.public_key(),
        features,
        service_kinds,
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

async fn reconcile_instance_key<C: ConnectionTrait>(
    db: &C,
    data_key: &Key,
) -> Result<InstanceKey, InstanceError> {
    let unreadable = |reason: String| InstanceError::Stored {
        key: KV_INSTANCE_KEY,
        reason,
    };
    match kv::get(db, KV_INSTANCE_KEY).await? {
        Some(value) => {
            let sealed: Sealed =
                serde_json::from_value(value).map_err(|error| unreadable(error.to_string()))?;
            InstanceKey::open(data_key, &sealed).map_err(unreadable)
        }
        None => {
            let instance_key = InstanceKey::generate().map_err(InstanceError::Entropy)?;
            let sealed = instance_key
                .seal(data_key)
                .map_err(|error| unreadable(error.to_string()))?;
            let value =
                serde_json::to_value(&sealed).map_err(|error| unreadable(error.to_string()))?;
            kv::insert(db, KV_INSTANCE_KEY, &value).await?;
            Ok(instance_key)
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
