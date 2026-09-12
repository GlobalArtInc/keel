use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, TransactionTrait};
use serde_json::Value;

use crate::kv;
use crate::storage::lock;

pub const KEY: &str = "products.enabled";

#[derive(Debug, thiserror::Error)]
pub enum ProductsError {
    #[error("keel_kv holds an unreadable {KEY} value: {reason}")]
    Stored { reason: String },
    #[error("the database refused a query: {0}")]
    Database(#[from] DbErr),
}

pub async fn enabled(db: &DatabaseConnection) -> Result<Vec<String>, ProductsError> {
    let Some(value) = kv::get(db, KEY).await? else {
        return Ok(Vec::new());
    };
    parse(&value)
}

pub async fn set_enabled(db: &DatabaseConnection, ids: &[String]) -> Result<(), ProductsError> {
    let txn = db.begin().await?;
    txn.execute_unprepared(&format!("SELECT pg_advisory_xact_lock({})", lock::INSTANCE))
        .await?;
    let value = Value::Array(ids.iter().map(|id| Value::String(id.clone())).collect());
    kv::set(&txn, KEY, &value).await?;
    txn.commit().await?;
    Ok(())
}

fn parse(value: &Value) -> Result<Vec<String>, ProductsError> {
    let Value::Array(items) = value else {
        return Err(ProductsError::Stored {
            reason: "not an array".to_owned(),
        });
    };
    let mut ids = Vec::with_capacity(items.len());
    for item in items {
        match item.as_str() {
            Some(id) if !ids.iter().any(|known: &String| known == id) => ids.push(id.to_owned()),
            Some(_) => {}
            None => {
                return Err(ProductsError::Stored {
                    reason: format!("{item} is not a product id"),
                });
            }
        }
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_stored_value_is_a_list_of_ids_without_repeats() {
        assert_eq!(
            parse(&json!(["demo", "dns", "demo"])).unwrap(),
            ["demo", "dns"]
        );
        assert!(parse(&json!([])).unwrap().is_empty());
        let error = parse(&json!({ "demo": true })).unwrap_err();
        assert_eq!(
            error.to_string(),
            "keel_kv holds an unreadable products.enabled value: not an array"
        );
        let error = parse(&json!(["demo", 7])).unwrap_err();
        assert!(
            error.to_string().contains("7 is not a product id"),
            "{error}"
        );
    }
}
