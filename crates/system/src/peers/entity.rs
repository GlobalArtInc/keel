pub mod product {
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "peer_product")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub instance_id: String,
        pub products_json: Json,
        pub keel_version: String,
        pub protocol: i32,
        pub url: Option<String>,
        pub public_key: String,
        pub inbound_token_hash: String,
        pub inbound_scopes_json: Json,
        pub outbound_token_enc: String,
        pub webhook_secret_enc: Option<String>,
        pub status: String,
        pub last_seen_at: Option<DateTimeUtc>,
        pub created_at: DateTimeUtc,
        pub principal_id: Option<String>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod pair_code {
    use keel_sdk::sea_orm;
    use keel_sdk::sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "peer_pair_code")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub code_hash: String,
        pub scopes_json: Json,
        pub expires_at: DateTimeUtc,
        pub created_by: String,
        pub created_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}
