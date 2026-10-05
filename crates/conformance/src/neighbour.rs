use std::io::Read;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
use keel_types::{Id, InstanceInfo, PROOF_SCHEME, PROTOCOL};

use crate::report::{Failure, fail};
use crate::signature::hex;

pub const PEER_TOKEN_PREFIX: &str = keel_client::PEER_PREFIX;
pub const WEBHOOK_SECRET_PREFIX: &str = "whs_";
pub const PUBLIC_KEY_SCHEME: &str = "ed25519:";
pub const FEATURE: &str = "peers";
const ENTROPY: &str = "/dev/urandom";
// https://www.rfc-editor.org/rfc/rfc8410#section-4
const SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

#[derive(Clone, Debug)]
pub struct Identity {
    pub id: Id,
    pub keel: String,
    pub key: SigningKey,
}

impl Identity {
    pub fn new(id: Id, keel: &str) -> Result<Self, Failure> {
        Ok(Self {
            id,
            keel: keel.to_owned(),
            key: SigningKey::from_bytes(&random()?),
        })
    }

    pub fn instance(&self) -> InstanceInfo {
        InstanceInfo {
            instance_id: self.id,
            keel: self.keel.clone(),
            protocol: PROTOCOL,
            products: Vec::new(),
            public_key: public_key(&self.key.verifying_key()),
            features: vec![FEATURE.to_owned()],
            service_kinds: Vec::new(),
        }
    }
}

pub fn random() -> Result<[u8; 32], Failure> {
    let mut bytes = [0u8; 32];
    std::fs::File::open(ENTROPY)
        .and_then(|mut source| source.read_exact(&mut bytes))
        .map_err(|error| fail(format!("{ENTROPY} could not be read: {error}")))?;
    Ok(bytes)
}

pub fn public_key(key: &VerifyingKey) -> String {
    let mut der = Vec::with_capacity(SPKI_PREFIX.len() + 32);
    der.extend_from_slice(&SPKI_PREFIX);
    der.extend_from_slice(key.as_bytes());
    format!("{PUBLIC_KEY_SCHEME}{}", STANDARD.encode(der))
}

pub fn challenge() -> Result<String, Failure> {
    Ok(hex(&random()?))
}

pub fn proves(instance: &InstanceInfo, challenge: &str, proof: &str) -> bool {
    let key = instance
        .public_key
        .strip_prefix(PUBLIC_KEY_SCHEME)
        .and_then(|encoded| STANDARD.decode(encoded).ok())
        .and_then(|der| der.strip_prefix(&SPKI_PREFIX).map(<[u8]>::to_vec))
        .and_then(|raw| <[u8; 32]>::try_from(raw).ok())
        .and_then(|raw| VerifyingKey::from_bytes(&raw).ok());
    let signature = proof
        .strip_prefix(PROOF_SCHEME)
        .and_then(|encoded| STANDARD.decode(encoded).ok())
        .and_then(|raw| <[u8; 64]>::try_from(raw).ok())
        .map(|raw| Signature::from_bytes(&raw));
    match (key, signature) {
        (Some(key), Some(signature)) => key
            .verify_strict(&instance.proof_message(challenge), &signature)
            .is_ok(),
        _ => false,
    }
}

pub fn peer_token() -> Result<String, Failure> {
    Ok(format!("{PEER_TOKEN_PREFIX}{}", hex(&random()?)))
}

pub fn webhook_secret() -> Result<String, Failure> {
    Ok(format!("{WEBHOOK_SECRET_PREFIX}{}", hex(&random()?)))
}

