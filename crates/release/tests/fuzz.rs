use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use keel_release::{Digest, KeyRing, Manifest, ReleaseError, SecretKey, Signed, Version};
use proptest::prelude::*;
use proptest::test_runner::Config;
use serde_json::Value;

const CASES: u32 = 2048;

fn config() -> Config {
    let from_environment = Config::default();
    Config {
        cases: from_environment.cases.max(CASES),
        failure_persistence: None,
        ..from_environment
    }
}

fn key() -> SecretKey {
    SecretKey::generate("fuzz").unwrap()
}

fn ring(key: &SecretKey) -> KeyRing {
    KeyRing::new(
        key.public(),
        vec![SecretKey::generate("previous").unwrap().public()],
    )
}

fn manifest() -> impl Strategy<Value = Manifest> {
    (
        (0u64..50, 0u64..50, 0u64..50),
        (0u64..50, 0u64..50, 0u64..50),
        any::<u32>(),
        any::<String>(),
        prop::collection::vec(any::<u8>(), 0..64),
    )
        .prop_map(|(version, minimum, contract, notes, archive)| Manifest {
            version: Version::new(version.0, version.1, version.2),
            sha256: Digest::of(&archive).hex(),
            min_upgrade_from: Version::new(minimum.0, minimum.1, minimum.2),
            contract,
            notes,
        })
}

fn envelope_of(signed: &Signed) -> Value {
    serde_json::from_str(&signed.to_json().unwrap()).unwrap()
}

fn decoded(envelope: &Value, field: &str) -> Vec<u8> {
    STANDARD.decode(envelope[field].as_str().unwrap()).unwrap()
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn any_bytes_are_refused_by_the_verifier_without_a_panic(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
        let key = key();
        let verified = Signed::parse(&bytes).and_then(|signed| signed.verify(&ring(&key)));
        prop_assert!(verified.is_err());
    }

    #[test]
    fn any_envelope_of_strings_is_refused_by_the_verifier(payload in any::<String>(), signature in any::<String>()) {
        let key = key();
        let text = serde_json::json!({ "payload": payload, "signature": signature }).to_string();
        let verified = Signed::parse(text.as_bytes()).and_then(|signed| signed.verify(&ring(&key)));
        prop_assert!(verified.is_err());
    }

    #[test]
    fn a_signed_manifest_round_trips_through_its_envelope(manifest in manifest()) {
        let key = key();
        let text = Signed::sign(&manifest, &key).unwrap().to_json().unwrap();
        let verified = Signed::parse(text.as_bytes()).unwrap().verify(&ring(&key)).unwrap();
        prop_assert_eq!(verified, manifest);
    }

    #[test]
    fn a_changed_bit_of_the_payload_or_the_signature_never_verifies(
        manifest in manifest(),
        in_signature in any::<bool>(),
        at in any::<prop::sample::Index>(),
        bit in 0u8..8,
    ) {
        let key = key();
        let mut envelope = envelope_of(&Signed::sign(&manifest, &key).unwrap());
        let field = if in_signature { "signature" } else { "payload" };
        let mut bytes = decoded(&envelope, field);
        let position = at.index(bytes.len());
        bytes[position] ^= 1 << bit;
        envelope[field] = Value::String(STANDARD.encode(&bytes));
        let verified = Signed::parse(envelope.to_string().as_bytes())
            .and_then(|signed| signed.verify(&ring(&key)));
        let refused = matches!(
            verified,
            Err(ReleaseError::Signature | ReleaseError::SignatureLength { .. })
        );
        prop_assert!(refused);
    }

    #[test]
    fn an_envelope_with_its_bytes_changed_verifies_only_as_the_signed_manifest(
        manifest in manifest(),
        edits in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>(), 0..3u8), 1..6),
    ) {
        let key = key();
        let mut bytes = Signed::sign(&manifest, &key).unwrap().to_json().unwrap().into_bytes();
        for (at, byte, kind) in edits {
            let position = at.index(bytes.len().max(1));
            match kind {
                0 => bytes[position] = byte,
                1 => bytes.insert(position, byte),
                _ => {
                    bytes.remove(position);
                }
            }
            if bytes.is_empty() {
                bytes.push(byte);
            }
        }
        if let Ok(verified) = Signed::parse(&bytes).and_then(|signed| signed.verify(&ring(&key))) {
            prop_assert_eq!(verified, manifest);
        }
    }

    #[test]
    fn a_manifest_signed_by_a_key_outside_the_ring_never_verifies(manifest in manifest()) {
        let key = key();
        let stranger = SecretKey::generate("stranger").unwrap();
        let text = Signed::sign(&manifest, &stranger).unwrap().to_json().unwrap();
        let verified = Signed::parse(text.as_bytes()).unwrap().verify(&ring(&key));
        let refused = matches!(verified, Err(ReleaseError::Signature));
        prop_assert!(refused);
    }
}
