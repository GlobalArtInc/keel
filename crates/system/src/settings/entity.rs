pub mod value {
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "settings_value")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub scope_id: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub key: String,
        #[sea_orm(column_type = "JsonBinary")]
        pub value: Json,
        pub updated_at: DateTimeUtc,
        pub updated_by: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}
