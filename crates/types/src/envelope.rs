use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, Deserializer};
use serde::ser::{SerializeStruct, Serializer};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::id::Id;
use crate::level::Level;
use crate::principal::ActorVia;

#[derive(Clone, Debug, PartialEq)]
pub enum Envelope<T> {
    Ok { data: T, meta: Meta },
    Err { error: CallError },
}

impl<T> Envelope<T> {
    pub fn ok(data: T, meta: Meta) -> Self {
        Envelope::Ok { data, meta }
    }

    pub fn err(error: CallError) -> Self {
        Envelope::Err { error }
    }

    pub fn status(&self) -> u16 {
        match self {
            Envelope::Ok { .. } => 200,
            Envelope::Err { error } => error.code.status(),
        }
    }
}

impl<T: Serialize> Serialize for Envelope<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Envelope::Ok { data, meta } => {
                let mut envelope = serializer.serialize_struct("Envelope", 3)?;
                envelope.serialize_field("ok", &true)?;
                envelope.serialize_field("data", data)?;
                envelope.serialize_field("meta", meta)?;
                envelope.end()
            }
            Envelope::Err { error } => {
                let mut envelope = serializer.serialize_struct("Envelope", 2)?;
                envelope.serialize_field("ok", &false)?;
                envelope.serialize_field("error", error)?;
                envelope.end()
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
struct RawEnvelope<T> {
    ok: bool,
    #[serde(default, deserialize_with = "present")]
    data: Option<T>,
    meta: Option<Meta>,
    error: Option<CallError>,
}

fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Envelope<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawEnvelope::<T>::deserialize(deserializer)?;
        match (raw.ok, raw.data, raw.meta, raw.error) {
            (true, Some(data), Some(meta), None) => Ok(Envelope::Ok { data, meta }),
            (false, None, None, Some(error)) => Ok(Envelope::Err { error }),
            (true, ..) => Err(de::Error::custom(
                "an ok envelope carries data and meta and no error",
            )),
            (false, ..) => Err(de::Error::custom(
                "an error envelope carries error and neither data nor meta",
            )),
        }
    }
}

