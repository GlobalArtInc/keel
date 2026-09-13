use std::fmt;

use aes_gcm::aead::{Aead, Nonce};
use aes_gcm::{Aes256Gcm, KeyInit};
use keel_sdk::KeelError;
use keel_sdk::system::SecretCipher;
use rand::TryRng;
use rand::rngs::SysRng;
use serde::{Deserialize, Serialize};

const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const ALGORITHM: &str = "A256GCM";

#[derive(Clone, PartialEq, Eq)]
pub struct Key([u8; KEY_LEN]);

#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("the operating system gave no randomness: {0}")]
    Entropy(String),
    #[error("a key is {KEY_LEN} bytes written as {} hex characters", KEY_LEN * 2)]
    Format,
    #[error("the value could not be sealed")]
    Seal,
    #[error("the sealed value is not {ALGORITHM} with a {NONCE_LEN}-byte nonce")]
    Sealed,
    #[error("the sealed value does not open under this key")]
    Open,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct Sealed {
    pub alg: String,
    pub nonce: String,
    pub data: String,
}

impl Key {
    pub fn generate() -> Result<Self, KeyError> {
        let mut bytes = [0u8; KEY_LEN];
        SysRng
            .try_fill_bytes(&mut bytes)
            .map_err(|error| KeyError::Entropy(error.to_string()))?;
        Ok(Self(bytes))
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self, KeyError> {
        <[u8; KEY_LEN]>::try_from(bytes)
            .map(Self)
            .map_err(|_| KeyError::Format)
    }

    pub fn parse_hex(text: &str) -> Result<Self, KeyError> {
        let bytes = decode_hex(text.trim()).ok_or(KeyError::Format)?;
        Self::from_slice(&bytes)
    }

    pub fn to_hex(&self) -> String {
        encode_hex(&self.0)
    }

    pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }

    pub fn seal(&self, plaintext: &[u8]) -> Result<Sealed, KeyError> {
        let mut nonce = [0u8; NONCE_LEN];
        SysRng
            .try_fill_bytes(&mut nonce)
            .map_err(|error| KeyError::Entropy(error.to_string()))?;
        let data = self
            .cipher()
            .encrypt(&Nonce::<Aes256Gcm>::from(nonce), plaintext)
            .map_err(|_| KeyError::Seal)?;
        Ok(Sealed {
            alg: ALGORITHM.to_owned(),
            nonce: encode_hex(&nonce),
            data: encode_hex(&data),
        })
    }

    pub fn open(&self, sealed: &Sealed) -> Result<Vec<u8>, KeyError> {
        if sealed.alg != ALGORITHM {
            return Err(KeyError::Sealed);
        }
        let nonce: [u8; NONCE_LEN] = decode_hex(&sealed.nonce)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or(KeyError::Sealed)?;
        let data = decode_hex(&sealed.data).ok_or(KeyError::Sealed)?;
        self.cipher()
            .decrypt(&Nonce::<Aes256Gcm>::from(nonce), data.as_slice())
            .map_err(|_| KeyError::Open)
    }

    fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new(&aes_gcm::Key::<Aes256Gcm>::from(self.0))
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Key(***)")
    }
}

pub struct DataKeyCipher {
    key: Key,
}

impl DataKeyCipher {
    pub fn new(key: Key) -> Self {
        Self { key }
    }
}

impl fmt::Debug for DataKeyCipher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DataKeyCipher(***)")
    }
}

impl SecretCipher for DataKeyCipher {
    fn seal(&self, plaintext: &[u8]) -> keel_sdk::Result<String> {
        let sealed = self
            .key
            .seal(plaintext)
            .map_err(|error| KeelError::internal("keel.error.secret_seal").source(error))?;
        serde_json::to_string(&sealed)
            .map_err(|error| KeelError::internal("keel.error.secret_seal").source(error))
    }

