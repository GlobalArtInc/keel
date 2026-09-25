use std::sync::Arc;

use keel_release::{KeyRing, ReleaseError, SecretKey, Signed};
use keel_sdk::system::LicenseKey;
use serde_json::Value;

pub struct Issuer {
    secret: SecretKey,
    ring: Arc<ThrowawayKey>,
}

struct ThrowawayKey(KeyRing);

impl Issuer {
    pub fn generate() -> Result<Self, ReleaseError> {
        let secret = SecretKey::generate("keel test license key")?;
        let ring = Arc::new(ThrowawayKey(KeyRing::new(secret.public(), Vec::new())));
        Ok(Self { secret, ring })
    }

    pub fn sign(&self, lease: &Value) -> Result<String, ReleaseError> {
        let payload = serde_json::to_vec_pretty(lease).map_err(ReleaseError::Serialise)?;
        Signed::seal(payload, &self.secret).to_json()
    }

    pub fn key(&self) -> Arc<dyn LicenseKey> {
        self.ring.clone()
    }
}

impl LicenseKey for ThrowawayKey {
    fn open(&self, signed: &[u8]) -> Result<Vec<u8>, String> {
        Signed::parse(signed)
            .and_then(|signed| signed.open(&self.0).map(<[u8]>::to_vec))
            .map_err(|error| error.to_string())
    }
}
