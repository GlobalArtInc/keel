use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use keel_sdk::KeelError;
use keel_types::Id;
use sea_orm::{ConnectionTrait, DbBackend, Statement, Value as DbValue};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const TTL: Duration = Duration::hours(24);

#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub func: String,
    pub request_hash: String,
    pub response: Value,
}

pub enum Recorded {
    Stored,
    Taken,
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

pub async fn record<C: ConnectionTrait>(
    db: &C,
    principal: Id,
    key: &str,
    func: &str,
    request_hash: &str,
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
                DbValue::from(key),
                DbValue::from(func),
                DbValue::from(request_hash),
                DbValue::from("200"),
                DbValue::Json(Some(Box::new(response.clone()))),
                DbValue::from(now),
                DbValue::from(now + TTL),
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
}
