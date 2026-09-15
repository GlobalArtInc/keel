pub mod deliveries;
pub mod subscriptions;

use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::EntityTrait;

use super::entity::subscription;

pub const PERMISSION_MANAGE: &str = "webhook.manage";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Pick {
    pub id: Id,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub struct Subscription {
    pub id: String,
    pub name: String,
    pub url: String,
    pub events: Vec<String>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
    pub updated_at: DateTime<Utc>,
    #[schemars(extend("x-keel" = json!({ "level": "reseller" })))]
    pub account_id: String,
    #[schemars(extend("x-keel" = json!({ "level": "reseller" })))]
    pub principal_id: String,
}

pub fn subscription_of(row: subscription::Model) -> Subscription {
    Subscription {
        id: row.id,
        name: row.name,
        url: row.url,
        events: row.events,
        enabled: row.enabled,
        created_at: row.created_at,
        created_by: row.created_by,
        updated_at: row.updated_at,
        account_id: row.account_id,
        principal_id: row.principal_id,
    }
}

pub async fn load_subscription(db: &Db, id: Id) -> Result<subscription::Model> {
    subscription::Entity::find_by_id(id.to_string())
        .one(db)
        .await?
        .ok_or_else(|| KeelError::not_found("keel.error.not_found"))
}
