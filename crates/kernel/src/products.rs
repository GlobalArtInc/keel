use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use keel_sdk::ProductManifest;
use sea_orm::{ConnectionTrait, DbErr};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::kv;
use crate::storage::lock;

pub(crate) const KEY: &str = "products";
pub const INSTALL: &str = "install";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Enabled {
    pub enabled_at: DateTime<Utc>,
    pub by: String,
}

pub type EnabledSet = BTreeMap<String, Enabled>;

pub(crate) struct Composed {
    manifests: Vec<&'static ProductManifest>,
}

impl Composed {
    pub(crate) fn new(manifests: &[&'static ProductManifest]) -> Self {
        Self {
            manifests: manifests.to_vec(),
        }
    }

    pub(crate) fn manifests(&self) -> &[&'static ProductManifest] {
        &self.manifests
    }

    pub(crate) fn manifest(&self, id: &str) -> Option<&'static ProductManifest> {
        self.manifests
            .iter()
            .copied()
            .find(|manifest| manifest.id == id)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProductsError {
    #[error("keel_kv holds an unreadable {KEY} value: {reason}")]
    Stored { reason: String },
    #[error(
        "product {by} reaches {id} through its local bridge; disabling {id} moves {by} to its remote side, which answers unavailable until a neighbour is paired; pass --switch-remote to accept that"
    )]
    Bridged { id: String, by: String },
    #[error("the database refused a query: {0}")]
    Database(#[from] DbErr),
}

pub async fn enabled<C: ConnectionTrait>(db: &C) -> Result<EnabledSet, ProductsError> {
    let Some(value) = kv::get(db, KEY).await? else {
        return Ok(EnabledSet::new());
    };
    parse(&value)
}

pub async fn enable<C: ConnectionTrait>(
    tx: &C,
    id: &str,
    by: &str,
    now: DateTime<Utc>,
) -> Result<bool, ProductsError> {
    let mut set = locked(tx).await?;
    if set.contains_key(id) {
        return Ok(false);
    }
    set.insert(
        id.to_owned(),
        Enabled {
            enabled_at: now,
            by: by.to_owned(),
        },
    );
    write(tx, &set).await?;
    Ok(true)
}

pub async fn disable<C: ConnectionTrait>(
    tx: &C,
    id: &str,
    carried: &[&'static ProductManifest],
    switch_remote: bool,
) -> Result<bool, ProductsError> {
    let mut set = locked(tx).await?;
    if !set.contains_key(id) {
        return Ok(false);
    }
    if !switch_remote && let Some(by) = bridged_by(carried, &set, id) {
        return Err(ProductsError::Bridged {
            id: id.to_owned(),
            by: by.to_owned(),
        });
    }
    set.remove(id);
    write(tx, &set).await?;
    Ok(true)
}

fn bridged_by(
    carried: &[&'static ProductManifest],
    set: &EnabledSet,
    id: &str,
) -> Option<&'static str> {
    carried
        .iter()
        .find(|manifest| {
            manifest.id != id
                && set.contains_key(manifest.id)
                && manifest.bridges.iter().any(|bridge| bridge.to == id)
        })
        .map(|manifest| manifest.id)
}

async fn locked<C: ConnectionTrait>(tx: &C) -> Result<EnabledSet, ProductsError> {
    tx.execute_unprepared(&format!("SELECT pg_advisory_xact_lock({})", lock::INSTANCE))
        .await?;
    enabled(tx).await
}

async fn write<C: ConnectionTrait>(tx: &C, set: &EnabledSet) -> Result<(), ProductsError> {
    let value = serde_json::to_value(set).map_err(|error| ProductsError::Stored {
        reason: error.to_string(),
    })?;
    kv::set(tx, KEY, &value).await?;
    Ok(())
}

fn parse(value: &Value) -> Result<EnabledSet, ProductsError> {
    if !value.is_object() {
        return Err(ProductsError::Stored {
            reason: "not an object".to_owned(),
        });
    }
    serde_json::from_value(value.clone()).map_err(|error| ProductsError::Stored {
        reason: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_stored_value_maps_product_ids_to_when_and_by_whom_they_were_enabled() {
        let set = parse(&json!({
            "demo": { "enabledAt": "2026-09-12T10:00:00Z", "by": "install" },
            "dns": { "enabledAt": "2026-09-13T08:30:00+02:00", "by": "alice" }
        }))
        .unwrap();
        assert_eq!(set.keys().collect::<Vec<_>>(), ["demo", "dns"]);
        assert_eq!(set["demo"].by, INSTALL);
        assert_eq!(
            set["demo"].enabled_at.to_rfc3339(),
            "2026-09-12T10:00:00+00:00"
        );
        assert_eq!(
            set["dns"].enabled_at.to_rfc3339(),
            "2026-09-13T06:30:00+00:00"
        );
        assert!(parse(&json!({})).unwrap().is_empty());
        let error = parse(&json!(["demo"])).unwrap_err();
        assert_eq!(
            error.to_string(),
            "keel_kv holds an unreadable products value: not an object"
        );
        let error = parse(&json!({ "demo": { "by": "install" } })).unwrap_err();
        assert!(error.to_string().contains("enabledAt"), "{error}");
        let error = parse(&json!({ "demo": { "enabledAt": "yesterday", "by": "x" } })).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("keel_kv holds an unreadable products value: "),
            "{error}"
        );
    }

    #[test]
    fn a_record_serialises_with_camel_case_members_and_an_rfc3339_time() {
        let record = Enabled {
            enabled_at: "2026-09-12T10:00:00Z".parse().unwrap(),
            by: "alice".to_owned(),
        };
        assert_eq!(
            serde_json::to_value(&record).unwrap(),
            json!({ "enabledAt": "2026-09-12T10:00:00Z", "by": "alice" })
        );
    }
}
