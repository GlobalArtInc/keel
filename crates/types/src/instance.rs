use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::id::Id;

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
}
