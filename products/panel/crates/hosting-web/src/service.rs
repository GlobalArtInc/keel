use keel::sdk::sea_orm;
use keel::sdk::sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "hosting_site_service")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub service_id: String,
    pub account_id: String,
    pub seq: i64,
    pub site_limit: Option<i32>,
    pub held: bool,
    pub closed: bool,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
