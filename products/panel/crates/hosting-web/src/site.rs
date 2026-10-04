use keel::sdk::Owned;
use keel::sdk::schemars::JsonSchema;
use keel::sdk::sea_orm;
use keel::sdk::sea_orm::FromJsonQueryResult;
use keel::sdk::sea_orm::entity::prelude::*;
use keel::sdk::serde::{Deserialize, Serialize};
use keel::sdk::serde_json;

pub(crate) const STATE_PENDING: &str = "pending";
pub(crate) const STATE_ACTIVE: &str = "active";
pub(crate) const STATE_ERROR: &str = "error";

#[derive(
    Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, FromJsonQueryResult,
)]
#[serde(crate = "keel::sdk::serde", transparent)]
#[schemars(crate = "keel::sdk::schemars")]
pub(crate) struct Aliases(pub(crate) Vec<String>);

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", rename_all = "camelCase")]
#[schemars(
    crate = "keel::sdk::schemars",
    rename = "HostingSite",
    rename_all = "camelCase"
)]
#[sea_orm(table_name = "hosting_site")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub account_id: String,
    pub user_id: String,
    pub domain: String,
    #[sea_orm(column_type = "JsonBinary")]
    pub aliases: Aliases,
    pub root: String,
    pub php: Option<String>,
    #[serde(skip)]
    pub php_applied: Option<String>,
    pub suspended: bool,
    #[serde(skip)]
    pub server_config: String,
    #[serde(skip)]
    pub pool_config: String,
    pub state: String,
    pub error: Option<String>,
    pub zone_error: Option<String>,
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
