use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::id::Id;

pub const CHALLENGE_PARAM: &str = "challenge";
pub const CHALLENGE_MIN_LEN: usize = 32;
pub const CHALLENGE_MAX_LEN: usize = 128;
pub const PROOF_CONTEXT: &str = "keel-instance-proof";
pub const PROOF_SCHEME: &str = "ed25519:";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InstanceInfo {
    pub instance_id: Id,
    pub keel: String,
    pub protocol: u32,
    pub products: Vec<String>,
    pub public_key: String,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub service_kinds: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProvenInstance {
    #[serde(flatten)]
    pub instance: InstanceInfo,
    pub proof: String,
}

impl InstanceInfo {
    pub fn proof_message(&self, challenge: &str) -> Vec<u8> {
        let signed = json!([
            challenge,
            self.instance_id.to_string(),
            self.public_key,
            self.keel,
            self.protocol,
            self.products,
        ]);
        format!("{PROOF_CONTEXT}\n{signed}").into_bytes()
    }
}

pub fn is_challenge(text: &str) -> bool {
    (CHALLENGE_MIN_LEN..=CHALLENGE_MAX_LEN).contains(&text.len())
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_handshake_travels_with_the_member_names_of_the_contract() {
        let info = InstanceInfo {
            instance_id: "01J9B00000000000000000000A".parse().unwrap(),
            keel: "1.2.0".to_owned(),
            protocol: crate::PROTOCOL,
            products: vec!["dns".to_owned(), "panel".to_owned()],
            public_key: "ed25519:MCowBQYDK2VwAyEA".to_owned(),
            features: vec!["peers".to_owned()],
            service_kinds: vec!["dns.zones".to_owned()],
        };
        let value = serde_json::to_value(&info).unwrap();
        assert_eq!(
            value,
            json!({
                "instanceId": "01J9B00000000000000000000A",
                "keel": "1.2.0",
                "protocol": 1,
                "products": ["dns", "panel"],
                "publicKey": "ed25519:MCowBQYDK2VwAyEA",
                "features": ["peers"],
                "serviceKinds": ["dns.zones"]
            })
        );
        assert_eq!(serde_json::from_value::<InstanceInfo>(value).unwrap(), info);
    }

    #[test]
    fn a_neighbour_without_features_or_service_kinds_is_still_read() {
        let info: InstanceInfo = serde_json::from_value(json!({
            "instanceId": "01J9B00000000000000000000A",
            "keel": "1.1.0",
            "protocol": 1,
            "products": [],
            "publicKey": "ed25519:MCowBQYDK2VwAyEA"
        }))
        .unwrap();
        assert!(info.features.is_empty());
        assert!(info.service_kinds.is_empty());
    }

    fn handshake() -> InstanceInfo {
        InstanceInfo {
            instance_id: "01J9B00000000000000000000A".parse().unwrap(),
            keel: "1.2.0".to_owned(),
            protocol: crate::PROTOCOL,
            products: vec!["dns".to_owned(), "panel".to_owned()],
            public_key: "ed25519:MCowBQYDK2VwAyEA".to_owned(),
            features: vec!["peers".to_owned()],
            service_kinds: vec!["dns.zones".to_owned()],
        }
    }

    #[test]
    fn a_proof_signs_the_challenge_the_identity_the_version_the_protocol_and_the_products_without_ambiguity()
     {
        let info = handshake();
        let challenge = "c".repeat(CHALLENGE_MIN_LEN);
        assert_eq!(
            String::from_utf8(info.proof_message(&challenge)).unwrap(),
            format!(
                "keel-instance-proof\n[\"{challenge}\",\"01J9B00000000000000000000A\",\"ed25519:MCowBQYDK2VwAyEA\",\"1.2.0\",1,[\"dns\",\"panel\"]]"
            )
        );
        let mut joined = info.clone();
        joined.products = vec!["dns,panel".to_owned()];
        assert_ne!(
            joined.proof_message(&challenge),
            info.proof_message(&challenge)
        );
        let mut far = info.clone();
        far.protocol += 2;
        assert_ne!(
            far.proof_message(&challenge),
            info.proof_message(&challenge)
        );
        let mut newer = info.clone();
        newer.keel = "9.9.9".to_owned();
        assert_ne!(
            newer.proof_message(&challenge),
            info.proof_message(&challenge)
        );
        let mut trimmed = info.clone();
        trimmed.features.clear();
        trimmed.service_kinds.clear();
        assert_eq!(
            trimmed.proof_message(&challenge),
            info.proof_message(&challenge)
        );
        assert_ne!(
            info.proof_message(&"d".repeat(CHALLENGE_MIN_LEN)),
            info.proof_message(&challenge)
        );
    }

    #[test]
    fn a_challenge_is_32_to_128_letters_digits_dashes_or_underscores() {
        for accepted in [
            "a".repeat(CHALLENGE_MIN_LEN),
            "Z9-_".repeat(CHALLENGE_MAX_LEN / 4),
            "0123456789abcdef".repeat(4),
        ] {
            assert!(is_challenge(&accepted), "{accepted}");
        }
        for refused in [
            String::new(),
            "a".repeat(CHALLENGE_MIN_LEN - 1),
            "a".repeat(CHALLENGE_MAX_LEN + 1),
            format!("{} ", "a".repeat(CHALLENGE_MIN_LEN)),
            format!("{}\n", "a".repeat(CHALLENGE_MIN_LEN)),
            format!("{}.", "a".repeat(CHALLENGE_MIN_LEN)),
            "ж".repeat(CHALLENGE_MIN_LEN),
        ] {
            assert!(!is_challenge(&refused), "{refused:?}");
        }
    }

    #[test]
    fn a_proven_handshake_is_the_handshake_with_its_proof_beside_it() {
        let proven = ProvenInstance {
            instance: handshake(),
            proof: "ed25519:c2lnbmF0dXJl".to_owned(),
        };
        let value = serde_json::to_value(&proven).unwrap();
        assert_eq!(value["instanceId"], json!("01J9B00000000000000000000A"));
        assert_eq!(value["proof"], json!("ed25519:c2lnbmF0dXJl"));
        assert_eq!(
            serde_json::from_value::<InstanceInfo>(value.clone()).unwrap(),
            handshake()
        );
        assert_eq!(
            serde_json::from_value::<ProvenInstance>(value).unwrap(),
            proven
        );
    }
}
