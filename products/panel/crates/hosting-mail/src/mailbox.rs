use keel::sdk::Owned;
use keel::sdk::schemars::JsonSchema;
use keel::sdk::sea_orm;
use keel::sdk::sea_orm::FromJsonQueryResult;
use keel::sdk::sea_orm::entity::prelude::*;
use keel::sdk::serde::{Deserialize, Serialize};
use keel::sdk::serde_json;

#[derive(
    Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, FromJsonQueryResult,
)]
#[serde(crate = "keel::sdk::serde", transparent)]
#[schemars(crate = "keel::sdk::schemars")]
pub(crate) struct Names(pub(crate) Vec<String>);

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", rename_all = "camelCase")]
#[schemars(
    crate = "keel::sdk::schemars",
    rename = "HostingMailbox",
    rename_all = "camelCase"
)]
#[sea_orm(table_name = "hosting_mailbox")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub account_id: String,
    pub domain_id: String,
    pub local: String,
    pub address: String,
    #[serde(skip)]
    pub password_hash: String,
    pub quota: i32,
    #[sea_orm(column_type = "JsonBinary")]
    pub aliases: Names,
    #[sea_orm(column_type = "JsonBinary")]
    pub forwards: Names,
    pub suspended: bool,
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
