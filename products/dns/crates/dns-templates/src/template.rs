use keel::sdk::Owned;
use keel::sdk::schemars::JsonSchema;
use keel::sdk::sea_orm;
use keel::sdk::sea_orm::FromJsonQueryResult;
use keel::sdk::sea_orm::entity::prelude::*;
use keel::sdk::serde::{Deserialize, Serialize};
use keel::sdk::serde_json;

use crate::record::TemplateRecord;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, FromJsonQueryResult)]
#[serde(crate = "keel::sdk::serde", transparent)]
#[schemars(crate = "keel::sdk::schemars")]
pub(crate) struct Records(pub(crate) Vec<TemplateRecord>);

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", rename_all = "camelCase")]
#[schemars(
    crate = "keel::sdk::schemars",
    rename = "Template",
    rename_all = "camelCase"
)]
#[sea_orm(table_name = "dns_template")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub account_id: String,
    pub name: String,
    #[sea_orm(column_type = "JsonBinary")]
    pub records: Records,
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
