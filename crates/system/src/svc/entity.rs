pub mod service {
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "svc_service")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub kind: String,
        pub external_id: String,
        pub account_id: String,
        pub status: String,
        pub params_json: Json,
        pub state_json: Json,
        pub schema_version: i32,
        pub peer_id: Option<String>,
        pub retain_until: Option<DateTimeUtc>,
        pub suspended_account: bool,
        pub created_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}
