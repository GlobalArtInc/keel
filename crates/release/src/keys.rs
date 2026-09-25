use std::fmt;
use std::fs;
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::str::FromStr;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rand::TryRng;
use rand::rngs::SysRng;

use crate::ReleaseError;

const PUBLIC_KIND: &str = "ed25519";
const SECRET_KIND: &str = "ed25519-secret";
const KEY_LENGTH: usize = 32;
const RELEASE_KEY: &str = include_str!("../keys/release.pub");
const LICENSE_KEY: &str = include_str!("../keys/license.pub");
const PREVIOUS_KEYS: &[&str] = &[];

#[derive(Clone, PartialEq, Eq)]
pub struct PublicKey {
    key: VerifyingKey,
    comment: String,
}

pub struct SecretKey {
    key: SigningKey,
    comment: String,
}

pub struct KeyRing {
    current: PublicKey,
    previous: Vec<PublicKey>,
}

fn parse(text: &str, kind: &'static str) -> Result<([u8; KEY_LENGTH], String), ReleaseError> {
    let failed = |detail: String| ReleaseError::KeyText { kind, detail };
    let mut words = text.split_whitespace();
    match words.next() {
        Some(word) if word == kind => {}
        Some(word) => return Err(failed(format!("the first word is \"{word}\""))),
        None => return Err(failed("the text is empty".to_owned())),
    }
    let encoded = words
        .next()
        .ok_or_else(|| failed("the key bytes are missing".to_owned()))?;
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|error| failed(error.to_string()))?;
    let bytes: [u8; KEY_LENGTH] = bytes.try_into().map_err(|bytes: Vec<u8>| {
        failed(format!("{} bytes instead of {KEY_LENGTH}", bytes.len()))
    })?;
    let comment = words.collect::<Vec<_>>().join(" ");
    Ok((bytes, comment))
}

fn read(path: &Path) -> Result<String, ReleaseError> {
    fs::read_to_string(path).map_err(|source| ReleaseError::Read {
        path: path.to_owned(),
        source,
    })
}

fn write_new(path: &Path, text: &str, mode: u32) -> Result<(), ReleaseError> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
        .map_err(|source| match source.kind() {
            io::ErrorKind::AlreadyExists => ReleaseError::Exists {
                path: path.to_owned(),
            },
            _ => ReleaseError::Write {
                path: path.to_owned(),
                source,
            },
        })?;
    io::Write::write_all(&mut file, text.as_bytes()).map_err(|source| ReleaseError::Write {
        path: path.to_owned(),
        source,
    })
}

impl PublicKey {
    pub fn read(path: &Path) -> Result<Self, ReleaseError> {
        read(path)?.parse()
    }

    pub fn write(&self, path: &Path) -> Result<(), ReleaseError> {
        write_new(path, &format!("{self}\n"), 0o644)
    }

    pub(crate) fn verifies(&self, payload: &[u8], signature: &Signature) -> bool {
        self.key.verify_strict(payload, signature).is_ok()
    }
}

impl FromStr for PublicKey {
    type Err = ReleaseError;

    fn from_str(text: &str) -> Result<Self, ReleaseError> {
        let (bytes, comment) = parse(text, PUBLIC_KIND)?;
        let key = VerifyingKey::from_bytes(&bytes).map_err(|error| ReleaseError::KeyText {
            kind: PUBLIC_KIND,
            detail: error.to_string(),
        })?;
        Ok(Self { key, comment })
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{PUBLIC_KIND} {}", STANDARD.encode(self.key.to_bytes()))?;
        if !self.comment.is_empty() {
            write!(f, " {}", self.comment)?;
        }
        Ok(())
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({self})")
    }
}

impl SecretKey {
    pub fn generate(comment: &str) -> Result<Self, ReleaseError> {
        let mut bytes = [0u8; KEY_LENGTH];
        SysRng
            .try_fill_bytes(&mut bytes)
            .map_err(|error| ReleaseError::Random(error.to_string()))?;
        Ok(Self {
            key: SigningKey::from_bytes(&bytes),
            comment: comment.to_owned(),
        })
    }

    pub fn read(path: &Path) -> Result<Self, ReleaseError> {
        read(path)?.parse()
    }

    pub fn write(&self, path: &Path) -> Result<(), ReleaseError> {
        write_new(path, &format!("{self}\n"), 0o600)
    }

    pub fn public(&self) -> PublicKey {
        PublicKey {
            key: self.key.verifying_key(),
            comment: self.comment.clone(),
        }
    }

    pub(crate) fn sign(&self, payload: &[u8]) -> Signature {
        self.key.sign(payload)
    }
}

impl FromStr for SecretKey {
    type Err = ReleaseError;