pub fn is_peer_token(token: &str) -> bool {
    token
        .strip_prefix(PEER_TOKEN_PREFIX)
        .is_some_and(|rest| rest.len() == 64 && rest.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::Signer;

    use super::*;

    fn published(instance: &InstanceInfo) -> VerifyingKey {
        let der = STANDARD
            .decode(
                instance
                    .public_key
                    .strip_prefix(PUBLIC_KEY_SCHEME)
                    .expect("the scheme"),
            )
            .expect("base64");
        assert_eq!(der.len(), 44);
        assert_eq!(der[..12], SPKI_PREFIX);
        let raw: [u8; 32] = der[12..].try_into().expect("32 bytes of key");
        VerifyingKey::from_bytes(&raw).expect("an Ed25519 point")
    }

    #[test]
    fn a_neighbour_presents_a_fresh_ed25519_key_in_the_form_of_the_handshake() {
        let first = Identity::new(Id::generate(), "0.3.0").unwrap();
        let second = Identity::new(first.id, "0.3.0").unwrap();
        let instance = first.instance();
        assert_eq!(instance, first.instance());
        assert_eq!(instance.instance_id, second.instance().instance_id);
        assert_ne!(instance.public_key, second.instance().public_key);
        assert_eq!(instance.protocol, PROTOCOL);
        assert_eq!(instance.keel, "0.3.0");
        assert!(instance.products.is_empty());
        assert_eq!(published(&instance), first.key.verifying_key());
    }

    #[test]
    fn a_neighbour_keeps_the_signing_key_behind_its_handshake_so_it_can_prove_possession() {
        let identity = Identity::new(Id::generate(), "0.3.0").unwrap();
        let challenge = b"keel peer challenge 01J9EV5K";
        let signature = identity.key.sign(challenge);
        let key = published(&identity.instance());
        key.verify_strict(challenge, &signature).unwrap();
        assert!(key.verify_strict(b"another challenge", &signature).is_err());
        let other = Identity::new(identity.id, "0.3.0").unwrap();
        assert!(
            published(&other.instance())
                .verify_strict(challenge, &signature)
                .is_err()
        );
    }

    #[test]
    fn a_proof_of_the_instance_key_verifies_for_its_challenge_and_handshake_alone() {
        let identity = Identity::new(Id::generate(), "0.4.0").unwrap();
        let instance = identity.instance();
        let challenge = challenge().unwrap();
        assert!(keel_types::is_challenge(&challenge));
        let signature = identity.key.sign(&instance.proof_message(&challenge));
        let proof = format!("{PROOF_SCHEME}{}", STANDARD.encode(signature.to_bytes()));
        assert!(proves(&instance, &challenge, &proof));
        let flipped = if challenge.starts_with('0') { '1' } else { '0' };
        let another = format!("{flipped}{}", &challenge[1..]);
        assert_ne!(another, challenge);
        assert!(!proves(&instance, &another, &proof));
        let mut far = instance.clone();
        far.protocol += 2;
        assert!(!proves(&far, &challenge, &proof));
        let mut newer = instance.clone();
        newer.keel = "0.4.1".to_owned();
        assert!(!proves(&newer, &challenge, &proof));
        let other = Identity::new(identity.id, "0.4.0").unwrap().instance();
        assert!(!proves(&other, &challenge, &proof));
        assert!(!proves(&instance, &challenge, "ed25519:"));
        assert!(!proves(&instance, &challenge, &proof[PROOF_SCHEME.len()..]));
    }

    #[test]
    fn a_peer_token_is_the_prefix_and_thirty_two_random_bytes_as_hex() {
        let token = peer_token().unwrap();
        assert!(is_peer_token(&token), "{token}");
        assert_ne!(peer_token().unwrap(), token);
        assert!(webhook_secret().unwrap().starts_with(WEBHOOK_SECRET_PREFIX));
        assert!(!is_peer_token("kpt_short"));
        assert!(!is_peer_token(&format!("{token}0")));
        assert!(!is_peer_token(&format!(
            "{PEER_TOKEN_PREFIX}{}",
            "g".repeat(64)
        )));
        assert!(!is_peer_token(&token.replacen("kpt_", "kat_", 1)));
    }
}
