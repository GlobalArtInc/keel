use std::fmt;

use argon2::{Algorithm, Argon2, Params, Version};
use rand::TryRng;
use rand::rngs::SysRng;
use serde::{Deserialize, Serialize};

use super::BackupError;
use crate::secret::{Key, KeyError, Sealed};

pub const FILE: &str = "secret.key.sealed";
pub const PASSPHRASE_VARIABLE: &str = "KEEL_BACKUP_PASSPHRASE";
const KDF: &str = "argon2id";
const SALT_LEN: usize = 16;
const KEY_LEN: usize = 32;
const MEMORY_KIB: u32 = 64 * 1024;
const ITERATIONS: u32 = 3;
const PARALLELISM: u32 = 1;

#[derive(Clone, PartialEq, Eq)]
pub struct Passphrase(String);

impl Passphrase {
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl fmt::Debug for Passphrase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Passphrase(***)")
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SealedKey {
    kdf: String,
    salt: String,
    memory: u32,
    iterations: u32,
    parallelism: u32,
    sealed: Sealed,
}

impl SealedKey {
    pub fn seal(key: &Key, passphrase: &Passphrase) -> Result<Self, BackupError> {
        let mut salt = [0u8; SALT_LEN];
        SysRng
            .try_fill_bytes(&mut salt)
            .map_err(|error| BackupError::Seal(KeyError::Entropy(error.to_string())))?;
        let derived = derive(passphrase, &salt, MEMORY_KIB, ITERATIONS, PARALLELISM)?;
        let sealed = derived.seal(key.as_bytes()).map_err(BackupError::Seal)?;
        Ok(Self {
            kdf: KDF.to_owned(),
            salt: salt.iter().map(|byte| format!("{byte:02x}")).collect(),
            memory: MEMORY_KIB,
            iterations: ITERATIONS,
            parallelism: PARALLELISM,
            sealed,
        })
    }

    pub fn open(&self, passphrase: &Passphrase) -> Result<Key, BackupError> {
        if self.kdf != KDF {
            return Err(BackupError::Passphrase);
        }
        let salt = decode_hex(&self.salt).ok_or(BackupError::Passphrase)?;
        let derived = derive(
            passphrase,
            &salt,
            self.memory,
            self.iterations,
            self.parallelism,
        )?;
        let bytes = derived
            .open(&self.sealed)
            .map_err(|_| BackupError::Passphrase)?;
        Key::from_slice(&bytes).map_err(|_| BackupError::Passphrase)
    }
}

fn derive(
    passphrase: &Passphrase,
    salt: &[u8],
    memory: u32,
    iterations: u32,
    parallelism: u32,
) -> Result<Key, BackupError> {
    let params = Params::new(memory, iterations, parallelism, Some(KEY_LEN))
        .map_err(|error| BackupError::Seal(KeyError::Entropy(error.to_string())))?;
    let mut derived = [0u8; KEY_LEN];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, &mut derived)
        .map_err(|error| BackupError::Seal(KeyError::Entropy(error.to_string())))?;
    Key::from_slice(&derived).map_err(BackupError::Seal)
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(text.get(index..index + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_sealed_under_a_passphrase_opens_with_it_alone() {
        let key = Key::generate().unwrap();
        let passphrase = Passphrase::new("correct horse battery staple");
        assert_eq!(format!("{passphrase:?}"), "Passphrase(***)");
        let sealed = SealedKey::seal(&key, &passphrase).unwrap();
        assert_eq!(sealed.kdf, "argon2id");
        assert_eq!(sealed.salt.len(), 32);
        assert_eq!(sealed.memory, 65536);
        let text = serde_json::to_string(&sealed).unwrap();
        assert!(!text.contains(&key.to_hex()), "the key is not in the clear");
        let parsed: SealedKey = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed.open(&passphrase).unwrap(), key);
        let wrong = parsed.open(&Passphrase::new("wrong")).unwrap_err();
        assert!(matches!(wrong, BackupError::Passphrase), "{wrong}");
        let other = SealedKey {
            kdf: "scrypt".to_owned(),
            ..parsed
        };
        assert!(matches!(
            other.open(&passphrase).unwrap_err(),
            BackupError::Passphrase
        ));
        assert_eq!(decode_hex("0aff"), Some(vec![10, 255]));
        assert_eq!(decode_hex("0af"), None);
        assert_eq!(decode_hex("zz"), None);
    }
}
