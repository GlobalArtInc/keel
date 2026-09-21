use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::Signature;
use serde::{Deserialize, Serialize};

use crate::{KeyRing, Manifest, ReleaseError, SecretKey};

#[derive(Serialize, Deserialize)]
struct Envelope {
    payload: String,
    signature: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signed {
    payload: Vec<u8>,
    signature: Signature,
}

impl Signed {
    pub fn sign(manifest: &Manifest, key: &SecretKey) -> Result<Self, ReleaseError> {
        let payload = serde_json::to_vec_pretty(manifest).map_err(ReleaseError::Serialise)?;
        let signature = key.sign(&payload);
        Ok(Self { payload, signature })
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, ReleaseError> {
        let envelope: Envelope = serde_json::from_slice(bytes).map_err(ReleaseError::Envelope)?;
        let payload = STANDARD
            .decode(envelope.payload)
            .map_err(|source| ReleaseError::Base64 {
                field: "payload",
                source,
            })?;
        let signature =
            STANDARD
                .decode(envelope.signature)
                .map_err(|source| ReleaseError::Base64 {
                    field: "signature",
                    source,
                })?;
        let signature: [u8; Signature::BYTE_SIZE] =
            signature
                .try_into()
                .map_err(|bytes: Vec<u8>| ReleaseError::SignatureLength {
                    expected: Signature::BYTE_SIZE,
                    actual: bytes.len(),
                })?;
        Ok(Self {
            payload,
            signature: Signature::from_bytes(&signature),
        })
    }

    pub fn to_json(&self) -> Result<String, ReleaseError> {
        let envelope = Envelope {
            payload: STANDARD.encode(&self.payload),
            signature: STANDARD.encode(self.signature.to_bytes()),
        };
        let mut text = serde_json::to_string_pretty(&envelope).map_err(ReleaseError::Serialise)?;
        text.push('\n');
        Ok(text)
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub fn verify(&self, keys: &KeyRing) -> Result<Manifest, ReleaseError> {
        if !keys.verifies(&self.payload, &self.signature) {
            return Err(ReleaseError::Signature);
        }
        serde_json::from_slice(&self.payload).map_err(ReleaseError::Payload)
    }
}

#[cfg(test)]
mod tests {
    use semver::Version;

    use super::*;
    use crate::Digest;

    fn manifest() -> Manifest {
        Manifest {
            version: Version::new(0, 2, 0),
            sha256: Digest::of(b"archive").hex(),
            min_upgrade_from: Version::new(0, 1, 0),
            contract: 1,
            notes: "Second release.\nWith two lines and ünïcödé.".to_owned(),
        }
    }

    #[test]
    fn a_manifest_signed_with_the_release_key_verifies_and_a_changed_byte_does_not() {
        let key = SecretKey::generate("release").unwrap();
        let ring = KeyRing::new(key.public(), Vec::new());
        let signed = Signed::sign(&manifest(), &key).unwrap();
        let json = signed.to_json().unwrap();
        let parsed = Signed::parse(json.as_bytes()).unwrap();
        assert_eq!(parsed, signed);
        assert_eq!(parsed.verify(&ring).unwrap(), manifest());

        let mut envelope: serde_json::Value = serde_json::from_str(&json).unwrap();
        let mut payload = STANDARD
            .decode(envelope["payload"].as_str().unwrap())
            .unwrap();
        let position = payload.iter().position(|byte| *byte == b'1').unwrap();
        payload[position] = b'2';
        envelope["payload"] = serde_json::Value::String(STANDARD.encode(&payload));
        let tampered = Signed::parse(envelope.to_string().as_bytes()).unwrap();
        assert!(matches!(
            tampered.verify(&ring),
            Err(ReleaseError::Signature)
        ));

        let other = SecretKey::generate("other").unwrap();
        let foreign = Signed::sign(&manifest(), &other).unwrap();
        assert!(matches!(
            foreign.verify(&ring),
            Err(ReleaseError::Signature)
        ));
    }

    #[test]
    fn a_manifest_signed_with_a_previous_key_still_verifies() {
        let old = SecretKey::generate("2025").unwrap();
        let new = SecretKey::generate("2026").unwrap();
        let ring = KeyRing::new(new.public(), vec![old.public()]);
        let signed = Signed::sign(&manifest(), &old).unwrap();
        assert_eq!(signed.verify(&ring).unwrap(), manifest());
        let current_only = KeyRing::new(new.public(), Vec::new());
        assert!(matches!(
            signed.verify(&current_only),
            Err(ReleaseError::Signature)
        ));
    }

    #[test]
    fn an_envelope_that_is_not_the_shape_is_refused_before_any_key_is_tried() {
        assert!(matches!(
            Signed::parse(b"not json"),
            Err(ReleaseError::Envelope(_))
        ));
        assert!(matches!(
            Signed::parse(br#"{"payload":""}"#),
            Err(ReleaseError::Envelope(_))
        ));
        assert!(matches!(
            Signed::parse(br#"{"payload":"","signature":"","later":1}"#),
            Err(ReleaseError::SignatureLength {
                expected: 64,
                actual: 0
            })
        ));
        assert!(matches!(
            Signed::parse(br#"{"payload":"***","signature":""}"#),
            Err(ReleaseError::Base64 {
                field: "payload",
                ..
            })
        ));
        assert!(matches!(
            Signed::parse(br#"{"payload":"","signature":"YWJj"}"#),
            Err(ReleaseError::SignatureLength {
                expected: 64,
                actual: 3
            })
        ));
        let key = SecretKey::generate("k").unwrap();
        let ring = KeyRing::new(key.public(), Vec::new());
        let signed = Signed {
            payload: b"{\"version\":1}".to_vec(),
            signature: key.sign(b"{\"version\":1}"),
        };
        assert!(matches!(
            signed.verify(&ring),
            Err(ReleaseError::Payload(_))
        ));
    }
}
