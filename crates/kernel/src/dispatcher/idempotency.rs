use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use keel_sdk::KeelError;
use keel_types::Id;
use schemars::{Schema, json_schema};
use sea_orm::{ConnectionTrait, DbBackend, Statement, Value as DbValue};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const HOURS_SETTING: &str = "keel.idempotency.hours";
pub const DEFAULT_HOURS: u32 = 24;
pub const MAX_HOURS: u32 = 720;
pub const KEY_MAX_LEN: usize = keel_types::IDEMPOTENCY_KEY_MAX_LEN;
pub const MALFORMED_KEY: &str = "keel.error.idempotency_key";
pub const REQUIRED_KEY: &str = "keel.error.idempotency_required";
pub const MISMATCH_KEY: &str = "keel.error.idempotency_mismatch";
pub const RACE_KEY: &str = "keel.error.idempotency_race";

const LOCK: &str = "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))";
const LOCK_PREFIX: &str = "keel_idempotency";

#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub func: String,
    pub request_hash: String,
    pub response: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Keyed {
    pub key: String,
    pub hash: String,
    pub ttl: Duration,
}

pub enum Recorded {
    Stored,
    Taken,
}

pub fn admit(key: &str) -> Result<(), KeelError> {
    if !key.is_empty()
        && key.len() <= KEY_MAX_LEN
        && key.bytes().all(|byte| (b' '..=b'~').contains(&byte))
    {
        return Ok(());
    }
    Err(KeelError::validation(MALFORMED_KEY).param("max", KEY_MAX_LEN))
}

pub fn hours_schema() -> Schema {
    json_schema!({ "type": "integer", "minimum": 1, "maximum": MAX_HOURS })
}

pub fn ttl(setting: Option<Value>) -> Duration {
    let hours = match setting {
        None => DEFAULT_HOURS,
        Some(value) => match serde_json::from_value::<u32>(value.clone()) {
            Ok(hours) => hours,
            Err(_) => {
                tracing::warn!(key = HOURS_SETTING, %value, default = DEFAULT_HOURS, "the setting is not a number of hours; the default applies");
                DEFAULT_HOURS
            }
        },
    };
    Duration::hours(i64::from(hours.clamp(1, MAX_HOURS)))
}

pub fn hash(params: &Value) -> String {
    let canonical = serde_json::to_string(&canonical(params)).unwrap_or_default();
    let digest = Sha256::digest(canonical.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let sorted: BTreeMap<&String, Value> = object
                .iter()
                .map(|(key, value)| (key, canonical(value)))
                .collect();
            Value::Object(
                sorted
                    .into_iter()
                    .map(|(key, value)| (key.clone(), value))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

pub async fn lookup<C: ConnectionTrait>(
    db: &C,
    principal: Id,
    key: &str,
    now: DateTime<Utc>,
) -> Result<Option<Record>, KeelError> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT func, request_hash, response_json FROM keel_idempotency WHERE principal_id = $1 AND key = $2 AND expires_at > $3",
            [
                DbValue::from(principal.to_string()),
                DbValue::from(key),
                DbValue::from(now),
            ],
        ))
        .await?;
    row.map(|row| {
        Ok(Record {
            func: row.try_get("", "func")?,
            request_hash: row.try_get("", "request_hash")?,
            response: row.try_get("", "response_json")?,
        })
    })
    .transpose()
}

pub async fn claim<C: ConnectionTrait>(
    transaction: &C,
    principal: Id,
    key: &str,
    now: DateTime<Utc>,
) -> Result<Option<Record>, KeelError> {
    transaction
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            LOCK,
            [DbValue::from(format!("{LOCK_PREFIX}:{principal}:{key}"))],
        ))
        .await?;
    lookup(transaction, principal, key, now).await
}

pub async fn record<C: ConnectionTrait>(
    db: &C,
    principal: Id,
    keyed: &Keyed,
    func: &str,
    response: &Value,
    now: DateTime<Utc>,
) -> Result<Recorded, KeelError> {
    let affected = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO keel_idempotency (principal_id, key, func, request_hash, status, response_json, created_at, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (principal_id, key) DO UPDATE \
             SET func = EXCLUDED.func, request_hash = EXCLUDED.request_hash, status = EXCLUDED.status, \
                 response_json = EXCLUDED.response_json, created_at = EXCLUDED.created_at, expires_at = EXCLUDED.expires_at \
             WHERE keel_idempotency.expires_at <= $7",
            [
                DbValue::from(principal.to_string()),
                DbValue::from(keyed.key.as_str()),
                DbValue::from(func),
                DbValue::from(keyed.hash.as_str()),
                DbValue::from("200"),
                DbValue::Json(Some(Box::new(response.clone()))),
                DbValue::from(now),
                DbValue::from(now + keyed.ttl),
            ],
        ))
        .await?
        .rows_affected();
    Ok(if affected == 1 {
        Recorded::Stored
    } else {
        Recorded::Taken
    })
}

pub async fn prune<C: ConnectionTrait>(db: &C, now: DateTime<Utc>) -> Result<u64, KeelError> {
    let pruned = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM keel_idempotency WHERE expires_at <= $1",
            [DbValue::from(now)],
        ))
        .await?
        .rows_affected();
    Ok(pruned)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_request_hash_ignores_key_order_and_sees_every_value() {
        let a = hash(&json!({ "title": "x", "nested": { "b": 1, "a": [1, { "z": 0, "y": 1 }] } }));
        let b = hash(&json!({ "nested": { "a": [1, { "y": 1, "z": 0 }], "b": 1 }, "title": "x" }));
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
        assert_ne!(
            a,
            hash(&json!({ "title": "y", "nested": { "b": 1, "a": [1, { "z": 0, "y": 1 }] } }))
        );
        assert_ne!(hash(&json!([1, 2])), hash(&json!([2, 1])));
    }

    #[test]
    fn a_key_is_one_to_255_printable_ascii_characters() {
        for key in [
            "k",
            "bill-9912-open",
            "a key with spaces",
            "~!@#$%^&*()_+{}|:\"<>?",
        ] {
            assert!(admit(key).is_ok(), "{key}");
        }
        assert!(admit(&"k".repeat(KEY_MAX_LEN)).is_ok());
        for key in [
            String::new(),
            "k".repeat(KEY_MAX_LEN + 1),
            "tab\there".to_owned(),
            "line\nbreak".to_owned(),
            "ключ".to_owned(),
            "del\u{7f}".to_owned(),
        ] {
            let error = admit(&key).expect_err(&key);
            assert_eq!(error.code(), keel_types::ErrorCode::Validation);
            assert_eq!(error.key(), MALFORMED_KEY);
            assert_eq!(error.params()["max"], json!(KEY_MAX_LEN));
            assert_eq!(error.field_path(), None);
        }
    }

    #[test]
    fn the_lifetime_follows_the_setting_within_one_to_720_hours_and_falls_back_to_24() {
        assert_eq!(ttl(None), Duration::hours(24));
        assert_eq!(ttl(Some(json!(2))), Duration::hours(2));
        assert_eq!(ttl(Some(json!(720))), Duration::hours(720));
        assert_eq!(ttl(Some(json!(0))), Duration::hours(1));
        assert_eq!(ttl(Some(json!(u32::MAX))), Duration::hours(720));
        assert_eq!(ttl(Some(json!("six"))), Duration::hours(24));
        assert_eq!(ttl(Some(json!(-3))), Duration::hours(24));
        assert_eq!(
            hours_schema().as_value(),
            &json!({ "type": "integer", "minimum": 1, "maximum": 720 })
        );
    }
}
