use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;
use std::sync::Mutex;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use ulid::{Generator, Ulid};

static GENERATOR: Mutex<Generator> = Mutex::new(Generator::new());

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Id(Ulid);

#[derive(Debug, thiserror::Error)]
#[error("invalid identifier: {0}")]
pub struct InvalidId(String);

const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

impl Id {
    pub const ROOT_ACCOUNT: Id = Id(Ulid::nil());

    pub fn generate() -> Self {
        let mut generator = GENERATOR
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let ulid = match generator.generate() {
            Ok(ulid) => ulid,
            Err(overflow) => overflow.commit_overflow_increment(),
        };
        Self(ulid)
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
    fn the_root_account_is_the_nil_identifier() {
        assert_eq!(Id::ROOT_ACCOUNT, Id::from(Ulid::nil()));
        assert_eq!(Id::ROOT_ACCOUNT.to_string(), "00000000000000000000000000");
        assert_eq!(
            "00000000000000000000000000".parse::<Id>().unwrap(),
            Id::ROOT_ACCOUNT
        );
    }

    #[test]
    fn generated_ids_are_unique_and_sort_by_time() {
        let first = Id::generate();
        let second = Id::generate();
        assert_ne!(first, second);
        assert!(Ulid::from(first).timestamp_ms() <= Ulid::from(second).timestamp_ms());
    }

    #[test]
    fn ids_generated_within_one_millisecond_sort_in_generation_order() {
        let ids: Vec<Id> = (0..10_000).map(|_| Id::generate()).collect();
        for pair in ids.windows(2) {
            assert!(pair[0] < pair[1], "{} then {}", pair[0], pair[1]);
            assert_eq!(
                pair[0].to_string().cmp(&pair[1].to_string()),
                std::cmp::Ordering::Less
            );
        }
    }

    #[test]
    fn ids_generated_from_several_threads_are_unique() {
        let handles: Vec<_> = (0..8)
            .map(|_| std::thread::spawn(|| (0..2_000).map(|_| Id::generate()).collect::<Vec<Id>>()))
            .collect();
        let mut all: Vec<Id> = handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap())
            .collect();
        let count = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), count);
    }

    #[test]
    fn schema_describes_a_fixed_length_string() {
        let schema = schemars::schema_for!(Id);
        assert_eq!(schema.get("type"), Some(&serde_json::json!("string")));
        assert_eq!(schema.get("minLength"), Some(&serde_json::json!(26)));
    }
}
