use keel::sdk::Owned;
use keel::sdk::schemars::JsonSchema;
use keel::sdk::sea_orm;
use keel::sdk::sea_orm::entity::prelude::*;
use keel::sdk::serde::{Deserialize, Serialize};

pub(crate) const KIND_PRIMARY: &str = "primary";
pub(crate) const STATE_PENDING: &str = "pending";
pub(crate) const STATE_ACTIVE: &str = "active";
pub(crate) const STATE_ERROR: &str = "error";

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", rename_all = "camelCase")]
#[schemars(
    crate = "keel::sdk::schemars",
    rename = "Zone",
    rename_all = "camelCase"
)]
#[sea_orm(table_name = "dns_zone")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub account_id: String,
    pub name: String,
    pub kind: String,
    pub serial: i64,
    pub refresh: i32,
    pub retry: i32,
    pub expire: i32,
    pub minimum: i32,
    pub template: Option<String>,
    pub state: String,
    pub error: Option<String>,
    pub served: bool,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

impl Owned for Entity {
    fn account_column() -> Self::Column {
        Column::AccountId
    }
}

impl Model {
    pub(crate) fn file(&self) -> String {
        file_of(&self.name)
    }

    pub(crate) fn include(&self) -> String {
        include_of(&self.name)
    }
}

pub(crate) fn file_of(name: &str) -> String {
    format!("{}{name}.zone", crate::DROP_IN_PREFIX)
}

pub(crate) fn include_of(name: &str) -> String {
    format!("{}{name}.conf", crate::DROP_IN_PREFIX)
}