    fn from_str(text: &str) -> Result<Self, ReleaseError> {
        let (bytes, comment) = parse(text, SECRET_KIND)?;
        Ok(Self {
            key: SigningKey::from_bytes(&bytes),
            comment,
        })
    }
}

impl fmt::Display for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{SECRET_KIND} {}", STANDARD.encode(self.key.to_bytes()))?;
        if !self.comment.is_empty() {
            write!(f, " {}", self.comment)?;
        }
        Ok(())
    }
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SecretKey({SECRET_KIND} *** {})", self.comment)
    }
}

impl KeyRing {
    pub fn new(current: PublicKey, previous: Vec<PublicKey>) -> Self {
        Self { current, previous }
    }

    pub fn compiled() -> Result<Self, ReleaseError> {
        let current = RELEASE_KEY.parse()?;
        let previous = PREVIOUS_KEYS
            .iter()
            .map(|text| text.parse())
            .collect::<Result<Vec<PublicKey>, ReleaseError>>()?;
        Ok(Self { current, previous })
    }

    pub fn license() -> Result<Self, ReleaseError> {
        Ok(Self::new(LICENSE_KEY.parse()?, Vec::new()))
    }

    pub(crate) fn verifies(&self, payload: &[u8], signature: &Signature) -> bool {
        std::iter::once(&self.current)
            .chain(&self.previous)
            .any(|key| key.verifies(payload, signature))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_pair_round_trips_through_its_text_with_the_comment() {
        let secret = SecretKey::generate("release 2026").unwrap();
        let text = secret.to_string();
        assert!(text.starts_with("ed25519-secret "), "{text}");
        assert!(text.ends_with(" release 2026"), "{text}");
        let parsed: SecretKey = text.parse().unwrap();
        assert_eq!(parsed.public(), secret.public());
        assert_eq!(parsed.comment, "release 2026");
        let public = secret.public();
        let parsed: PublicKey = public.to_string().parse().unwrap();
        assert_eq!(parsed, public);
        assert_eq!(parsed.comment, "release 2026");
        assert_eq!(
            format!("{secret:?}"),
            "SecretKey(ed25519-secret *** release 2026)"
        );
        assert!(!format!("{secret:?}").contains(&text[15..59]));
    }

    #[test]
    fn a_key_text_of_another_kind_or_length_is_refused() {
        let secret = SecretKey::generate("").unwrap();
        let public = secret.public().to_string();
        assert!(matches!(
            public.parse::<SecretKey>(),
            Err(ReleaseError::KeyText {
                kind: "ed25519-secret",
                ..
            })
        ));
        assert!(matches!(
            "ed25519 YWJj".parse::<PublicKey>(),
            Err(ReleaseError::KeyText { detail, .. }) if detail == "3 bytes instead of 32"
        ));
        assert!(matches!(
            "ed25519 not*base64".parse::<PublicKey>(),
            Err(ReleaseError::KeyText { .. })
        ));
        assert!(matches!(
            "".parse::<PublicKey>(),
            Err(ReleaseError::KeyText { detail, .. }) if detail == "the text is empty"
        ));
        assert!(matches!(
            "ed25519".parse::<PublicKey>(),
            Err(ReleaseError::KeyText { detail, .. }) if detail == "the key bytes are missing"
        ));
    }

    #[test]
    fn the_key_files_are_written_once_with_their_modes() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let secret = SecretKey::generate("t").unwrap();
        let secret_path = dir.path().join("release.key");
        let public_path = dir.path().join("release.pub");
        secret.write(&secret_path).unwrap();
        secret.public().write(&public_path).unwrap();
        assert_eq!(
            fs::metadata(&secret_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&public_path).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(
            SecretKey::read(&secret_path).unwrap().public(),
            PublicKey::read(&public_path).unwrap()
        );
        assert!(matches!(
            secret.write(&secret_path),
            Err(ReleaseError::Exists { path }) if path == secret_path
        ));
        assert!(matches!(
            PublicKey::read(&dir.path().join("missing.pub")),
            Err(ReleaseError::Read { .. })
        ));
    }

    #[test]
    fn the_compiled_release_key_parses() {
        let ring = KeyRing::compiled().unwrap();
        assert_eq!(ring.current.to_string().trim(), RELEASE_KEY.trim());
        assert!(ring.previous.is_empty());
    }

    #[test]
    fn the_compiled_license_key_parses_and_differs_from_the_release_key() {
        let ring = KeyRing::license().unwrap();
        assert_eq!(ring.current.to_string().trim(), LICENSE_KEY.trim());
        assert_ne!(ring.current, KeyRing::compiled().unwrap().current);
    }
}
