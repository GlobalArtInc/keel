use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{PUBLIC_KEY_LENGTH, SIGNATURE_LENGTH, Signature, VerifyingKey};
use keel_sdk::types::{InstanceInfo, PROOF_SCHEME};
use keel_sdk::{KeelError, Result};
use rand::TryRng;
use rand::rngs::SysRng;

use super::codes::ERROR_ENTROPY;

pub const CHALLENGE_BYTES: usize = 32;
// https://www.rfc-editor.org/rfc/rfc8410#section-4
const SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

pub fn challenge() -> Result<String> {
    let mut bytes = [0u8; CHALLENGE_BYTES];
    SysRng.try_fill_bytes(&mut bytes).map_err(|error| {
        KeelError::internal(ERROR_ENTROPY).source(std::io::Error::other(error.to_string()))
    })?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn key_of(public_key: &str) -> Option<VerifyingKey> {
    let der = STANDARD
        .decode(public_key.strip_prefix(PROOF_SCHEME)?)
        .ok()?;
    let raw: [u8; PUBLIC_KEY_LENGTH] = der.strip_prefix(&SPKI_PREFIX)?.try_into().ok()?;
    VerifyingKey::from_bytes(&raw).ok()
}

fn signature_of(proof: &str) -> Option<Signature> {
    let bytes = STANDARD.decode(proof.strip_prefix(PROOF_SCHEME)?).ok()?;
    let raw: [u8; SIGNATURE_LENGTH] = bytes.try_into().ok()?;
    Some(Signature::from_bytes(&raw))
}

pub fn proves(instance: &InstanceInfo, challenge: &str, proof: &str) -> bool {
    let (Some(key), Some(signature)) = (key_of(&instance.public_key), signature_of(proof)) else {
        return false;
    };
    key.verify_strict(&instance.proof_message(challenge), &signature)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signer, SigningKey};
    use keel_sdk::types::{Id, PROTOCOL, is_challenge};

    use super::*;

    fn signed_by(key: &SigningKey) -> InstanceInfo {
        let mut der = SPKI_PREFIX.to_vec();
        der.extend_from_slice(key.verifying_key().as_bytes());
        InstanceInfo {
            instance_id: Id::generate(),
            keel: "0.4.0".into(),
            protocol: PROTOCOL,
            products: vec!["dns".into()],
            public_key: format!("{PROOF_SCHEME}{}", STANDARD.encode(der)),
            features: Vec::new(),
            service_kinds: Vec::new(),
        }
    }

    fn prove(key: &SigningKey, instance: &InstanceInfo, challenge: &str) -> String {
        let signature = key.sign(&instance.proof_message(challenge));
        format!("{PROOF_SCHEME}{}", STANDARD.encode(signature.to_bytes()))
    }

    #[test]
    fn a_challenge_is_thirty_two_random_bytes_as_hex_the_handshake_accepts() {
        let first = challenge().unwrap();
        assert_eq!(first.len(), CHALLENGE_BYTES * 2);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(is_challenge(&first));
        assert_ne!(challenge().unwrap(), first);
    }

    #[test]
    fn a_proof_holds_for_its_key_its_challenge_and_the_handshake_it_signed_alone() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let instance = signed_by(&key);
        let challenge = "a".repeat(64);
        let proof = prove(&key, &instance, &challenge);
        assert!(proves(&instance, &challenge, &proof));
        assert!(!proves(&instance, &"b".repeat(64), &proof));
        let mut far = instance.clone();
        far.protocol += 2;
        assert!(!proves(&far, &challenge, &proof));
        let mut offering = instance.clone();
        offering.products.push("panel".into());
        assert!(!proves(&offering, &challenge, &proof));
        let mut newer = instance.clone();
        newer.keel = "0.4.1".into();
        assert!(!proves(&newer, &challenge, &proof));
        let copied = InstanceInfo {
            public_key: signed_by(&SigningKey::from_bytes(&[9u8; 32])).public_key,
            ..instance.clone()
        };
        assert!(!proves(&copied, &challenge, &proof));
        let other = SigningKey::from_bytes(&[9u8; 32]);
        assert!(!proves(
            &instance,
            &challenge,
            &prove(&other, &instance, &challenge)
        ));
        for broken in [
            String::new(),
            "ed25519:".to_owned(),
            "ed25519:!!!".to_owned(),
            proof.trim_start_matches(PROOF_SCHEME).to_owned(),
            format!("{PROOF_SCHEME}{}", STANDARD.encode([0u8; 63])),
            format!("{PROOF_SCHEME}{}", STANDARD.encode([0u8; 64])),
        ] {
            assert!(!proves(&instance, &challenge, &broken), "{broken}");
        }
        for key in [
            "",
            "ed25519:",
            "ed25519:MCowBQYDK2VwAyEA",
            "rsa:MCowBQYDK2VwAyEA",
        ] {
            let unreadable = InstanceInfo {
                public_key: key.to_owned(),
                ..instance.clone()
            };
            assert!(!proves(&unreadable, &challenge, &proof), "{key}");
        }
    }
}
