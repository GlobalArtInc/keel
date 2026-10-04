use keel::sdk::sea_orm;
use keel::sdk::sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "dns_account")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub account_id: String,
    pub template: Option<String>,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
