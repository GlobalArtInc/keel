use keel::sdk::Owned;
use keel::sdk::schemars::JsonSchema;
use keel::sdk::sea_orm;
use keel::sdk::sea_orm::entity::prelude::*;
use keel::sdk::serde::{Deserialize, Serialize};

pub(crate) const STATE_PENDING: &str = "pending";
pub(crate) const STATE_ACTIVE: &str = "active";
pub(crate) const STATE_SUSPENDED: &str = "suspended";
pub(crate) const STATE_ERROR: &str = "error";

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", rename_all = "camelCase")]
#[schemars(
    crate = "keel::sdk::schemars",
    rename = "HostingUser",
    rename_all = "camelCase"
)]
#[sea_orm(table_name = "hosting_user")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub account_id: String,
    pub name: String,
    pub uid: Option<i64>,
    pub home: Option<String>,
    pub shell: bool,
    pub state: String,
    pub error: Option<String>,
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
