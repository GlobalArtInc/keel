use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Id(Ulid);

#[derive(Debug, thiserror::Error)]
#[error("invalid identifier: {0}")]
pub struct InvalidId(String);

const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

impl Id {
    pub fn generate() -> Self {
        Self(Ulid::generate())
    }

    fn is_canonical(text: &str) -> bool {
        let bytes = text.as_bytes();
        bytes.len() == 26
            && (b'0'..=b'7').contains(&bytes[0])
            && bytes.iter().all(|byte| ALPHABET.contains(byte))
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl FromStr for Id {
    type Err = InvalidId;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if !Self::is_canonical(text) {
            return Err(InvalidId(text.to_owned()));
        }
        Ulid::from_string(text)
            .map(Self)
            .map_err(|_| InvalidId(text.to_owned()))
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = Cow::<str>::deserialize(deserializer)?;
        text.parse().map_err(de::Error::custom)
    }
}

impl From<Ulid> for Id {
    fn from(ulid: Ulid) -> Self {
        Self(ulid)
    }
}

impl From<Id> for Ulid {
    fn from(id: Id) -> Self {
        id.0
    }
}

impl JsonSchema for Id {
    fn schema_name() -> Cow<'static, str> {
        "Id".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::Id").into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": "^[0-7][0-9A-HJKMNP-TV-Z]{25}$",
            "minLength": 26,
            "maxLength": 26
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "01J9Q3W6B1R8N5X2K4T7M0V9Z3";

    #[test]
    fn id_serialises_as_its_26_char_string() {
        let id: Id = TEXT.parse().unwrap();
        assert_eq!(serde_json::to_value(id).unwrap(), serde_json::json!(TEXT));
        assert_eq!(id.to_string(), TEXT);
        assert_eq!(id.to_string().len(), 26);
    }

    #[test]
    fn id_deserialises_from_its_string() {
        let id: Id = serde_json::from_value(serde_json::json!(TEXT)).unwrap();
        assert_eq!(id.to_string(), TEXT);
    }

    #[test]
    fn malformed_id_fails_to_deserialise() {
        for text in [
            "01J9Q3",
            "01J9Q3W6B1R8N5X2K4T7M0V9Z3A",
            "01J9Q3W6B1R8N5X2K4T7M0V9ZI",
            "01j9q3w6b1r8n5x2k4t7m0v9z3",
            "8ZZZZZZZZZZZZZZZZZZZZZZZZZ",
            "",
        ] {
            assert!(text.parse::<Id>().is_err(), "{text:?}");
            assert!(
                serde_json::from_value::<Id>(serde_json::json!(text)).is_err(),
                "{text:?}"
            );
        }
        assert!(serde_json::from_value::<Id>(serde_json::json!(12)).is_err());
    }

    #[test]
    fn id_accepts_exactly_what_its_schema_pattern_accepts() {
        let pattern = schemars::schema_for!(Id)
            .get("pattern")
            .and_then(|pattern| pattern.as_str())
            .map(str::to_owned)
            .unwrap();
        let pattern = regex::Regex::new(&pattern).unwrap();
        for text in [
            TEXT,
            "00000000000000000000000000",
            "7ZZZZZZZZZZZZZZZZZZZZZZZZZ",
            "8ZZZZZZZZZZZZZZZZZZZZZZZZZ",
            "01j9q3w6b1r8n5x2k4t7m0v9z3",
            "01J9Q3W6B1R8N5X2K4T7M0V9ZI",
            "01J9Q3W6B1R8N5X2K4T7M0V9ZO",
            "01J9Q3W6B1R8N5X2K4T7M0V9ZU",
            "01J9Q3W6B1R8N5X2K4T7M0V9ZL",
            "01J9Q3W6B1R8N5X2K4T7M0V9Z",
        ] {
            assert_eq!(
                text.parse::<Id>().is_ok(),
                pattern.is_match(text),
                "{text:?}"
            );
        }
    }

    #[test]
    fn generated_ids_are_unique_and_sort_by_time() {
        let first = Id::generate();
        let second = Id::generate();
        assert_ne!(first, second);
        assert!(Ulid::from(first).timestamp_ms() <= Ulid::from(second).timestamp_ms());
    }

    #[test]
    fn schema_describes_a_fixed_length_string() {
        let schema = schemars::schema_for!(Id);
        assert_eq!(schema.get("type"), Some(&serde_json::json!("string")));
        assert_eq!(schema.get("minLength"), Some(&serde_json::json!(26)));
    }
}
