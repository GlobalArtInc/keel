use keel::sdk::sea_orm;
use keel::sdk::sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "hosting_database_grant")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub database_user_id: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub database_id: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
