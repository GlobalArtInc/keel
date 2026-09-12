use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::Deserializer;
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

impl Serialize for Secret {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_none()
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self)
    }
}

impl JsonSchema for Secret {
    fn schema_name() -> Cow<'static, str> {
        "Secret".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::Secret").into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({ "type": "string", "writeOnly": true })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Serialize, Deserialize)]
    struct Row {
        name: String,
        key: Option<Secret>,
    }

    #[test]
    fn a_secret_is_read_from_a_string_and_never_written_back() {
        let row: Row =
            serde_json::from_value(json!({ "name": "a.org", "key": "hunter2" })).unwrap();
        assert_eq!(row.key.as_ref().unwrap().expose(), "hunter2");
        assert_eq!(
            serde_json::to_value(&row).unwrap(),
            json!({ "name": "a.org", "key": null })
        );
        assert_eq!(
            serde_json::to_value(Secret::new("hunter2")).unwrap(),
            json!(null)
        );
    }

    #[test]
    fn debug_output_is_redacted() {
        let secret = Secret::from("hunter2");
        assert_eq!(format!("{secret:?}"), "Secret(***)");
        assert_eq!(format!("{:?}", Some(secret.clone())), "Some(Secret(***))");
        assert_eq!(secret.into_inner(), "hunter2");
    }

    #[test]
    fn schema_is_a_write_only_string() {
        let schema = schemars::schema_for!(Secret);
        assert_eq!(schema.get("type"), Some(&json!("string")));
        assert_eq!(schema.get("writeOnly"), Some(&json!(true)));
    }
}
