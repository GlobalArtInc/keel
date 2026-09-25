mod digest;
mod keys;
mod manifest;
mod signed;

use std::io;
use std::path::PathBuf;

pub use semver::Version;

pub use digest::{Digest, Hasher};
pub use keys::{KeyRing, PublicKey, SecretKey};
pub use manifest::{Channel, Manifest, archive_path, manifest_path};
pub use signed::Signed;

pub const CHANNEL_URL: &str = "https://dl.globalart.hu/keel/";

#[derive(Debug, thiserror::Error)]
pub enum ReleaseError {
    #[error("the key text is not \"{kind} <base64 of 32 bytes>\": {detail}")]
    KeyText { kind: &'static str, detail: String },
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
    #[error("{path} exists; a key is never overwritten")]
    Exists { path: PathBuf },
    #[error("the system's random source failed: {0}")]
    Random(String),
    #[error("the manifest is not a signed envelope: {0}")]
    Envelope(#[source] serde_json::Error),
    #[error("the envelope's {field} is not base64: {source}")]
    Base64 {
        field: &'static str,
        #[source]
        source: base64::DecodeError,
    },
    #[error("the envelope's signature is not {expected} bytes but {actual}")]
    SignatureLength { expected: usize, actual: usize },
    #[error("the signature matches none of the trusted keys")]
    Signature,
    #[error("the signed payload is not a release manifest: {0}")]
    Payload(#[source] serde_json::Error),
    #[error("the manifest could not be serialised: {0}")]
    Serialise(#[source] serde_json::Error),
    #[error("{version} is below min_upgrade_from {min_upgrade_from} of the manifest of {of}")]
    BelowMinimum {
        version: String,
        min_upgrade_from: String,
        of: String,
    },
    #[error("the archive's sha256 {actual} differs from the manifest's {expected}")]
    Digest { expected: String, actual: String },
    #[error("the channel \"{0}\" is neither stable nor beta")]
    Channel(String),
}