    fn open(&self, sealed: &str) -> keel_sdk::Result<Vec<u8>> {
        let sealed: Sealed = serde_json::from_str(sealed)
            .map_err(|error| KeelError::internal("keel.error.secret_open").source(error))?;
        self.key
            .open(&sealed)
            .map_err(|error| KeelError::internal("keel.error.secret_open").source(error))
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    text.as_bytes()
        .chunks(2)
        .map(|pair| std::str::from_utf8(pair).ok())
        .map(|pair| pair.and_then(|pair| u8::from_str_radix(pair, 16).ok()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_key_round_trips_through_its_hex_form() {
        let key = Key::generate().unwrap();
        let hex = key.to_hex();
        assert_eq!(hex.len(), KEY_LEN * 2);
        assert_eq!(Key::parse_hex(&format!("{hex}\n")).unwrap(), key);
        assert_ne!(key, Key::generate().unwrap());
        assert_eq!(format!("{key:?}"), "Key(***)");
    }

    #[test]
    fn a_key_of_the_wrong_length_or_alphabet_is_refused() {
        assert!(matches!(Key::parse_hex("abcd"), Err(KeyError::Format)));
        assert!(matches!(
            Key::parse_hex(&"zz".repeat(KEY_LEN)),
            Err(KeyError::Format)
        ));
        assert!(matches!(Key::from_slice(&[0; 31]), Err(KeyError::Format)));
    }

    #[test]
    fn a_sealed_value_opens_only_under_the_key_that_sealed_it() {
        let key = Key::generate().unwrap();
        let sealed = key.seal(b"data key material").unwrap();
        assert_eq!(sealed.alg, "A256GCM");
        assert_eq!(sealed.nonce.len(), NONCE_LEN * 2);
        assert_eq!(key.open(&sealed).unwrap(), b"data key material");
        let other = Key::generate().unwrap();
        assert!(matches!(other.open(&sealed), Err(KeyError::Open)));
        let mut tampered = sealed.clone();
        tampered.data.replace_range(0..2, "00");
        assert!(matches!(key.open(&tampered), Err(KeyError::Open)));
        let mut foreign = sealed.clone();
        foreign.alg = "XCHACHA".to_owned();
        assert!(matches!(key.open(&foreign), Err(KeyError::Sealed)));
    }

    #[test]
    fn two_seals_of_the_same_value_differ_by_nonce() {
        let key = Key::generate().unwrap();
        let first = key.seal(b"x").unwrap();
        let second = key.seal(b"x").unwrap();
        assert_ne!(first.nonce, second.nonce);
        assert_ne!(first.data, second.data);
    }

    #[test]
    fn the_cipher_seals_to_json_text_that_only_its_own_key_opens() {
        let key = Key::generate().unwrap();
        let cipher = DataKeyCipher::new(key.clone());
        let sealed = cipher.seal(b"JBSWY3DPEHPK3PXP").unwrap();
        assert!(!sealed.contains("JBSWY3DPEHPK3PXP"));
        let parsed: Sealed = serde_json::from_str(&sealed).unwrap();
        assert_eq!(key.open(&parsed).unwrap(), b"JBSWY3DPEHPK3PXP");
        assert_eq!(cipher.open(&sealed).unwrap(), b"JBSWY3DPEHPK3PXP");
        let other = DataKeyCipher::new(Key::generate().unwrap());
        assert_eq!(
            other.open(&sealed).unwrap_err().key(),
            "keel.error.secret_open"
        );
        assert_eq!(
            cipher.open("not json").unwrap_err().key(),
            "keel.error.secret_open"
        );
        assert_eq!(format!("{cipher:?}"), "DataKeyCipher(***)");
    }

    #[test]
    fn a_sealed_value_is_plain_json() {
        let key = Key::generate().unwrap();
        let sealed = key.seal(b"v").unwrap();
        let json = serde_json::to_value(&sealed).unwrap();
        assert_eq!(json["alg"], "A256GCM");
        let back: Sealed = serde_json::from_value(json).unwrap();
        assert_eq!(key.open(&back).unwrap(), b"v");
    }
}