impl<T: JsonSchema> JsonSchema for Envelope<T> {
    fn schema_name() -> Cow<'static, str> {
        format!("Envelope_for_{}", T::schema_name()).into()
    }

    fn schema_id() -> Cow<'static, str> {
        format!("{}::Envelope<{}>", module_path!(), T::schema_id()).into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "oneOf": [
                {
                    "type": "object",
                    "properties": {
                        "ok": { "const": true },
                        "data": generator.subschema_for::<T>(),
                        "meta": generator.subschema_for::<Meta>()
                    },
                    "required": ["ok", "data", "meta"]
                },
                {
                    "type": "object",
                    "properties": {
                        "ok": { "const": false },
                        "error": generator.subschema_for::<CallError>()
                    },
                    "required": ["ok", "error"]
                }
            ]
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<MetaPrincipal>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MetaPrincipal {
    pub id: Id,
    pub login: String,
    pub level: Level,
    pub account: Id,
    pub actor: Option<MetaActor>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MetaActor {
    pub id: Id,
    pub login: String,
    pub level: Level,
    pub via: ActorVia,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CallError {
    pub code: ErrorCode,
    pub key: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub params: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<FieldError>,
}

impl CallError {
    pub fn new(code: ErrorCode) -> Self {
        Self {
            code,
            key: code.key(),
            params: Map::new(),
            request_id: None,
            fields: Vec::new(),
        }
    }

    pub fn status(&self) -> u16 {
        self.code.status()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FieldError {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub params: Map<String, Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Validation,
    Forbidden,
    ForbiddenField,
    Unauthenticated,
    NotFound,
    Conflict,
    IdempotencyMismatch,
    LimitExceeded,
    License,
    RateLimited,
    Unavailable,
    Peer,
    Internal,
}

impl ErrorCode {
    pub const ALL: [ErrorCode; 13] = [
        ErrorCode::Validation,
        ErrorCode::Forbidden,
        ErrorCode::ForbiddenField,
        ErrorCode::Unauthenticated,
        ErrorCode::NotFound,
        ErrorCode::Conflict,
        ErrorCode::IdempotencyMismatch,
        ErrorCode::LimitExceeded,
        ErrorCode::License,
        ErrorCode::RateLimited,
        ErrorCode::Unavailable,
        ErrorCode::Peer,
        ErrorCode::Internal,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            ErrorCode::Validation => "validation",
            ErrorCode::Forbidden => "forbidden",
            ErrorCode::ForbiddenField => "forbidden_field",
            ErrorCode::Unauthenticated => "unauthenticated",
            ErrorCode::NotFound => "not_found",
            ErrorCode::Conflict => "conflict",
            ErrorCode::IdempotencyMismatch => "idempotency_mismatch",
            ErrorCode::LimitExceeded => "limit_exceeded",
            ErrorCode::License => "license",
            ErrorCode::RateLimited => "rate_limited",
            ErrorCode::Unavailable => "unavailable",
            ErrorCode::Peer => "peer",
            ErrorCode::Internal => "internal",
        }
    }

    pub const fn status(self) -> u16 {
        match self {
            ErrorCode::Validation => 422,
            ErrorCode::Forbidden | ErrorCode::ForbiddenField => 403,
            ErrorCode::Unauthenticated => 401,
            ErrorCode::NotFound => 404,
            ErrorCode::Conflict | ErrorCode::IdempotencyMismatch | ErrorCode::LimitExceeded => 409,
            ErrorCode::License => 402,
            ErrorCode::RateLimited => 429,
            ErrorCode::Unavailable => 503,
            ErrorCode::Peer => 502,
            ErrorCode::Internal => 500,
        }
    }

    pub fn key(self) -> String {
        format!("keel.error.{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ok_envelope_serialises_ok_data_and_meta() {
        let envelope = Envelope::ok(
            json!({ "id": "01J9Q3W6B1R8N5X2K4T7M0V9Z3" }),
            Meta {
                request_id: "req-1a2b".into(),
                principal: None,
            },
        );
        assert_eq!(
            serde_json::to_value(&envelope).unwrap(),
            json!({ "ok": true, "data": { "id": "01J9Q3W6B1R8N5X2K4T7M0V9Z3" }, "meta": { "requestId": "req-1a2b" } })
        );
        assert_eq!(envelope.status(), 200);
    }

    #[test]
    fn error_envelope_serialises_ok_false_and_error() {
        let mut error = CallError::new(ErrorCode::NotFound);
        error.request_id = Some("req-1".into());
        let envelope = Envelope::<()>::err(error);
        assert_eq!(
            serde_json::to_value(&envelope).unwrap(),
            json!({ "ok": false, "error": { "code": "not_found", "key": "keel.error.not_found", "requestId": "req-1" } })
        );
        assert_eq!(envelope.status(), 404);
    }

    #[test]
    fn envelope_with_unit_data_round_trips() {
        let envelope = Envelope::ok(
            (),
            Meta {
                request_id: "r".into(),
                principal: None,
            },
        );
        let wire = serde_json::to_value(&envelope).unwrap();
        assert_eq!(wire["data"], Value::Null);
        assert_eq!(
            serde_json::from_value::<Envelope<()>>(wire).unwrap(),
            envelope
        );
    }

    #[test]
    fn mixed_envelopes_are_rejected() {
        assert!(
            serde_json::from_value::<Envelope<Value>>(
                json!({ "ok": true, "error": { "code": "internal", "key": "k" } })
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<Envelope<Value>>(json!({ "ok": false, "data": 1 })).is_err()
        );
        assert!(
            serde_json::from_value::<Envelope<Value>>(json!({ "ok": true, "data": 1 })).is_err()
        );
        assert!(
            serde_json::from_value::<Envelope<Value>>(
                json!({ "data": 1, "meta": { "requestId": "r" } })
            )
            .is_err()
        );
    }

    #[test]
    fn unknown_error_code_fails_to_deserialise() {
        assert!(serde_json::from_value::<ErrorCode>(json!("teapot")).is_err());
        assert!(serde_json::from_value::<ErrorCode>(json!("NotFound")).is_err());
    }

    #[test]
    fn every_error_code_has_a_wire_name_and_a_status() {
        for code in ErrorCode::ALL {
            assert_eq!(serde_json::to_value(code).unwrap(), json!(code.as_str()));
            assert_eq!(
                serde_json::from_value::<ErrorCode>(json!(code.as_str())).unwrap(),
                code
            );
            assert!((400..=599).contains(&code.status()));
            assert_eq!(code.key(), format!("keel.error.{}", code.as_str()));
        }
    }

    #[test]
    fn envelope_schema_is_a_choice_between_ok_and_error() {
        let schema = schemars::schema_for!(Envelope<Value>);
        let value = schema.as_value();
        assert_eq!(value["title"], json!("Envelope_for_AnyValue"));
        let branches = value["oneOf"].as_array().unwrap();
        assert_eq!(branches.len(), 2);
        assert_eq!(branches[0]["properties"]["ok"]["const"], json!(true));
        assert_eq!(branches[1]["properties"]["ok"]["const"], json!(false));
        assert!(value["$defs"]["CallError"].is_object());
        assert!(value["$defs"]["Meta"].is_object());
    }
}
