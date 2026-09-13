use keel::sdk::Owned;
use keel::sdk::schemars::JsonSchema;
use keel::sdk::sea_orm;
use keel::sdk::sea_orm::entity::prelude::*;
use keel::sdk::serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", rename_all = "camelCase")]
#[schemars(
    crate = "keel::sdk::schemars",
    rename = "Note",
    rename_all = "camelCase"
)]
#[sea_orm(table_name = "note_item")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub account_id: String,
    pub title: String,
    pub body: String,
    pub kind: String,
    pub tags: Vec<String>,
    pub due: Option<Date>,
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
